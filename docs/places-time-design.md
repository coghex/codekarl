# Places and the game clock: design (r4)

**Status: milestone 7 design (r4, 2026-11-06, by ck-designer-1, Class S, `claude-opus-5-5`, high); milestone 7 is accepted.** Later
milestones changed parts of it, noted where they apply ("Since M9"); the M8 time bar and its controls are in `docs/ui-design.md`. §2
describes the code as it was before M7.
- r4 answers design review 1 (`design-review-1-reviewer-1.md`, CHANGES_REQUESTED on r3) and ck-reviewer-1's CK-191 questions
  (`nzrrvmwpfkenxvkqvb43j5wsqw`). It changes no owner decision and no slice:
  - **F1**, the pointer ring (§6);
  - **F2**, the readout wakes on any displayed change (§4.3);
  - **F3**, the gameplay VM's configuration surface, and the gameplay scripts in the save's compatibility inputs (§5.3, §5.5);
  - **N1**, the middle dot and unsupported characters (§7);
  - the exact catchment arithmetic and the kinds-table validation (§3.1, §3.2).
- r3 answers ck-reviewer-1's source questions (`k497jffi9nm2mkzvf5ethr67i6`) and fixes the river draw order. It changes no owner decision
  and no slice:
  - one boundary order, with the commands' place in it (§4.1, §4.2, §4.3);
  - the deterministic gameplay profile closes every environmental path (§5.5);
  - GLFW scroll units (§6);
  - rivers over everything (§3.3, map-design §3.4 Firm).
- r2 records the owner's six answers (`owner-answers-r1.md`) as Firm and proposes the milestone 7 slices (§9).
- Changes from r1: the start date and scenario (§3.1, §4.1), the mouse gestures (§6), the text scope (§7), the gameplay VM's rules
  (§4.3, §5.5) and the slice table (§9).

**Firm** marks owner decisions, with their date and source. **Proposal** marks designer recommendations. **Open** marks decisions still
unmade. Nothing here is an issue spec.

This extends `docs/vision.md` ("Time", "Player, tags and territory"), `docs/map-design.md` (§3.4, §5.5, §7, §9.9) and
`docs/runtime-design.md` (§5, the gameplay VM). It sets up `docs/game/economy.md` without designing any of it.

## 1. Owner direction

- **Firm (owner, 2026-11-06, relayed by ck-manager).** Milestone 7 is "Places + game clock": Ireland's towns and cities as places on the
  map, with the hourly game clock, pause and speed, and reproducible saves underneath, setting up the economy next. Mouse drag and zoom may
  be a small first slice.
- **Firm (owner, 2026-11-06, answers to `owner-questions-r1.md`, recorded in `owner-answers-r1.md`):**
  1. **The start date is 768.**
  2. **Reproducible means the same build on any supported machine and backend.**
  3. **The speeds are as proposed:** pause plus 1, 6, 24, 72 and 168 game hours per real second. Space pauses, and the number keys pick
     a speed. The simulation slows down rather than skipping.
  4. **Text is a pixel-font date and speed readout, with place names in a hover tooltip.**
  5. **Every scroll, wheel or trackpad, zooms at the cursor; left-drag pans.** (The owner chose (b), not the designer's recommendation.)
  6. **The gameplay Lua VM joins the clock now:** deterministic, and logging only in M7.
- **Firm (vision, "Time").**
  - The logical tick is hourly. The economy and other systems run at coarser cadences, such as daily.
  - In single-player the player can pause freely, issue orders while paused and choose speeds.
  - In multiplayer, time controls are left to server settings chosen at game creation.
  - Reproducible saves: loading the same save and repeating the same actions gives the same outcomes, random events and battles
    included. The owner added: "at some point i may make decisions that change this".
- **Firm (vision).** Major towns and cities are simulated individually. Every building belongs to a place on the map.
- **Firm (map-design §5.5).**
  - Every location is placed by hand, and locations have no owner.
  - The skeleton comes first (towns, castles, abbeys, major settlements), then farms.
  - A location covers one or a few cells and draws resources from the cells within a radius. Where radii overlap, the nearest location
    takes the cell, and land outside every radius belongs to no location.
- **Firm (map-design §3.4).**
  - Zoomed out, the map is geographic only. Closer in, places are markers; closer still, they become houses.
  - Houses are decoration, scaled by size. Notable buildings are always drawn.
  - How locations look is deferred by the owner; generated houses are "fine for now".
- **Firm (map-design §5.1).** No procedural generation for gameplay-critical objects. Decoration may be generated.
- **Firm (map-design §4, §9.7).** 60 frames per second while the map moves; idle otherwise.
- **Firm (owner, 2026-11-05).** Window tests may take focus.

## 2. What exists (accepted, milestone 6)

- **`ck-map`** loads and validates the map directory, owns the coordinates and the reference colour function, and prepares river pixels
  once per zoom step, selecting the visible ones per view.
- **The map lane** (`ck-render-2d`) carries the packed grids and a camera snapshot with the visible river pixels. Both GPU owners draw them
  in the map pass.
- **The map controller** (`apps/codekarl/src/map_app.rs`) owns the one camera, with six keyboard actions from `[input.bindings]`.
- **Input is keyboard only.** No backend delivers mouse buttons, motion or scroll.
- **Two Lua VMs**, `ui` and `gameplay`, each on its own thread, with `StdLib::ALL_SAFE`.
  - The UI VM draws through the scene API: `render.create_quad{texture=, x=, y=, width=, height=}`, one whole texture per quad. The
    bridge's sampling is nearest by default.
  - It receives `frame_update`.
  - Nothing calls the gameplay VM after start-up.
- **There is no text drawing, clock, simulation loop or save.**

## 3. Places

The data word is **location**, as in map-design §5.5; "place" is the same thing.

### 3.1 Scenario and location data (Proposal on Firm 768)

**Which places exist depends on the start date**, so locations belong to a **scenario**, not to the map. This departs from map-design
§9.5, which listed `locations.toml` among the later map files; the terrain stays scenario-independent.

| File | Holds |
|---|---|
| `assets/scenarios/ireland-768/scenario.toml` | The scenario's id, its map directory, its start date and its locations file |
| `…/ireland-768/locations.toml` | One `[[location]]` per place: `id`, `name`, `lat`/`lon`, `kind`, `size`, notable `buildings`, an optional `catchment_m`, `source` and `confidence` |
| `assets/map/ireland/location_kinds.toml` | Kinds (`monastery`, `royal_site`, `ringfort`, `crannog`, `longphort` later, …), their size range, default catchment radius by size, marker glyph and house rule; each notable building's glyph (`church`, `round_tower`, `high_cross`, `rath`). The look is shared by every scenario on this map |

```toml
[[location]]
id = "clonmacnoise"          # stable, unique, lower-case ASCII; saves and the economy refer to it
name = "Cluain Mhic Nóis"
lat = 53.326
lon = -7.986
kind = "monastery"
size = 3
buildings = ["church", "round_tower", "high_cross"]
source = "..."
confidence = "high"
```

- **Start: 9 October 768, 00:00 (Proposal).** The year is Firm. The day is the vision's example date (Charlemagne's coronation); the
  scenario file holds it, so changing it changes data only.
- **Position:** the cell the point falls in (map-design §9.3). In M7 each location occupies one cell; multi-cell footprints are Open.
- **Strict validation:**
  - unique ids;
  - every point in the crop and on land;
  - at most one location per cell;
  - every kind and building known;
  - each size in its kind's range;
  - the scenario's map directory loads;
  - **the kinds table:**
    - kind and building ids are unique and well formed;
    - `sizes = [min, max]` with 1 ≤ min ≤ max ≤ 16;
    - `catchment_m` has exactly max − min + 1 entries, each 1 to 50,000;
    - a glyph is rectangular, at most 9 × 9, with at least one `#`;
    - colours are `#rrggbb`.

  A failure names the file, the entry and the problem.
- **Ireland in 768 (for slice 6).** About 80–150 skeleton locations:
  - the great monasteries: Armagh, Clonmacnoise, Kildare, Glendalough, Bangor, Durrow, Clonard, Lismore (Kells is founded only around
    807);
  - the royal seats: Tara, Cashel, Cruachan, Ailech, Naas (Emain Macha and Dún Ailinne were long abandoned by 768);
  - the main ringfort and crannog centres of the over-kingdoms.

  There are no towns and no Norse ports; Dublin (841) and the others arrive later through play, and are not placed. The list is authored
  with the owner, as the terrain was, with a guesswork log.

### 3.2 Catchments (Proposal)

The Firm rule is: cells within the radius; the nearest location takes an overlapped cell; the rest belongs to no location.

- **Computed once at load** by `ck-map`, as one location index (or none) per cell. Gameplay-critical, so exact integer arithmetic with
  **no division and no rounding** after the scale table:
  - `s` is the location's row ground scale, cos(latitude) in 1/65,536 (the frozen per-row table, f64 rounded once and pinned by a test);
  - the offsets in projected metres are `dx = di × 839` and `dy = dj × 839`;
  - a cell is inside the radius `r` (ground metres) when `(dx² + dy²) × s² ≤ r² × 2³²`, evaluated in `i128`;
  - the nearest location minimises `(dx² + dy²) × s²`, compared exactly in `i128`;
  - a tie goes to the location that comes first in `locations.toml`;
  - the bounding box of the cells visited is `±⌈r × 65,536 / (839 × s)⌉` cells, computed in integers, and the exact test filters it.
- Sea and lake cells are included; the economy decides what each terrain yields.
- Not drawn in the game in M7. The preview tool writes a catchment image for the owner to check the radii.

### 3.3 The look on the map (Proposal, from mock-up 2)

- **Below step 15** (coarser than about 2.1 cells, or 1.1 km, per map pixel): geography only.
- **From step 15:** each location draws its kind's **marker glyph**, centred on its cell. The glyphs are small pixel patterns in
  `location_kinds.toml`, written as rows of characters as in mock-up 2, in a light and a dark colour, with a one-pixel dark outline.
- **From step 46** (about 0.25 cells, or 125 m, per map pixel), **houses** surround the marker:
  - their count and spread come from the kind and the size;
  - their positions come from a frozen hash of the id and the house index;
  - each notable building's glyph is drawn at a hashed position in the house area.
- **Drawing (map-design §9.9).** Everything goes into the map target in the map pass, through the rivers' mechanism:
  - `ck-map` prepares **location pixels** once per step, and the camera snapshot carries the visible ones;
  - each GPU owner draws them as one-map-pixel quads;
  - the overlay pixels gain a palette index;
  - the draw order is terrain, houses, glyphs, then rivers last: map-design §3.4 (Firm) draws rivers "over everything, at every zoom",
    as mock-up 2 does (r2 had glyphs over rivers);
  - the reference colour function gains the same rule, so readback fixtures still compare byte for byte.
- Names are never drawn into the map; they are UI (§7). Roads stay out.

## 4. The game clock

### 4.1 Time and the calendar (Proposal)

- **The tick is one game hour (Firm).** Game time is a `u64` count of hours since a fixed epoch; dates are derived from it.
- **The calendar is Julian, with exact integer arithmetic**, correct for 768. A Gregorian switch is Open and far off.
- **Cadences.** A system registers for one of four: hourly, daily (hour 0), monthly (the 1st, hour 0) or yearly. On a boundary the coarser
  cadences run first: yearly, then monthly, daily and hourly. This is the one boundary order; every other section follows it. Within a
  cadence, systems run in registration order, and the order is frozen
  in the spec. The real systems' order stays Open until the economy.
- **Since M9 (CK-224, accepted 2026-11-08; `docs/economy-design.md` §3.9):** each coarse cadence carries an **hour of day**: `Yearly { hour }`,
  `Monthly { hour }` and `Daily { hour }`. The default is 0, so the midnight behaviour above still holds for every system registered
  without an hour.
  - Systems due on a tick are those whose cadence and hour match, run coarser first and in registration order.
  - The economy is one `Daily { 12 }` system at noon.

### 4.2 Speeds and pausing (Firm speeds; Proposal details)

| Speed | Hours per second | One day takes |
|---|---|---|
| 1 | 1 | 24 s |
| 2 | 6 | 4 s |
| 3 | 24 | 1 s |
| 4 | 72 | ⅓ s |
| 5 | 168 | ⅐ s |

- **Actions** in `[input.bindings]`:
  - `time_pause`, which toggles (default Space, Firm);
  - `time_speed_1` to `time_speed_5` (defaults Digit1 to Digit5, Firm);
  - `time_faster` and `time_slower` (proposed defaults `]` and `[`).

  A new or loaded game starts paused at speed 1.
- **Since CK-297 (the pause menu; `docs/ui-design.md` §3.15 and §3.6):**
  - the simulation also takes a control that **only pauses**: a running game pauses with the reason `player`, and a paused
    game is left as it is, its reason included. No action and no key maps to it: the UI sends it with `game.pause()` when the
    pause menu opens;
  - while that menu is open, the time actions above, `game_quicksave` and `game_quickload` (§5.3) and the six map actions do
    nothing.
- **Never skip (Firm).** The clock decides only how many ticks are due. If the simulation falls behind, it runs slower; it never skips,
  merges or shortens a tick. A single wake runs at most a bounded number of ticks, so a stall cannot cause a burst.
- **Real time never changes outcomes.** Speed, pause, frame rate and machine load are not simulation state, and the save does not hold
  them.
- **Orders while paused (Firm).** A player order is a **command**. While running, it applies at the start of the next tick. While paused,
  it applies at once, at the current tick boundary, in arrival order. It is always journalled with its tick and sequence number.
  - **The command phase.** A command stamped tick t applies before every system of tick t, ahead of the yearly ones.
  - A replay applies the journal's tick-t commands, in sequence order, at the same point.
  - A paused command applied at once and a running command applied by the next step therefore land in the same phase and give the same
    state. M7 has no
  gameplay commands; the first come with the economy, and §5 tests the path with a test-only command.
- **Multiplayer (Firm: server settings)** is not built. Speed and pause are requests to the one clock owner, so a later server can decide
  who may make them.

### 4.3 Where it runs (Proposal, with Firm question 6)

- **A new package, `ck-sim` (`crates/sim`)**, backend-neutral and windowless, with no Lua or window dependency. It holds the calendar,
  clock, scheduler, commands and journal, random streams, digest and save format.
- **The simulation thread.** The application runs the simulation on its own thread, as a runtime service. It starts after the scenario
  loads and before the first frame, and stops in the lifecycle's reverse order. It never runs on the main or render thread, which keeps
  60 fps at speed 5.
- **Publication.** The simulation publishes an immutable snapshot, latest wins, with the tick, date, speed and paused (with a pause
  reason: player, error or load).
  - **It wakes the UI whenever any displayed field changes**: the shown hour, the speed or the paused state. That includes a pause, a speed
    picked while paused, an error pause and a load, even when the shown hour is unchanged. (r3 woke on the hour only: F2.)
  - Wakes are coalesced to at most one per frame. When nothing changes, nothing wakes; there is no polling.
  - Rendering never waits on the simulation.
- **The gameplay VM on the clock (Firm, question 6).**
  - **The calls are scheduler systems.** In M7, `on_month(date)` was registered as the last monthly system, and `on_day(date)` as the
    last daily system, at midnight.
    - On the 1st of a month they therefore ran in the §4.1 order: the monthly Rust systems, `on_month`, the daily Rust systems,
      `on_day`, then the hourly systems. (r2 had `on_day` before `on_month`.)
    - **Since M9 (CK-224):** both are `Daily { 12 }` systems registered after the noon economy system. `on_month` fires only on the
      1st, before `on_day`, so Lua sees each day's finished economy at noon.
    - The simulation thread posts each call to the gameplay VM and waits for its reply under the existing call deadlines. `date` is a table of integers:
    `{year, month, day, hour, tick}`.
  - The M7 skeleton script only logs, for example `log.info("day " .. date.year .. "-" .. date.month .. "-" .. date.day)`.
  - A callback that is not registered is skipped.
  - **A script error** is logged with its traceback, and the clock pauses at that boundary. The error is deterministic: the same save and
    commands give the same error.
  - **A missed deadline** is a runtime failure under the existing rules, because it is not deterministic.
  - Under §5.5, the gameplay VM keeps no game state between calls in M7.

## 5. Reproducible saves

### 5.1 The rule (Firm scope, question 2)

**Firm:** the same build gives the same outcomes on any supported machine and backend. **Proposal:** simulation state at tick T is a pure
function of four things:
1. the scenario and map content (by hash);
2. the master seed;
3. the state it was loaded from;
4. the ordered command journal up to T.

Everything else is excluded: wall-clock time, thread timing, frame rate, speed, pause, `HashMap` or `HashSet` order (simulation code uses
index order or `BTreeMap`), allocation addresses and uninitialised data.

**All simulation arithmetic is integer or fixed point.** Floats may compute display values from state; they never write state.

### 5.2 Randomness and commands (Proposal)

- **Counter-based random streams.** A draw is a frozen 64-bit mix (SplitMix64) of four things: the master seed, a stream id, the tick and
  a draw counter.
  - Each system, and later each battle or event, has its own stream, so adding one never changes another's numbers.
  - A new game draws its master seed from the operating system and records it. `--seed N` fixes it for tests.
- **Commands** are tagged values with a tick, a sequence number and an issuer (the player now; tags and network players later).
  - The journal is kept in memory and saved with the state, so tests and later desync checks can replay it.
  - Replay is a testing aid, not a player feature.

### 5.3 The save file (Proposal)

- **One file.** A TOML header holds:
  - the format version and the build id;
  - the scenario id and the scenario and map content hashes;
  - the **gameplay scripts' content hash**: the entry script and anything the gameplay VM is given. They are deterministic inputs like
    the scenario (F3);
  - the start date, the current tick and the master seed.

  A binary body follows: canonical little-endian, length-prefixed sections, each with a checksum. In M7 it holds the clock, the
  random-stream state, the pending commands and the journal. Locations are static in M7, so only their hash is saved.
- **Since M9 (CK-225):** the header also holds `rules_hash`, the content hash of the shared rules (`assets/game/economy.toml`) and the
  map's `terrain.toml`, checked on load like the others. The body's sections are `CLCK` (the tick), `STAT` (the game state, which holds
  the economy), `PEND` and `JRNL`, each with its checksum, then a whole-file checksum.
- **A save is taken at a tick boundary.** The simulation finishes its tick, writes the file, then continues.
- **Load checks:**
  - the build id must match for a guaranteed replay. An older build's save is refused in M7 with a named error; cross-version loading is
    Open;
  - the hashes must match the loaded scenario.

  Nothing loads halfway, and a loaded game starts paused.
- **Controls:** `game_quicksave` and `game_quickload` (defaults F5 and F9) use `[game] saves` (default `saves/`), and `codekarl --load
  PATH` starts from a save. There is no save screen in M7.

### 5.4 How it is shown (Proposal)

These are windowless tests in `ck-sim`, with a fake clock and test systems that draw random numbers and change state on every cadence. A
**state digest** is a stable 64-bit hash of the canonical saved form. It is used only by tests and never shown in the game.

1. **Run twice:** the same seed and journal give equal digests after N days.
2. **Save, load, continue:** a run saved at day k, loaded and continued, gives the same digest at day N as an uninterrupted run.
3. **Real time changes nothing:** different speed and pause patterns and wake batch sizes give the same digest.
4. **Stream independence:** a stream's numbers are unchanged whatever other streams draw.
5. **The calendar:** dates are right across Julian leap years, month ends and the 768 start.
6. **Commands while paused** apply at the boundary in arrival order, and replay identically.

### 5.5 Lua determinism rules (Proposal, for Firm question 6)

Lua 5.4 seeds its string hash and `math.random` from the time and from memory addresses. So `pairs` order and `math.random` differ between
runs, and the gameplay VM must change.

- **`math.random` and `math.randomseed`** are replaced by the simulation's own stream for the gameplay VM (`sim.random(m, n)`, integers
  only).
- **`pairs`** is replaced by ordered iteration: integer keys ascending, then string keys by byte order. A table with any other key type is
  an error. **`next`** is removed, and `sim.is_empty(t)` takes its common use. Iteration order then never depends on the hash seed.
- **Every environmental path is closed in the gameplay VM**, not only the clock functions (r2 named only some):
  - the whole `os` library (`os.execute`, `os.tmpname`, `os.remove`, `os.rename` and `os.exit` included) and the whole `io` library;
  - `loadfile`, `dofile`, `require` and `package`. M7 loads the entry script only; module loading is Open;
  - `load` accepts text chunks only;
  - `collectgarbage`, whose results and timing depend on memory.
- **Garbage-collector order:** `setmetatable` refuses a metatable with `__gc` or `__mode`, so finalizers and weak tables cannot make state
  depend on when the collector runs.
- **Addresses:** `tostring` of a table, function, thread or userdata returns its type name only, and `string.format` refuses `%p`, so no
  memory address reaches a value.
- `print` and `log` stay. They are output, not input.
- **The configuration surface (F3).** Today every VM gets the whole application configuration as `config` (`apps/codekarl/src/vm.rs`).
  That includes the backend, window, frame and log settings, which differ between machines and backends. Under the deterministic profile,
  the gameplay VM's `config` holds only a canonical gameplay subset taken from the scenario: `{scenario = id, start = {year, month, day,
  hour}}`. That subset is already covered by the scenario hash in the save.
  - No host, backend, window, UI, path or log setting is visible to the gameplay VM.
  - The profile is applied before the entry script runs.
  - The UI VM keeps the full configuration.
  - A later gameplay setting must join this subset and the save's inputs; it is never read from the host configuration.
- **No game state in Lua (M7).** The gameplay VM keeps no state between calls that the save would need. How Lua-held state is saved is
  Open, and comes with the economy.
- **Proof:** a windowless application test runs a script that iterates tables with `pairs` and draws `sim.random` on each day. It folds
  the results into a test system, and two runs, and a save-load-continue run, give equal digests. The runs **also vary the host
  settings**: backend, window, frame and log levels. The digests and the gameplay log lines (without timestamps) must still be equal.
  A check that the gameplay VM's `config` has exactly the canonical keys completes it.

## 6. Mouse pan and zoom (Firm gestures)

- **Firm:** left-drag pans, and every scroll, from a wheel or a trackpad, zooms at the cursor.
- **Native events, which need work on both backends:**
  - Cocoa: left button down, dragged and up; `scrollWheel` with `hasPreciseScrollingDeltas`;
  - GLFW: cursor position, mouse button and scroll callbacks. GLFW's scroll callback carries only x and y offsets: on macOS it scales a
    precise delta by 0.1 and drops the precision flag. So the GLFW backend reports every scroll as **lines**, accumulated fractionally,
    and there is no normalisation contract. On a trackpad, GLFW then zooms one step per about 10 points of travel, against Cocoa's
    configurable 24. That difference is documented, not corrected.

  Pinch (magnify) is not handled in M7 (Open).
- **`ck-input` gains an ordered pointer ring (F1).** Latest-position latches lose gestures between paced updates (a whole down, move,
  up, or a scroll at A followed by motion to B), so pointer input is kept in order, as key transitions already are.
  - **The events:** a fixed-capacity ring (proposed 64) of `Down(pos)`, `Move(pos)`, `Up(pos)` and `Scroll(pos, amount, unit)`. Positions
    are in 1/256 point, and amounts in 1/256 line or point.
  - **Coalescing keeps it small without losing meaning:** a `Move` directly after a `Move` replaces it. A `Scroll` directly after a
    `Scroll` with the same position, unit and sign adds to it. A sign change starts a new entry, so a reversal survives.
  - **Snapshots:** a snapshot copies the retained events and the current position and held state. Only acknowledging consumes them, up to
    the snapshot's sequence.
  - **Overflow:** the oldest detail is dropped, and the snapshot carries an overflow flag with the current position and held state. The
    controller then ends or continues any drag from its own retained anchor to the current position, and the lost scroll is discarded. Like
    key resync, this keeps the final state and never invents order.
  - **Focus loss** appends an `Up` at the current position if the button was held, and sets reset.
- **Tests (CK-170 layer).** The owner's mouse is already dropped in window tests. Tests inject marked synthetic pointer events through the
  existing test-only path, extended from keys to the pointer.
- **The map controller:**
  - **The controller replays a snapshot's pointer events in order**, then applies keys.
  - **Drag:** `Down` records the anchor position and camera. Each `Move` or `Up` sets the drag's total to `floor((pos − anchor) / 4
    points)` map pixels, and the controller applies the difference from what it has already applied, so there is no drift. `Up` ends the
    drag. The map follows the cursor, so the land never shimmers, and a drag continues outside the window while the button is held. A
    whole down, move and up within one update applies fully.
  - **Scroll zooms at the cursor:** scrolling away from the user zooms in, and the operating system's direction setting is respected.
    - A wheel's line delta gives one step per line.
    - A trackpad's precise delta builds up to one step per `[map] scroll_points_per_step` (proposed default 24; range 1 to 256).
    - The remainder is kept, and is cleared when the direction reverses. Each `Scroll` entry zooms at **its own** position, so a scroll at
      A followed by motion to B still anchors at A.
  - **The anchor:** the world point under the cursor stays under the cursor. This generalises map-design §9.3's centre rule to any
    anchor, and the spec gives the exact integer formula. Keyboard zoom keeps the centre.
  - Drag and scroll publish camera snapshots as the keys do, at 60 fps while moving.

## 7. Text: readout and hover names (Firm)

- **A pixel font as data, with no engine work.**
  - Each glyph is its own small PNG, 5×7 font pixels with a variable advance, under `assets/ui/font/`, and `font.toml` maps characters to
    glyphs and advances.
  - The UI VM draws each glyph as a scene quad, nearest-sampled, at 2 points per font pixel (configurable).
  - The set is printable ASCII, the Irish vowels with fada (á é í ó ú, Á É Í Ó Ú), and the middle dot (·, U+00B7), which the readout
    uses (N1).
  - **Unsupported characters** draw a fixed replacement glyph (a hollow box) and never fail; there is no logging per frame. A windowless
    data check requires every built-in UI string and every name in a shipped scenario to be fully supported, so Ireland's names never
    show the box.
  - The glyphs are authored in the project; there is no outside font licence.
  - A Lua UI helper, `scripts/ui/text.lua`, lays out a line and keeps one quad per glyph, updating them only when the text changes.
- **The readout:** one line in the top-left corner, `9 Oct 768 · 14:00 · speed 3`, or `paused` in place of the speed. It is updated from
  the simulation snapshot (§4.3).
- **Hover names:**
  - The map controller hit-tests the cursor against the visible location glyphs from step 15 (§3.3): the location whose marker
    rectangle, outline included, contains the cursor's map pixel, with ties going to file order.
  - It publishes the hovered location's id, name and anchor point to the UI. The UI draws the name in a tooltip box next to the cursor.
  - Below step 15 nothing is hovered.

## 8. Packages and ownership (Proposal)

| Package | Change |
|---|---|
| `ck-input` | Pointer state and its mapper (§6) |
| `ck-window`, `ck-window-cocoa`, `ck-window-glfw` | Pointer events and the test injection path |
| `ck-map` | The scenario and its locations: loading, validation, catchments, location pixels per step, the reference rule, and the preview tool's markers and catchment image |
| `ck-render-2d`, `ck-gpu-metal`, `ck-gpu-vulkan` | Coloured overlay pixels in the map lane and map pass |
| `ck-sim` (new) | Calendar, clock, scheduler, commands and journal, random streams, digest and save format |
| `ck-lua` | The deterministic gameplay-VM options (§5.5), as a VM configuration rather than a game concept |
| `apps/codekarl` | The scenario configuration, the simulation service, the time and save actions, `--load` and `--seed`, the snapshots to the UI, the gameplay VM calls, the mouse controller, the font, the readout and the tooltip |

## 9. Milestone 7 slices (Proposal, dependency-ordered)

Every GPU slice lands on Metal and Vulkan together. "Windowless" means no GPU and no window. Window tests run inside the CK-170 layer and
may take focus.

### Slice 1: Mouse pan and zoom at the cursor

- **Depends on:** nothing.
- **Behaviour:** §6. Left-drag pans by whole world pixels. Wheel and trackpad scroll zoom at the cursor. `[map] scroll_points_per_step`
  is configurable.
- **Crates:** `ck-input`, `ck-window`, `ck-window-cocoa`, `ck-window-glfw`, `apps/codekarl` (`map_app.rs`, configuration).
- **Tests:**
  - windowless: the pointer mapper (edges, accumulation, acknowledgement, overflow);
  - windowless: the controller, with the drag remainder, the exact anchor formula at several steps and positions, line and precise
    scroll, direction reversal and the clamp at steps 0 and 75;
  - window tests on Cocoa and GLFW: injected drag and scroll events move the published camera as expected, and the window closes
    normally.
- **GPU or window:** window groups on both backends; no new GPU work.
- **Owner proof:** the owner drags and scrolls the running map of Ireland.

### Slice 2: Calendar, clock and scheduler

- **Depends on:** nothing.
- **Behaviour:** §4.1 and §4.2 in `ck-sim`:
  - hour ticks, Julian dates and cadence boundaries in their frozen order;
  - the speed table and pause, driven by a supplied clock;
  - never skip, and a bounded number of ticks per wake;
  - when commands apply, running and paused.
- **Crates:** `ck-sim` (new).
- **Tests:** windowless. Calendar vectors across leap years and month ends from 768. Cadence order. Ticks due under a fake clock at every
  speed, including a slow consumer. Command timing (§5.4 items 5 and 6).
- **GPU or window:** none.
- **Owner proof:** none (groundwork).

### Slice 3: Reproducible state and saves

- **Depends on:** 2.
- **Behaviour:** §5.1 to §5.4: random streams, the journal, the digest, and the save file (written and checked on load), with named load
  errors.
- **Crates:** `ck-sim`.
- **Tests:** windowless. §5.4 items 1 to 4. Save round-trip byte equality. A corrupted section, a wrong hash and a wrong build id are each
  refused with a named error. A partial file is refused.
- **GPU or window:** none.
- **Owner proof:** none (groundwork).

### Slice 4: Scenario and location data

- **Depends on:** nothing.
- **Behaviour:** §3.1 and §3.2:
  - `scenario.toml`, `locations.toml` and `location_kinds.toml`, with strict validation;
  - catchments;
  - the preview tool drawing marker glyphs and writing a catchment image.
- **Crates:** `ck-map` (and its preview tool).
- **Tests:** windowless, on a small asymmetric fixture: each validation refusal names its file and entry; exact catchment ownership,
  including overlaps, ties and the latitude correction; preview output.
- **GPU or window:** none.
- **Owner proof:** preview images.

### Slice 5: Locations on the map

- **Depends on:** 4.
- **Behaviour:** §3.3:
  - location pixels (glyphs, houses and notable buildings) prepared per step and selected per view;
  - coloured overlay pixels in the map lane;
  - both map passes;
  - the reference rule.
- **Crates:** `ck-map`, `ck-render-2d`, `ck-gpu-metal`, `ck-gpu-vulkan`, `apps/codekarl`.
- **Tests:**
  - windowless: the reference rule (thresholds at steps 14, 15, 45 and 46, draw order, hash vectors);
  - readback fixtures on both backends: the map target equals the reference byte for byte, with locations at a marker step and a house
    step, rivers crossing them, and a pan away and back.
- **GPU or window:** GPU readback on both backends.
- **Owner proof:** the preview tool, and the running map once slice 6's data exists.

### Slice 6: Ireland's 768 locations

- **Depends on:** 4.
- **Behaviour:** the authored scenario `ireland-768`: about 80–150 locations, with their kinds and glyphs, sources, confidence and a
  guesswork log, made with the owner as the terrain was (preview pages at three steps).
- **Crates:** data only (`assets/`), plus the guesswork log.
- **Tests:** the scenario loads and validates (windowless).
- **GPU or window:** none.
- **Owner proof:** **owner sign-off** on the preview pages.

### Slice 7: The simulation in the application

- **Depends on:** 2, 3, 4.
- **Behaviour:** §4.3, §5.3 controls:
  - `[game] scenario` and `[game] saves` configuration;
  - the simulation service on its own thread;
  - the time actions, quicksave and quickload, `--load` and `--seed`;
  - the snapshot to the UI.
- **Crates:** `apps/codekarl`.
- **Tests:**
  - windowless: lifecycle start and stop; a headless run of N days at a fixed seed is reproducible; quicksave, quickload and `--load`
    round-trips; configuration and command-line refusals; snapshot publication on every displayed change (a pause, a speed while paused,
    an error pause and a same-hour load) and not otherwise;
  - window tests on both backends: injected time actions pause, change speed and resume; the map keeps its frame timing at speed 5 while
    panning; normal close.
- **GPU or window:** window groups on both backends.
- **Owner proof:** none on its own (the readout arrives in slice 9); log lines show the date.

### Slice 8: The gameplay VM on the clock

- **Depends on:** 7.
- **Behaviour:** §4.3 (`on_month` and `on_day` as the last systems of their cadences; errors pause the clock) and §5.5 (`sim.random`,
  ordered `pairs`, `next` removed, every environmental path closed, the `__gc` and `__mode` and address rules); the gameplay skeleton logs
  each day.
- **Crates:** `ck-lua` (the deterministic VM options), `apps/codekarl`.
- **Tests:** windowless:
  - the ordered-`pairs` and removed-globals contract;
  - call order against the Rust systems;
  - an error pauses at the boundary, deterministically;
  - the §5.5 proof: two runs and a save-load-continue give equal digests, with the host settings varied (F3);
  - the gameplay VM's `config` has exactly the canonical subset;
  - a changed gameplay script changes the save's compatibility hash.
- **GPU or window:** none.
- **Owner proof:** the log shows the gameplay VM's daily line.

### Slice 9: Pixel font and the date readout

- **Depends on:** 7.
- **Behaviour:** §7: the font data, `text.lua`, the replacement glyph, and the top-left readout driven by the simulation snapshot. It
  wakes on any displayed change (§4.3).
- **Crates:** `apps/codekarl` (UI scripts, font assets, the snapshot into `frame_update` or a posted call).
- **Tests:**
  - windowless: font data validation (every listed character has a glyph, and the advances are positive); a layout test (glyph quads
    and positions for a sample line); the replacement glyph; the data check over the built-in readout strings and the shipped
    scenario's names;
  - readback on both backends: the readout's quads draw the expected pixels;
  - window tests: the readout changes on injected pause and speed actions **with no tick advancing** (while paused), and on a same-hour
    load; there is no continuous polling.
- **GPU or window:** readback and window groups on both backends.
- **Owner proof:** the owner sees the date and speed on screen.

### Slice 10: Hover names

- **Depends on:** 1, 5, 9.
- **Behaviour:** §7: the hit test against visible glyphs, publication to the UI, and the tooltip.
- **Crates:** `ck-map` (the hit test), `apps/codekarl`.
- **Tests:**
  - windowless: the hit test at marker and house steps, outlines, ties and nothing below step 15;
  - window tests: an injected pointer over a fixture location shows its name, and moving away clears it.
- **GPU or window:** window groups on both backends.
- **Owner proof:** the owner hovers over places on the running map.

**Parallel work:** slices 1, 2 and 4 have no dependencies. 3 follows 2; 5 and 6 follow 4; 7 needs 2, 3 and 4; 8 and 9 follow 7; 10 comes
last.

**Milestone acceptance (Proposal):**
- The owner reads the running map, with Ireland's 768 places at several zooms and their names on hover.
- The owner pans and zooms with the mouse, and runs, pauses and changes the speed of the clock, reading the date on screen.
- Quicksave and quickload work.
- The reproducibility tests of slices 3 and 8 pass.
- 60 fps while panning and zooming at speed 5, on both backends, and both backends close normally.
- The full-level check runs once at milestone completion.

## 10. Owner questions

All six of `owner-questions-r1.md` are answered (§1). None are outstanding for slicing.

## 11. Open, not blocking M7

- The location look (owner, deferred), multi-cell footprints, villages and farms, roads and permanent name labels.
- Pinch zoom.
- Rulers, counties, the political map and de facto rule (map-design §5.4 and §5.6).
- A catchment map mode and the other map modes.
- Multiplayer server time settings, autosave, a save screen, cross-version loading and Lua-held state in saves.
- The Gregorian switch, and the real systems' order within a tick.

## Provenance

- r1, 2026-11-06, for ck-manager's request `n3dxjbq6q2dmvd7vcbhsd2cxxa`.
- r2, 2026-11-06, for `gws74kcx9tiv4fnymvptcaqzin`, recording `owner-answers-r1.md`.
- r3, 2026-11-06, for ck-reviewer-1's questions `k497jffi9nm2mkzvf5ethr67i6` and ck-manager's answer `icrg9vxrny5bnuxfnveig6gnhw`.
- r4, 2026-11-06, for design review 1 (`design-review-1-reviewer-1.md`, e339bcf9; request `qh7ii2fdmv94y773xhczijfxp6`) and the CK-191
  questions `nzrrvmwpfkenxvkqvb43j5wsqw`.

Read: `docs/vision.md`, `docs/game/economy.md`, `docs/game/crown-military-church.md`, `docs/map-design.md`, `docs/runtime-design.md`,
`docs/scene-design.md`, mock-up 2's sprites, and the accepted code:
- `map_app.rs`, `render_api.rs` (whole-texture scene quads), `frame_api.rs` and `vm.rs` in `apps/codekarl/src`;
- `crates/render-2d` (nearest sampling);
- `crates/lua` (`StdLib::ALL_SAFE`, reserved globals);
- `crates/input`, `crates/window-cocoa` and `crates/window-glfw` (no pointer events).

No code, issues, builds or runs.
