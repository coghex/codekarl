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

## Character events, attributes and skills (CK-363, characters slice 4)

`events/character-events.toml` and `events/attributes-skills.toml` are the designer's data (ck-designer-1), copied **byte for byte** from
`codekarl-content/events/`. Every number marked TUNE is a placeholder; the model is not. The files write ordinary decimals; the loader
reads each from its text and never through a binary float (chances in parts per billion, every other value in thousandths).
**The life-log lines of the events are player-facing: the owner checks their wording by hand before they ship.**

| File | Revision | Pijul change | SHA-256 |
|---|---|---|---|
| `events/character-events.toml` | r2 (56 events) | `UBXVOCVQ` | `9497fbbf0547a916fdd8162f6a701879860e3a2ea854c020e7a395ce5beb7fa4` |
| `events/attributes-skills.toml` | r3 (with the `[catalog]` of 57 keys) | `UBXVOCVQ` | `062a57c331ad021f795a1d161bf2aea6d41ef007cf9e238ea964c53efadd31b0` |

Source of the copies: `codekarl-content/events/` (2026-11-19/20).

The design they implement:

- `history-scratch/events-data-r1.md` (SHA-256 `5cb78331…`): what the owner approved and what is a placeholder.
- The approved specification: CK-363 r3, SHA-256 `18b02949f3c02104a0e0164bea8454b509d3c2cb2e818cb9d8109890aa98701a`.

Both files join the save's `rules_hash` (a changed event table refuses an existing save) and the `economy.core` check paths. The loader
refuses a key that is not in the `[catalog]`, an effect its kind does not allow, a `[keep_while]` entry for anything but a calling, station
or background, and a text placeholder no effect of the event can set. The TOML reader keeps no file order, so the order of the `[catalog]` (the order precedence scans in) is read from the text, one key per
line; the order of a focus's odds is alphabetical, and the events run in file order. The skill parameters have a validated range
(`gain_rate` at most 100000, focus multipliers at most 10, the home-education fractions at most 1).

## Life cycle and names (CK-364, characters slice 5a)

`lifecycle/life-cycle.toml` and `lifecycle/names.toml` are the designer's data (ck-designer-1), copied **byte for byte** from
`codekarl-content/lifecycle/` (change `KXAUBYA6`). Every number marked TUNE is a placeholder; the model is not. The files write ordinary
decimals; the loader reads each from its text and never through a binary float (chances and factors in parts per billion, per-point
values in millionths, every other quantity in thousandths). **The names of the non-Irish pools are placeholders drafted from memory (in
768 only Mayo's three Northumbrian monks use them); the death lines the code writes are player-facing, and the owner checks their wording.**

| File | Revision | Pijul change | SHA-256 |
|---|---|---|---|
| `lifecycle/life-cycle.toml` | r1 | `KXAUBYA6` | `a231dd162fd8f5fdaa7e012cd0748831444a7413b4a38ec022b5f95c57f4f44d` |
| `lifecycle/names.toml` | r1 | `KXAUBYA6` | `54f0ffe059c02129f5762f38b220fdaac1cb37058412f201321881d308f236bf` |

Source of the copies: `codekarl-content/lifecycle/` (2026-11-19). `epithets.toml` is imported by 5b and `connections.toml` by slice 6 (both below).

The design they implement:

- `history-scratch/lifecycle-data-r1.md` (SHA-256 `98d938e7…`): what the owner approved and what is a placeholder.
- The approved specification: CK-364 r5, SHA-256 `17502d9ed79ed8df9e04bd9a3ce369ff353a1d52c78772becca0b915c3e2dc22`.

Slice 5a reads only `[death]`, `[majority]`, `[marriageable]`, `[relations]` and `[conception]` of `life-cycle.toml` and the pools and
`[culture_pool]` of `names.toml`; the sections of the later issues (marriage, upbringing, nutrition) load untouched (the `[namesakes]` table is read by 5b). Both files
join the save's `rules_hash` (a changed table refuses an existing save) and the `economy.core` check paths. The loader refuses a
probability outside 0..1, age curves whose ages do not strictly increase or whose factors are negative, `infant_years` or `old_doubling`
that is not a positive whole number, a registry culture without a pool, a pool without men or women, and a man's name without a genitive
where the pool has a patronym.

## Namesakes and epithets (CK-365, characters slice 5b)

`lifecycle/epithets.toml` is the designer's data (ck-designer-1), copied **byte for byte** from `codekarl-content/lifecycle/epithets.toml`
(r2, 2026-11-19: the ten live epithets as structured conditions). It holds 30 epithets, ten of them gainable now; the other 20 are
`later` (their systems are not built) and load without being gained. Every chance and importance is a placeholder to tune; the model is
not. The loader reads each decimal from its text (a chance in parts per billion, a stat limit and the weight ratio in thousandths) and
never through a binary float. **The `shown` texts are player-facing; the owner checks their wording.** 5b also reads the `[namesakes]`
table of `lifecycle/names.toml` (imported by 5a, unchanged).

| File | Revision | Pijul change | SHA-256 |
|---|---|---|---|
| `lifecycle/epithets.toml` | r2 | the CK-365 change that adds it | `5848c087c5360402669c4a8c8faf56c07d5d861bcd2912f859f5075f8bfa236f` |

The design it implements:

- `history-scratch/lifecycle-data-r1.md` (SHA-256 `98d938e7…`) and the designer's rulings in `history-scratch/ck364-dispositions-r1.md`
  (SHA-256 `ebc7b694…`, 5b section).
- The approved specification: CK-365 r2, SHA-256 `759a007f3d90494b62693022c7f9a96e457f7b7dae49c1da9da7fdc53d770cad`.

`epithets.toml` joins the save's `rules_hash` (a changed epithet table refuses an existing save) and the `economy.core` check paths
(`assets/game/lifecycle/`). A `scope` is `universal`, a culture subgroup of the registry or a religion of the economy rules; any other
value, or a key that is both a subgroup and a religion, is a load error, as are an unknown key, a bad value, a `later` epithet with a
`when` or a `chance`, and a gainable epithet without them. The condition vocabulary is slice 4's plus `grade_min`, `stat_max_by_sex`,
`weight_ratio_min`, `count_min` and `adult`, which an event's `when` still refuses.

## Connections and passions (CK-368, characters slice 6)

`lifecycle/connections.toml` is the designer's data (ck-designer-1), copied **byte for byte** from `codekarl-content/lifecycle/connections.toml`
(r3, change `AXA4PPEY`, 2026-11-20). It holds the contact settings (four live, four inert), the circle, the eight strand kinds with their
chance factors, the happenings that make strands, orientation, passions (four live moments, three inert), the settling-in and burn-in
lengths, the opinion terms and the life-log lines. Every number marked TUNE is a placeholder; the model is not. The loader reads each decimal
from its text (a chance, a factor or a per-point weight in parts per billion, an opinion weight in thousandths of a point) and never through a
binary float. **The log lines are player-facing; the owner checks their wording.** An inert entry (`live = false` moments, and the `council`,
`army`, `hostage` and `foster_kin` settings) is admitted with its deferred conditions and targets: only its shape is checked, and it never acts.

| File | Revision | Pijul change | SHA-256 |
|---|---|---|---|
| `lifecycle/connections.toml` | r3 | `AXA4PPEY` | `823157ad664a377761963923ead88668be43b7aefcb2457489c728a53d325395` |

Source of the copy: `codekarl-content/lifecycle/connections.toml` (2026-11-20).

The design it implements:

- `history-scratch/lifecycle-data-r1.md` (SHA-256 `98d938e7…`) and the owner-approved `history-scratch/characters-spec-r1.md` (SHA-256 `1d3f8a56…`).
- The approved specification: CK-368 r11, SHA-256 `0369a5fb215f6b63e9a3c37213ebdea1a601801b1db3bf2209011fba383aa5aa`.

`connections.toml` joins the save's `rules_hash` (a changed table refuses an existing save) and the `economy.core` check paths
(`assets/game/lifecycle/`). The loader refuses an unknown key, a probability or contact outside 0..1, a negative strength, size or year
count, a zero divisor (`skills_divisor`, `envy_divisor`, `age_gap_years`), a grade table that is not seven values, orientation weights that
do not sum above 0, a live moment's `when` key outside slice 4's vocabulary plus `ruler_years_min`, `years_in_pop_min`, `in_ruling_house` and
`standing_min`, and a live moment's target that is not one of the five the engine resolves.

## The generator (CK-369, characters slice 7)

`lifecycle/generator.toml` is the designer's data (ck-designer-1), copied **byte for byte** from `codekarl-content/lifecycle/generator.toml`
(change `RBD5D3G5`, 2026-11-20). It holds the recipe (the eleven steps in order), the generated past (the concocted happenings, battles,
raids, duels and the yearly strain of a life lived before the start), the reference rates the tests hold, the second-language ranges, and
the slot rules (ages, reach and weights). Every number marked TUNE is a placeholder; the model is not. The loader reads each decimal from
its text (a probability in parts per billion, a skill level, a stress amount, a weight and a language strength in thousandths) and never
through a binary float. The prose `from` fields of the slots and the `[marriage_candidates]` and `[real_households]` sections are
descriptions: the behaviour they describe is code.

| File | Revision | Pijul change | SHA-256 |
|---|---|---|---|
| `lifecycle/generator.toml` | r1 | `RBD5D3G5` | `651837c64acaa588ac25633bfe81deade1d1aed878f6a3b5023d796eb017bb9c` |

Source of the copy: `codekarl-content/lifecycle/generator.toml` (2026-11-20).

The design it implements:

- `history-scratch/generator-data-r1.md` (SHA-256 `81136f43…`) and the owner-approved `history-scratch/characters-spec-r1.md` (SHA-256 `1d3f8a56…`).
- The approved specification: CK-369 r4, SHA-256 `78b87ea39174bbcc50cf24d8d447e942c5e4ceef32a10f8b4d63f17a0ed9af90`.

`generator.toml` joins the save's `rules_hash` (a changed table refuses an existing save) and the `economy.core` check paths
(`assets/game/lifecycle/`, which also holds the `ck-characters` tests' data). The loader refuses a recipe order that is not exactly the
eleven shipped steps, a probability or band outside 0..1, a reference key the tests cannot build, a language range outside 0..1 or whose low
is above its high, a role of `latin_by_office` that is not in the **role vocabulary** (a fixed list in the code, `ck_economy::events::ROLES`,
not derived from any data file), a slot kind other than `fosterling`, `hostage` and `student`, a negative `reach_km` or weight, a past that
kills, and a skill, pop type or key the engine does not have. It also reads the mother tongue of every culture from the registry (CK-356)
and requires the `latin` language. `connections.toml`'s `[orientation] clergy` factors, which slice 6 validated and did not keep, are kept
now: they weight the orientation of a character generated in a monk pop.
