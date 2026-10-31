//! The off-screen texture store: a preparation worker, two leases, pooled image memory, and an owner that
//! alone submits (milestone 5.5, issue 1). Its contract is the Metal store's (`ck-gpu-metal`'s
//! `textures.rs`); the differences are stated where they arise.
//!
//! - **The worker** (`ck-vulkan-upload`) takes offered images in batches, creates each image and its view,
//!   suballocates and binds its memory, copies the pixels into the lease's staging buffer and records the
//!   batch into the lease's one command buffer. It **never** calls `vkQueueSubmit`, writes a descriptor, or
//!   touches a pool it does not own, and it never holds the queue (that is a [`RenderQueue`], which only the
//!   owner has).
//! - **The two leases.** Each owns one command pool, one command buffer, one fence and one persistently
//!   mapped staging buffer; there are two pools in all, never one per image or per batch. A lease belongs to
//!   the worker while a batch is prepared, then to the closed envelope, then to the submitted batch, and
//!   returns to the worker only when the owner's [`poll`](TextureOwner::poll) has seen its fence signalled.
//! - **The owner** ([`TextureOwner`]) submits a closed envelope when none is in flight, and polls the fence
//!   with `vkGetFenceStatus`. There is no completion callback in Vulkan, so there is **no `completed` wake**
//!   (a stated difference from Metal): [`TextureOwner::has_submitted`] says a fence is outstanding, so a
//!   render loop can bound its idle wait.
//! - **Per-item versus store-wide failures.** An image that cannot be placed because of a limit (the
//!   `texture_bytes` ceiling, the allocation count, a size the device cannot take, a bad source) fails alone,
//!   with its reason. A failure of Vulkan itself (an allocation or binding that the device refuses, a submit
//!   that fails, a lost device) is latched once for the whole store: its batch fails, offers are refused, and
//!   the end is proven when nothing was possibly in use.
//! - **Hooks** (the wakes) run after the store's lock is released and must not panic; a panic is caught, its
//!   payload disposed of, and the state being announced stays stored.
//! - **The end** is two steps, as on Metal: [`TextureOwner::prove`] is non-destructive and
//!   [`TextureOwner::release`] destroys only after a complete proof. No `vkDeviceWaitIdle`, ever.

use std::cell::Cell;
use std::collections::VecDeque;
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ash::vk;

use crate::context::{FORMAT, RenderQueue, VulkanContext};
use crate::memory::{
    AllocError, Dedication, ImagePool, MemorySource, Placement, Request, SourceError, align_up,
};
use crate::quad::{MAX_IMAGE_BYTES, MAX_IMAGE_SIDE, PixelSource};
use crate::{Error, unwind};

/// The usage of every store image: filled by a copy, sampled, and read back by tests.
const USAGE: vk::ImageUsageFlags = vk::ImageUsageFlags::from_raw(
    vk::ImageUsageFlags::TRANSFER_DST.as_raw()
        | vk::ImageUsageFlags::SAMPLED.as_raw()
        | vk::ImageUsageFlags::TRANSFER_SRC.as_raw(),
);

/// How long a test readback waits for its one submit.
const READBACK_WAIT_NS: u64 = 5_000_000_000;

/// The number of leases: one batch prepared or closed while another is in flight.
const LEASES: usize = 2;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

type DevicePool = ImagePool<vk::DeviceMemory>;

/// Memory for the image pool, from the context (which counts every allocation).
struct DeviceSource {
    context: Arc<VulkanContext>,
    /// A test's injected refusal: the allocation with this one-based number fails after the budget admitted it.
    fail_at: Option<usize>,
    calls: usize,
}

impl DeviceSource {
    fn new(context: Arc<VulkanContext>) -> Self {
        Self {
            context,
            fail_at: None,
            calls: 0,
        }
    }
}

impl MemorySource for DeviceSource {
    type Handle = vk::DeviceMemory;

    fn allocate(
        &mut self,
        size: u64,
        memory_type: u32,
        dedicated: Option<u64>,
    ) -> Result<vk::DeviceMemory, SourceError> {
        use ash::vk::Handle as _;
        self.calls += 1;
        if self.fail_at == Some(self.calls) {
            return Err(SourceError::Device(
                "injected allocation failure".to_owned(),
            ));
        }
        self.context
            .allocate_memory(size, memory_type, dedicated.map(vk::Image::from_raw))
    }

    fn free(&mut self, handle: vk::DeviceMemory) {
        self.context.free_memory(handle);
    }
}

/// One pixel source and the size it claims.
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

/// Validated upload caps, with Metal's fields, defaults, ranges and cross-field rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureLimits {
    /// The most images offered and not yet completed (the prepared window), 2..=4096.
    pub max_prepared_images: usize,
    /// The most images in one batch, 1..=2048.
    pub max_upload_images_per_batch: usize,
    /// The staging bytes one normal batch may take, 1 MiB..=256 MiB.
    pub max_upload_bytes_per_batch: u64,
    /// The ceiling for the two leases' staging buffers, 128 MiB..=4 GiB.
    pub staging_bytes: u64,
    /// The hard ceiling on the crate's image memory (blocks and dedicated allocations at their queried
    /// sizes), 64 MiB..=64 GiB. Driver-private memory is outside it.
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

/// The cumulative figures the shared metrics record reads for both backends. Nothing else is counted.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextureStats {
    /// Images created for uploads over the store's life (a failed creation counts nothing).
    pub textures_created: u64,
    /// Upload batches submitted.
    pub batches_submitted: u64,
    /// Upload batches whose completion the owner observed.
    pub batches_completed: u64,
    /// The most image memory ever reserved at once (blocks and dedicated allocations at their queried sizes).
    pub texture_bytes_peak: u64,
    /// The most staging memory ever allocated at once (the two leases' buffers).
    pub staging_bytes_peak: u64,
}

/// An item returned by non-blocking polling.
#[derive(Debug)]
pub enum Completed<T> {
    /// The image is uploaded and in `SHADER_READ_ONLY_OPTIMAL`.
    Ready(PreparedTexture<T>),
    /// The item failed alone, or its batch (or the store) failed.
    Failed {
        /// The tag the item was offered with.
        tag: T,
        /// Why.
        reason: String,
    },
}

/// One store image: the image, its view and its range of pooled memory. Dropping it destroys the view and the
/// image and returns the range, which is the only place a range is returned.
pub(crate) struct GpuImage {
    context: Arc<VulkanContext>,
    pool: Arc<Mutex<DevicePool>>,
    image: vk::Image,
    view: vk::ImageView,
    placement: Option<Placement<vk::DeviceMemory>>,
}

impl Drop for GpuImage {
    fn drop(&mut self) {
        let device = self.context.device();
        // SAFETY: the view and the image belong to this device, and every reader of them is proven finished
        // (the store drops an image only on a proven release or when it was never submitted).
        unsafe { device.destroy_image_view(self.view, None) };
        // SAFETY: as above.
        unsafe { device.destroy_image(self.image, None) };
        if let Some(placement) = self.placement.take() {
            let mut source = DeviceSource::new(Arc::clone(&self.context));
            lock(&self.pool).release(&mut source, placement);
        }
    }
}

/// A completed image. Dropping it releases its image and its range.
pub struct PreparedTexture<T> {
    /// The tag the item was offered with.
    pub tag: T,
    /// The width in pixels.
    pub width: u32,
    /// The height in pixels.
    pub height: u32,
    /// The bytes the driver asked for (`VkMemoryRequirements::size`).
    pub allocated_size: u64,
    image: Option<Arc<GpuImage>>,
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
    pub(crate) fn view(&self) -> vk::ImageView {
        self.image
            .as_ref()
            .map_or_else(vk::ImageView::null, |image| image.view)
    }

    pub(crate) fn image(&self) -> vk::Image {
        self.image
            .as_ref()
            .map_or_else(vk::Image::null, |image| image.image)
    }

    /// Reads the image back on `queue`, for tests: a barrier `SHADER_READ_ONLY_OPTIMAL → TRANSFER_SRC_OPTIMAL`,
    /// `vkCmdCopyImageToBuffer`, a barrier back to `SHADER_READ_ONLY_OPTIMAL` and a bounded fence wait. The
    /// production image is read; nothing is copied first.
    ///
    /// # Errors
    ///
    /// The Vulkan failure, or a submit or wait whose outcome is uncertain. In that case the readback's own
    /// objects are kept **and so is everything it may reference**: this texture's image, its range of the pool
    /// and the device owner stay alive for good, even when the texture is dropped afterwards. A failure that
    /// left nothing in flight cleans up and leaves the texture usable.
    #[doc(hidden)]
    pub fn readback(&self, queue: &mut RenderQueue) -> Result<Vec<u8>, Error> {
        let image = self
            .image
            .as_ref()
            .ok_or_else(|| Error("texture was released".to_owned()))?;
        let mut retained = false;
        let outcome = read_image(
            &image.context,
            queue,
            image.image,
            self.width,
            self.height,
            &mut retained,
        );
        if retained {
            // The GPU may still read the image: its destruction (and the return of its range) never happens.
            std::mem::forget(Arc::clone(image));
        }
        outcome
    }
}

/// An offered item returned unchanged when the prepared window has no room or the store is stopping.
pub struct Refused<I>(pub I);

/// Owner notifications run after the store lock has been released and must not block.
///
/// **Liveness precondition:** configured hooks must not panic. A panic is caught and its payload is disposed,
/// and the state being announced remains stored, but that notification is lost and is not retried. An
/// independent owner call or a later event must observe it. There is no `completed` hook: Vulkan has no
/// completion callback, and the owner learns of completion by polling.
#[derive(Clone, Default)]
pub struct TextureWakes {
    /// A closed envelope is ready for [`TextureOwner::submit`].
    pub ready_to_submit: Option<Arc<dyn Fn() + Send + Sync>>,
    /// Room in the prepared window opened up.
    pub room: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// A test's gate: the worker waits until the flag reads true.
#[doc(hidden)]
pub type StallGate = Arc<(Mutex<bool>, Condvar)>;

/// Hidden fault controls, reached only through `start_with_test_hooks`: the reviewed seam, and nothing else.
#[doc(hidden)]
#[derive(Debug, Clone, Default)]
pub struct TestHooks {
    /// The worker waits here after it took a batch's items and its lease, until the flag reads true.
    pub stall_gate: Option<StallGate>,
    /// Set by the worker when it has taken a batch and is waiting at the stall gate: the acknowledgement a test
    /// waits for instead of sleeping.
    pub stall_reached: Option<Arc<std::sync::atomic::AtomicBool>>,
    /// The image allocation with this one-based number fails after the budget admitted it.
    pub fail_allocation_at: Option<usize>,
    /// Lowers the context's allocation-count limit when the store starts.
    pub allocation_count_limit: Option<u32>,
}

/// A host-visible buffer that is mapped for the life of its lease.
#[derive(Clone, Copy)]
struct Staging {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    /// The buffer's size; a multiple of `nonCoherentAtomSize`.
    size: u64,
    /// What the driver allocated, charged to `staging_bytes`.
    allocated: u64,
    ptr: *mut u8,
    coherent: bool,
}

// SAFETY: the pointer maps device memory that only the lease's holder writes; the holder is one thread at a
// time (the worker while it prepares, then nobody writes until the lease returns).
unsafe impl Send for Staging {}

/// One of the two leases. The record in the state is the master: the worker works on a copy and writes back
/// what it changed (the staging buffer), so no handle is ever lost.
#[derive(Clone, Copy)]
struct Lease {
    pool: vk::CommandPool,
    cmd: vk::CommandBuffer,
    fence: vk::Fence,
    staging: Option<Staging>,
}

enum ItemOutcome {
    Ready { image: GpuImage, allocated: u64 },
    Failed(String),
}

struct ItemResult<T> {
    tag: T,
    width: u32,
    height: u32,
    outcome: ItemOutcome,
}

/// A closed batch: its items' outcomes and the lease that holds its recorded commands.
struct Envelope<T> {
    /// The lease index, when the batch has GPU work.
    lease: Option<usize>,
    items: Vec<ItemResult<T>>,
    count: usize,
}

#[expect(
    clippy::struct_excessive_bools,
    reason = "independent lifecycle facts are observed together for the end proof"
)]
struct State<T> {
    waiting: VecDeque<UploadItem<T>>,
    closed: Option<Envelope<T>>,
    submitted: Option<Envelope<T>>,
    immediate: VecDeque<Completed<T>>,
    stopping: bool,
    failed: Option<String>,
    failure_taken: bool,
    occupied: usize,
    worker_finished: bool,
    worker_panic: Option<String>,
    /// Batches that were never submitted or that were left behind by a failure: kept until the end.
    orphaned: Vec<Envelope<T>>,
    /// A batch whose submission may have reached the GPU (device loss): its objects are not safe to destroy.
    uncertain: bool,
    leases: Vec<Lease>,
    /// The leases the worker may take.
    free: Vec<usize>,
    /// The leases' staging allocations are destroyed.
    destroyed: bool,
    max: usize,
}

struct Shared<T> {
    state: Mutex<State<T>>,
    changed: Condvar,
    stats: Mutex<TextureStats>,
    wakes: TextureWakes,
    capacity: usize,
    context: Arc<VulkanContext>,
    images: Arc<Mutex<DevicePool>>,
    hooks: TestHooks,
}

/// A cloneable non-blocking producer handle.
pub struct UploadPort<T> {
    shared: Arc<Shared<T>>,
}

/// The unique owner-thread submission, polling and shutdown handle. It is `Send`, not `Sync` or `Clone`.
pub struct TextureOwner<T: Copy + Eq + std::hash::Hash + Send + 'static> {
    shared: Arc<Shared<T>>,
    context: Option<Arc<VulkanContext>>,
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
    /// Whether this owner observed the worker's exit and every submitted completion before its deadline.
    #[must_use]
    pub fn is_complete(self) -> bool {
        self.complete
    }
}

/// The outcome of a complete end proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreCleanup {
    /// Everything was proven finished and destroyed.
    Proven,
    /// Something remained outstanding; everything is retained.
    Unproven {
        /// Why.
        reason: String,
    },
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
        lock(&self.shared.state).room()
    }

    /// Offers without waiting. A refusal returns the exact input item to its caller.
    ///
    /// # Errors
    ///
    /// Returns the unchanged item when the prepared window is full, stopping, or failed.
    pub fn offer(&self, item: UploadItem<T>) -> Result<(), Refused<UploadItem<T>>> {
        let mut state = lock(&self.shared.state);
        if state.stopping || state.failed.is_some() || state.occupied >= self.shared.capacity {
            return Err(Refused(item));
        }
        state.occupied += 1;
        state.waiting.push_back(item);
        drop(state);
        self.shared.changed.notify_all();
        Ok(())
    }
}

impl<T> State<T> {
    fn room(&self) -> usize {
        self.max.saturating_sub(self.occupied)
    }
}

/// Whether a submit failure left the batch possibly in flight (anything but an out-of-memory refusal, which
/// Vulkan documents as leaving the resources untouched).
fn is_uncertain(code: vk::Result) -> bool {
    !matches!(
        code,
        vk::Result::ERROR_OUT_OF_HOST_MEMORY | vk::Result::ERROR_OUT_OF_DEVICE_MEMORY
    )
}

impl<T> TextureOwner<T>
where
    T: Copy + Eq + std::hash::Hash + Send + 'static,
{
    /// Non-blocking submit through `queue`: the closed envelope, when no batch is in flight.
    pub fn submit(&mut self, queue: &mut RenderQueue) -> bool {
        let (envelope, lease) = {
            let mut state = lock(&self.shared.state);
            if state.failed.is_some() || state.submitted.is_some() {
                return false;
            }
            let Some(envelope) = state.closed.take() else {
                return false;
            };
            let lease = envelope
                .lease
                .and_then(|index| state.leases.get(index).copied());
            (envelope, lease)
        };
        let result = match lease {
            Some(lease) => {
                let buffers = [lease.cmd];
                let info = vk::SubmitInfo::default().command_buffers(&buffers);
                queue.submit(&info, lease.fence)
            }
            None => Ok(()),
        };
        let mut state = lock(&self.shared.state);
        match result {
            Ok(()) => {
                if lease.is_some() {
                    lock(&self.shared.stats).batches_submitted += 1;
                }
                state.submitted = Some(envelope);
                self.shared.changed.notify_all();
                true
            }
            Err(code) => {
                let reason = format!("vkQueueSubmit failed: {code:?}");
                if is_uncertain(code) {
                    state.uncertain = true;
                }
                fail_envelope(&mut state, envelope, &reason);
                self.shared.changed.notify_all();
                drop(state);
                notify_wake(self.shared.wakes.room.as_ref());
                false
            }
        }
    }

    /// Polls completions without waiting and appends all finished item outcomes to `out`.
    pub fn poll(&mut self, out: &mut Vec<Completed<T>>) {
        let mut state = lock(&self.shared.state);
        while let Some(item) = state.immediate.pop_front() {
            out.push(item);
        }
        let Some(envelope) = state.submitted.as_ref() else {
            return;
        };
        let lease = envelope
            .lease
            .and_then(|index| state.leases.get(index).copied());
        if let Some(lease) = lease {
            let device = self.shared.context.device();
            // SAFETY: the fence belongs to this device and was submitted with the batch.
            match unsafe { device.get_fence_status(lease.fence) } {
                Ok(false) => return,
                Ok(true) => {
                    // SAFETY: the fence is signalled and no queue operation waits on it any more.
                    if let Err(code) = unsafe { device.reset_fences(&[lease.fence]) } {
                        state
                            .failed
                            .get_or_insert_with(|| format!("vkResetFences failed: {code:?}"));
                        state.stopping = true;
                    }
                }
                Err(code) => {
                    // The batch's fate is unknown: it stays where it is, uncertain, and the store stops.
                    let reason = format!("vkGetFenceStatus failed: {code:?}");
                    state.failed.get_or_insert_with(|| reason.clone());
                    state.stopping = true;
                    state.uncertain = true;
                    fail_waiting(&mut state, &reason);
                    if let Some(closed) = state.closed.take() {
                        fail_closed(&mut state, closed, &reason);
                    }
                    if let Some(submitted) = state.submitted.take() {
                        for item in &submitted.items {
                            let item_reason = match &item.outcome {
                                ItemOutcome::Ready { .. } => reason.clone(),
                                ItemOutcome::Failed(item_reason) => item_reason.clone(),
                            };
                            state.immediate.push_back(Completed::Failed {
                                tag: item.tag,
                                reason: item_reason,
                            });
                        }
                        state.occupied = state.occupied.saturating_sub(submitted.count);
                        state.orphaned.push(submitted);
                    }
                    while let Some(item) = state.immediate.pop_front() {
                        out.push(item);
                    }
                    self.shared.changed.notify_all();
                    drop(state);
                    notify_wake(self.shared.wakes.room.as_ref());
                    return;
                }
            }
        }
        let Some(mut envelope) = state.submitted.take() else {
            return;
        };
        let returned = envelope.count;
        for item in envelope.items.drain(..) {
            match item.outcome {
                ItemOutcome::Ready { image, allocated } => {
                    out.push(Completed::Ready(PreparedTexture {
                        tag: item.tag,
                        width: item.width,
                        height: item.height,
                        allocated_size: allocated,
                        image: Some(Arc::new(image)),
                    }));
                }
                ItemOutcome::Failed(reason) => out.push(Completed::Failed {
                    tag: item.tag,
                    reason,
                }),
            }
        }
        if let Some(index) = envelope.lease {
            state.free.push(index);
            lock(&self.shared.stats).batches_completed += 1;
        }
        state.occupied = state.occupied.saturating_sub(returned);
        drop(state);
        self.shared.changed.notify_all();
        notify_wake(self.shared.wakes.room.as_ref());
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

    /// Looks, without taking anything, for a latched GPU failure (the teardown reports it before the combined
    /// proof).
    #[must_use]
    pub fn peek_failure(&self) -> Option<String> {
        lock(&self.shared.state).failed.clone()
    }

    /// Whether a batch is submitted and not yet seen finished: a fence is outstanding, so a render loop can
    /// bound its idle wait.
    #[must_use]
    pub fn has_submitted(&self) -> bool {
        lock(&self.shared.state).submitted.is_some()
    }

    /// Whether a prepared, closed envelope waits for the owner to submit it (tests only: a phase handshake that
    /// reads, and takes nothing).
    #[doc(hidden)]
    #[must_use]
    pub fn envelope_ready_unsubmitted(&self) -> bool {
        lock(&self.shared.state).closed.is_some()
    }

    /// Waits, for tests, until the submitted batch's fence has signalled or `timeout` passes. It only reads
    /// the fence: nothing is polled, reset or released. `true` when the GPU finished the batch (or none is in
    /// flight).
    #[doc(hidden)]
    #[must_use]
    pub fn test_wait_submitted(&self, timeout: Duration) -> bool {
        let state = lock(&self.shared.state);
        let Some(lease) = state
            .submitted
            .as_ref()
            .and_then(|envelope| envelope.lease)
            .and_then(|index| state.leases.get(index).copied())
        else {
            return true;
        };
        let nanoseconds = u64::try_from(timeout.as_nanos()).unwrap_or(u64::MAX);
        // SAFETY: the fence belongs to this device. The lock is held so the batch cannot be released
        // under the wait; only this owner polls, and it is the caller.
        unsafe {
            self.shared
                .context
                .device()
                .wait_for_fences(&[lease.fence], true, nanoseconds)
        }
        .is_ok()
    }

    /// The command pool, command buffer and fence of each lease (tests only).
    #[doc(hidden)]
    #[must_use]
    pub fn lease_objects(&self) -> Vec<(vk::CommandPool, vk::CommandBuffer, vk::Fence)> {
        lock(&self.shared.state)
            .leases
            .iter()
            .map(|lease| (lease.pool, lease.cmd, lease.fence))
            .collect()
    }

    /// The command buffer of the submitted batch (tests only).
    #[doc(hidden)]
    #[must_use]
    pub fn submitted_command_buffer(&self) -> Option<vk::CommandBuffer> {
        let state = lock(&self.shared.state);
        state
            .submitted
            .as_ref()
            .and_then(|envelope| envelope.lease)
            .and_then(|index| state.leases.get(index))
            .map(|lease| lease.cmd)
    }

    /// The images the store itself still holds in its own maps: in a closed, submitted or orphaned batch
    /// (tests only: a read of the live maps, not a counter).
    #[doc(hidden)]
    #[must_use]
    pub fn held_images(&self) -> usize {
        let state = lock(&self.shared.state);
        let count = |envelope: &Envelope<T>| {
            envelope
                .items
                .iter()
                .filter(|item| matches!(item.outcome, ItemOutcome::Ready { .. }))
                .count()
        };
        state.closed.iter().map(count).sum::<usize>()
            + state.submitted.iter().map(count).sum::<usize>()
            + state.orphaned.iter().map(count).sum::<usize>()
    }

    /// A snapshot of the cumulative figures.
    #[must_use]
    pub fn stats(&self) -> TextureStats {
        let mut stats = lock(&self.shared.stats).clone();
        stats.texture_bytes_peak = lock(&self.shared.images).peak();
        stats
    }

    /// Performs the non-destructive worker and completion proof against one absolute deadline.
    pub fn prove(&mut self, deadline: Instant) -> Proof {
        let incomplete = Proof {
            owner_id: self.owner_id,
            complete: false,
        };
        {
            let mut state = lock(&self.shared.state);
            state.stopping = true;
            fail_waiting(&mut state, "store stopping");
            if let Some(envelope) = state.closed.take() {
                fail_closed(&mut state, envelope, "store stopping");
            }
            self.shared.changed.notify_all();
        }
        loop {
            let state = lock(&self.shared.state);
            if state.worker_finished {
                break;
            }
            let now = Instant::now();
            if now >= deadline {
                return incomplete;
            }
            let wait = deadline
                .saturating_duration_since(now)
                .min(Duration::from_millis(2));
            let (next, _) = self
                .shared
                .changed
                .wait_timeout(state, wait)
                .unwrap_or_else(PoisonError::into_inner);
            drop(next);
        }
        if let Some(worker) = self.worker.take() {
            unwind::join(worker);
        }
        let (panicked, uncertain, lease) = {
            let state = lock(&self.shared.state);
            (
                state.worker_panic.is_some(),
                state.uncertain,
                state
                    .submitted
                    .as_ref()
                    .and_then(|envelope| envelope.lease)
                    .and_then(|index| state.leases.get(index).copied()),
            )
        };
        if panicked || uncertain {
            return incomplete;
        }
        if let Some(lease) = lease {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let nanoseconds = u64::try_from(remaining.as_nanos()).unwrap_or(u64::MAX);
            // SAFETY: the fence belongs to this device; the wait is bounded by the deadline.
            let waited = unsafe {
                self.shared
                    .context
                    .device()
                    .wait_for_fences(&[lease.fence], true, nanoseconds)
            };
            if waited.is_err() {
                return incomplete;
            }
        }
        Proof {
            owner_id: self.owner_id,
            complete: true,
        }
    }

    /// Destroys the whole graph, only after this owner's complete proof: the images of every batch, the
    /// leases' staging buffers, fences and command pools, and any block that is still held.
    pub fn release(&mut self, proof: Proof) {
        if proof.owner_id != self.owner_id || !proof.complete || self.released {
            return;
        }
        let mut state = lock(&self.shared.state);
        let mut batches: Vec<Envelope<T>> = Vec::new();
        batches.extend(state.closed.take());
        batches.extend(state.submitted.take());
        batches.append(&mut state.orphaned);
        state.immediate.clear();
        state.waiting.clear();
        state.occupied = 0;
        // The images go first (their ranges return to the pool), then the leases' objects.
        drop(state);
        drop(batches);
        let mut state = lock(&self.shared.state);
        let device = self.shared.context.device();
        for lease in state.leases.drain(..) {
            destroy_staging(&self.shared.context, lease.staging);
            // SAFETY: the lease's batch is proven finished (or was never submitted), so nothing uses them.
            unsafe { device.destroy_fence(lease.fence, None) };
            // SAFETY: as above; destroying the pool frees its command buffer.
            unsafe { device.destroy_command_pool(lease.pool, None) };
        }
        state.free.clear();
        state.destroyed = true;
        drop(state);
        // The pooled blocks are not freed here: a texture the store handed out (and that is still alive) owns
        // its range, keeps the pool and the context alive, and returns the range, and so the block, when it is
        // dropped. A block with no range left has already been freed.
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
                // Retaining the shared graph and the context to process exit is the specified unproven-end
                // policy: nothing is destroyed under work that may still be running.
                std::mem::forget(Arc::clone(&self.shared));
                if let Some(context) = self.context.take() {
                    std::mem::forget(context);
                }
            }
        }
    }
}

/// Latches `reason` for the store, fails the envelope's items and everything still waiting, and keeps the
/// envelope for the end.
fn fail_envelope<T: Copy>(state: &mut State<T>, envelope: Envelope<T>, reason: &str) {
    state.failed.get_or_insert_with(|| reason.to_owned());
    state.stopping = true;
    fail_closed(state, envelope, reason);
    fail_waiting(state, reason);
    if let Some(closed) = state.closed.take() {
        fail_closed(state, closed, reason);
    }
}

/// Fails every item still waiting.
fn fail_waiting<T>(state: &mut State<T>, reason: &str) {
    while let Some(item) = state.waiting.pop_front() {
        state.immediate.push_back(Completed::Failed {
            tag: item.tag,
            reason: reason.to_owned(),
        });
        state.occupied = state.occupied.saturating_sub(1);
    }
}

/// Fails the items of a closed envelope that will never be submitted, and keeps it for the end.
fn fail_closed<T: Copy>(state: &mut State<T>, envelope: Envelope<T>, reason: &str) {
    for item in &envelope.items {
        let item_reason = match &item.outcome {
            ItemOutcome::Ready { .. } => reason.to_owned(),
            ItemOutcome::Failed(item_reason) => item_reason.clone(),
        };
        state.immediate.push_back(Completed::Failed {
            tag: item.tag,
            reason: item_reason,
        });
    }
    state.occupied = state.occupied.saturating_sub(envelope.count);
    state.orphaned.push(envelope);
}

/// Starts a worker that prepares image upload batches.
pub struct TextureStore;

impl TextureStore {
    /// Starts the worker and returns the producer and unique owner handles.
    ///
    /// # Errors
    ///
    /// Returns invalid limits, a lease object that could not be created, or an upload-worker spawn error.
    pub fn start<T>(
        context: Arc<VulkanContext>,
        limits: TextureLimits,
        wakes: TextureWakes,
    ) -> Result<(UploadPort<T>, TextureOwner<T>), Error>
    where
        T: Copy + Eq + std::hash::Hash + Send + 'static,
    {
        Self::start_inner(context, limits, wakes, TestHooks::default())
    }

    /// Test-only fault boundary. Production paths never call this constructor.
    ///
    /// # Errors
    ///
    /// As [`start`](Self::start).
    #[doc(hidden)]
    pub fn start_with_test_hooks<T>(
        context: Arc<VulkanContext>,
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
        context: Arc<VulkanContext>,
        limits: TextureLimits,
        wakes: TextureWakes,
        hooks: TestHooks,
    ) -> Result<(UploadPort<T>, TextureOwner<T>), Error>
    where
        T: Copy + Eq + std::hash::Hash + Send + 'static,
    {
        let limits = limits.validate()?;
        if let Some(count) = hooks.allocation_count_limit {
            context.lower_allocation_limit(count);
        }
        let leases = create_leases(&context)?;
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
                uncertain: false,
                free: (0..leases.len()).collect(),
                leases,
                destroyed: false,
                max: limits.max_prepared_images,
            }),
            changed: Condvar::new(),
            stats: Mutex::new(TextureStats::default()),
            wakes,
            capacity: limits.max_prepared_images,
            images: Arc::new(Mutex::new(ImagePool::new(limits.texture_bytes))),
            context: Arc::clone(&context),
            hooks,
        });
        let worker_shared = Arc::clone(&shared);
        let worker = thread::Builder::new()
            .name("ck-vulkan-upload".to_owned())
            .spawn(move || worker_main(&worker_shared, limits))
            .map_err(|error| {
                destroy_leases(&shared);
                Error(format!("upload worker spawn failed: {error}"))
            })?;
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

/// The two leases: a command pool, one command buffer and one unsignalled fence each. The staging buffers
/// come later, when a batch needs them.
fn create_leases(context: &Arc<VulkanContext>) -> Result<Vec<Lease>, Error> {
    let device = context.device();
    let mut leases: Vec<Lease> = Vec::with_capacity(LEASES);
    let undo = |leases: &[Lease]| {
        for lease in leases {
            // SAFETY: nothing was submitted; these were just created.
            unsafe { device.destroy_fence(lease.fence, None) };
            // SAFETY: as above.
            unsafe { device.destroy_command_pool(lease.pool, None) };
        }
    };
    for _ in 0..LEASES {
        let pool_info = vk::CommandPoolCreateInfo::default().queue_family_index(context.family());
        // SAFETY: the device is live and `pool_info` is valid for the call.
        let pool = match unsafe { device.create_command_pool(&pool_info, None) } {
            Ok(pool) => pool,
            Err(code) => {
                undo(&leases);
                return Err(crate::error::vk_error("vkCreateCommandPool", code));
            }
        };
        let allocate = vk::CommandBufferAllocateInfo::default()
            .command_pool(pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: the pool is live and `allocate` is valid for the call.
        let cmd = match unsafe { device.allocate_command_buffers(&allocate) } {
            Ok(buffers) => buffers[0],
            Err(code) => {
                // SAFETY: nothing was submitted.
                unsafe { device.destroy_command_pool(pool, None) };
                undo(&leases);
                return Err(crate::error::vk_error("vkAllocateCommandBuffers", code));
            }
        };
        // SAFETY: the device is live.
        let fence = match unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) } {
            Ok(fence) => fence,
            Err(code) => {
                // SAFETY: nothing was submitted.
                unsafe { device.destroy_command_pool(pool, None) };
                undo(&leases);
                return Err(crate::error::vk_error("vkCreateFence", code));
            }
        };
        leases.push(Lease {
            pool,
            cmd,
            fence,
            staging: None,
        });
    }
    Ok(leases)
}

/// Destroys the leases of a store whose worker never started.
fn destroy_leases<T>(shared: &Shared<T>) {
    let mut state = lock(&shared.state);
    let device = shared.context.device();
    for lease in state.leases.drain(..) {
        destroy_staging(&shared.context, lease.staging);
        // SAFETY: nothing was ever submitted.
        unsafe { device.destroy_fence(lease.fence, None) };
        // SAFETY: as above.
        unsafe { device.destroy_command_pool(lease.pool, None) };
    }
}

/// Unmaps and destroys a staging buffer and frees its memory.
fn destroy_staging(context: &Arc<VulkanContext>, staging: Option<Staging>) {
    let Some(staging) = staging else {
        return;
    };
    let device = context.device();
    // SAFETY: the memory is mapped by this store and nothing uses the buffer (its batch is proven finished
    // or was never submitted).
    unsafe { device.unmap_memory(staging.memory) };
    // SAFETY: as above.
    unsafe { device.destroy_buffer(staging.buffer, None) };
    context.free_memory(staging.memory);
}

/// Creates a mapped staging buffer of at least `size` bytes.
/// Why a staging buffer could not be made. Only `Native` is a failure of Vulkan itself.
enum StagingError {
    /// The `staging_bytes` ceiling would be passed by the queried allocation size.
    Budget(String),
    /// A limit (the allocation count): this batch's items fail alone, the store goes on.
    Resource(String),
    /// Vulkan refused: the store latches.
    Native(String),
}

fn create_staging(
    context: &Arc<VulkanContext>,
    size: u64,
    admit: &dyn Fn(u64) -> Result<(), String>,
) -> Result<Staging, StagingError> {
    let device = context.device();
    let atom = context.limits().non_coherent_atom_size;
    let size = align_up(size.max(1), atom)
        .ok_or_else(|| StagingError::Native("staging size overflows".to_owned()))?;
    let info = vk::BufferCreateInfo::default()
        .size(size)
        .usage(vk::BufferUsageFlags::TRANSFER_SRC)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: the device is live and `info` is valid for the call.
    let buffer = unsafe { device.create_buffer(&info, None) }.map_err(|code| {
        StagingError::Native(format!(
            "vkCreateBuffer(staging, {size} bytes) failed: {code:?}"
        ))
    })?;
    // SAFETY: the buffer was just created on this device.
    let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
    let Some(memory_type) = context.memory_type(
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE,
        vk::MemoryPropertyFlags::HOST_COHERENT,
    ) else {
        // SAFETY: the buffer is unbound and unused.
        unsafe { device.destroy_buffer(buffer, None) };
        return Err(StagingError::Native(
            "no host-visible memory type for the staging buffer".to_owned(),
        ));
    };
    // Admission is by the size the driver says the allocation takes, queried before `vkAllocateMemory`: the
    // allocation never passes the `staging_bytes` ceiling, not even for a moment.
    if let Err(message) = admit(requirements.size) {
        // SAFETY: the buffer is unbound and unused.
        unsafe { device.destroy_buffer(buffer, None) };
        return Err(StagingError::Budget(message));
    }
    let memory = match context.allocate_memory(requirements.size, memory_type, None) {
        Ok(memory) => memory,
        Err(error) => {
            // SAFETY: the buffer is unbound and unused.
            unsafe { device.destroy_buffer(buffer, None) };
            return Err(match error {
                SourceError::Count { in_use, limit } => StagingError::Resource(format!(
                    "allocation count limit: {in_use} of {limit} memory allocations are in use (a staging buffer needs one)"
                )),
                SourceError::Device(message) => StagingError::Native(message),
            });
        }
    };
    // SAFETY: the memory is unbound and large enough for the buffer.
    if let Err(code) = unsafe { device.bind_buffer_memory(buffer, memory, 0) } {
        // SAFETY: the buffer is unused.
        unsafe { device.destroy_buffer(buffer, None) };
        context.free_memory(memory);
        return Err(StagingError::Native(format!(
            "vkBindBufferMemory(staging) failed: {code:?}"
        )));
    }
    // SAFETY: the memory is host-visible and not mapped.
    let ptr = match unsafe {
        device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
    } {
        Ok(ptr) => ptr.cast::<u8>(),
        Err(code) => {
            // SAFETY: the buffer is unused.
            unsafe { device.destroy_buffer(buffer, None) };
            context.free_memory(memory);
            return Err(StagingError::Native(format!(
                "vkMapMemory(staging) failed: {code:?}"
            )));
        }
    };
    Ok(Staging {
        buffer,
        memory,
        size,
        allocated: requirements.size,
        ptr,
        coherent: context.is_coherent(memory_type),
    })
}

/// Calls a wake hook, containing a panic: the payload is read and disposed of without unwinding, even one
/// whose `Drop` panics. Returns normally in every case.
pub(crate) fn notify_wake(wake: Option<&Arc<dyn Fn() + Send + Sync>>) {
    if let Some(wake) = wake
        && let Err(payload) = catch_unwind(AssertUnwindSafe(|| wake()))
    {
        unwind::dispose(payload);
    }
}

/// Whether a staging allocation of `requested` bytes (the queried `VkMemoryRequirements::size`) may be made while
/// the other lease holds `other` allocated bytes, under the `staging_bytes` ceiling.
fn staging_admission(other: u64, requested: u64, limit: u64) -> Result<(), String> {
    match other.checked_add(requested) {
        Some(total) if total <= limit => Ok(()),
        _ => Err(format!(
            "staging allocation of {requested} bytes would take the two leases past staging_bytes ({other} held, {limit} allowed)"
        )),
    }
}

/// The offset alignment of an image inside a staging buffer: `max(4, optimalBufferCopyOffsetAlignment)`.
fn offset_alignment(context: &VulkanContext) -> u64 {
    context.limits().optimal_buffer_copy_offset_alignment.max(4)
}

/// The bytes of an image's rows, tightly packed, and the staging bytes of an item.
fn image_bytes(item: &UploadItem<impl Sized>) -> Option<u64> {
    u64::from(item.width)
        .checked_mul(u64::from(item.height))?
        .checked_mul(4)
}

/// The staging bytes of a batch: each image at an aligned offset, the total rounded to the atom size.
/// Items that fail validation take none.
fn staging_need<'a, T: 'a>(
    items: impl Iterator<Item = &'a UploadItem<T>>,
    alignment: u64,
    atom: u64,
) -> u64 {
    staging_layout(
        items.filter(|item| validate_item(item).is_ok()),
        alignment,
        atom,
    )
    .1
}

/// The offsets of each image in the staging buffer and the buffer's total need.
fn staging_layout<'a, T: 'a>(
    items: impl Iterator<Item = &'a UploadItem<T>>,
    alignment: u64,
    atom: u64,
) -> (Vec<u64>, u64) {
    let mut offsets = Vec::new();
    let mut end = 0_u64;
    for item in items {
        let bytes = image_bytes(item).unwrap_or(u64::MAX);
        let offset = align_up(end, alignment).unwrap_or(u64::MAX);
        offsets.push(offset);
        end = offset.saturating_add(bytes);
    }
    let total = if end == 0 {
        0
    } else {
        align_up(end, atom).unwrap_or(u64::MAX)
    };
    (offsets, total)
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

fn worker_main<T>(shared: &Arc<Shared<T>>, limits: TextureLimits)
where
    T: Copy + Eq + std::hash::Hash + Send + 'static,
{
    let outcome = catch_unwind(AssertUnwindSafe(|| worker_loop(shared, limits)));
    if let Err(payload) = outcome {
        let reason = unwind::message(&*payload);
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

/// What preparing one image came to.
enum Prepared {
    Ready {
        image: GpuImage,
        allocated: u64,
    },
    /// This image alone failed, for a limit.
    Item(String),
    /// Vulkan refused: the store latches.
    Native(String),
}

#[expect(
    clippy::too_many_lines,
    reason = "one upload pass keeps its lease and its batch transitions auditable together"
)]
fn worker_loop<T>(shared: &Arc<Shared<T>>, limits: TextureLimits)
where
    T: Copy + Eq + std::hash::Hash + Send + 'static,
{
    let context = &shared.context;
    let alignment = offset_alignment(context);
    let atom = context.limits().non_coherent_atom_size;
    let mut source = DeviceSource::new(Arc::clone(context));
    source.fail_at = shared.hooks.fail_allocation_at;
    loop {
        let (mut items, index) = {
            let mut state = lock(&shared.state);
            while !state.stopping
                && (state.closed.is_some() || state.waiting.is_empty() || state.free.is_empty())
            {
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner);
            }
            if state.stopping {
                break;
            }
            let Some(first) = state.waiting.pop_front() else {
                continue;
            };
            let mut batch = vec![first];
            let first_is_oversized =
                staging_need(batch.iter(), alignment, atom) > limits.max_upload_bytes_per_batch;
            while !first_is_oversized && batch.len() < limits.max_upload_images_per_batch {
                let Some(next) = state.waiting.front() else {
                    break;
                };
                let need = staging_need(batch.iter().chain(std::iter::once(next)), alignment, atom);
                if need > limits.max_upload_bytes_per_batch {
                    break;
                }
                if let Some(next) = state.waiting.pop_front() {
                    batch.push(next);
                }
            }
            let index = state.free.pop().unwrap_or(0);
            (batch, index)
        };

        if let Some(gate) = &shared.hooks.stall_gate {
            if let Some(reached) = &shared.hooks.stall_reached {
                reached.store(true, Ordering::SeqCst);
            }
            let (mutex, changed) = &**gate;
            let mut released = lock(mutex);
            while !*released {
                released = changed
                    .wait(released)
                    .unwrap_or_else(PoisonError::into_inner);
            }
        }

        // Each source is classified once, and that result is kept: a `PixelSource` may answer differently
        // when asked again, so nothing below repeats the question.
        let valid: Vec<Result<usize, String>> = items.iter().map(validate_item).collect();
        let (offsets, need) = staging_layout(
            items
                .iter()
                .zip(&valid)
                .filter_map(|(item, checked)| checked.is_ok().then_some(item)),
            alignment,
            atom,
        );
        let mut native_failure: Option<String> = None;
        let mut resource_failure: Option<String> = None;
        let mut lease = lock(&shared.state).leases[index];
        if need != 0 {
            match ensure_staging(shared, index, &mut lease, need, limits) {
                Ok(()) => {}
                Err(StagingError::Native(reason)) => native_failure = Some(reason),
                Err(StagingError::Budget(reason) | StagingError::Resource(reason)) => {
                    resource_failure = Some(reason);
                }
            }
        }

        let mut results: Vec<ItemResult<T>> = Vec::with_capacity(items.len());
        let mut copies: Vec<(vk::Image, u64, u32, u32)> = Vec::new();
        let mut valid_index = 0;
        for (position, item) in items.drain(..).enumerate() {
            let (width, height, tag) = (item.width, item.height, item.tag);
            let outcome = |outcome| ItemResult {
                tag,
                width,
                height,
                outcome,
            };
            if let Some(reason) = &resource_failure {
                results.push(outcome(ItemOutcome::Failed(reason.clone())));
                continue;
            }
            if let Some(reason) = &native_failure {
                results.push(outcome(ItemOutcome::Failed(reason.clone())));
                continue;
            }
            if let Err(reason) = &valid[position] {
                results.push(outcome(ItemOutcome::Failed(reason.clone())));
                continue;
            }
            let offset = offsets[valid_index];
            valid_index += 1;
            if width.max(height) > context.limits().max_image_dimension_2d {
                results.push(outcome(ItemOutcome::Failed(format!(
                    "image {width}x{height} exceeds the device's maxImageDimension2D {}",
                    context.limits().max_image_dimension_2d
                ))));
                continue;
            }
            match create_gpu_image(shared, &mut source, width, height) {
                Prepared::Item(reason) => results.push(outcome(ItemOutcome::Failed(reason))),
                Prepared::Native(reason) => {
                    native_failure = Some(reason.clone());
                    results.push(outcome(ItemOutcome::Failed(reason)));
                }
                Prepared::Ready { image, allocated } => {
                    lock(&shared.stats).textures_created += 1;
                    if let Some(staging) = lease.staging {
                        // The slice is fetched once, and the length and the range are checked on this very
                        // slice: a source whose `pixels()` answers differently from the call that validated it
                        // can fail its item, never write outside the staging range.
                        let pixels = item.pixels.pixels();
                        let expected = image_bytes(&item);
                        let fits = expected == u64::try_from(pixels.len()).ok()
                            && expected.is_some_and(|bytes| {
                                offset
                                    .checked_add(bytes)
                                    .is_some_and(|end| end <= staging.size)
                            });
                        if !fits {
                            drop(image);
                            results.push(outcome(ItemOutcome::Failed(format!(
                                "the pixel source changed length after it was checked ({} bytes, expected {})",
                                pixels.len(),
                                expected.map_or_else(|| "overflow".to_owned(), |bytes| bytes.to_string())
                            ))));
                            continue;
                        }
                        let offset_bytes = usize::try_from(offset).unwrap_or(0);
                        // SAFETY: the staging buffer is mapped and `offset + pixels.len()` is within its
                        // `size` (checked just above, on this slice); this worker holds the lease, and the GPU
                        // is not using it (its fence was seen signalled when the lease returned).
                        let destination = unsafe { staging.ptr.add(offset_bytes) };
                        // SAFETY: as above: the destination range is inside the mapped staging buffer, and the
                        // source slice is a different allocation.
                        unsafe {
                            std::ptr::copy_nonoverlapping(
                                pixels.as_ptr(),
                                destination,
                                pixels.len(),
                            );
                        }
                    }
                    copies.push((image.image, offset, width, height));
                    results.push(outcome(ItemOutcome::Ready { image, allocated }));
                }
            }
            drop(item.pixels);
        }

        let mut recorded = false;
        if native_failure.is_none() && !copies.is_empty() {
            match record_batch(shared, &lease, &copies, need) {
                Ok(()) => recorded = true,
                Err(reason) => native_failure = Some(reason),
            }
        }
        let count = results.len();
        let lease_index = recorded.then_some(index);
        if let Some(reason) = native_failure {
            latch_prepared_failure(shared, results, &reason, index, lease_index);
            continue;
        }
        let mut state = lock(&shared.state);
        if !recorded {
            state.free.push(index);
        }
        if state.stopping {
            let reason = state
                .failed
                .clone()
                .unwrap_or_else(|| "store stopping".to_owned());
            let envelope = Envelope {
                lease: lease_index,
                items: results,
                count,
            };
            fail_closed(&mut state, envelope, &reason);
            shared.changed.notify_all();
            drop(state);
            notify_wake(shared.wakes.room.as_ref());
            continue;
        }
        state.closed = Some(Envelope {
            lease: lease_index,
            items: results,
            count,
        });
        drop(state);
        notify_wake(shared.wakes.ready_to_submit.as_ref());
    }
}

/// What to do about a staging allocation the `staging_bytes` ceiling refused.
#[derive(Debug, PartialEq, Eq)]
enum StagingStep {
    /// Free the other lease's idle buffer (its lease is not in use) and try again.
    FreeIdle,
    /// The other lease is in flight: wait for it to return, then try again.
    Wait,
    /// Nothing left to free or wait for: this batch's items fail alone with the reason.
    Refuse,
}

/// The step after a budget refusal, given what the other lease holds.
fn staging_next_step(other_idle_bytes: u64, other_busy: bool, stopping: bool) -> StagingStep {
    if other_idle_bytes > 0 {
        StagingStep::FreeIdle
    } else if other_busy && !stopping {
        StagingStep::Wait
    } else {
        StagingStep::Refuse
    }
}

/// Makes sure the lease's staging buffer holds `need` bytes, reallocating only when it does not. The master
/// record in the state is updated as soon as the new buffer exists. The `staging_bytes` ceiling is checked
/// against the queried allocation size before the allocation; a refusal frees the other lease's idle buffer, or
/// waits for the other lease to return, before it fails the batch alone. A limit (the allocation count) fails
/// the batch alone at once. Only a failure of Vulkan itself is `Native`.
fn ensure_staging<T>(
    shared: &Arc<Shared<T>>,
    index: usize,
    lease: &mut Lease,
    need: u64,
    limits: TextureLimits,
) -> Result<(), StagingError> {
    if lease.staging.is_some_and(|staging| staging.size >= need) {
        return Ok(());
    }
    let context = &shared.context;
    let old = lease.staging.take();
    let previous = old.map_or(0, |staging| staging.size);
    // The old buffer is idle (the lease is ours), and a larger one replaces it. Growth doubles up to the batch
    // target, so a ramp of slowly larger batches does not reallocate each time.
    let target = need.max(
        previous
            .saturating_mul(2)
            .min(limits.max_upload_bytes_per_batch),
    );
    {
        let mut state = lock(&shared.state);
        state.leases[index].staging = None;
    }
    destroy_staging(context, old);
    loop {
        let other: u64 = lock(&shared.state)
            .leases
            .iter()
            .enumerate()
            .filter(|(position, _)| *position != index)
            .map(|(_, other)| other.staging.map_or(0, |staging| staging.allocated))
            .sum();
        // Each lease's buffer is at most the larger of the batch target and the largest image (64 MiB) and
        // `staging_bytes` is at least twice that, so by the validated ranges the two leases fit; a driver whose
        // queried sizes exceed that is handled below, not assumed away.
        match create_staging(context, target, &|requested| {
            staging_admission(other, requested, limits.staging_bytes)
        }) {
            Ok(staging) => {
                {
                    let mut peak = lock(&shared.stats);
                    peak.staging_bytes_peak = peak
                        .staging_bytes_peak
                        .max(other.saturating_add(staging.allocated));
                }
                lock(&shared.state).leases[index].staging = Some(staging);
                lease.staging = Some(staging);
                return Ok(());
            }
            Err(StagingError::Budget(reason)) => {
                let mut state = lock(&shared.state);
                let idle: Vec<usize> = (0..state.leases.len())
                    .filter(|position| {
                        *position != index
                            && state.free.contains(position)
                            && state.leases[*position].staging.is_some()
                    })
                    .collect();
                let idle_bytes: u64 = idle
                    .iter()
                    .filter_map(|position| state.leases[*position].staging)
                    .map(|staging| staging.allocated)
                    .sum();
                let busy = (0..state.leases.len())
                    .any(|position| position != index && !state.free.contains(&position));
                match staging_next_step(idle_bytes, busy, state.stopping) {
                    StagingStep::FreeIdle => {
                        for position in idle {
                            let held = state.leases[position].staging.take();
                            destroy_staging(context, held);
                        }
                    }
                    StagingStep::Wait => {
                        // `poll` returns the other lease and notifies; a stop notifies too.
                        drop(
                            shared
                                .changed
                                .wait(state)
                                .unwrap_or_else(PoisonError::into_inner),
                        );
                    }
                    StagingStep::Refuse => return Err(StagingError::Resource(reason)),
                }
            }
            Err(error) => return Err(error),
        }
    }
}

/// Creates one image, its view and its range, or says why not.
#[expect(
    clippy::too_many_lines,
    reason = "one image is created, sized, placed, bound and viewed in a fixed order, undoing each step on failure"
)]
fn create_gpu_image<T>(
    shared: &Arc<Shared<T>>,
    source: &mut DeviceSource,
    width: u32,
    height: u32,
) -> Prepared {
    let context = &shared.context;
    let device = context.device();
    let info = vk::ImageCreateInfo::default()
        .image_type(vk::ImageType::TYPE_2D)
        .format(FORMAT)
        .extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        })
        .mip_levels(1)
        .array_layers(1)
        .samples(vk::SampleCountFlags::TYPE_1)
        .tiling(vk::ImageTiling::OPTIMAL)
        .usage(USAGE)
        .sharing_mode(vk::SharingMode::EXCLUSIVE)
        .initial_layout(vk::ImageLayout::UNDEFINED);
    // SAFETY: the device is live and `info` is valid for the call.
    let image = match unsafe { device.create_image(&info, None) } {
        Ok(image) => image,
        Err(code) => return Prepared::Native(format!("vkCreateImage failed: {code:?}")),
    };
    let mut dedicated = vk::MemoryDedicatedRequirements::default();
    let mut requirements = vk::MemoryRequirements2::default().push_next(&mut dedicated);
    let query = vk::ImageMemoryRequirementsInfo2::default().image(image);
    // SAFETY: the image was just created on this device; the structures are valid for the call.
    unsafe { device.get_image_memory_requirements2(&query, &mut requirements) };
    let requirements = requirements.memory_requirements;
    let dedication = if dedicated.requires_dedicated_allocation == vk::TRUE {
        Dedication::Required
    } else if dedicated.prefers_dedicated_allocation == vk::TRUE {
        Dedication::Preferred
    } else {
        Dedication::No
    };
    let Some(memory_type) = context.memory_type(
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::empty(),
        vk::MemoryPropertyFlags::DEVICE_LOCAL,
    ) else {
        // SAFETY: the image is unbound and unused.
        unsafe { device.destroy_image(image, None) };
        return Prepared::Item("no memory type can hold the image".to_owned());
    };
    let request = Request {
        size: requirements.size,
        alignment: requirements.alignment,
        memory_type,
        dedication,
        image: {
            use ash::vk::Handle as _;
            image.as_raw()
        },
    };
    let placed = lock(&shared.images).allocate(source, &request);
    let placement = match placed {
        Ok(placement) => placement,
        Err(error) => {
            // SAFETY: the image is unbound and unused.
            unsafe { device.destroy_image(image, None) };
            return match error {
                AllocError::Budget {
                    needed,
                    reserved,
                    limit,
                } => Prepared::Item(format!(
                    "texture_bytes budget: {needed} more bytes would pass the limit ({reserved} of {limit} reserved)"
                )),
                AllocError::Source(SourceError::Count { in_use, limit }) => {
                    Prepared::Item(format!(
                        "allocation count limit: {in_use} of {limit} memory allocations are in use"
                    ))
                }
                AllocError::Source(SourceError::Device(message)) => Prepared::Native(message),
            };
        }
    };
    let release_range = |placement: Placement<vk::DeviceMemory>| {
        let mut source = DeviceSource::new(Arc::clone(context));
        lock(&shared.images).release(&mut source, placement);
    };
    // SAFETY: the image and the memory belong to this device; the range is inside the memory and meets the
    // queried alignment; the image has no memory yet.
    let bound = unsafe { device.bind_image_memory(image, placement.memory, placement.offset) };
    if let Err(code) = bound {
        // SAFETY: the image is unbound and unused.
        unsafe { device.destroy_image(image, None) };
        release_range(placement);
        return Prepared::Native(format!("vkBindImageMemory failed: {code:?}"));
    }
    let view_info = vk::ImageViewCreateInfo::default()
        .image(image)
        .view_type(vk::ImageViewType::TYPE_2D)
        .format(FORMAT)
        .subresource_range(color_range());
    // SAFETY: the image is bound and `view_info` is valid for the call.
    let view = match unsafe { device.create_image_view(&view_info, None) } {
        Ok(view) => view,
        Err(code) => {
            // SAFETY: the image is unused.
            unsafe { device.destroy_image(image, None) };
            release_range(placement);
            return Prepared::Native(format!("vkCreateImageView failed: {code:?}"));
        }
    };
    Prepared::Ready {
        allocated: requirements.size,
        image: GpuImage {
            context: Arc::clone(context),
            pool: Arc::clone(&shared.images),
            image,
            view,
            placement: Some(placement),
        },
    }
}

fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange::default()
        .aspect_mask(vk::ImageAspectFlags::COLOR)
        .level_count(1)
        .layer_count(1)
}

/// Records one batch into the lease's command buffer: one barrier `UNDEFINED → TRANSFER_DST_OPTIMAL` for
/// every image, the copies, and one barrier `TRANSFER_DST_OPTIMAL → SHADER_READ_ONLY_OPTIMAL`, which also
/// orders the later frames on the same queue. The pool is reset first: the lease came back to the worker only
/// after its fence was seen signalled.
fn record_batch<T>(
    shared: &Arc<Shared<T>>,
    lease: &Lease,
    copies: &[(vk::Image, u64, u32, u32)],
    need: u64,
) -> Result<(), String> {
    let device = shared.context.device();
    let Some(staging) = lease.staging else {
        return Err("a batch with images has no staging buffer".to_owned());
    };
    if !staging.coherent {
        let range = vk::MappedMemoryRange::default()
            .memory(staging.memory)
            .offset(0)
            .size(need.min(staging.size));
        // SAFETY: the memory is mapped, `need` is a multiple of the atom size inside the buffer.
        unsafe { device.flush_mapped_memory_ranges(&[range]) }
            .map_err(|code| format!("vkFlushMappedMemoryRanges failed: {code:?}"))?;
    }
    // SAFETY: the pool belongs to the lease the worker holds, and its last submission is proven finished.
    unsafe { device.reset_command_pool(lease.pool, vk::CommandPoolResetFlags::empty()) }
        .map_err(|code| format!("vkResetCommandPool failed: {code:?}"))?;
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: the command buffer is in the initial state after the pool reset.
    unsafe { device.begin_command_buffer(lease.cmd, &begin) }
        .map_err(|code| format!("vkBeginCommandBuffer failed: {code:?}"))?;
    let to_destination: Vec<vk::ImageMemoryBarrier<'_>> = copies
        .iter()
        .map(|&(image, ..)| {
            vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(color_range())
        })
        .collect();
    // SAFETY: the command buffer is recording; the barriers name images of this device.
    unsafe {
        device.cmd_pipeline_barrier(
            lease.cmd,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &to_destination,
        );
    }
    for &(image, offset, width, height) in copies {
        let region = vk::BufferImageCopy::default()
            .buffer_offset(offset)
            .image_subresource(
                vk::ImageSubresourceLayers::default()
                    .aspect_mask(vk::ImageAspectFlags::COLOR)
                    .layer_count(1),
            )
            .image_extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            });
        // SAFETY: the image is in `TRANSFER_DST_OPTIMAL` after the barrier and the region is inside both the
        // image and the staging buffer.
        unsafe {
            device.cmd_copy_buffer_to_image(
                lease.cmd,
                staging.buffer,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
        }
    }
    let to_sampled: Vec<vk::ImageMemoryBarrier<'_>> = copies
        .iter()
        .map(|&(image, ..)| {
            vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(color_range())
        })
        .collect();
    // SAFETY: the command buffer is recording; the barriers name images of this device.
    unsafe {
        device.cmd_pipeline_barrier(
            lease.cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &to_sampled,
        );
    }
    // SAFETY: the command buffer is recording.
    unsafe { device.end_command_buffer(lease.cmd) }
        .map_err(|code| format!("vkEndCommandBuffer failed: {code:?}"))
}

/// Latches a native failure found while preparing: the batch's items fail, the envelope is kept for the end.
fn latch_prepared_failure<T>(
    shared: &Arc<Shared<T>>,
    items: Vec<ItemResult<T>>,
    reason: &str,
    index: usize,
    lease: Option<usize>,
) where
    T: Copy,
{
    let count = items.len();
    let mut state = lock(&shared.state);
    state.failed.get_or_insert_with(|| reason.to_owned());
    state.stopping = true;
    state.free.push(index);
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
        lease,
        items,
        count,
    });
    state.occupied = state.occupied.saturating_sub(count);
    fail_waiting(&mut state, reason);
    drop(state);
    shared.changed.notify_all();
    notify_wake(shared.wakes.room.as_ref());
    notify_wake(shared.wakes.ready_to_submit.as_ref());
}

/// Reads an image in `SHADER_READ_ONLY_OPTIMAL` back to host memory on `queue`, for tests: a barrier to
/// `TRANSFER_SRC_OPTIMAL`, `vkCmdCopyImageToBuffer`, a barrier back to `SHADER_READ_ONLY_OPTIMAL` and a
/// bounded fence wait. Its own objects are destroyed after the wait; when the wait did not finish they are
/// kept, because the GPU may still use them.
///
/// # Errors
///
/// The Vulkan failure, or a wait that did not finish.
///
/// When the outcome is uncertain the device owner (`context`) is retained here for good, with the objects; a
/// caller that owns the image retains it too (see [`PreparedTexture::readback`]). This raw form takes the image
/// as a handle, so its caller must keep the image alive after an uncertain error.
#[doc(hidden)]
pub fn readback_image(
    context: &Arc<VulkanContext>,
    queue: &mut RenderQueue,
    image: vk::Image,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, Error> {
    let mut retained = false;
    read_image(context, queue, image, width, height, &mut retained)
}

/// [`readback_image`], saying through `retained` that an uncertain exit kept the readback's objects and the
/// device owner.
#[expect(
    clippy::too_many_lines,
    reason = "one readback records, submits, waits and cleans up in a fixed order"
)]
fn read_image(
    context: &Arc<VulkanContext>,
    queue: &mut RenderQueue,
    image: vk::Image,
    width: u32,
    height: u32,
    retained: &mut bool,
) -> Result<Vec<u8>, Error> {
    let device = context.device();
    let bytes = u64::from(width) * u64::from(height) * 4;
    let fail = |call: &str, code: vk::Result| crate::error::vk_error(call, code);
    let buffer_info = vk::BufferCreateInfo::default()
        .size(bytes)
        .usage(vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    // SAFETY: the device is live and `buffer_info` is valid for the call.
    let buffer = unsafe { device.create_buffer(&buffer_info, None) }
        .map_err(|code| fail("vkCreateBuffer(readback)", code))?;
    // SAFETY: the buffer was just created on this device.
    let requirements = unsafe { device.get_buffer_memory_requirements(buffer) };
    let cleanup_buffer = |memory: Option<vk::DeviceMemory>| {
        // SAFETY: nothing was submitted that uses the buffer.
        unsafe { device.destroy_buffer(buffer, None) };
        if let Some(memory) = memory {
            context.free_memory(memory);
        }
    };
    let Some(memory_type) = context.memory_type(
        requirements.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE,
        vk::MemoryPropertyFlags::HOST_COHERENT,
    ) else {
        cleanup_buffer(None);
        return Err(Error(
            "no host-visible memory type for the readback".to_owned(),
        ));
    };
    let memory = context
        .allocate_memory(requirements.size, memory_type, None)
        .map_err(|error| {
            cleanup_buffer(None);
            Error(format!("readback allocation failed: {error:?}"))
        })?;
    // SAFETY: the memory is unbound and large enough.
    if let Err(code) = unsafe { device.bind_buffer_memory(buffer, memory, 0) } {
        cleanup_buffer(Some(memory));
        return Err(fail("vkBindBufferMemory(readback)", code));
    }
    let pool_info = vk::CommandPoolCreateInfo::default().queue_family_index(context.family());
    // SAFETY: the device is live.
    let pool = match unsafe { device.create_command_pool(&pool_info, None) } {
        Ok(pool) => pool,
        Err(code) => {
            cleanup_buffer(Some(memory));
            return Err(fail("vkCreateCommandPool(readback)", code));
        }
    };
    let cleanup_all = |fence: Option<vk::Fence>| {
        // SAFETY: nothing submitted uses the pool or the fence any more.
        unsafe { device.destroy_command_pool(pool, None) };
        if let Some(fence) = fence {
            // SAFETY: as above.
            unsafe { device.destroy_fence(fence, None) };
        }
        cleanup_buffer(Some(memory));
    };
    let allocate = vk::CommandBufferAllocateInfo::default()
        .command_pool(pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    // SAFETY: the pool is live.
    let cmd = match unsafe { device.allocate_command_buffers(&allocate) } {
        Ok(buffers) => buffers[0],
        Err(code) => {
            cleanup_all(None);
            return Err(fail("vkAllocateCommandBuffers(readback)", code));
        }
    };
    // SAFETY: the device is live.
    let fence = match unsafe { device.create_fence(&vk::FenceCreateInfo::default(), None) } {
        Ok(fence) => fence,
        Err(code) => {
            cleanup_all(None);
            return Err(fail("vkCreateFence(readback)", code));
        }
    };
    let begin =
        vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
    // SAFETY: the command buffer is in the initial state.
    if let Err(code) = unsafe { device.begin_command_buffer(cmd, &begin) } {
        cleanup_all(Some(fence));
        return Err(fail("vkBeginCommandBuffer(readback)", code));
    }
    let transition = |old: vk::ImageLayout, new: vk::ImageLayout, src, dst| {
        vk::ImageMemoryBarrier::default()
            .src_access_mask(src)
            .dst_access_mask(dst)
            .old_layout(old)
            .new_layout(new)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(color_range())
    };
    let to_source = [transition(
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        vk::AccessFlags::empty(),
        vk::AccessFlags::TRANSFER_READ,
    )];
    let region = vk::BufferImageCopy::default()
        .image_subresource(
            vk::ImageSubresourceLayers::default()
                .aspect_mask(vk::ImageAspectFlags::COLOR)
                .layer_count(1),
        )
        .image_extent(vk::Extent3D {
            width,
            height,
            depth: 1,
        });
    let to_sampled = [transition(
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        vk::AccessFlags::TRANSFER_READ,
        vk::AccessFlags::SHADER_READ,
    )];
    let to_host = [vk::BufferMemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
        .dst_access_mask(vk::AccessFlags::HOST_READ)
        .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
        .buffer(buffer)
        .offset(0)
        .size(vk::WHOLE_SIZE)];
    // SAFETY: the command buffer is recording; the image is in `SHADER_READ_ONLY_OPTIMAL` (the store's
    // contract).
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &to_source,
        );
    }
    // SAFETY: the image is in `TRANSFER_SRC_OPTIMAL` after the barrier, the region is inside it, and the buffer
    // holds `bytes`.
    unsafe {
        device.cmd_copy_image_to_buffer(
            cmd,
            image,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            buffer,
            &[region],
        );
    }
    // SAFETY: the command buffer is recording; the buffer and the image belong to this device.
    unsafe {
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::FRAGMENT_SHADER | vk::PipelineStageFlags::HOST,
            vk::DependencyFlags::empty(),
            &[],
            &to_host,
            &to_sampled,
        );
    }
    // SAFETY: the command buffer is recording.
    if let Err(code) = unsafe { device.end_command_buffer(cmd) } {
        cleanup_all(Some(fence));
        return Err(fail("vkEndCommandBuffer(readback)", code));
    }
    let buffers = [cmd];
    let submit = vk::SubmitInfo::default().command_buffers(&buffers);
    if let Err(code) = queue.submit(&submit, fence) {
        if is_uncertain(code) {
            *retained = true;
            std::mem::forget(Arc::clone(context));
            return Err(fail("vkQueueSubmit(readback), its objects are kept", code));
        }
        cleanup_all(Some(fence));
        return Err(fail("vkQueueSubmit(readback)", code));
    }
    // SAFETY: the fence belongs to this device; the wait is bounded.
    if let Err(code) = unsafe { device.wait_for_fences(&[fence], true, READBACK_WAIT_NS) } {
        *retained = true;
        std::mem::forget(Arc::clone(context));
        return Err(fail(
            "vkWaitForFences(readback), its objects are kept",
            code,
        ));
    }
    let mut pixels = vec![0_u8; usize::try_from(bytes).unwrap_or(0)];
    // SAFETY: the memory is host-visible and not mapped; the GPU is finished.
    let mapped =
        unsafe { device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty()) };
    match mapped {
        Ok(ptr) => {
            if !context.is_coherent(memory_type) {
                let range = vk::MappedMemoryRange::default()
                    .memory(memory)
                    .offset(0)
                    .size(vk::WHOLE_SIZE);
                // SAFETY: the memory is mapped.
                let _ = unsafe { device.invalidate_mapped_memory_ranges(&[range]) };
            }
            // SAFETY: the mapping holds at least `bytes` bytes, written by the finished copy.
            unsafe {
                std::ptr::copy_nonoverlapping(ptr.cast::<u8>(), pixels.as_mut_ptr(), pixels.len());
            }
            // SAFETY: the memory is mapped and the copy out of it is done.
            unsafe { device.unmap_memory(memory) };
        }
        Err(code) => {
            cleanup_all(Some(fence));
            return Err(fail("vkMapMemory(readback)", code));
        }
    }
    cleanup_all(Some(fence));
    Ok(pixels)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{TextureLimits, UploadItem, image_bytes, staging_layout, staging_need};
    use crate::quad::PixelSource;

    struct Blank(Vec<u8>);

    impl PixelSource for Blank {
        fn pixels(&self) -> &[u8] {
            &self.0
        }
    }

    fn item(width: u32, height: u32) -> UploadItem<u32> {
        UploadItem {
            tag: 0,
            width,
            height,
            pixels: Arc::new(Blank(vec![0; width as usize * height as usize * 4])),
        }
    }

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
        let named = |limits: TextureLimits, field: &str| {
            let error = limits
                .validate()
                .expect_err("outside its range")
                .to_string();
            assert!(error.contains(field), "{error} names {field}");
            error
        };
        named(
            TextureLimits {
                max_prepared_images: 1,
                ..TextureLimits::default()
            },
            "max_prepared_images",
        );
        named(
            TextureLimits {
                max_upload_images_per_batch: 2048,
                ..TextureLimits::default()
            },
            "max_prepared_images",
        );
        let error = named(
            TextureLimits {
                max_upload_bytes_per_batch: 256 << 20,
                ..TextureLimits::default()
            },
            "max_upload_bytes_per_batch",
        );
        assert!(error.contains("staging_bytes/2"));
        named(
            TextureLimits {
                staging_bytes: (128 << 20) - 1,
                ..TextureLimits::default()
            },
            "staging_bytes",
        );
        named(
            TextureLimits {
                texture_bytes: (64 << 20) - 1,
                ..TextureLimits::default()
            },
            "texture_bytes",
        );
    }

    #[test]
    fn each_image_is_aligned_and_the_total_is_rounded_to_the_atom() {
        let items = [item(1, 1), item(3, 1), item(5, 5)];
        // 4-byte alignment, 64-byte atoms: offsets 0, 4, 16 (the 12-byte row of the second image ends at 16),
        // the last image takes 100 bytes and ends at 116, which rounds to 128.
        let (offsets, total) = staging_layout(items.iter(), 4, 64);
        assert_eq!(offsets, vec![0, 4, 16]);
        assert_eq!(total, 128);
        // A larger offset alignment pads between images, and the padding is part of the need.
        let (offsets, total) = staging_layout(items.iter(), 64, 64);
        assert_eq!(offsets, vec![0, 64, 128]);
        assert_eq!(total, 256);
        assert_eq!(image_bytes(&items[2]), Some(100));
    }

    #[test]
    fn a_staging_allocation_is_admitted_by_its_queried_size_before_it_is_made() {
        use super::staging_admission;
        assert!(
            staging_admission(64 << 20, 64 << 20, 128 << 20).is_ok(),
            "exactly at the ceiling"
        );
        let refused =
            staging_admission(64 << 20, (64 << 20) + 1, 128 << 20).expect_err("one byte over");
        assert!(
            refused.contains("staging_bytes") && refused.contains("held"),
            "{refused}"
        );
        assert!(
            staging_admission(u64::MAX, 1, u64::MAX).is_err(),
            "an overflow is a refusal"
        );
    }

    #[test]
    fn a_refused_staging_allocation_frees_waits_or_fails_the_batch_alone() {
        use super::{StagingStep, staging_next_step};
        // The other lease holds an idle buffer: free it and try again.
        assert_eq!(staging_next_step(1, false, false), StagingStep::FreeIdle);
        assert_eq!(staging_next_step(1, true, true), StagingStep::FreeIdle);
        // It is in flight and nothing is idle: wait for it to return, unless the store is stopping.
        assert_eq!(staging_next_step(0, true, false), StagingStep::Wait);
        assert_eq!(staging_next_step(0, true, true), StagingStep::Refuse);
        // Nothing to free and nothing to wait for: the items fail alone.
        assert_eq!(staging_next_step(0, false, false), StagingStep::Refuse);
    }

    #[test]
    fn the_atom_rounding_applies_to_one_image_and_an_empty_batch_needs_nothing() {
        let (offsets, total) = staging_layout([item(1, 1)].iter(), 4, 256);
        assert_eq!(offsets, vec![0]);
        assert_eq!(total, 256);
        let (offsets, total) = staging_layout(std::iter::empty::<&UploadItem<u32>>(), 4, 256);
        assert!(offsets.is_empty());
        assert_eq!(total, 0);
    }

    #[test]
    fn batch_formation_counts_the_padding_and_skips_invalid_images() {
        let mut bad = item(4, 4);
        bad.pixels = Arc::new(Blank(vec![0; 3]));
        let items = [item(8, 8), bad, item(8, 8)];
        // Two valid 256-byte images at 4-byte alignment: 512, rounded to 512; the invalid one takes nothing.
        assert_eq!(staging_need(items.iter(), 4, 64), 512);
        // At 1 KiB alignment the second valid image starts at 1024.
        assert_eq!(staging_need(items.iter(), 1024, 64), 1024 + 256);
    }
}
