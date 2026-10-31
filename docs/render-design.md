# Milestones 4 and 4.5: the textured quad and `render-2d`

**Status: technical design approved in independent design review 3 (2026-10-21, `design-review-3-m4.md`, on `67ef8a95…`); the owner
answered every open question the same day (§1). Milestone 4 is accepted (2026-10-21): CK-85 (several Lua extension tables), CK-90
(`ck-render-2d` core), CK-91 (Metal quad), CK-92 (`ck-render-2d` backends), CK-93 (`codekarl` through `ck-render-2d`, the last in batch
`1792642383528609000`). As accepted, the GLFW and Vulkan path keeps its own presenter adapter in `codekarl`, wrapped by `ck-render-2d`'s
clear-only backend. The owner ran the demo ("seems to work great", 2026-10-22); the milestone 4 guide review found one P3, repaired as
CK-94 (batch `1792684698784608000`). The owner then settled the package rule for milestone 4.5 (§1, 2026-10-22): GPU-specific code stays
in the GPU crates, and nothing moves into `ck-render-2d`. Milestone 4.5's design (§3, §7, §10) was approved in design review 5 (2026-10-22, `runtime-m4/design-review-5-m45.md`, on
`5eee25da…`). Milestone 4.5 is accepted (2026-10-23): CK-95 (Vulkan quad off screen), CK-96 (caught panic payloads), CK-97 (Vulkan
quad presented), CK-98 (`ck-render-2d` `vulkan` feature), CK-99 (`codekarl` through the Vulkan quad, the last in batch
`1792743628066239000`). `ClearOnly` stays in `ck-render-2d`, unused by the application; its removal is a later decision. The owner
ran `config/demo-quad-vulkan.toml` ("yes that looks right", 2026-10-23).**
Owner decisions are
marked **Firm** with their date. Everything else is a **Proposal** (ck-manager's) or **Open**. A Proposal is not an owner decision until
the owner accepts it; approved Proposals are settled in the issue specs and their reviews.

It extends `docs/window-design.md` (milestones 3 and 3.5) and `docs/runtime-design.md`. Their rules still apply unless this document says
otherwise: the window core and its close protocol, redraw on demand, the render thread, the failure boundary and teardown order, logging,
the VM owners and the exit statuses.

## 1. Owner direction (Firm)

Given directly to ck-manager on 2026-10-21.

- **Two milestones, one per backend:** "milestone 4 will be a textured quad in metal, milestone 4.5 will be a textured quad in vulkan".
  As with milestones 3 and 3.5, this document designs both, and the issues for 4.5 are filed after milestone 4 is accepted and reviewed.
- **The image comes from a file,** which the owner provides "when we get there". **PNG only.**
- **The UI VM chooses the image and places the quad:** "we will need the lua vm to start a ui vm that shooses the image and places the
  quad". The UI VM already exists (milestone 2) and owns the window (milestone 3); its script chooses the image and the quad's place.
- **It does not move yet.** The quad is drawn and redrawn on demand. The continuous frame loop stays deferred (`docs/window-design.md`
  §1).
- **One `render-2d` on both GPU packages:** "this is going to be a singular render-2d that sits on both packages. metal and vulkan at this
  point should be interchangable to the rest of the project." This is the owner's design discussion that the standing rule requires before
  a renderer abstraction exists (AGENTS.md, `docs/vision.md` "Engineering direction"), for this scope: one 2D renderer over the two GPU
  crates. ECS, map representation and a general renderer remain reserved.
- **Sampling is nearest for now, chosen in the settings file:** "nearest is fine for now, but this needs to be in a settings data file".
- **Placement is in window points from the top-left corner.** The quad's size defaults to the image's own size; the script may give a
  width and height.
- **Transparent pixels blend over the clear colour.**
- **Answers to the design's questions (owner, 2026-10-21, after design review 3):**
  - images live in the configured `assets/` directory, and the render API opens nothing outside it (§5, §6);
  - during milestone 4 a quad on the GLFW and Vulkan path logs one warning and draws nothing (§7);
  - one image pixel is one window point by default (§4.4);
  - after milestone 4.5, on a Vulkan surface with no UNORM format, the quad is skipped with one warning and the window keeps clearing
    (§7).
- **GPU-specific code stays in its own crates (2026-10-22).** Asked whether the application's Vulkan presenter adapter should move into
  `ck-render-2d`, the owner answered: "why would you move it into render-2d? did you move metal presenter code into render-2d, i thought
  we agreed that vulkan and metal specific code would be in seperate crates". So Vulkan drawing goes into `ck-gpu-vulkan`, as Metal's
  went into `ck-gpu-metal`; `ck-render-2d` keeps only thin conversion adapters, and the application keeps its window and GPU set-up (§3,
  §7).

## 2. Facts that shape the design

- `ck-window` drives a `Presenter` (start, `set_clear`, `set_pixel_size`, `request_redraw`, reports, stop and cleanup). It knows nothing
  of Lua, Metal or Vulkan, and its request slot carries the window description, the clear colour, `open` and `close`.
- `ck-gpu-metal`'s `MetalPresenter` and `ck-gpu-vulkan`'s `VulkanPresenter` each own a render thread and clear a window. Both already have
  an off-screen readback used by their tests (`readback_clear`), and `ck-gpu-metal`'s `build.rs` already compiles a `.metal` shader.
- The Metal drawable is `BGRA8Unorm`, set explicitly, so colours are not converted on the way to the screen: the clear colour's numbers
  are the bytes written. The Vulkan swapchain **prefers** `B8G8R8A8_UNORM`, then accepts `B8G8R8A8_SRGB`, then the first format the
  surface offers (`crates/gpu-vulkan/src/gpu.rs`). UNORM is what this Mac's MoltenVK gives, but it is not an invariant (§7, §8).
- `ck-lua` installs **one** extension table per VM (`VmSettings::extension`), and the UI VM's is `window`. A second table needs `ck-lua`
  work (§5, §10).
- The accepted Lua setup includes the standard library (`io`, `os`, `loadfile`, `dofile`): scripts are not sandboxed, and nothing here
  changes that.
- `codekarl` adapts each presenter to `ck-window` itself (`native.rs`, `vulkan.rs`), so the application names both GPU crates today.
- The UI VM's `window` extension writes the request slot from the UI VM's thread and never waits.
- AGENTS.md keeps GPU infrastructure independent of a future `render-2d`: the GPU crates must not depend on it.

## 3. Package boundaries (Proposal, ck-manager)

```
codekarl ──► ck-render-2d ──► ck-gpu-metal   (feature `metal`)
    │              │     └──► ck-gpu-vulkan  (feature `vulkan`)
    │              └──► ck-window, ck-log, png
    └──► ck-window, ck-window-cocoa / ck-window-glfw, ck-lua, ...
```

- **`ck-render-2d`** (`crates/render-2d`, new) is the only drawing API the application's backend-neutral code sees: the Lua surface, the
  windowed run, the scene and their tests. The backend-choice modules (`native.rs`, `vulkan.rs` and `vulkan/`) are the exception: they
  create the window and the GPU objects, construct the backend, and own that set-up's lifetime (as accepted in CK-92 and CK-93, and
  confirmed by the owner for milestone 4.5, §1). It owns:
  - **`Image`:** an RGBA8 image with premultiplied alpha, decoded from PNG with the `png` crate (§8);
  - **`Filter`:** `Nearest` or `Linear`;
  - **`Quad`:** an image and a rectangle in window points (top-left origin);
  - **the scene bridge:** the latest quad (or none) in points, written from the UI VM's thread without waiting, and published to the GPU
    in pixels (§4);
  - **`Renderer`:** a `ck_window::Presenter` over one GPU backend. It forwards the window calls and hands the backend the scene in
    pixels;
  - **the backend trait:** what a GPU crate's presenter must offer for 2D. It is implemented inside `ck-render-2d` for each GPU crate,
    behind its feature, so the GPU crates never depend on `ck-render-2d`. **As built:** Metal's backend (`MetalBackend`) lives in
    `ck-render-2d` because a Cocoa window hands over everything it needs. The Vulkan path's set-up (loader, GLFW, instance, surface) has
    more to own, so its backend stays in the application, and `ck-render-2d`'s `vulkan` feature gives it only the scene conversion (§7).
    Neither adapter holds drawing code: that is in the GPU crates.
- **The GPU crates** gain a textured-quad draw in their own plain terms: upload these RGBA8 premultiplied pixels, draw them at this
  rectangle in pixels, with this filter, blended over the clear colour, or draw no quad. Each keeps its own shader, pipeline and texture
  lifetime. `ck-gpu-metal` adds `shaders/quad.metal` to the shaders its `build.rs` compiles.
- **`codekarl`** creates a `Renderer` for the configured backend and otherwise uses only `ck-render-2d`'s types: the Lua extension, the
  windowed run and the tests are the same for both backends. Choosing the backend and creating the window stay where they are. A source
  guard checks that no application file except the backend-choice module names a GPU crate. It reads the application's sources, so it is
  registered in the catalog's guard inventory (CK-84).
- **`ck-window` is unchanged.** The quad does not pass through its request slot, which stays free of rendering data (§4).

## 4. Threads and the scene bridge (Proposal, ck-manager)

- **Decoding happens in the Lua call,** on the UI VM's thread, so a missing or broken file is an ordinary Lua error raised by
  `render.quad` (§5). A large image delays the UI VM for that call, and with it anything waiting on that VM: its start-up completion,
  callback delivery and `shutdown()`. A host call cannot be interrupted by the Lua instruction hook (`docs/runtime-design.md` §5.2), so a
  decode that outlives a start or stop deadline makes the run end incomplete with exit 3, as any slow host call does. The size budgets
  (§8) bound the data, work and memory of a decode, but not its elapsed time: file I/O and scheduling can still stall it, and no hard
  decode-time bound is promised. There is no decoder thread.

### 4.1 The bridge

`ck-render-2d` owns one **scene bridge** per run. Its state, behind one small lock:
- the **scene** in points: the latest quad (an `Arc<Image>` and a rectangle), or none;
- the **metrics:** the window's size in points (from `WindowDesc`) and its current pixel size, absent before the presenter starts;
- a **generation** counter;
- the **attached GPU handle**, absent before start and after stop;
- the **phase:** `Before`, `Attached` or `Ended`.

Every change goes through the lock, whichever thread makes it:
- a **scene write** from the UI VM's thread (`render.quad`, `render.clear_quad`);
- **attach** from `Renderer::start` on the main thread, with the `WindowDesc` and `PixelSize` that `Presenter::start` already receives;
- a **metrics update** from `Renderer::set_pixel_size`;
- **detach** when the `Renderer` begins stopping.

Under the lock, a change updates the state and increments the generation. When attached, it computes the **pixel snapshot**: the
`Arc<Image>`, the destination rectangle in pixels, the source rectangle after clipping, the filter and the generation. It then publishes
that snapshot to the GPU handle **before releasing the lock**.

The rectangle arithmetic is a few multiplications and the image is shared, not copied, so the lock is short. Decoding happens before the
lock is taken. A replaced `Arc<Image>` is swapped out under the lock and dropped after it is released, so freeing a large buffer never
holds the lock.

Because publication happens under the bridge lock, snapshots reach the GPU handle in generation order:
- an old-scale publication can never overwrite a newer one, since every metrics change and scene write is serialised and each
  publishes the conversion with the current metrics;
- attach publishes the current scene under the same lock that writes take, so a write made between start and attach cannot be stranded.

The main loop does not mediate an active write: the UI thread publishes directly.

### 4.2 The GPU handle

The GPU handle is a backend-owned, latest-value slot with a wake, which `ck-render-2d` drives through its backend trait. It holds plain
data: the pixel snapshot, in the GPU crate's own types (shared pixel storage, a rectangle, a filter, a generation), never
`ck-render-2d`'s `Image`. It holds no GPU object, so a handle kept alive after teardown can touch nothing on the GPU.

- **Lock order:** the bridge lock may be held while publishing to the handle. The render thread never takes the bridge lock, and no image
  buffer is dropped while either lock is held.
- **The first frame:** the handle exists, and is seeded with the bridge's state, before the render thread may draw its first frame. The
  presenter is constructed with the handle, or its first draw waits until attach. Today's presenters spawn their thread before `start`
  returns, so an attach after that could let a quad-less first frame win. The GPU issue spec makes this ordering explicit.

- `publish` replaces the slot's value and wakes the render thread. Its lock is held only to swap the value in, so it never waits for a
  frame, an upload or a stalled render thread.
- The render thread takes the latest value when it draws. It uploads a texture only when the snapshot's image differs from the resident
  one (by `Arc` identity). It records the generation it last drew in a bounded, coalesced observation: the latest value, not a report per
  frame. Tests assert that generation, not merely that some frame was queued. The existing reports (one first-frame report, terminal
  failures) and `ck-window`'s interface are unchanged.

### 4.3 End

- **Detach and end.** When the window loop starts teardown it stops the presenter. The `Renderer` detaches the GPU handle from the
  bridge first, and the phase becomes `Ended`.
- **Later writes are discarded and counted.** A scene write after `Ended` still validates and decodes, so script behaviour is unchanged.
  It then publishes nothing and touches no GPU state. It is counted, like `ck-window`'s discarded requests, and the count appears in the
  run's `debug` record. This covers a decode that finishes after teardown, or a write from the UI VM's `shutdown()`.
- **The CPU scene survives until the run ends.** It is dropped with the bridge.

### 4.4 Other rules

- **The clear colour still goes through `ck-window`.** A colour change and a quad change made together may land in different frames. Each
  converges to the latest value, and nothing animates, so no frame is wrong for longer than the next redraw.
- **Points to pixels:** one image pixel is one window point by default, so on a 2x display each image pixel covers 2x2 device pixels,
  which is crisp with nearest sampling. The issue spec settles the mechanism:
  - edge rounding: each edge at the rounded product of points and scale;
  - fractional and unequal horizontal and vertical scales;
  - clipping, which trims the source rectangle in proportion instead of stretching what remains;
  - a zero pixel extent, which draws nothing;
  - very large finite coordinates, clamped to representable backend geometry.
- **Texture lifetime:** the render thread owns the texture, its upload buffer and (Vulkan) its descriptor. A replaced or removed texture is
  released only after every command buffer (Metal) or frame (Vulkan, under the ledger of `docs/window-design.md` §8) that used it has
  completed. Partial setup, partial encoding, a panic after submit and a failed completion follow the existing proofs, and keep the
  objects on an unproven end.
- **A bounded retirement list.** Uploads happen only when a frame is drawn, and frames are bounded by the existing in-flight cap. So at
  any time there is one resident texture and one being uploaded, and retired textures number at most the in-flight cap. The specs enforce
  this with explicit admission and reaping against the bound, rather than inferring it from the frame count. Repeated writes
  while frames are stalled replace the slot's value without allocating on the GPU. Teardown keeps one absolute deadline, terminal failure
  reporting and the loader ownership of `docs/window-design.md` §7.
- **GPU allocation can fail** even within the size limits. A failed texture allocation or upload is a presenter failure through the
  existing report and exit rules.

## 5. The Lua surface (Proposal, ck-manager)

A new `render` extension, given to the VM named `ui` only, next to `window`. This needs `ck-lua` to install several extension tables on
one VM (§10, issue 1). Each table is still installed on the owner thread before the entry script runs. Table names are distinct and
callback names unique across a VM's tables, and a duplicate is refused when the settings are built. Window callbacks, posted-call routing
and cancellation are unchanged, and VMs other than `ui` get neither table.

The calls:

- `render.quad{image = "name.png", x = 0, y = 0, width = w, height = h}` decodes the image and sets the one quad, replacing any earlier
  one. `x` and `y` default to 0; `width` and `height` default to the image's size in pixels, read as points. Numbers must be finite;
  sizes must be greater than 0 and at most `MAX_DIMENSION`.
- `render.clear_quad()` removes it. With no quad it changes nothing, and it is counted like any accepted write.
- **Errors** are ordinary Lua errors from the call, naming the file and the reason: a missing or unreadable file, a file that is not a
  PNG or does not decode, an image over the budgets (§8), a name refused by the assets rule below, or a bad number. The script may catch
  them with `pcall`. **A failed call changes nothing:** the previously accepted quad stays.
- **Headless runs** validate and decode the same way, so a script behaves identically, and ignore the result. The first accepted call
  logs one `info render: scene calls ignored: headless run` record, counted with the window calls in the run's `debug` record. Headless
  takes precedence over the milestone 4 Vulkan warning (§7).
- **The assets rule** decides which files *this API* opens. It does not sandbox the script, which can still use Lua's `io`. The check
  is the same in windowed, headless and Vulkan runs:
  - The root is resolved at the first `render.quad` call, not when the configuration loads, so an existing configuration that never
    renders, with no assets directory beside it, starts and stops exactly as before. The first call canonicalises the root and keeps
    the result for the run. A missing or unusable root, whether the default or one configured explicitly, makes that call (and every
    later one) a Lua error naming the root, and changes no scene state.
  - A name must be relative, non-empty, and made of normal components only: no `..`, no `.`, no root or prefix.
  - The joined path is canonicalised, which resolves symlinks. It must lie beneath the canonical root. A symlink is allowed when its
    target stays beneath the root, and refused when the file or any directory on the way leads outside.
  - The file is then opened by that canonical path and decoded from the opened handle, never reopened by name.
  - **Limit, stated plainly:** this does not protect against a hostile process replacing paths inside the assets directory between the
    check and the open. The assets directory is trusted local files, and a race-safe confinement is out of scope until modding needs it.

## 6. Settings (Proposal, ck-manager; sampling is Firm)

A new `[render]` table in the configuration file:

```toml
[render]
filter = "nearest"        # nearest | linear (Firm: in the settings file; nearest for now)
assets = "../assets"      # where render.quad finds its images, relative to this file
```

- `filter` defaults to `nearest` and applies to every quad in the run.
- `assets` defaults to `assets` next to the configuration file. Without `--config`, there is no assets directory and `render.quad` raises
  an error naming that.
- Unknown keys are errors, as in the other tables. There is no command-line override.

## 7. Milestone 4.5: the Vulkan quad (Proposal, ck-manager; the non-UNORM fallback is Firm)

During milestone 4 the Vulkan path already goes through `ck-render-2d`: `Renderer<ClearOnly<VulkanAdapter>>` validates and decodes a
`render.quad`, logs one `warn render: textured quads on glfw-vulkan arrive in milestone 4.5` record per run, and draws nothing. Milestone
4.5 mirrors Metal (CK-91 to CK-93) under the owner's package rule (§1, 2026-10-22). Nothing moves between crates.

### 7.1 Who owns what

- **`ck-gpu-vulkan`** gains the drawing, in its own plain types and with no dependency on `ck-render-2d` (as CK-91 did for Metal):
  - shaders `shaders/quad.vert` and `shaders/quad.frag`, compiled to SPIR-V by `glslc` in `build.rs`, from an explicit path
    (`$VULKAN_SDK/bin`, `/usr/local/bin`, `/opt/homebrew/bin`), never only `PATH`. A missing `glslc` fails the build with the install
    command. `tools/bootstrap` installs it and the graphics doctor (`doctor.graphics`) checks it;
  - the pipeline (premultiplied blend `ONE, ONE_MINUS_SRC_ALPHA`), its layouts, a fixed descriptor pool and one sampler per filter
    (clamp to edge);
  - a staging-buffer upload into an `R8G8B8A8_UNORM` sampled image, with its layout transitions recorded in the frame's command buffer;
  - texture retirement under the existing ledger, the drawn generation, and a readback that draws the quad off screen.
- **`ck-render-2d`'s new `vulkan` feature** holds only the scene conversion: a sink that turns each `Snapshot` into the GPU crate's
  scene, with the weak wrapper cache and the once-only refusal latch of the Metal sink (CK-92, CK-94). It does not depend on
  `ck-window-glfw`, holds no Vulkan object and makes no Vulkan call. `metal.rs` is not changed (so no Metal or native-window group
  runs during the Vulkan work); sharing code between the two sinks can come later, in Metal work.
- **`codekarl`'s Vulkan modules** keep the run's set-up and its lifetime exactly as accepted (CK-80: the loader, GLFW, the instance,
  the surface, the destroy helper and the five audited `unsafe` operations). The adapter (`VulkanAdapter`) stops being a plain
  `ck_window::Presenter` wrapped by `ClearOnly` and becomes the `Gpu2d` backend itself: its `new_sink` gets the sink from the stack, and
  its `start` passes the matching slot to the presenter. The `Stack` seam gains the quad, so the failure-boundary fakes keep running the
  same code. `ClearOnly` and its warning leave the application; whether the type stays in `ck-render-2d` is the last issue's decision.

### 7.2 Interfaces and lifetimes

| Item | Owner and type | Created | Released | Notes |
| --- | --- | --- | --- | --- |
| Pixel source | `ck-gpu-vulkan`'s `PixelSource` trait (`Send + Sync`): premultiplied RGBA8 bytes, rows top first. `ck-render-2d` implements it over `Arc<Image>`, no copy | by the sink, one wrapper per image | when no scene and no cache entry holds it; the render thread drops its reference once the bytes are copied into the staging buffer | **Source identity** is the outer `Arc<dyn PixelSource>` allocation plus width and height, as Metal's; the sink's weak cache reuses one wrapper per image, so moving or refiltering a quad never uploads |
| Quad and scene | `VulkanQuad`, validated **without a device** by `VulkanQuad::new`: each side 1 to 8192, at most 64 MiB, exact byte length, destination in whole pixels, finite source inside the image, filter. `VulkanScene { generation, Option<VulkanQuad> }` | by the sink, under the bridge lock, possibly before start | with the slot's next value, dropped after the slot's lock is released | The generation travels with or without a quad. **Device checks come later** (§7.2.1) |
| Slot | `VulkanQuadSlot` (`Send + Sync`, shared by `Arc`), the latest value | by the stack's `new_sink`, before start; may be published to before start | with its last `Arc` | Holds no Vulkan object, so a publish after teardown touches nothing on the GPU. `publish` swaps under a short lock and signals the render thread; it never waits |
| Presenter | `VulkanPresenter::start_2d(instance, surface, size, clear, wake, slot)`, `unsafe`, the same contract as `start`; `start` stays clear-only and unchanged | by `Real::start`, as one of the five `unsafe` operations (it replaces the `start` call, so the count stays five) | as today: proven or unproven end | The render thread reads the slot before its first frame |
| Pipeline, layouts, descriptor pool, samplers | the render thread | at set-up, after the render pass, only when the chosen format is supported (§7.3) | proven teardown only, before the render pass and device, in the order of `docs/window-design.md` §7 | A failure creating them is a `Failed` report under the existing rules |
| Texture (image, memory, view, descriptor set) | the render thread | when a frame finds a pixel source unlike the resident one | **retired** when replaced or removed, destroyed once the submit fence of the last frame that used it signalled | At most one resident and one retired. While one is retired no new texture is created: the frame waits (redraw kept) |
| Staging buffer | the render thread | with its texture's upload | once the upload frame's submit fence signalled | At most one. Upload copies happen only in a frame, inside the existing one-frame-in-flight cap |
| Format notice | the presenter: `VulkanPresenter::take_quad_notice() -> Option<QuadSkipped>` (the format), beside the report queue, which keeps its two variants so the application's exhaustive mapping is untouched until it opts in. The application's `Running`/`Stack` seam carries it | the first frame that has a quad while the format is unsupported (§7.3) | taken by the main thread, at the latest in the final drain after the end is observed (§7.3) | Once per run. The render thread calls the presenter's `wake` after recording it, as after a report. It is a log record only, never a report, so terminal failures keep their priority |
| Refusal | `ck-render-2d`'s latch, as Metal's after CK-94 | when the GPU crate refuses a converted quad (defence in depth: placement prevents it) | `take_report` on the main thread, once, as `Failed` | The sink calls the loop's wake; the latch is once-only under concurrency from the start |

**GPU memory bound.** The enforced bound is an **object cap**: at most two quad textures (one resident, one retired) and one staging
buffer, counted from the moment each object's creation begins, so a partial creation counts. No third texture or second staging buffer
is created while the cap is reached: the frame waits, its redraw kept. Their **payload** is at most 64 MiB each, so at most 192 MiB of
image data. The memory actually allocated for each object is its `VkMemoryRequirements::size` (layout and alignment included), which
the payload figure does not cover: it is not capped separately, and the observer records each allocation's queried size so the tests can
check the cap and the measured sizes. This quad memory is in addition to the swap chain, the fixed pools and any readback output.
Repeated writes while frames are stalled replace the slot's value and allocate nothing. GPU allocation can still fail within the limits;
that is a `Failed` report naming the size (§4.4).

#### 7.2.1 Device checks on the render thread

The sink and slot exist, and may be published to, before the render thread has chosen a device, so `VulkanQuad::new` checks only what
needs no device. On the render thread, after device selection and **before creating any object for a quad**, the presenter checks the
chosen device: the image fits its `maxImageDimension2D` (Vulkan only guarantees 4096), and `R8G8B8A8_UNORM` supports sampling and
transfer destination with optimal tiling. A quad that fails them is a `Failed` report naming the image's size and the device's limit (or
the missing capability), under the existing rules: nothing more is drawn, and cleanup follows the proofs. No object is created for it,
and `start`'s contract (an `Err` means no thread and no GPU object) is unchanged, since these checks run on the thread. Tests: the
constructor's boundaries (8192 accepted, 8193 refused, exactly 64 MiB accepted, one row more refused, a wrong byte length refused) and a
test hook that lowers the device limit the thread sees, giving one `Failed` with no texture created.

**Wakes:**
- a slot publish wakes the render thread only, never the main loop;
- the presenter's existing `wake` (the notifier's) runs on the render thread after every report and after the format notice;
- the sink's refusal sets a sticky flag and calls the notifier's wake, under the bridge lock (it only sets a flag and posts a native wake).
The application builds both wakes from the run's one notifier in its production factory (`run_closure_with`), and a committed test
proves that wiring through that factory (the gap CK-93's review noted for `ClearOnly`).

**Every end path** keeps milestone 4's rules: `Renderer::request_stop` detaches the bridge before forwarding the stop; the application
detaches the bridge on every path (CK-93), headless, no window, a quit before start, a failed window or start-up, a cancel. A failed
`start_2d` spawns nothing and creates no GPU object, and the bridge is detached. A proven end destroys every new object; an unproven
end keeps every one (and the instance, surface, window and loader, as now). The slot and the CPU scene may outlive the presenter; they
hold no GPU object.

### 7.3 The colour-format contract (Firm fallback, owner 2026-10-21)

Textured drawing uses the stored-value model of §8, which holds only on a UNORM attachment. The swap chain's format choice is unchanged:
it is chosen once at set-up, preferring `B8G8R8A8_UNORM`, then `B8G8R8A8_SRGB`, then the first format the surface offers, so no surface
that works today stops working and clear-only behaviour is untouched. **Supported** means `B8G8R8A8_UNORM` or `R8G8B8A8_UNORM`. On any
other format the quad is not drawn: the first frame that has a quad records one `QuadSkipped` notice naming the format, and the window
keeps clearing. The pipeline is not created then, so an unsupported format cannot fail set-up.

**Logging the notice (the rule `ClearOnly` follows today).** The application's adapter takes the notice through the `Running` seam on
every `Gpu2d` call and logs it as one `warn render: textured quad skipped: the glfw-vulkan surface format is <format>, not UNORM` record
through `try_log`, which never waits. While the run is active a refused record is kept and tried again at the next call; on the end
paths (`request_stop`, `poll_end`) a refused record is given up. **Final drain:** when `poll_end` observes the end (proven or unproven),
the adapter takes the notice once more **before** it drains the remaining reports and releases the presenter handle, so a notice
recorded after the last ordinary call is not lost to the handle's removal. Any time spent there comes out of `poll_end`'s own budget.
A stalled or full log queue therefore never delays the end: the warning is best-effort under that saturation, and the logger counts a
dropped record. Tests over the fakes: a notice that becomes available during `poll_end`, with a proven and with an unproven end; a
saturated logger (the record refused, given up, no wait); the existing report draining and failure priority unchanged.

Tests force the format choice through a test hook that restricts the formats the thread sees to a subset of what the real surface
actually offers, never an invented format: the preferred UNORM, an sRGB-only list and an unsupported one (a format the surface offers
outside the supported pair, for example `A2B10G10R10_UNORM_PACK32` or `R16G16B16A16_SFLOAT` on this Mac's MoltenVK). If the surface
offers no such format the test fails and names the formats it saw: the full level does not skip it. They check the drawn quad on the supported path, with
known-channel alpha results, and the one notice and the plain clear on the others. An off-screen readback alone is not proof of the
surface path.

### 7.4 Tests and the owner's rule

The owner's rule for this work: no `metal.gpu` or `native.window` run while only Vulkan code changes. The issues stay out of Metal files,
and `tools/check.py` gives the new `ck-render-2d` Vulkan files to the plain and Vulkan groups, never to `native.window`. That covers
every input the new feature touches, not only new `.rs` files: the `Cargo.toml` feature entries, the shaders and `build.rs` inputs, and
any cross-file guard that reads them, with catalog selection tests. A change that is truly mixed still selects every group it affects.
The batch's full level still runs every group once.

- **`ck-gpu-vulkan`, no window (`vulkan.gpu`):** readbacks of a quad under validation, exact as Metal's: nearest blocks, the
  premultiplied blend over a known clear, linear within a tolerance, a clipped source, no quad gives the clear; `VulkanQuad::new`
  boundaries; the ledger's texture and staging bookkeeping without a device.
- **`ck-gpu-vulkan`, real GLFW windows (`vulkan.window`):** a quad published before start is in the first frame (by generation); a
  later "no quad" scene; no allocation while frames are stalled; never more than two textures and one staging buffer; release only
  after the fence; no upload for a move or a filter change; no CPU reference after the copy; a proven end releases everything and an
  unproven end nothing; forced allocation and pipeline failures each give one `Failed`; the forced formats of §7.3; `start` without a
  slot unchanged. Hook isolation (`vulkan.isolation`) covers every new hook name.
- **`ck-render-2d` (plain):** the conversion, the wrapper reuse and the weak cache, the once-only refusal latch with a gated concurrent
  regression, delivery to an idle `ck_window::run` loop.
- **`codekarl` (`rust.app`, and `vulkan.app` for the real window):** the `Stack` fakes carry the quad through every failure-boundary
  case; the production factory's wakes; the format warning's text; a real `glfw-vulkan` run that draws the quad, observed by
  generation, closed by the script itself with the owner-typing convention of the existing `vulkan_app` tests.

**Owner demonstration (milestone 4.5):** `config/demo-quad-vulkan.toml`, the same script and image as milestone 4 on `glfw-vulkan`.

## 8. Images and colour (Proposal, ck-manager)

- **Decoder:** the `png` crate, pure Rust and widely used. Every PNG colour type and bit depth is converted to 8-bit RGBA: grey, grey with
  alpha, RGB, palette with transparency, 16-bit (reduced to 8) and interlaced. A generic "expand" transformation alone does not produce
  RGBA8 for every input, so the conversion is explicit and tested per type.
- **Premultiplied alpha** at load, so linear sampling and blending give no dark fringes. Each channel is premultiplied with integer
  rounding, `(c × a + 127) / 255`, and a fully transparent pixel becomes all zeros. The blend is `ONE, ONE_MINUS_SRC_ALPHA`:
  `result = source + destination × (1 − source alpha)`. Row 0 of the image is its top, and texture coordinates start at the top-left.
- **No colour conversion:** image bytes go to the screen as they are, like the clear colour (§2). Blending therefore happens on the
  stored values, as most 2D engines do. That is exact on a UNORM attachment (Metal always; Vulkan when it chose UNORM, §7). An
  sRGB-correct pipeline would change how today's clear colours look, so it is left for later.
- **Budgets** (fixed in the core spec, CK-90 r2, after its spec review 1):

  | Budget | Value | What it measures |
  | --- | --- | --- |
  | Encoded file | at most 64 MiB | Streamed through one 64 KiB read buffer, never held whole. |
  | Dimensions | at most 8192 per side | Checked from the header before any image-sized allocation. |
  | Decoded RGBA8 image | at most 64 MiB, for example 4096 × 4096 or 8192 × 2048 | Lowered from this design's earlier 256 MiB proposal. |
  | Decoding intermediate | one raw row plus the read buffer | Rows, and interlaced passes' rows, convert straight into the final buffer, so a 16-bit image never needs a whole raw frame. |
  | `png`'s own buffers | a 16 MiB limit | The crate calls this best-effort, and it does not count its caller's buffers, so it bounds only the library. |

  The chunks the pixel conversion needs are kept: `PLTE` and `tRNS`. A `tRNS` grey or RGB sample is compared at the image's full bit
  depth, before any 16-to-8 reduction. Unused metadata (text, ICC, gamma, sRGB and unknown ancillary chunks) is skipped without being
  stored. An image is never refused for metadata alone, since the encoded cap bounds the cost.
- **Residency:** the bridge has exactly one writer (`Send`, not `Sync`, not `Clone`), the UI VM's thread. The renderer owns at most
  four distinct decoded images at once, at most 256 MiB:
  - the bridge's scene;
  - the sink's latest snapshot;
  - the one the render thread holds while uploading (a backend drops its CPU reference once the upload completes);
  - the writer thread's one in-hand image: either the image it is decoding or the one it just swapped out, never both. GPU residency is one resident texture, one in upload, and retired
  textures up to the in-flight cap. Each backend's spec (milestone 4's issue 3 for Metal, and the milestone 4.5 Vulkan spec) fixes that bound
  before implementation.
- **Dimensions and the GPU:** Metal on Apple GPUs and MoltenVK both allow 16384 per side, so the dimension limit is compatible with this
  Mac's GPUs. That is a dimension claim, not a promise that allocation succeeds: an actual allocation failure is a presenter failure
  (§4.4).

## 9. Proposed acceptance evidence

**Automated** (in `tools/ck check`):
- **`ck-lua`:** two extension tables on one VM, both usable from the same script; window callbacks still delivered; duplicate table or
  callback names refused deterministically; no extension in unrelated VMs; a render error catchable without disturbing the window
  table or cancellation; every existing `ck-lua` test unchanged.
- **`ck-render-2d`, a new plain check group, with no GPU feature:**
  - decoding small committed PNG fixtures of every colour type, bit depth and interlacing to the exact expected premultiplied bytes;
  - refusing truncated, non-PNG, oversized-dimension and oversized-file images with their reasons, before any large allocation. A
    small image with large metadata under the encoded cap decodes, its metadata skipped;
  - one total-allocation envelope for a decode (the final image, a raw row, the read buffer, `png`'s 16 MiB and 1 MiB of slack),
    including a 16-bit interlaced image;
  - points to pixels at scale 1, 2, fractional and unequal scales, the default size, clipping with proportional source trimming, zero
    pixel extent and very large coordinates;
  - the bridge, with barrier-controlled interleavings:
    - a write before start drawn by the first frame;
    - attach racing a write, so nothing is stranded;
    - a scale change racing a write, so the last publication uses the current metrics;
    - stop racing a write, so nothing is published after detach and the write is counted;
    - a late decode after proven cleanup, discarded and counted;
    - a stalled GPU handle, while the producer keeps making progress;
  - the main loop not involved in an active write; decoding and buffer drops outside the bridge lock;
  - the assets rule: nested names; absolute, empty, `.` and `..` names refused; a file link and a directory link escaping the root
    refused; a link inside the root allowed; the same results in every run mode;
  - the `filter` and `assets` settings: absent configuration, table or key; an unknown filter or key;
  - the `Renderer` over a fake backend that records each snapshot and generation. The fake distinguishes a backend that draws the quad
    from a clear-only one.
- **`ck-gpu-metal`, in `metal.gpu`:**
  - an off-screen readback of a textured quad. With nearest sampling the pixels are exact: a 2x2 checker drawn at 8x8 gives 4x4 blocks;
  - half-transparent pixels blended over a known clear colour, exact to the premultiplication rule;
  - linear sampling, checked within a stated tolerance and different from nearest at the block edges;
  - a quad partly outside the target, clipped without stretching;
  - replacing and removing the quad, with the observer counting texture releases only after completion;
  - the retirement bound under repeated writes with frames stalled;
  - fault tests that leave every texture and buffer alive on an unproven end: partial setup and encoding, a failed completion, a panic
    after submit, an allocation failure reported as a presenter failure;
  - the drawn generation in the frame report;
  - Metal validation with no messages, and every existing test unchanged.
- **`codekarl`, without a window (`rust.app`):**
  - `render.quad` errors: a missing file, a non-PNG file, an absolute or escaping name, a bad number. After each, the previous quad
    is unchanged;
  - `render.clear_quad` with and without a quad;
  - a headless run validates and ignores, with its one record, and takes precedence over the Vulkan warning;
  - over a fake GPU, both backends receive the same quad in pixels from the same script;
  - on the Vulkan path, the milestone 4 warning;
  - the source guard: no application file except the backend choice names a GPU crate.
- **`codekarl`, with a window (`native.window`):** one case where the demo script with a fixture image opens a real native window,
  reports a frame with the submitted quad's generation, and closes on Escape with exit 0. A queued frame is not proof of what is seen,
  so the owner's demonstration remains the visual evidence.

**Owner demonstration (milestone 4):** `config/demo-quad.toml` with the owner's image in `assets/`. The window opens with the image placed
by `scripts/ui/demo_quad.lua`, nearest-sampled, transparent parts showing the clear colour, and Escape closes it. Until the owner's file
arrives the demo uses a committed fixture image.

## 10. Proposed issues (milestones 4 and 4.5)

Filed one at a time after this design is reviewed, each with fresh independent review:

1. **`ck-lua` several extension tables per VM** (the prerequisite of §5), with its tests.
2. **`ck-render-2d` core:**
   - the crate and workspace member;
   - `Image` with the `png` decoder, `Filter`, `Quad` and points to pixels;
   - the bridge (§4), the backend trait and `Renderer`, over a fake backend;
   - the assets rule and the decoder budgets;
   - its check group, with catalog ownership.
3. **`ck-gpu-metal` textured quad:** the quad shader, texture upload, samplers, blending, the latest-value GPU handle, the readback with a
   quad, the retirement bound, the fault tests and the observer counters.
4. **The `ck-render-2d` backends:** the full Metal backend and the clear-only Vulkan backend.
5. **`codekarl` through `ck-render-2d`:**
   - the `[render]` settings and the `render` Lua extension;
   - both run paths through `Renderer`, and the source guard (registered in the catalog's guard inventory);
   - the demo configuration, script and assets directory.

**Milestone 4.5** (filed one at a time after its design review, each with fresh independent review; §7):

6. **`ck-gpu-vulkan` quad, off screen:** the `glslc` toolchain (`build.rs`, `tools/bootstrap`, the graphics doctor), the shaders, the
   plain types (`PixelSource`, `VulkanQuad`, `VulkanScene`, `VulkanQuadSlot`), the pipeline, samplers and staging upload, and the quad
   readback with its exact pixel tests (`vulkan.gpu`).
7. **`ck-gpu-vulkan` quad, presented:** `start_2d`, the slot read before the first frame, upload and retirement under the ledger, the
   memory bound and its admission, the drawn generation, the `QuadSkipped` notice and the forced formats, the fault and teardown tests
   (`vulkan.window`, `vulkan.isolation`).
8. **`ck-render-2d` `vulkan` feature:** the scene conversion, the wrapper cache and the once-only refusal latch, with the catalog
   ownership that keeps it out of `native.window`.
9. **`codekarl` through the Vulkan quad:** the `Stack` seam carries the quad, `VulkanAdapter` becomes the `Gpu2d` backend, `ClearOnly`
   leaves the application, the format warning, the production-factory wake regression, the real `glfw-vulkan` case and
   `config/demo-quad-vulkan.toml`.

## 11. Open decisions

### Owner questions

None open: the owner answered all four on 2026-10-21 (§1), each as recommended. They were:

1. **Assets:** images are named relative to a configured assets directory, `assets/` by default. The render API opens nothing outside it,
   symlinks included, though scripts themselves are not sandboxed (§5, §6). Is that the right home for the owner's image and later art?
2. **Vulkan during milestone 4:** a quad on the GLFW and Vulkan path logs one warning and draws nothing until milestone 4.5 (§7), rather
   than failing the script. Agreed?
3. **Scale:** one image pixel is one window point by default, so on this Mac's display each image pixel is 2x2 device pixels (§4). Or
   should one image pixel be one device pixel?
4. **Vulkan surfaces without UNORM, after milestone 4.5** (§7). This Mac's MoltenVK offers UNORM, so this matters for other machines, such
   as Linux later. When the surface only offers sRGB or another format:
   - (a) keep clearing and skip the quad with one warning (ck-manager's recommendation: nothing that works today stops working);
   - (b) treat a quad request there as a failure; or
   - (c) build a composition path that gives the same result on sRGB.

   The format handling and its tests are mechanism, but this fallback is visible behaviour, so it is yours to choose.

### Designer recommendations (mechanism; settled in issue specs and their reviews)

- the package layering, with GPU crates independent of `ck-render-2d` (§3);
- the scene bridge outside `ck-window`, decoding in the Lua call, publication under the bridge lock, the GPU handle, the end rules,
  the texture lifetime and the retirement bound (§4);
- several extension tables per Lua VM (§5);
- the Vulkan colour-format handling: keep today's format choice, detect a non-UNORM attachment, and test the forced cases (§7). The
  fallback on such a surface is owner question 4;
- the `render.quad` and `render.clear_quad` names and errors (§5);
- premultiplied alpha, no colour conversion and the decoder budgets (§8);
- the `png` crate (§8).

## Provenance

Drafted by ck-manager (Claude Opus 5.5, Class S, high effort) on 2026-10-21 from:
- the owner's direction and answers given directly to ck-manager that day (§1);
- `docs/window-design.md`, `docs/runtime-design.md` and `docs/vision.md`;
- the accepted repository state after batch `1792596621943066000`.

Revised the same day for independent design review 1 (`.pipeline/coordination/runtime-m3/design-review-1-m4.md`, ck-reviewer-1,
CHANGES_REQUESTED on `f64c4ffe…`):
- F1: the scene bridge, its attach, scale and end transitions;
- F2: the assets rule with symlinks, and its stated limits;
- F3: the `ck-lua` prerequisite;
- F4: the corrected format facts and the Vulkan colour-format contract.

Revised again for design review 2 (`design-review-2-m4.md`, CHANGES_REQUESTED on `81fda222…`):
- F5: the chunks the conversion needs are kept;
- F6: the assets root is resolved at first use, so existing configurations start unchanged;
- F7: the non-UNORM fallback is owner question 4.

It also takes the review's notes: no decode-time bound promised, budgets marked as a proposal, first-frame seeding, lock order, a
bounded generation observation, and enforced retirement admission.

Review 1's spec prerequisites, folded in with the first revision: decoder budgets, the bounded retirement list, rounding and clipping, the alpha details,
failed-call state, and the drawn generation in frame reports.

Revised on 2026-10-22 for milestone 4.5 (§1's package rule, §3's wording, §7 and §10), after the milestone 4 guide review
(`docs/guide/20261022t135100z-bcf77413.md`), which asked that the 4.5 interfaces and lifetimes be settled in one table before dispatch.
Revised the same day for design review 4 (`runtime-m4/design-review-4-m45.md`, ck-reviewer-1, CHANGES_REQUESTED on `79434a55…`): F1
device checks on the render thread (§7.2.1), F2 the object cap and what the payload bound measures, F3 the format notice's final drain
and non-waiting logging (§7.3).

Handoff: `.pipeline/coordination/runtime-m3/handoff.md`, and `runtime-m4/handoff.md` from milestone 4 on.
