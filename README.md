# Codekarl

A 2D pixel-art economic grand-strategy game, and a reusable collection of Rust/Metal packages, with Pijul changes accepted
in batches. The game vision is being captured in an ongoing owner interview ([docs/vision.md](docs/vision.md), provisional).

Accepted so far, each milestone with its design (the [documentation index](docs/README.md) lists them all):

- **Milestones 1 and 2** ([docs/runtime-design.md](docs/runtime-design.md)): a runtime that loads its configuration, logs and shuts
  down cleanly, and runs two Lua VMs in parallel, `ui` and `gameplay`. There is also a Metal GPU probe.
- **Milestones 3 and 3.5** ([docs/window-design.md](docs/window-design.md)): the `ui` VM opens a native macOS window cleared by Metal
  (`--features metal`), or a GLFW window cleared by Vulkan (`--features vulkan`), and its Lua callback decides each close request.
- **Milestones 4 and 4.5** ([docs/render-design.md](docs/render-design.md)): a textured quad from a PNG, drawn through `ck-render-2d`
  by either backend.
- **Milestones 5 and 5.5** ([docs/scene-design.md](docs/scene-design.md)): textures loaded in the background through reusable
  handles, many ordered quads, configurable key bindings, a paced update, and tens of thousands of quads and distinct textures through
  a bindless texture table, on both backends
  ([docs/evidence/m55-scale-2026-11-01.md](docs/evidence/m55-scale-2026-11-01.md)).
- **Milestone 6** ([docs/map-design.md](docs/map-design.md)): the map of Ireland, written by the GPU from authored terrain data, with
  major rivers, a camera, and a preview tool that renders the reference colours offline.
- **Milestone 7** ([docs/places-time-design.md](docs/places-time-design.md)): the 768 scenario's places and their catchments, a game
  clock with pause and five speeds, reproducible saves (quicksave and quickload), mouse pan and zoom, the `gameplay` VM on the game
  clock, a pixel font, the date readout and place names on hover.
- **Milestone 8** ([docs/ui-design.md](docs/ui-design.md)): UI windows and panels in Lua. Map input never waits on the UI; there is a
  widget library, scene groups with clipping, a time-control bar, a places list, a place window opened by clicking a place, scroll
  lists and a themed look.
- **Milestone 9** ([docs/economy-design.md](docs/economy-design.md), [docs/map-design.md](docs/map-design.md) §12): the first economy.
  Each place has pops and buildings. Buildings produce, keep herds and stores, and pay wages in goods. Pops eat at noon each day,
  are born, die, are hired and move. Each place's prices move with its pops' and buildings' orders, though no goods change hands
  between them yet. Land has footprints, fertility and a computed climate. A location panel shows a place's
  economy, the map has a new look, and three map modes colour it by stockpile, expected yield or elevation. New render features
  land on native Metal only; their Vulkan port is parked (CK-239).

## Run it

From this directory:

```sh
tools/ck cargo run                                  # the runtime, built-in defaults
tools/ck cargo run --features metal -- --config config/game.toml      # the game (below)
tools/ck cargo run --features metal -- --config config/codekarl.toml  # the two Lua VMs and the demo window
tools/ck cargo run -- --config config/codekarl.toml --headless       # the same, headless (any build)
tools/ck cargo run --features vulkan -- --config config/demo-glfw-vulkan.toml  # the demo window through GLFW and Vulkan
tools/ck cargo run --features metal -- --config config/demo-quad.toml  # the textured-quad demo (Metal)
tools/ck cargo run --features vulkan -- --config config/demo-quad-vulkan.toml  # the textured-quad demo (Vulkan)
tools/ck cargo run --features metal -- --config config/demo-scene.toml  # the live-scene demo: loading tiles, a gem to move
tools/ck cargo run --features vulkan -- --config config/demo-scene.toml --backend glfw-vulkan  # the same demo on GLFW and Vulkan
tools/ck cargo run --features metal -- --metal-smoke
tools/ck cargo run --release --bin economy-soak -- --years 10   # the economy, headless, as CSV (below)
tools/ck check --changed # the groups your change affects (and why); the usual check, and what a batch runs
tools/ck check --quick   # fmt, Ruff, ShellCheck: while editing
tools/ck check           # full: every non-optional group, Metal and native windows included (once per milestone)
tools/ck status
```

The package has two programs, `codekarl` and `economy-soak`; `cargo run` runs `codekarl` (its `default-run`), and the other needs
`--bin economy-soak`.

The Metal probe compiles a shader during the Cargo build, dispatches it on the
GPU, and checks all 64 results in Rust. Full checks require the Metal compiler and
a working GPU; missing prerequisites fail the run rather than skip it.

### The game

`config/game.toml` runs the game: the native Metal window, the Ireland scenario `assets/scenarios/ireland-768`, and the UI and
gameplay scripts (paths in the file are relative to it). Run it with

```sh
tools/ck cargo run --features metal -- --config config/game.toml
```

Saves go to `saves/` at the top of the copy (`[game] saves = "../saves"`; without the key the default is `saves` beside the
configuration file). `/saves` is in `.ignore`, so Pijul does not see the saves and they never show as changes. The player's
settings file, `settings.toml`, is kept beside that directory and is in `.ignore` too (see below).

- **A new game** starts paused at the scenario's start date, with a master seed from `--seed N` or the operating system;
  `--load PATH` starts from a save instead.
- **Time:** Space pauses, `1` to `5` pick a speed, `]` and `[` go faster and slower, and the time bar's buttons do the same. F5
  quicksaves and F9 quickloads.
- **Saves:** the quicksave is `saves/quicksave.cksave`. A save loads only into the build that wrote it, and a save written before
  CK-302 (2026-11-14) has the older header and is refused. The game can now also write, list, load and delete named saves, one
  file each, and knows whether it has unsaved changes; the save/load screen that will use them is not built yet
  ([docs/places-time-design.md](docs/places-time-design.md) §5.3).
- **Keys** act only with exactly the modifiers their binding names (CK-315): with Shift, Control, Alt or Command held, a plain
  key such as `1`, an arrow or Escape does nothing. `+` (`Shift+=`) is bound, and zooms in as `=` does
  ([docs/scene-design.md](docs/scene-design.md) §10.1).
- **The menu:** Escape closes what is open, one thing a press: a tooltip, a list's picker, a window. With nothing left to
  close it opens the pause menu, a small window in the middle of the dimmed screen, and pauses the game. While the menu is open
  the mouse reaches nothing behind it, and the time, quicksave, quickload and map keys do nothing. "return", or Escape again,
  closes the menu and leaves the game paused: Space or the time bar's play button resumes it. "quit to desktop" ends the game at
  once without asking, and so does the window's own close button. Neither saves, and F5 does nothing while the menu is open, so
  quicksave before you open it. "settings", "glossary" and "save / load" are greyed until those screens are built
  ([docs/ui-design.md](docs/ui-design.md) §3.15).
- **The map:** drag to pan and scroll to zoom at the cursor, or use the arrow keys and `=` and `-`. Click a place to show it in the
  location panel (tabs: location, pops and buildings). The two icons at the top right open the ledger, a list on the right of the
  screen with a locations tab and a pops tab; clicking a row shows that place in the panel. The three buttons centred at the bottom of
  the screen switch the map mode (stockpile, expected yield, elevation); pressing the one showing turns it off.
- **Tooltips:** rest the pointer on a place for 0.3 seconds to see its tooltip: its name, population, majority culture and
  religion, usable land, and the value produced and consumed last month. Icon buttons, shortened tabs, the prices in the location
  tab and herd icons have label tooltips too, and in the ledger, the location tab and the pop and building cards a word shows its
  explanation and a number its breakdown. Keep it there for another second and the tooltip locks
  (an orange bar fills along its bottom edge): it then stays until you press Escape or click away from it, and a click on a button
  or other control leaves it open. In the locked tooltip, rest on a number for its breakdown or on a light-blue word for its
  explanation. `[ui.tooltips]` in the configuration sets `delay` (0 to 2 seconds), `lock` (`"timer"`,
  `"middle_mouse"` or `"disabled"`) and `lock_timer` (0.3 to 3 seconds)
  ([docs/ui-design.md](docs/ui-design.md) §3.13), and the settings file can override them.
- **The settings file** (CK-298): `settings.toml` holds the player's own settings and overrides the configuration file. It may
  hold `[ui.tooltips]` `delay`, `lock` and `lock_timer`, and `[map]` `pan_pixels_per_frame`, `zoom_steps_per_second` and
  `scroll_points_per_step`, each checked as the configuration checks it. A missing file is normal. An entry that is not accepted
  is named in a `warn` record and ignored, and the rest applies. The game's settings calls write it, but no settings window
  exists yet, so today it holds only what is put there by hand ([docs/ui-design.md](docs/ui-design.md) §3.17).
- **Lists:** a list longer than its view has a thin scroll bar at its right edge: drag its thumb, or press the track to move one
  view ([docs/ui-design.md](docs/ui-design.md) §3.16).
- **The window** opens at 1280×800 screen pixels, its minimum. Drag an edge to enlarge it (the game lays itself out again when you
  let go), or use macOS full screen: the green title-bar button or Control-Command-F. The UI scale follows the window: 1 from
  1280×800 screen pixels, 1.5 from 1920×1200 and 2 from 2560×1600, so a UI pixel is 2, 3 or 4 screen pixels. `[ui] scale = 1`,
  `1.5` or `2` caps it (the lower of the two is used; read at startup), and the old `[ui] points_per_pixel` is refused, naming `scale`.
- **Run the game on native Metal.** Milestone 9's render features (the new map look and the elevation mode) exist on native Metal
  only. Since CK-238 the Vulkan backend is only kept compiling, and its port of them (CK-239) is parked until the owner says so.

`economy-soak [--scenario DIR] [--years N] [--seed N]` runs the economy without a window from the 768 scenario (the default) and
prints one CSV row per pop per month: workers, dependents, food, births and deaths, employment, movement, crowding, prices,
production, stores, herds, posted wages and losses. It is a tuning tool, outside the checks.

## The runtime command

`codekarl` starts the runtime, logs, and shuts down. With no arguments it uses built-in defaults; `codekarl --config PATH` reads a
TOML file instead (only that path; no search of the working directory or the environment). Since CK-298 a run with `--config`
also reads the settings file, `settings.toml` in the parent directory of that configuration's saves directory, when one is
there ([The game](#the-game)); a run without `--config` reads none. `--help`, `--version` and `--metal-smoke` do not start the
runtime.

```
codekarl [--config PATH] [--headless | --backend NAME] [--load PATH | --seed N] | --help | --version | --metal-smoke
```

`--load` and `--seed` need a configuration with a `[game] scenario` ([The game](#the-game)). The tables below are the core ones. The
others (`[map]`, `[game]`, `[ui]`, `[render]`, `[resources]`, `[frame]`, `[input.bindings]`) are defined in
`apps/codekarl/src/config.rs`, and the scene's settings are listed in [docs/scene-design.md](docs/scene-design.md) §7.1.

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

Every key is optional, and an empty file equals the defaults. An unknown key, a wrong type or an invalid value is an error. (The
settings file is read more leniently: a bad entry there is named and ignored.)

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
  Lua error); `window.set_clear(r, g, b)` changes the colour; `window.close()` closes it. Since milestone 9, `window.open` also takes
  `unit` (`"points"`, the default, or `"pixels"`), `resizable` and, for a resizable window, `min_width` with `min_height`. It returns
  the size the script lays out in: points for a window opened in points, UI units at the window's UI scale for one opened in pixels.
- `window.on_opened(fn)` runs once the first frame has been queued for presentation; this does not prove it is visible.
  `window.on_close_request(fn)` gets `"key"` (Escape) or `"button"` (the close button) and closes the window when it returns `true`.
  With no callback registered, a close request closes the window.
- **Closing:** the window closes first, then both VMs run `shutdown()`. An error in a window callback is logged with its traceback,
  the window closes, the VMs shut down, and the exit status is 1.
- **Size and menu:** a window has a fixed size unless it is opened with `resizable = true`, as the game's is (see "The game" above).
  While its edge is dragged, the newly uncovered margin shows the clear colour the window was created with: a later
  `window.set_clear` does not reach it (a known limit with a follow-up; the shipped game never changes the colour).
  There is no menu, so Cmd-Q does nothing; Ctrl-C and SIGTERM quit as above.
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
percentile, but at first took about 155 ms of GPU time per frame, about 250 times Metal's. CK-168 traced that to MoltenVK's Metal
resource allocation ([docs/evidence/m55-vulkan-gpu-gap-2026-11-02.md](docs/evidence/m55-vulkan-gpu-gap-2026-11-02.md)): with
MoltenVK allocating from Metal heaps the same scene took about 0.75 ms, and since CK-175 the Vulkan instance asks MoltenVK for heaps
unless `MVK_CONFIG_USE_MTLHEAP` is set in the environment. Whether native Linux Vulkan behaves the same is unmeasured.
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
| `apps/codekarl` | Executable: arguments, configuration, logging set-up, runtime and Lua VMs, the window, map and game composition, the Lua APIs, exit status; also the `economy-soak` tool |
| `crates/lua` | `ck-lua`: one Lua 5.4 state per VM owner thread, deadlines, cancellation and its instruction hook, `log` and `config` |
| `crates/log` | `ck-log`: lossless structured logging to stdout, with bounded calls for the control path |
| `crates/runtime` | `ck-runtime`: starts services in order and supervises thread owners with deadlines, stops in reverse, reports the outcome |
| `crates/window`, `crates/window-cocoa`, `crates/window-glfw` | `ck-window`: the platform-free window loop; its native macOS (`AppKit`) and GLFW backends |
| `crates/input` | `ck-input`: portable key names, validated action bindings and the bounded input mapper |
| `crates/resources` | `ck-resources`: PNG decoding under budgets, the assets rule and the background resource service |
| `crates/render-2d` | `ck-render-2d`: the only drawing API the rest of the project sees: the scene bridge, the ordered scene, the map input and map modes it hands the GPU crates' map passes, and the Metal and Vulkan backends |
| `crates/gpu-metal` | Native Metal ownership boundary and build-time shader compilation |
| `crates/gpu-metal/native` | C header and Objective-C/ARC implementation |
| `crates/gpu-metal/shaders` | Metal Shading Language source |
| `crates/gpu-vulkan` | `ck-gpu-vulkan`: the Vulkan presenter, texture store and scene drawing (compile-only since CK-238) |
| `crates/map` | `ck-map`: map coordinates, camera math, loading and validating the map and scenario files, the reference colours, and the `map-preview` tool |
| `crates/sim` | `ck-sim`: the calendar, the clock, the tick scheduler, commands, random streams and saves |
| `crates/economy` | `ck-economy`: the economy's data, starting state and daily and monthly rules, independent of maps, windows, Lua and renderers |
| `config`, `scripts` | The shipped configurations, and the Lua scripts: the `ui` VM's game UI and widget library (`scripts/ui/lib`), the demos and the `gameplay` skeleton |
| `assets` | The map of Ireland, the 768 scenario (places and economy), the economy rules, the UI font and theme, and the demo images |
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
edges. Each crate above was added when a real system needed it; later ones, such as `render-3d`, are options to add the same way,
not an approved package roadmap. The GPU crates do not depend on `ck-render-2d`, and the simulation and economy crates stay
independent of rendering, windows and Lua. The executable is the composition point. No empty placeholder crates are kept.

## Documentation

The [documentation index](docs/README.md) lists every document: the current guides and contracts, one design per milestone, and
the completed records.

| Document | Status |
| --- | --- |
| [docs/tooling.md](docs/tooling.md) | Current: workflow, check levels, GC, recovery, Metal and Cargo |
| [docs/agent-workflow.md](docs/agent-workflow.md) | Current: roles, model classes, sessions |
| [docs/chat-workflow.md](docs/chat-workflow.md) | Current: chat contract; dated rollout sections are historical |
| [docs/review-gate-plan.md](docs/review-gate-plan.md) | Current specification of the installed review gate |
| [docs/vision.md](docs/vision.md), [docs/game/](docs/game/) | Provisional: the game vision and its systems |
| Milestone designs | From [docs/runtime-design.md](docs/runtime-design.md) (milestones 1 and 2) to [docs/economy-design.md](docs/economy-design.md) (milestone 9); see the index |
| [docs/archive/](docs/archive/) | Completed records: the setup hardening, the 2026-10-15 pilot findings, the 2026-10-14 identity incident and its reviews |

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
