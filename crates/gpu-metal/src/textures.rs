//! Off-screen texture upload preparation and its explicit ownership boundary.
//!
//! The preparation worker owns private texture creation and CPU copies. It closes immutable batches;
//! the unique [`TextureOwner`] is the only handle allowed to submit them. A completion is the proof
//! that releases staging storage. The texture-byte budget charges measured `allocatedSize`, with one
//! serialized not-yet-measured allocation and driver-private memory outside the bound.

use std::{
    cell::Cell,
    collections::VecDeque,
    ffi::{c_char, c_void},
    marker::PhantomData,
    panic::{AssertUnwindSafe, catch_unwind},
    ptr::NonNull,
    sync::{
        Arc, Condvar, Mutex, MutexGuard,
        atomic::{AtomicI32, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{
    Error, message,
    quad::{MAX_IMAGE_BYTES, MAX_IMAGE_SIDE, PixelSource},
    unwind,
};

unsafe extern "C" {
    fn ck_mc_context_new(error: *mut c_char, capacity: usize) -> *mut c_void;
    fn ck_mc_context_free(context: *mut c_void);
    fn ck_mc_context_device_id(context: *mut c_void) -> i64;
    fn ck_mc_context_queue_id(context: *mut c_void) -> i64;
    fn ck_mc_live_objects(
        contexts: *mut u64,
        textures: *mut u64,
        batches: *mut u64,
        frame_readers: *mut u64,
        residency_sets: *mut u64,
        residency_members: *mut u64,
        last_residency_remove_order: *mut u64,
        last_texture_release_order: *mut u64,
    );
    fn ck_mc_texture_estimate(context: *mut c_void, width: u32, height: u32) -> u64;
    fn ck_mc_texture_free(texture: *mut c_void);
    fn ck_mc_texture_metal(texture: *const c_void) -> *mut c_void;
    fn ck_mc_texture_resource_id(texture: *mut c_void) -> u64;
    fn ck_mc_texture_read(
        texture: *mut c_void,
        pixels: *mut u8,
        length: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn ck_mc_measure_shared(
        context: *mut c_void,
        width: u32,
        height: u32,
        count: usize,
        images: *const u8,
        image_length: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn ck_mc_batch_new(
        context: *mut c_void,
        need: u64,
        nil_staging: i32,
        nil_command: i32,
        nil_encoder: i32,
        allocated: *mut u64,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn ck_mc_batch_texture(
        batch: *mut c_void,
        width: u32,
        height: u32,
        fail_allocation: i32,
        allocated: *mut u64,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn ck_mc_batch_discard_texture(batch: *mut c_void, texture: *mut c_void);
    fn ck_mc_batch_copy(
        batch: *mut c_void,
        texture: *mut c_void,
        width: u32,
        height: u32,
        offset: u64,
        pixels: *const u8,
        length: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn ck_mc_batch_close(batch: *mut c_void, error: *mut c_char, capacity: usize) -> i32;
    fn ck_mc_batch_commit(
        batch: *mut c_void,
        callback: Option<extern "C" fn(*mut c_void)>,
        userdata: *mut c_void,
        fail_commit: i32,
        error_completion: i32,
        completion_delay_ms: u32,
        completion_gate: *const AtomicI32,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn ck_mc_command_token_new(error: *mut c_char, capacity: usize) -> *mut c_void;
    fn ck_mc_token_done(token: *mut c_void) -> i32;
    fn ck_mc_token_status(token: *mut c_void, error: *mut c_char, capacity: usize) -> i32;
    fn ck_mc_token_free(token: *mut c_void);
    fn ck_mc_batch_free(batch: *mut c_void);
    fn ck_mc_frame_reader_new(
        context: *mut c_void,
        source: *mut c_void,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn ck_mc_frame_reader_prepare(
        reader: *mut c_void,
        completion_delay_ms: u32,
        error_completion: i32,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn ck_mc_frame_reader_commit(reader: *mut c_void, error: *mut c_char, capacity: usize) -> i32;
    fn ck_mc_frame_reader_read(
        reader: *mut c_void,
        pixels: *mut u8,
        length: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    fn ck_mc_frame_reader_free(reader: *mut c_void);
}

/// Shared Metal device and upload queue. Its native retain outlives every texture made from it.
pub struct MetalContext(Arc<ContextInner>);

struct ContextInner {
    handle: NonNull<c_void>,
}

// SAFETY: Metal device and command queue creation are thread-safe; this type exposes only those
// operations and keeps its opaque native retain alive through Arc.
unsafe impl Send for ContextInner {}
// SAFETY: as above; no owner-thread presenter API is exposed here.
unsafe impl Sync for ContextInner {}

impl Drop for ContextInner {
    fn drop(&mut self) {
        // SAFETY: this Arc owns the one retained native context, released after its final texture.
        unsafe { ck_mc_context_free(self.handle.as_ptr()) };
    }
}

impl Clone for MetalContext {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}

impl MetalContext {
    /// Creates the system Metal device and its command queue.
    ///
    /// # Errors
    ///
    /// Returns the native device or queue creation error.
    pub fn new() -> Result<Arc<Self>, Error> {
        let mut error = [0_u8; 1024];
        // SAFETY: the native call writes no more than the supplied buffer capacity.
        let handle = unsafe { ck_mc_context_new(error.as_mut_ptr().cast(), error.len()) };
        let handle = NonNull::new(handle).ok_or_else(|| message(&error))?;
        Ok(Arc::new(Self(Arc::new(ContextInner { handle }))))
    }

    /// The native context handle, for the presenter's native setup (it takes its device and queue from it).
    pub(crate) fn raw(&self) -> *mut c_void {
        self.0.handle.as_ptr()
    }

    /// Raw context for test-only native probes linked from the test support archive.
    #[doc(hidden)]
    #[must_use]
    pub fn test_raw_handle(&self) -> *mut c_void {
        self.0.handle.as_ptr()
    }

    fn estimate(&self, width: u32, height: u32) -> u64 {
        // SAFETY: context is retained by self and dimensions were validated by the caller.
        unsafe { ck_mc_texture_estimate(self.0.handle.as_ptr(), width, height) }
    }

    /// Returns native live wrapper counts for lifecycle tests.
    #[doc(hidden)]
    #[must_use]
    pub fn native_live_objects(&self) -> NativeLiveObjects {
        let (
            mut contexts,
            mut textures,
            mut batches,
            mut frame_readers,
            mut residency_sets,
            mut residency_members,
            mut last_residency_remove_order,
            mut last_texture_release_order,
        ) = (0, 0, 0, 0, 0, 0, 0, 0);
        // SAFETY: each output points to a writable local for the synchronous native snapshot.
        unsafe {
            ck_mc_live_objects(
                &raw mut contexts,
                &raw mut textures,
                &raw mut batches,
                &raw mut frame_readers,
                &raw mut residency_sets,
                &raw mut residency_members,
                &raw mut last_residency_remove_order,
                &raw mut last_texture_release_order,
            );
        };
        NativeLiveObjects {
            contexts,
            textures,
            batches,
            frame_readers,
            residency_sets,
            residency_members,
            last_residency_remove_order,
            last_texture_release_order,
        }
    }

    /// The identity (object address) of the device, for the tests: the number the presenter records for the device it
    /// took from this context.
    #[doc(hidden)]
    #[must_use]
    pub fn device_identity(&self) -> i64 {
        // SAFETY: the context is retained by `self`; the call reads one object address.
        unsafe { ck_mc_context_device_id(self.0.handle.as_ptr()) }
    }

    /// The identity (object address) of the command queue, for the tests: the number the presenter records for the
    /// queue it took from this context.
    #[doc(hidden)]
    #[must_use]
    pub fn queue_identity(&self) -> i64 {
        // SAFETY: the context is retained by `self`; the call reads one object address.
        unsafe { ck_mc_context_queue_id(self.0.handle.as_ptr()) }
    }

    /// The device's heap size-and-align estimate for an RGBA8 private texture.
    #[doc(hidden)]
    #[must_use]
    pub fn heap_texture_size_estimate(&self, width: u32, height: u32) -> u64 {
        self.estimate(width, height)
    }

    /// Measures the test-only shared-storage `replaceRegion` alternative without a command buffer.
    #[doc(hidden)]
    ///
    /// # Errors
    ///
    /// Returns invalid input or native allocation failure.
    pub fn measure_shared_replacement(
        &self,
        width: u32,
        height: u32,
        count: usize,
        images: &[u8],
    ) -> Result<Duration, Error> {
        let mut error = [0_u8; 1024];
        let started = Instant::now();
        // SAFETY: context and image slice remain live for the synchronous test-only native measurement.
        let result = unsafe {
            ck_mc_measure_shared(
                self.0.handle.as_ptr(),
                width,
                height,
                count,
                images.as_ptr(),
                images.len(),
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        if result == 0 {
            Ok(started.elapsed())
        } else {
            Err(message(&error))
        }
    }

    fn batch(
        &self,
        need: u64,
        hooks: &TestHooks,
    ) -> Result<(NativeBatch, u64), (Option<NativeBatch>, u64, Error)> {
        let mut allocated = 0;
        let mut error = [0_u8; 1024];
        // SAFETY: the context retain and output buffer stay live throughout the native allocation.
        let raw = unsafe {
            ck_mc_batch_new(
                self.0.handle.as_ptr(),
                need,
                i32::from(hooks.nil_staging),
                i32::from(hooks.nil_command),
                i32::from(hooks.nil_encoder),
                &raw mut allocated,
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        let Some(handle) = NonNull::new(raw) else {
            return Err((None, allocated, message(&error)));
        };
        let batch = NativeBatch(Arc::new(NativeBatchInner {
            handle,
            context: Arc::clone(&self.0),
        }));
        if error[0] != 0 {
            return Err((Some(batch), allocated, message(&error)));
        }
        Ok((batch, allocated))
    }
}

#[derive(Clone)]
pub(crate) struct NativeTexture(Arc<NativeTextureInner>);

struct NativeTextureInner {
    handle: NonNull<c_void>,
    context: Arc<ContextInner>,
}

// SAFETY: an inner holder is retained in the shared preparation ledger before any user code can
// unwind. Its pointer is only dereferenced by the worker during preparation or by the owner after
// publication/completion; ledger clones only extend lifetime and are never used to access it.
unsafe impl Send for NativeTextureInner {}
// SAFETY: same single-access protocol as Send; shared references are lifetime-only keepalives.
unsafe impl Sync for NativeTextureInner {}

impl NativeTexture {
    pub(crate) fn clone_for_frame(&self) -> Self {
        self.clone()
    }

    pub(crate) fn raw_handle(&self) -> *mut c_void {
        self.0.handle.as_ptr()
    }

    /// The Metal texture (an `id<MTLTexture>`, not retained) the scene presenter binds; valid while this holder lives.
    pub(crate) fn metal(&self) -> *mut c_void {
        // SAFETY: the holder is live and its native texture lives as long as it does; the call reads one pointer.
        unsafe { ck_mc_texture_metal(self.0.handle.as_ptr()) }
    }

    /// The direct Metal resource identifier copied into a bindless table while this holder is retained.
    pub(crate) fn resource_id(&self) -> u64 {
        // SAFETY: this holder owns the native texture for the duration of the synchronous identifier read.
        unsafe { ck_mc_texture_resource_id(self.0.handle.as_ptr()) }
    }

    fn read(&mut self, output: &mut [u8]) -> Result<(), Error> {
        let mut error = [0_u8; 1024];
        // SAFETY: this unique native holder and an exactly sized output buffer remain live for the call.
        let result = unsafe {
            ck_mc_texture_read(
                self.0.handle.as_ptr(),
                output.as_mut_ptr(),
                output.len(),
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(message(&error))
        }
    }
}

impl Drop for NativeTextureInner {
    fn drop(&mut self) {
        // SAFETY: this holder owns exactly one native retain and consumes it once.
        unsafe { ck_mc_texture_free(self.handle.as_ptr()) };
    }
}

#[derive(Clone)]
struct NativeBatch(Arc<NativeBatchInner>);

struct NativeBatchInner {
    handle: NonNull<c_void>,
    context: Arc<ContextInner>,
}

// SAFETY: the worker is the only accessor while preparing; after publication, only the owner
// accesses the closed batch. The State ledger stores clones only to keep the native holder alive.
unsafe impl Send for NativeBatchInner {}
// SAFETY: same exclusive-access protocol; shared references from the ledger are lifetime-only.
unsafe impl Sync for NativeBatchInner {}

impl NativeBatch {
    fn discard_texture(&mut self, texture: &NativeTexture) {
        // SAFETY: the open batch owns this exact texture until it is removed from its array.
        unsafe { ck_mc_batch_discard_texture(self.0.handle.as_ptr(), texture.0.handle.as_ptr()) };
    }

    fn texture(
        &mut self,
        width: u32,
        height: u32,
        fail_allocation: bool,
    ) -> Result<(NativeTexture, u64), Error> {
        let mut allocated = 0;
        let mut error = [0_u8; 1024];
        // SAFETY: the batch is open, its output buffer is writable, and the native call is synchronous.
        let raw = unsafe {
            ck_mc_batch_texture(
                self.0.handle.as_ptr(),
                width,
                height,
                i32::from(fail_allocation),
                &raw mut allocated,
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        let handle = NonNull::new(raw).ok_or_else(|| message(&error))?;
        Ok((
            NativeTexture(Arc::new(NativeTextureInner {
                handle,
                context: Arc::clone(&self.0.context),
            })),
            allocated,
        ))
    }

    fn copy(
        &mut self,
        texture: &NativeTexture,
        width: u32,
        height: u32,
        offset: u64,
        pixels: &[u8],
    ) -> Result<(), Error> {
        let mut error = [0_u8; 1024];
        // SAFETY: native batch, texture and immutable source bytes remain live for the copy.
        let result = unsafe {
            ck_mc_batch_copy(
                self.0.handle.as_ptr(),
                texture.0.handle.as_ptr(),
                width,
                height,
                offset,
                pixels.as_ptr(),
                pixels.len(),
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(message(&error))
        }
    }

    fn close(&mut self) -> Result<(), Error> {
        let mut error = [0_u8; 1024];
        // SAFETY: this is the unique owner of an open native envelope.
        let result = unsafe {
            ck_mc_batch_close(
                self.0.handle.as_ptr(),
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        if result == 0 {
            Ok(())
        } else {
            Err(message(&error))
        }
    }

    fn commit(
        self,
        hook: Box<CompletionHook>,
        test_hooks: &TestHooks,
    ) -> Result<(NativeBatch, NativeToken), (NativeBatch, Error)> {
        let mut error = [0_u8; 1024];
        let userdata = Box::into_raw(hook).cast::<c_void>();
        // A test's completion gate: one share of its `Arc` is deliberately leaked per gated commit, so the pointer the
        // native completion handler reads stays valid however long after the store the handler runs.
        let gate = test_hooks
            .completion_gate
            .as_ref()
            .map_or(std::ptr::null(), |gate| Arc::into_raw(Arc::clone(gate)));
        // SAFETY: the closed envelope is unique; the callback takes ownership of `userdata` exactly once; `gate` is null
        // or a leaked, forever-valid `AtomicI32`.
        let raw = unsafe {
            ck_mc_batch_commit(
                self.0.handle.as_ptr(),
                Some(completion_callback),
                userdata,
                i32::from(test_hooks.fail_commit),
                i32::from(test_hooks.error_completion),
                u32::try_from(test_hooks.completion_delay.as_millis()).unwrap_or(u32::MAX),
                gate,
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        let Some(token) = NonNull::new(raw) else {
            // SAFETY: a null token means native submission registered no callback, so reclaim its box here.
            unsafe { drop(Box::from_raw(userdata.cast::<CompletionHook>())) };
            return Err((self, message(&error)));
        };
        Ok((self, NativeToken { handle: token }))
    }
}

impl Drop for NativeBatchInner {
    fn drop(&mut self) {
        // SAFETY: consumes this one opaque batch holder after completion or proof it was never committed.
        unsafe { ck_mc_batch_free(self.handle.as_ptr()) };
    }
}

struct NativeToken {
    handle: NonNull<c_void>,
}

/// Native wrapper counts used to verify `Proven` release and `Unproven` retention.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct NativeLiveObjects {
    /// Live shared device and queue wrappers.
    pub contexts: u64,
    /// Live private texture wrappers.
    pub textures: u64,
    /// Live upload batch wrappers, including staging and command objects.
    pub batches: u64,
    /// Live test frame-reader command wrappers.
    pub frame_readers: u64,
    /// Live queue-attached bindless residency sets.
    pub residency_sets: u64,
    /// Current texture allocations in all live residency sets.
    pub residency_members: u64,
    /// Monotonic order of the most recent native residency removal.
    pub last_residency_remove_order: u64,
    /// Monotonic order of the most recent native texture release.
    pub last_texture_release_order: u64,
}

// SAFETY: token fields are written/read atomically by the native completion handler and Rust owner.
unsafe impl Send for NativeToken {}

impl NativeToken {
    fn done(&self) -> bool {
        // SAFETY: token remains live until this wrapper is dropped.
        unsafe { ck_mc_token_done(self.handle.as_ptr()) != 0 }
    }

    fn status(&self) -> Result<(), String> {
        let mut error = [0_u8; 1024];
        // SAFETY: token and output buffer remain live for the synchronous status read.
        let result = unsafe {
            ck_mc_token_status(self.handle.as_ptr(), error.as_mut_ptr().cast(), error.len())
        };
        match result {
            0 => Ok(()),
            1 => Err(message(&error).to_string()),
            _ => Err("completion token is not done".to_owned()),
        }
    }
}

impl Drop for NativeToken {
    fn drop(&mut self) {
        // SAFETY: completion was observed before the token is released.
        unsafe { ck_mc_token_free(self.handle.as_ptr()) };
    }
}

/// A baseline item (milestone 5, issue 5b-1): a texture the presenter itself uploads once at setup, outside the store's
/// items, charges and prepared window (the checker). It uses a staging buffer and one command buffer on the shared
/// context's queue, with its own completion token; the completion handler only records into that token and calls `wake`.
/// Nothing waits for it.
///
/// **Retention.** The native completion handler holds the raw token until it has run, so the token, the staging buffer and
/// the texture must never be freed under a pending completion, however the owner ends: dropping a `BaselineUpload` whose
/// completion has not arrived (an unwind out of the render thread, a setup that unwinds) **retains the whole graph to the
/// process end**, as the ledger does for an unproven graph, and never waits in `Drop`. Only a completed upload is freed,
/// token first, staging next, the texture last.
pub(crate) struct BaselineUpload {
    parts: Option<BaselineParts>,
    texture_bytes: u64,
    staging_bytes: u64,
}

struct BaselineParts {
    texture: NativeTexture,
    batch: NativeBatch,
    token: NativeToken,
}

impl BaselineParts {
    /// Frees the graph in dependency order; only after the completion is proven.
    fn free(self) {
        let Self {
            texture,
            batch,
            token,
        } = self;
        drop(token);
        drop(batch);
        drop(texture);
    }
}

impl Drop for BaselineUpload {
    fn drop(&mut self) {
        if let Some(parts) = self.parts.take() {
            if parts.token.done() {
                parts.free();
            } else {
                // The completion handler may still write the token: keep the token, the staging buffer, the texture and
                // (through them) the context alive for the life of the process.
                std::mem::forget(parts);
            }
        }
    }
}

impl BaselineUpload {
    /// Creates the texture (private storage, shader read: the same descriptor options as store textures), fills a
    /// staging buffer with `pixels`, and commits the blit. `hooks` are the test fault switches of the upload.
    ///
    /// # Errors
    ///
    /// The native failure (no staging buffer, command buffer, encoder or texture, or a refused commit). Nothing was
    /// submitted, so what was made is released here.
    pub(crate) fn start(
        context: &MetalContext,
        width: u32,
        height: u32,
        pixels: &[u8],
        wake: Option<Arc<dyn Fn() + Send + Sync>>,
        hooks: &TestHooks,
    ) -> Result<Self, Error> {
        let need = align_up(pixels.len() as u64, 256)
            .ok_or_else(|| Error("the baseline texture's staging size overflows".to_owned()))?;
        let (mut batch, staging_bytes) =
            context.batch(need, hooks).map_err(|(_, _, error)| error)?;
        let (texture, texture_bytes) =
            batch.texture(width, height, hooks.fail_texture_allocation_at.is_some())?;
        batch.copy(&texture, width, height, 0, pixels)?;
        batch.close()?;
        let hook = Box::new(CompletionHook {
            wake,
            stats: Arc::new(Mutex::new(TextureStats::default())),
        });
        let (batch, token) = batch.commit(hook, hooks).map_err(|(_, error)| error)?;
        Ok(Self {
            parts: Some(BaselineParts {
                texture,
                batch,
                token,
            }),
            texture_bytes,
            staging_bytes,
        })
    }

    fn parts(&self) -> &BaselineParts {
        self.parts
            .as_ref()
            .expect("an upload keeps its parts until it is released or dropped")
    }

    /// Whether the completion has arrived (one read, never waiting).
    pub(crate) fn is_done(&self) -> bool {
        self.parts().token.done()
    }

    /// The completion's outcome once it arrived.
    pub(crate) fn status(&self) -> Result<(), String> {
        self.parts().token.status()
    }

    /// The Metal texture to bind; valid until [`release`](Self::release).
    pub(crate) fn metal(&self) -> *mut c_void {
        self.parts().texture.metal()
    }

    /// The direct resource identifier of the presenter-owned checker texture.
    pub(crate) fn resource_id(&self) -> u64 {
        self.parts().texture.resource_id()
    }

    /// The measured size of the texture, in bytes.
    pub(crate) fn texture_bytes(&self) -> u64 {
        self.texture_bytes
    }

    /// The measured size of the staging buffer, in bytes.
    pub(crate) fn staging_bytes(&self) -> u64 {
        self.staging_bytes
    }

    /// Releases the upload after its completion was proven: the token, then the staging buffer, the texture last. An upload
    /// whose completion has not arrived is retained, as in `Drop`.
    pub(crate) fn release(self) {
        drop(self);
    }
}

/// A test reader command buffer that samples one exact native revision into shared readback storage.
#[doc(hidden)]
pub struct FrameCommandBuffer {
    handle: NonNull<c_void>,
    completion: Option<FrameCompletion>,
    width: u32,
    height: u32,
    delay: Duration,
    error_completion: bool,
    committed: bool,
    _not_sync: PhantomData<Cell<()>>,
}

/// Cloneable completion observation used by the frame ledger.
#[derive(Clone)]
pub(crate) struct FrameCompletion {
    token: Arc<Mutex<NativeToken>>,
    committed: Arc<std::sync::atomic::AtomicBool>,
}

impl FrameCompletion {
    pub(crate) fn is_done(&self) -> bool {
        lock(&self.token).done()
    }

    pub(crate) fn status(&self) -> Result<(), String> {
        lock(&self.token).status()
    }

    pub(crate) fn is_committed(&self) -> bool {
        self.committed.load(Ordering::Acquire)
    }
}

/// A completion token for a command buffer the presenter owns: allocated here, before the commit, and completed by
/// the presenter's own completion handler (before its own token). Returns the completion the ledger observes and
/// the native token the handler captures. The token is freed with the completion, only after it was observed
/// done (or retained to the process end on an unproven end).
pub(crate) fn new_command_token() -> Result<(FrameCompletion, *mut c_void), Error> {
    let mut error = [0_u8; 256];
    // SAFETY: the native call writes at most `error.len()` bytes into the buffer and returns a fresh token.
    let raw = unsafe { ck_mc_command_token_new(error.as_mut_ptr().cast(), error.len()) };
    let handle = NonNull::new(raw).ok_or_else(|| message(&error))?;
    let completion = FrameCompletion {
        token: Arc::new(Mutex::new(NativeToken { handle })),
        committed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    Ok((completion, raw))
}

impl FrameCompletion {
    /// Records that the presenter's native commit returned.
    pub(crate) fn mark_committed(&self) {
        self.committed.store(true, Ordering::Release);
    }
}

impl FrameCommandBuffer {
    pub(crate) fn new(
        texture: &NativeTexture,
        width: u32,
        height: u32,
        delay: Duration,
        error_completion: bool,
    ) -> Result<Self, Error> {
        let mut error = [0_u8; 1024];
        // SAFETY: the retained texture keeps its context alive; native creation retains the
        // source texture in the command buffer before this call returns.
        let raw = unsafe {
            ck_mc_frame_reader_new(
                texture.0.context.handle.as_ptr(),
                texture.raw_handle(),
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        let handle = NonNull::new(raw).ok_or_else(|| message(&error))?;
        Ok(Self {
            handle,
            completion: None,
            width,
            height,
            delay,
            error_completion,
            committed: false,
            _not_sync: PhantomData,
        })
    }

    pub(crate) fn prepare(&mut self) -> Result<FrameCompletion, Error> {
        let mut error = [0_u8; 1024];
        // SAFETY: the native reader is live and uniquely accessed by its owner thread.
        let raw = unsafe {
            ck_mc_frame_reader_prepare(
                self.handle.as_ptr(),
                u32::try_from(self.delay.as_millis()).unwrap_or(u32::MAX),
                i32::from(self.error_completion),
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        let token = NonNull::new(raw).ok_or_else(|| message(&error))?;
        let completion = FrameCompletion {
            token: Arc::new(Mutex::new(NativeToken { handle: token })),
            committed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        };
        self.completion = Some(completion.clone());
        Ok(completion)
    }

    /// Commits after the ledger has attached the completion token.
    ///
    /// # Errors
    ///
    /// Returns a bounded native submission failure.
    pub fn commit(&mut self) -> Result<(), Error> {
        let mut error = [0_u8; 1024];
        // SAFETY: the prepared native reader is uniquely accessed by its owner thread.
        let result = unsafe {
            ck_mc_frame_reader_commit(self.handle.as_ptr(), error.as_mut_ptr().cast(), error.len())
        };
        if result == 0 {
            self.committed = true;
            if let Some(completion) = &self.completion {
                completion.committed.store(true, Ordering::Release);
            }
            Ok(())
        } else {
            Err(message(&error))
        }
    }

    /// Reads pixels after the attached completion token has been observed done.
    ///
    /// # Errors
    ///
    /// Returns if the frame is incomplete or native readback fails.
    pub fn readback(&self) -> Result<Vec<u8>, Error> {
        if !self.committed
            || !self
                .completion
                .as_ref()
                .is_some_and(FrameCompletion::is_done)
        {
            return Err(Error(
                "frame readback requested before completion".to_owned(),
            ));
        }
        let length = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| Error("frame readback length overflow".to_owned()))?;
        let mut pixels = vec![0; length];
        let mut error = [0_u8; 1024];
        // SAFETY: completion is observed, output has the exact texture size, and reader is live.
        let result = unsafe {
            ck_mc_frame_reader_read(
                self.handle.as_ptr(),
                pixels.as_mut_ptr(),
                pixels.len(),
                error.as_mut_ptr().cast(),
                error.len(),
            )
        };
        if result == 0 {
            Ok(pixels)
        } else {
            Err(message(&error))
        }
    }
}

impl Drop for FrameCommandBuffer {
    fn drop(&mut self) {
        // SAFETY: this consumes the one reader wrapper. Metal retains committed commands and
        // their resources; the ledger independently retains completion tokens and revision pins.
        unsafe { ck_mc_frame_reader_free(self.handle.as_ptr()) };
    }
}

/// Opaque source identity for one upload request.
pub struct UploadItem<T> {
    /// Caller-owned tag returned with completion; the store never interprets it.
    pub tag: T,
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Immutable premultiplied RGBA8 source, rows top first.
    pub pixels: Arc<dyn PixelSource>,
}

impl<T> std::fmt::Debug for UploadItem<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UploadItem")
            .field("tag", &"<opaque>")
            .field("width", &self.width)
            .field("height", &self.height)
            .finish_non_exhaustive()
    }
}

/// Validated upload caps. Defaults bound the two native leases and their measured memory charges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    missing_docs,
    reason = "field contracts are summarized by TextureLimits and validate"
)]
pub struct TextureLimits {
    pub max_prepared_images: usize,
    pub max_upload_images_per_batch: usize,
    pub max_upload_bytes_per_batch: u64,
    pub staging_bytes: u64,
    pub texture_bytes: u64,
}

impl Default for TextureLimits {
    fn default() -> Self {
        Self {
            max_prepared_images: 256,
            max_upload_images_per_batch: 128,
            max_upload_bytes_per_batch: 16 * 1024 * 1024,
            staging_bytes: 128 * 1024 * 1024,
            texture_bytes: 2 * 1024 * 1024 * 1024,
        }
    }
}

impl TextureLimits {
    /// Validates all ranges and cross-field constraints.
    ///
    /// # Errors
    ///
    /// Returns the invalid field and its allowed range or relationship.
    pub fn validate(self) -> Result<Self, Error> {
        let range = |name: &str, value: u64, minimum: u64, maximum: u64| -> Result<(), Error> {
            if !(minimum..=maximum).contains(&value) {
                return Err(Error(format!(
                    "{name}={value} is outside {minimum}..={maximum}"
                )));
            }
            Ok(())
        };
        range(
            "max_prepared_images",
            self.max_prepared_images as u64,
            2,
            4096,
        )?;
        range(
            "max_upload_images_per_batch",
            self.max_upload_images_per_batch as u64,
            1,
            2048,
        )?;
        range(
            "max_upload_bytes_per_batch",
            self.max_upload_bytes_per_batch,
            1 << 20,
            256 << 20,
        )?;
        range("staging_bytes", self.staging_bytes, 128 << 20, 4 << 30)?;
        range("texture_bytes", self.texture_bytes, 64 << 20, 64 << 30)?;
        if self.max_prepared_images < self.max_upload_images_per_batch.saturating_mul(2) {
            return Err(Error(format!(
                "max_prepared_images={} must be at least twice max_upload_images_per_batch={}",
                self.max_prepared_images, self.max_upload_images_per_batch
            )));
        }
        if self.max_upload_bytes_per_batch > self.staging_bytes / 2 {
            return Err(Error(format!(
                "max_upload_bytes_per_batch={} exceeds staging_bytes/2={}",
                self.max_upload_bytes_per_batch,
                self.staging_bytes / 2
            )));
        }
        Ok(self)
    }
}

/// Bounded aggregate diagnostics for texture and staging reservations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[expect(
    missing_docs,
    reason = "fields are bounded aggregate diagnostics documented by this type"
)]
pub struct TextureStats {
    pub texture_bytes_reserved: u64,
    pub texture_bytes_measured: u64,
    pub texture_bytes_peak: u64,
    pub texture_transient_peak: u64,
    pub live_textures: usize,
    pub smallest_texture: Option<u64>,
    pub largest_texture: Option<u64>,
    pub staging_bytes: u64,
    pub staging_bytes_peak: u64,
    /// Number of batches that had to wait for staging capacity.
    pub staging_waits: u64,
    pub batches_prepared: u64,
    pub batches_submitted: u64,
    pub batches_completed: u64,
    /// Native textures successfully created for uploads over the store's life (cumulative; never decreases, unlike
    /// `live_textures`): one that the measured-size check then discards counts too, a failed allocation does not.
    pub textures_created: u64,
    pub images_per_batch_max: usize,
    pub prepared_images: usize,
    pub prepared_images_peak: usize,
    pub refused_offers: u64,
    pub item_failures: u64,
    pub wake_panics: u64,
}

/// An item returned by non-blocking polling.
#[derive(Debug)]
#[expect(
    missing_docs,
    reason = "variants and payloads are summarized by the completion contract"
)]
pub enum Completed<T> {
    Ready(PreparedTexture<T>),
    Failed { tag: T, reason: String },
}

/// A completed private texture. Dropping it releases its texture-byte reservation.
#[expect(
    missing_docs,
    reason = "public payload fields are summarized by the prepared texture contract"
)]
pub struct PreparedTexture<T> {
    pub tag: T,
    pub width: u32,
    pub height: u32,
    pub allocated_size: u64,
    native: Option<NativeTexture>,
    stats: Arc<Mutex<TextureStats>>,
}

impl<T: std::fmt::Debug> std::fmt::Debug for PreparedTexture<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedTexture")
            .field("tag", &self.tag)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("allocated_size", &self.allocated_size)
            .finish_non_exhaustive()
    }
}

impl<T> PreparedTexture<T> {
    pub(crate) fn metal(&self) -> *mut c_void {
        self.native
            .as_ref()
            .map_or(std::ptr::null_mut(), NativeTexture::metal)
    }

    pub(crate) fn resource_id(&self) -> u64 {
        self.native.as_ref().map_or(0, NativeTexture::resource_id)
    }

    pub(crate) fn native_for_frame(&self) -> Option<NativeTexture> {
        self.native.as_ref().map(NativeTexture::clone_for_frame)
    }

    /// Reads back the texture for test and diagnostic use; production submission remains asynchronous.
    #[doc(hidden)]
    ///
    /// # Errors
    ///
    /// Returns a bounded GPU readback failure.
    pub fn readback(&mut self) -> Result<Vec<u8>, Error> {
        let length = (self.width as usize)
            .checked_mul(self.height as usize)
            .and_then(|n| n.checked_mul(4))
            .ok_or_else(|| Error("texture readback length overflow".to_owned()))?;
        let mut output = vec![0; length];
        self.native
            .as_mut()
            .ok_or_else(|| Error("texture was released".to_owned()))?
            .read(&mut output)?;
        Ok(output)
    }
}

impl<T> Drop for PreparedTexture<T> {
    fn drop(&mut self) {
        self.native.take();
        let mut stats = lock(&self.stats);
        stats.texture_bytes_reserved = stats
            .texture_bytes_reserved
            .saturating_sub(self.allocated_size);
        stats.texture_bytes_measured = stats
            .texture_bytes_measured
            .saturating_sub(self.allocated_size);
        stats.live_textures = stats.live_textures.saturating_sub(1);
    }
}

/// An offered item returned unchanged when the prepared window has no room or the store is stopping.
pub struct Refused<I>(pub I);

/// Owner notifications run after the store lock has been released and must not block.
///
/// **Liveness precondition:** configured hooks must not panic. A panic is caught and its payload is disposed,
/// and the state being announced remains stored, but that notification is lost and is not retried. An
/// independent owner call or a later event must observe it. `TextureStats::wake_panics` counts violations.
#[derive(Clone, Default)]
#[expect(
    missing_docs,
    reason = "wake hook roles are named by their fields and type contract"
)]
pub struct TextureWakes {
    pub ready_to_submit: Option<Arc<dyn Fn() + Send + Sync>>,
    pub completed: Option<Arc<dyn Fn() + Send + Sync>>,
    pub room: Option<Arc<dyn Fn() + Send + Sync>>,
}

struct Shared<T> {
    state: Mutex<State<T>>,
    changed: Condvar,
    stats: Arc<Mutex<TextureStats>>,
    wakes: TextureWakes,
    capacity: usize,
    test_hooks: Arc<TestHooks>,
}

struct State<T> {
    waiting: VecDeque<UploadItem<T>>,
    closed: Option<Envelope<T>>,
    submitted: Option<Submitted<T>>,
    immediate: VecDeque<Completed<T>>,
    stopping: bool,
    failed: Option<String>,
    failure_taken: bool,
    occupied: usize,
    worker_finished: bool,
    worker_panic: Option<String>,
    orphaned: Vec<Envelope<T>>,
    preparing_batch: Option<NativeBatch>,
    preparing_textures: Vec<NativeTexture>,
    max: usize,
}

struct ItemResult<T> {
    tag: T,
    width: u32,
    height: u32,
    outcome: ItemOutcome,
}

enum ItemOutcome {
    Ready {
        native: NativeTexture,
        allocated: u64,
    },
    Failed(String),
}

struct Envelope<T> {
    batch: Option<NativeBatch>,
    items: Vec<ItemResult<T>>,
    count: usize,
    staging_charge: u64,
}

struct Submitted<T> {
    envelope: Envelope<T>,
    token: Option<NativeToken>,
}

/// A cloneable non-blocking producer handle.
pub struct UploadPort<T> {
    shared: Arc<Shared<T>>,
}

/// The unique owner-thread submission, polling and shutdown handle. It is `Send`, not `Sync` or `Clone`.
pub struct TextureOwner<T: Copy + Eq + std::hash::Hash + Send + 'static> {
    shared: Arc<Shared<T>>,
    context: Option<Arc<MetalContext>>,
    worker: Option<JoinHandle<()>>,
    owner_id: u64,
    released: bool,
    _not_sync: PhantomData<Cell<()>>,
}

/// A non-destructive end proof, tied to the owner which created it.
#[derive(Clone, Copy)]
pub struct Proof {
    owner_id: u64,
    complete: bool,
}

impl Proof {
    /// Whether this owner observed worker exit and every submitted completion before its deadline.
    #[must_use]
    pub fn is_complete(self) -> bool {
        self.complete
    }
}

/// The outcome of a complete end proof.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    missing_docs,
    reason = "variants describe the only two cleanup outcomes"
)]
pub enum StoreCleanup {
    Proven,
    Unproven { reason: String },
}

static NEXT_OWNER: AtomicU64 = AtomicU64::new(1);

impl<T: Send + 'static> Clone for UploadPort<T> {
    fn clone(&self) -> Self {
        Self {
            shared: Arc::clone(&self.shared),
        }
    }
}

impl<T: Send + 'static> UploadPort<T> {
    /// Number of image slots available in the prepared window.
    #[must_use]
    pub fn room(&self) -> usize {
        lock(&self.shared.state).occupied_room()
    }

    /// Offers without waiting. A refusal returns the exact input item to its caller.
    ///
    /// # Errors
    ///
    /// Returns the unchanged item when the prepared window is full, stopping, or failed.
    pub fn offer(&self, item: UploadItem<T>) -> Result<(), Refused<UploadItem<T>>> {
        let mut state = lock(&self.shared.state);
        let max = self.shared.capacity;
        if state.stopping || state.failed.is_some() || state.occupied >= max {
            drop(state);
            lock(&self.shared.stats).refused_offers += 1;
            return Err(Refused(item));
        }
        state.occupied += 1;
        state.waiting.push_back(item);
        let occupied = state.occupied;
        drop(state);
        let mut counters = lock(&self.shared.stats);
        counters.prepared_images = occupied;
        counters.prepared_images_peak = counters.prepared_images_peak.max(occupied);
        drop(counters);
        self.shared.changed.notify_all();
        Ok(())
    }
}

impl<T> State<T> {
    fn occupied_room(&self) -> usize {
        self.max.saturating_sub(self.occupied)
    }
}

impl<T> TextureOwner<T>
where
    T: Copy + Eq + std::hash::Hash + Send + 'static,
{
    /// Non-blocking submit. Only this owner can commit a closed envelope.
    pub fn submit(&mut self) -> bool {
        let mut state = lock(&self.shared.state);
        if state.failed.is_some() || state.submitted.is_some() {
            return false;
        }
        let Some(mut envelope) = state.closed.take() else {
            return false;
        };
        let had_native_batch = envelope.batch.is_some();
        let token = if let Some(batch) = envelope.batch.take() {
            if let Some(observer) = &self.shared.test_hooks.thread_observer {
                lock(observer).commits.push(thread::current().id());
            }
            let hook = Box::new(CompletionHook {
                wake: self.shared.wakes.completed.clone(),
                stats: Arc::clone(&self.shared.stats),
            });
            match batch.commit(hook, &self.shared.test_hooks) {
                Ok((batch, token)) => {
                    envelope.batch = Some(batch);
                    Some(token)
                }
                Err((batch, error)) => {
                    state.failed.get_or_insert_with(|| error.to_string());
                    state.stopping = true;
                    envelope.batch = Some(batch);
                    let count = envelope.count;
                    for item in &envelope.items {
                        let reason = match &item.outcome {
                            ItemOutcome::Ready { .. } => error.to_string(),
                            ItemOutcome::Failed(reason) => reason.clone(),
                        };
                        state.immediate.push_back(Completed::Failed {
                            tag: item.tag,
                            reason,
                        });
                    }
                    state.occupied = state.occupied.saturating_sub(count);
                    lock(&self.shared.stats).prepared_images = state.occupied;
                    state.orphaned.push(envelope);
                    state.submitted = None;
                    self.shared.changed.notify_all();
                    drop(state);
                    notify_wake(&self.shared, self.shared.wakes.room.as_ref());
                    return false;
                }
            }
        } else {
            None
        };
        if had_native_batch {
            lock(&self.shared.stats).batches_submitted += 1;
        }
        state.submitted = Some(Submitted { envelope, token });
        // The worker waits while a closed envelope is available. Taking that envelope is the
        // event that lets it prepare the next bounded batch, even before this batch completes.
        self.shared.changed.notify_all();
        drop(state);
        // Test only: the owner is paused between this submit and the poll that follows it (the unfavourable
        // schedule a gate must defeat); no lock is held.
        if had_native_batch && !self.shared.test_hooks.submit_pause.is_zero() {
            thread::sleep(self.shared.test_hooks.submit_pause);
        }
        true
    }

    /// Polls completions without waiting and appends all finished item outcomes to `out`.
    pub fn poll(&mut self, out: &mut Vec<Completed<T>>) {
        let mut state = lock(&self.shared.state);
        while let Some(item) = state.immediate.pop_front() {
            out.push(item);
        }
        let done = state
            .submitted
            .as_ref()
            .is_some_and(|submitted| submitted.token.as_ref().is_none_or(NativeToken::done));
        if !done {
            return;
        }
        let Some(mut submitted) = state.submitted.take() else {
            return;
        };
        let completion = submitted
            .token
            .take()
            .map_or(Ok(()), |token| token.status());
        let mut other_failed = 0_usize;
        if let Err(reason) = &completion {
            state.failed.get_or_insert_with(|| reason.clone());
            state.stopping = true;
            let reason = reason.clone();
            while let Some(item) = state.waiting.pop_front() {
                state.immediate.push_back(Completed::Failed {
                    tag: item.tag,
                    reason: reason.clone(),
                });
                other_failed += 1;
            }
            if let Some(closed) = state.closed.take() {
                other_failed += closed.count;
                for item in &closed.items {
                    state.immediate.push_back(Completed::Failed {
                        tag: item.tag,
                        reason: reason.clone(),
                    });
                }
                state.orphaned.push(closed);
            }
        }
        let failed_batch = completion.err();
        let returned = submitted.envelope.count;
        for item in submitted.envelope.items.drain(..) {
            match item.outcome {
                ItemOutcome::Ready { native, allocated } if failed_batch.is_some() => {
                    release_charge(&self.shared.stats, allocated);
                    out.push(Completed::Failed {
                        tag: item.tag,
                        reason: failed_batch
                            .clone()
                            .unwrap_or_else(|| "upload failed".to_owned()),
                    });
                    drop(native);
                }
                ItemOutcome::Ready { native, allocated } => {
                    out.push(Completed::Ready(PreparedTexture {
                        tag: item.tag,
                        width: item.width,
                        height: item.height,
                        allocated_size: allocated,
                        native: Some(native),
                        stats: Arc::clone(&self.shared.stats),
                    }));
                }
                ItemOutcome::Failed(reason) => out.push(Completed::Failed {
                    tag: item.tag,
                    reason,
                }),
            }
        }
        let staging_charge = submitted.envelope.staging_charge;
        drop(submitted.envelope);
        release_staging(&self.shared.stats, staging_charge);
        state.occupied = state
            .occupied
            .saturating_sub(returned.saturating_add(other_failed));
        let occupied = state.occupied;
        lock(&self.shared.stats).prepared_images = occupied;
        lock(&self.shared.stats).batches_completed += 1;
        drop(state);
        self.shared.changed.notify_all();
        notify_wake(&self.shared, self.shared.wakes.room.as_ref());
    }

    /// Returns and clears the first latched GPU failure once.
    pub fn take_failure(&mut self) -> Option<String> {
        let mut state = lock(&self.shared.state);
        if state.failure_taken {
            return None;
        }
        let failure = state.failed.clone()?;
        state.failure_taken = true;
        Some(failure)
    }

    /// Looks, without taking or releasing anything, for a GPU failure: one already latched, or the error of a
    /// submitted batch whose completion has arrived (still held, with its staging, until `poll` or `release`). The
    /// teardown uses it to report a failure before the combined proof without destroying a completed envelope.
    #[must_use]
    pub fn peek_failure(&self) -> Option<String> {
        let state = lock(&self.shared.state);
        if let Some(failure) = &state.failed {
            return Some(failure.clone());
        }
        state
            .submitted
            .as_ref()
            .and_then(|submitted| submitted.token.as_ref())
            .filter(|token| token.done())
            .and_then(|token| token.status().err())
    }

    /// Whether the submitted batch's completion has arrived and the batch is still held, not yet polled (tests
    /// only: a phase handshake that reads, and takes nothing).
    #[doc(hidden)]
    #[must_use]
    pub fn completion_arrived_unpolled(&self) -> bool {
        lock(&self.shared.state)
            .submitted
            .as_ref()
            .and_then(|submitted| submitted.token.as_ref())
            .is_some_and(NativeToken::done)
    }

    /// Whether a prepared, closed envelope waits for the owner to submit it (tests only: a phase handshake that
    /// reads, and takes nothing).
    #[doc(hidden)]
    #[must_use]
    pub fn envelope_ready_unsubmitted(&self) -> bool {
        lock(&self.shared.state).closed.is_some()
    }

    /// A consistent snapshot of bounded aggregate counters.
    #[must_use]
    pub fn stats(&self) -> TextureStats {
        lock(&self.shared.stats).clone()
    }

    /// Performs the non-destructive worker and completion proof against one absolute deadline.
    pub fn prove(&mut self, deadline: Instant) -> Proof {
        {
            let mut state = lock(&self.shared.state);
            state.stopping = true;
            while let Some(item) = state.waiting.pop_front() {
                state.immediate.push_back(Completed::Failed {
                    tag: item.tag,
                    reason: "store stopping".to_owned(),
                });
                state.occupied = state.occupied.saturating_sub(1);
            }
            if let Some(envelope) = state.closed.take() {
                for item in &envelope.items {
                    let reason = match &item.outcome {
                        ItemOutcome::Ready { .. } => "store stopping".to_owned(),
                        ItemOutcome::Failed(reason) => reason.clone(),
                    };
                    state.immediate.push_back(Completed::Failed {
                        tag: item.tag,
                        reason,
                    });
                }
                state.occupied = state.occupied.saturating_sub(envelope.count);
                state.orphaned.push(envelope);
            }
            let occupied = state.occupied;
            lock(&self.shared.stats).prepared_images = occupied;
            self.shared.changed.notify_all();
        }
        loop {
            let mut state = lock(&self.shared.state);
            if state.worker_finished {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                return Proof {
                    owner_id: self.owner_id,
                    complete: false,
                };
            }
            let wait = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(2));
            let (next, _) = self
                .shared
                .changed
                .wait_timeout(state, wait)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state = next;
            drop(state);
        }
        if let Some(worker) = self.worker.take() {
            unwind::join(worker);
        }
        if lock(&self.shared.state).worker_panic.is_some() {
            return Proof {
                owner_id: self.owner_id,
                complete: false,
            };
        }
        loop {
            let done = lock(&self.shared.state)
                .submitted
                .as_ref()
                .is_none_or(|submitted| submitted.token.as_ref().is_none_or(NativeToken::done));
            if done {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                return Proof {
                    owner_id: self.owner_id,
                    complete: false,
                };
            }
            thread::sleep(
                deadline
                    .saturating_duration_since(now)
                    .min(Duration::from_millis(1)),
            );
        }
        Proof {
            owner_id: self.owner_id,
            complete: true,
        }
    }

    /// Releases native graph only after this owner's complete proof.
    pub fn release(&mut self, proof: Proof) {
        if proof.owner_id != self.owner_id || !proof.complete {
            return;
        }
        let mut state = lock(&self.shared.state);
        if let Some(mut envelope) = state.closed.take() {
            for item in envelope.items.drain(..) {
                drop_item_result(item, &self.shared.stats);
            }
            release_staging(&self.shared.stats, envelope.staging_charge);
        }
        for mut envelope in state.orphaned.drain(..) {
            for item in envelope.items.drain(..) {
                drop_item_result(item, &self.shared.stats);
            }
            release_staging(&self.shared.stats, envelope.staging_charge);
        }
        if let Some(mut submitted) = state.submitted.take() {
            for item in submitted.envelope.items.drain(..) {
                drop_item_result(item, &self.shared.stats);
            }
            release_staging(&self.shared.stats, submitted.envelope.staging_charge);
        }
        state.immediate.clear();
        state.preparing_batch = None;
        state.preparing_textures.clear();
        state.occupied = 0;
        lock(&self.shared.stats).prepared_images = 0;
        self.context.take();
        self.released = true;
    }

    /// Standalone end (`prove` followed by release) using one deadline for every stage.
    pub fn shutdown(&mut self, deadline: Instant) -> StoreCleanup {
        let proof = self.prove(deadline);
        if proof.complete {
            self.release(proof);
            StoreCleanup::Proven
        } else {
            let reason = lock(&self.shared.state)
                .worker_panic
                .clone()
                .unwrap_or_else(|| {
                    "worker or submitted upload remained outstanding at deadline".to_owned()
                });
            StoreCleanup::Unproven { reason }
        }
    }
}

impl<T> Drop for TextureOwner<T>
where
    T: Copy + Eq + std::hash::Hash + Send + 'static,
{
    fn drop(&mut self) {
        if !self.released {
            let result = self.shutdown(Instant::now());
            if matches!(result, StoreCleanup::Unproven { .. }) {
                // SAFETY: retaining the shared graph to process exit is the specified unproven-end policy.
                let graph = Arc::clone(&self.shared);
                std::mem::forget(graph);
                if let Some(context) = self.context.take() {
                    std::mem::forget(context);
                }
            }
        }
    }
}

/// A test's gate: the worker waits until the flag reads true.
#[doc(hidden)]
pub type StallGate = Arc<(Mutex<bool>, Condvar)>;

/// Hidden fault controls, available only from `start_with_test_hooks`.
#[doc(hidden)]
#[derive(Debug, Clone, Default)]
#[expect(
    missing_docs,
    reason = "fault switches are test-only and named by their contract"
)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "the fault switches are deliberately independent"
)]
pub struct TestHooks {
    /// Overrides the bindless residency work budget for deterministic ledger tests.
    pub residency_change_budget: Option<usize>,
    /// Forces the bindless capability gate to observe Tier 1 support.
    pub force_bindless_tier1: bool,
    /// Forces native residency-set creation to fail.
    pub fail_residency_creation: bool,
    pub fail_texture_allocation_at: Option<usize>,
    pub force_estimate_at: Option<(usize, u64)>,
    pub nil_staging: bool,
    pub nil_command: bool,
    pub nil_encoder: bool,
    pub fail_commit: bool,
    pub force_measured_size: Option<u64>,
    pub error_completion: bool,
    pub completion_delay: Duration,
    /// Delays completion publication off Metal's completion thread (bounded at 30 s) until this reads non-zero.
    /// The test decides when the submitted batch's completion may arrive; the hook never blocks that thread.
    pub completion_gate: Option<Arc<AtomicI32>>,
    /// The owner sleeps this long right after it submitted a batch, before anything else (the stall between a
    /// submit and the poll that follows it, that a gate must make harmless).
    pub submit_pause: Duration,
    pub panic_worker: bool,
    pub panic_worker_after_batch: bool,
    pub panic_worker_after_texture: bool,
    pub panic_worker_payload_drop_panics: bool,
    pub block_worker: Option<Arc<(Mutex<bool>, Condvar)>>,
    pub block_after_texture: Option<Arc<(Mutex<bool>, Condvar)>>,
    pub block_after_texture_at: Option<usize>,
    /// A second stall: the `n`-th allocation waits for this gate too (a later image kept pending behind an earlier one).
    pub block_after_texture_second: Option<(usize, StallGate)>,
    pub staging_reservation: Option<Arc<AtomicU64>>,
    pub thread_observer: Option<Arc<Mutex<TestThreadObservation>>>,
    pub panic_wake_ready: bool,
    pub panic_wake_completed: bool,
    pub panic_wake_room: bool,
}

/// Test-only evidence of which thread prepared and committed an upload.
#[doc(hidden)]
#[derive(Debug, Default)]
pub struct TestThreadObservation {
    /// Worker thread that prepared the envelope.
    pub worker: Option<thread::ThreadId>,
    /// Threads that called the owner-only native commit boundary.
    pub commits: Vec<thread::ThreadId>,
}

/// Starts a worker that prepares private-texture upload envelopes.
pub struct TextureStore;

impl TextureStore {
    /// Starts the worker and returns the producer and unique owner handles.
    ///
    /// # Errors
    ///
    /// Returns invalid limits or an upload-worker spawn error.
    pub fn start<T>(
        context: Arc<MetalContext>,
        limits: TextureLimits,
        wakes: TextureWakes,
    ) -> Result<(UploadPort<T>, TextureOwner<T>), Error>
    where
        T: Copy + Eq + std::hash::Hash + Send + 'static,
    {
        Self::start_inner(context, limits, wakes, TestHooks::default())
    }

    /// Test-only fault boundary. Production paths never call this constructor.
    #[doc(hidden)]
    pub fn start_with_test_hooks<T>(
        context: Arc<MetalContext>,
        limits: TextureLimits,
        wakes: TextureWakes,
        hooks: TestHooks,
    ) -> Result<(UploadPort<T>, TextureOwner<T>), Error>
    where
        T: Copy + Eq + std::hash::Hash + Send + 'static,
    {
        Self::start_inner(context, limits, wakes, hooks)
    }

    fn start_inner<T>(
        context: Arc<MetalContext>,
        limits: TextureLimits,
        wakes: TextureWakes,
        hooks: TestHooks,
    ) -> Result<(UploadPort<T>, TextureOwner<T>), Error>
    where
        T: Copy + Eq + std::hash::Hash + Send + 'static,
    {
        let limits = limits.validate()?;
        let hooks = Arc::new(hooks);
        let wakes = test_wakes(wakes, &hooks);
        let stats = Arc::new(Mutex::new(TextureStats::default()));
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                waiting: VecDeque::new(),
                closed: None,
                submitted: None,
                immediate: VecDeque::new(),
                stopping: false,
                failed: None,
                failure_taken: false,
                occupied: 0,
                worker_finished: false,
                worker_panic: None,
                orphaned: Vec::new(),
                preparing_batch: None,
                preparing_textures: Vec::new(),
                max: limits.max_prepared_images,
            }),
            changed: Condvar::new(),
            stats,
            wakes,
            capacity: limits.max_prepared_images,
            test_hooks: Arc::clone(&hooks),
        });
        let worker_shared = Arc::clone(&shared);
        let worker_context = Arc::clone(&context);
        let worker = thread::Builder::new()
            .name("ck-metal-upload".to_owned())
            .spawn(move || worker_main(&worker_shared, &worker_context, limits, &hooks))
            .map_err(|error| Error(format!("upload worker spawn failed: {error}")))?;
        let owner_id = NEXT_OWNER.fetch_add(1, Ordering::Relaxed);
        Ok((
            UploadPort {
                shared: Arc::clone(&shared),
            },
            TextureOwner {
                shared,
                context: Some(context),
                worker: Some(worker),
                owner_id,
                released: false,
                _not_sync: PhantomData,
            },
        ))
    }
}

struct CompletionHook {
    wake: Option<Arc<dyn Fn() + Send + Sync>>,
    stats: Arc<Mutex<TextureStats>>,
}

extern "C" fn completion_callback(raw: *mut c_void) {
    // SAFETY: successful native submission transfers this unique box to its one completion callback.
    let hook = unsafe { Box::from_raw(raw.cast::<CompletionHook>()) };
    if let Some(wake) = &hook.wake
        && let Err(payload) = catch_unwind(AssertUnwindSafe(|| wake()))
    {
        unwind::dispose(payload);
        lock(&hook.stats).wake_panics += 1;
    }
}

fn notify_wake<T>(shared: &Shared<T>, wake: Option<&Arc<dyn Fn() + Send + Sync>>) {
    if let Some(wake) = wake
        && let Err(payload) = catch_unwind(AssertUnwindSafe(|| wake()))
    {
        unwind::dispose(payload);
        lock(&shared.stats).wake_panics += 1;
    }
}

fn test_wakes(wakes: TextureWakes, hooks: &TestHooks) -> TextureWakes {
    fn wrap(
        wake: Option<Arc<dyn Fn() + Send + Sync>>,
        panic: bool,
        label: &'static str,
    ) -> Option<Arc<dyn Fn() + Send + Sync>> {
        if !panic {
            return wake;
        }
        Some(Arc::new(move || {
            let _ = &wake;
            panic!("test {label} wake panic");
        }))
    }
    TextureWakes {
        ready_to_submit: wrap(wakes.ready_to_submit, hooks.panic_wake_ready, "ready"),
        completed: wrap(wakes.completed, hooks.panic_wake_completed, "completed"),
        room: wrap(wakes.room, hooks.panic_wake_room, "room"),
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn align_up(value: u64, alignment: u64) -> Option<u64> {
    value
        .checked_add(alignment.checked_sub(1)?)
        .map(|v| v / alignment * alignment)
}

fn image_staging_need(item: &UploadItem<impl Sized>) -> Option<(usize, u64)> {
    let bytes = usize::try_from(item.width)
        .ok()?
        .checked_mul(usize::try_from(item.height).ok()?)?
        .checked_mul(4)?;
    Some((bytes, align_up(bytes as u64, 256)?))
}

fn packed_staging_need<'a, T: 'a>(items: impl Iterator<Item = &'a UploadItem<T>>) -> u64 {
    let packed = items
        .filter_map(image_staging_need)
        .fold(0_u64, |sum, (_, bytes)| {
            align_up(sum, 256)
                .and_then(|aligned| aligned.checked_add(bytes))
                .unwrap_or(u64::MAX)
        });
    if packed == 0 {
        0
    } else {
        align_up(packed, 16 * 1024).unwrap_or(u64::MAX)
    }
}

fn worker_main<T>(
    shared: &Arc<Shared<T>>,
    context: &Arc<MetalContext>,
    limits: TextureLimits,
    hooks: &TestHooks,
) where
    T: Copy + Eq + std::hash::Hash + Send + 'static,
{
    if let Some(observer) = &hooks.thread_observer {
        lock(observer).worker = Some(thread::current().id());
    }
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        worker_loop(shared, context, limits, hooks);
    }));
    if let Err(payload) = outcome {
        let reason = payload
            .downcast_ref::<WorkerPanicPayload>()
            .map(|payload| payload.reason.to_owned())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_owned()))
            .unwrap_or_else(|| "upload worker panicked".to_owned());
        unwind::dispose(payload);
        let mut state = lock(&shared.state);
        state.worker_panic = Some(reason.clone());
        state.failed.get_or_insert(reason);
        state.stopping = true;
        state.worker_finished = true;
        shared.changed.notify_all();
        return;
    }
    let mut state = lock(&shared.state);
    state.worker_finished = true;
    shared.changed.notify_all();
}

struct WorkerPanicPayload {
    reason: &'static str,
}

impl Drop for WorkerPanicPayload {
    fn drop(&mut self) {
        panic!("negative control: worker panic payload drop");
    }
}

fn panic_worker_if(hooks: &TestHooks, enabled: bool) {
    if !enabled {
        return;
    }
    if hooks.panic_worker_payload_drop_panics {
        std::panic::panic_any(WorkerPanicPayload {
            reason: "test upload worker panic",
        });
    }
    panic!("test upload worker panic");
}

#[expect(
    clippy::too_many_lines,
    reason = "one upload pass keeps its lease and reservation transitions auditable together"
)]
fn worker_loop<T>(
    shared: &Arc<Shared<T>>,
    context: &Arc<MetalContext>,
    limits: TextureLimits,
    hooks: &TestHooks,
) where
    T: Copy + Eq + std::hash::Hash + Send + 'static,
{
    if let Some(gate) = &hooks.block_worker {
        let (mutex, changed) = &**gate;
        let mut released = lock(mutex);
        while !*released {
            released = changed
                .wait(released)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
    let mut allocation_index = 0_usize;
    let mut estimate_index = 0_usize;
    loop {
        let mut items = {
            let mut state = lock(&shared.state);
            while !state.stopping && (state.closed.is_some() || state.waiting.is_empty()) {
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if state.stopping {
                break;
            }
            let Some(first) = state.waiting.pop_front() else {
                continue;
            };
            let mut batch = vec![first];
            let staging_need = packed_staging_need(batch.iter());
            let first_is_oversized = staging_need > limits.max_upload_bytes_per_batch;
            while !first_is_oversized && batch.len() < limits.max_upload_images_per_batch {
                let Some(next) = state.waiting.front() else {
                    break;
                };
                let next_need = packed_staging_need(batch.iter().chain(std::iter::once(next)));
                if next_need > limits.max_upload_bytes_per_batch {
                    break;
                }
                if let Some(next) = state.waiting.pop_front() {
                    batch.push(next);
                }
            }
            batch
        };

        let mut valid = Vec::with_capacity(items.len());
        let mut offsets = Vec::with_capacity(items.len());
        let mut need = 0_u64;
        for item in &items {
            if let Ok(length) = validate_item(item) {
                need = align_up(need, 256)
                    .and_then(|n| n.checked_add(length as u64))
                    .unwrap_or(u64::MAX);
                offsets.push(need - length as u64);
                valid.push(Some(length));
            } else {
                offsets.push(0);
                valid.push(None);
            }
        }
        let actual_need = if need == 0 {
            0
        } else {
            // Staging buffers use the device's reported allocation, with a conservative 16 KiB
            // page granularity for the packed upload rather than one page per image.
            align_up(need, 16 * 1024).unwrap_or(u64::MAX)
        };

        if actual_need != 0 {
            let mut state = lock(&shared.state);
            let mut counted_wait = false;
            while !state.stopping {
                let reserved = lock(&shared.stats).staging_bytes;
                let test_reserved = hooks
                    .staging_reservation
                    .as_ref()
                    .map_or(0, |reservation| reservation.load(Ordering::Acquire));
                if reserved
                    .saturating_add(test_reserved)
                    .saturating_add(actual_need)
                    <= limits.staging_bytes
                {
                    break;
                }
                if !counted_wait {
                    lock(&shared.stats).staging_waits += 1;
                    counted_wait = true;
                }
                // Staging capacity is returned only after `poll` proves batch completion, then it
                // notifies this condition variable. Stop also notifies it. An idle timeout would
                // mask a missing wake and keep the worker needlessly running while blocked.
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if state.stopping {
                for item in items.drain(..) {
                    state.immediate.push_back(Completed::Failed {
                        tag: item.tag,
                        reason: "store stopping".to_owned(),
                    });
                }
                state.occupied = state.occupied.saturating_sub(valid.len());
                let occupied = state.occupied;
                lock(&shared.stats).prepared_images = occupied;
                drop(state);
                shared.changed.notify_all();
                notify_wake(shared, shared.wakes.room.as_ref());
                continue;
            }
            let mut counters = lock(&shared.stats);
            counters.staging_bytes = counters.staging_bytes.saturating_add(actual_need);
            counters.staging_bytes_peak = counters.staging_bytes_peak.max(counters.staging_bytes);
        }

        panic_worker_if(hooks, hooks.panic_worker);
        let mut staging_charge = actual_need;
        let mut batch = if actual_need == 0 {
            None
        } else {
            match context.batch(actual_need, hooks) {
                Ok((batch, measured)) if measured != 0 && measured <= actual_need => {
                    if measured < actual_need {
                        let mut stats = lock(&shared.stats);
                        stats.staging_bytes =
                            stats.staging_bytes.saturating_sub(actual_need - measured);
                    }
                    staging_charge = measured;
                    lock(&shared.state).preparing_batch = Some(batch.clone());
                    panic_worker_if(hooks, hooks.panic_worker_after_batch);
                    Some(batch)
                }
                Ok((batch, measured)) => {
                    lock(&shared.state).preparing_batch = Some(batch.clone());
                    let mut stats = lock(&shared.stats);
                    stats.staging_bytes = stats
                        .staging_bytes
                        .saturating_sub(actual_need)
                        .saturating_add(measured);
                    stats.staging_bytes_peak = stats.staging_bytes_peak.max(stats.staging_bytes);
                    drop(stats);
                    staging_charge = measured;
                    latch_native_failure_with_batch(
                        shared,
                        &mut items,
                        &format!(
                            "staging allocatedSize {measured} exceeds reserved need {actual_need}"
                        ),
                        Some(batch),
                        staging_charge,
                    );
                    continue;
                }
                Err((partial, measured, error)) => {
                    // Even a batch whose command/encoder setup failed can own a staging
                    // buffer. Keep that native holder and its measured charge in the
                    // orphan ledger until the worker has stopped and owner proof runs.
                    {
                        let mut stats = lock(&shared.stats);
                        stats.staging_bytes = stats
                            .staging_bytes
                            .saturating_sub(actual_need)
                            .saturating_add(measured);
                        stats.staging_bytes_peak =
                            stats.staging_bytes_peak.max(stats.staging_bytes);
                    }
                    staging_charge = measured;
                    if let Some(batch) = &partial {
                        lock(&shared.state).preparing_batch = Some(batch.clone());
                    }
                    latch_native_failure_with_batch(
                        shared,
                        &mut items,
                        &error.to_string(),
                        partial,
                        staging_charge,
                    );
                    continue;
                }
            }
        };

        let mut results = Vec::with_capacity(items.len());
        let mut native_failure: Option<String> = None;
        for (index, item) in items.drain(..).enumerate() {
            if let Some(reason) = &native_failure {
                results.push(ItemResult {
                    tag: item.tag,
                    width: item.width,
                    height: item.height,
                    outcome: ItemOutcome::Failed(reason.clone()),
                });
                drop(item.pixels);
                continue;
            }
            let Some(_length) = valid[index] else {
                lock(&shared.stats).item_failures += 1;
                results.push(ItemResult {
                    tag: item.tag,
                    width: item.width,
                    height: item.height,
                    outcome: ItemOutcome::Failed(validate_item(&item).expect_err("invalid source")),
                });
                continue;
            };
            estimate_index += 1;
            let estimate = hooks
                .force_estimate_at
                .filter(|(index, _)| *index == estimate_index)
                .map_or_else(
                    || context.estimate(item.width, item.height),
                    |(_, estimate)| estimate,
                );
            if estimate == 0 {
                lock(&shared.stats).item_failures += 1;
                results.push(ItemResult {
                    tag: item.tag,
                    width: item.width,
                    height: item.height,
                    outcome: ItemOutcome::Failed(
                        "Metal returned a zero texture estimate".to_owned(),
                    ),
                });
                continue;
            }
            {
                let mut stats = lock(&shared.stats);
                if stats.texture_bytes_reserved.saturating_add(estimate) > limits.texture_bytes {
                    stats.item_failures += 1;
                    results.push(ItemResult { tag: item.tag, width: item.width, height: item.height,
                        outcome: ItemOutcome::Failed(format!("texture budget refused estimate {estimate} with {} of {} bytes charged", stats.texture_bytes_reserved, limits.texture_bytes)) });
                    continue;
                }
                stats.texture_bytes_reserved += estimate;
                stats.texture_bytes_peak =
                    stats.texture_bytes_peak.max(stats.texture_bytes_reserved);
                stats.texture_transient_peak = stats
                    .texture_transient_peak
                    .max(stats.texture_bytes_reserved);
            }
            allocation_index += 1;
            let Some(batch_ref) = batch.as_mut() else {
                unreachable!("valid item reserves staging")
            };
            let (texture, mut measured) = match batch_ref.texture(
                item.width,
                item.height,
                hooks.fail_texture_allocation_at == Some(allocation_index),
            ) {
                Ok(value) => value,
                Err(error) => {
                    release_estimate(&shared.stats, estimate);
                    native_failure = Some(error.to_string());
                    results.push(ItemResult {
                        tag: item.tag,
                        width: item.width,
                        height: item.height,
                        outcome: ItemOutcome::Failed(error.to_string()),
                    });
                    drop(item.pixels);
                    continue;
                }
            };
            // A native texture now exists: it counts, whatever the measured-size check below decides (a texture created and
            // then discarded is still texture work). A failed native allocation returned above and counts nothing.
            lock(&shared.stats).textures_created += 1;
            lock(&shared.state).preparing_textures.push(texture.clone());
            let first = hooks.block_after_texture.as_ref().filter(|_| {
                hooks
                    .block_after_texture_at
                    .is_none_or(|target| target == allocation_index)
            });
            let second = hooks
                .block_after_texture_second
                .as_ref()
                .filter(|(target, _)| *target == allocation_index)
                .map(|(_, gate)| gate);
            for gate in first.into_iter().chain(second) {
                let (mutex, changed) = &**gate;
                let mut released = lock(mutex);
                while !*released {
                    released = changed
                        .wait(released)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            }
            panic_worker_if(hooks, hooks.panic_worker_after_texture);
            if let Some(forced) = hooks.force_measured_size {
                measured = forced;
            }
            let within_budget = {
                let mut stats = lock(&shared.stats);
                let without = stats.texture_bytes_reserved.saturating_sub(estimate);
                let transient = without.saturating_add(measured);
                stats.texture_transient_peak = stats.texture_transient_peak.max(transient);
                if transient > limits.texture_bytes {
                    stats.texture_bytes_reserved = without;
                    false
                } else {
                    stats.texture_bytes_reserved = transient;
                    stats.texture_bytes_measured =
                        stats.texture_bytes_measured.saturating_add(measured);
                    stats.texture_bytes_peak = stats.texture_bytes_peak.max(transient);
                    stats.live_textures += 1;
                    stats.smallest_texture =
                        Some(stats.smallest_texture.map_or(measured, |v| v.min(measured)));
                    stats.largest_texture =
                        Some(stats.largest_texture.map_or(measured, |v| v.max(measured)));
                    true
                }
            };
            if !within_budget {
                batch_ref.discard_texture(&texture);
                lock(&shared.state).preparing_textures.pop();
                drop(texture);
                lock(&shared.stats).item_failures += 1;
                results.push(ItemResult {
                    tag: item.tag,
                    width: item.width,
                    height: item.height,
                    outcome: ItemOutcome::Failed(format!(
                        "measured texture size {measured} exceeds texture_bytes budget {}",
                        limits.texture_bytes
                    )),
                });
                drop(item.pixels);
                continue;
            }
            if let Err(error) = batch_ref.copy(
                &texture,
                item.width,
                item.height,
                offsets[index],
                item.pixels.pixels(),
            ) {
                native_failure = Some(error.to_string());
                results.push(ItemResult {
                    tag: item.tag,
                    width: item.width,
                    height: item.height,
                    outcome: ItemOutcome::Ready {
                        native: texture,
                        allocated: measured,
                    },
                });
                drop(item.pixels);
                continue;
            }
            drop(item.pixels);
            results.push(ItemResult {
                tag: item.tag,
                width: item.width,
                height: item.height,
                outcome: ItemOutcome::Ready {
                    native: texture,
                    allocated: measured,
                },
            });
        }

        if let Some(reason) = native_failure {
            latch_prepared_failure(shared, results, &reason, batch, staging_charge);
            continue;
        }
        if let Some(native_batch) = batch.as_mut()
            && let Err(error) = native_batch.close()
        {
            latch_prepared_failure(shared, results, &error.to_string(), batch, staging_charge);
            continue;
        }
        let item_count = results.len();
        let mut state = lock(&shared.state);
        if state.stopping {
            let reason = state
                .failed
                .clone()
                .unwrap_or_else(|| "store stopping".to_owned());
            for item in &results {
                let item_reason = match &item.outcome {
                    ItemOutcome::Ready { .. } => reason.clone(),
                    ItemOutcome::Failed(item_reason) => item_reason.clone(),
                };
                state.immediate.push_back(Completed::Failed {
                    tag: item.tag,
                    reason: item_reason,
                });
            }
            if batch.is_some() || staging_charge != 0 || !results.is_empty() {
                state.orphaned.push(Envelope {
                    batch,
                    items: results,
                    count: item_count,
                    staging_charge,
                });
                state.preparing_batch = None;
                state.preparing_textures.clear();
            } else {
                release_staging(&shared.stats, staging_charge);
            }
            state.occupied = state.occupied.saturating_sub(item_count);
            let occupied = state.occupied;
            lock(&shared.stats).prepared_images = occupied;
            shared.changed.notify_all();
            drop(state);
            notify_wake(shared, shared.wakes.room.as_ref());
            continue;
        }
        let mut counters = lock(&shared.stats);
        counters.batches_prepared += 1;
        counters.images_per_batch_max = counters.images_per_batch_max.max(item_count);
        drop(counters);
        state.closed = Some(Envelope {
            batch,
            items: results,
            count: item_count,
            staging_charge,
        });
        state.preparing_batch = None;
        state.preparing_textures.clear();
        drop(state);
        notify_wake(shared, shared.wakes.ready_to_submit.as_ref());
    }
}

fn latch_native_failure_with_batch<T: Copy>(
    shared: &Arc<Shared<T>>,
    items: &mut Vec<UploadItem<T>>,
    reason: &str,
    batch: Option<NativeBatch>,
    staging: u64,
) {
    let results = items
        .drain(..)
        .map(|item| ItemResult {
            tag: item.tag,
            width: item.width,
            height: item.height,
            outcome: ItemOutcome::Failed(reason.to_owned()),
        })
        .collect();
    latch_prepared_failure(shared, results, reason, batch, staging);
}

fn latch_prepared_failure<T>(
    shared: &Arc<Shared<T>>,
    items: Vec<ItemResult<T>>,
    reason: &str,
    batch: Option<NativeBatch>,
    staging: u64,
) where
    T: Copy,
{
    let count = items.len();
    let mut state = lock(&shared.state);
    state.failed.get_or_insert_with(|| reason.to_owned());
    state.stopping = true;
    for item in &items {
        let item_reason = match &item.outcome {
            ItemOutcome::Ready { .. } => reason,
            ItemOutcome::Failed(item_reason) => item_reason,
        };
        state.immediate.push_back(Completed::Failed {
            tag: item.tag,
            reason: item_reason.to_owned(),
        });
    }
    state.orphaned.push(Envelope {
        batch,
        items,
        count,
        staging_charge: staging,
    });
    state.preparing_batch = None;
    state.preparing_textures.clear();
    state.occupied = state.occupied.saturating_sub(count);
    lock(&shared.stats).prepared_images = state.occupied;
    drop(state);
    shared.changed.notify_all();
    notify_wake(shared, shared.wakes.room.as_ref());
    notify_wake(shared, shared.wakes.ready_to_submit.as_ref());
}

fn drop_item_result<T>(item: ItemResult<T>, stats: &Arc<Mutex<TextureStats>>) {
    if let ItemOutcome::Ready { native, allocated } = item.outcome {
        drop(native);
        release_charge(stats, allocated);
    }
}

fn release_charge(stats: &Arc<Mutex<TextureStats>>, bytes: u64) {
    let mut stats = lock(stats);
    stats.texture_bytes_reserved = stats.texture_bytes_reserved.saturating_sub(bytes);
    stats.texture_bytes_measured = stats.texture_bytes_measured.saturating_sub(bytes);
    stats.live_textures = stats.live_textures.saturating_sub(1);
}

fn release_estimate(stats: &Arc<Mutex<TextureStats>>, bytes: u64) {
    let mut stats = lock(stats);
    stats.texture_bytes_reserved = stats.texture_bytes_reserved.saturating_sub(bytes);
}

fn release_staging(stats: &Arc<Mutex<TextureStats>>, bytes: u64) {
    let mut stats = lock(stats);
    stats.staging_bytes = stats.staging_bytes.saturating_sub(bytes);
}

fn validate_item<T>(item: &UploadItem<T>) -> Result<usize, String> {
    if !(1..=MAX_IMAGE_SIDE).contains(&item.width) || !(1..=MAX_IMAGE_SIDE).contains(&item.height) {
        return Err(format!(
            "image dimensions {}x{} are outside 1..={MAX_IMAGE_SIDE}",
            item.width, item.height
        ));
    }
    let length = (item.width as usize)
        .checked_mul(item.height as usize)
        .and_then(|n| n.checked_mul(4))
        .ok_or_else(|| "image byte length overflows usize".to_owned())?;
    if length as u64 > MAX_IMAGE_BYTES {
        return Err(format!(
            "image byte length {length} exceeds {MAX_IMAGE_BYTES}"
        ));
    }
    if item.pixels.pixels().len() != length {
        return Err(format!(
            "image has {} bytes, expected {length}",
            item.pixels.pixels().len()
        ));
    }
    Ok(length)
}

#[cfg(test)]
mod tests {
    use super::TextureLimits;

    #[test]
    fn defaults_and_inclusive_limit_edges_are_valid() {
        assert!(TextureLimits::default().validate().is_ok());
        assert!(
            TextureLimits {
                max_prepared_images: 2,
                max_upload_images_per_batch: 1,
                max_upload_bytes_per_batch: 1 << 20,
                staging_bytes: 128 << 20,
                texture_bytes: 64 << 20,
            }
            .validate()
            .is_ok()
        );
        assert!(
            TextureLimits {
                max_prepared_images: 4096,
                max_upload_images_per_batch: 2048,
                max_upload_bytes_per_batch: 256 << 20,
                staging_bytes: 512 << 20,
                texture_bytes: 64 << 30,
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn rejects_ranges_and_names_fields_and_two_lease_relationships() {
        let limits = TextureLimits {
            max_prepared_images: 1,
            ..TextureLimits::default()
        };
        assert!(
            limits
                .validate()
                .unwrap_err()
                .to_string()
                .contains("max_prepared_images")
        );

        let limits = TextureLimits {
            max_upload_images_per_batch: 2048,
            ..TextureLimits::default()
        };
        assert!(
            limits
                .validate()
                .unwrap_err()
                .to_string()
                .contains("max_prepared_images")
        );

        let limits = TextureLimits {
            max_upload_bytes_per_batch: 256 << 20,
            ..TextureLimits::default()
        };
        let error = limits.validate().unwrap_err().to_string();
        assert!(error.contains("max_upload_bytes_per_batch"));
        assert!(error.contains("staging_bytes/2"));

        let limits = TextureLimits {
            staging_bytes: (128 << 20) - 1,
            ..TextureLimits::default()
        };
        assert!(
            limits
                .validate()
                .unwrap_err()
                .to_string()
                .contains("staging_bytes")
        );

        let limits = TextureLimits {
            texture_bytes: (64 << 20) - 1,
            ..TextureLimits::default()
        };
        assert!(
            limits
                .validate()
                .unwrap_err()
                .to_string()
                .contains("texture_bytes")
        );
    }
}
