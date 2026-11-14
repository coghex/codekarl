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
>   ledger row shows that place in the panel;
> - **the map-mode bar:** three icon buttons in one row, centred at the bottom of the screen and independent of the other panels (owner,
>   2026-11-10, CK-255), for the stockpile, yield and elevation map modes. They call
>   `game.set_map_mode`; the button of the mode showing is selected, and pressing it turns the mode off.
>
> **The owner's UI feedback (2026-11-10, after running the accepted game; `.pipeline/coordination/m9-design/owner-feedback-ui-r1.md`).**
> Accepted as CK-255 to CK-260; the owner's decisions are in the `owner-direction-*` files named below, in the same folder:
> - **The ledger** (CK-258; `owner-direction-ui-ledger-lists-r1.md`) replaces the places bar and its dropdown:
>   - closed (the start), it is a bar at the top right with two icon buttons, **locations** (a round tower) and **pops** (a figure);
>   - either button opens the **ledger**, a window titled "ledger" docked on the right third of the screen, from the top margin to the
>     bottom, with the two icons as tabs in its title bar and a close button that returns to the bar;
>   - **locations** lists every place in file order in columns (name, people, food in days); **pops** lists every pop at every place
>     (its type's colour stripe and icon, type and culture, people, place). A row shows that place in the location panel, a pops row on
>     its pops tab. Long names are cut on whole characters with "..". Since CK-295 that order is the default one, and each tab has
>     a list bar that filters and sorts it (§3.14);
>   - list items can have cells (`lib/list.lua`), and the pop-type colours and the dim text colour are `[colour]` keys of
>     `assets/ui/theme.toml` (`pop_peasant`, `pop_monk`, `pop_labourer`, `dim`). Labelled hover tooltips on the icons come next milestone.
> - **Pops and buildings as list elements** (CK-259; same record): the location panel's pops and buildings tabs show **cards** (the
>   default) or **rows**, switched by two icon buttons at the bottom of the panel; one choice covers both tabs of every place for the
>   session. A pop card has its type's colour stripe and icon, headcount, culture and religion, workers, dependents and trend, bars for
>   food, employment and the three needs, mood, and its housing and crowding; a building card has its icon, methods, wage, housing,
>   staffing and store bars, and herds. A row is one line, and clicking it opens its details beneath. Both looks use the new scroll view
>   `lib/cards.lua` (items of their own heights, clipped like a list). The location tab keeps its text lines.
> - **Window size and UI scale** (CK-257 and CK-256; `owner-direction-ui-size-r1.md`, `owner-direction-window-resizable-r1.md`,
>   `owner-direction-window-resize-behaviour-r1.md`): the game window opens at its minimum, **1280×800 screen pixels**, and can be
>   resized and made full screen (macOS standard: the green button or Control-Command-F). The UI and map lay out again when a drag ends.
>   The **UI scale** (§3.3) follows the window. See `docs/window-design.md` for the window itself.
> - **Superscripts** (CK-260; `owner-direction-superscript-r1.md`): the font has 3×5 superscript digits and signs (⁰–⁹, ⁺, ⁻), drawn
>   high, and subscript digits (₀–₉), drawn low, so text such as "m²" draws without markup (§2.4).
>
> **The UI polish round (2026-11-11; records in `.pipeline/coordination/ui-polish-design/`).**
> - **Goods icons** (CK-267; `owner-direction-goods-icons-r1.md` to `-r3.md`): each of the 22 goods has a full-colour 9×9 icon with no
>   outline and no tile (the owner: "A: no outline", "9×9 inline is right", then "Good, all 22 approved"):
>   - **the art** is drawn as letter grids in `assets/ui/goods/make_goods.py`, which writes the PNGs beside it; `--check` fails when an
>     image differs from the art or is missing. A test checks that its goods are exactly those of `assets/game/economy.toml`;
>   - **in the prices list** of the location tab the icon replaces the good's name: a line is the icon, then the price and its base in
>     brackets;
>   - **on herd lines** (cattle, pigs, sheep): on a building card each herd's head count and maximum follows its icon; in a building
>     row's details each herd has a line with its icon;
>   - **the fixed list:** `scripts/ui/lib/location.lua` names the goods that have icons (`GOODS_WITH_ICONS`), and a good not on it kept
>     its name line, so a good added later still read without an icon file (since CK-287 it gets the unknown good icon, below). The
>     same test checks that this list equals the generator's;
>   - **a list item's icon** (`lib/list.lua`) is either a theme icon tinted with a colour (`{name = ..., colour = ...}`, 9×11) or an image
>     drawn untinted (`{image = "goods/oats"}`, 9×9, from `assets/ui/`). Giving both, or neither, is an error.
>
>   With the names gone from the prices list a good is known by its icon alone until labelled hover tooltips arrive, which the owner
>   put in this round ("Yes, this round"). More goods come later, with merchants ("Later, with merchants").
> - **Two smaller fonts** (CK-268; `owner-direction-tooltips-r1.md` to `-r4.md`): the owner asked for "small and compact text" in
>   tooltips, chose font B and wanted font C supported "for certain circumstances", and approved both as drawn ("Both approved"). They
>   are the fonts **small** (a 9-row box) and **tiny** (an 8-row box), beside the standard 11-row font. A text line names its font
>   (§2.4). Tooltips draw with small, and with tiny where a span asks for it (CK-270, §3.13). How the glyphs were made is in
>   `docs/ui-art-provenance.md`.
> - **Middle and right mouse buttons** (CK-269; `owner-direction-tooltips-r1.md`, and `-r3.md` answer 7: "In this topic"): the
>   window now passes presses and releases of the middle and right buttons to the UI, as `button_down` and `button_up` events (§2.6,
>   §3.5). A middle press locks a tooltip when `[ui.tooltips] lock = "middle_mouse"` (CK-270, §3.13); nothing else acts on them.
>   The map never gets them.
> - **The tooltip system** (CK-270; `owner-direction-tooltips-r1.md` to `-r5.md`, `-manager-r3.md` to `-r5.md`, `-designer-r9.md` and
>   `-r10.md`; §3.13): one module, `scripts/ui/lib/tooltip.lua`, now shows every tooltip.
>   - **Delay and lock:** a tooltip opens when the pointer has rested on its target for 0.3 s. After a further 1.0 s there it **locks**,
>     shown by a yellow-orange bar filling along its bottom edge. The owner asked for three lock types, "disabled, middle mouse, and timer".
>     They and the two times are settings, `[ui.tooltips]` in the configuration file.
>   - **Nesting and closing:** a locked tooltip stays when the pointer leaves, and its linked words and numbers open tooltips of
>     their own. The owner: "escape should clear one nested tooltip at a time but clicking away should clear them all". A click on a
>     working control closes none.
>   - **Shared definitions and the glossary:** content comes from one definition per class of information (the owner: "one
>     definition called in two different ways"). Words with an entry in the new glossary, `assets/ui/glossary.toml`, are light blue
>     and open an explanation.
>   - **The look:** every number is green and a title is dark gold. Content of more than 12 lines scrolls inside the tooltip.
>   - **The map's press:** the UI now also hears a left press on the map, as a `map_down` event (§3.5).
> - **The glossary's entries** (CK-273; `owner-direction-tooltips-designer-r12.md` to `-r16.md`): `assets/ui/glossary.toml` now holds
>   its **92 entries**, the text the owner approved in five groups (`glossary/glossary-r1.toml`, sha256 `75ae9a2f…`). They explain the
>   game's concepts and name every good, production method, culture and religion of the game data. A change to an entry's text
>   is a data change that needs the owner's approval (§3.13).
> - **The location tooltip** (CK-275; `owner-direction-tooltips-manager-r3.md` and `-r4.md`, `-designer-r6.md` to `-r10.md`; §3.13):
>   hovering a place on the map now shows its name, population, majority culture and religion, usable land, and the value produced
>   and consumed last month. The owner: "mousing over a location should give basing info on a location". Each number opens its
>   breakdown and each word its glossary explanation. The content is seven shared definitions in the new
>   `scripts/ui/lib/classes.lua`: `location`, `population`, `cultures`, `religions`, `land`, `produced` and `consumed`.
> - **Label tooltips** (CK-271; `owner-direction-goods-icons-r3.md`: "Yes, this round"; §3.13): every icon button and the speed boxes
>   have a one-line label, and a shortened tab shows its full label. In the location tab a price row shows the good's name with its
>   price and base, so a good is no longer known by its icon alone, and a herd's icon shows its species. List rows can now have a
>   tooltip without being selectable (`lib/list.lua`, `tooltips = true`).
> - **The price breakdown** (CK-272; `owner-direction-tooltips-r2.md` to `-r4.md`; §3.13): in a price row's tooltip the price opens
>   a nested tooltip with the good's price, its base price, and the units bought and offered in the place's latest daily price step.
>   The owner chose breakdowns for **"Prices"** this round, in the **"Shorter"** form. The engine now keeps each place's daily buy
>   and sell orders for this; they are not saved (`docs/economy-design.md` §3.16).
> - **Tooltips on the bars, the ledger, the location tab and the window title** (CK-277; §3.13): the date and the map-mode buttons
>   show glossary explanations. In the ledger and on the location tab every word opens its explanation and every number its
>   breakdown, cell by cell, and the location panel's title shows the location tooltip. Seven more shared classes serve them
>   (`pop_size`, `place_food`, `housing`, `crowding`, `building_land`, `storage`, `overflow`).
> - **Tooltips on the pop and building cards and rows** (CK-278; §3.13): every field of a pop or building card, of a row and of an
>   open row's details shows its explanation or its breakdown: a pop's food by good, its jobs, housing and crowding, and a
>   building's staff, store, herds and the wage of its last twelve months. Eight more shared classes serve them (`pop_food`,
>   `jobs`, `pop_housing`, `pop_crowding`, `staff`, `store`, `wage`, `herd`).
>
>   That completes the tooltip topic: CK-268 to CK-278 are accepted.
> - **A crash fixed** (CK-279, 2026-11-13, reported by the owner running the accepted game): locking a tooltip that had a link or a
>   term ended the run, about 1.3 s after the pointer rested on a map place with the default settings. The hit key of a link or
>   term held a `:`, which `render.set_hit` refuses, and the UI-rig tests did not catch it because their stand-in for `set_hit`
>   accepted any key. Those keys are now short, valid and bounded, and the stand-in applies the real key rule (§3.13).
> - **Term and number colours in the panels** (CK-280; `owner-direction-colours-manager-r1.md` to `-designer-r7.md`; §3.7): the
>   ledger, the location tab, the cards, the rows and the time bar's date now use the tooltip's colours. A word that opens a glossary
>   explanation is light blue and every number is green. On a highlight fill a number is purple instead: dark `#663069` on the
>   location panel's cyan, light `#d4a8d7` on the red of a hovered or pressed ledger row. There is no good or bad colour: the
>   theme's `good` is removed. Lists opt in with `numbers = true`.
> - **A strict UI test rig** (CK-281; `owner-direction-ui-tweaks-manager-r1.md`: "fix the tests to make the stand-ins strict.";
>   §3.10): the rig's `render` and `texture` stand-ins now call the real host functions, so a call the real renderer refuses fails
>   in the tests too. Nothing the player sees changed.
> - **Numbers past a limit are red** (CK-283; `owner-direction-ui-tweaks-manager-r1.md` and `-designer-r2.md`; §3.7): crowding over
>   100%, housing occupants over capacity, storage and a building's store over capacity, building land over its limit, and any
>   overflow loss are drawn in `#f66155`, in the panels and in their tooltips. On the location panel's light fills the colour is
>   `#4b0b06`. A number exactly at its limit stays green.
> - **The building card's words** (CK-284; `owner-direction-ui-tweaks-manager-r1.md` and `-designer-r2.md`; §3.13): the card's
>   `staff` and `store` are now `positions` and `storage`, light blue words that open their glossary explanations. Their bars get
>   shorter to make room, and on a card too narrow for everything the line drops its bars, then `storage`, then the count, never
>   cutting a word. The open building row and the rows header use the same words.
> - **Closing the window with a tooltip open** (CK-285, from the owner's report; §3.4 and §3.13): it no longer ends the run as
>   failed. The group calls and `render.set_hit` now end with the quad calls, when the resource service closes, not when the
>   scene is detached at the end of the window loop. The tooltip's overlay is kept on top through `lib/ui.lua` where groups are
>   created or raised, so an idle update with a tooltip open makes no scene call.
> - **The market view** (CK-282; `owner-direction-ui-tweaks-manager-r1.md` and `-designer-r2.md`; §3.13): the prices on the
>   location tab are a table with a header: each good's icon and name, its price, its base price and a **balance bar**. The bar
>   fills left in blue (`#4a7fd6`) when more is bought than offered and right in orange (`#d9702b`) when more is offered, from the
>   place's latest daily orders. The goods' names are back beside their icons. List rows can now carry a bar (`lib/list.lua`).
> - **The unknown good icon** (CK-287; `owner-direction-ui-tweaks-manager-r3.md` and `-designer-r5.md`; §3.13): a good or a herd's
>   species without its own icon is drawn with a 9×9 sack carrying a question mark, `assets/ui/goods/fallback/unknown.png`. The
>   owner: "goods without icons would get this icon (a questionmark over a ambiguous looking good)", then "i approve the sack".
>   - Such a good is now an ordinary row of the market table, and its name is a glossary term only when the glossary has an entry
>     for it.
>   - One function chooses the image, for the market rows and for herds, and the fixed list still decides which goods have their
>     own icon.
>   - All 22 goods and the three herd species have their own icon and a glossary entry today, so nothing the player sees changed.
>   - The art's record is in `docs/ui-art-provenance.md`.
> - **Labels never lock** (CK-286; `owner-direction-tooltips-manager-r17.md`; §3.13): a one-line tooltip that only names a control
>   is now its own kind, a **label**. The owner: "these sorts of simple descriptor tooltips should not be lockable, the yellow bar
>   shouldnt appear, they should be a different new type of tooltip."
>   - The buttons' and tabs' labels and the speed boxes' are labels: they open after the same delay, never lock, show no lock bar
>     and let a press through to the control under them.
>   - A tooltip that explains a word or breaks down a number locks as before, the map-mode buttons' and the date's among them.
>   - **No coloured number in a label** (CK-289, 2026-11-13; `owner-direction-round-end-manager-r1.md`): a label's line is drawn
>     wholly in the tooltip text colour, so the 3 of `speed 3` is no longer green. The owner: "numbers in labels are green, but i
>     would think we shouldnt have this".
> - **Filter and sort on every list** (CK-294 to CK-296, 2026-11-13; `owner-direction-lists-designer4-r1.md` to `-r5.md`; §3.14):
>   every list has two small icon buttons under it at the right, **filter** and **sort**. The owner: "i want two small buttons at
>   the bottom right side of lists, the filter button with a filter icon, and the sort button with an arrow down icon".
>   - **Each button opens a picker,** a small panel of choices for that list: checkboxes for the filter, a list of orders for the
>     sort. The owner named them: "these should not be called popups, popups will be a real feature we add in the future".
>   - **The filter** starts with nothing ticked and everything showing; a ticked box shows only the rows that match it, and a
>     count such as `10 of 94` appears beside the button. One "all" button clears the ticks.
>   - **The sort** starts in the data's own order, named "default". Choosing an order uses its first direction (text a to z, numbers
>     largest first), and choosing it again turns it round.
>   - **Which lists:** the ledger's places and pops (CK-295), and the location panel's market, pops and buildings (CK-296). The
>     market filters by the goods' tags, food, clothing and herd, which came with CK-294 (`docs/economy-design.md` §3.1).
>   - **Nothing is remembered:** the choices start again each time a list is brought up. The owner: "dont save it, just reset it
>     everytime the list is brought up".
>   - **Escape** closes an open picker before it closes a window (§3.5).
>   - One new module, `scripts/ui/lib/listbar.lua`, does all of it; the list and card widgets did not change. The art's record is
>     in `docs/ui-art-provenance.md`.
>
>   That completes UI polish item 2, the lists.
> - **The pause menu** (CK-297; `owner-direction-game-menu-manager-r1.md` and `-designer-r2.md` to `-r6.md`, dated 2026-11-13 and
>   2026-11-14; §3.15): Escape with nothing left to close now opens a menu, and no longer ends the game. The owner: "ok pause menu
>   with buttons for resume, settings, glossary, save/load, and quit to desktop".
>   - **The menu** is a window titled "Menu" in the middle of the screen, over a backdrop that dims everything behind it by
>     half. It has five buttons: "return", "settings", "glossary", "save / load" and "quit to desktop". "save / load" is
>     greyed and does nothing until its screen is built. "settings" and "glossary" were greyed too, until CK-301 and CK-300
>     (below).
>   - **It pauses and blocks:** opening it pauses the game. While it is open the pointer reaches nothing behind it, and the
>     game's own keys (time, quicksave and quickload, the map's keys) do nothing.
>   - **"return" and Escape** close it and leave the game paused. The owner: "return should return us to the game, but leave it
>     paused".
>   - **"quit to desktop"** ends the game at once, with no question. The game window's own close button still ends it too.
>   - **For scripts:** the button library has a width option and an inactive (greyed) state, and the game API has
>     `game.pause()` and `game.set_modal(on)` (§3.6).
>
>   That is the first step of UI polish item 3, the game menu.
> - **A scroll bar on every scrolling list** (CK-299, 2026-11-14; `owner-direction-glossary-reader-designer-r1.md` and `-r2.md`;
>   §3.16): a list, a card stack and a picker with more rows than fit now show a thin bar at their right edge while their
>   content is taller than their view. The owner chose "Wheel + scroll bar", the look "B2: grey thumb" and "Every scrolling
>   list".
>   - The thumb can be dragged, a press on the track moves one view, and the wheel works as before.
>   - While the bar shows, the rows are 5 pixels narrower. One new module, `scripts/ui/lib/scrollbar.lua`, is the bar.
> - **A list may choose its starting ticks** (CK-304, 2026-11-14; `owner-direction-save-load-designer-r2.md`; §3.14): the rule
>   that every list starts with nothing ticked is retired. The owner: "that rule is not applicable anymore". `listbar.new` takes
>   the boxes ticked at the start. With none given a bar still starts with nothing ticked, as the five shipped lists do.
> - **The settings file** (CK-298, 2026-11-14; `owner-direction-settings-designer-r1.md`; §3.17): the player's settings are kept
>   in `settings.toml`, which overrides the configuration file. The UI VM's `settings` table can set and reset the three tooltip
>   settings and the three map settings; a change acts at once and is written. Since CK-301 the settings window uses
>   those calls. Since CK-307 and CK-309 the file also holds the graphics settings and the player's key bindings (below).
> - **Named saves and unsaved changes, for scripts** (CK-302 and CK-311, 2026-11-14; `owner-direction-save-load-designer-r1.md`
>   and `-r2.md`; §3.6): the game API lists, writes, loads and deletes named saves, and the game knows whether it has unsaved
>   changes. The location panel's look buttons and the map-mode buttons now tell it of a change. The save/load screen is not
>   built yet (`docs/places-time-design.md` §5.3).
> - **Exact modifiers** (CK-315, 2026-11-14; `owner-direction-game-menu-manager-r9.md` and `-r10.md`): a key binding fires only
>   with exactly its modifiers, so Escape with Shift held closes nothing and opens no menu (`docs/scene-design.md` §10.1).
> - **The settings window** (CK-301, 2026-11-14; `owner-direction-settings-designer-r1.md` to `-r4.md` and
>   `owner-direction-settings-manager-r7.md`; §3.18): the pause menu's "settings" button now opens a window titled "Settings",
>   above the menu. The owner: "i think i want a tabbed window".
>   - **Three tabs** in a column down its left side: general (the three tooltip settings), map (the three map settings) and
>     input (the keys in force; read-only until CK-310, below). A slider or a choice acts at once and is written to the
>     settings file (§3.17), and "defaults" resets the tab that shows.
>   - **Its "x" or Escape** closes it and leaves the menu open and the game paused.
>   - **For scripts:** `button.flow` lays out a row of buttons that wraps, and a window made with `modal = true` is above the
>     menu's backdrop. Every other window now stays under the backdrop while the menu is open.
>   - **CK-318** (2026-11-14) gave the general tab's first slider its "show delay" label, which CK-301 had left out, and
>     put every slider where the mock-up draws it.
> - **Typed text, for scripts** (CK-314, 2026-11-14; `owner-direction-settings-designer-r3.md`; §3.19): a widget can now be
>   typed into. `window.set_text_input(on)` turns typing on, each update then carries the characters and edit commands typed,
>   and `scripts/ui/lib/typing.lua` gives them to the one widget that is editing. Only the Cocoa window gives typed text. The
>   number field (CK-306, below) is the first widget that uses it.
> - **The glossary reader** (CK-300, 2026-11-14; `owner-direction-glossary-reader-designer-r1.md` and `-r2.md`; §3.20): the
>   pause menu's "glossary" button now opens a window titled "Glossary", above the menu. The owner: "a column on the left with
>   a list of glossary terms alphabetically, and a larger column on the right, defaults to empty".
>   - A click on a title shows the entry on the right: its title in gold, then its text. A linked word has its term's tooltip,
>     and a click on it opens that entry in the reader.
>   - **For scripts:** a list made with `selectable = true` can keep one chosen row filled (`lib/list.lua`).
> - **The number field, for scripts** (CK-306, 2026-11-14; `owner-direction-settings-designer-r3.md` and `-r5.md`; §3.21):
>   `scripts/ui/lib/numberfield.lua` is a field that a whole number is typed into. A click starts the editing. Enter, Tab or a
>   click elsewhere applies the number, brought into its range. Escape puts the old number back and closes nothing. No shipped
>   screen has a field yet: the graphics tab will be the first.
> - **The graphics settings, for scripts** (CK-307, 2026-11-14; `owner-direction-settings-designer-r3.md` and `-r4.md`; §3.17):
>   the configuration and the settings file take `[ui] list_look` and `[window] width` and `height`, and the settings file
>   takes `[ui] scale`. `settings.resize(width, height)` resizes the game's window, and a change of `[ui] scale` now acts while
>   the game runs. No shipped screen calls them yet, and the game still opens at 1280 by 800.
> - **Key capture and live key bindings, for scripts** (CK-309, 2026-11-14; `owner-direction-keys-designer-r1.md` and
>   `-r2.md`; §3.17): `settings.capture_key` gives a script the next key the player presses, `settings.set_bindings` changes
>   the keys of the player's actions while the game runs, and the settings file keeps them in `[input.bindings]`. Escape's
>   action, `close`, is fixed. Since CK-310 the settings window's input tab calls them (below).
> - **Rebinding keys** (CK-310, 2026-11-14; `owner-direction-keys-designer-r1.md` and `-r2.md`,
>   `owner-direction-game-menu-manager-r9.md` and `-r10.md`; §3.18): the settings window's input tab now changes the keys.
>   Each action has two key slots, the look the owner chose: "lets do K1 for now".
>   - **A click on a slot** makes it wait for a key, and the next key pressed is set. Escape cancels the wait. Backspace, or a
>     right-click on the slot, empties it. The owner: "right click should clear, backspace while waiting".
>   - **A key that another action has** opens the prompt "Key in use", with "replace", "clear" and "cancel", and "swap" when
>     the slot had a key of its own. The owner: "it should pop up a popup for confirmation on replacement of the old value".
>   - **The menu's row** (Escape) is greyed and cannot be changed. "defaults" puts every key back.
>   - **For scripts:** `window.set_modal_overlay` gives a modal window a layer of its own, just under it.
> - **The text field, for scripts** (CK-303, 2026-11-14; `owner-direction-save-load-designer-r1.md`; §3.22):
>   `scripts/ui/lib/textfield.lua` is a field that a line of text is typed into, with a caret that Left and Right move. It
>   counts characters, not bytes, and takes only characters the project font draws. It shares its editing with the number
>   field through the new `scripts/ui/lib/fieldedit.lua`, so one field edits at a time, of either kind. No shipped screen has
>   a text field yet: the save/load screen will be the first.

## Owner answers (2026-11-07)

From `.pipeline/coordination/m8-design/owner-answers-r1.md`. **No question is open.**

1. **The look:** "I'll describe it". The owner describes the look, and a mock-up follows the description. Theming (slice 7) waits for
   both. The description is pending. *(Historical: given and mocked up; see "Delivered" above.)*
2. **Escape:** it closes the top window; with none open, it requests the close as today (§3.5). *(Superseded by CK-297: with
   nothing open, Escape opens the pause menu and the run goes on; see §3.15.)*
3. **UI scale:** one `[ui] points_per_pixel` for the font, borders and icons; the old key is refused, naming the new one (§3.3).
   *(Superseded on 2026-11-10: one UI scale still covers the font, borders and icons, but it follows the window, and
   `points_per_pixel` is retired in favour of `[ui] scale`; see §3.3.)*
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
  - **since CK-268 there are three fonts,** each with the same characters and a replacement glyph:

    | Name | File in `assets/ui/font/` | Box height | Glyph images |
    |---|---|---|---|
    | `standard` | `font.toml` | 11 rows | `glyphs/` |
    | `small` | `small.toml` | 9 rows | `small/` |
    | `tiny` | `tiny.toml` | 8 rows | `tiny/` |

    - `small.toml` and `tiny.toml` are in `font.toml`'s format with one more key, `height`. The loader refuses, naming the file, a
      missing `height`, one outside 5 to 11 or other than the 9 and 8 it expects, a glyph without exactly that many rows, and a
      missing character; the width (1 to 5) and equal-digit-advance rules are the standard font's. `font.toml` itself has no `height`.
    - `font.layout(text, name)` takes the font's name as an optional second argument and returns that font's `height` and textures.
      With no name it is the standard font, as before. Any other name is a Lua error naming the three.
    - `text.line(x, y, group, colour, font_name)` (`scripts/ui/lib/text.lua`) keeps an optional font name on the line, and `text.set`
      lays out with it; a line without one is in the standard font.
    - `assets/ui/font/make_font.py` writes the images of all three, and its `--check` covers all three folders.
    - The small and tiny glyphs are the owner-approved art, kept as `assets/ui/font/tooltip-fonts-art.txt`; a test checks every
      glyph of both files against it. Their provenance is in `docs/ui-art-provenance.md`;
  - the scale is the UI scale of §3.3 (since CK-256; it was `[ui] points_per_pixel`, which owner Q3 renamed from
    `font_points_per_pixel`, CK-208);
  - besides printable ASCII, the fada vowels and the middle dot, it has superscript digits and signs (⁰–⁹, ⁺, ⁻) and subscript digits
    (₀–₉), 3×5 glyphs drawn high or low in the box (CK-260); every other character draws the replacement glyph.
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
  - **Since CK-315** a binding fires only with exactly the modifiers it names, decided at the press of its main key.
  - `close` is reserved: its press requests the window close with source `key`.
  - The time and save actions become simulation controls (`apps/codekarl/src/game/controls.rs`).
  - The six map actions pan and zoom (`map_app.rs`).
- **The pointer** is the left button, moves and vertical scroll only (`crates/input/src/lib.rs`, `PointerEvent`). There is no right
  button and no text input.
  - **Since CK-269** `PointerEvent` also has `ButtonDown` and `ButtonUp`, each with a `PointerButton` (`Middle` or `Right`) and its
    position: a press and a release of those buttons. `Down` and `Up` are still the left button, and the one held flag
    (`pointer_held`) is still the left button's: the other buttons have no held state. Other mouse buttons are not reported, and
    there is still no text input. Both window backends report them: Cocoa (`rightMouseDown:`, `otherMouseDown:` for button 2 and
    their releases; a drag with either held still moves the pointer; a right click opens no context menu) and GLFW (buttons 1 and 2).
    §3.5 says where they go.
  - **Since CK-314** there is text input: what the player types reaches the UI as typed entries, a list apart from the key
    actions and the pointer's events (§3.19).
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
  - **UI units.** Every position and size is a multiple of the UI pixel, which is **2 UI units** (`ui.P`, `scripts/ui/lib/ui.lua`).
    In a window opened in points (tests and demos) a unit is a point; in a window opened in screen pixels (the game, CK-257) a unit is
    `s` screen pixels, where `s` is the UI scale (CK-256).
  - **One UI scale (Firm, owner Q3; follows the window since 2026-11-10, `owner-direction-ui-size-r1.md`):**
    - `s` is 1, 1.5 or 2, so a UI pixel is 2, 3 or 4 screen pixels, for the font, panel art and icons alike. It is the largest whose
      `1280·s × 800·s` screen pixels the window's drawable holds (1 from 1280×800, 1.5 from 1920×1200, 2 from 2560×1600), or a lower
      `[ui] scale` (1, 1.5 or 2) when the config sets one. Scale 2 on a 2× display is the look of the old default.
    - The UI's view is `⌊drawable pixels ÷ s⌋` units; pointer positions, hits and point-scroll amounts are converted to units at the
      same scale; a scale change reaches the UI as a resize. In a pixels window `window.open` returns the first view in units.
    - `[ui] points_per_pixel` is retired and refused, naming `[ui] scale`; `[ui] font_points_per_pixel` keeps its own refusal.
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
- **Scene calls after the window loop has ended** (CK-285, 2026-11-13, from the owner's report: closing the window with a
  tooltip open logged `run failed: ui callback frame_update failed: … render.raise_groups: the resource service has ended`).
  - **A UI callback can still be running or queued when the loop ends.** The loop keeps posting updates while a close request is
    outstanding, and a quit ends it at once. After the loop returns, the run waits up to 1 s for the calls still outstanding (the
    late-callback drain, `apps/codekarl/src/windowed.rs`), and only then closes the resource service.
  - **The scene bridge is detached as the loop ends,** before that wait. A scene edit made afterwards is accepted by the writer and
    its publication is discarded and counted (`crates/render-2d/src/bridge.rs`).
  - **One end for every scene call** (`apps/codekarl/src/render_api.rs`). The group calls and `render.set_hit` refuse with "the
    resource service has ended" at the same moment as the quad calls: when the resource service is closed, or when the scene
    writer has been ended. A detached bridge alone is not that end.
    - Before CK-285 the group calls and `set_hit` refused as soon as the bridge was detached, although the service was still
      open. A late update that raised a group, set a hit region or moved a window then failed, and the drain reported it as the
      run's failure.
    - A run without a resource service (only tests build one) has no end from the service.
  - **A limitation that remains, disclosed and not changed:** a callback still running or queued when the service itself closes.
    - That can happen after a quit, which skips the drain, or when a callback outlasts the drain. Nothing guarantees an
      outstanding callback has finished by then.
    - Its scene and texture calls then raise the same error, and its failure is reported once, as the late-callback rules
      require (`apps/codekarl/src/windowed.rs`, module documentation). The run was already ending and no state is lost.
    - Changing it means either not delivering queued calls after the end, or not counting that refusal as a failure. Neither
      is scheduled.
  - **Tests:** `group_and_hit_calls_share_the_scene_lifetime_across_detach_service_close_and_end` (`scene_api_tests.rs`) checks
    the nine calls in the three states. The owner's case is in the real chain (`ui_link/group_tests.rs`,
    `the_real_map_population_tooltip_locks_and_opens_its_nested_tooltip`: a locked tooltip, the bridge detached, one more
    update) and in the run closure with fakes (`windowed/tests.rs`,
    `a_scene_write_from_an_update_queued_behind_close_runs_after_detach_but_late_errors_still_fail`).

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
    - Otherwise the gesture belongs to the map, as today (drag to pan). Since CK-270 the UI is also told of that press, as a
      `map_down` event (below).
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
    `crates/input/src/lib.rs`) and the button is no longer held (`pointer_held` is false). The `Up` may have been lost;
  - **the pause menu's key block turns on** (CK-297, §3.15): the first tick that sees the block on ends the gesture someone
    holds, as a focus loss does. A UI capture gets `cancel`, and a map drag ends.

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
  - **`button_down` and `button_up`** (CK-269) are in the same list, in order: a press or release of the middle or right button,
    with `button = "middle"` or `"right"`, its position, the hit key under it (`false` over none, such as the map) and `inside`.
    - They always go to the UI and never to the map, wherever the pointer is.
    - They are apart from the left button's gesture: they start, end and cancel no capture and no map drag, and `held` stays the
      left button's. There is no held flag and no `cancel` for them, so a consumer must accept a press with no release (focus lost,
      or the release dropped by an overflow) and a release with no press.
    - They have no effect of their own on hover: the event's position is an ordinary latest position, so the UI's `over` and the map's
      hover come out as they would for a move to the same spot.
    - The dispatcher (`lib/pointer.lua`) gives a keyed one to that key's widget like any event, and every widget ignores a kind it
      does not know, so a middle or right click presses, selects and raises nothing.
  - **`map_down`** (CK-270) is in the same list, in order: a left press over no hit region, the press that gives the gesture to the
    map. It is `{kind = "map_down", x, y, key = false, inside = false}`, at the press's position in UI units.
    - Nothing else changed: the map still gets the press and owns the gesture, and its moves and release go only to the map. A press
      over a hit region makes no `map_down`: the UI gets that one as a keyed `down`.
    - It has no key, so the dispatcher gives it to no widget. The tooltips read it: a press on the map closes them (§3.13).
    - The stash keeps it like any event that is not a move. One dropped by an overflow is never delivered.
  - **`functional`** (CK-270): the dispatcher sets `event.functional` on each keyed `down` before any widget hears it: whether that
    key's widget would act on the press (`pointer.functional`, §3.13).
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
- **Typed text** (CK-314) is three more fields of `frame_update`: `typed`, `typed_lost` and `typed_epoch`. The entries wait in
  the same stash as the pointer's events, and the library handles an update's typing before its pointer events (§3.19).
- **A key capture's result** (CK-309) is one more field, `capture`, in the update that carries one (§3.17).
- **Keys** stay global, as now: actions from `[input.bindings]`.
  - A focused window does not take keys from the map or the time controls in M8. Text entry would need that, and it is out of scope.
    - **Since CK-297 the pause menu is the one exception:** while it is open, the game's sixteen key actions reach neither the
      map nor the simulation. Every other action, `close` included, still arrives (§3.15).
    - **Since CK-314 typed text has a route of its own** (§3.19). It takes no key from anyone: a key typed into a widget is
      still recorded as a key, and its action still fires.
  - The one change is **Escape**, the `close` action (Firm, owner Q2): with a window open it closes the top window; with none open it
    requests the app's close as today.
    - **This needs no engine change.** The `close` press already reaches Lua as `close_request` with source `"key"`, and a callback
      that does not return `true` declines (`ui_link.rs`, module docs: a `Returned` that is not `true` is `Answer::Decline`).
    - The library's handler closes the top window and declines when a window is open, and accepts otherwise.
    - The close button (source `"button"`) still always quits.
    - **Since CK-270 an open tooltip comes first:** the handler closes the newest tooltip and declines, so one press closes one
      tooltip, and only with none open does it go on to the top window (§3.13).
    - **Since CK-295 an open list-bar picker comes next:** with no tooltip to close, the handler closes the open picker
      (`listbar.close_open()`) and declines. So the order was: a tooltip, the picker, the top window, the app's close (§3.14).
    - **Since CK-297 the last step is the pause menu, not the app's close.** `main.lua`'s handler is now `menu.close_request`
      (`lib/menu.lua`). For Escape it calls `window.close_request`, and when that has nothing left to close it opens the pause
      menu and declines. So the order is: a tooltip, the picker, the top window, the pause menu, and Escape never ends the run.
      The close button still always quits (§3.15).
    - **Since CK-301, while the pause menu is open,** "the top window" is the top one of the windows above the menu's
      backdrop: the settings window or the glossary reader when one is open, else the menu. Escape never closes a window
      under the backdrop (§3.18, §3.20).
    - **Since CK-306 a number field that is being edited comes first of all:** the handler puts its old number back, ends
      its editing and declines. So the order is: an editing number field, a tooltip, the picker, the top window, the pause
      menu (§3.21). **Since CK-303** an editing text field is the same step: the one field that edits, of either kind, gets
      its old value back (§3.22).
    - **Since CK-309 a key capture comes before the handler:** while a capture waits for a key, a press of Escape is the
      key it captures, and no close request is made (§3.17). **Since CK-310** that is how Escape cancels a key slot that
      waits in the settings window's input tab. The "Key in use" prompt, while it is open, is the top window that Escape
      closes (§3.18).

### 3.6 From the UI to the game (Proposal)

- **`game.control(name)`** sends the same `Control` a key binding sends: `"time_pause"`, `"time_speed_1"` to `"time_speed_5"`,
  `"time_faster"`, `"time_slower"`, `"game_quicksave"`, `"game_quickload"` (`controls.rs`, `control_of`).
  - It returns nothing, and an unknown name is an error.
  - **It never blocks.** It is a send on the existing channel (`controls.rs`, `SimControls::send` ignores the send's result). After the
    simulation owner has ended, the control is dropped silently, exactly as a key press is then.
  - It goes through the same channel, so a click and a key are indistinguishable to the simulation and nothing new is journalled.
- **`game.pause()`** (CK-297) sends a control that only pauses, on the same channel (`Control::Pause`; `core.rs`, `pause_only`).
  - A running game pauses with the reason `player`. A paused game is left as it is, its reason (`player`, `error` or `load`)
    included. It never resumes.
  - It takes no argument and returns nothing. An argument is an error.
  - **No key reaches it.** No input action maps to this control, and `game.control` has no name for it: only this call sends it.
  - **Why it is not `game.control("time_pause")`:** that one toggles. A script that toggled on the `paused` it last heard from
    `game_time` would resume the game when that value was out of date. `game.pause()` gives the same result either way.
- **`game.set_modal(on)`** (CK-297) turns the block on the game's own key actions on (`true`) or off (`false`). It takes exactly
  one boolean, returns nothing, and is off at the start of a run. The pause menu is its one caller (§3.15, which lists the
  actions and says what the block does).
- **`game.mark_unsaved()`** (CK-311) tells the game that the player made a choice that counts as an unsaved change
  (`docs/places-time-design.md` §5.3). It takes no argument, returns nothing and never waits: it is a send on the same channel.
  - **Its two callers:** the location panel's look buttons, when the look changes (pressing the look that shows makes no
    call), and the map-mode buttons, when the mode shown changes (a mode the map refuses makes no call).
  - Nothing else in the shipped UI calls it. The owner: "simply navigating the ui and windows will not trigger it".
- **The named saves** (CK-302; `docs/places-time-design.md` §5.3): five calls, each run by the simulation owner between ticks.
  **Each waits for the finished operation** and returns a table: `{ok = true}`, or `{ok = false, reason = "..."}` for a
  refusal.
  - `game.saves()` returns `{ok = true, entries = {...}}`: the saves in file-name order, each `{filename, name, fit, this_build}`
    with `written` and `date` when the save's header has them. `fit` is `"fits"`, `"version"`, `"content"` or `"unreadable"`,
    and `this_build` is `"yes"`, `"no"` or `"unknown"`.
  - `game.save(name)` saves the game as a new save under `name`. It never replaces a file.
  - `game.save_over(filename)` saves over a listed save and keeps its name. `game.load_save(filename)` loads one and leaves
    the game paused with `load`. Both refuse an entry that does not fit.
  - `game.delete_save(filename)` removes a listed save, fitting or not.
  - A wrong argument is a Lua error. A refusal is not: it is the table returned, and the game and its files are as they were.
  - After the simulation owner has ended, the answer is a refusal that says the game is no longer running.
  - No shipped script calls them yet.
- **`game.load_save_guarded(filename)`** (CK-311) asks for a load that the game refuses while it has unsaved changes. It
  returns nothing and never waits.
  - **The answer comes later,** to the callback that `game.on_save_result(fn | nil)` registers: `fn(filename, result)`, with
    `result` a table as above. A game with unsaved changes answers `{ok = false, reason = "unsaved"}`: nothing was loaded and
    no file was looked at. Answers are kept, in order, until a callback is registered.
  - **The game decides, not the script.** `game_time` has no field for unsaved changes. This request, `game.mark_unsaved()`
    and the save calls reach the simulation owner on one channel, in the order the script made them, so a mark made just
    before the request counts.
  - `game.load_save(filename)` is the load for a player who was asked and said yes: it never asks.
  - No shipped script calls it yet: it is for the save/load screen.
- **`window.set_text_input(on)`** (CK-314) is not a game call. It asks the window's main thread to turn typed text on or off,
  through the window's request slot, and never waits (§3.19).
- **The `settings` calls** (CK-298, CK-307, CK-309) are not game calls either. They are in §3.17: the settings in force,
  the window's resize, the player's key bindings and key capture.
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
- **Term and number colours in the panels** (CK-280; spec sha256 `3940599b…`; the owner's records are
  `owner-direction-colours-manager-r1.md` to `-designer-r7.md` in `.pipeline/coordination/ui-polish-design/`). The tooltip's two
  colours (§3.13) now hold in the panels too. The owner: "i still want consistent colors for glossary terms and numbers in the game
  menus, not jsut the tooltips" and "all numbers should be green, that's the numerical color, not just the tooltip ones".
  - **Number runs are shared:** `text.number_runs(s)` (`scripts/ui/lib/text.lua`) splits a string into pieces `{text, number}`.
    - A number starts at a digit, takes further digits, a `.` or `,` with a digit after it, and one following `%`.
    - Nothing else is a number: not a sign, `²`, `·` or `—`. `+3` gives only `3`, and `1,200` is one run.
    - `lib/tooltip.lua` uses it too, in place of its own copy, so tooltips draw as before.
    - `text.colour_numbers` and `text.colour_segments` turn a string, or pieces with their own colours and terms, into tinted
      pieces, and `text.set_segments(line, pieces)` draws a line whose pieces each have a tint.
  - **The rule:**
    - **a number** is drawn in a number colour, whatever colour the rest of its text has;
    - **a term,** a word whose tooltip is a glossary explanation, is drawn in `[colour] term` (`#92ddea`);
    - **everything else** keeps its colour: units (`d`, `days`, `L`, `m²`), punctuation, signs, `·` and other words.

    Only colours changed: layout, positions, hit areas and tooltip sources are as they were.
  - **The number colours, and no others** (CK-283 added two for a number past a limit, below). A numerical value is drawn in
    exactly one of:
    - `[colour] number` (`#5fb35a`), normally;
    - its kind's `highlight_number`, when it is drawn on that kind's highlight fill: its `outline` or `hot` colour behind text,
      which includes a list's hovered and pressed rows.

    | Kind | `highlight_number` | Its highlight fills |
    |---|---|---|
    | `place` | `#663069`, dark purple | cyan, with dark text on them |
    | `window`, `time_bar`, `places_bar` | `#d4a8d7`, light purple of the same hue | red, with light text on them |

    `theme.kind(name).highlight_number` gives it, and falls back to `number` for a kind without one (`tooltip`).
  - **No good or bad colour** (the owner: "i didnt realize there was a good/bad color, make sure that is not a thing"): no value is
    coloured by being high, low, positive or negative. The theme's `good` colour is removed. A pop's mood has its digits in
    `number` and its sign in `text`.
  - **Where it applies** (the owner chose **"All panels shown"**):
    - **the time bar's date:** the digits are `number`; the month, `·` and `:` stay `text`;
    - **the ledger:** the headings `name`, `people` and `food` are terms; people, food days and a pop's size are numbers. A place's
      name stays `text` (it opens the location tooltip), and a pop's type and culture keep the pop type's colour;
    - **the location tab:** each line's word is a term and its numbers are numbers, on every row of a wrapped line; the price rows
      and plain lines have their numbers coloured too;
    - **pop cards:** the type, culture, religion and the labels `food`, `job`, `needs`, `mood`, `housing` and `crowd` are terms.
      The size, the trend's numbers, the food days, the mood, the housing and the crowd are numbers;
    - **building cards:** the kind, `wage`, each method's name and the `housing` label are terms. The wage, the method levels, the
      housing count, the staff count and the herd counts are numbers. `staff` and `store` stayed `dim` in CK-280: they opened
      classes, not explanations, and the owner said to leave them for now. Since CK-284 the card says `positions` and
      `storage`, which open explanations and are terms (§3.13);
    - **rows:** a closed row's header and an open row's detail lines follow the same rule. An **open row's header** keeps its cyan
      band and dark words, and its numbers are the `place` kind's `highlight_number`.
  - **Lists** (`lib/list.lua`): `list.new{…, numbers = true}` draws the number runs of each cell and string item in `number` and
    the rest in the cell's colour. On the hovered and the pressed row the numbers use the list's kind's `highlight_number`; when
    the highlight moves, only the rows it leaves and reaches are retinted. Without the option a list draws as before. The ledger
    and the location tab's list set it.
  - **No gold in panels** (the owner's choice): window titles, buttons, tabs, the Help window, the speed boxes and the bars keep
    their colours. Title bars and pressed or selected buttons draw no numbers today; one drawn there later takes its kind's
    `highlight_number`.
- **Numbers past a limit are red** (CK-283; spec sha256 `4c3d9c00…`; `owner-direction-ui-tweaks-manager-r1.md` and
  `-designer-r2.md`). The owner: "this red color is for numbers that are currently hitting or surpassing limits", then, on the
  mock-up: "over 100 should be red, at 100 stays green, that goes for any value, things at their limit should stay green, red is
  for things that are past their limit". This adds a third and fourth number colour to the two above.
  - **The colours:** `[colour] over_limit = "#f66155"`, and `[kind.place] highlight_over_limit = "#4b0b06"` for a number on the
    place kind's light fills, where text is inverted. `theme.kind(name).highlight_over_limit` gives it and falls back to
    `over_limit` for a kind without one. No other kind sets one: no limited number is drawn on a red fill today.
  - **Past, not at.** A number takes the over-limit colour exactly when:

    | Number | Past its limit when |
    |---|---|
    | crowding | it is over 100% (`crowding_pm > 1000`) |
    | housing occupants | occupants > capacity, judged separately for basic and makeshift |
    | storage, and a building's store | used > capacity |
    | building land | in use > the smaller of prepared and buildable |
    | lost to overflow | units lost > 0: a loss means the store was full and more arrived |

    A number exactly at its limit stays green: `100%`, `300/300`.
  - **Only the number past the limit changes.** In `a/b`, `a / b` and `a of b` that is `a`; the capacity `b` stays `number`.
    Units and words are as above.
  - **Where it applies:**
    - **the location tab:** the crowding percentage, each housing occupant count, the storage used, the building land in use and
      the overflow units;
    - **a pop card's** crowd percentage and **an open pop row's** crowding;
    - **an open building row's** store used;
    - **tooltips** (the owner chose **"Yes, tooltips too"**): the classes `crowding`, `pop_crowding`, `housing`, `storage`, `store`,
      `building_land` (its `in use` line) and `overflow`.
  - **Not limits:** staff and herd counts, so a full `16/16` or `64/64` stays green. The ledger's numbers and an open row
    header's numbers have no limit.
  - **How:** a tooltip span may carry `over_limit = true`, and its number runs are then drawn in `over_limit` (any other type is
    an error). A list cell may carry `over_limit` too: its number runs are drawn in `over_limit`, or in its kind's
    `highlight_over_limit` while its row is hovered or pressed. `text.colour_segments` uses a segment's own `number_colour` when
    it has one.

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
- **The UI rig** (`apps/codekarl/src/readout_tests.rs`, the `Readout` rig that `widget_tests.rs` and others use) runs the shipped
  `main.lua` and `lib/` in a UI VM over a scene that records what it is asked to do.
  - **Its `render` and `texture` stand-ins are strict** (CK-281, 2026-11-13; the owner: "fix the tests to make the stand-ins
    strict."). A call the real extension refuses is refused in the rig with the real message, so it fails the test as it would end
    the game. That covers the argument count and types, the rectangle and colour rules, the hit key and hit rectangles, a texture
    path, and a quad or group that was never created or is already destroyed.
  - **No rule is written twice.** Each stand-in calls the real host function (`render_api.rs`, `texture_api.rs`) on a real scene
    writer and a headless resource service, and records the call only when it was accepted. A refused call records nothing.
  - **The rig's names stay.** Scripts and expectations still see the rig's stable names (`q1`, `g1`, `t:<filename>`). The rig maps
    a name to the real handle it stands for before the call, and a returned handle back to its name.
    - A destroyed name stays mapped to its stale handle, so a later use reaches the real stale check.
    - A name the rig never gave out is passed as a handle from another scene or service, which the real extension refuses as one
      it never issued.
    - A refusal's message is the real one, unchanged: where it prints a handle, it prints the real token.
  - **The stand-ins:** `render.create_group`, `raise_groups`, `create_quad`, `set_hit`, `set_position`, `set_rect`, `set_tint`,
    `set_group_clip`, `set_group_offset`, `set_group_visible`, `destroy_group`, `destroy_quad`, and `texture.load`. Stand-ins for
    other extensions (`game`, `window`, `frame`) are not covered.
  - **Tests:** `every_readout_render_and_texture_standin_matches_the_real_refusal` runs each refused call against the real
    function and the stand-in and compares the results; `strict_readout_standins_keep_the_accepted_call_records` pins what an
    accepted call records. Making the rig strict found no bad call in the shipped scripts.
  - **Why:** CK-279's crash passed the tests because the rig's `set_hit` accepted any key (§3.13).
- **The real chain's rig** (`apps/codekarl/src/ui_link/group_tests.rs`, `Rig`) runs the shipped `main.lua` and `lib/` over the real
  render extension, with the engine's hit test choosing the key under the pointer.
  - **`Rig::deliver_game_time(snapshot)` returns only after the `game_time` callback has run** (CK-286, 2026-11-13). It publishes
    the snapshot and delivers a fixture frame, then delivers a second frame and waits for its answer.
  - **Why two frames:** the link posts `game_time` after the first frame's update, and its answer can still be pending when that
    frame returns. The second frame is queued behind `game_time`, so its answer means the callback has finished.
  - **What it fixed:** a test that read the UI straight after the helper could run before the callback. In CK-286's first batch
    `the_open_pause_label_reads_play_after_a_game_time_update` failed that way in the combined check and passed when the same
    tree was run again; the integration review named it a test race, not a product fault.
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
    - **Since CK-297:** while the pause menu's key block is on, the tick first takes the blocked actions out of what the map
      and the simulation are given. The UI's share is not filtered (§3.15);
  - **appends the UI's share to its stash.**
- **The stash,** which is what the UI has not yet been given, is bounded:
  - routed UI pointer events, ordered, at most 64; when full the oldest is dropped and `overflow` is set;
  - the latest pointer position, `held` and `over`;
  - per action, the latest `held`, with `pressed` and `released` OR-ed since the last accepted delivery;
  - transitions, ordered, at most 256 (`ck-input`'s `TRANSITION_CAPACITY`); when full the oldest is dropped and `overflow` is set;
  - `reset`, OR-ed;
  - **since CK-314,** typed entries, ordered, at most 256 (`ck-input`'s `TYPED_CAPACITY`). When full the later ones are
    dropped and `typed_lost` is set. A tick with a newer `typed_epoch` discards the entries gathered before it (§3.19);
  - **since CK-309,** one key-capture result, the latest (§3.17).

  Its size is bounded by the action count (at most 64) plus 256 plus 64, and since CK-314 256 typed entries.
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

### 3.13 Tooltips and the glossary (accepted, CK-270)

One module, `scripts/ui/lib/tooltip.lua`, shows every tooltip (spec:
`.pipeline/coordination/ui-polish-design/specs/tooltips-t3-core-designer4-r8.md`, sha256 `09832bdf…`). CK-270 is the mechanism. The
glossary's entries came with CK-273, the labels with CK-271, the price breakdown with CK-272, the location tooltip with CK-275,
the bars, the ledger, the location tab and the window title with CK-277, and the pop and building cards and rows with CK-278.
Lengths below are in UI pixels (`ui.P` units each, §3.3) unless they say units.

- **Settings:** `[ui.tooltips]` in the configuration file, every key optional (the owner chose **"Config file now"**). Since CK-298
  the settings file can override each key, and a script can change one while the game runs (§3.17).

  | Key | Values | Default |
  |---|---|---|
  | `delay` | seconds before a tooltip opens: 0 to 2, in steps of 0.1 | 0.3 |
  | `lock` | `"timer"`, `"middle_mouse"` or `"disabled"` | `"timer"` |
  | `lock_timer` | seconds from opening to a timer lock: 0.3 to 3, in steps of 0.1 | 1.0 |

  - A value out of range, off the step or of the wrong type, and an unknown key, are refused at startup. The message names the file,
    `[ui.tooltips]`, the key and what is accepted (`apps/codekarl/src/config.rs`, `parse_tooltips`).
  - The UI VM reads them with `settings.tooltips()`, which returns `{delay, lock, lock_timer}` as they are in force when it is
    called. Since CK-298 the module reads them when it is loaded and again whenever a new target comes under the pointer, not at
    every update, so a change acts from the next tooltip target on.
- **Theme** (`assets/ui/theme.toml`): `[colour]` has `lock = "#f0a830"` (the lock bar), `term = "#92ddea"` (glossary words) and
  `number = "#5fb35a"` (numbers); `[kind.tooltip]` has `title_text = "#c9932e"`. The panel is theme kind `tooltip` with a cut corner
  of 3.
- **Content** is a table: an optional `title` string, then a list of lines.
  - A line is a string, or a list of spans. A span is one of:
    - a string: plain text;
    - `{text = …, term = <glossary key>}`: a **term**, which opens that entry's explanation;
    - `{text = …, class = <name>, args = <table>}`: a **link** to a shared definition;
    - `{text = …, tooltip = <function>}`: a link to one-off content, which the function returns.
  - A table span may add `font = "tiny"`. Everything else is in font `small` (§2.4). A line is as tall as its tallest span's font (9
    or 8).
  - **Colour follows what the text is, not whether it opens anything:**
    - the title is in the kind's `title_text` (dark gold) and is plain: it is never a term, a link or a target;
    - a term is in `term` (light blue);
    - in every other span, plain or link, each **number** is in `number` (green) and the rest in the kind's `text`. A label is
      the exception: its line has no number colour (CK-289, the label kind, below).
    - A number is a run of digits, with single `.` or `,` between digits and a `%` directly after: `412`, `96.5`, `4,210` and `73%`
      whole, and only the `12.5` of `12.5 km²`. So a link is found by hovering it, not by its colour. Since CK-280 the runs come
      from `text.number_runs`, which the panels share (§3.7).
  - A span naming more than one of `term`, `class` and `tooltip`, a font other than `small` or `tiny`, and a line or span of the wrong
    type are Lua errors starting `tooltip:`.
  - **No content:** nil, an empty table without a title, and content whose lines all have no spans (a line with no spans has no height
    and is left out). Nothing opens for it.
- **Shared definitions** (the owner: "one definition called in two different ways"):
  - `tooltip.define(class, function(args) return content end)` defines one class of information. Defining a class twice is an error
    naming it.
  - `tooltip.content(class, args)` returns that content, so a window or a list can show the lines a tooltip shows. An undefined class
    is an error naming it.
  - The game's classes are defined in `scripts/ui/lib/classes.lua` (the location tooltip, below) and, for `price`, in
    `scripts/ui/lib/location.lua`.
- **Targets:**
  - `tooltip.attach(key, source)` gives the widget under hit key `key` a tooltip, and `tooltip.detach(key)` takes it away. A key has
    one source: attaching again replaces it. `tooltip.label(key, words)` gives it a label, a tooltip that never locks (CK-286,
    the label kind, below).
  - `source` is `{class = …, args = …}`, or a `function(x, y)`. The function is given the pointer position in UI units and returns
    the content (nil for none here) and optionally a `part`, a string naming the part of the widget under the pointer. The **target**
    is the key with its part.
  - `tooltip.map_hover(hover, source)` sets the map's hovered place (`main.lua` passes `game.on_hover`'s value, nil for none). It is
    the target only while the pointer is over no UI key.
  - Every term and link of a **locked** tooltip is a target (below).
- **Each update:** `main.lua` calls `tooltip.update(update)` from `frame.on_update`, after `pointer.dispatch`. It reads `dt`, the view
  size and `update.pointer` (`over`, `x`, `y`, `events`). Positions are in UI units throughout, and it converts nothing. The events of
  an overflow packet are ignored, so a lost press closes nothing.
- **Showing:**
  - A tooltip opens **unlocked** when the pointer has rested on one target for `delay`. Another key or another part restarts the
    count. A function source is asked again as the pointer moves, to learn the part.
  - An unlocked tooltip closes as soon as the pointer leaves its target. It has no hit region, so the pointer acts on what is under it.
  - **Size:** the widest line + 6 wide, at least 9; the line heights + 4 tall; text from (3, 2).
  - **Placement,** fixed at opening:
    - for a UI key, its top-left is 6 right of and 12 below the pointer;
    - for the map's place, it is 4 units right of the place (`hover.right`), or 4 units left of it (`hover.left`) when the right side
      does not fit and the left does, centred on `hover.y`;
    - for a term or link, it is at the span's bottom-left, 2 lower.

    It is then moved inside the view and snapped to whole UI pixels.
- **Locking** (the owner's three types: "disabled, middle mouse, and timer"):
  - `timer`: from opening, the lock bar fills evenly over `lock_timer` while the pointer stays on the target. Full is **locked**.
  - `middle_mouse`: no bar while unlocked. A middle `button_down` (§3.5), whatever its key, locks the unlocked tooltip at once.
  - `disabled`: nothing locks and no bar is drawn, so nested tooltips cannot be reached.
  - **The lock bar** is one row, 1 inside the bottom outline (row `height − 2`), from x 1, `round((width − 2) × fill)` long and at
    most `width − 4`, so the cut corner stays outline. It is in theme `lock`, and full on a locked tooltip.
  - **A locked tooltip** stays when the pointer leaves its target. It gets a hit region: one key for its panel, and one for each term
    and link in it. Several tooltips from unrelated targets can be locked at once.
  - **Its hit keys** (CK-279) all satisfy `render.set_hit`'s rule, `[a-z0-9_]{1,48}`, however long the game runs:
    - a panel's key is `tooltip_panel_<n>`, counted up from 1. The module refuses to make another once `n` reaches 2⁵³ − 1, with the
      error `tooltip panel key space exhausted`;
    - a term's or link's key is `tooltip_span_<id>`: one module-wide decimal counter from 0, kept as a string so it stays exact,
      and at most 35 digits, which makes 48 characters. Past that the module refuses with `span hit key space exhausted`. It never
      reuses an id, so two spans never share a key, in one tooltip or across tooltips;
    - a span keeps its key for as long as it is shown, through scrolls and refreshes. The key is retired when its line leaves the
      view, or the span goes or changes what it opens.
- **Nesting:** resting on a term or link of a locked tooltip for `delay` opens its tooltip, which can lock in turn and has targets of
  its own. The code sets no depth limit; three levels are tested.
- **Closing.** Open tooltips form a stack, newest on top. They are drawn in one group, the **overlay**, which is kept above
  every other group (below).
- **The overlay stays on top without a call per update** (CK-285; spec `specs/close-error-manager-r3.md`, sha256 `6b43d08f…`).
  Until CK-285 `tooltip.update` raised the overlay on every update while a tooltip was open. That broke the idle rule (§3.9),
  made every such update publish a frame, and was the call that failed when the window closed (§3.4).
  - **The seam** is three functions in `scripts/ui/lib/ui.lua`:
    - `ui.set_top_group(group)` registers the group that is kept on top;
    - `ui.create_group()` calls `render.create_group()` and then, when a top group is registered, `render.raise_groups({top})`.
      It returns the new group;
    - `ui.raise_groups(groups)` makes one `render.raise_groups` call: the groups in the order given, with any occurrence of the
      top group taken out, then the top group once. With none registered it is exactly `render.raise_groups(groups)`.
  - **Every place that creates or raises a group uses it:** `main.lua` (the ledger bar and the map-mode bar), `lib/window.lua` (a
    window's two groups, and its three raises), `lib/list.lua` and `lib/cards.lua`. No file under `scripts/ui/` but `lib/ui.lua`
    calls `render.create_group` or `render.raise_groups`, and none calls `render.raise_group` or `render.lower_group`.
  - **The tooltip** creates its overlay with `ui.create_group()` at the first tooltip of a run and registers it at once. It stays
    registered for the rest of the run, and `tooltip.update` raises nothing.
  - **So the overlay is the topmost group at the end of every UI callback,** and a frame is published only when a callback has
    returned (§3.4). No frame shows a window above a tooltip, also when the window is opened or raised by a `game_select` or
    `game_time` callback and not by an update.
  - **An update with an open tooltip and nothing to change makes no scene call** and publishes no frame.
  - **The cost:** once a tooltip has been shown in a run, creating a group makes one more `raise_groups` call.
  - **Tests** (`apps/codekarl/src/readout_tests.rs`): the UI rig has a test-only observer of the host calls it makes, which
    sees `raise_groups` (the rig's log does not record it). `ui_group_seam_keeps_the_registered_group_last_and_observes_exact_host_calls`
    and `tooltip_open_is_quiet_and_game_callbacks_restore_the_overlay_in_the_same_call` use it; the real chain checks that ten
    idle updates with a locked tooltip publish no frame.
  - **Escape** closes one: `close_request` in `lib/window.lua` first calls `tooltip.close_top()`, which closes the newest tooltip and
    says whether it closed one. When it did, nothing else happens (§3.5). Since CK-286 it does not count labels (below). When it
    closed none, an open list-bar picker closes next, before any window (CK-295, §3.14).
  - **`tooltip.close_all()`** (CK-295) closes every open tooltip, locked or not, and drops a pending one. The list bar calls it when
    a filter or sort choice changes (§3.14).
  - **A left press** (the owner: "escape should clear one nested tooltip at a time but clicking away should clear them all"):
    - on a tooltip, its panel or one of its terms or links: closes the tooltips opened from it and keeps it and the rest;
    - on a **functional widget**: closes nothing;
    - anywhere else: closes **all** tooltips. That is a `map_down` (§3.5), a window's frame or empty body, a bar's background and any
      widget that is not functional.

    The press then does what it always did: a map press still selects.
  - **A functional widget** is one that would act on that press. `pointer.functional(key)` (`lib/pointer.lua`) is true when the key's
    registered widget has a `functional` method that returns true:

    | Widget | Functional when |
    |---|---|
    | a button (`lib/button.lua`: also tabs, close buttons, the map-mode and ledger buttons) | it has `on_click` and, since CK-297, is not inactive (§3.15) |
    | a list row (`lib/list.lua`) | its list has `on_select` |
    | a card item (`lib/cards.lua`) | the item at that index has `on_click` now |
    | a window's title (`lib/window.lua`) | the window is draggable and not `fixed`, or is `fixed` with `on_title_click` |
    | a time bar speed box (`main.lua`) | always |

    A window's frame key, scroll areas, keys with no widget and widgets with no `functional` method are not functional. No widget's
    own behaviour changed.
  - **Decided at the dispatch boundary.** A control's own press can remove it: a close button closes its window, and a row can
    rebuild its list, within one packet. So `pointer.dispatch` sets `event.functional` on each keyed `down` **before** it calls the
    owner's `on_child_down` or the widget's `on_event`, and `tooltip.update` reads that field and never asks again.
  - Closing a tooltip closes the tooltips opened from it.
- **Long content scrolls** (the owner chose a scroll bar and 12 lines). Content of more than 12 lines, the title counted, shows 12 at
  a time from line `first` (1 at opening).
  - **Fixed rows:** every line then takes a row of 9, whatever its fonts. Shown line `k` is at y `2 + (k − first) × 9`, a `tiny` line
    at the top of its row. Lines outside the 12 have no quads and no keys.
  - **The panel** is 12 × 9 + 4 = **112** tall, and the widest line of all the content + 10 wide. It does not change size as it
    scrolls.
  - **The scroll bar** is inside the right edge: a track 1 wide at x `width − 4`, from row 3, 105 long, in theme `dim`; and a thumb
    3 wide from x `width − 5`, `max(3, round(105 × 12 / lines))` tall, in the kind's `text`. The thumb's top is at
    `3 + round((105 − thumb) × (first − 1) / (lines − 12))`.
  - **Scrolling** needs a lock: a `scroll` event on a locked tooltip's panel or one of its span keys moves `first`, in
    `lib/list.lua`'s direction (a positive `amount` moves back toward line 1).
    - The tooltip keeps a fractional line count, `carry`. An event adds `−amount` for `unit = "lines"`, or `−amount ÷ (9 × P)` for
      `unit = "points"`.
    - `first` moves by the whole part of `carry` and the fraction is kept. `first` is held to `1 … lines − 11`, and a move stopped
      by either end drops the fraction.
  - An unlocked tooltip shows its scroll bar but cannot scroll.
- **Live** (the owner chose **"Live"**): `tooltip.refresh()` runs the source of every open tooltip again and redraws it in place, its
  top-left kept and its panel resized. `main.lua` calls it from `game.on_time`.
  - A tooltip whose content is now none closes, with those opened from it.
  - A long tooltip keeps `first`, held to the new length, and its `carry` returns to 0.
- **The glossary** is `assets/ui/glossary.toml`, loaded and checked at startup by `apps/codekarl/src/glossary.rs`.
  - **An entry** is `[term.<key>]` with `title` (a short name) and `text` (the explanation). The key matches `[a-z][a-z0-9_]*`.
  - **In `text`,** `{key}` is a term showing that entry's `title`, and `{key|words}` a term showing `words`, such as `{pop|pops}`.
    `{{` writes a literal `{` and `}}` a literal `}`.
  - **Refused at startup,** naming the file and the entry: a top-level table other than `term`, a key of the wrong form, an unknown
    key in an entry, a missing `title` or `text`, a `{…}` naming no entry, an unclosed `{`, a `}` with no `{`, empty `words`, and a
    character with no glyph in font `small`.
  - **The extension:** `glossary.entry(key)` returns `{key, title, text}` or nil, and `glossary.keys()` the keys in sorted order.
    - The file is read when the run has assets and a game or a UI VM, and the UI VM gets the extension only when the file exists.
    - A glossary file without the small font is refused at startup.
  - **The shipped file has 92 entries** (CK-273), after a comment giving the format.
    - **The text is the owner's approved text,** byte for byte and in its order: `glossary/glossary-r1.toml` (sha256 `75ae9a2f…`) in
      `.pipeline/coordination/ui-polish-design/`, assembled from the five groups the owner approved
      (`owner-direction-tooltips-designer-r12.md` to `-r16.md`). That file is not shipped.
    - **A change to an entry's `title` or `text` is a data change that needs the owner's approval.** It needs no code change.
    - **The owner's rules for an entry** (the same records): no "for example", no examples drawn from the work done so far and no
      local specifics; it describes the game's aims, not how today's code works; a name's entry is one plain line.
    - **The names' keys are the game data's ids:** every good, production method, culture and religion in
      `assets/game/economy.toml` has an entry under its id. A test reads the data file and fails when one is missing, so a good or
      a culture added later needs its entry.
    - **The location tooltip and its breakdowns are the first to show terms** (below). The reader screen for the whole glossary
      comes with the game menu.
  - **An explanation** is a term's tooltip: the entry's `title` as its title, then its `text` in font `small`, wrapped at 160 and
    broken at spaces, with its `{…}` terms as terms. It is a tooltip like any other, so a term inside it opens its own explanation. A
    term whose key has no entry opens nothing.
  - **Doubled braces in an explanation** (CK-289, 2026-11-13): outside markup, `wraps` in `tooltip.lua` draws `{{` as one `{`
    and `}}` as one `}`, as the file's comment says and the loader accepts.
    - Before CK-289 it drew `}}` as two braces, because it looked only for `{` (a mismatch reported on 2026-11-12).
    - A test (`widget_tests.rs`, `an_explanation_draws_doubled_braces_outside_markup_as_single_braces`) lays out `a {{b}} c` as
      `a {b} c`, and `{{{braces}}} {{x}}` as the term's title in braces, then `{x}`.
    - No entry uses braces yet.
- **Labels** (CK-271; spec `specs/tooltips-t4-labels-designer4-r4.md`, sha256 `fffd8dc0…`). The owner put labelled hover tooltips
  in this round ("Yes, this round") when the goods icons replaced the names in the prices list.
  - **Buttons:** `button.new{…, tooltip = "text"}` gives a button a one-line tooltip on its key, and `button:set_tooltip(text)`
    changes it (nil removes it; anything but a string or nil is an error). Destroying the button detaches it. Since CK-286 it is
    a tooltip of the label kind (below). The labels:

    | Control | Label |
    |---|---|
    | the ledger bar's two buttons, and the ledger's two tabs | `places`, `pops` |
    | the map-mode buttons | `stockpile map`, `yield map`, `elevation map` (replaced by explanations in CK-277, below) |
    | the time bar's rewind and fast-forward buttons | `slower`, `faster` |
    | the time bar's pause button | `pause` while running, `play` while paused |
    | speed box *n* (not a button: `main.lua` gives it one with `tooltip.label`; with `tooltip.attach` before CK-286) | `speed n` |
    | the location panel's look buttons | `cards`, `list` |

  - **Tabs:** `lib/tabs.lua` passes an item's `tooltip` to its button. A text tab shows its full label as a tooltip while it shows
    its short or tiny label, and has none while it shows the full one.
  - **Goods names:** `lib/location.lua` names a good by its id with `_` as a space, except `peas_beans`, which is `peas and beans`.
  - **Price rows:** in the location tab, each price row with an icon had a two-line tooltip: the good's name, then
    `price <price>, base <base>`. With CK-272 the price became a link (the price breakdown, below). CK-282 replaced that tooltip:
    the row is now a table row whose cells have their own sources (the market view, below). A good without an icon kept its
    text line, which had no tooltip, until CK-287 made it a table row too (the unknown good icon, below).
  - **Tooltip-only rows** (`lib/list.lua`):
    - a cell item may carry `tooltip`: a function returning content (or nil), or `{class = <name>, args = <table>}`. Any other type
      is refused, naming the item;
    - `list.new{…, tooltips = true}` gives every row in view a hit key, as `on_select` does. The row's source reads the item now at
      that row each time it is asked, so `set_items` changes what a row's tooltip shows without attaching again;
    - with `tooltips` and no `on_select` a row is **passive**: it passes a scroll to the list and ignores everything else, so it
      shows no hover or pressed look, selects nothing and is not functional (a press on it is a click away);
    - a row's key is detached when the row leaves the view and when the list is destroyed.
  - **Card items** (`lib/cards.lua`): every card's key has a source that calls the item's `tooltip(x, y)`, when it has one, with
    the pointer's position in UI pixels from the card's top-left. It returns the content and a part.
  - **Herds:** on a building card the species' name showed over a herd's 9×9 icon, with the species as the part; in the rows look
    an open building row did the same for each herd line's icon. CK-278 replaced that label with the species' explanation, and
    gave the rest of the card its tooltips (below).
- **The label kind** (CK-286; spec sha256 `362074cf…`; `owner-direction-tooltips-manager-r17.md`, sha256 `96eb0e7a…`) is a tooltip
  that only names its control. The owner: "some tooltips should be lockable, others should not be lockable", and "these sorts of
  simple descriptor tooltips should not be lockable, the yellow bar shouldnt appear, they should be a different new type of
  tooltip."
  - **Declared at the source:** `tooltip.label(key, words)` (`lib/tooltip.lua`) attaches a label to `key`.
    - `words` is a non-empty string. Any other call fails with `tooltip: label: expected a key and non-empty words`.
    - Nothing decides the kind from the content or its length.
  - **Who declares one:**
    - `button:set_tooltip(words)` (`lib/button.lua`) calls `tooltip.label`. So every button's `tooltip`, the tabs' tooltips and a
      shortened tab's full name are labels;
    - `main.lua` calls `tooltip.label(key, "speed " .. index)` for each speed box;
    - every other source is `tooltip.attach`, and lockable.

    | Source | Kind |
    |---|---|
    | the ledger bar's `places` and `pops` buttons, and the ledger's two icon tabs | label |
    | the time bar's `slower`, `pause` or `play`, and `faster` | label |
    | `speed 1` to `speed 5` | label |
    | the location panel's look buttons, `cards` and `list` | label |
    | a shortened text tab's full name (`location`, `pops`, `buildings`) | label |
    | the map-mode buttons: `main.lua` attaches their explanation after the button is made, which replaces its label | lockable |
    | the date readout | lockable |
    | the map's hovered place | lockable |
    | the ledger's rows and header, the location tab's rows and the market table | lockable |
    | pop and building cards, rows and open rows | lockable |
    | the location window's title | lockable |
    | terms and links inside a locked tooltip | lockable |

  - **What a label is:**
    - it opens after `delay`, where any tooltip of a UI key opens;
    - it is one line in font `small`, drawn whole in the kind's `text` colour. The words are plain, never a term or a link;
    - **no number in it is coloured** (CK-289, 2026-11-13; spec sha256 `e5f580df…`): the 3 of `speed 3` is not green, and
      nothing in a label is red past a limit. Until CK-289 a label's numbers were in `number`, as a lockable tooltip's still
      are. The owner (`owner-direction-round-end-manager-r1.md`, sha256 `a2599f61…`): "numbers in labels are green, but i would
      think we shouldnt have this, any label with numbers is likely to require the tooltip treatment". `speed 1` to `speed 5`
      stay labels: making a label that carries a number a lockable tooltip is left to the owner and is not built;
    - **it never locks:** the timer does not run for it, a middle press does not lock it, and in every `lock` setting no lock
      bar is drawn on it;
    - **it takes no input:** it has no panel hit and no span hits, so a press at a point it covers reaches the control under it;
    - it closes as soon as the pointer leaves its target;
    - **new words show in place:** an open label reads its key's current words. When time pauses, the pause button's open label
      changes from `pause` to `play` at the next update.
  - **Beside other tooltips:**
    - a label opens while tooltips are locked, on top of them, and does not close, move or change them. When it closes they stay;
    - **a middle press** with only a label open locks nothing: a label is never the unlocked tooltip that press locks;
    - **a left press** treats it as any tooltip: on a functional widget (its own button) it stays, and a press anywhere else,
      a `map_down` included, closes all, labels too;
    - **Escape skips labels:** `tooltip.close_top()` closes the newest tooltip that is not a label. With only labels open it
      closes nothing and returns false, so Escape does what it does with no tooltip open.
  - **A change of kind on a key** closes what the key had open. Each attached key records its kind, and each open tooltip the
    key it was opened for.
    - `tooltip.label` on a key with a lockable source, and `tooltip.attach` on a key with a label, close every open tooltip of
      that key in that call, locked or not, with the tooltips opened from it, and clear its pending one.
    - Nothing carries over: no lock, no lock time, no hit key, no bar. A pointer still resting there starts `delay` again under
      the new kind.
    - **The same kind again:** `tooltip.label` on a key that has a label only changes its words. `tooltip.attach` on a key that
      has a lockable source is as before: an open tooltip keeps the source it opened with until it closes.
    - **Removal:** `tooltip.detach` (and `button:set_tooltip(nil)`) on a key with a label closes its open label and clears its
      pending one in that call. So a shortened tab's open label closes when the tab comes to show its full name.
  - **The owner on the spec's open points** (2026-11-13, `owner-direction-round-end-manager-r1.md`). The spec listed them as
    open and built its defaults. The owner: "for the label-tooltip defaults, i agree, labels are different than tooltips and
    should not behave the same such as ignoring escape, with a label open escape should just bypass it, while with a tooltip
    the escape applies to that."
    - **Escape skipping labels** is confirmed, as built.
    - **Numbers in a label** were changed by CK-289 (above).
    - **Not settled:** every label waiting the full `delay`, also when the pointer moves straight from one labelled control to
      the next. The owner: "label wait full delay i dont understand, labels should dissapear with the mouse hover leaving". A
      label does close when the pointer leaves its target; the wait before the next label opens is still the spec's default.
    - **No separate delay setting for labels** is also the spec's default. The owner's record does not name it.
  - **Tests:** the UI rig (`apps/codekarl/src/readout_tests.rs`) has the call's refusals, a label under each `lock` setting, the
    changes of kind, live words, and a label beside a locked tooltip with Escape. The real chain (`ui_link/group_tests.rs`) has:
    - `a_speed_label_has_no_hit_and_the_real_hit_route_delivers_the_underlying_press`: with the label `speed 1` open, a press at
      a point that the label and speed box 2 both cover is handled by speed box 2;
    - `the_open_pause_label_reads_play_after_a_game_time_update`;
    - `a_speed_label_draws_its_one_line_in_the_tooltip_text_colour_with_no_number_colour` (CK-289): every run drawn for the
      open label `speed 3` has the tooltip text colour, and none the number or the over-limit colour. The test reads the three
      colours from the theme in the same VM.
- **The price breakdown** (CK-272) is the shared class `price`, with `args = {place = <id>, good = <id>}`, defined in
  `lib/location.lua`.
  - **Reaching it:** in a price row's tooltip the price was a link to it, so it opened from the locked tooltip. The owner asked
    to "hover over those numeirical values in the tooltip and get another tooltip showing the component values of that number".
    Since CK-282 it opens directly from a row's price and its balance bar (the market view, below).
  - **Its four lines** (the owner chose the **"Shorter"** form, `owner-direction-tooltips-r4.md`): `<good name>: price <price>`,
    `base price <base>`, `bought (buy orders) <units>` and `offered (sell orders) <units>`. The price is written as the row writes
    it.
  - **The orders** are the place view's `bought` and `offered` for that good: the units in the latest daily price step's buy and
    sell orders (`docs/economy-design.md` §3.11 and §3.16). A place with none yet, before its first step or after a load, shows `-`.
  - **Live:** it reads `game.place` at the newest economy revision the location panel was told (`location:set_revision`), so a
    refresh after a day shows the new price and orders.
- **The market view** (CK-282; spec sha256 `fb521a91…`; `owner-direction-ui-tweaks-manager-r1.md` and `-designer-r2.md`) is the
  prices part of the location tab, now a table. The owner: "it needs to be more readable than that … i want not only the numbers,
  but a bar that shows the deficit or surplus from buy/sell orders", going "left and blue when there is a deficit" and "to the
  right and color itself orange when there is a surplus"; on the mock-up the owner chose **"A: table with bar"**.
  - **A header row** replaces the line `prices (base):`. `good`, `price` and `base` are terms and open the explanations `good`,
    `price` and `base_price`; `buy/sell` is `dim` and opens nothing.
  - **One row a good,** in the view's order. For a content width `w`, in UI pixels from the content's left:

    | Column | Where | What |
    |---|---|---|
    | the icon | x 4 | the good's own icon, or the unknown good icon (CK-287, below) |
    | the name | x 16, left-aligned | the good's name, a term when it has a glossary entry (CK-287, below) |
    | the price | right-aligned at `w − 82` | the price, a number |
    | the base | right-aligned at `w − 56` | the base price, a number |
    | the balance bar | from `w − 48`, 46 wide | below |

    A name is cut on whole characters with `..` to end 4 before the widest price.

  - **The balance bar** is 3 high, 5 below the row's top, drawn in `[colour] track`, with a 1-wide centre tick in `dim` from 1
    above the bar to 1 below it.
    - With `bought` and `offered` both known and not both 0, `f = (offered − bought) / max(bought, offered)`, from −1 to 1, and
      the fill is `n = min(half − 1, round(|f| × half))` pixels, where `half` is half the bar's width, rounded down.
    - **A deficit** (more bought than offered) fills the `n` pixels left of the tick in `[colour] deficit`, `#4a7fd6`.
    - **A surplus** fills the `n` pixels right of the tick in `[colour] surplus`, `#d9702b`.
    - **No fill** when `n` is 0, when both are 0, and when either is unknown: before the place's first daily price step and after
      a load (`docs/economy-design.md` §3.11).
    - A hovered or pressed row keeps its bar's colours.
  - **What a row shows on hover,** by the cell under the pointer: the icon and the name, the good's explanation (nothing when the
    good has no glossary entry, CK-287); the price and the bar, the class `price` (the breakdown, above); the base, the explanation
    `base_price`. This replaces CK-271's two-line price tooltip.
  - **A good without its own icon** is an ordinary row with the unknown good icon (CK-287, below). CK-282 drew it as a plain line,
    `<good> <price> (<base>)`, with no bar and no tooltip. All 22 goods have icons today.
  - **Bars in lists** (`lib/list.lua`): a row item with `cells` may carry `bar = {x, width, value}`.
    - `x` and `width` are in UI pixels from the row's left, like a cell's `x`; `value` is nil or a number from −1 to 1.
    - It is checked before anything is drawn. `x` must be a whole number of at least 0, `width` a whole number of at least 3, and
      `value` nil or a finite number in range. Anything else is refused, naming the item:
      `bar needs whole x >= 0, whole width >= 3 and a value from -1 to 1 or nil`.
    - A fill of 0 pixels makes no quad. A bar follows its row through scrolling and `set_items`.
    - The row's tooltip function gets `cell = "bar"` when the pointer is over the bar and over no text cell (the row seam of
      CK-277, in the same units).
- **The unknown good icon** (CK-287; spec sha256 `248f0dc2…`; `owner-direction-ui-tweaks-manager-r3.md` and `-designer-r5.md`) is
  what a good, or a herd's species, without its own icon is drawn with. The owner: "we can use a unknown good icon, this can be
  helpful for out development, and any future modder usage, goods without icons would get this icon (a questionmark over a
  ambiguous looking good)"; then, after trying the variants on a mock-up, "the sack looks best, against my expectations, i approve
  the sack".
  - **The art** is a 9×9 tan sack tied at the neck, with a dark question mark, in the goods icons' own palette.
    - It is the grid `UNKNOWN` in `assets/ui/goods/make_goods.py`, kept apart from `ART` so that no good's id names it.
    - The script writes it to `assets/ui/goods/fallback/unknown.png`. A good's own image is always `goods/<id>.png`, so the
      subfolder keeps the two apart.
    - `--check` also fails when that image differs or is missing, or when `fallback/` holds any other PNG.
    - How it was made is recorded in `docs/ui-art-provenance.md`.
  - **One function chooses the image:** `good_icon(id)` in `lib/location.lua` gives `goods/<id>` for an id in `GOODS_WITH_ICONS`
    and `goods/fallback/unknown` for any other. The market rows and both herd places (below) use it.
    - No texture is loaded for an unlisted id's own file.
    - The unknown image is loaded once, however many rows and herds show it (`ui.texture` keeps each texture for the run).
  - **The fixed list decides** which goods have their own icon. The owner: "i think the default is fine, the modder friendly
    alternative is something that can be implemented another time".
    - A good added later gets the sack until its id is added to the list beside its image.
    - The alternative, deciding by whether the image file exists, is not built. `texture.load` returns at once and reports a
      missing file only later, after the magenta checkerboard has shown (`docs/scene-design.md`), and Lua has no call that says
      whether a file exists. Because one function decides, a later change touches one place.
  - **In the market table** a good without its own icon is an ordinary row: the unknown icon in the icon cell, then its name,
    price, base and bar as for any good. CK-282's plain line is no longer drawn.
  - **A name is a glossary term only when it has an entry,** so a word in `term` always opens an explanation (CK-280, §3.7):
    - with an entry (`glossary.entry(id)` is not nil), the name is the entry's title in `term`, and the name and the icon open
      the good's explanation;
    - without one, the name is the good's shown name (its id with `_` as spaces) in `text`, and the name and the icon open
      nothing;
    - the icon and the entry are independent: a good may have either, both or neither. The price, the base and the bar are the
      same for every good.
  - **Herds:** on a building card and on an open building row, a species not in the list is drawn with the unknown icon at the
    same place and size. Its icon opens the species' explanation when it has an entry and nothing otherwise; its count opens the
    class `herd`, as before.
  - **Today** all 22 goods and the three herd species (cattle, pigs, sheep) are in the list, and each has a glossary entry whose
    title equals its shown name, so nothing the player sees changed. The icon appears when a good is added without its own.
- **The location tooltip** (CK-275) is what the map's hovered place shows: `main.lua` gives `tooltip.map_hover` the source
  `{class = "location", args = {place = hover.id}}`. It waits `delay` and can lock like any other.
  - **Its classes** are in `scripts/ui/lib/classes.lua`. Each takes `args = {place = <id>}` and reads the place's view,
    `game.place(id, revision)` (`docs/economy-design.md` §3.11), at the newest economy revision `main.lua` has received
    (`classes.set_revision`, called from `game.on_time`). A place with no view yet gives no tooltip.

    | Class | Its lines |
    |---|---|
    | `location` | the title, the place's name; then the six lines below |
    | `population` | one line per pop type present, with its people; then the workers and the dependents |
    | `cultures`, `religions` | one line per culture (religion) present: its people and its share |
    | `land` | one line per terrain with usable land, in km²; then `not usable` and `sea` |
    | `produced`, `consumed` | `value at local prices:`, then one line per good with units: its value |

  - **The `location` lines,** each a word and a number: **population** and the people (workers and dependents), opening
    `population`; **culture,** the majority culture and its share, opening `cultures`; **religion** likewise, opening `religions`;
    **usable land** in km², opening `land`; **produced** last month and **consumed** last month, each a value opening its class.
  - **A number opens its breakdown and a word its explanation** (the owner chose this, `owner-direction-tooltips-designer-r6.md`).
    The words are glossary terms: population, culture, religion, usable land, produced, consumed, worker, dependent, and the names
    of pop types, cultures, religions and goods, shown by their entries' titles. Terrain names are plain text: terrains have no
    entries.
  - **Order:** every list is largest first. Equal people go in alphabetical order of the id, because the Lua UI cannot see the game
    data's order of pop types, cultures and religions. Equal values keep the goods' order of the view.
  - **Numbers:** a share is `floor(part × 100 / whole)` with `%`, so shares can sum below 100. An area is cells ÷ 4 in km², with at
    most two decimals and no trailing zeros. A value is `value_milli ÷ 1000` rounded to the nearest whole; the two totals sum the
    goods before rounding.
  - **Lines left out or empty:** with no pops there is no culture or religion line. Until a month has closed
    (`last_month.closed`), the produced and consumed lines show `-` and open nothing; a closed month with no activity shows `0`.
    `land` leaves out `not usable` and `sea` when they are 0.
  - **Consumed is the pops' use** (`used_by_pops`): buildings take no inputs yet.
  - **No county or region line** until the map data has those units (`owner-direction-tooltips-designer-r7.md`).
  - **Elsewhere:** the ledger, the location tab and the window title use these classes too (CK-277, below). `produced` can pass
    12 lines, and then scrolls.
- **The bars, the ledger, the location tab and the window title** (CK-277; spec `specs/tooltips-t8b-panels-designer-r3.md`,
  sha256 `87b4ad0b…`). The rule is the location tooltip's: a word opens its explanation and a number its breakdown.
  - **`tooltip.explanation(key)`** returns a glossary entry's explanation as content (nil when there is no entry), so a widget can
    show one without a term span.
  - **The date** in the time bar has a hit key of its own (`date_readout`) and shows the explanation `date`.
  - **The map-mode buttons** show the explanations `stockpile_map`, `yield_map` and `elevation_map`. `main.lua` attaches these
    over the buttons' CK-271 labels, which they replace, so these three stay lockable (CK-286, above).
  - **Seven more classes** in `lib/classes.lua`, each for a place (`args = {place = <id>}`; `pop_size` also takes `pop`, the pop's
    index in the place view):

    | Class | Its lines |
    |---|---|
    | `pop_size` | workers; dependents; births and deaths |
    | `place_food` | one line per pop, least food first: its type, its culture and its days of food |
    | `housing` | basic and makeshift, each occupants `of` capacity |
    | `crowding` | people; room (both capacities added); crowding in % |
    | `building_land` | in use, prepared and buildable, in m² |
    | `storage` | the pops' litres used and capacity; then one line per building with a store |
    | `overflow` | one line per good lost last month, most first; `nothing lost` when none |

  - **A list row's cells** (`lib/list.lua`, on CK-271's tooltip rows): an item's `tooltip` function is called as
    `tooltip(lx, ly, cell)`.
    - `lx` and `ly` are the pointer's position from the row's left edge and its top, in UI units.
    - `cell` is `"icon"` over the item's icon, else the index of the cell whose drawn text is under the pointer, else nil.
    - The function returns the content and a part. The game's rows give the cell as the part, so moving to another cell of the
      same row restarts the delay.
  - **The ledger** (`main.lua`), cell by cell:

    | Tab | Row | Cell and what it shows |
    |---|---|---|
    | locations | the heading | `name`, `people`, `food`: the explanations `location`, `population`, `food_days` |
    | locations | a place | name: `location`; people: `population`; food: `place_food` |
    | pops | a pop | icon and type: the type's explanation; people: `pop_size`; place: `location` |

    A cell with nothing drawn has no tooltip, and a class gives none for a place with no view yet. The rows still select as before.
  - **The location tab** (`lib/location.lua`): each line above the prices is a word and its numbers, as cells. The word opens its
    explanation and the numbers their class:

    | Line | Word (explanation) | Numbers (class) |
    |---|---|---|
    | people, workers, dependents | `population`, `worker`, `dependent` | `population` |
    | housing | `housing`, and `basic_housing` and `makeshift_housing` on their words | `housing` |
    | crowding | `crowding` | `crowding` |
    | food | `food_days` | `place_food` |
    | building land | `building_land` | `building_land` |
    | storage | `storage` | `storage` |
    | lost to overflow last month | `overflow` | `overflow` |

    A line too wide for the panel wraps as before, and each wrapped row keeps its own cells.
  - **The location panel's title** shows the `location` class for its place: the same lines the map hover shows.
- **The pop and building cards, rows and open rows** (CK-278; spec `specs/tooltips-t8c-cards-designer-r4.md`, sha256
  `18ea63ec…`). A card's or row's `tooltip` function (CK-271) answers by the field under the pointer, with the field's name as
  the part. Nothing about pressing, selecting, opening or the look changed.
  - **Eight more classes** in `lib/classes.lua`. A pop's take `args = {place, pop = <index>}` and a building's
    `{place, building = <index>}`, the index being the entry's place in the place view. `herd` also takes `species`. A class
    whose place, pop, building or species is not in the view gives none.

    | Class | Its lines |
    |---|---|
    | `pop_food` | one line per food the pop holds: the good, its units and unit, and its share as `P% of meals` |
    | `jobs` | `employed N`; `unemployed N` |
    | `pop_housing` | basic and makeshift, the pop's people in each |
    | `pop_crowding` | the pop's crowding in % (`—` when it has none); housing, its housed people |
    | `staff` | `filled N`; `open N` |
    | `store` | `used N L of N L` |
    | `wage` | the twelve months, newest first, then `average W` |
    | `herd` | the species, `N of N`: head of maximum |

  - **The wage lines:** a month is `<Mon> <year>: W per head (N paid)`. `<Mon>` is the month's three-letter English name, `W` the
    wage with one decimal, and `(N paid)` shows only when the paid heads are known. A month no payday has written has no year and
    reads `<Mon>: W per head`. With the average that is 13 lines, so the tooltip scrolls. The data is the view's `wages`
    (`docs/economy-design.md` §3.11).
  - **The food lines:** the unit is the data's, with `s` added when the units are not 1, unless it already ends in `s`, is `head`
    or holds a space. The share is the view's `share_pm` ÷ 10, floored.
  - **A pop card,** field by field:

    | Field | Shows |
    |---|---|
    | the icon and the type; the culture; the religion | each one's explanation |
    | the size, and the workers, dependents and trend text | `pop_size` |
    | the food bar and its days | `pop_food` |
    | the job bar | `jobs` |
    | the needs bars | the explanation `needs` |
    | the mood value | the explanation `satisfaction` |
    | the housing numbers | `pop_housing` |
    | the crowd value | `pop_crowding` |
    | the words `food`, `job`, `needs`, `mood`, `housing`, `crowd` | the explanations `food_days`, `jobs`, `needs`, `satisfaction`, `housing`, `crowding` |

    Needs and mood have no breakdown in this round.
  - **A building card,** field by field:

    | Field | Shows |
    |---|---|
    | the icon and the kind | the kind's explanation |
    | `wage N` | the word: the explanation `wage`; the number: class `wage` |
    | the methods text | the explanation of the method under the pointer |
    | `housing N` | the explanation `housing` |
    | `positions` (CK-284; it was `staff`) | the explanation `position` |
    | the positions bar and the count | `staff` |
    | `storage` (CK-284; it was `store`) | the explanation `storage` |
    | the storage bar | `store` |
    | a herd's icon | the species' explanation, whose title is its name (nothing without a glossary entry); it replaced CK-271's label |
    | a herd's count | `herd` for that species |

  - **Rows and open rows** (the list look of the pops and buildings tabs, and an open row's detail lines): the same fields give
    the same explanations and classes, by the cell or the detail line under the pointer.
  - **The building card's words** (CK-284; spec sha256 `3cd204b0…`). The owner: "staff should be positions or employees i would
    think, storage you could make an entry for", then chose **"1: positions, storage"** on the mock-up. The glossary already had
    the entries `position` and `storage`; the classes `staff` and `store` keep their names.
    - **The card's third line** reads `positions`, a bar, the count `filled/(filled + open)`, `storage`, a bar. Both words are
      terms and open their explanations; the bars and the count open the classes, as in the table above.
    - **It is laid out from real widths.** One function (`building_line_layout`, `lib/location.lua`) takes the card's width `W`
      and the font's widths of `positions`, of `storage` and of the count, and both the drawing and the tooltip source use its
      result. From x 5 each drawn element is followed by its gap: `positions`, 4, the positions bar, 4, the count, 12,
      `storage`, 4, the storage bar, and a right margin of 6. The room left for the two bars is `R`.
    - **The first step that fits is used:**

      | Step | When | What is drawn |
      |---|---|---|
      | 1, full | `R ≥ 34` | everything; the positions bar is 24 wide and the storage bar `R − 24` |
      | 2, shared | `12 ≤ R < 34` | everything; the positions bar is `floor(R / 2)` wide and the storage bar the rest |
      | 3, no bars | the words and the count fit | `positions`, the count, `storage` |
      | 4, no storage | `positions` and the count fit | `positions`, the count |
      | 5, positions only | `positions` fits | `positions` |
      | 6, empty | otherwise | nothing on the line |

      The usual card, with an ordinary count, is step 1: the look the owner approved. Only a narrow card or a very long count
      gives another step.
    - **Nothing is cut or squeezed:** text is never clipped, and a bar is at least 10 wide in step 1 and 6 in step 2. An element
      that does not fit is left out whole, no element ends past `W − 6`, and an element not drawn has no tooltip.
    - **An open building row:** its second line is `storage a / b L`, and its positions line `positions N filled, N open`. On
      each, the first word is a term and opens its explanation (`storage`, `position`); the rest of the line opens the class
      (`store`, `staff`).
    - **The rows look's header** reads `building`, `positions`, `wage`; it is `dim` and has no tooltip.
- **Tests:** the UI rig in `apps/codekarl/src/widget_tests.rs` drives `tooltip.update` with made-up updates;
  `ui_link/widget_witness_tests.rs` drives a locked tooltip through real pointer ticks; `config.rs` and `glossary.rs` test the
  settings, the loader and the shipped entries (92 keys, three exact literals, an entry for every id of the game data);
  `ui_link.rs` tests the routing of `map_down`. The location classes are tested in `widget_tests.rs` with a fixture place view, and
  `ui_link/group_tests.rs` hovers a place through the real `main.lua`. The labels, the tooltip-only rows and the herd tooltips are
  tested in `widget_tests.rs`, `ui_link/list_tests.rs` and `ui_link/location_tests/` (`tooltips.rs`).
  - **`ui_link/location_tests/` is a directory** (CK-291, 2026-11-13). Until then it was one file, `ui_link/location_tests.rs`,
    of 4762 lines, and two slices adding tests to it conflicted when their patches were replayed, three times on 2026-11-13.
    - `mod.rs` holds the shared fixture and helpers, and no test.
    - Nine files hold the 67 tests by subject: `selectors_tabs.rs`, `market_table.rs`, `tooltips.rs`, `cards.rs`, `rows.rs`,
      `panel_layout_refresh.rs`, `mode_bar.rs`, `ledger.rs` and `scale_limits.rs`.
    - The split moved the tests and changed none: each kept its name and its body.
  - **The UI rig's `render.set_hit`** (the recording stand-in in `apps/codekarl/src/readout_tests.rs`) refuses a key the real
    call refuses, with the same message (CK-279). Before that it accepted any string, which is how a span key holding `:` passed
    the tests and crashed the game. Since CK-281 every `render` and `texture` stand-in of the rig calls the real function
    (§3.10).
  - **Through the real render extension:** `ui_link/group_tests.rs` rests the pointer on a map place until the location tooltip
    locks, then opens its nested population tooltip (CK-279).

### 3.14 The list bar and its pickers (accepted, CK-295 and CK-296)

Every list has two small buttons under it at the right, **filter** and **sort**. Each opens a **picker**: a small panel of choices
for that list. The owner asked for them on 2026-11-13 (UI polish item 2): "i want two small buttons at the bottom right side of
lists, the filter button with a filter icon, and the sort button with an arrow down icon. these will act differently based on the
list they are working on."

- **The records** are `owner-direction-lists-designer4-r1.md` to `-r5.md` in `.pipeline/coordination/ui-polish-design/`, and the
  look the owner chose is `mockups/lists-r2.html` there.
- **The name** "picker" is the owner's choice of word: "these should not be called popups, popups will be a real feature we add in
  the future".
- **The slices:** the goods' tags are CK-294 (`docs/economy-design.md` §3.1), the module and the ledger are CK-295, and the
  location panel is CK-296.

Lengths below are in UI pixels.

- **One module,** `scripts/ui/lib/listbar.lua`, is the bar and both pickers. `lib/list.lua` and `lib/cards.lua` did not change.
  - `listbar.new{window, sorts, groups, on_change}` makes a bar for one open window. `sorts` is the sort picker's orders, each
    `{key, label, first}` with `first` either `"ascending"` or `"descending"`. `groups` is the filter's groups, each `{key, label}`.
    Since CK-304 it also takes `ticked`, the boxes ticked at the start (starting ticks, below).
  - **The bar does not know the list widget.** The caller gives it the list's rows, `bar:view(rows)`, and shows the rows that
    come back: a new array of the caller's own row tables, those that pass the filter, in the chosen order. A row carries
    `sort` (a string or a number for each sort key, or none) and `filter` (a string, or an array of strings, for each group key,
    or none).
  - **It uses the bottom 19 pixels** of its window's content area (`listbar.HEIGHT`), and the caller makes its list that much
    shorter. It is an attached item of its window, so it follows the window and goes when the window closes.
  - **The choices are the bar's alone.** A new bar has the default order and, unless its caller gives starting ticks, nothing
    ticked. Nothing of them outlives
    `bar:destroy()`, and nothing is written to a file. The owner: "dont save it, just reset it everytime the list is brought up".
- **The two buttons** are icon buttons in the window's kind, 13 by 15: `filter`, a funnel, and `sort`, an arrow down beside four bars.
  - The sort button's right edge is the content's right edge, the filter button's left edge is 16 pixels left of the sort
    button's, and their tops are 17 pixels above the content's bottom.
  - Their labels (§3.13) are `filter` and `sort`.
  - The filter button is drawn selected while any box is ticked, and the sort button while the order is not the default.
  - Pressing a button opens its picker, and pressing it again closes it.
- **The count:** while any box is ticked, the bar shows `N of M` left of the filter button: the rows showing, of the rows given to
  `view`. Its right end is 5 pixels left of the button and its top 2 pixels below the buttons' top. The numbers are in the theme's
  `number` colour and ` of ` in `dim`. With nothing ticked there is no count.
- **The filter rule** is the owner's: "the list starts unticked and showing everything, press a tick and it resets to a whitelist
  with the single checked box". Its first half, that every list starts unticked, was retired on 2026-11-14 (starting ticks,
  below); the rest stands.
  - With nothing ticked every row shows.
  - Otherwise a row shows when, in **every group that has a ticked box**, one of the row's values is ticked. Two ticks in one
    group show the rows of either; ticks in two groups show the rows that match both.
  - A row with no value in a group shows only while that group has nothing ticked.
- **Starting ticks (CK-304).** The owner retired the rule that every list starts with nothing ticked: "this is an exception to
  that rule, and that rule is not applicable anymore, i expect to add other exceptions like this in the future"
  (`owner-direction-save-load-designer-r2.md`). So a list may choose what its filter starts with, and a list that chooses
  nothing still starts with nothing ticked.
  - `listbar.new` takes `ticked`: a list of boxes, each `{group = <one of the groups' keys>, value = <a string>}`. Anything
    else is an error.
  - **A bar so made is as if the player had ticked those boxes by hand:** its filter button is selected, its count shows once
    it is given rows, "all" clears the ticks, and the filter rule is the ordinary one. A value that no row has is still a box,
    ticked.
  - The bar copies the list, so clearing it changes nothing for the next bar its caller makes from the same list.
  - **No shipped list uses it yet:** the five lists in the table below start with nothing ticked. It was built for the save
    list, which is to open showing this version's saves.
- **The order.**
  - **The default** is the order of the rows as given, which is the data's own. The owner: "start the lists with the data file
    order".
  - **A sort** is first in its `first` direction and, chosen again, in the other.
  - Numbers compare as numbers. Texts compare without regard to case, with á é í ó ú counted as a e i o u.
  - Rows with equal values keep the default order, in both directions.
  - Rows with no value for the chosen sort come last, in both directions, in the default order.
- **A picker** is a cut-3 panel (§3.7) in its window's kind, with no title bar.
  - **Where:** its right edge is the content's right edge and there is one clear pixel between its bottom edge and the buttons'
    top. The panel is 8 wider and 6 taller than what it holds, which starts 4 from its left edge and 3 from its top.
  - **Above its window, below tooltips:** it is made when it opens, as its window's last attached item (`window:attach` in
    `lib/window.lua`: attached items are drawn above the content, in attach order).
  - Its rows are 13 high. The row under the pointer is filled with the kind's `row_hover`, except the sort picker's chosen row,
    which keeps its own fill.
  - A press on a row or on "all" leaves the picker open. The wheel over a picker never scrolls the list under it.
  - **Too tall:** a picker's top is never above its window's content top. One with more rows than fit is as tall as that
    leaves, and its rows scroll with the wheel as a list's rows do, clipped. Since CK-299 it then has the scroll bar (§3.16):
    the panel is 5 wider, growing to the left, and the bar stands in that room beside the rows. A press on the bar does not
    close the picker.
- **The filter picker.**
  - **"all"** is a text button at the top left. It is drawn selected while nothing is ticked, and pressing it unticks every box. It
    stays in place when the rows scroll.
  - The rows start 16 pixels below the top of the panel's content, group after group.
  - **Headings:** with more than one group, each group starts with a heading row, its label in `dim`. With one group there is
    no heading.
  - **A box row** is a checkbox and a label 12 pixels in, and a press anywhere on the row ticks or unticks it. The checkbox is a
    7 by 7 outline one pixel thick in the kind's text colour; a ticked one has a filled 3 by 3 square at its centre. The owner
    chose the square over a tick mark: "i want the square".
  - **A group's boxes** are the distinct values of the rows last given to `view`, and any value that is still ticked though no
    row has it now. A box's label is its value with `_` drawn as a space, and the boxes are in label order.
  - The content is 4 wider than the widest of the "all" button, a heading, and a label with its 12 pixels.
- **The sort picker** is one row for "default" and one for each sort.
  - The chosen row is filled with the kind's `hot` colour and its text is in `pressed_text`. With the default order, "default" is
    the chosen row.
  - A chosen sort's row has a direction mark at its right, a 5 by 5 arrow: up for ascending, down for descending.
  - Pressing a row that is not chosen chooses it, a sort in its `first` direction. Pressing the chosen sort turns it round.
    Pressing "default" when it is chosen does nothing.
  - The content is 22 wider than the widest label.
- **A change** is a tick, an untick, "all" with something ticked, or a sort choice. The bar then:
  1. updates its two buttons and the open picker;
  2. **closes every open tooltip** (`tooltip.close_all()`, §3.13). A locked tooltip opened from a list row follows the row's
     position, and would show another item's data once the rows move;
  3. calls the caller's `on_change`, which shows the list again through `view`.

  Opening a picker is not a change and closes no tooltip.
- **One picker at a time,** in the whole UI: opening one closes the other. An open picker closes for:
  - its own button, or Escape;
  - a press on the map (`map_down`, §3.5), or a press on anything else in the UI that is not the picker itself. The press still
    does what it does;
  - a change of the game window's size;
  - its window closing, or its bar being destroyed.

  It stays open through a new economy revision: an open filter picker then shows the new boxes, at the same scroll. The
  events of an overflow packet (§3.5) close nothing. `main.lua` calls `listbar.update(update)` every frame, after the pointer
  dispatcher, for the presses and the size.
- **Escape** (§3.5) closes one thing a press, in this order: the newest tooltip that is not a label; else the open picker; else
  the top window. With nothing left it asked to close the app; since CK-297 it opens the pause menu (§3.15).
- **Hit keys:** every key the bar makes comes from `ui.key` with a short fixed prefix (`listbar_panel`, `listbar_box`,
  `listbar_sort`). A value, a label or a group key is data and is never part of a key: the host accepts only `[a-z0-9_]{1,48}`,
  and a label such as `base price` is not that (§3.13, CK-279).
- **The art:** the two icons and the two direction marks are grids in `assets/ui/theme/make_theme.py`, which writes
  `icon_filter.png`, `icon_sort.png`, `sort_down.png` and `sort_up.png`; its `--check` covers them. The checkbox is drawn from
  `white.png`. How they were made is in `docs/ui-art-provenance.md`.

**Which lists have a bar, and their choices.** The owner approved the table ("your table was fine, i approve"). "First" is the
direction a sort has when it is chosen.

| List | Sorts, in the picker's order (first) | Filter groups |
|---|---|---|
| ledger, places | name (a to z), people (most first), food (most first) | kind |
| ledger, pops | type (a to z), culture (a to z), people (most first), place (a to z) | type, religion, culture |
| location panel, market | name (a to z), price (highest first), base price (highest first), buy/sell (largest surplus first) | tag |
| location panel, pops | type (a to z), people (most first), food (most first), mood (highest first) | type |
| location panel, buildings | kind (a to z), positions (most first), wage (highest first) | kind |

- **The ledger** (`main.lua`, CK-295) has a bar on each tab, and its list is 19 pixels shorter.
  - **Places:** name is the place's name; people is its workers and dependents; food is its `food_days`; kind is the place's
    kind (in the shipped scenario monastery, ringfort, royal site and crannog). A place that shows no people and no food has no
    value for those two sorts, so it comes last.
  - The places tab's header row stays first. It is not sorted, filtered or counted.
  - **Pops:** type is the pop's kind, people its workers and dependents, and place the name of its place.
  - A row keeps its cells, its colours, its tooltips and the place it opens, wherever it is shown.
  - **The choices start again** when the ledger opens and when its tab changes. **They stay** across a new economy revision and
    a change of the game window's size.
  - After a change the list shows from its first row. A refresh for a new revision keeps the scroll.
- **The location panel** (`lib/location.lua`, CK-296) has a bar on each of its three tabs, in the panel's kind.
  - **The location tab:** the list is 19 pixels shorter. Only the market's goods rows are filtered, ordered and counted; the
    lines of figures and the market's header row stay above them.
    - name is the name the row shows, before it is shortened to fit; price is `price_milli`; base price is `base`; buy/sell is
      the balance bar's value (§3.13), and a good whose bar has none comes last.
    - tag is the good's tags. The boxes are the tags the place's goods carry, today clothing, food and herd. A good without a
      tag (herbs and beeswax today) shows only while no box is ticked. The owner: "untagged goods simply not showing up on those
      filters unless 'all' is selected (or no filters are selected)".
    - The owner means to move the market to its own tab next milestone, and the buttons go with it: "for now you can place the
      buttons below the market".
  - **The pops and buildings tabs:** the bar shares the 19-pixel strip under the stack with the cards-or-rows selector, which
    stays at the left. The stack's height did not change.
    - Cards and rows show the same items in the same order.
    - Each card and row is still built with the item's index in `place.pops` or `place.buildings`, so its tooltips, and which
      row is open, name the same pop or building as before.
    - The rows look's header stays first and is not counted.
    - food is the pop's `food_days`, mood its `satisfaction`, and positions the building's `filled`.
    - A tick that no item passes shows no card (in rows, only the header) and a count of `0 of M`. The lines `no pops` and
      `no buildings` are still for a place that has none.
    - An open row that a tick hides is open again when it shows.
  - **The choices start again** when the panel opens, when its tab changes (by a click, or by a ledger pops row that opens the
    pops tab), and when the panel shows another place.
  - **They stay** across a new economy revision, a change of the game window's size, the same place shown again on the same
    tab, and a change of look between cards and rows.
  - After a change the pops or buildings stack shows from its top. The location tab's list keeps its scroll, cut to its new
    length, so the market stays in view.
- **A known limit:** in a very narrow panel the count, and then the bar's buttons, can run under the selector's two buttons. It
  is disclosed, not designed for.
- **Not there:** remembering a choice, sorting by a click on a column heading, a search box, and keyboard use of a picker.
- **Tests,** all through the shipped scripts in the UI rig, with no window: `ui_link/listbar_tests.rs` (the module: the filter
  rule, the orders, the buttons, the count, both pickers, what closes one, the keys); `ui_link/location_tests/ledger.rs`
  (the ledger's two bars); `market_table.rs`, `cards.rs`, `rows.rs` and `selectors_tabs.rs` beside it (the location panel's
  three); and `ui_link/group_tests.rs`
  (`escape_closes_the_top_tooltip_then_the_open_list_bar_picker_then_the_top_window_one_each`).

### 3.15 The pause menu (accepted, CK-297)

Escape, with nothing left to close, opens the **pause menu**: a small window in the middle of the screen, over a backdrop that
dims the game. Before CK-297 that press ended the run. The owner asked for it on 2026-11-13, as the first step of UI polish
item 3 (the game menu): "ok pause menu with buttons for resume, settings, glossary, save/load, and quit to desktop."

- **The records** are `owner-direction-game-menu-manager-r1.md` and `owner-direction-game-menu-designer-r2.md` to `-r6.md` in
  `.pipeline/coordination/ui-polish-design/`. The look the owner chose is look A of `mockups/game-menu-r2.html` there, and it
  uses no new art (`game-menu-provenance-r1.md`). The spec is `runtime-m55/ck297-spec.md` (sha256 `ef260486…`).
- **One module,** `scripts/ui/lib/menu.lua`, is the menu, its backdrop and the Escape rule. `main.lua` gives it the view's size.
  - `menu.open(view)` opens it, `menu.close()` closes it, and `menu.is_open()` says whether it is open.
  - `menu.close_request(source, view)` answers the game window's close request, and `menu.relayout()` follows a change of the
    view's size.
  - `menu.LABELS` is the five labels in order, and `menu.current()` gives a test the open menu's parts.

Lengths below are in UI pixels.

- **What opens it.** `main.lua` answers every close request with `menu.close_request(source, view)`.
  - **Escape** (source `"key"`) first does what it did before, through `window.close_request` (§3.5): it closes the newest
    tooltip that is not a label, else the open list-bar picker, else the top window, one thing a press, and declines. Only
    with nothing left to close does it open the menu, and it declines then too. So Escape never ends the run.
  - **The game window's close button** (source `"button"`) is accepted as before, with the menu open or not: the run ends.
  - **Nothing else opens it.** There is no menu button on the screen. The owner chose "Escape last" over a button on the time
    bar.
  - **Without a scenario** there is no `game` table. The menu opens all the same and makes neither game call.
- **The window** is a library window (`lib/window.lua`) of kind `window`, titled "Menu".
  - It is `fixed`, so it has no close button, and it is not draggable. Its title bar has no action. The owner chose no close
    button ("No x") and "Centred, fixed".
  - It is centred in the view when it opens, and `menu.relayout()` centres it again when the view's size changes. `main.lua`
    calls that after its other relayouts. It also centres an open settings window (CK-301, §3.18) or glossary reader (CK-300, §3.20) again.
  - **At most one.** `menu.open` on an open menu only raises it and returns the same window. It pauses nothing again.
- **The five buttons** are a column that is the window's whole content, in this order: "return", "settings", "glossary",
  "save / load", "quit to desktop".
  - All five have the same width: the widest label's `font.layout` width, plus the button's own 4 of padding, plus 16. Each is
    15 tall, with its text centred.
  - There are 3 pixels between two buttons, and 9 above "quit to desktop".
  - "save / load" is one button, the owner's choice over two.
- **The greyed button.** "save / load" is an inactive button (below) until its screen is built, which is a later step. Its
  text is in `dim` and its outline in `track`. It has no hover or pressed look, and a press on it does nothing. The owner
  chose greyed buttons over hidden ones, and this look ("G1: dark outline") over a red outline.
  - **"settings" was greyed** until CK-301. It now opens the settings window, or raises it when it is open (§3.18).
  - **"glossary" was greyed** until CK-300. It now opens the glossary reader, or raises it when it is open (§3.20).
- **The backdrop** dims everything behind the menu and takes the pointer.
  - It is one quad of the theme's white texture over the whole view, tinted with the colour `body` at alpha one half.
  - It is in a group of its own, made when the menu opens and before the menu's window. So it is above every bar and window
    that existed then, and below the menu. The tooltip overlay stays above both (§3.13).
    - **Since CK-301 the window library keeps it there.** The menu names the group to it (`window.set_modal_backdrop`), and a
      window made or raised while the menu is open, other than one the menu opens, is put under the backdrop (§3.18).
  - **It is a hit region** over the whole view, under a key from `ui.key("menu_backdrop")`. Its widget takes every press, drag,
    scroll and hover and does nothing with them. So none of them reaches the map, a bar or a window behind it, and the map's
    hover is suppressed as it is over any hit region (§3.5).
  - `menu.relayout()` fits the quad and the hit region to the view's new size.
  - The owner chose "Pauses and blocks".
- **Pausing.** `menu.open` calls `game.pause()` (§3.6). A running game pauses with the reason `player`, and a game that is
  already paused, for any reason, is left as it is. This holds when a `game_time` is still on its way to the UI, for example
  after Space pressed just before Escape, because the simulation decides from its own state.
- **The game's keys do nothing while the menu is open.** `menu.open` calls `game.set_modal(true)`, and closing the menu calls
  `game.set_modal(false)`.
  - **The block names sixteen actions** (`BLOCKED_ACTIONS`, `apps/codekarl/src/game/controls.rs`) and nothing else:
    `time_pause`, `time_speed_1` to `time_speed_5`, `time_faster`, `time_slower`, `game_quicksave`, `game_quickload`,
    `map_pan_left`, `map_pan_right`, `map_pan_up`, `map_pan_down`, `map_zoom_in` and `map_zoom_out`.
  - **It is not a rule about keys.** The owner: "all ignored, but not as a rule, later we will have a grave key shell that will
    still need to work, only the game keys can be ignored". `close` (Escape) and every action not on the list still arrive.
  - **How:** the block is one flag in `SimControls`, off at the start of a run. Each input tick (§3.12) reads it
    (`ui_link.rs`, `key_block`). While it is on, the map and the simulation are given the tick's input without the blocked
    actions: none of their transitions, and each one not held, not pressed and not released. So a held arrow key stops
    panning the map and a speed key sends no control.
  - **The UI still gets everything.** The input kept for `frame_update` is not filtered, so a script still sees those actions.
  - **Only key actions are blocked.** `game.control(name)` from a script still sends its control.
  - **When it starts.** The block acts from the first input tick after the UI's call. A game key pressed in a tick before that
    one still acts.
  - **A gesture already under way ends.** The first tick that sees the block on ends the gesture someone had captured, as a
    focus loss does (§3.5): a map drag ends and a pressed widget gets `cancel`. The rest of that press reaches nobody. This
    happens at every opening, not only the first of a run.
- **"return" and Escape close it, and the game stays paused.**
  - "return" closes the menu's window. Escape with the menu open closes it too, as the top window. An open settings
    window (CK-301) or glossary reader (CK-300) is above it, and Escape closes that one first (§3.18, §3.20).
  - Either way the window's `on_close` takes the backdrop away (its quad, its group and its hit region) and turns the key block
    off. It sends no control, so the game is paused afterwards, whether or not it was running before the menu opened. It
    first closes the settings window and the glossary reader, when one is open.
  - Space or the time bar's play button resumes it, as before.
  - A second Escape, with nothing open, opens the menu again.
  - The owner: "resume should be replaced by return, and return should return us to the game, but leave it paused, whenever we
    pause with escape it stops the game timer to paused, and when we return it maintains the pause."
- **"quit to desktop"** calls `window.close()` at once: the run ends. The menu asks nothing and saves nothing. The owner chose
  "Quits at once" over a confirmation.
- **Unchanged:** the time bar, the ledger and its bar, the map-mode bar, the location panel and the Help window. The Help
  window's text still reads `esc: close window`.
- **The button library** (`lib/button.lua`) has two new things, which the menu uses.
  - **A width:** `button.new{…, width = points}` makes a text button at least that wide, with its text centred. It never
    makes one narrower than its text needs, and it holds when `set_text` changes the text. On an icon button it is an error.
  - **An inactive state:** `button.new{…, inactive = true}` starts a button greyed, and `button:set_inactive(true or false)`
    greys it or makes it live again.
    - **The look:** the fill of its kind as before, the outline in `[colour] track` (`#3a3326`) and the text or icon in
      `[colour] dim` (`#9a927e`), whatever the pointer does and whether or not it is selected.
    - **A press does nothing:** nothing is pressed and nothing is clicked. A press under way when the button is greyed is let
      go without a click.
    - **It keeps its `on_click`,** so making it live again needs nothing else.
    - **It is not functional** (`button:functional()`, §3.13) while inactive, so a press on it closes the open tooltips as a
      press on a panel's body does.
  - A button made without these options is sized and behaves as before.
- **Not there:** the main menu and the loading screen; what "save / load" opens; a confirmation
  before quitting; a menu button on the screen. Key rebinding came with CK-310, in the settings window (§3.18).
- **Tests,** none with a window or the GPU:
  - `apps/codekarl/src/ui_link/menu_tests.rs` runs the shipped `lib/menu.lua`, `lib/window.lua` and `lib/button.lua` in a UI
    VM over the real `render` API and the real input path. Ten of its tests cover the Escape order, the window and its
    buttons, one menu at most, the resize, the backdrop, the pause-only control, "return" and Escape, quitting, the greyed
    button and the two button options. It has twenty-six more for the settings window, fourteen of them for the input
    tab of CK-310 (§3.18).
  - `ui_link/input_tick_tests.rs` has the key block at the tick: each of the sixteen actions stopped at the map and the
    simulation with the rest left alone, a held map key, a press in the tick before, and a captured drag ended at a first
    and at a second opening.
  - `game/tests/presses.rs` has the list of blocked actions and the filtering, and `game/tests/core_behaviour.rs` the
    pause-only control from a running game and from each paused state.
  - **The two shipped UI cases** (`app.ui-witness`; `apps/codekarl/tests/support/time_bar.rs` and `places_list.rs`) used to end
    their run with a last Escape. That Escape now opens the menu, so each case then calls the menu's own "quit to desktop"
    action.

### 3.16 The scroll bar (accepted, CK-299)

Everything that scrolls rows of its own shows a thin bar at its right edge while its content is taller than its view: the
scroll list (`lib/list.lua`), the card stack (`lib/cards.lua`) and a list bar's picker with more rows than fit (§3.14). The
owner asked for it with the glossary reader (UI polish item 3) and chose:

- "Wheel + scroll bar": "Also a thin scroll bar beside the list showing where you are (a new control)";
- the look "B2: grey thumb": "The same dark track, a grey thumb, lit cream while hovered or dragged";
- "Every scrolling list": "Built once in the shared list widget, so every list that scrolls gets it, shown only when the list is
  longer than its view."

The option texts are ck-designer-1's and the choices are the owner's (`owner-direction-glossary-reader-designer-r1.md` and
`-r2.md` in `.pipeline/coordination/ui-polish-design/`; the picture is `mockups/glossary-reader-r2-b2-scale1.png` there).

Lengths below are in UI pixels.

- **One module,** `scripts/ui/lib/scrollbar.lua`, is the bar. `scrollbar.new{window, owner}` makes one, hidden. Its owner gives
  it three functions, `scroll()`, `set_scroll(pos)` and `wheel(event)`; calls `bar:update(view, content, scroll)` when the
  view, the content or the scroll changed; and calls `bar:place(x, y, content_clip)` when its place or the window's content
  clip did.
- **The look:** a track 3 wide and as tall as the view, in the theme's `track`, and on it a thumb of the same width in `dim`, or
  in `text` while the pointer is over it or it is dragged. Both are plain rectangles of `white.png`
  (`docs/ui-art-provenance.md`).
- **Shown only while the content is taller than the view.** A bar that is not shown has no quad and no hit region.
- **The thumb.**
  - Its length is the shown share of the content, view × view ÷ content, rounded to a whole pixel, at least 9 and never
    longer than the view.
  - Its top is travel × scroll ÷ the greatest scroll, rounded to a whole pixel, where the travel is the view less the thumb.
  - A view too short to give the thumb any travel has a thumb that fills the track.
- **Use.**
  - Dragging the thumb keeps the point that was grabbed under the pointer, clamped to the ends.
  - A press on the track above or below the thumb moves one view that way.
  - The wheel over the bar scrolls its owner.
  - A drag ends at the release, at a `cancel`, at a pointer overflow (§3.5), and when the bar goes or is destroyed.
- **Hit regions.** The bar has a group of its own that does not scroll, above its owner's rows. Its two keys come from `ui.key`
  (`scroll_track` and `scroll_thumb`): the thumb's region is the thumb, and the track's are the parts above and below it, so the
  two never overlap. A bar whose view the window's content clip has cut away has none. Both are functional widgets (§3.13).
- **The width contract.** While the bar shows, the rows stop 2 short of the track, so a row is 5 narrower than the view
  (`scrollbar.RESERVE`). `scrollbar.row_width(width, view, content)` gives that width.
  - **The list** draws its hover and pressed fills and its rows' hit regions that wide. It cannot move the cells its caller
    laid out, so a caller that lays out columns asks `list.row_width(width, rows, count)` first. The bar appears or goes only
    in `list.new` and `list:set_items`, never in a scroll.
  - **The card stack** gives each item's `draw` the row width, and each item's hit region is as wide. The bar appears or goes
    only when the items change, and `set_items` then draws every item again at the width it has.
  - **A picker** whose rows overflow is 5 wider, growing to the left, with the bar in that room (§3.14).
- **In the game:** the ledger's two tabs and the location tab's list lay their columns out at `list.row_width`
  (`main.lua`, `lib/location.lua`), and the pops and buildings stacks draw their cards and rows at the stack's row width.
  `location.min_width()` is now the larger of what holds the title bar's tabs and close button and 59 pixels, which holds the
  market's columns beside the bar.
- **The tooltip's own scroll bar** (§3.13) is older and is not this module.
- **Tests,** through the shipped scripts in the UI rig, with no window: `ui_link/scrollbar_tests.rs` and `ui_link/list_tests.rs`.

### 3.17 The settings file (accepted, CK-298)

The player's settings are kept in `settings.toml`, a file apart from the configuration file. The owner chose "Own settings
file" and "At once" (`owner-direction-settings-designer-r1.md` in `.pipeline/coordination/ui-polish-design/`): a change acts
straight away and is kept. This is the first slice of the settings window (UI polish item 3). The window itself came with
CK-301 (§3.18): its general and map tabs are what sets and resets a setting in the shipped UI. Since CK-310 its input tab
sets and resets the player's keys.

- **Where:** in the parent directory of the saves directory (`[game] saves`). For `config/game.toml` that is the top of the
  copy, beside `saves/`; with no `[game]` table it is beside the configuration file. A run with no `--config` has no settings
  file. `/settings.toml` is in `.ignore`.
- **What it holds:** keys under the configuration file's own tables. The first six, of CK-298:
  - `[ui.tooltips]`: `delay`, `lock` and `lock_timer` (§3.13);
  - `[map]`: `pan_pixels_per_frame` (1 to 32), `zoom_steps_per_second` (1 to 60) and `scroll_points_per_step` (1 to 256).
  - **Since CK-307,** the four graphics settings: `[ui] scale`, `[ui] list_look`, `[window] width` and `[window] height`.
  - **Since CK-309,** `[input.bindings]`: the player's key bindings. Both are below.
- **Read at start,** after the configuration file (`apps/codekarl/src/config.rs`, `load_settings`). A key in it overrides the
  configuration's value; a key it does not have keeps the configuration's value, or the default. A missing file is not
  reported.
- **A bad entry is named and ignored.** An unknown table or key, or a value the configuration's own rule refuses, gives one
  `warn` record in the category `config` that names the file and the entry. The file's other entries apply and the game
  starts. A file that cannot be read or is not TOML is reported once and ignored whole. The configuration file itself is as
  strict as before.
- **The calls** are on the UI VM's `settings` table (`apps/codekarl/src/settings.rs`); the gameplay VM has none.
  - `settings.tooltips()` returns `{delay, lock, lock_timer}`, and `settings.map()` returns `{pan_pixels_per_frame,
    zoom_steps_per_second, scroll_points_per_step}`, as they are in force when called.
  - `settings.set(name, value)` sets one setting: `"ui.tooltips.delay"`, `"ui.tooltips.lock"`, `"ui.tooltips.lock_timer"`,
    `"map.pan_pixels_per_frame"`, `"map.zoom_steps_per_second"` or `"map.scroll_points_per_step"`. A refused value, a wrong
    type or an unknown name is a Lua error that names it, and nothing changes.
  - `settings.reset("tooltips")` and `settings.reset("map")` remove that tab's keys from the file and put back the
    configuration's values, or the defaults.
  - `settings.bindings()` returns the key bindings in force: an array of `{name, bindings}`, each binding a string of the
    `[input.bindings]` grammar such as `"Control+KeyP"`. The settings window's input tab shows them (§3.18). Since CK-309
    it lists only the player's actions, each with `default` and `fixed` too, and `settings.set_bindings` sets them (below).
- **A change acts at once.** The running map controller takes a new map value straight away. The tooltip module reads the
  tooltip values again when a new target comes under the pointer (§3.13).
- **And it is written at once.** Every accepted set and every reset writes the whole file through a temporary file and a
  rename, as a save is written. The file then holds exactly the overrides in force. A write that fails gives one `warn` record
  in `config`, and the change still holds for the run.

**Since CK-307: the graphics settings.** Four keys, a call that resizes the window, and a UI scale limit that acts at once. It
is the engine's side of the settings window's graphics tab, which is a later step: no shipped script reads or sets any of it
yet. The owner asked for a resolution to type: "it should be 'resolution' with two values available to input, here the player
can input whatever numbers they want, and it will clamp to minimums and maximums". For its range the owner chose
"Min 1280x800, max screen" (`owner-direction-settings-designer-r3.md` and `-r4.md`). The spec is
`runtime-m55/ck307-spec-r3.md` (sha256 `7a814cea…`).

- **The keys:**

  | Key | Values | Without it |
  |---|---|---|
  | `[ui] scale` | 1, 1.5 or 2 | no limit |
  | `[ui] list_look` | `"cards"` or `"rows"` | `"cards"` |
  | `[window] width` | a whole number of screen pixels, 1280 to 16,384 | 1280 |
  | `[window] height` | a whole number of screen pixels, 800 to 16,384 | 800 |

  - The configuration file takes all four (`[ui] scale` is older, §3.3) and refuses a bad value as it refuses any other. The
    settings file takes all four too, and names and ignores a bad one.
  - **`[window] width` and `height` are the resolution kept:** what the player asked for, brought into the range. Nothing
    opens the window at it yet: `main.lua` still opens the game at 1280 by 800.
  - **`[ui] list_look` is only kept and read.** The location panel still starts each run on cards.
- **`settings.graphics()`** returns `{list_look, width, height}` in force, and `scale` (1, 1.5 or 2) when a limit is set.
  `width` and `height` are the resolution kept, never the window's size, which the updates report.
- **`settings.set("ui.scale", value)`** takes 1, 1.5 or 2, and **`settings.set("ui.list_look", value)`** takes `"cards"` or
  `"rows"`. Both are written like any set.
- **`settings.resize(width, height)`** resizes the window. It takes two whole numbers of screen pixels that are not negative;
  anything else is a Lua error that names the argument.
  - **The resolution kept** is each number lowered to the screen's size and then brought into its key's range. It becomes
    `[window] width` and `height` in force and is written, in one write.
  - **The size used** is the resolution kept, lowered to the screen. On a screen smaller than 1280 by 800 it is the screen's
    size, while the pair kept and written stays 1280 by 800. So the file never holds a pair the next start refuses.
  - **The screen** is the main screen's visible size, in pixels, as the main thread read it at start
    (`apps/codekarl/src/window_api.rs`, `ResizeSession`). A headless run has no screen, and nothing limits a size there but
    16,384.
  - **It returns the size used,** width and height, at once. It asks the main thread for that content size through the
    window's request slot, in points (the pixels divided by the screen's pixels per point, rounded up), and never waits.
    Of two requests not yet taken, the later wins.
  - **Without an open resizable window** the call is a Lua error, and nothing is kept or written.
  - **The new size arrives as any size change does:** one `SizeChanged`, and then the update's `width`, `height`,
    `pixel_width` and `pixel_height` (`docs/window-design.md` §8).
  - **On Cocoa** the window keeps its top-left corner. A window that is full screen, or on its way in or out of it, is not
    resized; the resolution is kept all the same. The GLFW backend does not resize: the Vulkan path is compile-only.
  - **Headless,** the call is checked, clamped, kept and counted like the other window calls, and nothing is resized.
- **`settings.reset("graphics")`** removes the four keys and puts back the configuration's values, or the defaults.
  - It first asks the window for the size used for the resolution then in force (the configuration's, or 1280 by 800), by
    the same request. It writes no pair.
  - Without an open resizable window it is a Lua error, and nothing is reset.
- **The UI scale limit acts at once** (`crates/render-2d/src/bridge.rs`, `set_ui_scale_limit`). A set or a reset of
  `[ui] scale` gives the scene bridge the new limit, and the scale in use is the lower of the limit and what the drawable
  allows (§3.3).
  - **When the scale in use changes,** the bridge sets its metrics again, with the same points and pixels and the new scale,
    and drops the hit regions that were laid out at the old scale. The update route is woken though no size changed.
  - **The next update** gives the script `width` and `height` in its units at the new scale, for the same `pixel_width` and
    `pixel_height`, and the game's script lays out again as it does after a resize.
  - **A gesture under way ends.** The first input tick at the new scale is marked as a focus loss is (§3.5): a UI capture
    gets `cancel` and a map drag ends. That tick's pointer events reach nobody.
  - **Until the UI has published hit regions at the new scale,** pointer events reach nobody: not the old layout, and not
    the map.
  - **A limit the drawable cannot reach changes nothing that is seen:** the metrics and the hit regions stay as they are.
- **Tests,** none with the GPU: `apps/codekarl/src/settings.rs` and `window_api.rs` (the calls, the clamps, the file, headless),
  `config.rs` (the keys in both files), `crates/window/tests/window/` (the request through the loop), `crates/render-2d` and
  `ui_link/input_tick_tests.rs` (the live limit), and one case with a real window in `crates/window-cocoa/tests/resize/`.

**Since CK-309: the player's key bindings, and key capture.** A script can change the keys of the player's actions while the
game runs, and can ask for the next key the player presses. It is the engine's side of key rebinding. **Since CK-310 the
settings window's input tab uses it** (§3.18), and is its one shipped caller: it calls `settings.bindings`, `set_bindings`,
`capture_key` (through `typing.capture`), `cancel_capture` and `settings.reset("bindings")`. For
the menu key the owner chose "Fixed": it cannot be changed. The records are `owner-direction-keys-designer-r1.md` and `-r2.md`,
and the spec is `runtime-m55/ck309-spec-r2.md` (sha256 `e85d479f…`).

- **The player's actions** are sixteen that can be rebound, in a run that has them, and `close`: `time_pause`, `time_speed_1`
  to `time_speed_5`, `time_faster`, `time_slower`, `game_quicksave`, `game_quickload`, `map_pan_left`, `map_pan_right`,
  `map_pan_up`, `map_pan_down`, `map_zoom_in` and `map_zoom_out` (`REBINDABLE_ACTIONS`, `apps/codekarl/src/config.rs`). Any
  other action, the demo's included, is not listed, set or reset by these calls and keeps the configuration's keys.
- **`close` is fixed.** Its keys are the configuration file's, or Escape. No call and no entry of the settings file changes
  them.
- **`[input.bindings]` in `settings.toml`** holds an entry for an action the player changed: a list of at most two bindings in
  the configuration's grammar. An empty list unbinds the action.
  - **The keys in force** of a player's action are the settings file's entry, else the configuration file's, else the
    built-in default.
  - **A bad entry is named and ignored,** like the file's other entries: one for `close`, for an action that is not a
    player's or that this run does not have, one that is not a list of strings, one with more than two bindings, and one
    the binding rules refuse. The other entries apply.
- **`settings.bindings()`** now returns one table for `close` and for each rebindable action the run has:
  `{name, bindings, default, fixed}`.
  - `bindings` is every key in force, in order. A configuration file can give an action more than two.
  - `default` is what the action has with no settings-file entry, and `fixed` is `true` for `close` only.
- **`settings.set_bindings(changes)`** sets the keys of one or more actions at once. `changes` is one table from an action's
  name to its whole new list: none, one or two binding strings.
  - **It is checked whole first.** An empty table, an action that cannot be rebound in this run, an action whose keys in
    force number more than two, a list that is too long or holds something that is not a string, and a binding the rules
    refuse are each a Lua error that names it, and nothing changes.
  - **The new set is in force when the call returns:** `settings.bindings()` then shows all of it.
  - **It reaches the keys as one replacement.** The whole set goes to the main thread through the window's request slot,
    and the loop gives it to the mapper in one step (`docs/scene-design.md` §10.1). No input tick sees some of the actions
    changed and others not.
  - **It is written in one write.** An action whose new list equals its default has its entry taken out of the file. A
    write that fails gives one `warn` record in `config`, and the change still holds for the run.
- **`settings.reset("bindings")`** removes every `[input.bindings]` entry, in one write, and puts back the configuration's
  keys, or the defaults. `"defaults"` is accepted as the same name.
- **`settings.capture_key(id)`** asks for the next key press. `id` is a string of 1 to 64 bytes, which comes back with the
  result. **`settings.cancel_capture()`** cancels a capture that waits. Both write the request slot and never wait.
  - One capture waits at a time. A new one replaces it, and the old one ends as cancelled.
  - **What is captured** is the first key-down of a key that was up and is not a modifier. Escape and Backspace are captured
    like any other key, so an Escape pressed while a capture waits closes nothing. `docs/scene-design.md` §10.1 says what a
    capture swallows.
  - **The result comes in an update,** once, as the field `capture`: `{id, kind = "binding", value = "Shift+F5"}`, or
    `{id, kind = "cancelled", value = reason}` with the reason `"replaced"`, `"requested"` or `"focus"`. An update with no
    result has no `capture` field.
  - **Disclosed:** one result waits at a time, so a later result replaces one that no update has carried yet.
  - **Arming a capture turns text input off** (§3.19). It stays off while the capture waits and while the captured key is
    still held.
- **A script arms a capture through `typing.capture(id, settings.capture_key)`** (§3.19), so that a widget that is editing is
  applied first and none starts while the capture waits.
- **Tests,** with no window and no GPU: `crates/input/src/tests.rs` (the capture and the replacement in the mapper),
  `crates/window/tests/window/` (the slot and the loop, with a fake backend), `apps/codekarl/src/settings.rs` and `config.rs`
  (the calls and the file), and `ui_link/typing_tests.rs` and `numberfield_tests.rs` (capture against an editing widget).

### 3.18 The settings window (accepted, CK-301 and CK-310)

The pause menu's "settings" button opens the **settings window**: a tabbed window above the menu, where a change acts at once
and is written to the settings file (§3.17). It is the second slice of the settings (UI polish item 3). The owner: "i think i
want a tabbed window". Since CK-310 its input tab changes the keys.

- **The records** are `owner-direction-settings-designer-r1.md` to `-r4.md` and `owner-direction-settings-manager-r7.md` in
  `.pipeline/coordination/ui-polish-design/`.
  - The owner chose the look "S3: side column" and named the tabs: "i was thinking the tabs would be categories, general,
    graphics, gameplay, input".
  - The owner chose "At once", "Hidden for now" for the gameplay tab, "Show keys now, rebind later", "Over the menu, x + Esc"
    and the order "general, graphics, gameplay, map, input".
  - For a row of buttons wider than its room the owner chose "Wrap, no justify".
  - On a slider's fill, the owner: "i answered that to the designer, its the cream".
  - The look is `mockups/settings-r4.html` (the general tab) and `mockups/settings-r2.html` (the map and input tabs) there,
    and it uses no new art (`settings-provenance-r1.md`). The spec is `runtime-m55/ck301-spec.md` (sha256 `1baf6fd2…`).
  - **For the keys (CK-310)** the records are `owner-direction-keys-designer-r1.md` and `-r2.md` and
    `owner-direction-game-menu-manager-r9.md` and `-r10.md`. The look is K1 of `mockups/keys-r1.html` and the two prompts
    of `mockups/keys-r2.html`, with no new art (`keys-provenance-r1.md`). The spec is `runtime-m55/ck310-spec-r4.md` (sha256
    `03594c5a…`).
- **One module,** `scripts/ui/lib/settings_window.lua`, is the window, its tab column, its three tabs and the slider. Since
  CK-310 it is the key slots and the "Key in use" prompt too.
  - `settings_window.open(view)` opens it, or raises it when it is open, and returns the window. There is one at most.
  - `settings_window.close()` closes it, `settings_window.relayout(view)` centres it again, and `settings_window.current()`
    gives a test its parts.
  - **The menu** (`lib/menu.lua`) **opens and closes it:** the "settings" button opens it, `menu.relayout()` passes on a
    change of the view's size, and the menu's `on_close` closes it.
  - **Since CK-310 `main.lua` calls `settings_window.update(update)` in every update,** after the pointer's dispatch. It
    reads the update's key capture result and ends a slot's wait (below). With no settings window open it does nothing.

Lengths below are in UI pixels.

- **The window** is a library window of kind `window`, titled "Settings", with the close "x". It is not draggable. It is
  centred in the view when it opens and again when the view's size changes.
  - Its content is 168 tall, and 226 wide plus the tab column's width.
  - It is made with `modal = true` (below), so it is above the menu and the backdrop.
  - It is larger than the menu and both are centred, so it covers the menu's buttons while it is open.
- **The tab column** is down the left side: three rows, 13 tall, in the order general, map, input. The window opens on general.
  - A row is a text button of kind `window`, with its text 3 in and 1 down. Pressing it shows its tab. The chosen row keeps
    the selected look, filled with the kind's `hot`.
  - The column is as wide as the widest of the labels general, graphics, map and input, plus 12. "graphics" is counted though
    it is not listed, so the window keeps its size when a graphics tab is added. A row's fill stops 3 short of the column's
    right edge.
  - A 1-pixel line in the kind's `outline` stands at the column's right edge. It stops 6 short of the content's bottom.
  - A tab's content starts 6 right of the column's right edge and is 220 wide. It is one group attached to the window and
    clipped to the window's content. It is destroyed and made again whenever a tab is shown. Since CK-310 a second group
    and a scroll bar go with it, for the input tab's rows.
  - There is no graphics tab and no gameplay tab.
- **A tab's rows** have their label at the content's left and their control 84 right of it. Every label is cut to its room on
  whole characters, with `..`.
- **The slider** is drawn from the theme's white texture: a track 80 wide and 2 tall in `track`, filled from its left to the
  knob in `text` (`#ece3cb`, the owner's "the cream"), and a knob 3 wide and 7 tall in `text`. Its value is written beside
  it, 86 right of the control's left.
  - **A press on it, or a drag,** sets the value from the pointer's place along the track, snapped to the slider's step and
    kept inside its range. The release sets it once more and ends the drag. A `cancel` or a pointer overflow (§3.5) ends the
    drag and sets nothing.
  - **A new value is set at once** with `settings.set` (§3.17), which also writes the file. A move that snaps to the value in
    force makes no call. So a drag across the range makes one call, and one write, for each value it snaps to.
  - **The slider updates itself in place during a drag:** it moves its knob, resizes its fill and rewrites its value's text.
    The tab is not made again, so the widget that holds the pointer's capture lives through the whole drag.
  - **Its hit region** is 84 wide and 7 tall, the knob's height: the track's length and 2 more at each end.
  - **A greyed slider** has its track, fill and knob in `dim`. It takes no press and is not functional (§3.13).
  - **Its place in a row:** in a row whose top is at y, the knob's top is at y + 4 and the track's at y + 7, with the label
    and the value at y + 2, as the mock-up's script draws them (`make_settings_r1.py`, `slider_row`). CK-318 put them
    there: CK-301 had drawn the knob 3 lower and the track 2 lower.
- **Rows of buttons wrap.** `button.flow{x, y, room, items, selected, kind, group, owner, on_choose}` in `lib/button.lua` is
  the shared layout. It makes one text button for each item, left to right from `x`, with 3 between two buttons.
  - A button that would pass `x + room` starts the next line at `x`, 18 lower. The first button of a line never wraps, and a
    row that fits is one line.
  - The item equal to `selected` is drawn selected, and a press on a button calls `on_choose(item)`.
  - It returns the buttons and the height used: 18 for each line, less 3.
  - The settings window's lock row is its one caller. No row that existed before was changed.
- **The general tab** has a "tooltips" heading in `dim` and three rows.
  - **"show delay":** a slider from 0 to 2 s in steps of 0.1 s for `ui.tooltips.delay`, 14 below the heading. Its value reads
    like `0.3 s`. Its label is drawn as the other rows' labels are since CK-318; CK-301 had left it out.
  - **"lock":** three choice buttons, "disabled", "middle mouse" and "timer", for the values `disabled`, `middle_mouse` and
    `timer` of `ui.tooltips.lock`, 20 below the first row. The one in force is drawn selected. Their room is 136 wide and
    they wrap: "timer" is on the second line. A press sets the value, and the tab is made again when the pointer's dispatch
    ends.
  - **"lock timer":** a slider from 0.3 to 3 s in steps of 0.1 s for `ui.tooltips.lock_timer`, 5 below the lock row's last
    line. Unless the lock is "timer" it is greyed, and its label and value are in `dim` too.
  - What a change does to the tooltips is in §3.13 and §3.17.
- **The map tab** has three sliders, 20 apart, each stepping by 1: "pan speed" from 1 to 32 (`map.pan_pixels_per_frame`),
  "zoom speed" from 1 to 60 (`map.zoom_steps_per_second`) and "scroll step" from 1 to 256 (`map.scroll_points_per_step`).
  A value reads as its number, and the scroll step's as `24 points`.
- **"defaults"** is a button at the bottom left of each tab's content. It calls `settings.reset("tooltips")`,
  `settings.reset("map")` or, on the input tab since CK-310, `settings.reset("bindings")`, which resets that tab's settings
  only. The tab is then made again, when the pointer's dispatch ends, with the values in force.
- **The input tab** (CK-310) shows the keys in force, from `settings.bindings()` (§3.17), and changes them. Before CK-310 it
  was a read-only list of eight rows, with "changing keys comes later" under them.
  - **Groups and rows.** There are three groups, each under its name in `dim`: **time** (pause, speed 1 to speed 5, faster,
    slower), **game** (quicksave, quickload, menu) and **map** (pan left, pan right, pan up, pan down, zoom in, zoom out).
    A heading and a row are each 15 tall. Only the actions the run has are listed, and a group with none of its actions
    is left out. The menu's row, for `close`, is always there.
  - **A row** has the action's name, 4 in, and **two key slots:** the first 70 right of the rows' left edge and the second
    3 right of the first. They show the action's first and second keys in force.
  - **A slot** is a cut-3 frame, 13 tall and as wide as `font.layout("press a key")` plus 8, filled `body` and outlined in
    the window kind's `outline`. The key's shown name is centred in it, in `text`. An empty slot shows `-` in `dim`.
  - **The rows scroll.** Their view is the content less a footer 22 tall, so 146 tall, with the scroll bar (§3.16) at the
    content's right edge. The footer has "defaults".
  - **Shown names.** `KeyA` reads `A` and `Digit1` reads `1`. The arrows read `Left`, `Right`, `Up` and `Down`. Punctuation
    reads as its character: `Equal` is `=`, `Minus` is `-`, `BracketLeft` is `[`, `BracketRight` is `]`, and likewise the
    semicolon, quote, backslash, comma, period, slash and backquote. Every other key reads as its `[input.bindings]` name,
    such as `Space`, `Escape`, `Enter`, `Tab` and `F5`.
    - Modifiers come before the key, as the binding's string has them: `Control` reads `Ctrl`, `Alt` reads `Option`, `Meta`
      reads `Cmd`, and `Shift` is itself. So `Shift+Equal` reads `Shift+=`.
    - A name too wide for its slot is cut on whole characters, with `..`.
    - **Disclosed:** a name is a physical key position. On a keyboard layout that is not the US one, `A` may not be the
      letter printed on that key.
  - **The menu's row is greyed:** its name and its slots' text are in `dim` and its slots' outlines in `track`. A click or
    a right-click on it does nothing. The owner chose "Fixed" for the menu key.
  - **An action with more than two keys in force** is shown the same way: greyed, its first two keys in its slots, and
    `+N` in `dim` after them for the N more. Only a hand-written configuration can give one (§3.17). Nothing changes it
    here, so no key that is not shown is dropped. This is the spec's own rule, marked a proposal there.
- **Setting a key** (CK-310). A click on a slot that is not greyed makes it **wait:** it is filled and outlined in the
  kind's `hot`, it reads `press a key` in the kind's `pressed_text`, and "Escape cancels" shows in `dim` right of
  "defaults".
  - **The click** is the left button's release over the slot. It arms a key capture with
    `typing.capture(id, settings.capture_key)`, under an id from `ui.key("settings_capture")` (§3.17, §3.19).
  - **One slot waits at a time.** A click on another slot abandons the first wait and starts the second.
  - **The wait is abandoned,** and its capture cancelled with `settings.cancel_capture()`, by a left press anywhere but on
    the waiting slot, by a tab being shown, and by the settings window closing, as it does when the pause menu closes. A
    capture result of kind `cancelled`, which a loss of focus gives, ends the wait too. The slot is then as it was.
  - **The capture's result** comes in an update's `capture` field (§3.17), which `settings_window.update` reads:
    - `Escape` cancels: the slot is as it was, and nothing is closed;
    - `Backspace` empties the slot, so Backspace cannot be set as a key;
    - any other key K is set by the next rule.
- **Is the key free?** An action **holds** K when one of its keys in force is exactly K: the same key with the same
  modifiers, so `F5` and `Shift+F5` are two keys (§2.6). The tab compares the binding strings. K's **owners** are the
  other actions that `settings.bindings()` lists and that hold K, in the tab's order. The demo's actions are not listed,
  so a key that only they have is free. The first case that applies decides:
  - **The action already holds K,** in either of its slots: nothing changes. No call is made and no prompt opens.
  - **No owner:** K is set in the slot at once.
  - **An owner cannot be changed:** it is `close`, or an action greyed for its more than two keys, alone or among several
    owners. The prompt names every owner and offers only "cancel". This is the spec's own rule, marked a proposal there.
  - **One owner:** the prompt offers "replace", "clear" and "cancel", and "swap" too when the slot had a key of its own.
  - **Several owners,** which only a configuration can make: the prompt names them all and offers "replace", "clear" and
    "cancel". The owner chose "Name all, no swap".
- **The prompt "Key in use"** (CK-310) is a library window of kind `window` with that title. It is `fixed`, so it has no
  "x", it is not draggable, and it is modal. It is centred in the view, and `settings_window.relayout` centres it again.
  The owner approved the first two prompts below as drawn ("Yes, as drawn") and then added "clear": "it should have a
  fourth button, clear".
  - **It has a backdrop of its own:** one quad over the whole view, tinted with `body` at alpha one half, in a group of
    its own, and a hit region over the whole view under a key from `ui.key("key_prompt_backdrop")`. Its widget does
    nothing with what it takes. The window library keeps that group just under the prompt and above the settings window
    (below). So nothing behind the prompt responds, the settings window included.
  - **Its lines** are in `text`, 15 apart. Below, X is K's shown name, A the row being set, B the owner's row and Y the
    key the slot had.
    - **One owner, the slot was empty:** `X is B's key.`, then `Use it for A instead?`, then `B's slot will be empty.` The
      buttons are "replace", "clear" and "cancel".
    - **One owner, the slot had Y:** `X is B's key.`, then `Use it for A instead?`, then `swap: B gets Y.`, then
      `replace: B's slot will be empty.` The buttons are "swap", "replace", "clear" and "cancel".
    - **One owner that already holds Y:** the lines and buttons of the empty slot, with no "swap". B cannot hold Y twice,
      so a swap could give only what "replace" gives. This is the spec's own rule, marked a proposal there.
    - **Several owners:** `X is B's and C's key.`, or `X is B's, C's and D's key.` for more, then `Use it for A instead?`,
      then `Their slots will be empty.` The buttons are "replace", "clear" and "cancel".
    - **An owner that cannot be changed:** `X is B's key.`, or the line for several, then `It cannot be changed here.`
      The one button is "cancel". `close` is named "the menu", so the line reads `X is the menu's key.`
  - **Its size:** as wide as its widest line plus 10, or as its row of buttons plus 8 when that is more. It is as tall as
    its lines, 15 each, plus 7 and a button's height. The buttons are one row at the bottom right, 3 apart, in the order
    given above.
  - **"replace"** puts K in A's slot and takes K from every owner. **"swap"** puts K in A's slot and Y in K's place in B's
    list. **"clear"** empties A's slot, and the owners keep K. **"cancel"** changes nothing. Each closes the prompt.
- **Every change is one `settings.set_bindings` call** (§3.17), which holds each action it changes with its whole new
  list: the one action for a free key, a "clear" or an emptied slot; A and B for a "replace" or a "swap"; A and every
  owner for a "replace" with several owners. It never holds `close` or a greyed action.
  - **The tab is then made again** from `settings.bindings()`, so it shows what is in force.
  - **An emptied slot** leaves the action's other key first in its list: `[F5, Space]` with its F5 slot emptied is
    `[Space]`, shown `Space` and `-`.
  - **A refused call,** a Lua error, changes nothing. The tab stays as it was and the error is logged, at `info`.
- **A right-click** on a slot that is not greyed empties it at once, and abandons a wait.
- **Stacking: only the windows the menu opens are above its backdrop.** The window library (`lib/window.lua`) now knows the
  backdrop's group and which windows are modal.
  - `window.new{…, modal = true}` makes a modal window. The menu's window, the settings window and, since CK-300, the
    glossary reader (§3.20) are three. Since CK-310 the "Key in use" prompt is the fourth.
  - `window.set_modal_backdrop(group)` names the backdrop's group and `window.clear_modal_backdrop(group)` forgets it. The
    menu calls the first when it opens and the second when it closes.
  - **While a backdrop is named,** the library orders its windows' groups again at every change (a window made, raised or
    closed, an item attached): the windows that are not modal first, in their own order, then the backdrop, then the modal
    windows in their own order. The tooltip overlay stays above them all.
  - **Since CK-310 a modal window can have a layer of its own.** `window.set_modal_overlay(owner, group)` names a group
    that is kept just under the window `owner`, above the modal windows before it in the windows' order.
    `window.clear_modal_overlay(owner, group)` forgets it. Both order the groups again. The prompt's backdrop is the one
    such layer.
  - **So a window made or raised while the menu is open is under the backdrop** and under its hit region: a press, drag,
    scroll or hover over it reaches nothing. The location panel answering a click that was still on its way when Escape
    opened the menu is such a window.
  - `window.top()`, the window Escape closes, is then the top modal window.
  - When the menu closes, every window is ordinary again, in the windows' own order.
  - Only library windows are ordered this way.
- **Leaving.** The "x", or Escape with no tooltip or picker to close first, closes the settings window.
  - **Since CK-310:** while a slot waits, Escape is the captured key and only ends the wait. While the prompt is open,
    Escape closes the prompt alone, which is "cancel", and the next Escape closes the settings window.
  - Closing the settings window, with its "x" or with the menu, closes an open prompt as "cancel" and cancels a capture
    that waits.
  - The menu is as it was: still open, the game still paused, its keys still blocked. Closing the settings window sends no
    control and makes no `game.set_modal` call.
  - A further Escape closes the menu.
  - Closing the menu closes the settings window with it.
- **Not there:** a graphics tab and a gameplay tab; a field to type a number into; tooltips on the window's own controls;
  a mouse button or the wheel as a key; more than two keys for an action; rebinding the demo's actions.
- **Tests,** through the shipped scripts in the UI rig, with no window: twenty-six tests in
  `apps/codekarl/src/ui_link/menu_tests.rs`.
  - **Twelve** cover opening and the window; the general tab's sliders, choices and "defaults"; a slider's drag across
    several updates, on the general and on the map tab; `button.flow`; the map tab; the "x"; closing the menu; and the
    stacking of a window made late and of an older window raised. The two of CK-318 among them cover the "show delay"
    label and the sliders' places on the general and on the map tab.
  - **Fourteen are the input tab's** (CK-310), with the settings calls doubled: the slots, their names and the greyed menu
    row; a run with no game actions; a slot waiting; "swap", with the prompt blocking the settings window; "replace" with
    several owners in one call; no "swap" when the owner holds the old key; a key the action already holds, and a
    modifier's variant being free; owners that cannot be changed; the lists after a "replace" and a "swap"; Backspace,
    a right-click and the ways a wait is abandoned; "clear" and Escape at the prompt; closing the settings window; and
    closing the menu.
  - **One of the fourteen runs the real calls:** the shipped scripts over a real `Mapper`, the real settings calls and
    a private settings file. A captured key is in force at the mapper and in the file, a "swap" reaches the mapper as
    one whole set with both actions in the file, and "replace", "cancel", Escape and "defaults" follow.

### 3.19 Typed text (accepted, CK-314)

A widget can be typed into. What the player types, by the system's keyboard layout and with Shift, Option and dead keys, cannot
be had from key codes or from bound actions. So it has a route of its own: **typed entries**, one ordered list in each update.
It was built for the number field (CK-306, §3.21) and for the save name's text field, which came with CK-303 (§3.22). No
shipped screen has a field of either kind yet, so in the game text input is never turned on.

- **The records.** The owner, in `owner-direction-settings-designer-r3.md` in `.pipeline/coordination/ui-polish-design/`:
  "here the player can input whatever numbers they want". The spec is `runtime-m55/ck314-spec.md` (sha256 `7acea958…`).
- **A typed entry** is a character or an edit command.
  - **A character** is one Unicode scalar value that the system committed for a key. Committed text is made precomposed
    (normal form C) and split into its scalar values, one entry each. So a dead key and a vowel give the one entry `á`.
  - **The five edit commands** are backspace, enter, tab, left and right.
  - **Never an entry:** text still being composed; a control character (U+0000 to U+001F, U+007F to U+009F); the system's
    private codes for function and arrow keys (U+F700 to U+F8FF); any other command of the system's; Escape.
  - A held key gives another entry at each repeat the system sends.
- **On and off: `window.set_text_input(on)`,** on the UI VM's `window` table (`apps/codekarl/src/window_api.rs`).
  - It takes exactly one boolean; anything else is a Lua error. It writes the window's request slot and never waits. The
    last call wins.
  - **It returns the call's generation,** an integer: 1 for the first call of a run, and one more at each call.
  - Text input is off when a run starts.
  - **The main thread applies it** when it next reads the slot (`crates/window/src/main_loop.rs`). Applying a call always
    starts from nothing, on or off: text being composed and the entries not yet given to the UI are discarded.
    - **Since CK-309,** while a key capture is active a call for on is applied as off (§3.17).
  - **Off,** no key is given to the system's text input and there are no typed entries, as before CK-314.
  - **On or off, every key is still recorded as a physical key.** So the actions, the close request and the pause menu's key
    block (§3.15) are unchanged, and a key typed into a widget is also whatever it is bound to.
- **In the update.** `frame_update` has three more fields (`ui_link.rs`, `update_table`):
  - `typed`: the entries in the order typed. A character is `{kind = "char", text = "á"}`. A command is `{kind = name}`, the
    name being `"backspace"`, `"enter"`, `"tab"`, `"left"` or `"right"`;
  - `typed_lost`: `true` when entries were dropped at a bound;
  - `typed_epoch`: the generation of the latest `window.set_text_input` call the main thread has applied, and 0 before any.

  The entries wait in the link's stash with the rest of the UI's input (§3.12) and are delivered once, by the next accepted
  update.
- **The bound is 256 entries,** in three places: the Cocoa window's own store, `ck-input`'s ring between two acknowledged
  snapshots (`TYPED_CAPACITY`), and the UI's stash while the UI is busy. The first two hold their entries in fixed storage.
  - At a bound the **later** entries are dropped, the earlier ones keep their order, and `typed_lost` is set. The key
    transitions and the pointer's events drop their oldest instead.
  - What is lost is lost: nothing is typed again.
- **A loss of focus** discards text being composed. The entries already gathered are delivered, and text input stays as it
  was set.
- **What the Cocoa window does** (`crates/window-cocoa/native/window.m`). Its content view is the system's text input client
  (`NSTextInputClient`).
  - With text input on, `keyDown:` records the key as before and then gives the event to the system's interpretation
    (`interpretKeyEvents:`), unless Command or Control is held.
  - Committed text comes back as `insertText:`. The five commands come back as the system's own commands for those keys
    (`deleteBackward:`, `insertNewline:`, `insertTab:`, `moveLeft:`, `moveRight:`). Any other command is passed over, without
    a beep.
  - Text being composed is only remembered as a flag, and is not shown.
- **`glfw-vulkan` gives no typed entries.** The GLFW backend has no typed route: it keeps the backend interface's default,
  which does nothing, and reports no typed event. The call still returns its generation and the update's `typed_epoch` still
  follows it, but a widget there cannot be typed into. The Vulkan path is compile-only (`AGENTS.md`).
- **The library's typing function: `scripts/ui/lib/typing.lua`.** It is the one reader of the typed list, and one widget edits
  at a time.
  - `typing.start(widget)` starts the widget's editing. The widget needs an `on_typed` function, or the call is an error. It
    takes the typing from a widget that was editing. When none was, it turns text input on. A call for the widget that is
    already editing changes nothing.
    - **Since CK-309 it returns `true`,** or `false` when a key capture owns the keyboard: it then starts nothing (below).
  - `typing.stop(widget)` ends the widget's editing when it is the one editing, and turns text input off. Another widget's
    call changes nothing.
  - So text input is on while a widget edits, and stays on through a change from one widget to another.
  - `typing.editing()` returns the widget that is editing, or nil.
  - `typing.dispatch(update)` gives the update's entries to the widget that is editing, in order, as
    `widget:on_typed(entry)`. If the update's `typed_lost` is set and the widget has an `on_typed_lost` function, that is
    called after them. With no widget editing the entries are dropped.
- **Typing before the pointer's events.** `main.lua` calls `typing.dispatch(update)` first of all in its update handler,
  before `pointer.dispatch`.
  - A script cannot know the order of a key and a click inside one update: the typed list and the pointer's events are two
    lists with no common order. So the rule is that an update's typing is handled first, all of it, and then its pointer
    events.
  - Typing and then clicking away, the usual order, comes out right even inside one update: the press finds the widget with
    everything that was typed.
  - **Disclosed:** the other order inside one update, a click on another widget and then a key, gives that key to the first
    widget. Across updates the order is the updates' own.
- **The editing session.** The library keeps a number that changes whenever an editing starts, ends or passes to another
  widget.
  - An update's typing belongs to the session that was under way when its dispatch began.
  - When that session ends part way, for example an Enter that applies a field, the rest of the update's entries and its
    loss notice reach no one. That holds even when the same widget starts editing again at once, as a Tab handler might.
- **Which entries count.** The library keeps the generation of its latest `window.set_text_input` call. The typing of an
  update whose `typed_epoch` is older than that is dropped: it was typed under the earlier setting, or before the main thread
  turned text input on.
  - So a stale entry never reaches a widget that started after it was typed.
  - **Disclosed:** a key pressed between a widget's start and the main thread turning text input on is not typed.
- **Key capture** (CK-309, §3.17). `typing.capture(id, request)` arms a capture for a script. `request` is the host's
  `settings.capture_key`. Since CK-310 the settings window's key slots are its callers (§3.18).
  - **It applies and ends the editing first.** A widget that is editing has its `on_apply()` called, if it has one, and its
    editing is then stopped, which turns text input off.
  - It then calls `request(id)`. If that raises, no capture is noted and the error is raised again.
  - **While the capture waits, no widget starts editing:** `typing.start` returns `false`. That ends when an update's
    `capture` field carries the capture's id, which `typing.dispatch` reads.
  - **`on_apply()` is the editing widget's part of this.** The number field's applies what was typed and tells its caller, as
    Enter does (§3.21). Since CK-303 the text field's does the same (§3.22).
- **Nothing of the typing is a bound action.** No action exists for a digit, a letter or an editing key: a game's bindings
  are the 22 they were. Escape gives no entry and stays the `close` action (§3.5).
- **Not there:** showing text while it is composed; input methods for other scripts; selecting, copy and paste. A field
  that takes letters came with CK-303 (§3.22).
- **Tests,** none with the GPU:
  - `crates/input/src/tests.rs` has the ring: the order, the 257th entry dropped and the loss kept until its snapshot is
    acknowledged, and the reset with its epoch.
  - `crates/window/tests/window/update.rs` and `slot.rs` run the loop with a fake backend: the order in one update and across
    two, text input off, a key's action unchanged with it on, turning it off and on, a loss of focus, and the slot's
    generations.
  - `apps/codekarl/src/input_stash.rs`, `ui_link.rs` and `window_api.rs` have the stash's bound and epoch, the update's three
    fields and the call. `config.rs` has the 22 bindings.
  - `apps/codekarl/src/ui_link/typing_tests.rs` runs the shipped `lib/typing.lua` in the UI rig with small widgets of its own:
    thirteen tests, for the order, text input on and off, stale entries, the rule with the pointer in one update and in
    two, the sessions and, since CK-309, key capture.
  - `crates/window-cocoa/tests/typed/mod.rs` has six cases with a real window, in the group `native.window`: a key-down with
    characters, the five commands, a repeat, composition, Escape and a Command chord, and text input off. They post marked
    key-downs and ignore the owner's input like the rest of that group (`docs/scene-design.md` §10.3).

### 3.20 The glossary reader (accepted, CK-300)

The pause menu's "glossary" button opens the **glossary reader**: a window above the menu with every entry of the glossary
(§3.13) to read. The owner: "this one should be simple. same format sort of, a column on the left with a list of glossary
terms alphabetically, and a larger column on the right, defaults to empty, which when a item is selected, prints the text in
the right column for the player to read. terms need to have functional tooltips here."

- **The records** are `owner-direction-glossary-reader-designer-r1.md` and `-r2.md` in
  `.pipeline/coordination/ui-polish-design/`. The owner chose "Gold heading", "Opens it here" for a click on a linked term,
  "No tooltip" on the list's titles and "Wheel + scroll bar" (§3.16). The look is `mockups/glossary-reader-r1.html` there,
  with no new art (`glossary-reader-provenance-r1.md`). The spec is `runtime-m55/ck300-spec.md` (sha256 `d36d4df2…`).
- **One module,** `scripts/ui/lib/glossary_reader.lua`, is the reader.
  - `glossary_reader.open(view)` opens it, or raises it when it is open, and returns the window. There is one at most.
  - `glossary_reader.close()` closes it, `glossary_reader.relayout(view)` centres it again, and `glossary_reader.current()`
    gives a test its parts.
  - **Its one caller is the menu** (`lib/menu.lua`), as for the settings window: the "glossary" button opens it,
    `menu.relayout()` passes on a change of the view's size, and the menu's `on_close` closes it.

Lengths below are in UI pixels.

- **The window** is a library window of kind `window`, titled "Glossary", with the close "x": 400 by 300 over all and not
  draggable. It is centred in the view when it opens and again when the view's size changes. It is modal (§3.18), so it is
  above the menu and its backdrop.
- **The left column** is a scroll list (`lib/list.lua`) of every entry's title, with its scroll bar (§3.16).
  - The titles are sorted by `string.lower` of the title, so capitals do not count. Two equal titles go by their key.
  - It shows 21 rows, 13 tall, with the title 3 in. It is as wide as the widest title plus 12, plus the scroll bar's room.
  - **The chosen row** is filled with the kind's `hot`, and the fill stops 2 short of the row's right edge. A hovered row has
    the list's usual hover look. The titles have no tooltip.
- **A 1-pixel line** in the kind's `outline` divides the columns, down the whole content.
- **The right column** is empty when the reader opens. A click on a title chooses that entry and shows it, 8 right of the
  line:
  - the entry's title as a heading, in the tooltip kind's `title_text` (gold);
  - 18 below the heading's top, the entry's text in the panel font, in 13-pixel lines. It is wrapped on whole words to end 8
    short of the window's right edge. Plain words are in the window kind's `text` and linked words in `term`.
  - **Links read as the tooltip reads them** (§3.13): `{key}` shows the linked entry's title, `{key|words}` the words, and
    `{{` and `}}` are braces. A `{key}` that names no entry is drawn as plain words and is no link.
  - The page is one group attached to the window, destroyed and made again for each entry.
- **A linked word works.**
  - **Hovering it** opens its term's tooltip (`tooltip.explanation`), as a term in a panel does.
  - **A click on it opens its entry here:** it becomes the chosen entry, the right column shows it, and the list scrolls by
    the least amount that brings its row into view. The change is made when the pointer's dispatch ends.
  - Each link has one hit region, with a rectangle for each line it wraps over, under a key from `ui.key("glossary_link")`.
    No key is made from a title or a glossary key, which are data.
- **Leaving.** The "x", or Escape with no tooltip to close first, closes the reader. The menu is as it was: still open, the
  game still paused, its keys still blocked. Closing the menu closes the reader with it.
- **It remembers nothing.** Opened again, it is empty with the list at the top.
- **The chosen row of a list** is new in `lib/list.lua`, and a list has it only when asked.
  - `list.new{…, selectable = true}` gives the list a fill in the kind's `hot`, drawn under the hover and pressed fills.
    `selected_inset` (points, 0 unless given) makes it that much narrower than the row, at its right.
  - `list:select(index)` puts the fill on that row and `list:select(nil)` takes it away. `list:selected()` returns the chosen
    index, or nil. On a list made without the option `select` is an error.
  - The fill scrolls with the rows. A chosen row shows no hover fill, and a press on it still shows the pressed look.
  - `set_items` keeps the chosen index unless the list no longer has that row. The reader's list is the one shipped list
    with the option.
- **Not there:** searching or filtering; groups of entries; a way to the reader other than the pause menu.
- **Tests,** through the shipped scripts in the UI rig, with no window: seven tests in
  `apps/codekarl/src/ui_link/glossary_reader_tests.rs`. They cover opening above the backdrop, with "x" and Escape back to
  the menu; the resize and closing the menu; all 92 titles in order, with no tooltip and an empty right column; an entry's
  heading and wrapped text; links, braces and long text; a click on a linked term; and reopening.

### 3.21 The number field (accepted, CK-306)

A widget of the library that a whole number is typed into: `scripts/ui/lib/numberfield.lua`. It is the first widget that
edits through the typing function (§3.19). The owner asked for typed numbers ("here the player can input whatever numbers
they want, and it will clamp to minimums and maximums") and chose "Enter or leaving it" for when a typed value applies
(`owner-direction-settings-designer-r3.md` and `-r5.md` in `.pipeline/coordination/ui-polish-design/`). The look is the fields
of `mockups/settings-r4.html` there (`settings-provenance-r3.md`: plain shapes). The spec is `runtime-m55/ck306-spec-r3.md`
(sha256 `235c72a2…`).

**No shipped screen has a number field yet.** The settings window's graphics tab, a later step, will be the first.

**Since CK-303 the editing itself is shared with the text field** (§3.22). It is in `scripts/ui/lib/fieldedit.lua`, and one
field edits at a time, a number field or a text field. What a number field does is as it was.

Lengths below are in UI pixels.

- **`numberfield.new{x, y, value, least, greatest, on_apply, digits, kind, group, owner}`** makes one.
  - `value`, `least` and `greatest` are whole numbers that are not negative, with `least` not above `greatest`. The field
    starts on `value` brought into that range.
  - `digits` is how many digits it holds, 4 unless given. A `greatest` with more digits than that is an error.
  - `on_apply(number)` is the caller's function. It is told when an applied number differs from the one before.
  - `kind` is `window` unless given. With a `group` the field's quads and hit region are in it, at a place local to it.
- **The look:** a cut-3 frame 15 tall and as wide as that many zeros plus 8, filled `body` and outlined in the kind's
  `outline`. The number is right-aligned in `text`: it ends 4 short of the right edge, 2 down.
- **Editing.** A left press on the field starts it: the outline turns to the kind's `hot`, and a caret 1 wide and 11 tall, in
  `text`, stands just right of the number.
  - The number is kept. A digit is added at its end, and Backspace takes its last digit away.
  - A digit typed into a full field does nothing, and so does Backspace in an empty one.
  - **One field edits at a time.** A press on another field applies the first and starts the second, and text input stays
    on through the change.
- **The typing** comes only from the typing function: the field calls `typing.start` when its editing begins and
  `typing.stop` when it ends, and its `on_typed` gets the entries.
  - A character `0` to `9` is a digit, from the digit row or the number pad alike. `backspace`, `enter` and `tab` are the
    commands of those names. Every other character, `left` and `right` are passed over.
  - No bound action carries any of it. A key typed into a field is still its key's action (§3.19): the field is meant for
    windows the pause menu opens, where the game's keys are blocked (§3.15).
- **Applying** is Enter, Tab, or a left press anywhere that is not a field, the map included. Since CK-303 a text field
  counts as a field here: a press on one applies the number field and starts the text field.
  - The number is brought into the range, the field shows the result, the editing ends, and `on_apply(number)` is called
    once.
  - A field that was emptied puts its old number back. Neither that nor a number that did not change tells the caller.
  - Tab does nothing more: it starts no other field.
  - **The press still does what it does.** The field hears of it through `pointer.observe_press`, before the press reaches
    its own widget.
  - **The caller's function runs last.** The field's own state, the caret and the typing function are all settled before
    `on_apply` is called. So the caller may close the window, destroy fields or start an editing from inside it.
- **Escape** while a field edits puts the old number back, ends the editing and tells the caller nothing. It goes no
  further: `window.close_request` asks `numberfield.cancel()` first of all, before a tooltip, a picker or a window (§3.5).
  With no field editing, Escape acts as before. Since CK-303 that call cancels the field that edits, of either kind.
- **Closing the window, or destroying the field,** while it edits ends the editing and applies nothing.
- **Typing and the pointer in one update** follow the typing function's rule (§3.19): the update's typing first, then its
  pointer events.
  - A digit and then a click on another field: the first field gets the digit and is applied with it, and the second
    edits.
  - **Disclosed:** a click on another field and then a digit, inside one update, gives the same result: the digit goes to
    the first field.
  - An Enter and then a digit: the Enter applies the field, and the digit reaches no field.
- **Key capture** (CK-309, §3.17). The field has the editor's `on_apply()` method, which `typing.capture` calls: it applies
  what was typed and tells the caller, as Enter does, and leaves no field editing. While a capture waits, a press on a
  field starts nothing.
- **For scripts:**
  - `field:get()` returns the number last applied, and `field:set(number)` sets it, brought into the range. A field that is
    editing then shows the new number and goes on editing.
  - `field:move(x, y)`, `field:size()`, `field:editing()` and `field:destroy()`.
  - `numberfield.current()` returns the field that is editing, or nil, and `numberfield.cancel()` is Escape's step. Since
    CK-303 both are the shared editing's, so the field returned may be a text field.
- **`pointer.observe_press(fn)`** is new in `lib/pointer.lua`. `fn(event)` is called for each left press of a packet that is
  not an overflow packet, a `down` or a `map_down`, in the packet's order and before the press reaches its widget. It routes
  nothing and cannot stop the press.
- **Not there:** letters and a caret that moves, which the text field has (§3.22); selecting, copy and paste; a sign or a
  decimal point; a greyed field.
- **Tests,** through the shipped scripts in the UI rig, with typed entries and real pointer input and no window: 31 tests in
  `apps/codekarl/src/ui_link/numberfield_tests.rs`. They cover the look, typing, applying by Enter, by Tab and by a press
  elsewhere, two fields, the cases of one update and of two, Escape, the pause menu's key block, closing the window, what a
  caller's `on_apply` may do, and key capture.

### 3.22 The text field (accepted, CK-303)

A widget of the library that a line of text is typed into: `scripts/ui/lib/textfield.lua`. It is the number field (§3.21)
with any character of the font and a caret that moves. The owner asked for it for the save/load screen: "a textbar on top
that autofills with a generated save name" (`owner-direction-save-load-designer-r1.md` in
`.pipeline/coordination/ui-polish-design/`). Its look and its editing rules are the number field's, with the text
left-aligned as `mockups/save-load-r1.html` there draws it, and it uses no new art. The spec is
`runtime-m55/ck303-spec-r2.md` (sha256 `b066c2cb…`).

**No shipped screen has a text field yet.** The save/load screen, a later step, will be the first.

Lengths below are in UI pixels.

- **`textfield.new{x, y, width, value, greatest, on_apply, kind, group, owner}`** makes one.
  - `width` is the whole field's and is at least 16. `greatest` is the greatest length in characters, a whole number of
    at least 1. Anything else is an error.
  - `value` is the text it starts with, empty unless given. A value that is not valid UTF-8, is longer than `greatest` or
    has a character the field does not take is an error.
  - `on_apply(text)` is the caller's function. It is told when an applied text differs from the one before.
  - `kind` is `window` unless given. With a `group` the field's quads and hit region are in it, at a place local to it.
- **The look:** the number field's cut-3 frame, 15 tall, filled `body` and outlined in the kind's `outline`. The text is
  left-aligned in `text`, 3 from the left edge and 2 down.
- **Which characters.** A character is one Unicode scalar value. The field takes the space, and every character the
  project font has a glyph of its own for. It takes no control character, and none that the font would draw as its
  replacement glyph. Printable ASCII and the accented vowels `ÁÉÍÓÚáéíóú` are all taken.
- **Everything counts characters, not bytes.** The greatest length, the caret's place and what Backspace takes are
  counted in characters, and the text is kept as a list of them. So the text is valid UTF-8 after every edit, 48 accented
  vowels fit a field of 48, and Backspace takes an accented vowel whole.
- **Editing.** A left press on the field starts it: the outline turns to the kind's `hot`, and a caret 1 wide and 11 tall,
  in `text`, stands at the end of the text.
  - A typed character goes in at the caret. Backspace takes away the character before it. Left and Right move the caret
    one character.
  - A character typed into a full field does nothing. Neither do Left and Backspace at the start, or Right at the end.
  - **Text wider than the field scrolls,** a character at a time, so that the caret stays in view. The text's room is
    the field's width less 7. When characters are taken away, the text moves back to fill the room. A field that is not
    editing shows its text from the start.
  - A press on the field while it edits changes nothing: the caret is not placed with a click.
- **The typing** comes only from the typing function (§3.19), as the number field's does. `on_typed` gets a character, or
  one of the commands `backspace`, `left`, `right`, `enter` and `tab`, in the order typed.
  - So `a`, backspace, `b` leaves `b`, and `a`, `b`, left, `x` leaves `axb`.
  - A held key acts again at each entry it gives.
- **Applying** is Enter, Tab, or a left press anywhere that is not a field, the map included. The press still does what
  it does.
  - The field shows its text from the start, the editing ends, and `on_apply(text)` is called once, last of all.
  - A text that did not change tells the caller nothing.
  - **An empty text is the caller's to decide.** It is applied like any other, as `on_apply("")`, and the caller may
    `set` another text.
- **Escape** while the field edits puts the old text back, ends the editing and tells the caller nothing. It goes no
  further (§3.5).
- **Closing the window, or destroying the field,** while it edits ends the editing and applies nothing.
- **Typing with the pointer in one update, and key capture,** follow the number field's rules (§3.21). An update's typing
  is handled first, then its pointer events. Arming a capture applies the field first, and while a capture waits a press
  on the field starts nothing.
- **For scripts:**
  - `field:get()` returns the text last applied or set.
  - `field:set(text)` sets it, with the errors of `new`. A field that is editing then shows it, with the caret at its
    end, and goes on editing.
  - `field:move(x, y)`, `field:size()`, `field:editing()` and `field:destroy()`.
- **The shared editing, `scripts/ui/lib/fieldedit.lua`,** is new with CK-303. It owns which field edits, for both kinds.
  - **One field edits at a time,** a number field or a text field. A press on another field of either kind applies the
    first and starts the second, and the first one's caller is told last. Text input stays on through the change.
  - `fieldedit.begin(field)` starts a field's editing, and does nothing while a key capture waits.
    `fieldedit.apply(field)` applies it. `fieldedit.cancel()` is Escape's step, and `fieldedit.current()` returns the
    field that edits, or nil.
  - `numberfield.cancel()` and `numberfield.current()` now call those two. So `window.close_request` did not change for
    the text field.
  - `fieldedit.end_editing(field, handoff)` ends an editing and tells nobody, and `fieldedit.capture_apply(field)` is
    what a field's `on_apply()` calls for `typing.capture` (§3.19).
  - **A field takes part** by having `field_edit = true` on its class, the fields `is_editing` and `frame`, and the
    methods `commit()`, `revert()`, `show_editing()`, `hide_caret()` and `on_typed(entry)`.
  - **Every transition settles its own state before any caller's code runs,** and the caller's `on_apply` is the last
    thing it does. So that function may close the window, destroy fields or start an editing.
  - The module registers the one `pointer.observe_press` function (§3.21): a left press on a widget that is not a field
    applies the field that edits.
- **Not there:** selecting text, copy and paste; Delete, Home and End; placing the caret with a click; input methods for
  other scripts; a greyed field.
- **Tests,** through the shipped scripts in the UI rig, with typed entries and real pointer input and no window: 34 tests
  in `apps/codekarl/src/ui_link/textfield_tests.rs`. They cover the look and the refused arguments; the characters taken
  and refused; the counts in characters; the caret and the scrolling; the orders inside one update and across two; the
  pointer in the same update; a text field and a number field together; applying, the empty text and Escape; a held
  key; the pause menu's key block; closing the window; what a caller's `on_apply` may do; and key capture.

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
- The right mouse button and context menus (the input layer has only the left button). *(Since CK-269 the middle and right buttons'
  presses and releases reach the UI, §3.5; nothing uses the right button, and there are still no context menus.)*
- Drag and drop between windows; docking; resizable windows.
- Rich text, several fonts or sizes, kerning, wrapping beyond single lines (labels are single lines; lists are many labels).
  *(Two smaller fonts came later, with CK-268: §2.4. A glossary explanation in a tooltip wraps at spaces, CK-270: §3.13.)*
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
