# The interface's data files and where they come from

## The character glossary (CK-375, characters slice 9a)

`glossary_characters.toml` is the designer's character glossary (ck-designer-1), copied **byte for byte** from
`codekarl-content/glossary/characters.toml`. It holds the terms the character screen's tooltips explain: the body stats, the skills, the humours,
the vice/virtue pairs, the manner pairs, the standing values, stress and its threshold, and the later slices' terms (opinion, passion, claim ...).
It is loaded with `glossary.toml` as one glossary (`apps/codekarl/src/glossary.rs`): a link may name a term of either file, and a key in both is a
load error.

| File | Revision | Content change | SHA-256 |
|---|---|---|---|
| `glossary_characters.toml` | r3 (63 terms) | `L4FJQTVA` | `497c49b2ba42fd49d9d7f9b26d3b7ce4c51d1265802a0da9defc6bd51b3b0e99` |

Its header says the owner approved r1 on 2026-11-15 (`owner-direction-designer-r81`), that r2 reworded `stress` and `stress_threshold`, and that
r3 (CK-375's spec review) corrected the skill, fecundity and languages texts. Four texts go back to the owner; a text-only change later is a new
copy with a new hash here.

## The character screen's art

`characters/make_characters.py` writes `characters/*.png`: the 55 colour icons of the first two tabs (the owner-approved 9x9 set, r85-r87, frozen from
the designer's `char_icons.py`, SHA-256 `86dc5a31bce827c29f656889a42de47fea41dbad39974040ff614cc0eeec485c`) and the unknown portrait at two sizes. The
ledger's monochrome bust is `theme/icon_characters.png`, written by `theme/make_theme.py`. Each drawing is composed from plain shapes and copied
from no other image; the provenance notes are in the two scripts, and `--check` fails when a committed image differs from what its script writes.

The find popup river icon `theme/icon_river.png` is an original drawing of wavy strokes composed from plain shapes in `theme/make_theme.py`; it is copied from no other image.
