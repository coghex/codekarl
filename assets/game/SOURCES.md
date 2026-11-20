# Game data: sources

## Genetics (CK-361, characters slice 2)

`genetics/haplogroups.toml` and `genetics/traits.toml` are the designer's data (ck-designer-1), copied **byte for byte** from the
content repository. Every number in them is a placeholder to tune; the model is not. The files write ordinary decimals; the loader reads each from its text and
never through a binary float, as an integer in thousandths (stats, spreads, heritabilities, `w`, `d`, `s0`, `k`) or in parts per billion
(every probability and rate: `allele`, `penetrance`, `founder`, `m`, `q`, `founder_q`; `trait_rates` multipliers are thousandths).

| File | Revision | Pijul change | SHA-256 |
|---|---|---|---|
| `genetics/haplogroups.toml` | r2 | `CY7QDSH5` | `ce5f64a16a74a84827ac91c35859f0232a61b41a15aa42cfb2bf2072053edfe3` |
| `genetics/traits.toml` | r2 | `E2AX4M4S` (deaf and blind are weak dominants, penetrance 0.1) | `6d7eeba3ae8a36855683e013555a687621ff592dfa8a1204142652d9aaeafe26` |

Source of the copies: `codekarl-content/genetics/` (`haplogroups.toml` r2, 2026-11-19; `traits.toml` r2, 2026-11-19).

The design they implement:

- `history-scratch/genetics-r8.md` (owner-approved r109, with its 2026-11-19 correction of the child's spread for CK-361).
- `history-scratch/inheritance-math-r1.md` (the Mendelian modes and the graded-trait model, approved).
- `codekarl-content/lists/traits.md` (r7).
- The approved specification: CK-361 r5, SHA-256 `da44f6c229526cce1c37236d3c0c1c18769c66546878aaf7fb7535ab03fc71ee`.

Both files join the save's `rules_hash` (a changed genetic table refuses an existing save) and the `economy.core` check paths. The
`[culture_start]` table maps every culture of `peoples.toml` to a start set; the loader refuses a culture without one.

## Personality (CK-362, characters slice 3)

`personality.toml` is the designer's data (ck-designer-1), copied **byte for byte** from `codekarl-content/personality/personality.toml`.
Every number marked TUNE in it is a placeholder; the model is not. The file writes ordinary decimals; the loader reads each from its
text and never through a binary float, as an integer in thousandths (values, spreads, weights, factors, stress and the opinion maxima;
the grade-term lists are whole opinion points, stored times 1000).

| File | Revision | Pijul change | SHA-256 |
|---|---|---|---|
| `personality.toml` | r2 | `YTBTCJBN` (manner unbounded in storage; drunkard kinship added) | `a2d9f9e54b832c85c3fd694de73dcb5a66d2a1273a726bb150f2e36f601c57f1` |

Source of the copy: `codekarl-content/personality/personality.toml` (r2, 2026-11-19).

The design it implements:

- `history-scratch/personality-r12.md` and `history-scratch/personality-data-r1.md` (SHA-256 `bd769493…`): what the owner approved and what is a placeholder.
- The approved specification: CK-362 r3, SHA-256 `214c14e68c2c637cf986d1e95395dd8adb169c10f9c1ca8b6caefa5786d8baf5`.

The file joins the save's `rules_hash` (a changed personality table refuses an existing save) and the `economy.core` check paths. The
loader refuses a `[culture_means]` table that is not exactly the cultures of `peoples.toml`.
