# UI windows and panels: design (milestone 8)

> **Status:**
> - the owner approved the slice plan on 2026-11-07 ("start");
> - ck-reviewer-1 approved the design (`.pipeline/coordination/m8-design/ui-design-review-2-ck-reviewer-1.md`).
>
> This is the design-session draft r8 (SHA-256 `c0b59fad…a93a`), with the owner's answers recorded below and the revision history marked
> historical. Slices become issues one at a time, each with its own reviewed spec.
>
> **Labels:**
> - **Firm:** the owner's direction.
> - **Proposal:** the design, approved for slicing. Each slice spec may refine it.
>
> **Basis:** every "exists" claim cites the accepted tree as of CK-205 (2026-11-07). Author: ck-designer-1 (Claude, Class S).
>
> **Delivered.** Milestone 8 is accepted: slices 0 to 7 as CK-206 to CK-212 and CK-215 to CK-217, the owner's follow-up (clickable
> speed boxes and the places dropdown) as CK-219, and fixes and guide findings in CK-213, CK-214 and CK-218 to CK-223. The look (owner
> Q1) was mocked up from the owner's description (`m8-design/look-mockup-r1` to `r5`) before the theme slice, CK-216. Where a slice
> spec refined this design, the spec and the accepted code win over this text. "Proposal" below now means "the approved design as
> sliced".
>
> **Since milestone 8.** Milestone 9 added, with the same widgets and theme (`docs/economy-design.md` §3.11 and §3.22):
> - **the location panel:** a window filling the left third of the screen with the selected place's economy, in three tabs (location,
>   pops and buildings) shown as title-bar buttons. Its tab widget, `scripts/ui/lib/tabs.lua`, is the one new widget. A map click or a
>   places-list row shows that place in the panel;
> - **the map-mode bar:** three icon buttons, left of the places bar, for the stockpile, yield and elevation map modes. They call
>   `game.set_map_mode`; the button of the mode showing is selected, and pressing it turns the mode off.

## Owner answers (2026-11-07)

From `.pipeline/coordination/m8-design/owner-answers-r1.md`. **No question is open.**

1. **The look:** "I'll describe it". The owner describes the look, and a mock-up follows the description. Theming (slice 7) waits for
   both. The description is pending. *(Historical: given and mocked up; see "Delivered" above.)*
2. **Escape:** it closes the top window; with none open, it requests the close as today (§3.5).
3. **UI scale:** one `[ui] points_per_pixel` for the font, borders and icons; the old key is refused, naming the new one (§3.3).
4. **First real window:** a place window, opened by clicking a place marker (§3.11).
5. **Window positions:** not saved in M8.
6. **Map input and the UI:** "keeping the map input and the ui seperate is important, i think we should require it". Decoupling is
   required in M8: §3.12 and slice 0.

## 1. Owner direction

- **Firm (owner, 2026-11-07, `m8-design/owner-direction.md`).** Milestone 8 is **"UI windows and panels"**: the general window and panel
  framework first, with no gameplay behind it yet. The economy is milestone 9.
- **Firm (`docs/vision.md`):**
  - The game is UI-heavy: "many windows, panels, menus and UI elements".
  - "UI code is kept truly separate from world rendering" (responsiveness).
  - "As much as possible is moddable."
  - The map style: dark background, flat colours, low colour count.
- **Firm (`AGENTS.md`).** No window, renderer abstraction or similar before its design is discussed with the owner. This document is that
  discussion: it goes to the owner before any slice is filed.
- **Firm (owner, standing).** Proportionate work: no engine debug counters, no fixed sleeps, tests in their owning crates.

## 2. What exists (accepted)

### 2.1 The UI VM

- **One Lua VM**, named by `[lua.ui] entry` (`config/codekarl.toml`; `apps/codekarl/src/config.rs`, module docs).
  - It runs on its own owner thread under the `Standard` profile (`crates/lua/src/runner.rs`, `run`).
  - It gets the whole configuration as `config`.
- **Its surface** (`apps/codekarl/src/lib.rs`, the composition after line 575):

  | Global | What it does | Source |
  |---|---|---|
  | `window` | `open`, `set_clear`, `close`, `on_opened`, `on_close_request` | `window_api.rs` |
  | `frame` | `on_update(fn)` | `frame_api.rs` |
  | `render` | `create_quad`, `set_position`, `set_rect`, `destroy_quad`, legacy `quad`/`clear_quad` | `render_api.rs` |
  | `texture` | `load`, `reload`, `release`, `status` | `texture_api.rs` |
  | `game` | `on_time`, `on_hover`; only with `[game] scenario` | `game_api.rs` |
  | `font` | `layout`; only with a scenario | `font.rs` |
  | `log` | logging | |

- **Calls in flight.**
  - The VM has a call limit of 4 (`vm.rs`, `UI_CALL_LIMIT`).
  - `frame_update` is refused while 3 are outstanding, so a lifecycle call always has a place (`ui_link.rs`, `post`).
- **Publication.** A completion hook publishes the scene after `startup()` and after each posted callback with a result, and only when
  something changed (`render_api.rs`, `RenderEnd::completion_hook`).
- **Module loading.**
  - `Standard` keeps Lua's `package`/`require` with Lua's default search path (relative to the process's working directory). No path is
    set for `scripts/ui/`.
  - So `scripts/ui/main.lua` keeps its line helpers as local functions: "The UI VM has no module loading".
- **The process locale is fixed:** `os.setlocale` is removed in every VM (`runner.rs`, CK-200).

### 2.2 The scene

**Persistent ordered quads** (`crates/render-2d/src/scene.rs`, `bridge.rs`; `docs/scene-design.md` §8):
- **Placement:** a quad is a whole texture placed at a rectangle in window points (`FrameQuad`: texture, x, y, width, height).
- **Painter order** is creation order, kept stable (`scene.rs`: "in stable painter order"). There is no call to reorder, hide or group
  quads.
- **No per-quad colour or tint, and no clip rectangle.** Clipping happens only at the window edge (`FrameQuad::place`).
- **One sampling filter for the whole scene,** `[render] filter`.
- **Capacity:** `[render] max_quads`, default 131,072 (`SceneLimits::DEFAULT`).
- **Hiding:** with no visibility call, the M7 tooltip hides by moving its quads to −1,000,000 (`scripts/ui/main.lua`, `HIDDEN`).

### 2.3 Composition with the map

- **The map is drawn in Rust.** A map pass writes the map target; the window pass then draws the enlarged map, then the legacy quad, then
  the scene. "The UI is drawn last, over the map, at full resolution" (`docs/map-design.md` §9.2).
- **The scene is the UI layer.** Nothing in the UI is part of the map pass.

### 2.4 Text (CK-197)

- **The font:**
  - `assets/ui/font/font.toml` holds 5×7 glyphs in an 11-row box, with one PNG per glyph in a single colour (`#ece3cb`);
  - `font.layout(text)` returns glyph textures and offsets (`apps/codekarl/src/font.rs`);
  - the scale is `[ui] points_per_pixel` (1–8, default 2), which owner Q3 renamed from `font_points_per_pixel` (CK-208, §3.3).
- **The layout helpers:** `scripts/ui/main.lua`'s `line`/`set`/`move`/`destroy` keep one quad per glyph and touch only what changed.
- **The panel background** is `assets/ui/font/panel.png`, one pixel of `#2a2119`, stretched.

### 2.5 Delivery from the game (CK-197, CK-198)

- **The calls:** `game_time` and `game_hover` are posted by the UI link (`ui_link.rs`), latest wins, only on change, outside the reserved
  call place.
- **Hover is computed in Rust,** by the map controller (`map_app.rs`, `Hover`, `hover_at`), and given to Lua as
  `{id, name, left, right, y}` (`game_api.rs`, `hover_arguments`).
- **The tooltip** places itself against a hard-coded `WINDOW_WIDTH = 800` (`main.lua`), because the UI script does not register
  `frame.on_update`, which carries the window size.

### 2.6 Input (CK-189 and earlier)

- **Keys** become named actions through `[input.bindings]` (`crates/input`; `docs/scene-design.md` §10.1).
  - `close` is reserved: its press requests the window close with source `key`.
  - The time and save actions become simulation controls (`apps/codekarl/src/game/controls.rs`).
  - The six map actions pan and zoom (`map_app.rs`).
- **The pointer** is the left button, moves and vertical scroll only (`crates/input/src/lib.rs`, `PointerEvent`). There is no right
  button and no text input.
- **One snapshot goes to three consumers** (`ui_link.rs`, `CodekarlUi::post`):
  1. the UI VM gets `frame_update` (asynchronously, on its thread);
  2. once the post is accepted, the **map controller** is updated synchronously on the main thread with the same snapshot;
  3. the **simulation controls** are then sent.
- **The UI never sees the pointer.** `frame_update` carries `dt`, `elapsed`, `sequence`, the window size, `actions`, `transitions`,
  `overflow` and `reset` (`ui_link.rs`, `update_table`), but no pointer fields. The map consumes every press, drag and scroll.
- **A map run keeps `frame_update` on** whether or not the script registers it (`frame_api.rs`: `control.set(registered || map_enabled)`).

### 2.7 What M8 has to add

These are the gaps against "many windows, panels, menus":
1. a click on a panel would also drag the map, and the UI cannot see the click;
2. no z-order or visibility other than creation order and moving off-screen;
3. moving a window means one `set_position` per quad;
4. no clipping for scrolled content;
5. one text colour, and no tint for hover, pressed or disabled states;
6. no way for UI code to share modules;
7. no UI-to-simulation path except key bindings.

## 3. The proposed model

### 3.1 Who owns what (Proposal)

- **Lua owns the UI's state and logic.**
  - What it holds: which windows and panels exist, their contents, their positions, their z-order, focus, and widget state such as
    hovered and pressed.
  - This keeps the UI moddable: it is script and data, with no Rust per widget.
- **Rust owns what must be synchronous or fast,** and stays generic, with no concept of a "window" or "button":
  1. **Drawing:** the scene's quads, grouped and ordered (§3.4).
  2. **Pointer routing:** each pointer event is decided on the main thread against the UI's last published hit regions (§3.5). The map and
     the UI never both act on one event.
  3. **Text layout:** `font.layout` (exists).
- **UI state is never game state.**
  - It is not saved and not journalled, and it never reaches the gameplay VM.
  - The UI changes the game only through controls the simulation already accepts (§3.6), so CK-192's reproducibility is unaffected.
- **Separate from world rendering.**
  - The UI runs on its own thread and publishes immutable frames.
  - The map's pass never waits for Lua: the map is **drawn** at the frame rate whatever the UI is doing.
  - **Map input never waits on the UI (Firm, owner Q6; §3.12).**
    - **Today it does.** The loop posts a new update only when no update callback is outstanding (`crates/window/src/main_loop.rs`,
      `post_update`). The map controller and the time controls take the snapshot only after the UI VM accepted the post (`ui_link.rs`,
      `CodekarlUi::post`). So a slow UI callback stalls panning, zooming and time presses.
    - **M8 changes that first,** in slice 0. Input is consumed on the main thread at every input tick; the map and the simulation get
      their share at once; only the UI's share waits for the UI.

### 3.2 Vocabulary (Proposal)

- **Panel:** a rectangle of UI anchored to a window edge or corner, such as the date readout, a top bar or a time-control bar. It cannot be
  moved by the player.
- **Window:** a panel the player can open, close, raise and drag by its title bar. Windows form a stack: the most recently raised is on
  top.
- **Widget:** an element inside a panel or window:
  - M8 needs **label**, **button** (text or icon), **close button**, **separator** and **scroll list** (a vertical list of labels or
    buttons with a scroll position);
  - more widgets come with the systems that need them, in M9 and later.

### 3.3 Layout (Proposal)

- **Units:**
  - **Integer points.** Every position and size is a multiple of the UI pixel.
  - **One UI scale (Firm, owner Q3):**
    - The UI pixel is **`[ui] points_per_pixel`** points (1–8, default 2), for the font, panel art and icons alike.
    - It replaces `[ui] font_points_per_pixel`. The old key is refused with a message naming the new one (`config.rs`, where the old
      key's range is defined).
- **Anchors:** a panel's rectangle is an anchor (one of 9: corners, edge midpoints, centre) plus an offset and a size.
  - When the window size changes, anchored panels are recomputed.
  - Windows keep their position, clamped so the title bar stays reachable.
- **Inside a panel or window:** a vertical or horizontal stack with fixed spacing and padding. Sizes come from content: a label's text
  width, or a button's text plus padding.
  - There is no constraint solver and no flex layout.
  - A scroll list has a fixed height and clips (§3.4).
- **The window size** reaches Lua through `frame_update`'s `width`/`height`, which exist already. The UI script registers
  `frame.on_update` and relayouts only when the size changed. This also fixes the tooltip's hard-coded 800 (§2.5).

### 3.4 Drawing: scene groups (Proposal, engine)

The one engine change for windows: **ordered groups of quads**, added to `ck-render-2d`'s scene and the `render` API.

- **Order:**
  - `render.create_group{}` returns a group token.
  - A quad is created in a group (`render.create_quad{group = g, ...}`). With no `group`, it goes in the **base group**, which is always
    the bottom one, so existing scripts behave as today.
  - Groups are drawn in group order, and quads within a group in creation order. Painter order stays total and deterministic.
- **The calls:**
  - `render.raise_group(g)` puts the group on top, and `render.lower_group(g)` puts it at the bottom, above the base group.
  - `render.set_group_visible(g, bool)` shows or hides every quad of the group. A hidden group is skipped when the frame is built.
  - `render.set_group_offset(g, x, y)` translates every quad of the group by whole points. Dragging a window is one call, not one per
    quad.
  - `render.set_group_clip(g, rect | nil)`: the group's quads are clipped to that rectangle.
    - **The rectangle is in absolute window points** and is not moved by the group's offset, so scrolling (changing the offset) keeps the
      clip still.
    - The renderer trims the destination and source rectangles exactly as `FrameQuad::place` already does at the window edge. No scissor
      state is needed, and both backends keep their batching.
  - `render.raise_groups{g1, g2, ...}` puts the listed groups on top, in that order, as one change.
  - `render.destroy_group(g)` destroys the group and all its quads.
- **Groups are flat** (r2, ck-reviewer-1's question 3): no nesting, one offset and one clip per group.
  - **A window is two groups** that the library keeps adjacent:
    - its **chrome** (body, border, title bar, close button), unclipped;
    - its **content** above it, clipped to the content rectangle, whose offset is the window position plus the scroll.
  - **Coherence comes from publication,** not from the engine knowing the pair. A scene frame is published once per callback, after it
    completes (`RenderEnd::completion_hook`), so every change made in one callback appears together:
    - **raise** is `raise_groups{chrome, content}`;
    - **drag** sets both offsets and the content clip;
    - **hide** sets both groups' visibility.

    No frame can show a raised chrome with unraised content, or a moved window with an unmoved clip.
  - Nesting is not needed in M8 (windows do not contain windows). It is out of scope.
- **Tint:** `create_quad{..., tint = {r, g, b, a}}` and `render.set_tint(quad, tint)`, a premultiplied multiply.
  - With it, one white glyph set draws text in any palette colour, and a button can show hover, pressed and disabled states.
  - The default is white, which is today's look.
  - Tint is a per-instance value in both backends' scene shaders.
- **Cost:**
  - Groups add an order index and an offset to each published frame.
  - A frame is still built in O(live quads) outside the publication lock, under `scene-design` §8.1's rules.
  - The renderer resolves group visibility, offset and clip while building the frame. The GPU sees ordinary quads.
- **Limits:**
  - `[render] max_groups` (default 4,096, range 1 to 65,536) caps the groups.
  - `max_quads` still caps the quads.
  - A refusal is a catchable Lua error, as for quads.

### 3.5 Input routing between the UI and the map (Proposal)

The main thread must decide **synchronously** who gets each pointer event. It cannot wait for Lua.

- **Hit regions.**
  - A group may carry hit rectangles: `render.set_group_hit(g, {rect, ...} | nil)`.
    - **A finite cap (r4):** at most **16 rectangles per group**, and at most **4,096 live hit rectangles across the scene**. These are
      fixed limits in `ck-render-2d`, like the scene's other limits. A call that would exceed either is refused with a catchable Lua
      error and changes nothing.
    - **Routing cost per event** is therefore at most 4,096 rectangle tests on the main thread, and a snapshot carries at most 64
      pointer events. The rectangles are published as a flat array in painter order (topmost last) with each frame. Hit testing reads it
      without a lock beyond the existing publication exchange.
    - They are in points before the group's offset, and are moved by it.
    - They are **intersected with the group's clip**, so a list item scrolled out of view is not hit.
  - The rectangles are published in the same frame as the quads, so what is hit is what is drawn.
  - A hidden group has no hit region.
- **Routing happens at the input tick** (§3.12), in the link's `input` step on the main thread, before the map controller's update.
  - Each event is routed against the frame the UI most recently published by that tick.
  - The capture state lives in the link, on the main thread.
  - Whether the UI can take a call at that moment does not matter.
  - **Down:** a `Down` inside a visible hit region goes to the UI.
    - The topmost group with a region there is the **owner group**, and the pointer is **captured by that group** until the matching
      `Up`.
    - Otherwise the gesture belongs to the map, as today (drag to pan).
  - **Move and Up** go to whoever holds the capture. With no capture, the pointer position goes to both: the UI gets it to show hover, and
    the map gets it only when it is outside every hit region.
  - **Scroll** goes to the topmost hit region under the cursor if there is one, otherwise to the map (zoom). A scroll during a capture goes
    to the captured group.
  - **Map hover:** the map's hover (`game_hover`) is suppressed while the cursor is over a hit region or while the UI holds a capture, so
    no tooltip is drawn under a panel.
- **Cancellation** (r2, ck-reviewer-1's question 2). A capture ends with a **`cancel`** event to the UI, never with a synthesized `up` or
  click, when:
  - the snapshot's `reset` is set (focus loss);
  - the published frame no longer gives the owner group a hit region, because it was hidden, destroyed or its regions removed;
  - **pointer detail overflowed** (`pointer_overflow`; `ck-input` keeps at most `POINTER_CAPACITY` = 64 events,
    `crates/input/src/lib.rs`) and the button is no longer held (`pointer_held` is false). The `Up` may have been lost.

  After a cancel, the rest of that gesture, up to the next retained `Down`, goes to nobody: not to the map, so a cancelled UI drag never
  turns into a pan.
- **Overflow while the button is still held** keeps the capture. The UI gets the current position as one `move`. This mirrors the map
  controller's own overflow rule, which re-syncs a drag from `pointer_position` and ends it when `pointer_held` is false
  (`map_app.rs`, the `pointer_overflow` branch).
- **A lost `Down`** (overflow with no capture) starts nothing. As with the map, a gesture begins only at a retained `Down`.
- **The UI's input** is a new `pointer` field in `frame_update`:
  `{x, y, held, overflow, events = {...}, over = group_token | false}`.
  - `events` lists the UI's routed events in order: `down`, `move`, `up`, `scroll` and `cancel`. Each event has its position in points
    and the **`group`** it was routed to: the owner group for a captured gesture, the hit group otherwise.
  - `over` names the topmost group under the cursor.
  - `overflow` repeats `pointer_overflow`, so the library can drop its own pressed state.
  - **Delivery** (r8; replaces r4's, which acknowledged at the accepted post):
    - **Routed exactly once.** Every pointer event still in `ck-input`'s ring at an input tick is routed exactly once, at that tick. The
      mapper is acknowledged immediately after the tick's `input` step (§3.12), so no event is offered twice.
    - **The map's share is applied at the tick.**
    - **The UI's share goes into the link's stash** (§3.12) and is delivered exactly once, in order, by the next accepted
      `frame_update`, unless it is dropped by the stash's own cap.
    - **Two places can drop the oldest detail,** each signalled, never silent:
      - `ck-input`'s ring (`POINTER_CAPACITY` 64, within one tick period), as `pointer_overflow`;
      - the UI stash (64 pointer events, while the UI is busy).

      Either sets `overflow = true` in the UI's next `frame_update`. Dropped events are never delivered to anyone.
    - **The overflow semantics are the cancellation and overflow rules above,** evaluated at the tick:
      - a capture whose button is up is cancelled;
      - a capture still held continues from the current position;
      - a lost `Down` starts nothing.

      On a stash overflow the routing has already been decided correctly at each tick; only the UI's view lost events. The library
      therefore drops its pressed and click eligibility and resyncs from `held`.
  - **What routing sees** (r3; replaces r2's "Latency" bullet, whose claim that the player never clicks something not yet drawn is
    withdrawn).
    - **The policy is the latest published UI state.**
      - Routing uses the hit regions of the scene frame most recently published by the UI's completion hook, read on the main thread
        at the input tick.
      - It does **not** use a presented or adopted generation. No display acknowledgement reaches `CodekarlUi`. The presenters'
        `drawn_generation()` (`crates/gpu-metal/src/presenter.rs`, `crates/render-2d/src/metal.rs`, `clear_only.rs`) is the last
        committed frame, not a displayed-frame acknowledgement, and M8 does not wire it in.
    - **Disclosed lead over the screen.** The published state can be ahead of the displayed one.
      - The renderer consumes the latest snapshot later, on its own thread. A frame being built may still draw the previous one: Metal's
        `SceneSink` builds outside the exchange, and its `before_exchange` test seam marks that interval.
      - **The lead has no wall-clock bound.** The scene's fixed snapshot storage (one latest publication, one consumer and one build
        snapshot, `docs/scene-design.md` §8.1) bounds **memory**, not time. The displayed frame can trail the published state for as long
        as display does not advance: a GPU or display stall, occlusion, minimising, or a slow presenter. Routing keeps following the
        published state throughout.
      - **The player-visible consequence:** a click can follow a UI state (a just-opened or just-closed panel) that the screen does not
        show yet.
      - **Accepted, and stated as a property of the design, not engineered away.** Routing against the displayed generation would need a
        per-frame display acknowledgement from both presenters to the main thread. M8 adds none.
    - **While the UI is busy** (an update outstanding, or the call places full), routing still happens at every tick against the UI's
      last published frame, which cannot change until the UI runs again. A `Down` is routed once, at its tick, and its owner group is
      then fixed by capture.
      - The UI receives its share later, from the stash.
      - The map never waits.
    - **Overflow and reset** follow the cancellation rules above, evaluated at the tick.
    - **Hidden or clipped targets** are judged by the published frame at the tick:
      - a group hidden by then has no region;
      - a region scrolled outside its clip by then is not hit;
      - a `Down` over such a spot goes to the next region down, or to the map if there is none;
      - for an existing capture, a hidden or removed owner region is a `cancel` (above).
- **Keys** stay global, as now: actions from `[input.bindings]`.
  - A focused window does not take keys from the map or the time controls in M8. Text entry would need that, and it is out of scope.
  - The one change is **Escape**, the `close` action (Firm, owner Q2): with a window open it closes the top window; with none open it
    requests the app's close as today.
    - **This needs no engine change.** The `close` press already reaches Lua as `close_request` with source `"key"`, and a callback
      that does not return `true` declines (`ui_link.rs`, module docs: a `Returned` that is not `true` is `Answer::Decline`).
    - The library's handler closes the top window and declines when a window is open, and accepts otherwise.
    - The close button (source `"button"`) still always quits.

### 3.6 From the UI to the game (Proposal)

- **`game.control(name)`** sends the same `Control` a key binding sends: `"time_pause"`, `"time_speed_1"` to `"time_speed_5"`,
  `"time_faster"`, `"time_slower"`, `"game_quicksave"`, `"game_quickload"` (`controls.rs`, `control_of`).
  - It returns nothing, and an unknown name is an error.
  - **It never blocks.** It is a send on the existing channel (`controls.rs`, `SimControls::send` ignores the send's result). After the
    simulation owner has ended, the control is dropped silently, exactly as a key press is then.
  - It goes through the same channel, so a click and a key are indistinguishable to the simulation and nothing new is journalled.
- **No other command path in M8.** Game commands from windows, such as building or trade, come with the systems in M9.

### 3.7 The look in the map's style (Proposal; the look itself pending the owner's description)

- **Owner Q1:** "I'll describe it". The owner describes the look, a mock-up page follows that description, and the owner approves it
  before slice 7. The bullets below are only the mechanism (nine-slice, palette and theme data), which holds whatever the look is.

- **The panels:**
  - a flat dark body (today's `#2a2119`, or the map's background);
  - a one-UI-pixel border in one light palette colour;
  - window title bars in a second flat colour, optionally with the map's two-tone dither.

  They are drawn from small PNGs: a 3×3 nine-slice of corners, edges and fill, stretched by quads.
- **Text** uses the pixel font, tinted from a small named palette (text, dim, highlight, warning).
- **Data:**
  - colours and nine-slice art live in `assets/ui/theme.toml` and `assets/ui/theme/*.png`;
  - the Lua widget library reads the theme;
  - a mod changes the look by changing data.
- **Before the theming slice,** the mock-up of the owner's description is approved by the owner, as the M7 previews were.

### 3.8 Moddability (Proposal)

- **UI modules:**
  - the UI VM gets a `require` that searches only beneath the entry script's directory (`scripts/ui/?.lua`, `scripts/ui/?/init.lua`);
  - it uses text chunks only, with no C modules and no default `package.path`.
- **The widget library** is ordinary Lua in `scripts/ui/lib/`: `text.lua` (moved from `main.lua`), `panel.lua`, `button.lua`,
  `window.lua` and `list.lua`. The game's own UI uses it, and so can a mod.
- **Out of scope:** mod discovery, ordering and distribution (a later milestone). M8 keeps everything a mod would replace as plain files.

### 3.9 Responsiveness (Proposal)

- **Retained drawing.** Quads persist; the UI touches only what changed (CK-197's rule, kept), and dragging a window is one offset call.
- **No work on idle updates.**
  - With a map, `frame_update` arrives at the update rate whether or not anything happens (`frame_api.rs`).
  - The library returns at once when the update has no pointer events, no size change and no relevant transitions. The completion hook
    then publishes nothing (`publish` "only when something changed").
- **A slow callback never stalls map input** (§3.12). It delays only the UI's own response, and its input waits in the bounded stash.
- **No engine debug counters.** Responsiveness is checked as M7 checked it: with the owner's visible run and the existing metrics records.

### 3.10 Headless runs and tests

- **Headless:** everything new is windowless-testable.
  - Groups, order, visibility, offset, clip and tint are tested on the scene's frame building and the backends' readback.
  - Routing and capture are tested on `CodekarlUi` with synthetic snapshots, as CK-198's hover tests are.
  - The widget library is tested in a UI VM with a fake `render`.
- **Window tests** run under the existing input layer, one on each backend, per slice that changes what is drawn or routed.

### 3.11 The place window and map clicks (Firm, owner Q4; mechanism Proposal)

- **Owner Q4:** "Place window": clicking a place's marker opens a window with its name, kind and rank. This is the first map selection.
- **A map click** is a gesture the map owns (§3.5) whose `Down` and `Up` are both retained and which **never became a drag**.
  - **The latch (r7, F1).** The map controller keeps a per-gesture flag, `dragged`, set at `Down` to false.
    - **It becomes true** the first time the anchor-to-current displacement (`drag_delta`, `map_app.rs`) is nonzero, that is, the first
      time the pointer crosses a whole map pixel from where the gesture began. **This counts even when the crop clamps the camera so that
      nothing visibly moves:** the gesture was a drag attempt.
    - **It never goes back to false.** A gesture that moves out and back to its start, so that `Drag.applied` is `(0, 0)` again at the
      `Up`, is still a drag and not a click.
    - The final displacement is not the criterion: it is net, not history (`map_app.rs`, `Drag.applied` holds only the latest total).
  - **Not clicks:** a cancelled gesture (reset), a gesture whose detail overflowed (`pointer_overflow`), and a gesture whose `Down` was
    lost. The §3.5 rules apply unchanged.
- **The hit** reuses CK-198's marker hit test (`PreparedMarkers::hit`, `map_app.rs` `hover_at`) at the `Up` position:
  - the location drawn on top there, from step 15;
  - none below step 15, and none between markers. A click on empty map selects nothing, and M8 has no deselect.
- **To the UI:**
  - `game.on_select(fn | nil)` registers the callback `game_select`, delivered by the UI link as `game_hover` is (latest wins, outside
    the reserved call place, retried at the next pass).
  - **The argument:** `{seq, id, name, kind, size}`:
    - `kind` is the location kind's stable **id** (`LocationKind.id`, `crates/map/src/scenario.rs`). The scenario data has no display
      name for kinds, and M8 adds none: the window shows the id, and display labels come with localisation or later data.
    - `size` is the location's authored size, 1–4 (`ck_map::Location.size`), which the owner calls rank;
    - `seq` is a click counter that increases with every map click that selects a place. A second click on the same place is a new
      selection, so a closed window can be reopened.
  - Two clicks before one delivery deliver only the later (latest wins).
- **The window:**
  - The library opens the place window, or raises it if open, and shows the latest selection: one place window, not one per place.
  - Escape and its close button close it.
  - It holds only data the scenario already has, so it is no gameplay.
### 3.12 Map input independent of the UI (Firm, owner Q6; mechanism Proposal)

- **The owner:** "keeping the map input and the ui seperate is important, i think we should require it". Map pan and zoom never wait on a
  UI Lua callback.
- **Today** (`crates/window/src/main_loop.rs`):
  - one scheduler opportunity per `[frame]` period posts at most one `frame_update`, and none while one is outstanding (`post_update`);
  - the snapshot is acknowledged only after an accepted post;
  - the map and the time controls consume it inside that post (`ui_link.rs`, `CodekarlUi::post`).
- **The input tick (proposal).** Every due scheduler opportunity becomes an input tick, whether or not an update is outstanding:
  1. the loop takes `mapper.snapshot()`;
  2. it calls a new `UiLink::input(&InputTick)` on the main thread, synchronously, which never waits. `InputTick` carries the tick's
     `dt`, the window metrics and the snapshot;
  3. it acknowledges the mapper at the snapshot's sequence, immediately and unconditionally;
  4. then, only if no update is outstanding, it posts `UiEvent::Update` as today, for the UI's own callback.
- **What `CodekarlUi::input` does at each tick, all on the main thread:**
  - **routes the pointer** (§3.5) against the UI's last published frame, keeping the capture state;
  - **updates the map controller** with the map's share: its routed pointer events and the map actions. Pan, zoom, hover (CK-198) and map
    clicks (§3.11) are computed here. The camera publication is unchanged;
  - **sends the time and save presses** to `SimControls` (`controls.rs`), so they act at their tick, not when the UI is next free;
  - **appends the UI's share to its stash.**
- **The stash,** which is what the UI has not yet been given, is bounded:
  - routed UI pointer events, ordered, at most 64; when full the oldest is dropped and `overflow` is set;
  - the latest pointer position, `held` and `over`;
  - per action, the latest `held`, with `pressed` and `released` OR-ed since the last accepted delivery;
  - transitions, ordered, at most 256 (`ck-input`'s `TRANSITION_CAPACITY`); when full the oldest is dropped and `overflow` is set;
  - `reset`, OR-ed.

  Its size is bounded by the action count (at most 64) plus 256 plus 64.
- **Delivery to the UI:**
  - `post(Update)` builds the `frame_update` table (`ui_link.rs`, `update_table`) **from the stash**.
  - The stash is cleared only when the VM accepted the post. A refused post leaves the stash for the next one.
  - `dt` and `elapsed` stay the time between accepted UI updates, as now.
- **Unchanged:**
  - the close action (the loop's `take_close_presses`, independent of acknowledgement);
  - the `[frame]` pacing;
  - eligibility: no ticks while unfocused, minimised or zero-sized, as no updates today;
  - latest-wins `game_time`/`game_hover`/`game_select` delivery;
  - the existing update metrics, which keep measuring UI deliveries.
- **The result:** with the UI callback stuck, the camera keeps panning and zooming at the tick rate, and time presses act. With the UI
  free, behaviour is as today plus routing.
- **Headless runs** have no loop and are unchanged.

## 4. Packages and ownership

| Package | Change |
|---|---|
| `ck-render-2d` (+ `ck-gpu-metal`, `ck-gpu-vulkan` shaders) | Groups: order, visibility, offset, clip, hit regions in the frame; tint per quad |
| `ck-window` | The input tick: `UiLink::input`, acknowledgement at the tick, posting only the UI's update when free (§3.12) |
| `apps/codekarl` | `CodekarlUi::input`: map, controls and stash at the tick; the `frame_update` table from the stash; the `render` API calls; pointer routing and capture in `ui_link.rs`; hover suppression; `frame_update.pointer`; `game.control`; `[render] max_groups` |
| `ck-lua` | The rooted `require` for a VM (a VM option, not a game concept) |
| `scripts/ui/` | The widget library; the time bar, windows and theme use |
| `assets/ui/` | `theme.toml`, nine-slice and icon art; a white glyph set if tint replaces the coloured PNGs |

No new crate. No change to `ck-map`, `ck-sim` or the gameplay VM.

## 5. Milestone 8 slices (Proposal, dependency-ordered)

Every GPU slice lands on Metal and Vulkan together. "Windowless" means no GPU and no window.

### Slice 0: Map input independent of the UI (engine; owner Q6)

- The input tick in `ck-window`'s loop: `UiLink::input`, acknowledgement at every tick, and the UI's update posted only when free.
- `CodekarlUi::input`: the map controller and `SimControls` at the tick, and the UI stash with its caps.
- `frame_update` is built from the stash. There are no hit regions yet: every pointer event goes to the map, as today.
- **Done when:**
  - **windowless, `ck-window`** (fake backend, clock and link):
    - with an update outstanding, every due opportunity still calls `input` once and acknowledges the mapper;
    - no second update is posted while one is outstanding;
    - a key press reaches `input` exactly once;
  - **windowless, the application** (a real UI VM held mid-callback by the existing test `hold` extension, `vm.rs`):
    - while the UI is held, held pan keys and a drag move the camera at every tick;
    - a zoom-scroll zooms;
    - a time press reaches `SimControls` at its tick, exactly once;
    - after release, one `frame_update` carries every stashed transition in order, with the latches OR-ed;
    - 300 transitions while held keep the newest 256 and set `overflow`;
    - with the UI free, the existing CK-189/CK-193/CK-198 map, hover and controls tests pass unchanged;
  - **window tests, one on each backend,** under the input layer: with the UI VM held, injected pan keys move the map's camera (the
    existing map test observation); after release, the UI logs one update carrying the stashed transitions.

### Slice 1: Pointer routing to the UI

- Hit rectangles as a plain list published with the scene. Before groups exist, they are attached to the base group with `render.set_hit`.
- Routing and capture in `CodekarlUi::post`; map hover suppressed over the UI; `frame_update.pointer`.
- **Done when:**
  - a test UI script's rectangle receives a click, a drag starting on it does not pan the map, and a drag outside it does;
  - windowless cases:
    - capture holds through moves outside the rectangle;
    - `cancel` on reset, on the owner region's removal and on overflow with the button up;
    - a lost `Down` starts nothing;
    - each pointer event is routed once at its tick, and a UI-routed event reaches the UI exactly once across a refused and then an
      accepted post (from the stash);
    - routing during a held UI uses its last published frame, and a map-routed drag pans while the UI is held;
    - the 17th rectangle of a group and the 4,097th of the scene are refused;
    - map hover is suppressed over a region.

  Plus one window test on each backend.

### Slice 2: UI modules and the widget library (first half)

- The rooted `require` in `ck-lua`.
- `scripts/ui/lib/` with `text`, `panel`, `label` and `button`, where a button's hover and press come from `frame_update.pointer`.
- `main.lua` registers `frame.on_update`, relayouts on a size change and drops `WINDOW_WIDTH = 800`.
- `[ui] points_per_pixel` replaces `font_points_per_pixel` (owner Q3). The old key is refused, naming the new one.
- **The font without a scenario (r7).**
  - Today the font is loaded and the `font` extension installed only when `[game]` is configured (`lib.rs`, the `config.game.is_some()`
    branch).
  - The UI framework needs text without a game, so the font is loaded whenever a `ui` VM is configured, with the same startup refusal
    when it is missing or invalid.
  - `game` stays scenario-only.
- **Done when:** the library's layout is shown windowless as literal rectangles, and the tooltip flips against the real window width.

### Slice 3: The time-control bar (the first visible result)

- `game.control(name)`.
- A bar anchored top-left that replaces the readout's panel: the date and speed, plus buttons for pause, slower, faster and speeds 1–5.
- **Done when:** clicking the buttons changes the speed exactly as the keys do, a click on the bar never pans the map, and **the owner
  sees and uses it**.

### Slice 4: Scene groups (engine)

- Groups, `raise_group`/`raise_groups`/`lower_group`, visibility, offset and `max_groups` on both backends.
- **`set_group_clip` (absolute rectangle) on both backends (r7, F2: moved here from slice 6),** with hit regions intersected with the clip.
  Slice 5's windows need the chrome and clipped-content pair (§3.4).
- Hit rectangles move onto groups.
- **Done when:**
  - order, raising (including a multi-group raise in one publication), hiding and offsets are proven windowless and by readback on both
    backends;
  - **clipping:** a clipped group's quads are trimmed exactly at the clip, with destination and source both trimmed (readback); a clip is
    not moved by its group's offset; a hit rectangle outside its clip is not hit;
  - **the chrome and content pair:** one callback that sets both groups' offsets and the content clip publishes them in one frame.

### Slice 5: Windows

- `window.lua`: a title bar (drag to move, clamped), a close button, raise on click, and Escape closing the top window (§3.5, owner
  Q2).
- Map clicks and `game.on_select` (§3.11), and **the place window** as the first real window. A second window comes from the bar, for
  example the key bindings.
- **Done when:**
  - two windows overlap, raise, drag and close, and the map is never panned through a window;
  - a click on a marker opens the place window with its id, name, kind and size;
  - none of these opens it: a one-way pan, an **out-and-back drag** ending where it began, a drag clamped at the crop edge, a click on empty
    map, a reset or overflowed gesture;
  - dragging a window moves chrome, content and clip in one published frame.
  - one window test on each backend.

### Slice 6: Scroll lists

- `list.lua` with scroll-wheel scrolling, as a window's content group, on slice 4's clip.
- **Done when:**
  - a list longer than its window scrolls and clips exactly (readback);
  - an item scrolled out of view is not hit;
  - the scroll wheel over the list never zooms the map.

### Slice 7: Tint and theme

- After the owner's description (Q1) and the mock-up's approval:
- Per-quad tint on both backends.
- `theme.toml`, nine-slice art, the text palette and the white glyph set, after the owner approves a mock-up.
- **Done when:** readback shows tinted glyphs and the nine-slice at two UI scales, and the owner approves the look on the running game.

Slice 0 is the owner's precondition. Slices 1–3 are the small first visible result: a clickable time bar that does not fight the map.
Slices 4–7 give general windows.

## 6. Out of scope for M8

- Text entry, IME, clipboard and keyboard focus traversal.
- The right mouse button and context menus (the input layer has only the left button).
- Drag and drop between windows; docking; resizable windows.
- Rich text, several fonts or sizes, kerning, wrapping beyond single lines (labels are single lines; lists are many labels).
- Animation and transitions.
- Localisation.
- Mod discovery and loading.
- Every gameplay window: economy, pops and markets (M9 onwards).
- Selection beyond the place window: deselect, multiple selection, and selecting anything but places.
- Saving window layout (owner Q5).
- Engine debug counters or a UI profiler.

## Revision history (historical)

The design session's revision notes, kept as a record.

**Superseded by the answers above:**
- r6's "Question 6 (map input coupling) is still open";
- r7's conditional "if Q6 is answered 'decouple'". It became slice 0.
- `owner-questions-r2.md`'s recommendation to keep the coupling, which the owner overrode.

> **r2** answers ck-reviewer-1's source questions (`47nv3r3wbue2bgctiesv7h92as`):
> 1. map input is coupled to UI acceptance (§3.1, owner question 6);
> 2. pointer capture, cancellation and overflow (§3.5);
> 3. a window as two flat groups made coherent by per-callback publication (§3.4).
>
> **r3** answers ck-reviewer-1's follow-up (`k4c6dbn8p42d4tbfcdcbfm3df2`, `kfup72feratkpfxhqt2tnn3t46`). Routing uses the latest
> **published** UI state, not a presented generation. The lead over the screen is disclosed (r4: with no wall-clock bound), and the r2 claim of equality
> with what is drawn is withdrawn (§3.5, "What routing sees").
>
> **r4** answers ck-reviewer-1's `er5gcr9mdfupvwbrhy9mf3wzb6`:
> - no wall-clock bound on the lead over the screen;
> - exactly-once delivery of retained pointer detail, with explicit overflow;
> - a finite cap on hit rectangles (§3.5).
>
> **r5** is r4 plus the two slice 1 coverage cases r4's reply announced but the file lacked: exactly-once delivery across a refused
> post, and the cap refusals.
>
> **r6** folds in the owner's answers (`owner-answers-r1.md`, relayed by ck-manager `9rp8b6s84pce4f4a6ity7nh25a`):
> - Q1: the owner will describe the look;
> - Q2: Escape closes the top window;
> - Q3: one `[ui] points_per_pixel`;
> - Q4: a place window opened by a marker click (new §3.11);
> - Q5: no saved layout.
>
> Question 6 (map input coupling) is still open. *(Historical: answered; see "Owner answers", 6.)*
>
> **r7** answers ck-reviewer-1's design review 1 (`ui-design-review-1-ck-reviewer-1.md`, `d87c9270`):
> - **F1:** a map click is decided by a per-gesture latch, not by the final displacement (§3.11).
> - **F2:** group clipping moves to slice 4, before windows.
> - **Notes:**
>   - `kind` is the kind's id;
>   - `game.control` never blocks;
>   - the font is available without a scenario (slice 2);
>   - if Q6 is answered "decouple", an engine slice comes before slice 1.
>
> **r8** folds in the owner's answer to Q6 (`owner-answers-r1.md`, relayed by ck-manager `wudakkajpktfda6bg9fjei4rm6`): "keeping the map
> input and the ui seperate is important, i think we should require it".
> - **Map input never waits on the UI:** new §3.12, and slice 0 before slice 1.
> - §3.1, §3.5 and §3.9 change with it: routing happens at the input tick, and the UI's share waits in a bounded stash.

## Provenance

- **Owner:** the M8 direction, 2026-11-07, `m8-design/owner-direction.md`, relayed by ck-manager's request `3bezbvb548iy9me4qd9bzjskce`.
- **ck-reviewer-1's design review 1** (`ui-design-review-1-ck-reviewer-1.md`, `d87c9270`), answered in r7.
- **The owner's answer to Q6,** relayed by ck-manager `wudakkajpktfda6bg9fjei4rm6`, folded in by r8.
- **ck-reviewer-1's source questions** on r1 (`47nv3r3wbue2bgctiesv7h92as`), answered in r2; the follow-up on routing generations,
  answered in r3; and the follow-up on the lead, exactly-once delivery and the hit-rectangle cap (`er5gcr9mdfupvwbrhy9mf3wzb6`), answered
  in r4.
- **The accepted tree** at CK-205. Every path cited was read in the root checkout on 2026-11-07.
- **The owner's approval of the slice plan** ("start"), 2026-11-07, relayed by ck-manager `iw9x9dv3i85rp2s3faqi84buin`.
- **ck-reviewer-1's design review 2** (`ui-design-review-2-ck-reviewer-1.md`, `2a7dea89`), approving r8.
- **Author:** ck-designer-1 (Claude, Class S).
