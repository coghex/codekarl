# UI art: provenance

Where the UI's drawn art came from. The rule is the owner's (2026-11-11, on the goods icons): "you went off of basic shapes and not
your understanding of other games' icons, i dont want to accidently use an icon from another game". Art is composed from the plain
shape of the thing drawn, never from another game's or font's art, and each piece has a record saying how it was made.

This page holds the record for art that is in the tree and whose files point here. The statements about how a piece was made are its
author's record, moved here from the design round's notes; the files, hashes and checks named are in the accepted tree.

## The small and tiny fonts (CK-268)

- **What they are:** the fonts `small` (the round's "font B", a 9-row box) and `tiny` ("font C", an 8-row box), in
  `assets/ui/font/small.toml` and `tiny.toml`, with their glyph images in `assets/ui/font/small/` and `tiny/`
  (`docs/ui-design.md` §2.4).
- **Who drew them:** ck-designer-1 (Claude), for Codekarl, on 2026-11-12, in the UI polish round.
- **How:** every glyph (capitals, lower case, fada vowels, digits, superscripts, subscripts, punctuation, the middle dot and the
  replacement box) is a character grid, `#` for a set pixel and `.` for a clear one, written letter by letter from the plain shape of
  the character at that size, on the layout of the project's own standard font (`assets/ui/font/font.toml`).
  - The small font's superscripts and subscripts reuse the tiny font's 3×5 digits. The tiny font's own are new 3×4 grids.
  - The sample glyphs shown to the owner in the first round were kept unchanged.
- **What was not used:** no existing pixel font was opened, copied or used as a reference.
- **Limit:** at 3 to 4 pixels wide there are few ways to draw a letter, so some glyphs will match other small fonts by necessity, not
  by copying.
- **Approval:** the owner approved both full sets as drawn ("Both approved": freeze B and C as drawn;
  `.pipeline/coordination/ui-polish-design/owner-direction-tooltips-r4.md`).
- **The frozen art:** `assets/ui/font/tooltip-fonts-art.txt` (sha256 `ec5eeb08…`) is the approved art, one grid per glyph. The two
  font files are that art padded to the box height, with a space added.
  - A test (`apps/codekarl/src/font.rs`, `every_small_and_tiny_glyph_matches_the_frozen_art_after_box_padding`) checks every glyph of
    both files against it.
  - `assets/ui/font/make_font.py --check` fails when a glyph image differs from its font file.
- **The round's record:** `.pipeline/coordination/ui-polish-design/tooltips-provenance-r2.md` (sha256 `ab477469…`), which this
  section carries. The grids were first written in `mockups/tooltip_fonts_r1.py` in that folder.

## The unknown good icon (CK-287)

- **What it is:** the 9×9 image `assets/ui/goods/fallback/unknown.png`, a tan sack tied at the neck with a dark question mark. It
  is drawn for a good, or a herd's species, without its own icon (`docs/ui-design.md` §3.13).
- **Who drew it:** ck-designer-1 (Claude), for Codekarl, on 2026-11-13, in the UI polish round.
- **How:** it is a 9×9 grid of characters, one character a pixel and one letter a palette colour, composed from the plain shape of
  an everyday container: a sack tied at the neck. Two more grids were made the same way and not chosen: a square crate with a
  darker edge, and a rounded lump with a shaded corner.
  - The question mark is five pixels placed by hand on that grid: a two-pixel top, one pixel stepping right and down, one pixel
    back to the centre, and a dot below a one-pixel gap.
  - The colours are letters of the shipped goods icons' own palette (`assets/ui/goods/make_goods.py`).
- **What was not used:** no reference image, no image search, no image generator, no icon from any other game, and no glyph from
  any font, the project's own included.
- **Limit:** a question mark on a container is a common way to mark "unknown", and at 9×9 there are few ways to draw either, so the
  icon may resemble others by convergence. None was copied.
- **Approval:** the owner asked for "a questionmark over a ambiguous looking good"
  (`.pipeline/coordination/ui-polish-design/owner-direction-ui-tweaks-manager-r3.md`). After trying the variants on a mock-up the
  owner approved the sack: "the sack looks best, against my expectations, i approve the sack"
  (`owner-direction-ui-tweaks-designer-r5.md` in that folder). That replaced an earlier choice, the crate (`-designer-r4.md`).
- **The art** (`.` transparent; `T` `#c49a6c`, `t` `#8a6440`, `b` `#6b4522`, `k` `#2a221a`):

  ```
  ...TTT...
  ....b....
  ..TTTTT..
  .TTkkTTT.
  .TTTTkTT.
  .TTTkTTT.
  .TTTTTTT.
  .TTTkTTt.
  ..TTTtt..
  ```

  The three `T` at the top are the sack's gathered neck, the `b` under them its tie, and the `t` pixels at the lower right its
  shade.
- **In the tree:** the grid is `UNKNOWN` in `assets/ui/goods/make_goods.py`, which writes the image.
  - `make_goods.py --check` fails when the image differs or is missing, or when `fallback/` holds any other PNG.
  - A test (`apps/codekarl/src/theme.rs`, `unknown_good_art_check_requires_the_exact_single_fallback_image`) makes each of those
    three failures happen on a temporary copy of the goods folder.
- **The round's record:** `.pipeline/coordination/ui-polish-design/unknown-good-provenance-r2.md` (sha256 `7149a226…`), which this
  section carries. The three grids are in `mockups/unknown-good-art-r1.txt` (sha256 `c40ddbe2…`) in that folder, first typed into
  `mockups/make_unknown_good_r1.py`.

## The list bar's icons, direction marks and checkbox (CK-295)

- **What they are** (`docs/ui-design.md` §3.14), all in `assets/ui/theme/` and all white on clear, tinted by the theme when drawn:
  - `icon_filter.png` and `icon_sort.png`, the 9×11 icons of the two buttons under a list;
  - `sort_down.png` and `sort_up.png`, the 5×5 marks that show a sort's direction in the sort picker;
  - the filter picker's **checkbox**, which is not an image: `scripts/ui/lib/listbar.lua` draws it from stretched copies of
    `white.png`.
- **Who drew them:** ck-designer-4 (Claude), for Codekarl, on 2026-11-13, in the UI polish round's lists topic.
- **How:** each is a character grid typed by hand, `#` for a set pixel and `.` for a clear one, composed from plain geometric
  shapes at the size of the theme's own icons. The shapes are the everyday objects and signs the words name.
  - **filter** is a kitchen funnel, solid: a 9-wide rim, rows 7 and 5 wide under it, then a 3-wide spout cut at an angle.
  - **sort** is a 1-wide arrow pointing down (a stem and a 3-wide head) beside four bars 5, 4, 3 and 2 wide.
  - **The direction marks** are one arrow, pointing down and pointing up: a 1-wide stem and a 5-wide head.
  - **The checkbox** is a 7-pixel square outline one pixel thick. A ticked one holds a filled 3-pixel square at its centre.
- **What was not used:** no image, icon set, font or other game's art was opened, traced, copied or used as a reference.
- **Limit:** a funnel for "filter" and an arrow beside shortening bars for "sort" are common signs, and at 9×11 there are few
  ways to draw either, so the icons may resemble others by convergence. None was copied.
- **Approval:** the owner chose among drawn candidates on a mock-up page (`mockups/lists-r1.html` in
  `.pipeline/coordination/ui-polish-design/`).
  - The icons: "filter 1, sort 2" (`owner-direction-lists-designer4-r2.md`), the solid funnel and the arrow beside bars. Not
    chosen: the funnel as an outline, three centred bars, a plain thick arrow, and an up arrow beside a down arrow.
  - The checkbox: "i want the square" (`owner-direction-lists-designer4-r3.md`), over a tick mark.
  - The direction marks: the down mark was in the sort picker the owner chose on that page ("popup 3", the same file). Both
    marks are on the second page, `mockups/lists-r2.html`, the picture the owner had when saying "these look good, i approve
    them" (`owner-direction-lists-designer4-r5.md`).
- **The art:**

  ```
  filter       sort         sort_down   sort_up
  .........    .........    ..#..       ..#..
  #########    .#..#####    ..#..       .###.
  .#######.    .#.......    #.#.#       #.#.#
  ..#####..    .#..####.    .###.       ..#..
  ...###...    .#.......    ..#..       ..#..
  ...###...    .#..###..
  ...###...    .#.......
  ...###...    ###.##...
  ...##....    .#.......
  .........    .........
  .........    .........
  ```
- **In the tree:** the two icons are `filter` and `sort` in `ICONS`, and the two marks are `sort_down` and `sort_up` in `MARKS`,
  in `assets/ui/theme/make_theme.py`, which writes the four images.
  - `make_theme.py --check` fails when a committed image differs from what the script writes, or is missing or extra.
  - A test (`apps/codekarl/src/theme.rs`, `the_art_scripts_report_their_images_up_to_date`) runs that check.
- **The round's record:** `.pipeline/coordination/ui-polish-design/lists-provenance-designer4-r1.md` (sha256 `5b0a174a…`), which
  this section carries. The grids were first typed in `mockups/make_lists_r1.py` (sha256 `59bbddc6…`) in that folder, as `FUNNEL`,
  `ARROW_BARS`, `SMALL_DOWN` and `SMALL_UP`; the accepted grids are those, unchanged.

## Elsewhere

- **The goods icons** (CK-267): `.pipeline/coordination/ui-polish-design/goods-icons-provenance-r1.md`; `docs/ui-design.md` describes
  the icons and their generator.
