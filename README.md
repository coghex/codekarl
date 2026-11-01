# Codekarl

A 2D pixel-art economic grand-strategy game, and a reusable collection of Rust/Metal packages, with Pijul changes accepted
in batches. The game vision is being captured in an ongoing owner interview ([docs/vision.md](docs/vision.md), provisional).
Today there is a runtime that starts, loads its configuration, logs, and shuts down cleanly (milestone 1). With a configuration it also
runs two Lua VMs in parallel, `ui` and `gameplay`, each a printing skeleton (milestone 2). There is also a GPU probe.
Its design is [docs/runtime-design.md](docs/runtime-design.md). Milestone 3's native path is accepted
([docs/window-design.md](docs/window-design.md)): built with `--features metal`, the `ui` VM opens a native macOS window cleared
by Metal, and Escape or the close button ask its Lua callback whether to close. Milestone 3.5's GLFW and Vulkan path is accepted
too: built with `--features vulkan`, the same scripts open a GLFW window cleared by Vulkan (MoltenVK on this Mac). Milestone 4 is
accepted ([docs/render-design.md](docs/render-design.md)): the `ui` VM places a textured quad from a PNG in `assets/` with
`render.quad`, drawn by Metal through `ck-render-2d`. Milestone 4.5 is accepted too: the same quad is drawn by Vulkan on the GLFW
path, and the two backends are interchangeable to the rest of the project. Milestone 5 is accepted too
([docs/scene-design.md](docs/scene-design.md)): on the native Metal path the `ui` VM loads textures in the background through
reusable handles, places and moves many quads, reads configurable key bindings every update, and draws tens of thousands of quads
and distinct textures through a bindless texture table. Milestone 5.5 is accepted too: the live scene, its key bindings, the
update pacing and the bindless texture table run on `glfw-vulkan` as well, and the same scale was measured there
([docs/evidence/m55-scale-2026-11-01.md](docs/evidence/m55-scale-2026-11-01.md)).

## Run it

From this directory:

```sh
tools/ck cargo run                                  # the runtime, built-in defaults
tools/ck cargo run --features metal -- --config config/codekarl.toml  # the two Lua VMs and the demo window
tools/ck cargo run -- --config config/codekarl.toml --headless       # the same, headless (any build)
tools/ck cargo run --features vulkan -- --config config/demo-glfw-vulkan.toml  # the demo window through GLFW and Vulkan
tools/ck cargo run --features metal -- --config config/demo-quad.toml  # the textured-quad demo (Metal)
tools/ck cargo run --features vulkan -- --config config/demo-quad-vulkan.toml  # the textured-quad demo (Vulkan)
tools/ck cargo run --features metal -- --config config/demo-scene.toml  # the live-scene demo: loading tiles, a gem to move
tools/ck cargo run --features vulkan -- --config config/demo-scene.toml --backend glfw-vulkan  # the same demo on GLFW and Vulkan
tools/ck cargo run --features metal -- --metal-smoke
tools/ck check --changed # the groups your change affects (and why); the usual check
tools/ck check --quick   # fmt, Ruff, ShellCheck: while editing
tools/ck check           # full: every group, Metal and native windows included (the batch runs this)
tools/ck status
```

The Metal probe compiles a shader during the Cargo build, dispatches it on the
GPU, and checks all 64 results in Rust. Full checks require the Metal compiler and
a working GPU; missing prerequisites fail the run rather than skip it.

## The runtime command

`codekarl` starts the runtime, logs, and shuts down. With no arguments it uses built-in defaults; `codekarl --config PATH` reads a
TOML file instead (only that path; no search of the working directory or the environment). `--help`, `--version` and
`--metal-smoke` do not start the runtime.

```
codekarl [--config PATH] [--headless | --backend NAME] | --help | --version | --metal-smoke
```

```toml
[log]
level = "info"          # error | warn | info | debug | trace (default info)
timestamps = true       # default true
colour = "auto"         # auto | always | never (default auto)

[log.categories]        # optional per-category levels
runtime = "debug"

[window]                # optional
backend = "native-metal"  # native-metal | glfw-vulkan; without it the run is headless
```

Every key is optional, and an empty file equals the defaults. An unknown key, a wrong type or an invalid value is an error.

- **Windowed or headless:** a run is headless with no `--config`, with no `[window]` table or one without `backend`, or with
  `--headless`. Otherwise it is windowed with `--backend NAME` if given (it needs a configured backend to override), else the
  configured backend. `--headless` and `--backend` cannot be combined. The start-up log names the choice in a `window:` record.
  `native-metal` is available in builds with `--features metal` on macOS, and `glfw-vulkan` in builds with `--features vulkan`
  (both features may be on; `--backend` then picks either over the configured one). A windowed run with an unavailable backend stops with `backend <name> is not available in this build` (exit 1).
- **Ctrl-C and SIGTERM:** the first asks for a clean quit and is logged (`received SIGINT, quitting`); a second ends the process at
  once with exit 3.

- **Output:** everything goes to stdout, and stderr is redirected to the same place at start-up. Each record is one line:
  timestamp, level, category and message. With `colour = "auto"` the level is coloured only when stdout is a terminal and `NO_COLOR`
  is unset.
- **No lost lines:** ordinary logging waits for room rather than dropping a line. Only the start-up and shutdown path logs with a
  short time limit, so a stuck stdout cannot stop the process from ending.
  - A progress line lost that way is counted and makes the exit status 3.
  - Error messages are kept in a reserve instead. At exit they get up to 2 extra seconds to be written, but only when one is still
    pending.
- **Exit status:**
  - **0:** clean;
  - **1:** usage, configuration or runtime failure, for example a Lua VM failing to start or a window callback raising an error;
  - **130 or 143:** otherwise clean, but SIGINT (130) or SIGTERM (143) asked for the quit;
  - **3:** incomplete: the log was not fully written, a Lua VM missed its deadline, a VM thread never finished, the window and GPU
    teardown could not be proven complete, or a second signal arrived. The process then ends at once, without running exit handlers;
  - **4:** stderr could not be redirected.

  When several apply, 3 wins over 1, and 1 over 130 or 143.

### Lua VMs (milestone 2)

`config/codekarl.toml` runs two VMs, `ui` and `gameplay`, from `scripts/ui/main.lua` and `scripts/gameplay/main.lua`:

```toml
[runtime]                 # owner deadlines in seconds, > 0 and <= 60 (these are the defaults)
start_timeout = 5
stop_timeout = 5
cleanup_timeout = 1

[lua.ui]                  # one table per VM; entry is relative to this file
entry = "../scripts/ui/main.lua"

[lua.gameplay]
entry = "../scripts/gameplay/main.lua"
```

- **How they run:** each VM runs Lua 5.4 (bundled through `mlua`) on its own thread, with its own state. VMs start in name order and
  stop in reverse. Without `--config` no VM runs.
- **What scripts get:**
  - standard `print`;
  - `log.error/warn/info/debug/trace(message)`, in category `lua.<name>`;
  - a read-only `config` table holding the whole configuration;
  - optional global `startup()` and `shutdown()`.
- **Failures:** if any VM fails to start, every VM is cancelled and the exit status is 1.
- **Runaway scripts:** a script that loops forever is interrupted when its VM is cancelled or misses a deadline. `pcall` cannot catch
  that interruption. A loop inside a `__gc` finalizer, inside a `__close` handler in the interrupted function itself, or inside a
  blocked `print` cannot be interrupted; it ends as exit status 3.

### The window (milestone 3)

The VM named `ui` gets a `window` table; no other VM does:

- `window.open{title=, width=, height=, clear={r, g, b}}` opens the one window (a second `open`, or an invalid size or colour, is a
  Lua error); `window.set_clear(r, g, b)` changes the colour; `window.close()` closes it.
- `window.on_opened(fn)` runs once the first frame has been queued for presentation; this does not prove it is visible.
  `window.on_close_request(fn)` gets `"key"` (Escape) or `"button"` (the close button) and closes the window when it returns `true`.
  With no callback registered, a close request closes the window.
- **Closing:** the window closes first, then both VMs run `shutdown()`. An error in a window callback is logged with its traceback,
  the window closes, the VMs shut down, and the exit status is 1.
- **Fixed behaviour:** the window has a fixed size and no menu, so Cmd-Q does nothing; Ctrl-C and SIGTERM quit as above.
- **Headless:** the same calls are checked the same way, but a valid `window.open` opens nothing and logs one
  `window.open ignored: headless run` record, so a script runs unchanged either way.

Two demonstration configurations go with `config/codekarl.toml` (all three need `--features metal`):
`config/demo-decline.toml` declines the first close request and accepts the next, and `config/demo-callback-error.toml` raises an
error in its close callback (exit 1 with the traceback). The same three exist for GLFW and Vulkan, built with `--features vulkan`:
`config/demo-glfw-vulkan.toml`, `config/demo-glfw-vulkan-decline.toml` and `config/demo-glfw-vulkan-callback-error.toml`. They
behave the same way; the Vulkan library is loaded by its absolute path, so no `DYLD_*` variable is needed.

### The textured quad (milestones 4 and 4.5)

The VM named `ui` also gets a `render` table; no other VM does:

- `render.quad{image = "name.png", x =, y =, width =, height =}` decodes a PNG from the assets directory and places one quad,
  replacing any earlier one. Positions and sizes are window points from the top-left; `x` and `y` default to 0, and `width` and
  `height` to the image's size (one image pixel per point). Transparent pixels blend over the clear colour. A missing or broken file,
  a name outside the assets directory, or a bad number is an ordinary Lua error, and a failed call changes nothing.
  `render.clear_quad()` removes the quad. This quad does not move; milestone 5's handle-based quads (below) do.
- **Settings:** an optional `[render]` table, with `filter = "nearest"` (the default) or `"linear"`, and `assets = "<dir>"` relative
  to the configuration file (default `assets`). The assets directory is looked up at the first `render.quad`, so configurations
  without one work as before.
- **Images:** PNG only, at most 8192 pixels per side and 64 MiB decoded (for example 4096 × 4096).
- **Backends:** `native-metal` and `glfw-vulkan` both draw the quad, with the same result. This Mac's MoltenVK offers a UNORM
  surface format; on a Vulkan surface that offers none, the quad is skipped with one `textured quad skipped` warning and the window
  keeps clearing.
  Headless runs check the call the same way and log one `scene calls ignored: headless run` record.

`config/demo-quad.toml` (with `--features metal`) runs `scripts/ui/demo_quad.lua`, which places `assets/demo-quad.png`, a small
pixel-art placeholder with transparent areas, scaled up with nearest sampling. `config/demo-quad-vulkan.toml` (with
`--features vulkan`) runs the same script and image on `glfw-vulkan`. Replace that file to show your own image.

### The live scene (milestone 5)

On `native-metal` and `glfw-vulkan` the `ui` VM also gets `texture` and `frame` tables and handle-based quads:

- `texture.load("name.png")` returns a texture handle at once; the file is read, decoded and uploaded in the background. Until
  its pixels arrive, and for good if it cannot load, its quads show a magenta checkerboard. `texture.status(t)` reports its state
  (`pending`, `decoding`, `uploading`, `cpu_ready`, `ready` or `failed`, with any error) without waiting. `texture.reload(t)`
  reads the file again (there is no file watching); an unsuccessful replacement keeps the last good pixels.
  `texture.release(t)` gives the handle up; existing quads keep their retained texture.
- `render.create_quad{texture =, x =, y =, width =, height =}` returns a quad handle. Quads draw in creation order, later ones on
  top. `render.set_position(q, x, y)`, `render.set_rect(q, x, y, width, height)` and `render.destroy_quad(q)` change or remove
  one. Milestone 4's `render.quad` still works and draws below them.
- `frame.on_update(function(f) ... end)` targets `[frame] updates_per_second` updates a second (default 60) while the window
  is focused, is not minimized and has a nonzero drawable size. Missed updates coalesce; at most one callback is outstanding.
  The callback gets `f.dt` and `f.actions`, where each action has `held`, `pressed` and `released`. `frame.on_update(nil)` stops it.
- **Settings:** `[input.bindings]` maps action names to physical keys; Escape closing the window is now the `close` binding.
  `[resources]` and `[render]` set the capacities: textures, extra revisions, memory budgets, `max_quads` and
  `bindless_texture_slots`. Immediate admission refusals are ordinary Lua errors; later load or upload failures appear in
  `texture.status(t)`. The keys and defaults are in
  [docs/scene-design.md](docs/scene-design.md) §7.1.
- **Headless** runs validate and ignore the window and scene calls; a texture can reach `cpu_ready` there, never `ready`.
- **`glfw-vulkan`** runs all of this too (milestone 5.5), with the same script and settings, including the bindless table
  (`bindless_texture_slots`) and `diagnostics.metrics_mark`. A device without the descriptor-indexing support the table needs
  draws with ordinary per-texture bindings instead, and the log says so once.

`config/demo-scene.toml` runs `scripts/ui/demo_scene.lua`. Twelve tiles load two per update, each a checkerboard until its image
arrives; one names a missing file, so its checkerboard stays. The arrow keys or WASD move the gem, R reloads every texture, and
Escape closes the window. It looks and behaves the same on `native-metal` and on `glfw-vulkan`.

Measured on `native-metal` on this Mac (Apple M3 Max, release build) with 65,536 quads using 32,768 distinct textures in one
bindless draw: 8.25 ms of CPU time per frame at the 95th percentile, just over the 8 ms target and accepted by the owner, and
0.6 ms of GPU time. With textures constantly reloaded and recreated it is 7.35 ms, and loading runs at about 7,700 textures a
second.
On `glfw-vulkan` (MoltenVK on the same Mac) the same scene draws correctly with 15 ms of CPU time per frame at the 95th
percentile, but takes about 155 ms of GPU time per frame, about 250 times Metal's, so it runs at about 6 frames a second; a 248-image scene matches Metal. The cause is
being investigated (CK-168), including whether it is specific to MoltenVK or would also affect Linux.
Counter-level profiling is done with external tools (Instruments, the Metal debugger), not engine counters.

`tools/ck` supplies the project tool environment. It selects Xcode using
`DEVELOPER_DIR`, without changing the machine's `xcode-select` setting, and uses
the pinned Rust toolchain. Plain `cargo` may resolve to the existing Homebrew
installation instead. No shell configuration has been changed.

## Work in parallel, accept together

Create a worker from the accepted checkpoint:

```sh
cd ~/work/codekarl
tools/ck worker economy
cd .pipeline/workers/economy
```

Every change belongs to an issue (`CK-N`) whose specification has an approved
independent review. Edit code and its specs in that working copy, then:

```sh
tools/ck pijul add --recursive apps crates specs tests   # new files only
tools/ck check --changed
tools/ck record -m "Describe the change" --issue CK-N
tools/ck submit --issue CK-N --spec "Required behavior; specs/tests that establish it"
```

`ck record` adds session-verified provenance; a raw `pijul record` is refused.
Each submission needs a fresh independent review of its exact manifest; a fix is
a new submission with `--supersedes ID`. Other workers can submit from their own
copies at any time. From the main project directory the manager then accepts
reviewed work in three steps:

```sh
cd ~/work/codekarl
tools/ck batch prepare            # freeze eligible submissions, run full CI once
tools/ck batch status BID         # publication_basis: single-submission or integration-review
tools/ck review batch BID ...     # independent integration review, unless the basis is single-submission
tools/ck batch publish BID        # recheck everything under locks, move the pointer
tools/ck sync
```

Prepare assembles the reviewed changes on the latest accepted checkpoint and
runs the checks **once on the combined state**. Nothing publishes on CI alone.
Later submissions wait for the next batch. There is no CI daemon or per-patch
fast-forward requirement. `tools/ck accept` has been removed.

The approved Pijul record hook runs only formatting and Python lint checks. A
manual `tools/ck check` holds no project lock; only batch validation holds the
project CI lock. Each step has a 45-minute watchdog (exit 75 means a step
outlived it: an infrastructure failure, not a patch failure). This does not
coordinate the CI services of other projects on the same Mac.

If a batch fails, the command splits independent submission groups and tests the
survivors against the latest accepted checkpoint. Dependent submissions stay
together. Failing groups are quarantined, so unrelated work can proceed. See
[the workflow notes](docs/tooling.md) for diagnosis and resubmission.

The authority is `.pipeline/accepted-v2.json`, pointing at an immutable-by-convention
checkpoint repository. The top-level source directory is an editable working
copy. Never edit a checkpoint or submission capture directly; integrity checks
reject changed content.

## Workspace layout

| Location | Responsibility |
| --- | --- |
| `apps/codekarl` | Executable: arguments, configuration, logging set-up, runtime and Lua VMs, exit status |
| `crates/lua` | `ck-lua`: one Lua 5.4 state per VM owner thread, deadlines, cancellation and its instruction hook, `log` and `config` |
| `crates/log` | `ck-log`: lossless structured logging to stdout, with bounded calls for the control path |
| `crates/runtime` | `ck-runtime`: starts services in order and supervises thread owners with deadlines, stops in reverse, reports the outcome |
| `config`, `scripts` | The shipped configuration and the two skeleton Lua scripts |
| `crates/gpu-metal` | Native Metal ownership boundary and build-time shader compilation |
| `crates/gpu-metal/native` | C header and Objective-C/ARC implementation |
| `crates/gpu-metal/shaders` | Metal Shading Language source |
| `specs/bootstrap.json` | Executable CLI acceptance contract |
| `apps/codekarl/tests` | Rust CLI spec runner over `specs/bootstrap.json`, and runtime-command tests |
| `crates/gpu-metal/tests` | Supervised native GPU test with Metal validation layers |
| `tests` | Batch workflow specs using real temporary Pijul repositories |
| `tools/check.py` | Single validation recipe (quick, fast, full) used locally and by the gate |
| `tools/gate` | Issues, reviews, batches, publication, checkpoint leases and `ck gc` |
| `tools/pipeline.py` | Immutable captures, fingerprints and Pijul helpers |
| `tools/launcher.py` | Typed checkpoint launcher that `tools/ck` runs, after origin checks |
| `tools/chat` | Codekarl chat adapter, `ck-chat` code and installer |
| `.cargo/config.toml` | Forces the macOS deployment target to 26.0 (current Mac only) |

Cargo workspaces support separately compiled packages with explicit dependency
edges. Crates such as `core`, `resources`, `render-2d` and later `render-3d` are
options to add as actual systems appear, not an approved package roadmap. Logging and the
runtime are the first separate packages ([docs/runtime-design.md](docs/runtime-design.md)); further
splits, such as a resource manager, remain open (see [docs/vision.md](docs/vision.md)). Both renderers
can depend on `gpu-metal`; the shared GPU layer does not depend on either renderer. Simulation code should remain independent of
rendering. The executable is the composition point. No empty placeholder crates
or renderer interfaces are needed yet.

## Documentation

The [documentation index](docs/README.md) separates current guides and contracts from completed records.

| Document | Status |
| --- | --- |
| [docs/tooling.md](docs/tooling.md) | Current: workflow, check levels, GC, recovery, Metal and Cargo |
| [docs/agent-workflow.md](docs/agent-workflow.md) | Current: roles, model classes, sessions |
| [docs/chat-workflow.md](docs/chat-workflow.md) | Current: chat contract; dated rollout sections are historical |
| [docs/review-gate-plan.md](docs/review-gate-plan.md) | Current specification of the installed review gate |
| [docs/archive/setup-hardening-plan.md](docs/archive/setup-hardening-plan.md) | Completed record of the hardening slices (all accepted) |
| [docs/archive/live-pilot-findings.md](docs/archive/live-pilot-findings.md) | Closed findings ledger of the 2026-10-15 pilot |
| [docs/archive/chat-identity-incident.md](docs/archive/chat-identity-incident.md), [docs/archive/reviews/](docs/archive/reviews/) | Historical records (2026-10-14) |

The `ck-workflow-ready` setup goal is complete (2026-10-17). The owner's deliberate deferrals are listed in the hardening
record's §14; they are known limitations, not open work.

Ordinary docs bypass CI and review:

```sh
tools/ck docs -m "Explain the new behavior" README.md docs/tooling.md
```

Pass only files you edited. Leave new docs untracked; this command records them.
It publishes only README.md and ordinary Markdown under docs/, verifies that
code is unchanged, then updates those files in the working copy while preserving
unfinished code edits. Sync before editing docs if the copy is behind accepted
history. Executable Markdown, agent instructions, and documents consumed by tests
belong outside docs/ and go through the code gate.

This is the local Pijul counterpart to the owner's Git `docs-push` policy.

## Installed tools and rebuilding the setup

- Rust **1.91.1**, including rustfmt, Clippy and rust-analyzer, via rustup.
- Pijul **1.0.0-beta.24** in `~/.local/bin`.
- Ruff **0.14.0**, ShellCheck **0.11.0** and mypy **2.4.0** in the project-local
  `.tools/python` environment; `tools/ck doctor` fails a check when they differ.
- Python **3.11+**; the setup was verified with **3.14.8**.
- Full Xcode at `/Applications/Xcode.app`, including its Metal Toolchain.
- For `--features vulkan`: GLFW **3.4 or newer 3.x** (Homebrew `glfw`, installed by `tools/bootstrap`; verified with 3.5.1), and the
  Vulkan loader and Khronos validation layer from the LunarG Vulkan SDK for macOS (or Homebrew `vulkan-loader` and
  `vulkan-validationlayers`). The `doctor.graphics` check names whatever is missing and how to install it.

`tools/bootstrap` reinstalls the pinned command-line dependencies when needed;
`tools/ck doctor` reports the actual tool versions and verifies compiler access.
Install and complete Xcode's first-run setup separately. Its Metal component can
be installed with:

```sh
DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer xcodebuild -downloadComponent MetalToolchain
```

The application uses `serde` and `toml` for configuration and `libc` for platform
calls. `ck-log` has no third-party dependencies; `ck-runtime` depends only on
`ck-log`. The CLI spec test uses `serde_json` as a dev-dependency of
`apps/codekarl` only.
`Cargo.lock` is tracked, and acceptance uses `--locked`.

Back up two directories: the whole project `~/work/codekarl` (including `.pijul`,
`.pipeline` and `.tools/signing`, which holds the local signing key) and its sibling
metadata repository `~/work/codekarl-meta`, which holds the issues, the immutable
specs and the review receipts. Neither is usable without the other. Ignore patterns
affect version control, not Time Machine. On 2026-10-17 `tmutil isexcluded` reported
both directories as included in Time Machine's selection. That describes the selection
only; it does not assert that a backup has completed.

## License

All rights reserved; see [LICENSE](LICENSE).
