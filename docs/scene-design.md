# Milestones 5 and 5.5: live resources, scenes and input

**Status: design approved, 2026-10-23.** Written by the Codex co-designer (Class S, `gpt-6-astra`, high) from the owner's direction and ck-manager's requirements; verified by ck-manager; approved by independent design reviews 1 and 2 of both brands (ck-reviewer-2, Claude; ck-reviewer-1, Codex) on revision 4, sha256 `3b713870…985fbd`. Records in `.pipeline/coordination/runtime-m5/`. This is a design, not an implementation specification: each issue in §11 still needs its own spec and readiness review, and none is filed yet. On publication ck-manager added the §12 row for the all-quads-moving decision, as design review 2 suggested; nothing else changed.

**Firm** marks owner decisions, with their source/date. **Proposal** marks designer recommendations. **Open** marks decisions that still need an answer. Unless explicitly marked otherwise, the mechanisms, API names and numerical defaults below are **Proposal**. A manager's requested design constraint is identified as such, not attributed to the owner.

This extends `docs/runtime-design.md`, `docs/window-design.md` and `docs/render-design.md`. Existing lifecycle, failure reporting, color, window ownership and shutdown rules remain applicable except where a change is explicitly proposed here. The resource/scene substrate is the main result; a moving quad demonstrates it. Map representation is not defined by this milestone.

## 1. Owner direction and the milestone boundary

### 1.1 Firm direction

**Firm — owner, 2026-10-23, recorded in the M5 design-session log:**

> sure we can make the quad move as 5, but it cant just be funcitonality, we need to architect the structure that's going to hold the
> whole map, so likely implement a live texture loading, bindless with reusable handles, system that we use from lua to generate the quad,
> then also implement the configurable input code and settings. tell me if that's wrong

**Firm — owner answers, 2026-10-23, recorded verbatim in the design-session log:**

> for an unloaded texture i want a magenta checkerboard, explicit texture.reload, lua cannot
> create textures from raw pixels in this milestone, and roguhly how many quads it can handle is in the thousands, perhaps tens of
> thousands, that's why i need bindless.

Asked whether bindless belongs at the end of 5 and 5.5, after 5.5, or from the start, the owner selected the first option:

> i think your recommendation is fine, but one clarification, i want tens of thousands of quads and also tens of
> thousands of loaded textures, that's why i need bindless

**Firm.** Lifecycle architecture comes first, with ordinary instanced/batched drawing as an intermediate stage. Bindless is the **last stage of M5 on Metal and of M5.5 on Vulkan**, designed now and required for completion of each milestone. This supersedes the earlier agreement to put it after 5.5. Pending and failed initial loads display a magenta checkerboard; reload is explicit; Lua raw-pixel creation and automatic file watching are outside M5. Rust CPU sources and the future GPU-produced-source boundary remain in the architecture. Tens of thousands applies independently to quads and loaded textures; a large quad count sharing a few images is insufficient acceptance evidence.

**Firm — remaining M5 choices, owner 2026-10-23, recorded verbatim in the design-session log:**

> 1) A, 2) that sounds fine for now, 3) it doesnt matter to me, i guess pause makes the most sense

The three answers select upload choice A (worker preparation, render-owned submission), accept the §11.3 performance targets provisionally, and pause updates when unfocused/minimized, with held input cleared on focus loss. The targets are owner-accepted **for now** and may be revised by the owner after measurement; they are not measured results.

**Firm — inherited decisions:** GPU-specific code stays in the GPU crates (owner 2026-10-22); both backends are interchangeable through `ck-render-2d`; the UI VM owns presentation and window callbacks; the gameplay VM remains a separate execution owner; sampling comes from data-file settings, initially nearest. Map storage, tiles, geographic resolution, algorithms and GPU layout remain deliberately Open in the vision. Test windows must quietly discard the owner's keyboard input.

### 1.2 Scope and non-goals

**Proposal.** M5 establishes generational texture/quad handles, a bounded resource registry, PNG and Rust CPU-buffer sources, asynchronous file loading, explicit content replacement/reload, multiple persistent quads, paced UI updates, configurable keyboard actions and a native/Metal demonstration. Both milestones finish with bindless drawing at the owner’s scale. M5.5 supplies GLFW input and Vulkan resource/drawing parity through the same application/Lua contract.

**Manager's design constraints, adopted in this proposal:** distinguish logical texture identity, content revision and backend binding index; never expose that binding index to Lua or `ck-render-2d`; put pixels rather than filesystem paths at the upload boundary; preserve the legacy quad call; do not bake per-draw binding operations into neutral scene types.

**Firm — settled M5 choices (§12).** The owner has selected upload A, accepted the §11.3 performance targets provisionally, and selected unfocused/minimized pause, in addition to appearance, reload, Lua pixel scope, scale and bindless sequencing. Other numerical defaults and the detailed stress recipe remain Proposal for design verification; these three settled choices are no longer Open.

**Proposal.** Exclude map storage/generation algorithms, ECS, general scene graph, simulation scheduling, cross-VM rendering mutation, live backend switching, Lua hot reload, settings UI, runtime rebinding, text/IME entry and a general-purpose task system. Future GPU-produced textures must fit the identity/publication boundary, but M5 does not expose native writable images or build a render graph.

## 2. Existing behavior and the changes required

The accepted design and inspected sources provide these starting facts:

| Existing behavior | M5 proposal | Contract to preserve |
| --- | --- | --- |
| `render.quad{image=...}` decodes PNG synchronously in the UI call | Add a separate asynchronous texture API; retain the old call (§8.4) | Legacy file errors remain catchable at the call, with no scene change on failure |
| `SceneBridge` contains one `Option<Quad>` and one writer | Bounded ordered scene with persistent quad handles | Single UI scene writer, coherent snapshots, no GPU objects in the bridge |
| Texture identity is an `Arc` allocation, tracked weakly by the backend | Explicit logical identity and content revision | Movement/refiltering alone never re-uploads pixels |
| One resident texture, one retired texture and bounded staging | Bounded sets of live/reloading/retired textures (§7) | Admission before growth; release only with completion proof |
| Every GPU object is owned by the render thread | Upload choice A: worker preparation, render-owned submission (§6) | Main thread never performs blocking GPU work; no blanket `Send + Sync` change |
| Redraw on demand; `Wait` has only `Poll` and `UntilEvent` | Timed waits and an optional paced UI update source | Idle waits, sticky wakes, quit priority and native modal limitations |
| Escape directly produces a close request | Normalized physical keys mapped to actions, Escape as default close binding | Close callback accepts/declines; signals bypass Lua |
| Lua extensions exchange `ConfigValue`, including UTF-8 strings | String handle tokens; existing callback registration/posting | No `mlua` types in application-facing contracts; no cross-thread Lua state |

Relevant sources: `crates/render-2d/src/{bridge,quad,renderer}.rs`, `apps/codekarl/src/{render_api,ui_link,config}.rs`, `crates/lua/src/{extension,convert}.rs`, `crates/window/src/{backend,link,main_loop}.rs`, `crates/gpu-metal/src/lib.rs`, `crates/gpu-vulkan/src/{draw,residency,ledger}.rs`.

These observations are source inspection, not a new device-capability or performance measurement. Changes accepted after that inspection must be reconciled before issue specs are filed.

## 3. Package and thread boundaries

**Proposal.** Introduce two small reusable CPU-only components because the milestone now supplies their concrete use cases:

| Component | Owns | Does not own |
| --- | --- | --- |
| `ck-resources` | Texture identity/ownership, immutable CPU pixel revisions, PNG/assets validation, bounded loader service, per-resource status and budgets | Quads, Lua, native GPU objects, descriptor indices |
| `ck-input` | Portable physical key names, validated action bindings, held states, bounded transitions and focus reset | Window creation, Lua, native key codes or settings-file discovery |
| `ck-render-2d` | Ordered quads, logical resource references, point-to-pixel conversion, scene snapshots, backend-neutral renderer interface | Texture slots in a native descriptor table; Metal/Vulkan calls beyond existing thin construction/conversion seams |
| GPU crates | Native texture storage, upload preparation/submission, ordinary and bindless binding, instancing/batching, frame/upload ledgers and retirement | Asset paths, action names, Lua, map concepts |
| Window core/backends | Main-thread window/event processing, normalized input delivery, timed waits, existing close/teardown protocol | Resource loading, drawing implementation, gameplay scheduling |
| `ck-lua` | Owner-thread execution and a general callback-completion boundary if needed for scene publication | Meaning of an action, texture or frame |
| Application | TOML schema, service composition, UI frame scheduling, Lua extensions, backend setup and exit-status policy | GPU drawing implementation |

Move the current PNG/image/assets implementation to the resource component as one focused step, retaining `ck-render-2d` re-exports where needed for compatibility. Do not combine that move with changes to decoding/color rules. `ck-render-2d` may depend on `ck-resources`; GPU crates do not. Adapters pass backend-owned plain types containing validated logical keys/revisions and immutable pixel sources, as they currently adapt `PixelSource`.

**Proposal — owners.** Main thread: window, action accumulation, scheduling and supervision. UI owner: Lua and the authoritative editable quad collection. Resource service: resource state machine and bounded decode jobs. Render owner: native submission, active revisions and GPU lifetime ledger. The owner-selected choice A adds one upload-preparation worker per active backend (§6). Gameplay keeps its existing owner and no render extension. Only the selected backend runs services.

No queue grows without a declared cap. Native callbacks do constant recording work. Service/VM/render messages never wait for a peer to make progress. Locks protect short state exchanges, never I/O, decoding, Lua, native GPU calls, or a large buffer drop. Wakes use the existing lifetime-aware notifier; a late completion cannot call a destroyed window system.

## 4. Texture identity, ownership and retirement

### 4.1 Three identities

**Proposal.** Use these distinct concepts:

| Name | Representation | Rule |
| --- | --- | --- |
| Logical texture key | Registry/run identity + slot + generation | Stable for the resource's life; never means a native descriptor index |
| Content revision | Monotonically increasing revision within that logical key | Changes for reload/replacement, not movement; never wraps |
| Backend binding identity | Private native allocation/descriptor bookkeeping | May change without changing a logical handle; absent from neutral scene types |

Quad keys are a separate type/registry. Rust APIs keep types distinct. Lua gets bounded-length opaque strings with a version, kind, registry identity, slot and generation. Parse strictly: exact fields, bounded decimal/hex widths, checked integer conversion, no float conversion, no ignored suffixes. Check kind, run/registry, VM authority, live public ownership and generation on every API use. Tokens are process-local, not persistent asset IDs or save data. They are not a sandbox/security boundary; the Lua runtime remains unsandboxed.

Slot and generation widths are fixed in the core spec (proposed `u32` slot and `u64` generation). On generation/revision exhaustion, refuse further reuse/update rather than wrap. Registry IDs must distinguish simultaneous registries and later registry incarnations within the process; restarting a process never makes saved tokens an accepted input format.

### 4.2 Public ownership and scene references

**Proposal.** One `texture.load` or Rust CPU-source creation makes one logical resource and one public ownership token. Initially, repeated loads of the same path are separate resources: file deduplication is not implicit reference-counting policy. Many quads can reference the one resource. A quad holds an internal retained reference independently of the public token.

`texture.release(token)` invalidates the public token immediately and releases that ownership. Existing quads keep working; no new operation may use the released token. A second release is a stale-handle error. `render.destroy_quad` removes that quad and its reference; destroying the final reference starts resource retirement. Run shutdown releases every remaining public and scene reference, so forgotten Lua releases cannot outlive the run. Strings have no finalizer; garbage collection is not a reclamation protocol.

Only UI is permitted to create/mutate presentation resources in M5. The application can supply Rust-generated resources through the same registry with explicit ownership. Cross-VM transfer and gameplay presentation snapshots remain Open; two independent VMs never mutate one quad implicitly.

### 4.3 Resource state machine

**Proposal.** Track public ownership, the active content revision and one bounded replacement attempt separately:

| Transition | Effect |
| --- | --- |
| Admitted request | Allocate logical slot; status `pending`; schedule source work |
| Source work | `decoding` for PNG or validating/preparing a CPU source |
| CPU pixels complete | Freeze immutable pixel revision; status `uploading` in a windowed supported backend, `cpu_ready` headless |
| Upload proven safe | Backend reports a prepared revision as ready; resource service accepts it only if still current |
| Active revision published | Future frames may use it; `ready` after backend adoption acknowledgement |
| First load fails | `failed`, with bounded error text; pending appearance remains |
| Replacement fails | Keep last active revision; report `ready` plus reload error and failed requested revision |
| Last reference ends | Stop admission for this resource, cancel pending work, begin `retiring` |
| All references and work proven gone | Release native/CPU resources and make the logical slot reusable with a new generation |

Each job/result carries registry, slot, generation and requested revision. A newly admitted reload supersedes prior uncommitted work. If a prior result arrives later, it is discarded/retired and cannot replace the active revision. Serialize the current-request check and active-revision adoption through the resource state machine. The backend first prepares a usable immutable native publication (§13.1); the application adapter then compare-and-commits that prepared publication against the still-current requested revision. The bounded commit exchanges references/state only, with no GPU calls or large copies under a lock. A mismatch rejects the prepared revision. The service must not advertise an active revision without a matching usable native publication, and the backend must not promote an upload on its own. A frame already built before the new publication may finish with the old revision.

At most one executing replacement plus one coalesced latest request per resource; coalescing still obeys the global job cap. Superseding cannot overwrite a submitted upload's ledger entry. Old native work finishes into retirement even when no caller wants its result.

### 4.4 Exact reclamation condition

**Proposal.** Removal from the current scene is not enough to free a texture. Reclamation needs all of:

1. No public owner, live quad, queued scene snapshot or unsubmitted frame holds a reference capable of using it.
2. Every upload that can write its native storage is complete, or proven never submitted and no longer used by a worker.
3. Every submitted frame that can read it has the backend's completion proof.
4. No outstanding worker/result/adoption operation can republish that key/revision.
5. No native binding-table version retains a descriptor/resource ID for it that can reach a future submission; reusable versions must be sanitized before dropping their pins (§13).

Initially delay logical-slot reuse as well as native-slot reuse until this condition holds. This conservative policy simplifies stale-handle and lifetime proofs. Decode/upload cancellation is advisory until its owner is observed finished with the job. A stale CPU result must return its budget reservation even when discarded.

Each frame pins its exact native content revisions and binding objects until command completion. A hot reload allocates a fresh revision; it never overwrites sampled pixels or a live descriptor. Texture readers require GPU command-completion proof; existing drawable/swapchain presentation proofs additionally govern presentation objects. Preserve that distinction and the existing unproven-end retention rule.

## 5. Sources, loading and replacement

### 5.1 Source interface

**Proposal.** A resource has one of two implemented sources:

- **PNG file:** relative to the configured assets root, with the accepted path/symlink policy. All filesystem resolution, opening, reading and decoding for the new API runs off UI/main/render threads. Cheap syntax validation and admission happen in the caller. The trusted-local-files race limitation remains explicit, as in M4.
- **CPU pixels:** a Rust caller transfers an immutable, owned RGBA8 buffer under a budget reservation. Width, height, row layout, alpha convention and byte length are checked. The low-level upload interface takes premultiplied RGBA8 rows from top to bottom. A high-level constructor may premultiply straight-alpha input on a loader worker, explicitly named so it cannot double-premultiply.

Freeze ownership: no GPU-facing source aliases mutable caller memory. Zero-copy transfer is allowed only when exclusive ownership/freeze is established. Rust producers reserve capacity before generating large buffers if they want those allocations inside the engine bound; arbitrary memory allocated before calling the API is outside that bound and must not be counted as controlled by the engine.

**Proposal.** Both sources support replacing complete content while retaining the logical handle. File sources use `texture.reload`; CPU sources use Rust `replace_pixels`. Partial-region updates and GPU-written sources remain later mechanisms. Keep logical identity and adoption/retirement independent of whether a future backend revision contains uploaded pixels or the output of a GPU pass. Future writable access requires its own synchronization contract; M5 promises this extension boundary, not that GPU writes already work.

### 5.2 Admission, errors and pending appearance

**Proposal.** `texture.load` returns after syntax validation, slot/job admission and publication of the request. It makes no filesystem call. Missing roots/files, canonicalization refusal, broken PNGs and source-size failures are asynchronous status errors. Preserve M4's PNG conversions, premultiplication, stored-value color model and per-image decode limits (64 MiB encoded, 8192 per side, 64 MiB output, bounded row/library intermediates).

**Firm — pending appearance.** Pending and failed initial resources use a magenta checkerboard. **Proposal:** use a built-in magenta/black checker, charged once to the renderer baseline, in reserved native table slot 0. Geometry/order remains stable and status reports the reason. New handle-based quad creation requires explicit positive width/height, so dimensions arriving later never unexpectedly move neighboring content. Legacy image-size defaults remain (§8.4).

File/validation/budget failures are recoverable resource errors. Actual GPU allocation/encoding/submit failures retain the current presenter-failure path until a separately reviewed backend recovery policy exists. A failed GPU upload must not masquerade as a harmless missing file. No automatic unbounded retry or error per frame. Resource status stores the last error, and a bounded once-per-attempt observation can be logged under existing logger/control-path rules.

### 5.3 Explicit reload and source scope

**Firm.** Reload uses explicit `texture.reload(handle)`; automatic file watching is outside M5. **Proposal — semantics.** It schedules a new request revision and preserves the last good active content until a replacement is adopted. Reloading the same file still validates the path rule. Failed admission changes neither the active revision nor the currently admitted replacement. A valid reload that fails asynchronously records its error while the old revision stays visible. Quad geometry stays explicit even when the replacement dimensions change; sampling covers the new image with the existing destination rectangle.

The demo binds a reload action to `texture.reload`. No watcher, directory scan or settings/Lua reload is implied.

**Firm — no raw pixels from Lua in M5.** The Lua texture API accepts file sources only. Rust CPU sources remain supported. The current host converter accepts UTF-8 strings rather than arbitrary binary Lua strings; any future Lua pixel API needs a separately designed bounded binary transport, ownership and alpha convention. Do not encode pixels as millions of `ConfigValue` numbers. This limitation does not make paths part of the backend upload interface.

## 6. Upload ownership: choice A selected

**Firm — Q5, owner 2026-10-23 (§1.1): choice A, off-thread preparation with render-owned submission.** Moving upload preparation off the render owner changes the current safe boundary, which deliberately keeps every GPU object on one thread. The comparison below retains B as an unselected alternative and records its cost. Choice A refines the initial take's broad “upload worker” proposal by keeping all queue submission/presentation calls on the existing owner. It does not claim that the render owner makes no upload-related GPU call.

| Choice | Worker does | Render owner does | Cost/benefit |
| --- | --- | --- | --- |
| **A — selected** | Allocate unpublished storage, copy/stage pixels, record upload commands | Submit prepared uploads without waiting; observe completion; adopt/retire; draw | Removes bulk CPU work from drawing, but needs a narrow internal device-sharing and ownership-transfer proof |
| **B — unselected, simpler** | File I/O and decoding only | Budgeted allocation, copying, upload encoding/submission and drawing | Preserves single-thread native ownership; large allocations/copies can cause a late frame even under byte/work caps |

Neither promises a wall-clock maximum for a driver allocation, submit, acquire or present call. In B, “at most N upload bytes this pass” does not make a whole image allocation incremental. It must use an explicit per-pass admission policy and measure stalls; it must not be described as off-render-thread uploading. In A, serial queue submission may still place transfer work ahead of later rendering. GPU overlap/latency is measured, not inferred from a worker thread.

### 6.1 Choice A: shared device, exclusive job objects

**Proposal.** One backend-private device context is retained by the presenter and upload worker. Share only device access needed to create unpublished job-local objects. The public probe `Device` and presenter API do not gain unrestricted `Send + Sync`. The backend documents every cross-thread native operation and owns its unsafe boundary. There is one upload-preparation worker initially, not a pool of native creators.

| Object/work | Exclusive authority |
| --- | --- |
| Layer/surface/swapchain, render pipeline, draw state, descriptor pool/sets, all submit/present calls | Render owner |
| CPU decoder output | Immutable shared bytes, with a resource-service reservation |
| Preparing texture/staging storage and upload encoder | Upload worker, until it closes the job envelope |
| Closed upload envelope | Ownership transferred once to render owner; worker cannot mutate it afterwards |
| Submitted upload, frame bindings, active/retired native revisions | Render owner and its ledger |
| Resource publication/error status | Resource service, from bounded keyed backend observations |

**Metal proposal.** Use the presenter's device and a command buffer belonging to its queue, with exclusive encoding on the upload worker; only the render owner commits. The worker prepares a fresh texture and staging/blit commands, closes all encoders and hands over the envelope. Native retain/release is explicit across the handoff; completion handlers only publish a small token and wake, never free the scene or call Lua. No worker accesses `CAMetalLayer`, a drawable or an active sampled texture. Metal permits simultaneous encoding of separate command buffers and thread-safe command queues, but that does not permit simultaneous encoding of the same buffer. [Apple command organization](https://developer.apple.com/library/archive/documentation/Miscellaneous/Conceptual/MetalProgrammingGuide/Cmd-Submiss/Cmd-Submiss.html).

**Vulkan proposal.** Use the same device and graphics queue family as drawing. Each outstanding batch preparation lease owns a distinct command pool/buffer and its texture/staging objects plus exclusive allocator access while preparing; never share a command/descriptor pool concurrently. The worker records transitions and copies, including transfer-write to fragment-sampled-read synchronization and the final image layout. Noncoherent staging writes are flushed with the device's alignment rules before handoff. The render owner alone calls queue submit/present and owns submit fences. The worker performs no queue calls and no descriptor writes. Vulkan's external synchronization rules apply to pools and queues; different host threads need exclusive pool access. [Khronos threading guide](https://docs.vulkan.org/guide/latest/threading.html).

The closed pool/buffer lease transfers with the job. It may be reclaimed or returned to the worker only after the upload's fence and ownership-transfer acknowledgement; never reset it while its command buffer is pending. Keep two reusable upload batch leases initially: one preparing/closed and one submitted. Each can contain many images. Pool count is bounded by those leases, never one pool per loaded texture. Memory suballocation is owned by the preparation worker; the render owner returns completed retirement tokens to it through bounded state. Allocator mutations and native storage destruction cannot race preparation, drawing or pending transfers. The detailed backend spec must show its synchronization scopes; queue submission order alone is not a memory barrier. [Khronos synchronization specification](https://docs.vulkan.org/spec/latest/chapters/synchronization.html).

**Proposal — submission and adoption.** Maintain one drawing frame and at most one submitted upload batch in flight initially, accounted separately. The render loop makes a bounded non-waiting attempt to take a closed upload envelope. It submits when upload capacity is available; otherwise the envelope stays in its bounded slot and drawing continues with current revisions. Keep host-side polling nonblocking during normal operation. After the upload completion proof, publish a keyed ready observation to the resource service; after the service accepts that revision (§4.3), adopt it at the next frame boundary. No frame samples incomplete storage. No second queue, cross-family ownership transfer or semaphore dependency between separate queues is needed for this design.

Before native allocation, check the selected device's actual dimensions, sampled/transfer format support and requested filtering capability, as the current Vulkan path does. Decoder limits alone are not a device guarantee. The extra upload batch explicitly extends the current single-command-work cap: tests and shutdown ledgers must count both kinds of submitted work, not preserve an obsolete assumption that a drawing fence proves every upload complete.

An upload-ready observation remains stored in bounded per-resource/backend state until consumed; it is not dropped because a notification queue filled. Rejecting a superseded revision sends it into retirement. Resource status `ready` follows adoption acknowledgement; GPU completion alone says storage is usable, not that a frame displayed it. Backend completion/error handling uses the existing ledger/failure convention.

### 6.2 Shutdown proof for A

**Proposal.** Extend the presenter's cleanup proof to include its preparation worker. Use the existing absolute teardown deadline; do not restart it for each stage:

1. End frame scheduling and resource admission; detach scene publication. Mark all future loader results discard-only. Request preparation-worker stop.
2. Stop starting new frames/uploads. Retain every partially created or transferred native job in an explicit ledger/envelope. A worker must not drop unpublished native holders during an unproven unwind.
3. Observe the preparation worker actually finished, reclaim its closed/partial envelopes and allocator state, and establish that it can no longer access the device. Transfer exclusive allocator cleanup authority to the render owner; do not require a stopped worker to consume retirement tokens. An acknowledgement alone is not thread completion.
4. Prove all submitted drawing/upload commands and existing presentation tokens complete. Distinguish objects never submitted from objects whose submission outcome is uncertain; retain the latter.
5. Only after both worker and GPU proofs, destroy job pools/buffers, staging, retired/active textures and bindings in dependency order, then existing pipeline/queue/device objects. Release shared device access before device destruction. Return `Proven` only after the render thread itself is also observed ended under the existing outer contract.

If the worker is stuck in a native call, panics, disconnects or misses the deadline, retain the entire device-dependent object graph and return `Unproven`; do not destroy the device because the render thread itself happened to finish. Main-side window/surface/loader objects stay retained as today and the executable takes its existing incomplete-shutdown path. The new thread participates in the executable's resource-latch/panic policy, not only an inner `catch_unwind`.

CPU loaders own no native object and may finish separately, but their own runtime supervision is still bounded. Shutdown closes the backend handoff before stopping the GPU. A late CPU result releases only CPU reservations and cannot reopen admission or touch GPU/window state. Both VMs' `shutdown()` still run after window/GPU teardown; resource/scene calls then validate cheap arguments and report the ended service without starting new work. The legacy call retains its older discard behavior (§8.4).

### 6.3 What a fully independent upload queue would add

**Proposal — deferred.** A worker that submits directly to a second queue needs actual queue availability, shared-device lifetime proof, synchronization for readers and any queue-family transfer, separate completion/error accounting, and teardown covering both queues. A mutex around a shared queue can let a blocking present delay the upload worker or vice versa. Do not silently use that as a fallback. If “all upload submission off the render thread” is the owner's requirement, this is a third design to settle before implementation, not an undocumented variant of A.

## 7. Capacity, settings and resource service behavior

### 7.1 Proposed settings

**Proposal.** Extend the same explicit TOML configuration source used today. No implicit search, environment override or settings watching. Unknown keys and invalid values fail configuration load. Settings are copied read-only to VMs as today.

```toml
[render]
filter = "nearest"
assets = "assets"
max_quads = 131072
bindless_texture_slots = 131072   # native table entries, including checker slot 0

[resources]
max_textures = 65536             # logical resources, including retiring resources
max_extra_revisions = 1024       # simultaneous replacement/retired native revisions
max_pending_jobs = 65536         # cheap metadata, not decoded images
max_prepared_images = 256        # across preparing, closed and submitted batches
max_upload_images_per_batch = 128
max_upload_bytes_per_batch = 16777216  # 16 MiB; an oversized valid image runs alone
max_adoptions_per_frame = 256
decode_workers = 4
cpu_pixel_bytes = 536870912      # 512 MiB: reserved/queued/retained output pixels
staging_bytes = 134217728        # 128 MiB: all native staging allocations
texture_bytes = 2147483648      # 2 GiB: named native texture-memory charge, §7.2
metadata_bytes = 134217728      # 128 MiB: registry/jobs/paths/errors/snapshot storage
binding_geometry_bytes = 134217728 # 128 MiB: GPU tables and instance/upload control buffers

[frame]
updates_per_second = 60
max_delta_seconds = 0.1

[input.bindings]
move_left = ["KeyA", "ArrowLeft"]
move_right = ["KeyD", "ArrowRight"]
move_up = ["KeyW", "ArrowUp"]
move_down = ["KeyS", "ArrowDown"]
close = ["Escape"]
reload_texture = ["KeyR"]
```

These finite proposed capacities exceed the concrete 32,768-texture/65,536-quad target (§11.3); they do not promise that arbitrary full-size images fit. Require `bindless_texture_slots >= max_textures + max_extra_revisions + 1`, checked without overflow, and validate actual device support (§13). The default is 131,072 entries, not Synarchy's 16,384. Logical slots, native slots, revisions, image counts and bytes are distinct limits. The compatibility image consumes a logical slot; the immortal checker is separate. A slot freed by logical release is unavailable until retirement completes.

Settings define validated finite ranges, including caps on path/error strings (proposal: 1,024/2,048 UTF-8 bytes), input transitions, snapshot count and native leases. A configuration can fail device validation; never silently lower capacity while advertising the configured value. Array sizes/table arithmetic are checked independently of Lua token parsing. Descriptor implementation details other than the explicit capacity stay backend-private.

### 7.2 Memory, alignment and allocation overhead

**Proposal.** Admission is transactional: reserve all metadata/slot/job capacity needed for the initial request before publishing it. Later capacity failures report an asynchronous resource error, preserving the old revision. A full queue never blocks the caller. Reservations transfer with job/revision ownership and return exactly once on refusal, cancellation, supersession or proven destruction. No unbounded retries or automatic eviction of live references.

CPU pixel charges include decode destinations, queued revisions, CPU-generated sources and legacy retained images. Reserve output from checked dimensions before decoding. Encoded bytes, decoder rows and library intermediates have separate per-worker maxima using the accepted per-image limits; list their maximum envelope in diagnostics. Pending metadata does not allocate image buffers. Retained CPU bytes are normally dropped after upload stops needing them, except explicitly CPU-owned/legacy state. No device-loss reconstruction is promised from discarded pixels.

Native charges include uploading, active and retired versions, even if no current quad samples them. Reload requires old and new storage together. Staging counts alignment and every preparing/submitted lease. Texture count cannot be multiplied by just RGBA payload size: 32,768 tiny images may have substantial alignment, allocation and driver-object overhead. Measure logical pixel bytes, queried requirements, allocated block bytes, unused/fragmented block bytes, object counts and observed process/GPU memory separately. Swapchain/drawable/pipeline objects are a named baseline. Do not subtract CPU and GPU charges on the assumption that unified memory aliases them.

**Vulkan proposal.** Use pooled device-memory blocks for images and staging, not one `VkDeviceMemory` allocation per image. Query requirements, memory-type bits, alignment, buffer-image granularity and required dedicated allocations; charge complete pool blocks including unused space. Segregate incompatible allocation classes and preserve each block until its last suballocation is proven unused. Bound allocation count against the actual `maxMemoryAllocationCount`, including baseline objects and dedicated allocations. Reserve the next whole block before allocation; refuse if it cannot fit. A required dedicated allocation consumes its queried size and a count reservation. Pools reuse only retired suballocations and never alias live resources. This is necessary for thousands of images irrespective of descriptors. [Vulkan memory allocation specification](https://docs.vulkan.org/spec/latest/chapters/memory.html).

**Metal proposal.** Start with ordinary private texture allocations and residency sets (§13), measuring each resource's `allocatedSize`; do not add heaps merely to solve residency. The implementation must distinguish a preallocation estimate from a guarantee: `heapTextureSizeAndAlign` sizes a heap allocation, not a proof of every standalone allocation's total driver cost. Serialize unmeasured texture creation on the single preparation worker. Reserve its estimate first; reconcile the observed resource size before any publication or next texture creation. An over-budget unpublished texture is destroyed and its request fails; report its transient peak. Thus `texture_bytes` bounds admitted/retired reported resource allocations, **not a strict transient ceiling**: there can additionally be one not-yet-measured allocation. Driver-internal objects/process memory remain outside that API count. Do not describe a post-allocation check as a hard preallocation guarantee.

If measured standalone overhead, allocation time or this temporary allowance is unacceptable, the allocator issue must switch to bounded Metal heap blocks before scale acceptance: query size/alignment, reserve/charge whole heap blocks, retain suballocations through completion and add those heaps to the same residency set. That preserves the binding/lifetime design. No placement aliasing or compaction is needed in M5. A hard byte ceiling on application-controlled native storage requires that block strategy; it is not claimed by the standalone baseline. The stress evidence must state which strategy was accepted and its measured footprint. Heap use is an allocation decision, separate from the selected residency mechanism.

Count metadata/table/instance buffers explicitly. The native table has two versions (§13), GPU geometry has bounded reuse, and CPU scene copies remain capped at three. Metal residency-set bookkeeping (`allocatedSize`) is recorded separately from texture allocation charges; inspect its API meaning to avoid double-counting referenced memory. Vulkan descriptor-pool footprint is partly driver-private: report configured descriptor counts, host allocation observations and process memory, not an invented bytes-per-descriptor constant. Actual allocation failure below a budget follows §5.2's presenter policy.

### 7.3 Throughput without waiting for the loader

**Proposal.** A large bounded metadata queue feeds four decoding workers, gated by byte reservations and a bounded prepared-image window. Keep only small metadata for the other pending loads. Workers sleep on their service's capacity notifications; UI/main/render owners never wait for that capacity. Requests that cannot acquire even metadata/job admission fail promptly. A worker denied a reservation because live resources exhaust the budget produces a capacity error; transient staging occupancy can wait on bounded service state without monopolizing decoded buffers indefinitely.

Prepare multiple small images in each upload batch, up to both count and byte targets. An image larger than the normal 16 MiB batch target gets one batch alone, still subject to the accepted 64 MiB decoded-image limit, staging budget and GPU constraints. One batch prepares while at most one is submitted. Do not submit 32,768 individual command buffers or allocate 32,768 command pools. The render owner submits ready batches and handles at most the configured adoptions per pass, checking control work between phases. Completed results live in bounded keyed state until acknowledged, so a full notification channel cannot lose readiness or force a wait.

Table updates and residency membership changes are batched and bounded too. A loaded image is not advertised `ready` until its table entry and residency are usable by a future frame. Submission order/driver work can still affect frame latency; off-thread preparation does not prove free transfers. Record files/s, decoded MiB/s, uploads/s, queue depth, time to all-ready, per-frame render-owner upload/publication time and input/close responsiveness while loading. No per-texture per-frame logging or scan of the entire loaded registry. Drawing only traverses the current scene and changed resources; registry lookup is direct by validated key.

The Lua demo admits resources incrementally (e.g. 128 creations per callback), continuing to display pending checker quads and react to input. A startup loop performing tens of thousands of host calls would still monopolize the UI VM despite asynchronous I/O. Run a separate all-quads-moving profile to expose host-call and snapshot-copy cost. Efficient binding alone cannot remove Lua or O(visible-quads) instance construction cost.

**Firm — owner 2026-10-23, relayed in the design-session log:** “Measure it, don't gate it (Recommended)”. The §11.3 performance targets gate the steady (static) stress scene. Measure and report the all-quads-moving Lua profile without a performance gate. If it is slow, decide a separate focused bulk-geometry API follow-up issue from the measured results; it is not a prerequisite for milestone completion.

## 8. Scene and Lua behavior

### 8.1 Ordered persistent quads

**Proposal.** Each quad has an opaque quad handle, retained logical texture reference, finite position/positive size in window points, and stable creation order. Default UVs cover the image. Sampling remains the configured nearest/linear policy. Do not add a map transform or arbitrary material system here. Keep the accepted top-left origin, edge rounding, clipping, scale changes, premultiplied blending and UNORM color rules.

Backend-neutral scene entries describe geometry and resource identity. They contain no “bind texture,” descriptor set, argument-buffer offset or draw-call record. GPU backends resolve logical handles and active revisions on the CPU while building each immutable native frame packet. The packet pins its native table version and embeds private slot indices in per-instance data (§13); those indices never return to `ck-render-2d`. Ordinary drawing batches adjacent compatible textures; final bindless drawing instances across texture changes. Keep painter order in both paths. A textured alpha scene cannot generally be sorted by texture.

**Proposal.** The UI owns one editable scene and publishes immutable snapshots after startup and each completed posted callback that changes it. Use a general owner-thread completion hook, or an equivalent application-owned boundary that runs after callback side effects and before the result wake. Never copy the whole scene after every `set_position`. A callback with a caught per-call error can still publish other valid changes. An uncaught callback error has no rollback: report it and follow the existing shutdown policy; the final publication policy must be consistent on success and failure and tested.

Use bounded reusable snapshot storage: one latest publication, one consumer/frame snapshot and one writer/build snapshot initially. Old snapshots retain resource references until discarded; moving or replacing them releases references outside locks. Build/copy/convert O(max_quads) data outside the publication lock, leaving only pointer/version exchange inside. Resource adoption and metrics changes can request a redraw without running Lua. The renderer resolves all revisions for one frame coherently and pins them before encoding.

The existing attach/scale/end rules remain: initial publication precedes first draw, a newer metrics generation cannot be overwritten by stale conversion, and no publication reaches a detached backend. A failed per-call validation changes nothing. A resource loaded during startup does not delay window opening until pixels arrive; the first frame may contain the pending appearance. This differs from the legacy synchronous image call only for the new API.

### 8.2 Proposed API

```lua
local x, y = 32, 32
local speed = 160 -- window points per second
local image, quad

function startup()
    image = texture.load("unit.png")
    quad = render.create_quad{
        texture = image, x = x, y = y, width = 64, height = 64
    }
    frame.on_update(function(f)
        local dx = (f.actions.move_right.held and 1 or 0)
                 - (f.actions.move_left.held and 1 or 0)
        local dy = (f.actions.move_down.held and 1 or 0)
                 - (f.actions.move_up.held and 1 or 0)
        x, y = x + dx * speed * f.dt, y + dy * speed * f.dt
        render.set_position(quad, x, y)
        if f.actions.reload_texture.pressed then
            texture.reload(image)
        end
    end)
    window.open{title="Live scene", width=800, height=600, clear={0.1, 0.1, 0.1}}
end

-- While active, when removing an object explicitly:
-- render.destroy_quad(quad)
-- texture.release(image)
-- texture.status(image) returns state/revisions/dimensions/error without waiting.
```

**Proposal.** `frame.on_update(nil)` disables updates. `render.set_rect` can change position and size atomically; moving does not change texture content. `texture.status` is a snapshot, not a blocking wait and not a promise a pixel was displayed. Unknown fields, nonfinite geometry, wrong/stale tokens and immediate capacity refusal are catchable Lua errors. All host calls validate before mutation. Release/destroy calls are not silently idempotent on stale tokens; misuse should be diagnosable.

Polling status from the frame callback is sufficient for the demonstration. A future resource-change callback must use bounded/coalesced state, not add one result for every repeated failed poll. No synchronous `texture.wait` is introduced: waiting inside startup would prevent the main-thread run phase that starts the renderer.

### 8.3 Headless and ended services

**Proposal.** Headless preserves finite startup/shutdown with no window, GPU objects or frame callbacks. New texture calls validate/admit CPU source work; status can reach `cpu_ready`, never GPU `ready`. The ordinary headless end cancels remaining async work under the existing owner deadlines; it does not keep the process running just to finish optional textures. New asynchronous file errors are observable only if completed before that end. Document that limitation rather than imply the new API retains legacy synchronous validation semantics.

Pure resource tests can explicitly drive/poll the CPU service without introducing a production “headless game loop.” Scene edits in headless mode remain validated and ignored/countable as presentation, with the existing once-per-run headless notice adapted explicitly to the new API. After service end, new requests fail immediately as ended; they do not revive workers or perform I/O. Teardown itself releases remaining owners regardless of what Lua calls from `shutdown()`.

### 8.4 Legacy compatibility

**Proposal.** Keep `render.quad{image=..., x=..., y=..., width=..., height=...}` and `render.clear_quad()` as a separate compatibility lane. The old call still decodes synchronously, raises file/decode errors at the call, defaults size from the image and replaces exactly one compatibility quad. `clear_quad` removes only that quad, not new handle-based quads. Its painter-order position is fixed below new quads; replacing it does not reorder other content. It owns an internal resource reference, with no public texture token.

On the upgraded backend, admit the decoded immutable image through the common residency/retirement path, honoring the old guarantee that a failed call leaves the previous compatibility quad unchanged. Reserve capacity needed to admit its CPU image/scene change before replacement; retire old native content safely. Keep the accepted legacy first-frame image ordering by gating its initial image-bearing frame until that image's upload completes, without blocking UI/main threads. Its absence/failed presenter still follows the existing lifecycle. New asynchronous resources do not impose that gate.

Legacy decoding still has the documented possibility of stalling UI and reaching an owner deadline; the new API is the route to asynchronous loading. Headless legacy calls still validate/decode synchronously and ignore their result. Legacy writes after detach retain M4's validate/decode/discard/count semantics. Do not relabel the new API's ended-service errors as legacy behavior.

The compatibility decoder must acquire its pixel reservation before allocating image-sized output, not after decoding. Retain the previous compatibility image while validating/preparing the replacement, and release the reservation on every failed call. Existing per-image-valid calls can additionally fail a configured aggregate capacity limit when other new resources consume it; that is an explicit new resource-budget error, not an accidental disappearance of the previous quad.

During the M5-to-M5.5 interval, the accepted Vulkan legacy demo continues to work through its existing path. New handle/scene/frame features on `glfw-vulkan` fail explicitly as unsupported until the matching adapter is delivered; do not silently render only one of several quads. The configuration can still parse new shared settings, but explicit requests for unsupported input/frame behavior must be refused during mode validation, even if no new Lua API is called. In particular, custom close bindings must not silently leave hardcoded Escape active. Backend capability gating and first-use errors must be tested so merely upgrading the binary does not break old configurations.

## 9. Frame scheduling and timing

**Proposal.** A registered UI update callback enables a paced update source while the window is active; absence of the callback preserves on-demand redraw. The application scheduler uses a monotonic clock and timed event-loop waits, with a configurable target update rate (proposed default 60 Hz, matching the owner-accepted stress target). This is a visual/UI update rate, not simulation time and not proof of monitor refresh. Do not add a timer that appends an unbounded callback each tick.

Add a deadline wait to the window backend contract. Native and GLFW implementations use their supported timed event wait/wake mechanisms; the native callback only records. A due deadline or queued wake avoids sleeping. Always check quit/control state after the native pump and between bounded work phases. Normal render steps never sleep waiting for Lua, a decode or an upload; the scheduler's idle/pacing wait is distinct from waiting inside drawing. Existing native acquisition/presentation and modal-loop limitations remain stated.

Shared trait changes must leave both backend builds valid at each accepted step. Introduce timed-wait capability explicitly: the legacy GLFW adapter continues to serve its original event loop until M5.5 enables that capability, and the application refuses the new scheduler on it before making an unsupported timed call. Do not emulate an unsupported deadline wait with repeated `Poll` calls or silently turn it into an indefinite wait.

**Proposal.** At most one frame callback is admitted/running at a time. A later opportunity replaces the pending opportunity timestamp; no replay of missed callbacks. Compute `elapsed` from monotonic delivered-update boundaries, not each discarded opportunity; expose bounded visual `dt = min(elapsed, max_delta_seconds)` plus raw `elapsed`, sequence and window metrics. The first update has zero delta.

**Firm — owner 2026-10-23 (§1.1).** Updates pause while unfocused or minimized; held input is cleared on focus loss. **Proposal — scheduling details.** Zero-size windows also suspend updates; suspension clears held input, and resumption establishes a fresh time origin and zero first delta. Background animation is a later owner option.

On each admitted update, capture a coherent input snapshot and its acknowledgement boundary. Clear consumed transition flags only once that callback is successfully admitted; a `Full` refusal keeps them pending. Physical events arriving later belong to the next update. The render owner independently redraws the latest published scene for exposure, changed metrics, new content or completed updates; it never waits for the current callback. Drawing unchanged content for a delayed UI update is acceptable, and must not accumulate more frames than its fixed cap.

Keep close/lifecycle admission separate from frame work: check/post pending close first and reserve posted-call capacity for it, so continuous frame opportunities cannot occupy every call slot. An already executing Lua callback is not preempted merely by priority; signals/cancellation retain the existing escape route. A callback error logs its traceback and starts the existing clean-shutdown path, with exit 1 unless an incomplete result overrides it. Do not retry a callback with side effects.

**Proposal.** An input-driven movement test uses fake time to show equal displacement for equal delivered delta at different update rates. Long stalls exercise the documented visual clamp. The gameplay VM receives no frame delta and remains independent: the vision's hourly logical game tick and reproducibility need a separate simulation design.

## 10. Input actions and test isolation

### 10.1 Normalization and bindings

**Proposal.** Cocoa/GLFW normalize physical key presses/releases and focus changes into portable data. `ck-input` maps keys to configured action names; UI receives `held`, `pressed`, `released` and bounded ordered transitions in each update. Physical key names mean keyboard positions, not produced characters. The backend spec includes the exact mapping and supported modifiers. Text/IME is a separate future channel.

Load bindings at startup from the settings file. Missing action bindings use documented defaults; an explicit empty list unbinds that action. Unknown key names, malformed action names, duplicate entries and invalid combinations fail validation. Permit one physical key to bind multiple explicitly named actions, documenting that all fire; do not invent context priority in M5. Application action names are data; the close action is reserved by the application for its existing protocol.

**Proposal.** Held action state is the OR of its held bindings. Releasing one of two keys does not release the action while the other remains down. Key repeats do not generate additional pressed edges or accelerate movement. Opposed movement actions produce zero on that axis in the demo; diagonal speed policy belongs to the script. Focus loss clears key/action held state and supplies release/reset information. Modifier changes recompute held combinations so no action gets stuck.

Close's rising edge requests the existing Lua close callback with source `key`; the native close button uses source `button`. Repeated close events coalesce under the current one-outstanding rule. No callback means accept. Default Escape is now a binding, not a second hardcoded exit path. Cmd-Q/no-menu behavior and signal quit remain as accepted.

### 10.2 Bounded transition policy

**Proposal.** Keep a fixed key/action state set, one latched pressed/released bit per action since the last admitted update, and a finite ordered transition buffer (proposed 256). A press and release between updates sets both bits even when `held` ends false, so short taps survive. Under overflow, preserve final held state and the latched per-action bits, set `overflow=true`, and discard oldest detailed transitions. Exact order/count for the overflowed portion is explicitly lost. No overflow may leave a released key logically held, and close processing does not depend on the lossy detailed buffer.

Native event backlogs must be bounded/coalesced before application delivery as well, not merely after an unbounded native-to-Rust queue. GLFW's own pump may drain an OS backlog before returning; preserve the existing qualification on responsiveness. No per-key logging or console output. A diagnostic overflow counter is a bounded aggregate, not a log storm.

### 10.3 Tests while the owner types

**Firm — inherited test rule.** Real keyboard input to a test window must be dropped quietly.

**Proposal.** Extend test-executable-only filtering to all new paths and all windowed fixture processes, on both backends. Filter real key-down, key-up and modifier events before normalization/action mapping, key equivalents or text dispatch. No action, beep, close request, per-key log or input-count assertion may depend on the owner's typing. Merely declining Escape in a Lua fixture is no longer enough: letters would otherwise move the scene or trigger reload.

Pure mapper tests feed explicit synthetic records. Native bridge tests may allow only deliberately marked test events through the existing test-only mechanism. Keep keyboard filters/markers outside production support and enforce the existing symbol/source guards. The owner's interactive demo receives normal input. Automated windows close themselves and remain deterministic under concurrent unmarked typing. This design session performs none of those window runs.

## 11. Acceptance and independently reviewable delivery

**Proposal.** File immutable issues only after verification and independent design review by both brands; each still gets independent readiness review. These are dependency labels, not filed issue IDs. Every issue names its check-catalog ownership, meaningful spec coverage and required planned-check groups. Workers report selected groups/reasons; batch validation supplies full required GPU/window coverage. No issue is filed by this draft.

### 11.1 Milestone 5, native/Metal

| Order | Concern | Depends on | Acceptance focus |
| --- | --- | --- | --- |
| M5-1 | Pixel/assets extraction; generational registry, tokens and ownership | Design verification | Stale/foreign handles, exhausted generations, retirement, unchanged PNG/color behavior |
| M5-2 | Bounded loader, file/Rust CPU sources, revisions, settings and accounting | M5-1 | Queue/byte admission, superseded results, last-good reload, finite headless end; fake upload port |
| M5-3 | Ordered multi-quad scene, atomic publication and `ck-lua` post-callback publication hook | M5-1 | Hook tests for startup/callback publication and result-wake ordering; reference pins, painter order, coalescing, metrics/attach/end races; fake renderer |
| M5-4 | Metal upload preparation, allocation accounting and native retirement | M5-2; selected upload A | Batched transfer, footprint measurement, lease/budget caps, adoption and shutdown proofs |
| M5-5 | Metal ordinary instanced/batched drawing and legacy conversion | M5-3, M5-4 | Order/blend/color, adjacent batches, no movement upload, first-frame compatibility; ordinary baseline counters |
| M5-6 | Actions/settings, Cocoa key/focus delivery and test keyboard isolation | Reviewed input contract in §10 | Pure mapper edges/overflow/focus; native marked events and close behavior |
| M5-7 | Timed waits, bounded UI scheduling and intermediate `glfw-vulkan` scheduler refusal | M5-6 | Test refusal before unsupported GLFW timed-wait calls; fake-clock coalescing/delta, close priority, idle/resume, no scheduler GPU dependency |
| M5-8 | Lua texture/quad/frame API, application lifecycle/demo and intermediate `glfw-vulkan` API/settings refusal | M5-2, M5-5, M5-7 | Test explicit refusal of new unsupported calls/settings, including custom close bindings, and preserved legacy configuration behavior; shared integration, magenta pending/failure, explicit reload, incremental creation |
| M5-9 | **Final bindless stage:** Metal capability gate, versioned resource-ID tables and residency sets | M5-4, M5-8 | Supported SDK/device gate, immutable table epochs, add/remove retirement, exhaustion transactionality, bounded updates |
| M5-10 | **Final bindless stage:** texture-indexed instancing and scale acceptance | M5-9 | Tens of thousands of distinct loaded textures and quads; correct sampling/order, loading/churn/no-idle evidence and measured costs |

M5 is complete only after the final bindless stage, not at the moving-quad demo. M5-9/10 split binding lifetime from drawing/scale evidence; both belong to M5. If the selected choice A requires a substantial shared-device refactor, split it before M5-4. If allocator evidence requires Metal heaps, split that focused prerequisite before final acceptance; do not hide it in the scale test. The measured all-quads-moving profile may lead to a separate bulk-geometry API follow-up, not a milestone prerequisite (§7.3, §11.3). Backend lifecycle adapters land with their backend issues, leaving M5-8 as integration rather than a catch-all. Automatic watching and Lua raw pixels are excluded, not optional M5 issues awaiting an answer.

**Proposal — for the issue specs (design review 1 follow-through).** These notes make existing requirements explicit; they do not replace each issue’s readiness review.

| Named issue(s) | Detail to settle and test in the issue spec |
| --- | --- |
| M5-1, M5-6 | Create `ck-resources` in M5-1 and `ck-input` in M5-6 as they gain real code, not in empty prerequisite packages (Claude finding 8). |
| M5-2/4/9; counterparts M5.5-1/5 | Share an executable transition model naming ownership/reservation transfers. Distinguish reload before versus after compare-and-commit, rejection after spare-table preparation, an unsubmitted pinned packet, retirement with no subsequent Lua edit, and bounded dirty-slot progress while drawing continues (Codex note 2). |
| M5-4 | Record the reason for private textures plus blit and evaluate the simpler shared-texture/worker-write alternative without silently changing the selected design. Measure upload throughput against the accepted target. Report the observed allocation size of a 32×32 private texture early, so unacceptable overhead triggers the heap prerequisite before M5-10 (Claude findings 5, 7). |
| M5-4; counterpart M5.5-1 | Preserve the qualified memory promise in the allocator spec and diagnostics: Metal excludes one serialized unmeasured allocation and driver-private overhead; Vulkan charges whole blocks, unused space, allocation counts and required dedicated allocations. Staging and old revisions stay charged until their actual owners finish; this is not a strict process-memory ceiling (Codex note 3). |
| M5-9/10; counterparts M5.5-5/6 | Evaluate using the table-epoch pin alone to retain bindless sampled resources, since the table already retains all its entries; avoid redundant per-revision reference-count traffic if the spec proves equivalent lifetime coverage. Keep per-revision pins for the ordinary-binding stage (Claude finding 4). |
| M5-3/7/8 | Choose and test the exact final-publication rule after an uncaught callback error, including completion-hook/result-wake ordering; do not leave it to an unrecorded implementation choice (Codex note 4). |
| M5-10; counterpart M5.5-6 | Freeze warm-up, sample duration, percentile calculation, loaded/visible/submitted counts and the timed CPU interval. Report end-to-end presentation intervals and missed updates alongside CPU/GPU percentiles; separate the static acceptance gate from the reported all-quads-moving profile (Codex note 4). |

### 11.2 Milestone 5.5, GLFW/Vulkan parity

**Proposal.** After accepted M5 and reconciliation of its evidence:

| Order | Concern | Acceptance focus |
| --- | --- | --- |
| M5.5-1 | Vulkan pooled allocation, upload and retirement port | Ordinary-descriptor strategy/pool bounds resolved with M5.5-2 before filing (§11.2 below); queried allocation/count bounds, batch pool leases, synchronization and completion; off-screen pixels |
| M5.5-2 | Vulkan ordinary instanced/batched drawing and neutral conversion | Same 32,768-texture baseline under the specified descriptor bounds; same geometry/color/order, stable identity, no movement re-upload, legacy behavior and baseline counters |
| M5.5-3 | GLFW input normalization and timed waits | Same actions, close, focus and keyboard-drop test boundary |
| M5.5-4 | Application activation and common Lua demo | Same script/settings, surface generation/pixels, lifecycle parity; ordinary path is still an intermediate stage |
| M5.5-5 | **Final bindless stage:** actual-device feature/limit gate and versioned descriptor tables | Separate image/sampler bindings, explicit features/layout support, two table versions, transactionality/retirement without device idle |
| M5.5-6 | **Final bindless stage:** indexed instancing and identical scale acceptance | Distinct-image stress and reload/release churn, allocation-count limits, measured throughput/frame costs, parity with Metal |

**Open for the M5.5-1/2 issue specs — ordinary-binding descriptor strategy (Claude finding 3).** Resolve this choice before either spec is filed. Options are (a) one descriptor set per adopted revision, allocated at adoption and retired with that revision; (b) push descriptors, gated on actual device support; or (c) a bounded per-frame descriptor ring, rewritten only after all readers of the reused entries complete. The specs must select one strategy and state finite pool count, `maxSets` and descriptor counts where pools are used, or the equivalent push/ring limits, together with allocation/refusal/reuse rules and synchronization. Those bounds must support the same 32,768 distinct loaded textures in the ordinary baseline, including placeholder and overlapping replacement/retired revisions. Test bounded exhaustion and safe retirement as well as the baseline; no unbounded pool growth or silently reduced content. The final bindless strategy in §13.3 is unchanged.

GPU-specific code stays in `ck-gpu-vulkan`; application loader/instance/surface responsibilities stay as accepted. Keep the UNORM-only textured-color contract: other surface formats skip textured content with the existing once-per-run warning and keep clearing. `ready` does not mean visibly sampled on such a surface. The bindless capability failure policy is explicit (§13.3), and final acceptance requires a capable textured surface. Then conduct the **guide review of the completed 5/5.5 arc**; it is advisory and separate from implementation review/acceptance gates. Bindless does not wait for that guide review.

### 11.3 Required evidence and concrete scale recipe

**Proposal — CPU contracts.** Cover stale/foreign/released tokens and exhausted generations/revisions; public versus quad ownership; late completion after release/reuse; all-or-nothing slot/publication failure; old-revision preservation; bounded jobs/snapshots/bytes and exact reservation return; table-version state transitions using fake completions; deterministic painter order; geometry/color conversions; new load calls doing no I/O; action taps/multiple bindings/repeat/focus/overflow; close priority; fake timing; finite headless end and legacy errors. Test exhaustion at tiny injected capacities, plus checked production-size capacity arithmetic, without huge test allocations.

**Proposal — authorized GPU/window evidence.** Validate indexed image selection, overlap/alpha and both filters, all resources initialized, movement without re-upload, reload while a prior frame retains the old image, release/reuse while old snapshots/table versions exist, exhausted descriptor/native/byte capacities preserving old content, bounded cleanup and allocation counts. Validate a table switch against expected pixels, not merely a screenshot that “looks busy.” Keep actual native/GLFW keyboard suppression and normal GPU validation in affected groups. Fault/unproven paths use separately reviewed test seams in their later authorized workflow; no such experiments occur in this design session.

**Firm — scale intent:** tens of thousands of both quads and loaded textures. **Proposal — reproducible acceptance workload:**

- Load **32,768 distinct 32×32 RGBA PNGs** with deterministic identity-coded colors/patterns; verify 32,768 unique active native image identities, not duplicate handles to a tiny cache or unresolved checker entries. The set has 128 MiB of logical decoded pixels; native overhead is measured separately.
- Draw **65,536 quads**, two per texture, with painter-order overlap cases and both shared/distinct texture runs. A tiled/scrolling view and deterministic pixel readbacks make texture selection observable. Record viewport and actual visible/drawn counts; do not claim a mostly culled scene proves drawing 65,536 instances. A compact full-scene stress view submits every instance, alongside readable zoomed verification views.
- Incrementally load while moving a foreground quad and exercising input/close. Continue rendering checker entries while pending. Then measure steady rendering, all-quad movement, and bounded explicit reload/release/recreate churn with old frames in flight. Exercise full logical/native caps separately via smaller test seams; count rejected requests and peak reservations.
- Run an additional heterogeneous image-size fixture including larger images. Tiny textures expose object/alignment costs but cannot stand in for all texture memory/transfer workloads. No atlas packing in M5; reserved future views must not require one image per quad.
- Capture the ordinary-binding baseline before each backend's final stage, and rerun the same content after bindless. For the homogeneous pipeline/clip/filter stress pass, texture diversity alone must not cause additional draw calls: indexed instances are submitted in one draw, or bounded chunks justified by actual buffer/device limits. Preserve painter order. Record CPU snapshot/resolve/encode time, GPU time, frame intervals, draw/bind calls, allocations, residency operations, memory charges, loading throughput and warm/cold timing with hardware/OS/driver/configuration.

**Firm — owner-accepted provisional performance targets, 2026-10-23 (§1.1), accepted “for now”:** 1920×1080 drawable, 60 Hz update target, steady stress CPU and GPU frame p95 each under 8 ms, at least 1,024 warm tiny-texture loads per second, no application waits for texture completion and no device/queue-idle wait. These targets may be revised by the owner after measurement.

**Firm — owner 2026-10-23, relayed in the design-session log:** “Measure it, don't gate it (Recommended)”. The performance targets gate the steady (static) stress scene. Measure and report the all-quads-moving Lua profile without applying that performance gate. A slow result informs a separate focused bulk-geometry API follow-up issue, decided on the numbers; it does not make that API a prerequisite for M5 completion.

**Proposal — measurement details.** Run on the owner's current M3 Max. Time CPU frame preparation/encoding and GPU execution separately, with the exact intervals and sampling recipe frozen in M5-10/M5.5-6. Record end-to-end frame intervals and missed updates as well; independent CPU/GPU percentiles alone are not proof of smooth presentation. Report the all-quads-moving Lua profile and cold-file run separately. No measured results are claimed here. The detailed recipe remains subject to design verification and cannot replace tens of thousands of loaded textures with many quads sharing a few images.

### 11.4 Lessons from the owner's Synarchy reference

**Read-only reference, not a port.** Files below were inspected under `~/work/synarchy` on 2026-10-23/24:

| Reference | Verified lesson for this design |
| --- | --- |
| `src/Engine/Graphics/Vulkan/Texture/Limits.hs`, `Bindless.hs` | 16,384 texture slots including checker slot 0, and a separate 65,536-entry handle-to-slot buffer; Codekarl needs larger independently bounded capacities |
| `Texture/Release.hs` (#1281), under that Vulkan directory | Current code explicitly invalidates aliases before slot reuse; its documented past bug shows why every alias/reference and old GPU reader matters. This is not a claim that the repaired bug remains present |
| `Texture/Publish.hs` (#1690) | Current code prevents success publication after slot-registration refusal. Codekarl likewise publishes a revision only after allocation, slot/table and residency admission succeeds, preserving the old revision |
| `src/Engine/Scripting/Lua/Message/WorldTexture.hs` | `disposeTransientTexture` uses `deviceWaitIdle`; Codekarl must replace that synchronization strategy with completion-pinned immutable table versions, not copy it |
| `docs/texture_infrastructure.md`, Problem | Records 3,925 animation PNGs consuming individual image/slot allocations and about 70.8 MiB decoded before allocation overhead. This motivates measuring object/alignment costs and leaving room for future atlas views, not adding atlases now |

Codekarl resolves handles on the CPU (§13.1), so it does not import Synarchy's shader handle-to-slot map or monotonically spent handle namespace. Reusable generational logical keys remain separate from physical slots.

## 12. Decisions and design defaults

| Key | State | Decision or recommendation |
| --- | --- | --- |
| Q1 | **Firm, owner 2026-10-23** | Magenta checkerboard for pending and failed initial loads |
| Q2 | **Firm, owner 2026-10-23** | Explicit `texture.reload`; no automatic watching in M5 |
| Q3 | **Firm, owner 2026-10-23** | No Lua raw-pixel creation in M5; Rust CPU sources remain |
| Q4 | **Firm scale and provisional targets, owner 2026-10-23** | Tens of thousands of quads and loaded textures; §11.3 targets accepted for now, revisable by the owner after measurement. §7 capacities and the detailed measurement recipe remain Proposal for verification |
| Q5 | **Firm, owner 2026-10-23** | Choice A: worker prepares, render owner submits (§6); B is unselected |
| Unfocused/minimized | **Firm, owner 2026-10-23** | Pause updates; clear held input on focus loss (§9) |
| Bindless timing | **Firm, owner 2026-10-23** | Last stage of each of M5 and M5.5, after its lifecycle/ordinary-binding stage |
| All quads moving | **Firm, owner 2026-10-23** | "Measure it, don't gate it": the §11.3 targets gate the steady stress scene; the all-quads-moving Lua profile is measured and reported, and a bulk-geometry API is a separate follow-up decided on the numbers (§7.3, §11.3) |
| Metal residency | **Proposal, manager and co-designer agree** | Queue-attached residency sets plus resource-ID argument buffers, supported by the current deployment target; no reason from this concern to drop Metal |

**Proposal — defaults for verification.** UI-only presentation ownership; explicit quad size; optional callbacks with a 60 Hz default matching the accepted stress target and a 100 ms visual-delta clamp; zero-size suspension and resume timing; load-at-start settings; strict stale-token errors; legacy painter-order placement; byte/count defaults and memory-accounting scope. These are recommendations, not additional Firm owner decisions. §7.2 explicitly qualifies the standalone Metal allocation transient; choose bounded heaps before acceptance if that allowance is unacceptable. Map choices remain Open (§14).

## 13. Final bindless stage in each milestone

**Firm — sequencing.** First prove logical lifetimes, bounded loading, scene publication and ordinary instanced/batched drawing; finish M5 and M5.5 with their bindless stages. The implementation changes backend binding selection without changing the public texture/quad API or neutral scene meaning. The representative workload and baseline precede each conversion, not a later milestone.

### 13.1 CPU resolution, immutable tables and instancing

**Proposal — choose CPU resolution.** The backend builds a native frame packet from the immutable logical scene. In O(drawn quads) it validates retained keys, resolves each to the active content revision and private native slot, and writes that slot into its instance buffer. Pending/failed initial images resolve to checker slot 0. Use O(1) registry lookup and reuse resolved entries for repeated textures within a packet. No scan of all loaded textures per frame. The packet pins those revisions and the table version that defines those slot meanings.

This avoids a second shader-visible handle-to-slot lookup and its mutable alias/generation protocol. CPU resolution is already needed for coherent revision/lifetime selection and geometry; measure its cost at the proposed scale. A future GPU-generated scene could justify a shader mapping, but it is not present in M5. A stable public handle can acquire a new private slot after reload without changing Lua or scene entries. Native slots never equal public handle slots by contract. Future atlas views can resolve to native image plus UV subrectangle without changing this identity separation.

Use **two native binding-table versions** with fixed configured capacity, plus bounded instance-buffer storage. At most one drawing frame is pending; the other table is available for preparation. A submitted or unsubmitted pinned table is immutable until all its readers finish/are discarded. Keep current frame packets attached to their exact table epoch. A table can be reused unchanged across frames; do not rebuild 131,072 entries every frame. Initialize both tables once to the checker. Apply changed slots only when a version is reusable, with a bounded dirty-slot bitset/latest-state map rather than an unbounded change log. If updates take several passes, keep drawing with the last complete table/state pair until the spare is complete.

A table version retains every resource represented by its entries, conservatively including currently undrawn textures. Publishing a replacement requires: native upload complete; revision still authorized by the resource service; capacity/slot and residency prepared; spare table caught up. Commit the logical-active-revision mapping and usable table epoch as one coherent render-state publication, then acknowledge `ready` to the service. The service's current-request check remains authoritative (§4.3); the backend cannot resurrect a superseded result. The adapter performs the compare-and-commit of §4.3 with no GPU calls in its critical section; a reload admitted after that commit is a later replacement, while one admitted before it invalidates the candidate. Keep this bounded two-phase adoption protocol explicit in the issue spec; never publish `ready` before the table can sample it. Existing packets may finish with the old mapping/epoch.

Retirement replaces obsolete entries with the checker in **both** table versions as each becomes reusable, drops their table pins, proves every old frame/upload complete, then frees storage and native slot. Process cleanup even with no new scene changes, otherwise stale reusable tables would pin retired images forever. No table still capable of submission can retain a dangling image/ID. This deliberately conservative rule avoids relying on partial-binding semantics or guessing which entries a shader might sample. Capacity includes old versions until cleanup; exhaustion leaves current state unchanged.

An ordinary instance record contains geometry/UV/color and a backend-private texture index. Final bindless shaders select that image for each instance; Vulkan marks the sampled-image index nonuniform where required. Use a small shared sampler set (nearest/linear), not one sampler per texture. Pipeline, blend, clipping and painter order still define batching boundaries; differing texture identity alone no longer splits a draw. Bounds-check index production and initialize all table entries. A stale slot must never silently become another texture, even if a shader index remains in range.

### 13.2 Metal: residency sets and direct resource IDs

**Verified source evidence.** `.cargo/config.toml` forces `MACOSX_DEPLOYMENT_TARGET = "26.0"` for the project (CK-22). `tools/ck` selects `/Applications/Xcode.app/Contents/Developer`; its SDK resolves to `MacOSX27.0.sdk` in this session. Read-only inspection of that SDK's `Metal.framework/Headers` confirms `MTLResidencySet` and queue `addResidencySet:` are available from macOS 15, and texture `gpuResourceID` from macOS 13. These APIs are within the actual deployment target; no older-OS fallback is required for M5.

Apple's published tables place M3 in Apple9 and Tier 2 argument buffers on Apple6 onward, with one million argument-buffer textures per stage for those recent families. Device-tier and capacity checks still belong in the implementation; family maxima are not memory budgets or evidence of this scene's performance. [Apple feature tables](https://developer.apple.com/metal/limits/).

**Proposal — recommended implementation.** Attach one residency set to the existing command queue. Add newly prepared native textures, staging/table/instance buffers or allocation heaps before their first submission and commit membership in bounded batches. Keep membership for the union of preparing, active and retired readers. Remove allocations only after all upload/frame/table pins are gone, then commit the removal before final release. Only the render owner changes membership; the preparation worker hands over retained allocations. A queue attachment persists across frames. Therefore an unchanged loaded set requires no application per-texture `useResource` loop each encoder/frame. The driver still performs residency work, and change-batch/commit cost must be measured. SDK `MTLResidencySet.h`/`MTLCommandQueue.h` provide the checked contract; [Apple residency-set documentation](https://developer.apple.com/documentation/metal/simplifying-gpu-resource-management-with-residency-sets).

Fill the two CPU-writable argument buffers with native `MTLResourceID` values obtained from textures, using the matching Metal shader representation and explicit layout/size/alignment checks at the C/Rust/shader boundary. Never reinterpret a logical key as a GPU ID. Retain the textures separately; copying an ID is not ownership. Tier 2 with direct IDs avoids an `MTLArgumentEncoder` operation for each image. Flush/synchronize host writes as required by the selected buffer storage mode before submission. [Apple: Go bindless with Metal 3](https://developer.apple.com/videos/play/wwdc2022/10101/).

**Alternatives evaluated.** `useHeap:` reduces per-encoder declarations to one per heap, and heap blocks can improve allocation control (§7.2). Residency sets avoid those repeated application declarations and support either standalone resources or heaps. Prefer the set for residency; select heaps separately if allocation evidence requires them. Metal 4's argument-table API is also available at this deployment target but adds no required capability to this design; keep the current command-buffer architecture. Old per-encoder per-image declaration cost is a real concern with tens of thousands of images, but **it is not a reason to drop Metal under the current target**. Vulkan on this Mac uses Metal through MoltenVK, so changing the public backend alone would not eliminate underlying residency costs.

The authorized implementation checks actual Tier 2 support, set creation and configured table support before enabling the bindless scene path. Keep referenced allocations in the set during all pending work; modifying membership is not a replacement for command synchronization. A failure is diagnosed, not worked around by silently returning to a per-frame 32,768-resource declaration loop. No GPU query was run for this draft; the manager's reported M3 Max/macOS 26.7.1 configuration is historical context, not a fresh measurement here.

### 13.3 Vulkan: separate image/sampler tables and actual-device gate

**Versioned starting evidence.** The project previously recorded MoltenVK 1.2.11. Its device source exposes descriptor indexing capabilities and, when Tier 2 Metal argument buffers are active, sampled-image update-after-bind limits of one million per stage/set. Sampler limits are distinct, derived from the Metal device with a fallback. This is source evidence for a known version, not proof of the installed ICD or settings. [MoltenVK 1.2.11 device source](https://github.com/KhronosGroup/MoltenVK/blob/v1.2.11/MoltenVK/MoltenVK/GPUObjects/MVKDevice.mm). Argument-buffer use is configurable even when enabled by default. [MoltenVK configuration](https://github.com/KhronosGroup/MoltenVK/blob/v1.2.11/Docs/MoltenVK_Configuration_Parameters.md#mvk_config_use_metal_argument_buffers).

**Proposal.** Use a sampled-image descriptor array of the configured 131,072 entries and a separate small sampler binding. The shader has a runtime image array; the layout allocates the fixed configured count. Use the sampled-image update-after-bind layout/pool class to access its large limits, but update only reusable immutable versions. Do not use combined image samplers at that count. Initialize every entry to a valid checker image view/layout; neither partially bound nor variable descriptor count nor update-unused-while-pending is needed by this design.

In an authorized GPU session capture actual ICD/driver/version/configuration and query feature/property chains. Require and explicitly enable core sampled-image array dynamic indexing as used by the shader, `runtimeDescriptorArray`, `shaderSampledImageArrayNonUniformIndexing` and `descriptorBindingSampledImageUpdateAfterBind` (with the appropriate Vulkan core/extension path). Check the proposed layout using `vkGetDescriptorSetLayoutSupport`. Verify per-stage, per-set and aggregate update-after-bind sampled-image/resource limits, the two samplers' limits, both simultaneously allocated table sets, and `maxUpdateAfterBindDescriptorsInAllPools`. Apply the binding, layout and pool update-after-bind flags consistently. A version string or advertised extension alone is insufficient. [Khronos descriptor-indexing guide](https://github.khronos.org/Vulkan-Site/guide/latest/extensions/VK_EXT_descriptor_indexing.html).

Descriptor updates happen on the render owner and only on an unpinned table version; no descriptor/command pool is concurrently mutated by the upload worker. Image views, memory and samplers survive table/frame references. Update-after-bind allows certain update timing but **does not authorize changing an entry while a pending invocation may access it**. Our versioning deliberately does not rely on that. Never use `vkDeviceWaitIdle`/`vkQueueWaitIdle` to reload, release or reuse a slot during normal operation. Poll known submissions and defer retirement; wait/retain rules at shutdown remain as established. [Khronos descriptor binding flags](https://docs.vulkan.org/refpages/latest/refpages/source/VkDescriptorBindingFlagBits.html).

**Proposal — unsupported devices.** If required features or configured capacity are unsupported, refuse activation of the new bindless scene path with a bounded diagnostic naming the missing feature/limit and requested/available values. Preserve the existing legacy demonstration where its original capabilities suffice. Ordinary bindings remain the development/baseline stage, not a silent final-scale fallback or a basis for calling 5.5 complete. Changing the configured capacity is an explicit configuration change; supporting a lower-capability final scene path is a future owner decision.

Record MoltenVK's measured residency/submission cost as part of parity acceptance; its implementation details may differ from our direct Metal backend. Both must satisfy the same lifetime, scale and responsiveness contract. No driver update, capability query, native run or GPU probe occurred in this design session.

## 14. The map boundary

**Firm — vision.** The map idea is live-rendered pixels with sprites above them and detail appearing with zoom. The recorded storage, tiles, algorithms, geographic resolution and GPU layout are deliberately Open.

**Proposal.** M5's reusable contribution is stable resource identity, replaceable immutable content, explicit ownership, bounded loading/upload/retirement, ordered draw instances and separate UI timing/input. PNG and CPU-generated images exercise two producers without deciding a geographic data model. The upload interface must not require a filename; the scene must not require that each quad corresponds to one file or one native draw call.

**Open.** Raster versus another authoritative map representation; continuous versus tiled storage; tile/chunk dimensions; projection/wrapping and world units; CPU/GPU generation; dirty-region updates; LOD/mips; zoom thresholds and procedural dithering; sparse/virtual texturing; terrain/resource encoding; culling; world/UI composition and separate render targets. A later map transform composes with the existing UI point/pixel boundary instead of silently reinterpreting it. M5 does not build a map package before those decisions exist.

## 15. Provenance and next step

Drafted 2026-10-23 and revised 2026-10-24 UTC by the Codex co-designer, Class S, `gpt-6-astra`, high effort, native Codex client; session model/effort and resolver evidence recorded in `codex-initial-take.md`. Authorship is design work, not implementation under the automatically assigned chat name `ck-worker-3`.

Sources: owner messages and ck-manager's entries in `.pipeline/coordination/runtime-m5/design-session.md`; both independent initial takes; AGENTS.md and `skills/AGENT-POLICY.md`; current vision/runtime/window/render documents; source files in §2; the linked primary API documentation. The two newest Claude entries supersede the earlier after-5.5 deferral and settle appearance/reload/Lua scope/scale. This revision independently verifies the selected SDK residency APIs and reads the owner’s Synarchy reference; its mechanisms are not copied.

Only this coordination draft and the author's appended coordination-log entry are written. No source/spec/docs publication, issue creation, acceptance action, builds, tests, windows or GPU queries are performed. Next: ck-manager verifies revision 4, both design reviewers check the delta against their approved revision 3, and the manager publishes the verified/reviewed document through the Codekarl docs lane as requested for this design session.
