# Milestone 3: windowing design

**Status: approved in independent design reviews 3 and 4 (2026-10-19); the native path (CK-59 to CK-66) is accepted, the last of it in batch `1792505018462023000` on 2026-10-20; the milestone 3 guide review is done. Milestone 3.5, GLFW with Vulkan (CK-78 `ck-window-glfw`, CK-79 `ck-gpu-vulkan`, CK-80 the `glfw-vulkan` run path), is accepted, the last of it in batch `1792551333304013000` on 2026-10-20.** The owner's demonstrations of the GLFW and Vulkan path (§10) and the milestone 3.5 review follow. Owner
decisions are marked **Firm**, with the relay that carried them. Everything else is a **Proposal** (ck-guide's or ck-manager's, named) or
**Open**. A Proposal is not an owner decision until the owner accepts it; the approved Proposals are settled in the issue specs and
their reviews.

**Milestone split (owner, 2026-10-19):** "yes, you can hold off on glfw and vulkan, we can do a milestone 3.5 after 3, we need to run
the guide when 3 is done too". **Milestone 3 is the native path** (a native window with Metal); **milestone 3.5 is the GLFW and Vulkan
path**, designed here but not yet filed. ck-guide reviews milestone 3 when it is accepted. Where this document describes two paths or
two demonstrations, the GLFW and Vulkan half belongs to milestone 3.5.

It extends `docs/runtime-design.md` (milestones 1 and 2). That document's rules still apply unless this one says otherwise: logging
(§4.4), the VM owners and their supervision (§5), and the exit statuses.

## 1. Owner direction (Firm)

The owner gave this direction in the ck-guide conversation on 2026-10-19, and ck-guide relayed it in #codekarl as `dr38tadsfyntyhsvz64f6dajes`,
`54va48x56xm97kdizwfp3s6fwi` and `5i4edh3ipubucjhh94udejxyws`.

- **Milestone 3 is a solid-colour clear.** A textured quad is the owner's proposed milestone 4.
- **Lua scripts:**
  - window creation;
  - the close callback.
- **Input in milestone 3 (owner, relayed by ck-guide as `vku4eet4xgjkwucthvuw8wmmci`):** "fullscreen is much later, input is now. it
  doesnt need a full input setup yet, just hardcode escape as the exit, the rest of input will be defered". So:
  - **windowed only.** Fullscreen is much later;
  - **Escape is hardcoded** as the exit key on both backends;
  - **all other input is deferred:** no Lua key-binding API, no key, mouse or scroll events, and no input queue.

  This supersedes the earlier direction that Lua binds the quit key, and ck-guide's broader input proposal.
- **Two paths, developed alongside each other:**
  - a native window with Metal;
  - GLFW with Vulkan, for eventual Linux support.
- **Milestone 3 is two demonstrations on this Mac,** one per path. The Vulkan SDK is already installed. (Since split: the native
  demonstration is milestone 3, the GLFW and Vulkan one milestone 3.5; see the status above.)
- **One window only.** There is no multi-window use case.
- **The UI VM owns the window API and its callbacks.** There is no third VM.
- **Changing backend while running:** the owner would ideally like it, but this is explicitly tentative. A future switch may recreate the
  window.
- **One close-request callback:** the native close button and the quit key both go to the same Lua close-request callback. The callback
  may accept or decline the close.
  - **ck-guide's reading of the input decision, kept explicit here:** the agreed callback still applies to Escape. What is hardcoded is
    the key, not the close policy, so Escape asks the callback, which may decline, exactly like the close button.
- **Callback errors in milestone 3:** a UI callback error logs its traceback and starts a clean application shutdown.
  - The owner wants errors caught and recovered from eventually: "eventually we should catch these".
  - So fatal callbacks are milestone 3's behaviour only, not a permanent principle.
- **Standing rules still apply:**
  - small, well-established libraries only, and nothing as large as a windowing library. GLFW is the owner's own choice for the generic
    path (`docs/vision.md`, "Engineering direction");
  - packages are reusable across future Rust projects;
  - no ECS, renderer abstraction or map representation is designed here.

**Owner answers given directly to ck-manager on 2026-10-19** (in the Claude session, by structured questions with recommendations and
trade-offs; recorded in `.pipeline/coordination/runtime-m3/handoff.md`):
- **Backend selection: "Config, CLI overrides".** Both backends are built into one executable. The configuration names the backend
  (`[window] backend = ...`), and a command-line option can override it for one run.
- **Headless when no backend is configured.** The owner first chose that a configured backend be required to run at all, then
  replaced that the same day: "yes, those can work without a config, only the opening of a window should not work, i guess the solution
  shuold probably be, implement a --headless mode then when there is no window/backend configured, automatically run in headless". Asked
  to confirm, the owner chose **"Yes, replace it"**, and for `window.open` in a headless run, **"Ignored, logged"**. §1.1 gives the
  rules.
- **Window size: "Fixed size".** The window is not resizable by the user; its size comes from Lua.
- **Teardown timing: "Window closes first".** The window and GPU are torn down before the VMs' `shutdown()` run.
- **No close callback: "Close".** A close request with no callback registered is accepted.
- **Signals:** a clean signal-requested shutdown exits **130** (SIGINT) or **143** (SIGTERM). A **second** signal during that shutdown
  forces the immediate exit with status **3**.
- **A stuck close callback: "Ctrl-C only".** The close button and Escape keep waiting on the callback; Ctrl-C always quits.
- **Callback error exit status: 1.**
- **Cmd-Q: "Nothing, no menu",** on both backends.
- **Windowed tests: "Full check requires it".** `tools/ck check` at the full level requires a logged-in desktop session, and fails
  rather than skips without one, as for the GPU today.
- **Waiting out a native drag:** "yes that is acceptable, as long as its simply a wait until the drag is done, then i see no issue".
  So a quit during a native modal interaction is kept, and takes effect as soon as the interaction ends (§8). Nothing else changes.

**ck-guide's Proposals:**
- **Selection:** both backends are built into one executable and chosen at startup, with one common Lua demo for both. The owner's
  configuration and command-line answer above settles the selection. The one common demo for both backends stays ck-guide's Proposal.
- **Acceptance:** the two Mac demonstrations are milestone 3's acceptance.
- **Linux:** a portability check in UTM (Vulkan 1.3 through Mesa's VirtIO Venus driver) when available, and testing on real Linux
  hardware for GPU and performance later.
- **Live switching:** a future feature, which may recreate the window and its resources.

**Settled since:** the window mode and input scope, as above.

### 1.1 Windowed and headless runs (owner, §1)

- **Windowed:** the configuration has `[window] backend = ...`, and `--headless` is not given. The backend is the configured one, or
  the one named by the override option.
- **Headless, automatically:** the configuration has no `[window]` table or no `backend` key, or there is no `--config` at all. The run
  is exactly today's milestone 1 or 2 run: no window, no render thread, no window backend initialised. Nothing accepted is reversed, and
  the existing tests keep their meaning.
- **Headless, forced:** `--headless` runs headless even when a backend is configured, for example over SSH or for a quick script run.
- **Usage errors:** `--headless` together with the backend override option exits 1 with the usual usage error. So does the override
  option on its own when the configuration has no backend: it overrides a configured backend, and does not stand in for one.
- **The mode is always visible:** the start record names it, for example `window: glfw-vulkan` or `window: none (headless)`. A
  missing `[window]` table is therefore never silent.
- **`window.open` in a headless run** is ignored: the call records nothing and returns, no window opens, and one `info` record says
  the window was not opened because the run is headless. The run phase returns at once, and the application shuts down normally and
  exits, as today. The same script runs unchanged windowed or headless.
- **Other commands:** `--help`, `--version` and `--metal-smoke` need no configuration and are unchanged (owner).
- **Signals and the run phase** apply to headless runs too (§6.1). A headless run has no presenter, so the resource latch is never set.

## 2. Facts that shape the design

- **macOS main thread:** AppKit requires the application, its windows and its event loop on the process's main thread. GLFW requires the
  same on macOS for window creation and event handling. So the main thread must own the window and run its event loop.
- **The main thread today** runs the executable and the runtime's supervisor (`run_report`). After start-up completes, it goes straight
  into the stop phase. Nothing runs between the two.
- **The UI VM today** serves only two requests, `startup()` and `shutdown()` (CK-53 and CK-57). There is no way to call into Lua after
  start-up. Milestone 3 needs that, so that window events can reach Lua callbacks.
- **This Mac:**
  - an Apple M3 Max running macOS 26.7;
  - the Vulkan loader reports instance version 1.3.296;
  - the loader uses the SDK's MoltenVK (driver 1.2.11, installed under `/usr/local`), not the newer Homebrew MoltenVK. It gives the GPU
    device Vulkan 1.2 and needs `VK_KHR_portability_enumeration`;
  - that device exposes `VK_EXT_swapchain_maintenance1`, with its `swapchainMaintenance1` feature enabled. The instance exposes
    `VK_EXT_surface_maintenance1` and `VK_KHR_get_surface_capabilities2`. It does not expose `VK_KHR_present_wait` or `VK_KHR_present_id`.
    (`vulkaninfo`, 2026-10-19);
  - GLFW is not installed through Homebrew.
- **Native calls that block:** `vkQueuePresentKHR` may block for a finite time that the implementation chooses, not the application
  (Vulkan reference page). Metal's `nextDrawable` can wait up to one second. A driver fault can make either wait longer. A thread making
  these calls cannot promise its own deadlines.
- **A `ck-lua` wait that times out cancels the VM:** `wait_ready` and `await_started` abandon the request on their deadline
  (`crates/lua/src/owner.rs`, `await_report`). So calling them repeatedly with short timeouts would cancel a healthy, slow start-up on
  the first timeout.
- **One thread per GPU device:** `ck-gpu-metal`'s `Device` is neither `Send` nor `Sync` (runtime design §2), so it is created, used and
  dropped on one thread.
- **No signal handling yet:** runtime design §4.2 deferred SIGINT and SIGTERM handling "with the first long-running loop". Milestone 3's
  event loop is that loop.

## 3. Package boundaries (Proposal, ck-manager)

| Package | Path | Purpose |
| --- | --- | --- |
| `ck-window` | `crates/window` | Shared types, no platform code: the window description, commands, events, key names, the close protocol and the backend trait the main loop drives |
| `ck-window-cocoa` | `crates/window-cocoa` | The native macOS window: AppKit through a small C header and Objective-C/ARC, like `ck-gpu-metal`. Its view is backed by a `CAMetalLayer` |
| `ck-window-glfw` | `crates/window-glfw` | The GLFW window and Vulkan surface creation, through a minimal binding (§9) |
| `ck-gpu-metal` | `crates/gpu-metal` | Existing. Gains a presenter that clears the layer's drawable to a colour, used on the render thread |
| `ck-gpu-vulkan` | `crates/gpu-vulkan` | New. Device, swap chain and the clear, with the present-lifetime rule of §8, through a thin Vulkan binding (§9). The instance and surface helpers are used on the main thread |
| `ck-lua` | `crates/lua` | Gains a general way to call into a started VM (§5). It still knows nothing about windows |
| `ck-runtime` | `crates/runtime` | Gains a run phase between start-up and stop (§4.1), and start-phase waits that a quit can interrupt (§6.1) |
| `codekarl` | `apps/codekarl` | Pairs a window with its GPU backend, runs the render thread, installs the Lua window API into the UI VM, and runs the loop |

- **No window crate depends on a GPU crate, and no GPU crate on a window crate.** The executable connects them through raw native handles,
  each passed through one documented `unsafe` constructor:
  - the `CAMetalLayer` pointer for Metal;
  - the `GLFWwindow` pointer and `VkInstance` handle for creating the `VkSurfaceKHR`.
- **Fixed pairs in milestone 3:** native with Metal, and GLFW with Vulkan. Other pairings are technically possible but **Open**: GLFW
  with Metal through its Cocoa window, or native with Vulkan through `VK_EXT_metal_surface`.
- **Platform scope:**
  - `ck-window-cocoa` and `ck-gpu-metal` build only on macOS;
  - `ck-window-glfw` and `ck-gpu-vulkan` contain no macOS-only code apart from MoltenVK's portability flags, which are required by the
    Vulkan specification on any portability implementation, so they can be built on Linux later.

## 4. Threads and ownership

**Proposal (ck-guide, extended by ck-manager):** the main thread owns the window, a **render thread** owns the GPU, and the UI VM owns
its Lua state. They talk only through messages.

- **The main thread owns:**
  - the application object and the event loop;
  - the runtime's supervisor, as today;
  - the window and its native backing: the view and its `CAMetalLayer`, or the GLFW window with its `VkInstance` and `VkSurfaceKHR`.
    AppKit and GLFW require these on the main thread;
  - the window's current description: title, size and clear colour.

  **The main thread never calls a GPU function that can block.**
- **The render thread** is created by the executable when the window opens, and owns:
  - the GPU device, queue, swap chain or layer drawing, and every GPU object, all created, used and dropped on that thread;
  - it gets from the main thread: the native handles (the layer pointer, or the instance and surface handles), the current pixel size,
    the clear colour, redraw requests and stop;
  - it reports: the first frame presented, a setup failure, and its end.

  It is supervised like a VM owner, with deadlines, and it is observed finished, never assumed finished (runtime design §5.1).
- **Why a separate render thread:**
  - a present or drawable acquisition can block for a time the driver chooses (§2);
  - on the main thread, that would stall event handling and, worse, the supervisor. The accepted design relies on the supervisor to
    enforce deadlines and to reach the immediate exit (runtime design §4.4 and §5.2);
  - on its own thread, a blocked GPU call is that owner missing its deadline, which the existing incomplete-shutdown path already
    handles, with exit status 3. Clean shutdown keeps its accepted guarantees.
- **The UI VM thread owns its Lua state,** as in milestone 2. The gameplay VM has no window API.
- **Two directions, neither ever waited on synchronously across threads:**
  - **requests, from the UI VM to the main thread:** the window's desired state (open, title, size, clear colour) and a
    close. A Lua window call records its request, wakes the event loop and returns at once. Lua never waits for the main thread. §4.2
    bounds them;
  - **events, from the main thread to the UI VM:** opened, and close requested (with its source, `button` or `key`). No other input is
    delivered in milestone 3. The main thread posts an event without waiting and keeps running its loop. §4.2 bounds them.

  The main thread passes the colour and size on to the render thread through a latest-value slot of the same kind, so the render thread
  never waits on the main thread either.
- **Waking the loop:** every producer that may need the main thread wakes it through one notifier with an explicit lifetime (§4.3).

### 4.1 The run phase (Proposal, ck-manager)

`ck-runtime` gains a run phase between "start-up complete" and the stop phase:
- **The run phase** calls an application-supplied main-loop closure on the calling thread, which is the main thread.
- **The runtime itself stays windowless.** It only gives the closure a quit signal, and records how the run ended.
- **Without a run closure,** or when the UI VM never opens a window, the run phase returns at once. Without a run closure, the
  runtime's milestone 1 and 2 tests are unchanged. A headless run (§1.1) gives no run closure.

The executable's run closure:
1. **creates the window and starts the render thread** from the description the UI VM recorded during `startup()`. Window requests
   from `startup()` are recorded, and applied only once start-up is complete. During start-up the main thread is busy supervising
   (`await_started`), so it could not serve them;
2. **sends `opened`** to the UI VM once the render thread reports that its first frame was **queued for presentation**: Vulkan's
   `vkQueuePresentKHR` returned success, or Metal's command buffer with `presentDrawable` was committed. That is not proof the frame is
   visible on screen; neither API here reports that (§8). The owner's demonstration is the visual evidence;
3. **runs the event loop:** it handles requests and window events, and passes colour, size and redraw requests to the render thread;
4. **returns** when a quit is decided (§6), or when the window or the render thread's GPU setup fails.

### 4.2 No cross-thread deadlocks (Proposal, ck-manager)

Each rule removes one way a wait could become circular.

1. **The main thread never waits on the UI VM during the run phase.** It posts events and reads requests without blocking. A UI callback
   that is slow or stuck does not freeze the window, which matches the vision's responsiveness goal.
2. **The UI VM never waits on the main thread.** Window calls record and return. A Lua script cannot observe a window synchronously: for
   example, it learns that the window is open from the `opened` event.
3. **Start-up never needs the main thread's window work.** Requests are recorded until the run phase, so no `startup()` waits on the
   supervisor that is waiting on it.
4. **Stop has no window work left.** Before the stop phase, the render thread is stopped and observed finished, and then the window is
   destroyed on the main thread (§7). Requests from
   `shutdown()` are discarded and counted in a `debug` record. So the stop phase is exactly milestone 2's, with its deadlines.
5. **Signals never call into Lua.** They set the quit request and wake whatever the main thread is waiting on (§6.1).
6. **The main thread never waits on the render thread without a deadline.** It sends the render thread state through a slot, and observes
   its end with the same non-abandoning, deadline-bounded polls as the VM owners (§6.1).

**Bounded traffic in both directions (Proposal, ck-manager).** No queue between the threads can grow without bound, and none is ever
waited on:

- **Requests are state, not a queue.** The window's desired state lives in one shared slot that holds the latest value of each field:
  - `set_clear`, the title and the size overwrite their field, so the last call wins;
  - a thousand `set_clear` calls in one callback cost one redraw with the final colour;
  - `open` and `close` are one-shot flags. A second `open` raises an ordinary Lua error in the calling script, at once, since there is one
    window. `close` after `close` changes nothing;
  - the slot's lock is held only to copy a small value in or out. It is never held while Lua runs or a native call is made.

  The slot is bounded by its size, so nothing can fill it, and it needs no admission rule.
- **Events have fixed bounds per kind:** milestone 3 has only two kinds, so at most two events are ever pending:
  - **close requested:** at most one outstanding. Escape presses, including key repeat, and close-button clicks while one is pending are
    absorbed, as above;
  - **opened:** sent once;
  - **posting never blocks.** Events reach the UI VM as posted calls (§5). While the call limit is reached, an event waits on the main
    thread, within those bounds, and is posted as results free slots.

  General input (keys, mouse, scroll) is deferred. When it is designed, it needs the same rule: coalesced or bounded, never blocking.
- **Posted calls and their results in `ck-lua` are bounded too (§5).** A handle has a fixed number of calls in flight: posting beyond it
  returns `Full` at once. Each call yields exactly one result, so results never outnumber calls in flight.
- **Disconnection:**
  - if the UI VM's thread has ended (failed, panicked or cancelled), its handle reports disconnection. The main thread logs it as an error
    and quits, with exit status 1, as for a callback error (§7);
  - if the run phase has ended, Lua window calls still record and return, and the main thread discards them. Their count goes in one
    `debug` record.

**Work per loop pass (Proposal, ck-manager).** A pass is one return of the native event pump. The guarantee is about Codekarl's own
work, not the native pump's:

1. **run the native pump once.** The two backends differ:
   - **native path:** Codekarl owns the loop, taking events with `nextEventMatchingMask:untilDate:inMode:dequeue:` and dispatching them
     itself, so it can stop after a fixed number of events and check the quit request;
   - **GLFW path:** `glfwPollEvents` and `glfwWaitEvents` process **every** pending event before they return, and GLFW has no
     count-limited pump (GLFW 3.4 event processing; its Cocoa implementation drains `nextEvent` until it returns nothing). Codekarl cannot
     cap the native work inside one call. A native backlog, or a modal loop (§8), delays the return by however long the system takes;
   - **on both:** Codekarl's own callbacks do constant work per event. They record into Codekarl's state (a flag, a coalesced size, the
     pending close request), and never call back into the window library;
2. **check the quit request** after the pump returns, and again between the following steps;
3. read the request slot once. This is constant work however many calls Lua made;
4. read at most a capped number of posted-call results and render-thread reports;
5. pass any changed colour or size, and a redraw request, to the render thread's slot. This never waits (§8);
6. enter the native wait, if nothing is pending. The notifier (§4.3) wakes it.

**What is guaranteed:** once the pump returns, Codekarl's work before the next quit check is bounded, and requests are a slot, not a
queue, so draining them cannot starve native events or the quit. **What is not:** how long the native pump takes to return. On the GLFW
path that includes the backlog and any modal loop; on the native path, only a modal loop.

**Quit observed versus teardown entered:** a callback may observe the quit request, but it only records it. Teardown, window
destruction and `glfwTerminate` start only in the outer loop, after the pump has returned, never from inside a callback. GLFW forbids
those calls from its callbacks (its reentrancy rules), and the native path keeps the same rule.

### 4.3 The notifier (Proposal, ck-manager)

Several threads wake the main thread: the signal thread, the UI VM (window requests and posted-call results), and the render thread
(reports). The native wake calls are only valid while the backend is initialised: GLFW allows `glfwPostEmptyEvent` from any thread only
between `glfwInit` and `glfwTerminate`. So every wake goes through one backend-neutral notifier with an explicit lifetime:

- **States:** inactive (before the backend is initialised), armed (holding the native wake target), disarming, and disarmed. They are
  shared by all producers.
- **Every wake first records a pending flag,** so no wake is ever lost. The main thread checks the flag before each native wait. Its
  start-phase waits are sliced polls that check the quit request (§6.1). Its stop-phase waits are unchanged, and bounded by their
  deadlines; a quit then changes nothing (§6.1). So a wake while inactive or disarmed still
  reaches the main thread, without any native call.
- **Only while armed** does a wake also make the native call (`glfwPostEmptyEvent`, or posting an application-defined event to
  `NSApp`). Before making it, the producer registers itself as **in flight**, under the notifier's lock, and it deregisters afterwards.
  The lock is never held across the native call.
- **Arming:** the main thread arms the notifier only after the backend is initialised, and before its first native wait.
- **Disarming:** before any native teardown (window destruction, `glfwTerminate`, releasing the application's event target):
  1. the main thread moves the notifier to disarming, so no new producer makes a native call;
  2. it waits, with a deadline, until no producer is in flight. It never joins a producer thread;
  3. only then does it tear the backend down.

  If the in-flight count does not reach zero by the deadline, a native wake may still be running against the backend. Teardown then
  does not destroy the backend, the resources stay unproven (§7, "The failure boundary"), and the run ends incomplete with exit status
  3.
- **Producers of wakes:**
  - the signal thread, on SIGINT or SIGTERM;
  - a Lua window call, after recording its request;
  - the UI VM's owner thread, after sending a posted-call result (§5);
  - the render thread, after sending a report (first frame queued, setup failure, cleanup result).

  So a sleeping loop is woken for `opened` and for failures, not only for requests.

## 5. Calling into a started VM (Proposal, ck-manager)

`ck-lua` gains a general call mechanism. It knows nothing of windows: the window API is an application extension.

- **Extension at creation:** `VmSettings` takes an optional extension: a `Send` closure that runs on the owner thread after the standard
  host surface is installed and before the entry script, and registers further globals. Codekarl gives one only to the VM named as the
  window's owner.
- **Posting calls:** in the Started state, the handle can **post** a call: a callback name and `ConfigValue` arguments. The thread runs the
  callback registered under that name and reports the result. The result is either the callback's return value, or a failure with its
  message and traceback. A call to an unregistered callback reports "not registered", which the application can treat as a default.
- **Asynchronous results:** posting never waits. Results arrive on the handle's report channel, and the main thread reads them without
  blocking.
- **Bounded:** the application sets a limit on calls in flight when it creates the owner. Posting beyond it returns `Full` at once, and
  sends nothing. Each call yields exactly one result, so results are bounded by the same limit.
- **Disconnection:** once the owner's thread has ended, posting returns a disconnection error, and no result is pending.
- **A result wake:** the application can give the owner a `Send + Sync` callback, which the owner thread calls after sending each
  result. Codekarl uses it to wake the main thread through the notifier (§4.3). `ck-lua` knows nothing of what it does.
- **One call at a time,** in order, on the VM's own thread. A stop request is served after the call that is running, as `shutdown()` is
  today. The instruction hook (CK-55) still interrupts a looping callback when the VM is cancelled.
- **A failed callback is caught and unwound, nothing more.** The Lua error unwound to the owner thread, and the state can still serve
  calls. That is not a rollback: whatever the callback changed before it failed stays changed, including globals, recorded window
  requests and log records. A usable VM is not a VM in the state the script intended. Recovery policy (what, if anything, to undo, retry
  or reset) is future work, and the owner wants it eventually. Keeping the VM serviceable only avoids ruling that work out.

## 6. Closing and quitting

**Firm:** the close button and Escape go to the same Lua close-request callback, which accepts or declines. Escape is hardcoded on both
backends; that the callback may decline it is ck-guide's reading, kept explicit (§1).

**Proposal (ck-manager):**
- **Lua surface (UI VM only):**
  - `window.open{title=, width=, height=, clear={r, g, b}}`, a fixed-size window (owner, §1);
  - `window.set_clear(r, g, b)`;
  - `window.on_close_request(fn)`, where `fn(source)` receives `"button"` or `"key"` and returns `true` to accept;
  - `window.on_opened(fn)`;
  - `window.close()`.
  The exact names are for the issue spec.
- **The close button:** the native window refuses to close by itself. AppKit's `windowShouldClose:` returns `NO`, and GLFW's close flag is
  reset. Instead, the main thread posts `close requested (button)`.
- **Escape:** a press of Escape posts `close requested (key)`, on both backends: AppKit's `keyDown:` (or `cancelOperation:`) in the
  native window, and a GLFW key callback for `GLFW_KEY_ESCAPE`. Releases and repeats are ignored while a request is pending. No other
  key is handled, and nothing reaches Lua as key input.
- **Accept:** the callback returns `true`, or no callback is registered (owner, §1: no callback means the close is accepted). The window
  closes, and since there is only one window, the application quits.
- **Decline:** the callback returns anything else. Nothing happens, and the window stays open. The close request is then no longer pending.
- **`window.close()`** closes without asking the callback. This is the script's own decision, and it also lets an automated test open,
  present and close a window without synthetic input (§10).
- **Closing versus quitting:** in milestone 3, closing the only window quits the application. A future application could close and
  recreate its window without quitting, for example to switch backend. That is why closing (destroying the window) and quitting (ending
  the run phase) are separate steps inside the main loop, even though milestone 3 always does both.
### 6.1 Signals (Proposal, ck-manager)

SIGINT and SIGTERM request a quit directly. They do not go through the Lua callback: Ctrl-C must work even if a callback declines, loops
or hangs.

- **Which commands:**
  - only the runtime command (`codekarl`, with or without `--config`, windowed or headless) installs signal handling;
  - `--help`, `--version`, `--metal-smoke` and usage errors keep the default signal behaviour, and finish as today.
- **Mechanism:**
  - after argument parsing, and before the logger's writer or any other thread is created, the main thread blocks SIGINT and SIGTERM, so
    every later thread inherits the mask;
  - one signal thread waits for them with `sigwait`. On a signal it sets the shared quit request, and wakes whatever the main thread may
    be waiting in;
  - no code runs inside a signal handler;
  - **if the signal thread cannot be created,** the main thread restores the previous mask, so the signals are never left blocked with
    nothing consuming them. It then reports the failure as `codekarl: <message>`, like any error before the logger starts, and exits 1.
- **A quit during the start phase** (spawn, readiness, start requests and waits):
  - **no repeated abandoning waits.** Today's `wait_ready` and `await_started` abandon the request when their timeout passes (§2), so
    they cannot be sliced;
  - **a non-abandoning poll:** `ck-lua` gains `poll_ready(within)` and `poll_started(within)`, and the runtime's `Owner` trait gains the
    same pair. Each waits at most `within`, and returns ready, started, a failure, or **pending**. Pending changes nothing: no cancel flag,
    no state change, and the request stays in flight. The existing calls keep their exact behaviour;
  - **one real phase deadline:** the supervisor keeps the single absolute start-phase deadline it has today. It polls each owner in
    slices (a size for the issue spec, on the order of 10 ms). It checks the quit request between slices, and between the non-waiting
    steps too (each spawn, and each start request). When the phase deadline itself passes, it takes today's missed-deadline path once:
    the abandonment, the `missed its start deadline` record, and cancel-all. A slow start-up that finishes before the phase deadline is
    therefore unaffected by slicing;
  - **on a quit,** the supervisor does exactly what it does on a start failure: it cancels every spawned owner, waits for them against the
    cleanup deadline, and stops the started services;
  - one record is logged, `runtime: quit requested during start-up`, and it is not an error;
  - **priority:** a quit does not hide a worse result. If any owner missed a deadline or was not observed finished, the run is still
    incomplete, with exit status 3, as today. The quit's own exit status applies only to an otherwise clean result;
  - the run phase is skipped.
- **A quit during the run phase** wakes the event loop, which ends at its first quit check after the native pump returns (§4.2). The
  usual teardown and stop phase follow.
- **A quit during the stop phase** changes nothing. The stop phase is already bounded by its deadlines.
- **The signal thread's own end:**
  - it owns nothing. It takes the notifier's lock only briefly (§4.3), and never holds it across a native call. Nothing ever waits on
    it or joins it;
  - when `main` returns or the immediate exit runs, the process ends and the thread with it.

  So it adds no wait to shutdown.
- **Limits:**
  - a signal sent to the process group also reaches its child processes. A child started from Lua (`os.execute`, `io.popen`) inherits the
    blocked mask, so it ignores SIGINT and SIGTERM unless it resets its own mask. That is the standard behaviour of a blocked mask, and is
    stated in the crate docs;
  - while a native modal interaction runs (§8), a quit is recorded, and takes effect only when control returns to the outer loop. The
    render thread keeps drawing meanwhile.
- **Exit status (owner, §1):** a clean signal-requested shutdown exits 130 for SIGINT and 143 for SIGTERM, unless a worse result applies
  (the priority rule above).
- **A second signal (owner, §1):** during the shutdown that the first one started, a second SIGINT or SIGTERM ends the process at once
  through the immediate exit, with status 3, after one non-waiting attempt to log it. The resource latch is irrelevant then, because no
  release happens.

## 7. Errors and cleanup

- **Callback errors.**
  - **Firm (owner, milestone 3 only):** a UI callback error logs its traceback, and starts a clean application shutdown. It is not a
    permanent principle: "eventually we should catch these".
  - **Proposal (ck-manager), how:**
    - one `error` record with the traceback, for example `error runtime: ui callback <name> failed: <message>`;
    - then the clean shutdown follows the normal sequence: the event loop ends, the render thread and window are torn down (below), and
      the stop phase runs as in milestone 2, so both VMs' `shutdown()` run;
    - exit status 1, as for a start or stop failure (owner, §1).
  - **Caught, not rolled back:** the error is caught and unwound, and the VM stays serviceable, but nothing the callback did is undone
    (§5). Recovery policy is future work.
- **Window or GPU setup fails:** for example, the GLFW library or Vulkan loader is missing, a required extension is missing, no device
  is suitable, or AppKit refuses.
  - One `error` record names the backend and the failure.
  - **Setup is partial,** so it is torn down like a full presenter. Every object is recorded in the presenter's ledger as it is created,
    and the teardown below destroys exactly what exists, under the same proof rules.
  - **Exit status 1 only if that cleanup is proven:** the render thread, if started, was observed finished and sent a successful cleanup
    result, and every main-side object was destroyed. The run phase then returns, and the stop phase runs normally.
  - **Otherwise** the resources stay unproven, and the run ends incomplete with exit status 3 (the failure boundary, below).
- **A backend not built in** (its Cargo feature off) is a configuration error at load: exit 1, as today.
- **Teardown** (Proposal, ck-manager), when the event loop ends. It follows dependencies, not just the reverse order of creation: nothing
  is destroyed while anything that uses it still exists, or while the GPU or the presentation engine may still use it.
  1. **The main thread asks the render thread to stop,** and polls for its end against a teardown deadline (a size for the issue spec).
  2. **The render thread, on its own thread, proves the GPU and the presentation engine are done** (§8, "Present lifetime"), with bounded
     waits:
     - Vulkan: `vkWaitForFences` with a timeout on every outstanding submit fence and present fence. Never `vkDeviceWaitIdle` or
       `vkQueueWaitIdle`, which have no timeout;
     - Metal: semaphores signalled by the last command buffer's completion handler, and by the last drawable's presented handler, waited
       with a timeout. Never `waitUntilCompleted`.
  3. **The render thread then destroys its objects in dependency order:**
     - **Vulkan:**
       1. command buffers and their pool;
       2. the per-frame fences, and the acquire and render-finished semaphores, all proven unused;
       3. framebuffers or image views, and the render pass;
       4. the swap chain, and any retired swap chain still held (§8);
       5. the device;
     - **Metal:**
       1. any drawable and command buffers still held;
       2. the command queue and other device objects;
       3. the device;
     - it then sends an explicit **cleanup result**: success only if every wait above proved the objects unused and every object was
       destroyed.
  4. **Only when both hold:** the render thread is observed finished (`is_finished`, then joined), **and** its cleanup result is success.
     Then the main thread destroys what it owns, in dependency order:
     - **GLFW:**
       1. the surface, which needs the instance and the live window;
       2. the debug messenger, if any, and the instance;
       3. the GLFW window;
       4. `glfwTerminate`;
     - **native:**
       1. the layer is detached from its view;
       2. the window, its view and the layer are closed and released.
  5. The run phase ends. The stop phase then runs as in milestone 2.
  - **Late presenter failures** (CK-73, from guide finding GUIDE-1, accepted 2026-10-20): once the render thread is observed finished,
    the loop takes every report it left, before disarming or destroying anything, because a finished thread's report queue is finite.
    A `Failed` report seen there, or queued before a close or quit, is not lost: the loop ends with the presenter failure, the error is
    logged once (each distinct late failure once, next to any earlier primary failure), and the run exits 1, or 3 if cleanup is
    unproven. A presenter that keeps reporting past a fixed limit is itself a reported failure. The trait and loop wording is in
    `crates/window` (`link.rs`, `lib.rs`).
- **If the render thread misses its teardown deadline, the main thread destroys nothing:**
  - the render thread may still be using the device, the swap chain, the surface, the instance, the layer and the window behind them, so
    the main thread keeps all of them alive. It does not join the render thread;
  - the run is reported as incomplete, and the process ends through the existing immediate exit with status 3 (runtime design §4.4);
  - **no `Drop` runs:** the main thread's window, surface and instance are held in `ManuallyDrop`, as are the render thread's GPU objects,
    and only the explicit teardown above releases them. A panic or unwinding on any path therefore cannot release an object that may
    still be in use. Ending the process reclaims them;
  - **finished is not enough.** A render thread that panics, disconnects, or ends without sending a successful cleanup result can be
    finished with its `ManuallyDrop` GPU objects still alive and in use by the GPU. The main thread treats every one of these cases exactly
    like a missed deadline: it keeps every object, and the run ends incomplete with status 3;
  - the same holds if the render thread's own bounded waits time out: it destroys nothing, and reports failure.
- **The failure boundary** (Proposal, ck-manager). `ManuallyDrop` only keeps objects alive. It does not make the process end correctly.
  Today `Runtime::run_report` does not catch panics, and the executable's panic hook, after a complete log flush, returns and lets the
  standard unwind continue (runtime design §4.2). So a panic on the main thread during the run phase could unwind out of `main` with the
  ordinary panic exit, not the incomplete exit. The executable therefore adds a **resource latch**:
  - **set** when presenter setup creates its first native or GPU object, before the object exists;
  - **cleared** only when teardown is proven: the render thread observed finished, its cleanup result success, and every main-side
    object destroyed;
  - **read by the panic hook.** For a panic on the main thread or the render thread while the latch is set, the hook treats the shutdown
    as incomplete, whatever the log flush says. It makes its bounded report, spends the error grace if an error is undelivered, and ends
    the process through the immediate exit with status 3. It never returns into an unwind that could pass over unproven resources;
  - **panics on other threads** (the VM owners) are unchanged: caught at their thread boundary and reported as that owner's failure. The
    run then shuts down through the normal path, which reaches the latch through teardown;
  - **the runtime still never terminates the process.** The latch and the immediate exit belong to the executable, as in runtime design
    §4.4.
- **Teardown timing (owner, §1: "Window closes first"):** before the stop phase. The stop phase stays exactly milestone 2's, and the
  window disappears promptly.
- **Resource lifetime and future switching:**
  - a window's native objects (on the main thread) and its render thread make up one **presenter**: created together and torn down
    together, in the order above;
  - the backend-independent description (title, size, clear colour) lives outside it, in `ck-window`;
  - so a later live switch can tear one presenter down and build another from the same description, without Lua noticing more than a new
    `opened` event. Milestone 3 does not build switching, but it keeps this boundary.

## 8. Drawing (Proposal, ck-manager)

All drawing runs on the render thread (§4).

- **Clear and present:** each frame clears to the current colour and presents:
  - Metal: the layer's next drawable, and a render pass with a clear load action;
  - Vulkan: the swap chain image, and a render pass (or dynamic rendering) clear.
- **Redraw on demand:**
  - when the window first appears;
  - when it needs display, or its pixel size changes (moving between displays with different scale factors changes the pixel size of
    a fixed-size window);
  - when the clear colour changes.

  A continuous frame loop is deferred until there is something that animates (the textured quad, or later). This keeps an idle window from
  using the CPU and GPU.
- **High-resolution displays:** the drawable or swap chain uses the window's pixel size, not its point size.
- **Fixed size (owner, §1):** the window is created without the native resizable style (`NSWindowStyleMaskResizable` off, and
  `GLFW_RESIZABLE` false). There is no user resize, and no live-resize loop.

### Present lifetime (Vulkan)

A submit fence proves that the GPU finished the frame's commands. It does not prove that the presentation engine has finished with the
render-finished semaphore, the image or the swap chain (Khronos: "Swapchain semaphore reuse", and the swapchain recreation sample). So
milestone 3 uses present fences:

- **Required extensions:** the device's `VK_EXT_swapchain_maintenance1` with its feature enabled, and the instance's
  `VK_EXT_surface_maintenance1` and `VK_KHR_get_surface_capabilities2`. They are present on this Mac (§2). A device without them fails
  window setup with an error naming the missing extension (exit 1, §7). A fallback for such devices is later work, to be checked against
  the Linux driver when the UTM check runs.
- **Present fences:** every `vkQueuePresentKHR` carries a present fence (`VkSwapchainPresentFenceInfoEXT`). A present counts as finished
  only once its present fence has signalled.
- **Semaphore reuse:**
  - **acquire semaphores** are recycled after the submit that waited on them has completed (its submit fence);
  - **render-finished semaphores** are taken from a pool per present, and go back to the pool only after that present's fence has
    signalled. They are never indexed by frame alone.
- **Acquired images are always accounted for.** An acquired image is either submitted and presented, or released with
  `vkReleaseSwapchainImagesEXT`. Its acquire semaphore still has a pending signal either way. So the semaphore is consumed by a submit
  that waits on it (an empty submission if the frame was abandoned), and returns to its pool only after that submit's fence.
- **Fixed caps:**
  - the acquire semaphore pool and the render-finished semaphore and present fence pool each have a fixed size, derived from the swap
    chain's image count plus the retired swap chain's;
  - when a pool is empty, the frame is skipped, and the redraw stays pending until a fence frees an entry;
  - so outstanding presentation is bounded by the pools, not by "one frame in flight", which bounds only the GPU's command work.
- **Swap chain rebuild** (on `VK_ERROR_OUT_OF_DATE_KHR`, `VK_SUBOPTIMAL_KHR`, or a pixel-size change from a scale-factor change):
  - **at most one retired swap chain.** While one is retired and still waiting on its present fences, further rebuilds pause. Pixel-size
    changes meanwhile are coalesced to the latest size, and the rebuild runs once the retired swap chain has been destroyed;
  - the new swap chain is created with the old one as `oldSwapchain`;
  - images acquired from the old swap chain but never presented are released, as above;
  - the old swap chain is destroyed only after every one of its present fences has signalled. That is checked without waiting on each
    frame;
  - at teardown, the retired swap chain's present fences are waited on with the teardown timeout (§7).
- **A ledger, with every error transition named.** The render thread keeps a small ledger of every acquire, submit, present and retired
  swap chain, and the fence or drain submit that will prove each one finished. The issue spec lists each transition, at least:
  - **an acquire that succeeds** records the image as acquired, and its acquire semaphore as pending a signal;
  - **a submit** records the submit fence for the acquire semaphore and for the image's rendering;
  - **a present that returns success, `VK_SUBOPTIMAL_KHR`, `VK_ERROR_OUT_OF_DATE_KHR` or `VK_ERROR_SURFACE_LOST_KHR`** still queued its
    semaphore wait and present fence, so the ledger records the present fence as outstanding;
  - **a present that fails with `VK_ERROR_OUT_OF_HOST_MEMORY` or `VK_ERROR_OUT_OF_DEVICE_MEMORY`** queued nothing. Its present fence is
    never waited on, because it was never submitted. The render-finished semaphore still has a pending signal from the submit, so it is
    drained by a submit that waits on it before it is reused;
  - **a frame abandoned after acquire:** the image is released, and the acquire semaphore drained, as above;
  - **swap chain creation that fails:** the old swap chain passed as `oldSwapchain` is retired anyway, as the specification says, and
    can no longer be acquired from. It stays the one retired swap chain, rendering pauses, and the rebuild is retried once that swap
    chain is destroyed;
  - **`VK_ERROR_DEVICE_LOST`:** nothing can be proven, so every object is kept, and the cleanup result is failure (exit status 3).
- **Zero size:** while the window's pixel size is zero in either dimension (for example while minimised), rendering is suspended. No
  zero-size swap chain is created.
- **A present fence proves release, not display.** `VK_EXT_swapchain_maintenance1` lets the present fence signal before the frame is
  actually shown. It proves that the semaphore and swap chain resources may be released, nothing more. This draft claims no
  display-completion guarantee; `VK_KHR_present_wait` is not available here (§2).
- **Destroying:** a fence, semaphore, image view or swap chain is destroyed only once the fences proving it unused have signalled. On a
  timeout it is kept, never destroyed on a guess.

### Present lifetime (Metal)

Core Animation owns the drawables. The render thread holds a drawable only within one frame step: it lets go when the frame is presented
and committed, or at once when the frame is abandoned. What it keeps is a small token per drawable, set by that drawable's presented
handler (`addPresentedHandler`), and a completion token per command buffer. **Before it releases the command queue or the device,** it
waits, against one absolute stop deadline, for every command buffer's completion and every outstanding drawable token (CK-64).

**Verified, 2026-10-19** (ck-manager's research probe, `.pipeline/coordination/runtime-m3/metal-probe/`): on this Mac the presented
handler ran for a presented drawable, for one acquired and discarded without presenting, and for one presented while the window was
minimised, with `presentedTime` 0 each time (a release signal, not proof of display). Setting `drawableSize` and calling `nextDrawable`
from a non-main thread worked, and the Main Thread Checker reported nothing. The presenter issue keeps these as regression tests.

### Responsiveness, stated as it is

- **The main thread** makes no GPU rendering wait: rendering, acquisition and presentation are on the render thread. That is all the
  render thread removes. The main thread can still be delayed by native initialisation and window operations, by the operating system,
  and by brief holds of the slots' locks, and this draft claims no absolute bound on those. §4.2 bounds the work it does per pass,
  not wall-clock time. The native modal loop (below) is a further delay.
- **The render thread can block,** and this draft does not pretend otherwise:
  - `vkQueuePresentKHR` may block for a finite time the implementation chooses;
  - `vkAcquireNextImageKHR` is called with a timeout, but the specification lets an implementation return as soon as it can, not
    exactly at it;
  - Metal's `nextDrawable` waits up to one second, given `allowsNextDrawableTimeout = YES`. That is the default, and the presenter
    sets it explicitly;
  - a frame being finished (the previous submit fence) does not prove a drawable or swap chain image is available.

  The visible effect of such a wait is a late frame: a colour change shows late. The main thread's event handling and quit request do not
  wait on it.
- **What stays guaranteed:**
  - **once teardown has begun,** shutdown is bounded. The render thread is an owner with a teardown deadline, and a render thread
    blocked past it ends the run as incomplete, with exit status 3 (§7), exactly as a stuck VM does today. Teardown begins only in the
    outer loop, after the native pump returns (§4.2). The time from a quit request to that point is not bounded by Codekarl: it includes
    the native pump's backlog and any native modal interaction (below);
  - one frame in flight, and redraw on demand, keep the render thread's normal waits short. They are a performance choice, not a proof of
    any bound.
- **Pixel-size changes:**
  - the main thread passes the new pixel size to the render thread through its slot;
  - the render thread applies it on its next frame. Vulkan rebuilds the swap chain as above; Metal sets the layer's `drawableSize`. The
    question of whether `drawableSize` may be set from the render thread is a technical question to verify, like the presented handler;
  - a rebuild never waits without a timeout. On a timeout the old swap chain stays, and the rebuild is retried.
- **Native modal interactions, the same on both paths:** with a fixed-size window there is no live-resize loop. Some native interaction
  may still keep control inside the window system for a while, for example a title-bar window move if macOS tracks it in the process,
  or GLFW's event pump draining a backlog. Which interactions do this is a technical question to verify for each backend. During any of
  them, on either path:
  - Codekarl's callbacks and any run-loop timer only **record** the quit request. They never start teardown (§4.2);
  - the quit is **kept**, and takes effect as soon as control returns to the outer loop;
  - the render thread keeps drawing.

  The owner accepted exactly this: "acceptable, as long as its simply a wait until the drag is done" (§1). Codekarl provides no
  mechanism to force control out of a native modal interaction, and claims none.

## 9. Dependencies (designer recommendation, ck-manager; §11)

| Need | Proposal | Alternatives |
| --- | --- | --- |
| Native window | Own Objective-C/ARC bridge with a C header, like `ck-gpu-metal`; system frameworks only | none |
| GLFW | Own minimal binding (about twenty functions), linking GLFW 3.4 that `tools/bootstrap` installs and `tools/ck doctor` checks | the `glfw` crate (larger, can build GLFW from source with CMake) |
| Vulkan | `ash`: thin, generated, widely used, loading the system loader at runtime | own bindings (large); higher-level crates (too large for the rule) |
| Shaders | none for a clear | `glslc` from the SDK when the quad needs shaders (milestone 4) |

- **Validation:**
  - Vulkan tests enable `VK_LAYER_KHRONOS_validation` from the installed SDK, as the Metal test enables Metal validation;
  - a validation error fails the test;
  - a missing loader or layer fails the full check level, and is never skipped silently.

## 10. Proposed acceptance evidence

**Automated** (in `tools/ck check`):
- **`ck-window`:** the close protocol and the deadlock rules, driven by a fake backend on the test thread, with no window system:
  - accept, decline, and the default with no callback;
  - coalescing of repeated close requests;
  - a callback error ends the loop and gives exit status 1;
  - requests after the loop ends are discarded and counted;
  - a quit request wakes a sleeping loop;
  - a stuck UI callback does not delay a signal quit beyond one pass;
  - **the request slot:** many `set_clear` calls between two iterations give one redraw with the last colour, and a second `open` raises
    a Lua error;
  - **bounded events:** a burst of close requests (Escape repeat, button clicks) leaves at most one outstanding, and `opened` is sent once;
  - **work per pass:** after a pump return, Codekarl's own work before the next quit check is bounded, with many requests, results and
    reports pending. The fake backend models the real adapter contract: the GLFW fake drains every queued event per pump call, as GLFW
    does, and only the native fake stops after its cap;
  - **quit observed versus teardown entered:** a quit recorded inside a fake callback does not start teardown until the pump returns,
    and a teardown call from inside a callback is refused;
  - **a held modal interaction:** a fake pump that holds control (for both fake backends) while a quit arrives. The quit is recorded and
    kept, teardown does not begin while the pump is held, and teardown begins on the first pass after it returns;
  - **Escape maps to the close request** on each fake backend, and no other key produces an event;
  - **disconnection:** a UI VM whose thread ended makes the loop quit with status 1;
  - **the notifier:**
    - with gates, a wake that is in flight when disarming begins is waited for, and no native wake happens after disarm;
    - a wake before arming and one after disarming set only the pending flag, and the main thread still sees it;
    - a signal before backend initialisation, and one during teardown, are handled;
    - a UI request during the stop phase makes no native call;
    - a sleeping loop is woken by a render-thread report alone (`opened`, and a setup failure);
    - an in-flight wake that never finishes makes the run end incomplete, with nothing torn down;
  - **no release without a successful cleanup result:** a fake render thread that panics, one that ends without sending a result, and
    one that reports failure each leave the main thread's window objects alive, and end the run incomplete with status 3;
  - **a missed render-thread deadline:** with a fake render thread that never ends, the main thread's window objects are never dropped
    (a drop-recording value proves it), whether on the normal path, after a panic in the run phase, or on the immediate exit path, and the
    run ends incomplete with status 3;
  - **a render thread blocked in a fake present** does not delay event handling or a quit beyond one pass.
- **`ck-lua`:**
  - `poll_ready` and `poll_started` return pending without changing the state or setting the cancel flag, and a later poll or the
    existing wait still sees the result;
  - posted calls run in order after start-up and return values;
  - a failing callback reports its traceback, and the VM keeps serving. Its side effects are not undone, and the test shows a global it set
    is still set;
  - a looping callback is interrupted on cancel;
  - posting beyond the in-flight limit returns `Full` without waiting, and posting to an ended owner returns a disconnection error;
  - the extension runs before the entry script;
  - the milestone 2 tests are unchanged.
- **`ck-runtime`:**
  - the run phase runs between start-up and stop. Without a run closure, every `ck-runtime` milestone 1 and 2 test passes unchanged;
  - **a quit during the start phase:** a fake owner stuck in readiness, with a long start deadline. A quit request is seen within one wait
    slice, every owner is cancelled and observed against the cleanup deadline, and the run phase is skipped;
  - **slicing does not cancel:** a fake owner whose readiness and start each take many slices (for example 200 ms against 10 ms slices,
    within the phase deadline) starts normally, with no quit, no cancel and no missed-deadline record;
  - **one real deadline:** an owner that overruns the phase deadline gets exactly one abandonment and one `missed its start deadline`
    record, as today.
- **Signals (application test):**
  - a run receiving SIGTERM during a `startup()` that waits on a fixture file ends promptly and cleanly, well before its start
    deadline, with exit status 143 (the window is never opened, since start-up did not finish);
  - SIGINT after `window opened` gives a clean shutdown and exit status 130;
  - a second signal during that shutdown, held open by a `shutdown()` that waits on a fixture file, ends the process at once with
    status 3;
  - `--help` and `--version` keep the default signal behaviour.
- **Windowed and headless selection (application test and CLI spec):**
  - no `--config`, a configuration without `[window]`, and one without `backend` each run headless, with `window: none (headless)` in
    the start record, exactly the milestone 1 or 2 records otherwise. The accepted CLI spec and runtime tests are unchanged apart from
    that start-record field;
  - `--headless` with a configured backend runs headless;
  - a headless run whose UI script calls `window.open` logs one `info` record, opens no window and exits 0;
  - `--headless` with the override option, and the override option without a configured backend, are usage errors (exit 1);
  - an unknown backend name, in the configuration or the option, is a configuration error (exit 1).
- **`ck-gpu-metal` and `ck-gpu-vulkan`:** an off-screen clear to a known colour is read back and compared, under each API's validation.
  This proves the clear without a window or screen capture.
- **Present lifetime (`ck-gpu-vulkan`, with a window, under validation):** many presents, several swap chain rebuilds forced through the
  failure-injection seam's out-of-date results, and a teardown. Validation reports no semaphore or swap chain in use when reused or
  destroyed, and every retired swap chain is destroyed only after its present fences.
- **The failure boundary (executable level):** the gpu_probe test already runs a supervised worker process. The same pattern, a test
  binary re-executing itself in a child, runs the executable's run-phase code with a fake presenter. No hook is added to the production
  binary:
  - a main-thread panic during the run phase, with a healthy stdout so the log flush completes, and the latch set: the child exits 3,
    and its main-side objects are not released;
  - a partial setup failure that unwinds: exit 3 if cleanup is unproven, and exit 1 only if it is proven;
  - a render thread that ends with no result, or a failed one: exit 3.

  A drop-recording value proves that nothing was released; only the child's exit status proves the exit path.
- **Vulkan failure injection (`ck-gpu-vulkan`):** a thin test seam over acquire, present and swap chain creation injects each error
  transition of the ledger (§8). The test checks the ledger's state, and that nothing is destroyed before its proving fence. This runs
  alongside the validation runs, which cover only the successful paths.
- **Binary, per backend:** a supervised child, with a whole-process deadline, runs a fixture script. The script opens a window, and
  `window.close()`s it on `opened`, which means the first frame was queued for presentation, not that it was seen (§4.1). Expected:
  exit 0, and the records in order (`window opened`, `window closed`, `shutdown complete`).
  A second fixture sends SIGTERM after `window opened`, and expects a clean shutdown with exit status 143.
- **Not tested automatically:** real button clicks and Escape presses. Synthesising them needs accessibility permission, which the test
  should not depend on.

**Desktop session (owner, §1: "Full check requires it"):** these binary tests run at the full check level, as the Metal case does
today, and fail rather than skip when no desktop session is available.

**Owner demonstrations** (ck-guide's Proposal: milestone 3's acceptance), one per backend on this Mac:
1. A window opens with the configured clear colour.
2. Escape reaches the callback, which accepts: the window closes and the program exits 0.
3. The close button reaches the same callback. A demo setting declines the first request (and logs it), and accepts the next; the
   same for Escape.
4. Ctrl-C in the terminal quits cleanly, with exit status 130.
5. An error in the callback logs its traceback, shuts down cleanly and exits 1.
6. Cmd-Q does nothing on either backend, and the window cannot be resized.

**Later:** the Linux portability check in UTM, and testing on real Linux hardware (ck-guide's Proposals, §1).

## 11. Open decisions

### Owner questions

None open. Every earlier question is answered (§1, §1.1).

### Designer recommendations (mechanism; settled in issue specs and their reviews)

These change no visible behaviour. ck-manager recommends them, independent review checks them, and the owner can override any of them:
- the package split and raw-handle boundaries (§3);
- the run phase, the request slot, the event bounds, the work per pass and the notifier (§4);
- posted calls with an in-flight limit (§5);
- the signal thread, and the non-abandoning polls with one real phase deadline (§6.1);
- the render thread, and the main thread never calling a blocking GPU function (§4);
- the dependency teardown order, and keeping every object alive on a missed render-thread deadline (§7);
- the present-lifetime rules, which require `VK_EXT_swapchain_maintenance1`. That requirement narrows which Linux drivers the GLFW and
  Vulkan path runs on until a fallback exists, which matters for the owner's eventual Linux support (§8);
- the frame strategy, pixel-size changes, and the stated responsiveness limits (§8);
- the bindings: a minimal GLFW binding with GLFW 3.4 installed by `tools/bootstrap`, and `ash` for Vulkan (§9). Installing GLFW is a
  visible change to the machine's toolchain, which the owner should know about;
- the failure boundary and resource latch, and the Vulkan ledger (§7, §8);
- the window's owner VM is fixed as `ui` in the executable, not configurable: UI ownership is Firm (§1);
- the configuration key `[window] backend = "native-metal"` or `"glfw-vulkan"`, the override option `--backend NAME` and `--headless`.
  The names are for the issue spec.

## Provenance

Drafted by ck-manager (Claude Opus 5.5, Class S, high effort) on 2026-10-19, at ck-guide's request `5i4edh3ipubucjhh94udejxyws`, and revised
at its request `3pyxhtqyre3cjgap3ezpi7jzhn` (bounded traffic, signals across phases, dependency teardown and the GPU deadline, frame and
resize responsiveness, catching versus rollback, the narrowed Firm statement, and the open input scope), and again at its requests
`j7bt5cdy3mctjgm5h2uc3k3nge` (non-abandoning start-phase polls, the Vulkan present lifetime, and stated rather than asserted
responsiveness, which led to the render thread) and `qjgk4uyafwa6nias88q84cs8ia` (a cleanup result as well as thread completion, fixed
pool and retirement caps, acquired-image accounting, no absolute main-thread timing, Metal questions left to verify); and updated for the
owner's input decision relayed in `vku4eet4xgjkwucthvuw8wmmci` (windowed only, hardcoded Escape, other input deferred); and revised for
independent design review 1 (`.pipeline/coordination/runtime-m3/design-review-1.md`, ck-reviewer-1, CHANGES_REQUESTED: B1 GLFW pump, B2
failure boundary, B3 notifier lifetime, N1 Vulkan ledger, N2 present fence meaning, N3 selection default, N4 GLFW menu); and revised for
design review 2 (`design-review-2.md`, residual B1 native modal claim, stop-slicing and signal-thread lock consistency) together with the
owner's answers to every §11 question, given directly to ck-manager on 2026-10-19; and updated for the owner's headless decision, which
replaced "needed to run at all" the same day, from:
- the owner direction relayed in `dr38tadsfyntyhsvz64f6dajes`, `54va48x56xm97kdizwfp3s6fwi` and `5i4edh3ipubucjhh94udejxyws`;
- `docs/runtime-design.md` and `docs/vision.md`;
- the accepted repository state at pointer `fe2dccc2…`;
- this Mac's installed Vulkan loader and MoltenVK.

Handoff: `.pipeline/coordination/runtime-m3/handoff.md`.
