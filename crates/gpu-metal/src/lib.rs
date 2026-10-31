//! A narrow ownership boundary around Apple's native Metal API: the synchronous bootstrap probe
//! ([`Device`]) and the Metal presenter ([`MetalPresenter`]).
//!
//! # The presenter
//!
//! [`MetalPresenter`] clears a window's `CAMetalLayer` to a colour. The main thread drives a handle whose
//! methods never wait beyond a brief lock to copy a small value; a **render thread** owns every GPU object.
//!
//! - **Threads.** `start` spawns the render thread and returns at once; `Err` means no thread was spawned
//!   and no GPU object exists (the retain on the layer is undone). The thread creates the device and the
//!   command queue and configures the layer (`BGRA8Unorm`, `framebufferOnly`, three drawables, the
//!   drawable timeout, `drawableSize`); any failure there is a `Failed` report followed by an immediate
//!   release of what setup made and a `Proven` cleanup, without waiting for a stop. `set_clear`,
//!   `set_pixel_size` and `request_redraw` write a latest-value slot and signal the thread: a changed colour
//!   or size and a redraw request each want one frame, requests coalesce, and a frame is drawn only when the
//!   previous command buffer has completed (checked without waiting).
//! - **The lifetime contract of `start`** (an `unsafe` function): the layer pointer must point to a live
//!   `CAMetalLayer` that stays alive at least until the presenter's end is proven ([`MetalPresenter::poll_end`]
//!   returned [`CleanupResult::Proven`]) or the process ends. The presenter takes its own retain and
//!   releases it only on a proven end. `wake` is called on the render thread after each report and after
//!   the cleanup result; it must be short and must not block, and a panic in it is caught and ignored and
//!   never changes a result already sent.
//! - **Ownership in a frame.** The render thread holds a strong reference to a drawable only inside one
//!   frame step (inside its own autorelease pool): from `nextDrawable` until the drawable is presented and
//!   the command buffer committed, when Core Animation and the command buffer own it, or until the frame is
//!   abandoned, when the reference is dropped at once. Each drawable gets a small independent **token**,
//!   set by its presented handler; each committed command buffer gets one, set by its completion handler.
//!   A handler captures its token (and the test observer), never a drawable, a command buffer or the
//!   presenter, so a late callback can always touch it. At most one command buffer is in flight and at most
//!   `maximumDrawableCount` (3) drawable tokens are outstanding; a frame that would exceed either waits.
//! - **Teardown.** After `request_stop` the thread stops drawing and waits, against **one absolute
//!   deadline** (the moment of the first `request_stop` plus 1.5 s; a repeated stop does not move it, and
//!   time spent in a `nextDrawable` that was blocking when the stop arrived comes out of it), for every
//!   completion and drawable token. If all arrived it releases every holder it still owns (a command buffer,
//!   the queue, the device, the layer retain) and sends `Proven`; otherwise it releases **none** of them and
//!   sends `Unproven` naming what is outstanding. Holders live in a native core that nothing releases except
//!   this teardown: a thread that ends by unwinding releases nothing. While the proof is incomplete the
//!   native core drops **nothing** (not the command buffer, not the tokens): the thread looks at the tokens
//!   without waiting and re-reads the real clock against the deadline between looks, so a delayed thread
//!   gets no fresh interval. `Proven` is reported only from an explicit success the thread sent and an
//!   observed thread end (`is_finished`, then joined).
//! - **Frame failures.** A missing command buffer, render-pass descriptor or encoder after a drawable was
//!   acquired abandons the frame (an encoder that was started is ended, nothing is committed; the drawable's
//!   token stays outstanding until its handler runs) and reports `Failed`. A command buffer that completes
//!   with an error has its token set (Metal has finished with its resources); the core **latches** that
//!   error the first time anything sees the token done, before any pruning or replacement, and the thread
//!   reports `Failed` with the error's description exactly once, whether the completion arrived while idle,
//!   between redraws or during the teardown. After a `Failed` the thread draws nothing more and waits for the stop;
//!   cleanup follows the same token rule, so a failed presenter can still end `Proven`.
//! - **Responsiveness, stated as it is.** `nextDrawable` may wait up to one second (the layer's
//!   `allowsNextDrawableTimeout`, set explicitly); the main thread never calls it. A late frame is the
//!   visible effect; a stop is still bounded by the absolute deadline, and a thread still blocked in native
//!   acquisition past the caller's own deadline is simply not observed finished: `Unproven` at the outer level.
//! - **Test hooks** reach the presenter only through the `#[doc(hidden)]` `start_with_test_hooks`; there is
//!   no Cargo feature, environment variable or flag (the full check builds the workspace with
//!   `--all-features`, so a feature would reach `codekarl` too). The guarantee is that **no production path
//!   activates a hook**, not that the hook code is absent from every linked binary; a test checks that
//!   `codekarl`'s sources never name the constructor. The test support (the keyboard filter, the typist) is
//!   built into a separate native archive that only the test executables link.
//! - **Off-screen readback.** [`readback_clear`] renders the same clear into an off-screen texture and reads
//!   its pixels back on the calling thread; it is used by the readback test and by nothing else.
//!
//! # The textured quad
//!
//! [`MetalPresenter::start_2d`] is [`MetalPresenter::start`] plus an `Arc<`[`MetalQuadSlot`]`>`: the render
//! thread draws the slot's quad, if its scene has one, over the clear in the **same render pass**; `start`
//! is unchanged and draws the clear only. Nothing here depends on a renderer crate.
//!
//! - **The types** are plain data ([`quad`](MetalQuad)): a [`PixelSource`] (premultiplied RGBA8, rows top
//!   first), a validated [`MetalQuad`] (an image of at most [`MAX_IMAGE_BYTES`] = 64 MiB, a destination in whole
//!   drawable pixels, a source rectangle in image pixels, a [`MetalFilter`]), and a [`MetalScene`], which is a
//!   generation and a quad *or none*. [`MetalQuadSlot::publish`] swaps a scene in under a lock held only for
//!   the swap, drops the replaced scene after releasing it, and wakes the render thread; it never waits for a
//!   frame, an upload or a stalled thread. A slot can be published to before the presenter starts, and the
//!   render thread reads it before its first frame.
//! - **Drawing.** `quad.metal` (compiled by `build.rs`) draws four vertices as a triangle strip, with
//!   blending `ONE, ONE_MINUS_SRC_ALPHA` and a clamp-to-edge sampler, nearest or linear. The destination is
//!   in the drawable's pixel space from its top-left corner; texture coordinates are the source rectangle over
//!   the image size. No colour conversion happens: the bytes of the image are the values blended.
//! - **Upload.** Only in a frame being drawn, and only when the scene's pixel source is not the one the
//!   resident texture came from (by `Arc` identity; the thread keeps a *weak* reference, so the allocation is
//!   not reused for another image while it is resident). A new `RGBA8Unorm` shared-storage texture is made and
//!   the bytes copied in, and the render thread **drops its reference to the pixel source as soon as the
//!   step returns**. A change of position or filter alone uploads nothing.
//! - **The GPU bound.** At most one resident texture and one retired one are alive, each at most 64 MiB, plus
//!   the pipeline and two samplers. A replaced or removed texture is *retired* with the token of the command
//!   buffer that last used it and released only after that token arrives. **Admission is explicit**: while a
//!   retired texture still waits for its token the step makes no new texture (and removes none); it returns
//!   "busy", and the frame is drawn once the token arrives, under the existing rule that a frame waits. (With
//!   one command buffer in flight the last user has normally completed when a frame starts, so the wait is
//!   short; the rule is what keeps the bound true when it is not.)
//! - **Teardown.** The textures, pipeline and samplers are holders like the queue and the device: released on
//!   a proven end, **none** released on an unproven one. A texture or pipeline that cannot be made, or a quad
//!   that cannot be encoded, is a `Failed` report under the existing rules (after it nothing more is drawn and
//!   cleanup follows the token rule); an allocation failure names the size.
//! - **The drawn generation.** [`MetalPresenter::drawn_generation`] is the generation of the scene in the last
//!   committed frame, with a quad or without: one latest value read without waiting, not a report per frame.
//! - **Off-screen readback.** [`readback_quad`] renders a clear and a quad with the presenter's own encoding
//!   code, pipeline and samplers; it is used by the pixel tests and by nothing else.
//!
//! # The scene presenter
//!
//! [`MetalPresenter::start_scene_bindless`] is [`MetalPresenter::start_2d`] on a **shared device context** with the texture
//! store, the revision ledger and a [`TextureDriver`] (milestone 5, issues 5a and 5b-1). Its frames are the latest
//! [`MetalFrame`] of a [`MetalFrameSlot`]: the legacy quad and a scene of instanced quads (below).
//!
//! - **One context.** The caller creates a [`MetalContext`]; the render thread takes its device and command queue
//!   from it (the layer is configured exactly as before) and starts the store and ledger on it during setup. The
//!   presenter keeps a share of the context for its life and releases it last, on a proven end only.
//!   `start` and `start_2d` keep their signatures and behaviour: each makes its own context on the render thread,
//!   where the device and queue used to be made, and a missing device is the same `Failed` report from that thread.
//! - **The driver.** A [`TextureDriver`] does the logical side - offering uploads, relaying commit and reject,
//!   `adopt` at a boundary, relaying retirements - on the render thread, with the ledger, at every [`Boundary`]: a
//!   frame boundary immediately before a frame is built (the one place adoption happens), an ordinary pass when texture
//!   work woke the thread, and a last `Ending` pass at the start of the teardown. It must not block. The presenter calls
//!   `pump` before each pass, takes every `GpuFailure` itself (a `Failed` report, queued and woken **before** the
//!   driver runs, so a driver that panics cannot hide it) and hands the other events to the driver; a pass that reports
//!   runnable work left ([`PassOutcome::more`]) is followed by another without waiting for a wake. The `Ending` pass and
//!   the rest of the teardown only **observe** (no submit, poll, reap or release): nothing device-dependent is dropped
//!   before the combined proof is complete.
//! - **Wakes.** The store's wakes and [`MetalPresenter::texture_wake`] mark "texture work pending" and signal the
//!   render thread under the slot's mutex, exactly as a dirty slot does; a wanted frame goes first and its boundary pass
//!   serves the pending work. Committed revisions waiting for a boundary make a frame wanted (so adoption never
//!   waits for an unrelated redraw), again after each boundary while any remain; texture work with nothing committed
//!   never does.
//! - **Frames are tickets.** Every frame step begins a ledger ticket before the frame is built, attaches the frame's
//!   own command buffer's completion token before the commit, and ends by the frame's fate: committed ->
//!   `submitted`, prepared but not committed -> `not_committed`, abandoned before preparing (no drawable, a nil object,
//!   busy, skipped) -> `discarded`. No path drops a ticket without an ending.
//! - **One proven teardown.** After the driver's `Ending` pass the presenter looks, without waiting and again until the
//!   one absolute stop deadline, at the store and ledger (the worker finished, every submitted upload and frame token
//!   done, no uncertain ticket) **and** at its own drawable and command tokens. Only when both are complete does it
//!   release the store's and ledger's objects, then its own holders, then the queue and device, then the context.
//!   Otherwise it releases nothing device-dependent and ends `Unproven`, naming what is outstanding on either side. A
//!   setup that fails after the store started does the same proof before it releases anything.
//!
//! # The instanced scene
//!
//! A [`MetalFrame`] is a generation, a filter, the legacy quad and a [`MetalSceneSource`]. The source emits the scene's
//! visible quads in painter order **on the render thread at frame build** ([`MetalSceneSource::place`]), so the renderer's
//! bridge only exchanges an `Arc` under its lock; a source that carries its own metrics places with them and may ignore
//! the drawable size it is passed.
//!
//! - **A frame is built through its ticket.** For each [`MetalInstance`] the presenter checks it (finite, inside 0 to 1,
//!   non-empty; an invalid one is skipped and counted and takes no slot), resolves its key through the frame's ticket (the
//!   active revision, pinned once per revision per frame; repeated keys reuse the first answer through a per-frame map, so
//!   the cost is one lookup per drawn quad and no scan of the loaded textures), and writes one 32-byte record into the
//!   **instance buffer**. A key with nothing shown draws the **checker** (a 64 x 64 magenta/black texture of 8 x 8-pixel
//!   cells) over its destination. **Adjacent** instances with the same texture are one instanced draw; nothing is sorted
//!   by texture, so painter order holds. The legacy quad is drawn first, below every scene quad, with its own texture and
//!   sampler.
//! - **The instance buffer** is one shared-storage buffer for `max_quads` instances ([`SceneLimits`]), charged to
//!   `binding_geometry_bytes` at its measured `allocatedSize`; a configuration that does not fit is a setup failure naming
//!   both numbers. One buffer is enough only because the presenter keeps one frame in flight: the previous frame's
//!   **command completion token** is read before the first write of a frame (never a drawable's or an upload's), and a
//!   frame step that finds the buffer in use writes nothing and tries again. Raising the cap needs a buffer per frame in
//!   flight. A source emitting more than `max_quads` draws the first `max_quads` and the rest are counted as dropped.
//! - **The checker's startup contract.** The window opens and the presenter starts at once; the checker is uploaded on the
//!   shared queue with its own completion token, **no frame is committed until it is resident**, and its completion alone
//!   (the store's `completed` wake) makes the waiting first frame run, so the first committed frame already shows
//!   checkers for quads not yet shown. A failed upload is one `Failed` report; a stop while it uploads ends through the
//!   combined proof (its token is part of it). Its bytes are the presenter's baseline, in [`MetalPresenter::stats`], and
//!   never the store's `texture_bytes`, staging leases or prepared window or a ledger charge.
//! - **Blending and colour** are the legacy quad's: `ONE, ONE_MINUS_SRC_ALPHA` over `BGRA8Unorm`, texels unchanged. The
//!   instanced vertex function reads its instance by `first + instance_id` (the batch's first instance is passed
//!   explicitly; a draw's base instance is 0), the fragment function is shared.
//! - **Counters.** [`MetalPresenter::stats`] reports, per committed frame, the latest and the peak of the instances drawn,
//!   draw calls, texture binds, resolved and checker instances, skipped and dropped instances, distinct revisions pinned
//!   and the CPU time spent building and encoding: the ordinary-binding baseline of §11.3.
//! - **Off-screen readback.** [`readback_scene`] renders a clear, a legacy quad and instances through the same layout,
//!   batching, pipeline, samplers and encoding; it is used by the pixel tests and by nothing else.
//!
//! # Off-screen texture uploads
//!
//! [`TextureStore`] separates producer admission from GPU ownership. Cloned [`UploadPort`] handles only
//! enqueue immutable pixel sources into a bounded window; the unique [`TextureOwner`] submits closed upload
//! batches and polls them without waiting. Preparation uses private RGBA8 textures and shared staging
//! buffers. [`TextureStats::texture_bytes_reserved`] reports measured `allocatedSize` charges after allocation; that
//! budget is not a strict process-memory ceiling because one allocation is unmeasured at a time and driver
//! private storage is outside the count. A [`PreparedTexture`] can be dropped after upload completion.
//!
//! [`TextureLedger`] owns revisions through the service verdict and next frame boundary. It resolves
//! active content into ticket-scoped [`FrameTexture`] handles, pins each exact revision until its frame
//! completion token is observed, and retires old textures without waiting. `prove` gathers upload and
//! frame completion evidence; `release` destroys the texture graph only after a complete proof. Its
//! test-only blit reader verifies revision pixels without a window or presenter dependency.
//!
//! ## What was observed on this Mac (regressions, not guarantees)
//!
//! From the research probe of 2026-10-19 (`.pipeline/coordination/runtime-m3/metal-probe/`, macOS 26.7.1,
//! plain and under the Main Thread Checker, identical output): `addPresentedHandler` ran for a drawable that
//! was presented, for one acquired and **discarded** without presenting, and for one presented while the
//! window was **minimised**, with `presentedTime` 0 each time, so the handler is a release signal and not
//! proof of display; `nextDrawable`, and setting `drawableSize`, worked from a non-main thread and the next
//! drawable had the new size; `allowsNextDrawableTimeout` defaults to `YES` and `maximumDrawableCount` to 3.
//! The presenter's tests keep these as regressions. "Queued for presentation" (`FirstFrameQueued`) is not
//! proof that a frame is visible.
//!
//! # The probe
//!
//! [`Device`] is a synchronous, single-thread bootstrap probe, as it always was.

mod batch;
mod bindless;
mod frame;
mod hash;
mod ledger;
mod metrics;
mod presenter;
mod quad;
mod scene;
mod textures;
mod unwind;

pub use bindless::{BindlessLimits, TableView};
#[cfg(feature = "spare-faults")]
pub use ledger::SpareFault;
pub use ledger::{
    FrameTexture, FrameTicket, LedgerCleanup, LedgerError, LedgerEvent, LedgerLimits, LedgerProof,
    LedgerStats, OfferRefusal, RevisionState, TextureLedger,
};
pub use textures::{
    Completed, FrameCommandBuffer, MetalContext, NativeLiveObjects, PreparedTexture, Proof,
    Refused, StallGate, StoreCleanup, TestHooks as TextureTestHooks, TestThreadObservation,
    TextureLimits, TextureOwner, TextureStats, TextureStore, TextureWakes, UploadItem, UploadPort,
};

pub use batch::FrameCounters;
pub use frame::{
    InstanceRole, MetalFrame, MetalFrameSlot, MetalInstance, MetalSceneSource, SceneStats,
};
pub use metrics::{
    BUCKETS, EndError, FrameIntervals, FrameMarks, FrameStats, Histogram, MetricsWindow,
    Percentile, RANGE_END_NS, RequestError, Tracked, Windows, now_ns, seconds_to_ns,
};
pub use presenter::{
    CapturedFrame, CleanupResult, ErrorPoll, Gate, MarksProbe, MetalPresenter, MetricsEndGuard,
    MetricsReading, NeverComplete, Observer, PauseSite, PresenterReport, ReadbackImage,
    ReadbackInstance, ReadbackPath, ReadbackTexture, TestHooks, TimingProbe, TimingReplacement,
    WaitPause, readback_clear, readback_quad, readback_scene, readback_scene_counted,
};
pub use quad::{
    DestinationRect, MAX_IMAGE_BYTES, MAX_IMAGE_SIDE, MetalFilter, MetalQuad, MetalQuadError,
    MetalQuadSlot, MetalScene, PixelSource, SourceRect,
};
pub use scene::{
    Boundary, CHECKER_CELL, CHECKER_SIDE, PassOutcome, SceneLimits, SceneSetup, TextureDriver,
    TicketEnding, checker_pixels,
};

use std::{
    ffi::{CStr, c_char, c_int, c_void},
    marker::PhantomData,
    ptr::NonNull,
    rc::Rc,
};

unsafe extern "C" {
    fn ck_metal_create(
        bytes: *const u8,
        length: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    fn ck_metal_probe(
        handle: *mut c_void,
        output: *mut u32,
        count: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> c_int;
    fn ck_metal_destroy(handle: *mut c_void);
}

/// What the native Metal layer reported, or a bounded fallback when it did not report cleanly.
#[derive(Debug)]
pub struct Error(pub(crate) String);
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}
impl std::error::Error for Error {}

/// The error text when the native layer left no NUL inside the buffer it was given.
const UNTERMINATED: &str = "native error message was not NUL-terminated";

/// Reads the native error message from the buffer the native call filled. The read stops at the
/// first NUL and never goes past the buffer: a buffer with no NUL gets the fixed fallback text.
pub(crate) fn message(buffer: &[u8]) -> Error {
    match CStr::from_bytes_until_nul(buffer) {
        Ok(text) => Error(text.to_string_lossy().into_owned()),
        Err(_) => Error(UNTERMINATED.to_owned()),
    }
}

/// An owned Metal context. Neither Send nor Sync: all calls stay on its creating thread.
pub struct Device {
    handle: NonNull<c_void>,
    _thread: PhantomData<Rc<()>>,
}

impl Device {
    /// Creates the Metal context from the compiled probe library.
    ///
    /// # Errors
    ///
    /// Returns the native layer's message when no Metal device is available or the library
    /// cannot be loaded.
    pub fn new() -> Result<Self, Error> {
        let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/probe.metallib"));
        let mut error = [0_u8; 1024];
        // SAFETY: Both input and error buffers are valid for the synchronous call, and the C
        // side writes at most `error.len()` bytes into the error buffer.
        let handle = unsafe {
            ck_metal_create(
                bytes.as_ptr(),
                bytes.len(),
                error.as_mut_ptr().cast::<c_char>(),
                error.len(),
            )
        };
        NonNull::new(handle)
            .map(|handle| Self {
                handle,
                _thread: PhantomData,
            })
            .ok_or_else(|| message(&error))
    }

    /// Runs the synchronous bootstrap probe and returns its 64 results.
    ///
    /// # Errors
    ///
    /// Returns the native layer's message when the GPU work fails.
    pub fn probe(&mut self) -> Result<[u32; 64], Error> {
        let mut values = [0; 64];
        let mut error = [0_u8; 1024];
        // SAFETY: We exclusively borrow the live context and correctly sized buffers.
        // The native call waits for GPU completion before releasing these borrows.
        let result = unsafe {
            ck_metal_probe(
                self.handle.as_ptr(),
                values.as_mut_ptr(),
                values.len(),
                error.as_mut_ptr().cast::<c_char>(),
                error.len(),
            )
        };
        if result == 0 {
            Ok(values)
        } else {
            Err(message(&error))
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        // SAFETY: This consumes our one retained native reference, once, on the owner thread.
        unsafe { ck_metal_destroy(self.handle.as_ptr()) };
    }
}

#[cfg(test)]
mod tests {
    use super::{UNTERMINATED, message};

    #[test]
    fn a_buffer_without_a_terminator_gives_the_bounded_fallback() {
        let buffer = [b'x'; 1024];
        assert_eq!(message(&buffer).to_string(), UNTERMINATED);
    }

    #[test]
    fn the_message_stops_at_the_first_nul() {
        let mut buffer = [0_u8; 1024];
        buffer[..5].copy_from_slice(b"oops\0");
        buffer[6..10].copy_from_slice(b"junk");
        assert_eq!(message(&buffer).to_string(), "oops");
    }

    #[test]
    fn an_empty_buffer_and_an_empty_message_are_handled() {
        assert_eq!(message(&[]).to_string(), UNTERMINATED);
        assert_eq!(message(&[0]).to_string(), "");
    }
}
