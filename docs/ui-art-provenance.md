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

## Elsewhere

- **The goods icons** (CK-267): `.pipeline/coordination/ui-polish-design/goods-icons-provenance-r1.md`; `docs/ui-design.md` describes
  the icons and their generator.
