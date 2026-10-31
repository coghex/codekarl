//! The Vulkan presenter: a render thread that owns every device-level Vulkan object and clears a window's
//! surface to a colour, with the present-lifetime ledger of design §8.
//!
//! # Boundaries
//!
//! - **No window dependency.** The library depends on `ash` only. The surface arrives as a raw
//!   `VkSurfaceKHR` handle, created by whoever owns the window (`ck-window-glfw`'s
//!   `GlfwHandles::create_surface`) on an [`Instance`] this crate created, and the instance's raw handle goes
//!   the other way. The windowed tests create their GLFW windows through a dev-dependency, in the test
//!   binary only. The library contains no macOS-only code apart from the loader's file name and `MoltenVK`'s
//!   portability flags (`VK_KHR_portability_enumeration` with its flag on the instance,
//!   `VK_KHR_portability_subset` on a device that advertises it).
//! - **The presenter's own types.** [`VulkanPresenter`], [`PresenterReport`] and [`CleanupResult`] mirror
//!   `ck_window::Presenter`'s contract (this crate cannot name that trait: it has no window dependency); the
//!   adapter that implements the trait lives in `codekarl`, in one place.
//! - **The loader** ([`Loader`], [`load_loader`]) is loaded by an explicit absolute path
//!   (`$VULKAN_SDK/lib`, `/usr/local/lib`, `/opt/homebrew/lib`, then the platform's default search), never
//!   through a `DYLD_*` variable, and is reference-counted: see the [`loader`](crate::Loader) contract, which is
//!   shared word for word with `ck-window-glfw` and the executable. Every [`Instance`], every
//!   [`VulkanPresenter`] handle **and its render thread**, and [`readback_clear`] hold their own clone, so
//!   the library is unloaded only when the last user is gone, and never under a retained (unproven) thread.
//!
//! # The main thread
//!
//! [`create_instance`] / [`create_instance_with`] make the instance (the surface extensions the window
//! side requires, the present-fence extensions `VK_EXT_surface_maintenance1` and
//! `VK_KHR_get_surface_capabilities2` **and the extension they depend on, `VK_KHR_surface`, whatever the
//! caller passes**, so an empty list is headless use and a valid instance; and, when asked,
//! `VK_LAYER_KHRONOS_validation` with a debug messenger that records every warning and error **from the
//! creation of the instance to its destruction**: one messenger is chained into the instance create info
//! (it reports `vkCreateInstance` and `vkDestroyInstance`) and one is persistent, both into a log whose
//! storage outlives the instance). The surface is created by the window side. [`VulkanPresenter::start`]
//! spawns the render thread and returns at once; the handle's methods never wait beyond a brief lock to
//! copy a small value, and dropping the handle only requests the stop. After a proven end the main thread
//! calls [`destroy_surface_and_instance`], which destroys the surface and then the instance and **refuses**
//! unless the presenter reported [`CleanupResult::Proven`] and no render thread still holds the instance.
//! An unproven off-screen readback keeps the instance for the rest of the process; then the surface is
//! still destroyed and the [`DestroyRefused`] it returns says so (`surface_destroyed`), so the surface is
//! never destroyed twice.
//!
//! # The render thread
//!
//! - **Setup** (on the thread): a physical device with a graphics queue that can present to the surface, with
//!   `VK_KHR_swapchain` and `VK_EXT_swapchain_maintenance1` (feature enabled); otherwise a
//!   [`PresenterReport::Failed`] naming the missing extension, then a `Proven` end (nothing was created).
//!   The device, queue, command pool and one command buffer, the render pass, the fixed pools and the first
//!   swap chain follow; a failure in the middle destroys exactly what exists (nothing was submitted).
//! - **A frame** clears with a **render pass** (load op clear, final layout present; no dynamic
//!   rendering): acquire with a pool-taken semaphore (a 50 ms bounded wait), record, submit (waiting on the
//!   acquire semaphore, signalling a pool-taken render-finished semaphore, with a submit fence), present with
//!   a present fence (`VkSwapchainPresentFenceInfoEXT`). The swap chain uses FIFO and the surface's BGRA8
//!   UNORM format (then BGRA8 sRGB, then the first), at the window's pixel size. [`PresenterReport::FirstFrameQueued`]
//!   is sent once, after the first present call that queued (success or suboptimal): not proof that the frame
//!   is visible, and a present fence proves release, not display. At most one frame's commands are in flight.
//! - **Redraw on demand**: the first frame, a changed colour or size, and [`VulkanPresenter::request_redraw`],
//!   coalesced through a latest-value slot; no continuous loop. A zero pixel size in either dimension
//!   suspends rendering and creates no zero-size swap chain.
//! - **The ledger** (`ledger.rs`, pure bookkeeping tested without a device) records every acquire, submit,
//!   present and retired swap chain, and the fence or drain submit that proves each finished: an acquire
//!   semaphore returns to its fixed pool after the submit fence that waited on it; a render-finished
//!   semaphore and its present fence return together, only after the present fence signalled; a present
//!   that returned success, suboptimal, out-of-date or surface-lost queued its wait and fence (the fence is
//!   outstanding); a present that failed with out-of-memory queued nothing, so its fence is never waited on
//!   and its render-finished semaphore is drained by a waiting submit before reuse; a frame abandoned after
//!   acquire releases its image with `vkReleaseSwapchainImagesEXT` and drains its acquire semaphore;
//!   `VK_ERROR_DEVICE_LOST` keeps everything and makes the cleanup a failure. The pools have fixed sizes
//!   (twice the image count) and an empty pool skips the frame with the redraw kept pending. At most one swap
//!   chain is retired: further rebuilds pause, size changes coalesce to the latest, and the rebuild runs once
//!   the retired swap chain is gone.
//! - **Suboptimal.** On `VK_SUBOPTIMAL_KHR` the thread queries the surface's capabilities and rebuilds only
//!   when its configuration (the normalised, limit-clamped extent a swap chain would be created with now, which
//!   is `currentExtent` when defined and the window's pixel size otherwise, and `currentTransform`) differs
//!   from the current swap chain's creation parameters. At the parameters it already has, a suboptimal result
//!   does not rebuild (the frame still counts as presented); a later change of them, or an out-of-date result,
//!   does. Creation and comparison use one function, so they cannot disagree; a zero extent after the
//!   surface's limits is not drawable (rendering is suspended, no swap chain is created). A failed capability
//!   query follows the failure and proof rules.
//! - **Creation retry.** A swap chain creation failure retires the old swap chain anyway and pauses rendering.
//!   The retry runs once no swap chain is retired, and then on the next redraw trigger (a size or colour
//!   change, `request_redraw`) or after a backoff (50 ms, doubling, at most 1 s, reset by a success),
//!   whichever comes first: a retained redraw bit does not wake it, only a fresh external trigger does, the
//!   thread never retries without waiting, and `request_stop` ends the wait at once. There is no attempt limit:
//!   a creation failure alone never sends `Failed`, except `VK_ERROR_DEVICE_LOST` (the device-loss rule) and
//!   `VK_ERROR_SURFACE_LOST_KHR` or `VK_ERROR_NATIVE_WINDOW_IN_USE_KHR` (the surface cannot recover).
//! - **Retirement is not teardown.** While rendering is healthy a retired swap chain (with its image views and
//!   framebuffers) is destroyed once every present fence and submit that used it signalled. After
//!   `VK_ERROR_DEVICE_LOST`, and in the teardown before every proof of every swap chain has succeeded, it is
//!   not destroyed, even if its fences signal: a later timeout still leaves everything kept.
//! - **Waits are bounded and never `vkDeviceWaitIdle`/`vkQueueWaitIdle`**: the frame loop only asks fences
//!   for their status; the teardown waits with `vkWaitForFences` timeouts against the stop deadline.
//! - **Teardown.** After [`VulkanPresenter::request_stop`] the thread stops drawing and proves every submit
//!   fence, present fence (the retired swap chain's included) and drain against **one absolute deadline**
//!   (the first `request_stop` plus 1.5 s; a repeated stop does not move it, and time spent in a blocked
//!   acquire comes out of it). If all are proven it destroys, in design §7 order, the command buffers and
//!   pool; the fences and semaphores; the image views, framebuffers and render pass; the swap chains; the
//!   device, releases its holds on the instance and the loader and sends `Proven`. Otherwise (a fence that
//!   did not signal, a lost device, a panic) it destroys **nothing** and sends `Unproven`: the thread's
//!   holds on the instance and the loader are kept too, because they sit in `ManuallyDrop` and a thread that
//!   ends by unwinding releases nothing. `Proven` is reported only from an explicit success and an observed
//!   thread end.
//! - **Failures** after setup (any Vulkan error the ledger does not recover from) send
//!   [`PresenterReport::Failed`] once, draw nothing more, and then prove and tear down as far as the ledger
//!   allows (a surface loss or an out-of-memory present can still end `Proven`; a lost device cannot).
//!   Reports are queued in order and every one is available no later than after `poll_end` returned `Some`.
//!
//! # The textured quad
//!
//! [`VulkanPresenter::start_2d`] starts a presenter with a [`VulkanQuadSlot`], the latest-value slot a renderer
//! publishes [`VulkanScene`]s to. A publish marks a frame wanted and wakes the render thread only, never waits,
//! and works before the presenter starts (the first frame has the value) and after it ended (it only stores).
//! [`VulkanPresenter::drawn_generation`] reads, without waiting, the generation of the scene the last queued frame
//! drew (a bounded latest observation, not a history and not visual proof). The slot holds no Vulkan object.
//!
//! - **Drawing.** Each frame clears and then draws the slot's quad in the same render pass, through the drawing
//!   code the off-screen readback shares. The pipeline, its layouts, the descriptor pool and the samplers are
//!   created at set-up, after the render pass, only when the swap chain's format is `B8G8R8A8_UNORM` or
//!   `R8G8B8A8_UNORM`. On any other format none of them is created (so it cannot fail set-up), the window keeps
//!   clearing, and the first frame that has a quad records one [`QuadSkipped`] notice, read by
//!   [`VulkanPresenter::take_quad_notice`] (never a report), after which the presenter's `wake` runs.
//! - **Upload.** A frame uploads when the quad's pixel source differs from the resident one by identity (the
//!   outer `Arc` allocation plus width and height, held as a `Weak`). The render thread creates the texture and
//!   a staging buffer, copies the bytes into the staging buffer and **drops its reference to the pixel source**,
//!   however the step ends. The copy is recorded in the frame's command buffer before the render pass, and the
//!   texture counts as uploaded only once that frame is **submitted**: a frame abandoned after recording leaves
//!   the staging buffer, and the next frame records the upload again from it. A move or a filter change uploads
//!   nothing.
//! - **Lifetime.** At most two textures (one resident, one retired) and one staging buffer exist, counted from
//!   the moment each one's creation begins; a frame that would exceed that waits with its redraw kept. A replaced
//!   or removed texture is retired and destroyed once the ledger's own record has the proof of the submit that
//!   last drew it, a staging buffer once its upload's submit is proven. Not after a device loss, not once a stop
//!   was requested (decided from the stop state under its lock): only the proven teardown destroys then, after
//!   everything that uses the quad objects and before the render pass and the device. An unproven end destroys
//!   none.
//! - **Device checks** run on the render thread, after device selection and before any object is created for a
//!   quad: the image fits `maxImageDimension2D`, and `R8G8B8A8_UNORM` can be sampled and be a transfer
//!   destination with optimal tiling. A quad that fails them is a `Failed` report, as is a texture or staging
//!   allocation that fails (naming its size) and a pipeline that cannot be created.
//!
//! # The texture store and the revision ledger (milestone 5.5, issue 1)
//!
//! Off screen, with no window: [`VulkanContext`] (the shared logical device, `Send + Sync`, which hands out the one
//! [`RenderQueue`] once, so every `vkQueueSubmit` goes through `&mut RenderQueue`), [`TextureStore`] (a worker
//! that records upload batches into one of two leases and never submits; the owner submits and polls fences) and
//! [`TextureLedger`] (revisions, boundary adoption, frame tickets, one descriptor set per revision recycled only
//! on release, and frame completion as a recorded [`FrameSubmission`] number). Image memory is pooled in 64 MiB
//! blocks under a hard `texture_bytes` ceiling and the device's `maxMemoryAllocationCount`; the end is proven
//! (`prove`, then `release`) and never waits for the device or the queue to go idle. The module documentation of
//! `textures`, `revisions`, `memory`, `proofs` and `descriptors` carries the contract, which is the Metal
//! crate's (CK-112/113/148/150/154) wherever those say nothing different.
//!
//! # The scene presenter (milestone 5.5, issue 2a)
//!
//! [`VulkanPresenter::start_scene`] draws an ordinary instanced, batched scene on a shared [`VulkanContext`] made for the
//! surface: a [`VulkanFrameSlot`] of [`VulkanFrame`]s (a latest-value slot, like the quad's), a [`TextureDriver`] run on the
//! render thread's passes, and the texture ledger's frame tickets. `start` and `start_2d` keep their own device.
//!
//! - **Drawing.** Instances are written in painter order into one host-visible buffer (32 bytes each, charged to
//!   `SceneLimits::binding_geometry_bytes` and counted against `maxMemoryAllocationCount`); adjacent equal revisions
//!   share one instanced draw (`firstInstance` selects the batch, core Vulkan on a direct draw). A quad is six vertices of
//!   a triangle list, not a strip: `MoltenVK` warns that Metal cannot disable primitive restart for a strip. The effective
//!   sampler of a batch is the nearest set for the checker and the frame's filter otherwise; set 0 is rebound whenever it
//!   changes, set 1 is the batch's revision. The checker (64 x 64, 8-pixel magenta/black cells) is a baseline item of the
//!   store with its own permanent set, and no frame is attempted before it is resident.
//! - **Frame proofs.** A frame is submitted by `FrameTicket::submit` through the context's [`RenderQueue`]; the present
//!   ledger records that number and asks `TextureLedger::frame_proven(number)` wherever it asked a submit fence. The
//!   instance buffer is written only when the previous submission is proven (one frame in flight).
//! - **The idle rule.** There is no completion callback, so the render thread polls. Runnable work is never delayed (a
//!   capped pass, a closed upload envelope, a completion observed, a frame that is wanted with every gate open). A want
//!   that fails a gate (the checker, the drawable size, present capacity, the previous submission) is parked until a fact it
//!   depends on changes. Only with nothing runnable does the thread wait: until the earlier of a pending swap-chain creation
//!   retry (`creation_retry_at`) and 1 ms while GPU work is outstanding, and without a bound when neither exists.
//! - **The end** is the combined proof of the present ledger, the texture ledger and the store under the one absolute stop
//!   deadline; a complete proof releases in dependency order and the context last, anything uncertain retains everything.
//!
//! # The bindless gate (milestone 5.5, issue 5)
//!
//! The final binding stage (design §13.3) samples every loaded texture through one large sampled-image table, which needs
//! device features that can only be enabled when the device is created. [`VulkanContext::new`] therefore enables
//! `VK_EXT_descriptor_indexing` when the device advertises it (the instance requests Vulkan 1.1) and each of the four
//! features the tables use that the device reports, and keeps the report ([`BindlessCaps`]) once. The gate (`gate.rs`) is a
//! pure function of that report, the slot count ([`BindlessLimits`]) and the answer of `vkGetDescriptorSetLayoutSupport` for
//! the table's exact layout: it judges queried features and limits, never a driver name, and refuses with one bounded line
//! listing every failed row with the requested and the available value.
//!
//! # The bindless stage (milestone 5.5, issue 5)
//!
//! [`SceneSetup::new`] takes the tables' slot count ([`BindlessLimits`]). At the scene's start the gate decides: a device that
//! passes gets **two versions of one 131,072-entry (or configured) sampled-image table** (update-after-bind layout and pool,
//! allocated once, never freed or grown) and a separate sampler binding; any other device draws through the per-revision sets of
//! the ordinary stage with one logged notice ([`VulkanPresenter::take_binding_notice`]). The ledger gives each shown revision a
//! native slot, writes only a version no ticket pins (never on the upload worker, never after a wait for the device), publishes
//! a revision at a boundary once its slot is applied in the version the next frame binds, sets a retired revision's slot back to
//! the checker in **both** versions before it releases the image, and reuses a slot only after that. A frame binds its pinned
//! version once and pushes the batch's slot as a fragment constant (`shaders/scene_bindless.frag`); the batches are CK-157's.
//! The support query `vkGetDescriptorSetLayoutSupport` is logged and does not gate (see `gate.rs`).
//!
//! # What `MoltenVK` does (observed on this Mac, macOS 26.7, SDK loader 1.3.296)
//!
//! A surface accepts **one swap chain at a time**: creating a second one that does not name the first as
//! `oldSwapchain` fails with `VK_ERROR_NATIVE_WINDOW_IN_USE_KHR`, so a presenter whose end was not proven
//! leaves its swap chain on the surface and no other presenter can use that surface. A swap chain whose
//! extent differs from the layer's drawable size is reported `VK_SUBOPTIMAL_KHR` by present and acquire
//! (hence the once-per-size rule above). The device is a portability one and needs
//! `VK_KHR_portability_subset` enabled.
//!
//! # Test hooks and the readback
//!
//! The hooks ([`TestHooks`], [`Gate`], [`Observer`]) reach the presenter only through the `doc(hidden)`
//! `VulkanPresenter::start_with_test_hooks`: there is no Cargo feature, environment variable or flag (the
//! full check builds the workspace with `--all-features`, so a feature would reach `codekarl` too), and a
//! test checks that `codekarl`'s sources never name it. The guarantee is that no production path activates
//! a hook. A hook models the underlying operation faithfully: an injected out-of-date, suboptimal or
//! surface-lost present performs the real present first (so its semaphore wait and fence are queued); an
//! injected out-of-memory present queues nothing; an injected acquire error makes no acquire call;
//! an injected swap chain creation error makes no creation call, and the old swap chain is retired anyway.
//! [`readback_clear`] renders a clear into an off-screen image on a device from an instance, copies it to
//! a host-visible buffer and returns the first texel; it is used by tests and nothing else. **It owns what it
//! uses:** an owning reference to the instance and its own loader clone. A failure before any submit, or a
//! submit that queued nothing (out of memory), cleans up and returns an error. When its completion is unproven
//! (the fence wait times out or fails, a lost device from the submit or the wait, an unwind after the
//! submit) it returns an error naming that and **keeps** its device, its objects, the instance reference and
//! the loader clone for the rest of the process; [`readback_clear_with_test_hooks`] injects those cases.

mod batch;
mod bindless;
mod capture;
mod context;
mod descriptors;
mod draw;
mod error;
mod frame;
mod gate;
mod gpu;
mod gpu_quad;
mod hash;
mod hooks;
mod instance;
mod ledger;
mod loader;
mod memory;
mod presenter;
mod proofs;
mod quad;
mod readback;
mod readback_quad;
mod readback_scene;
mod render;
mod residency;
mod revisions;
mod scene;
mod scene_draw;
mod shared;
mod slot;
mod textures;
mod unwind;

pub use ash::vk;
pub use batch::FrameCounters;
pub use capture::{FrameCapture, FrameCaptures};
pub use context::{RenderQueue, TestFeatures, VulkanContext};
pub use error::Error;
pub use frame::{InstanceRole, VulkanFrame, VulkanFrameSlot, VulkanInstance, VulkanSceneSource};
pub use gate::{BindlessCaps, BindlessDeviceLimits, BindlessLimits, BindlessReport};
pub use hooks::{
    BuildRecord, CompatKind, Event, Gate, Observer, Pause, PipelineFault, QuadAllocation,
    QuadFaults, QuadStats, SceneHooks, SurfaceExtent, TestHooks, WaitKind, WaitObserver,
};
pub use instance::{
    DestroyRefused, Instance, InstanceFailure, InstanceOptions, InstanceTestHooks, LayoutScope,
    LogDrain, LogEntry, ValidationLogHandle, create_instance, create_instance_draining,
    create_instance_with, create_instance_with_test_hooks, destroy_surface_and_instance,
};
pub use ledger::LedgerSnapshot;
pub use loader::{
    GetInstanceProcAddr, Loader, LoaderWatch, load_loader, load_loader_from, search_directories,
};
pub use presenter::VulkanPresenter;
pub use quad::{
    DestinationRect, MAX_IMAGE_BYTES, MAX_IMAGE_SIDE, PixelSource, SourceRect, VulkanFilter,
    VulkanQuad, VulkanQuadError, VulkanScene,
};
pub use readback::{ReadbackHooks, readback_clear, readback_clear_with_test_hooks};
pub use readback_quad::{MAX_TARGET_SIDE, readback_quad, readback_quad_with_test_hooks};
pub use readback_scene::{
    PROBE_CELL, SceneContent, SceneReadback, readback_scene, readback_scene_with_held,
    readback_slots,
};
pub use revisions::{
    BindingChoice, BindingNotice, CheckerProgress, FrameSubmission, FrameTexture, FrameTicket,
    LedgerCleanup, LedgerError, LedgerEvent, LedgerLimits, LedgerProof, OfferRefusal,
    RevisionState, TextureLedger,
};
pub use scene::{
    Boundary, CHECKER_CELL, CHECKER_SIDE, HeldCompat, PassOutcome, SceneLimits, SceneSetup,
    TextureDriver, TicketEnding, checker_pixels,
};
pub use shared::{CleanupResult, PresenterReport, SceneStats};
pub use slot::{QuadSkipped, VulkanQuadSlot};
pub use textures::{
    Completed, PreparedTexture, Proof, Refused, StallGate, StoreCleanup,
    TestHooks as TextureTestHooks, TextureLimits, TextureOwner, TextureStats, TextureStore,
    TextureWakes, UploadItem, UploadPort, readback_image,
};
