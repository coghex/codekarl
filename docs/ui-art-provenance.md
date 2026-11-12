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

## Elsewhere

- **The goods icons** (CK-267): `.pipeline/coordination/ui-polish-design/goods-icons-provenance-r1.md`; `docs/ui-design.md` describes
  the icons and their generator.
