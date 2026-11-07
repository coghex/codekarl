# Ireland's map data: sources, derivation and guesswork

This directory is Ireland and its associated offshore islands in the map format of `ck-map` (`docs/map-design.md` section 9.5), for
milestone 6, slice 5 (CK-181), updated by CK-194 for the Ireland-768 scenario. **It is a draft toward medieval Ireland, approved by the owner at three preview views on 2026-11-05 (below); no file in it has a hand edit: every committed layer is reproduced byte for byte by the scripts and edits r8 in `edits.toml`.** The elevation, the
coast and the larger loughs come from real data; which terrain a cell has, which land is usable and the course of most rivers do not (see
the guesswork log).

| File | What it is | Kept by |
|---|---|---|
| `map.toml` | the crop and the layer names | `derive_elevation.py` |
| `elevation.png` | 16-bit grey, `elevation + 11000` in metres | `derive_elevation.py` (fully reproducible) |
| `terrain.toml` | the terrain types, their keys, palette and rules | by hand |
| `terrain.png` | the terrain type of each cell, as its key colour | `draft_terrain.py`, then `apply_edits.py` (edits r8; no hand edit) |
| `usable.png` | usable (`ffffff`) or not (`000000`) | `draft_terrain.py`, then `apply_edits.py` (edits r8; no hand edit) |
| `rivers.toml` | the major rivers, as latitude and longitude points | `derive_rivers.py`, unchanged (no hand edit) |
| `edits.toml` | the owner-delegated corrections toward medieval Ireland: one entry per change, with its reason (cited by `id` in the guesswork log below) | by hand (ck-designer-1, owner-delegated); r8 adds the two monastery islands at the owner's direction |
| `apply_edits.py` | applies `edits.toml` over the draft, in order, deterministically | |
| `edits.applied` | records that the edits were applied to these images (the SHA-256 of `edits.toml`) | `apply_edits.py` |
| `ireland_grid.py` | the world grid and tile arithmetic the scripts share | |

**The committed `elevation.png`, `terrain.png` and `usable.png` are the first draft plus edits r8 in `edits.toml` (the medieval corrections);
`rivers.toml` is `derive_rivers.py`'s output unchanged. Hand edits made after them are listed in the guesswork log.** Running `draft_terrain.py` or
`derive_rivers.py` again reproduces the first draft only and destroys those edits, so both refuse to overwrite their files unless
`--force` is given; `apply_edits.py` refuses to run twice over the same files (`edits.applied`) unless `--force` is given.
`derive_elevation.py` can be run again at any time, but its `--force` regenerates `elevation.png` and so removes the edits' sea paint:
after it, run `draft_terrain.py --force` and `apply_edits.py` again. The chain is: derive elevation, draft terrain, derive rivers, apply the edits (the edits read `rivers.toml` for the usable-land
suitability, and the rivers are derived from the first draft's elevation, before the edits change it).

## The crop

Latitude 51.25 to 55.55 N, longitude 10.90 to 5.30 W, in whole cells of the world grid (spherical Mercator, cells of exactly 839
projected metres): global origin cell (22436, 14979), 744 x 959 cells. A cell is about 500 m at Ireland's latitude (517 m at the south
coast, 476 m at the north). The margin of sea is about 8 km: Inishtearaght (10.66 W), Skellig Michael (10.54 W), Inishtrahull (55.43 N)
and the Fastnet Rock (51.39 N) lie inside it. `crates/map/tests/ireland.rs` loads the committed data through `ck-map`'s loader, which is
the authority for the grid.

## Sources, licences and attribution

### AWS Terrain Tiles (elevation, the coast and sea depth)

- **What:** the terrarium-encoded tiles of the Terrain Tiles dataset on the AWS Open Data registry
  (<https://registry.opendata.aws/terrain-tiles/>), downloaded from
  `https://s3.amazonaws.com/elevation-tiles-prod/terrarium/{z}/{x}/{y}.png`, zoom 10 (a pixel is 152.9 projected metres). The tiles are
  **not committed**; `derive_elevation.py` fetches them into a cache directory.
- **Cite as:** Terrain Tiles was accessed on 2026-11-05 from <https://registry.opendata.aws/terrain-tiles>.
- **Licence:** the registry points to the attribution document of the tile generator
  (<https://github.com/tilezen/joerd/blob/master/docs/attribution.md>): the tiles combine several sources, **each with its own terms**,
  and the attribution below must stay with the data and its public mirror. The sources that cover Ireland and the seas around it are
  EU-DEM (land, most of Europe at 30 m), SRTM and GMTED2010 (land, where those are used) and ETOPO1 (ocean bathymetry, 1 arc-minute); the
  data.gov.uk source (the Environment Agency, 2 m over most of the United Kingdom) lies under the cells of Wales and Great Britain that
  the crop holds and that are painted sea. The attribution each requires:
  - **EU-DEM:** "Produced using Copernicus data and information funded by the European Union - EU-DEM layers."
  - **ETOPO1:** "DOC/NOAA/NESDIS/NCEI > National Centers for Environmental Information, NESDIS, NOAA, U.S. Department of Commerce."
    (public domain in the United States).
  - **SRTM:** "SRTM data courtesy of the U.S. Geological Survey." (public domain).
  - **GMTED2010:** "GMTED2010 data courtesy of the U.S. Geological Survey." (public domain).
  - **United Kingdom (data.gov.uk), Open Government Licence v3:** "© Environment Agency copyright and/or database right 2015. All rights
    reserved."
- The attribution document does not say which source supplied which tile, so this file lists every source that may have contributed.

### Natural Earth (the larger loughs, and the rivers it has)

- **What:** `ne_10m_lakes`, `ne_10m_lakes_europe` (lakes), `ne_10m_rivers_lake_centerlines` and `ne_10m_rivers_europe` (rivers), 1:10m
  GeoJSON, from <https://github.com/nvkelso/natural-earth-vector> (`geojson/`). Downloaded into the cache directory, **not committed**.
- **Licence:** public domain (<https://www.naturalearthdata.com/about/terms-of-use/>); no attribution is required. Credit, as it asks:
  "Made with Natural Earth. Free vector and raster map data @ naturalearthdata.com."
- It holds the Shannon (to Limerick), the Barrow, the Suir and the Lower Bann, and thirteen lake features in the crop (twelve named).

### Not used

- **OpenStreetMap** geometry is not used: its licence (ODbL) is share-alike, and this directory reaches the public mirror. Rivers that
  Natural Earth lacks are placed by hand from a few anchors (below).

## The derivation

Needs Python 3.11 or newer with `numpy` and `Pillow` (run with numpy 2.3.4 and Pillow 12.2.0). From a checkout:

```
python3 assets/map/ireland/derive_elevation.py --source ../ireland-source --force   # map.toml, elevation.png
python3 assets/map/ireland/draft_terrain.py    --source ../ireland-source --force   # terrain.png, usable.png (the first draft only)
python3 assets/map/ireland/derive_rivers.py    --source ../ireland-source --force   # rivers.toml (from the draft elevation)
python3 assets/map/ireland/apply_edits.py                                           # edits.toml over the draft layers
```

This is the exact order that produced the committed bytes, and it reproduces them: run into an empty scratch directory with
`--out DIR` (copy `terrain.toml`, `edits.toml` and the scripts there first; `terrain.toml` is a hand-written input), the four steps
write `map.toml`, `elevation.png`, `terrain.png`, `usable.png`, `rivers.toml` and `edits.applied`, and `cmp` finds all of them, and
`terrain.toml` and `edits.toml` as inputs, **identical to the committed files** (checked 2026-11-05 with the cached sources: map.toml,
elevation.png, terrain.png, usable.png, rivers.toml and edits.applied byte for byte the same).

`--source` is the download cache (not committed). Each script's header documents its rules; the parameters:

- **Elevation** (`derive_elevation.py`): zoom 10 terrarium tiles; a sample belongs to the cell its centre falls in (about 30 samples a
  cell); a cell is land where more than half its samples are above 0 m; its elevation is the mean of its land samples (land) or its sea
  samples (sea), rounded to whole metres, at least 1 m for land and at most 0 m for sea; depths are kept; the file stores `elevation +
  11000`.
- **Foreign land:** two boxes of (south, north, west, east) degrees, (55.25, 90, -5.95, 180) for Kintyre, Arran and the islands to the
  north-east and (-90, 52.2, -5.4, 180) for Pembrokeshire, are painted sea at a depth of 80 m. 2251 land cells were painted.
- **Terrain types** (`draft_terrain.py`): sea, lake, pasture, forest, bog, marsh, mountain. Pasture everywhere on land, then in this
  order: forest where the forest noise > 0.56 and 15 m < e < 320 m; raised bog where 35 < e < 150, slope < 0.02, bog noise > 0.5 and more
  than 20 cells from water; blanket bog where 120 < e < 330, west of 8.3 W and bog noise > 0.42; marsh where e < 30, within 8 cells of
  water and marsh noise > 0.45; mountain where e > 300 + (mountain noise - 0.5) x 140; then two passes of a 3 x 3 majority filter. The
  noise is a hash-based value noise with fixed seeds (fractal, 4 octaves, base scales 40, 50, 30 and 16 cells for mountain, bog, forest and
  marsh). These are the rules of mock-up 2 (`.pipeline/coordination/map-design/mockup/build_data.py`).
- **Usable land (the first draft only; the committed `usable.png` is recomputed by `apply_edits.py`, below):** `0.62 x noise(10 cells) + 0.28 x noise(3.2 cells)` thresholded, for each terrain type separately, at the quantile that
  gives it the share of mock-up 2: pasture 32 %, forest 28 %, bog 20 %, marsh 15 %, mountain 8 %.
- **Lakes:** Natural Earth lake polygons drawn at 4 times the resolution; a cell is a lake where more than half of it is covered, on land
  only. Dropped as modern: Poulaphouca (Blessington) Reservoir, 1940 (and, by name, Inniscarra and Carrigadrohid, 1950s dams on the Lee,
  which Natural Earth does not have).
- **Rivers** (`derive_rivers.py`): see its header. 15 rivers in this draft: Shannon, Erne, Upper Bann, Lower Bann, Foyle, Moy, Corrib,
  Boyne, Liffey, Barrow, Nore, Suir, Slaney, Munster Blackwater, Lee. Natural Earth gives the Shannon, the Lower Bann, the Barrow and the
  Suir, but its lines are coarse (straight stretches of 10 km or more), so their simplified vertices are used as anchors too; for the rest a few anchors per river (towns and bridges, approximate to a kilometre or two) are moved to the cell of the greatest
  drainage area within 4 cells, and the line between them follows the downhill path of the elevation grid (depressions filled), or, where
  that does not reach the next anchor, the cheapest path over the grid (a step costs less where the drainage area is greater). Lines are
  simplified with a tolerance of 0.6 cell.

### The terrain keys (`terrain.png`)

| Terrain | Kind | Key | Solid | Share of land in the draft |
|---|---|---|---|---|
| sea | sea | `0a1a33` | | |
| lake | lake | `2f6fb8` | | |
| pasture | land | `a08944` | `a08944` | 48.0 % |
| forest | land | `367534` | `367534` | 21.3 % |
| bog | land | `2c4c2d` | `2c4c2d` | 20.8 % |
| marsh | land | `5c3e25` | `5c3e25` | 5.5 % |
| mountain | mountain | `7f8187` | `7f8187` | 4.4 % |

Usable land in the draft is 26.7 % of land. The palette and rules are in `terrain.toml` (mock-up 2's values).

## Guesswork log

**What is real:** the coast (at modern sea level, but for the reclamations below), the elevation and the sea depth (above) and the
larger loughs. **What is invented:** which terrain each land cell has, which land is usable, and the course of the rivers (Natural Earth's
vertices and hand-placed anchors, snapped to the valley floor of a 500 m grid). Each departure from a source is listed below, by area,
with its reason, so later research can correct it.

### The first draft (`derive_*.py`, `draft_terrain.py`, `derive_rivers.py`)

| Area | Departure from the sources | Reason |
|---|---|---|
| The whole map | Terrain types and usable land are **invented** by rules on elevation, slope, distance to water and noise | no medieval land-cover source; the first draft only needs to look like mock-up 2 |
| The whole map | The coast is the modern coast at 0 m: no medieval sea level or tidal flat (the reclamations below aside) | a source to correct with is a later question |
| Kintyre, Arran, Sanda and the islands north-east of Antrim; Pembrokeshire | painted sea at 80 m (2251 land cells) | only Ireland and its offshore islands are land (spec item 2); the two boxes are a working rule |
| Small islands | The Fastnet Rock, a rock of under 1 cell, is lost; Skellig Michael, the Tearaghts, the Copelands and the Saltees are one to seven cells | the cell is about 500 m; a cell is land where more than half its samples are above 0 m |
| Lough Mask | kept as a lake; Natural Earth classes it as "Reservoir" | it is a natural lake |
| An unnamed Natural Earth lake near 53.86 N, 7.96 W | kept as a lake; probably Lough Boderg or Bofin on the Shannon | a lake of the Shannon system |
| Poulaphouca (Blessington) Reservoir | not a lake | dammed in 1940 |
| Every river | the line follows the valley floor of the 500 m grid between anchors (hand-placed towns and bridges, or Natural Earth's simplified vertices), not the surveyed channel; the Shannon stops at Limerick | Natural Earth is coarse and does not have eleven of the fifteen |

### The medieval corrections (`edits.toml`), owner-delegated 2026-11-05

The owner, 2026-11-05: "if you know of modern reclamations, then we will have to fill them in with a best guess approximation of what
they should look like, sea or marsh"; "we want to guess on the medieval bogs, im sure its more than just one, many bogs were terraformed
into arible land, and we want to represent medieval ireland, so here use your judgement to make the bogs like they were"; "we are not
representing modern ireland, we are just using the data as a jumping off point, forests should look like they did then". On draft 2 the
owner said: "these look fine, but there is a lot more 'noise' like these bogs were added one at a time randomly, little disconnected bits
instead of larger 'islands' of usable resources. if this is accurate then i approve, what do you think?" and then: **"noise shouldnt be
used to place these, we need to expand existing features in an intelligent way"**. So no noise or random field places bog, forest or usable
land: they grow from existing features, by rules that stop on terrain (`apply_edits.py`'s header has the rules).

The target period is before the 17th-century clearances and the 18th-20th-century drainage and reclamation (the start date, 768 or the
early 1400s, is open in `docs/vision.md`). **Every coordinate, threshold and seed is a judgement of ck-designer-1, not a surveyed source**,
set against the draft's `elevation.png` (the elevation data reads a few metres high on reclaimed flats, so the slob limits are 3 to 12 m
by area); the basis of every row is "ck-designer-1 judgement, owner-delegated 2026-11-05". Edits apply in file order, each seeing what the
earlier ones left. The last column is the number of cells the entry changed.

* **What counts as an existing feature (ck-manager ruling, 2026-11-05):** the first draft's bog and forest were placed by a fractal noise
  rule, and their outlines are what the owner objected to, so they are **not** seeds. The first bog and forest edit clears them all to
  pasture. The eleven recorded woods (Glenconkeyne and Killetra, Kilultagh, the Fews, Shillelagh, the Wicklow glens, Leix and Offaly,
  Aherlow, Killarney, Duhallow, Slieve Aughty, Lough Erne) then grow nearest-first to 60 % of each circle, before any bog edit, over
  pasture below 300 m with slope 0.01 or more (Kilultagh, a flat lowland wood, has no slope limit), so the bogs fill the basins around them. Bog grows only from real seeds: the cells of very flat ground
  at lake edges (Central Plain, Ulster), 16 named bog locations in the midlands and Ulster and 36 named blanket-bog places in Erris
  (12), Connemara (13) and west Donegal (11) (each is a coordinate in `edits.toml`'s `seed_points`; 18
  of them are ck-designer-1's `western-bog-sites-r1.md`, added at the owner's direction). **Named sites always count (r7):** the cells within
  2 cells (5 x 5) of each named bog site that are pasture, forest or marsh, and of each wood's centre that are pasture, take the painted
  terrain whatever the stop rule says (`seed_radius`, `seed_over`), and the growth continues under the stop rule from them; `tidy` never
  removes a patch that contains or touches a named site of the same terrain (`keep_seed_points`). The check that every one of the 63
  named sites (52 bog, 11 woods) has cells of its terrain within one cell in the final `terrain.png` passes for the committed data; the
  first submission (edits r6) failed it for 14 sites (found by ck-worker-2 and confirmed by ck-reviewer-1: a wood excluded by a slope
  limit, sites on low-lying ground the wet rule excluded, and coordinates that lay in the sea, a lake or on a mountain). r7 moves six
  coordinates (Owenduff, the Mullet, Montiaghs Moss, the Maam valley, Glenveagh, Dunlewey) to the nearest ground at or below 200 m with
  slope at most 0.08, each reason in `edits.toml`. It spreads over connected basin ground (flat,
  low, at or below the local mean elevation), lowest and flattest first, in the west at most 10 km (straight-line) from a named place so
  that it follows the ground and not the region's outline. The uplands rule (every pasture cell from 150 m with slope up to 0.10
  becomes bog) runs after the growth and so seeds nothing. Steep valley sides (60 to 300 m, slope 0.06 or more, away from water) become
  forest, and general forest grows from existing forest onto dry sloping ground (steepest first), at most 8 cells out, to 26 % of the
  island's land. Small fragments are removed and real enclosed holes filled (`tidy`). No random seed is added to reach any share.
* **The owner, 2026-11-05, on the western bog share and usable land (relayed verbatim by ck-manager):** "66 is arbitrary target
  requirement, we dont need to strictly adhere to it. if there are named bogs/woods then they should be included ... as for the usable
  land hugging rivers, i think that's fairly realistic, we dont want each area of land to have the same amount of usable land, some areas
  are naturally more valuable and that is something we want to represent". So the change list's 66 % for the western regions is not a
  target: the western `share = 0.66` in `edits.toml` is only an upper cap, and the growth is bounded by the 10 km from the named places
  (over the original western districts the bog comes to, in ck-designer-1's prototype of r7, about 56 % in Erris, 60 % in Connemara and 55 % in west Donegal, plus 3 to
  10 % mountain). The Central Plain (43.7 % against 40 %) and Ulster (about 34 % against 25 %) are above their change-list figures
  because the uplands rule adds hill bog inside them.
* **Owner approval of the preview, 2026-11-05 (relayed verbatim by ck-manager, 54fbyx3b), at the three views (whole island step 10,
  Athlone step 40, Clonmacnoise step 60):** "yes i approve these views, they look good, draft 1 is mislabeled on the first view right?
  the one on the right is draft 4 correct? i would honestly accept either of these, but the one on the right looks better from a gameplay
  perspective". That approval was of draft 4, `edits.toml` r6 (sha256 `239ce3f2...`). The data before CK-194 was edits r7
  (sha256 `d4e4bcee...`), which differs from r6 by the named-site rules above. Edits r8 (sha256 `9898d782...`) adds only the two
  one-cell lake-island corrections below. **ck-manager's ruling, 2026-11-05:** "draft 5 needs no
  new owner sign-off. The owner approved draft 4 at the three views and separately required 'if there are named bogs/woods then they
  should be included'; draft 5 is draft 4 plus exactly that (63/63 named sites, Kilultagh 799 cells), visually almost identical at the
  three scales per the designer's check, and the owner asked to get back to work." ck-designer-1's check of draft 5 passed.
* **Usable land** is by suitability (rank of the elevation against its surroundings within 15 cells, of the slope and of the distance to
  the sea, a lake or a river line capped at 12 cells, box-blurred over 4 cells, best first), at mock-up 2's shares per terrain type, and
  patches under 20 cells are merged. The river lines are included so that usable land follows the inland valleys and lowlands, and it is
  **uneven on purpose**: some areas are naturally more valuable, and no rule here evens it out (`usable_local_radius`, which would, is not
  used).

| Edit | Area | Change | Reason | Confidence | Cells |
|---|---|---|---|---|---|
| `wexford-north-slob-sea` | North Slob, Wexford Harbour: lowest ground to sea | paint sea, depth 2 m; polygon of 4 points; stop rule: terrain pasture/forest/bog/marsh, elevation_max 4 | embanked and drained in the 1840s-50s; the medieval harbour was tidal flats | high | 20 |
| `wexford-north-slob-marsh` | North Slob, Wexford Harbour: low ground to marsh | paint marsh; polygon of 4 points; stop rule: terrain pasture/forest/bog, elevation_max 10 | embanked and drained in the 1840s-50s; the medieval harbour was tidal flats | high | 11 |
| `wexford-south-slob-sea` | South Slob, Wexford Harbour: lowest ground to sea | paint sea, depth 2 m; polygon of 4 points; stop rule: terrain pasture/forest/bog/marsh, elevation_max 3 | embanked and drained in the 1840s-50s | high | 2 |
| `wexford-south-slob-marsh` | South Slob, Wexford Harbour: low ground to marsh | paint marsh; polygon of 4 points; stop rule: terrain pasture/forest/bog, elevation_max 8 | embanked and drained in the 1840s-50s | high | 0 |
| `north-bull-island` | North Bull Island, Dublin Bay | paint sea, depth 2 m; box [53.365, 53.392, -6.155, -6.105]; stop rule: terrain pasture/forest/bog/marsh, elevation_max 3 | formed after the Bull Wall of the 1820s; leave the Clontarf-Dollymount mainland as it is (check on the preview) | high on the fact; the island's cells need a check | 22 |
| `dublin-liffey-mouth-sea` | Dublin, Liffey mouth, North Lotts, East Wall, Ringsend and the port: lowest ground to sea | paint sea, depth 2 m; polygon of 4 points; stop rule: terrain pasture/forest/bog/marsh, elevation_max 2 | 18th-20th-century quays and reclamation | high on the fact, low on the cell count | 7 |
| `dublin-liffey-mouth-marsh` | Dublin, Liffey mouth, North Lotts, East Wall, Ringsend and the port: low ground to marsh | paint marsh; polygon of 4 points; stop rule: terrain pasture/forest/bog, elevation_max 5 | 18th-20th-century quays and reclamation | high on the fact, low on the cell count | 31 |
| `belfast-harbour-sea` | Belfast Lough head, Queen's Island and the harbour estate: lowest ground to sea | paint sea, depth 2 m; box [54.595, 54.64, -5.935, -5.86]; stop rule: terrain pasture/forest/bog/marsh, elevation_max 2 | 19th-20th-century reclamation of the slob lands | high | 13 |
| `belfast-harbour-marsh` | Belfast Lough head, Queen's Island and the harbour estate: low ground to marsh | paint marsh; box [54.595, 54.64, -5.935, -5.86]; stop rule: terrain pasture/forest/bog, elevation_max 5 | 19th-20th-century reclamation of the slob lands | high | 18 |
| `cork-marshes-marsh` | Cork, the Lee's marshes and the Marina/Tivoli slobs: low ground to marsh | paint marsh; polygon of 4 points; stop rule: terrain pasture/forest/bog, elevation_max 12 | Cork stood on marsh islands (Corcach, marsh); the flats were drained and quayed from the 17th century | medium | 19 |
| `lough-foyle-levels-sea` | Lough Foyle, Myroe and Magilligan levels: lowest ground to sea | paint sea, depth 2 m; box [55.05, 55.17, -7.05, -6.9]; stop rule: terrain pasture/forest/bog/marsh, elevation_max 1 | 19th-century embankment | medium | 33 |
| `lough-foyle-levels-marsh` | Lough Foyle, Myroe and Magilligan levels: low ground to marsh | paint marsh; box [55.05, 55.17, -7.05, -6.9]; stop rule: terrain pasture/forest/bog, elevation_max 3 | 19th-century embankment | medium | 0 |
| `lough-swilly-inch-levels-sea` | Lough Swilly, Inch levels and Blanket Nook: lowest ground to sea | paint sea, depth 2 m; box [55.03, 55.08, -7.53, -7.45]; stop rule: terrain pasture/forest/bog/marsh, elevation_max 1 | 1850s embankment | medium | 2 |
| `lough-swilly-inch-levels-marsh` | Lough Swilly, Inch levels and Blanket Nook: low ground to marsh | paint marsh; box [55.03, 55.08, -7.53, -7.45]; stop rule: terrain pasture/forest/bog, elevation_max 5 | 1850s embankment | medium | 1 |
| `shannon-fergus-slobs-marsh` | Shannon and Fergus estuary slobs, around Shannon Airport: low ground to marsh | paint marsh; box [52.58, 52.75, -9.05, -8.65]; stop rule: terrain pasture/forest/bog, elevation_max 3 | embanked estuary slob; the airport stands on reclaimed land | medium | 172 |
| `strangford-north-marsh` | Strangford Lough, north end (Newtownards marshes): low ground to marsh | paint marsh; box [54.56, 54.6, -5.7, -5.65]; stop rule: terrain pasture/forest/bog, elevation_max 3 | 19th-century embankment | low | 3 |
| `lough-neagh-higher` | Lough Neagh's low shores | paint lake; box [54.45, 54.75, -6.65, -6.25]; stop rule: terrain pasture/forest/bog/marsh, elevation_max 11, near_water_max 1 | lowered by drainage schemes in the 1840s-50s and 1940s-50s; surface 9 m in the data, lake to 11 m one cell out | high on the lowering, low on the extent | 39 |
| `lough-neagh-margin` | Lough Neagh's new shore | paint marsh; box [54.45, 54.75, -6.65, -6.25]; stop rule: terrain pasture/forest/bog, elevation_max 12, near_water_max 1 | the wet margin of the higher lough, to 12 m | low | 13 |
| `shannon-callows` | Middle Shannon, Athlone to Portumna | paint marsh; polygon of 18 points; stop rule: terrain pasture/forest/bog, elevation_max 39 | the callows flooded each winter before the 19th-century drainage works; a 2 km corridor, floor to 39 m | medium | 313 |
| `suck-callows` | The Suck below Ballinasloe | paint marsh; polygon of 4 points; stop rule: terrain pasture/forest/bog, elevation_max 42 | tributary callows | low | 132 |
| `little-brosna-callows` | The Little Brosna | paint marsh; polygon of 4 points; stop rule: terrain pasture/forest/bog, elevation_max 42 | tributary callows | low | 117 |
| `upper-erne-shores` | Upper Lough Erne's shores | paint marsh; box [54.18, 54.35, -7.62, -7.35]; stop rule: terrain pasture/forest/bog, elevation_max 45, near_water_max 2 | flooded margins before the 1880s and 1950s drainage; surface 42 m in the data | low | 42 |
| `clear-draft-bog-forest` | The whole map | paint pasture; box [51.25, 55.55, -10.9, -5.3]; stop rule: terrain bog/forest | ck-manager ruling 2026-11-05: the draft's bog and forest were placed with fbm noise and are not existing features; cleared so placement starts only from named sites and terrain rules | n/a (method) | 144898 |
| `wood-glenconkeyne-killetra` | Glenconkeyne and Killetra (south Londonderry) | grow forest from forest and 1 known locations (each site's 5 x 5 cells that are pasture take the paint whatever the stop rule says), order near, to 60 % of the region's land; circle [54.8, -6.72, 15]; stop rule: terrain pasture, elevation_max 300, slope_min 0.01 | a large wood in 16th-17th-century accounts and maps; placed before the bogs (ck-manager ruling i9jc48iu) and grown from its centre over drier ground (slope at least 0.01), nearest first, to 60 % of the circle's land below 300 m; centre and size are a judgement | medium | 1659 |
| `wood-kilultagh` | Kilultagh (around Lisburn) | grow forest from forest and 1 known locations (each site's 5 x 5 cells that are pasture take the paint whatever the stop rule says), order near, to 60 % of the region's land; circle [54.5, -6.1, 10]; stop rule: terrain pasture, elevation_max 300 | a lowland wood in the flat Lagan valley, so it has no slope_min (with one its centre was excluded and it took 0 cells); a large wood in 16th-17th-century accounts and maps; placed before the bogs (ck-manager ruling i9jc48iu) and grown from its centre over drier ground (slope at least 0.01), nearest first, to 60 % of the circle's land below 300 m; centre and size are a judgement | medium | 799 |
| `wood-the-fews` | The Fews (south Armagh) | grow forest from forest and 1 known locations (each site's 5 x 5 cells that are pasture take the paint whatever the stop rule says), order near, to 60 % of the region's land; circle [54.13, -6.6, 10]; stop rule: terrain pasture, elevation_max 300, slope_min 0.01 | a large wood in 16th-17th-century accounts and maps; placed before the bogs (ck-manager ruling i9jc48iu) and grown from its centre over drier ground (slope at least 0.01), nearest first, to 60 % of the circle's land below 300 m; centre and size are a judgement | medium | 779 |
| `wood-shillelagh` | Shillelagh (south Wicklow) | grow forest from forest and 1 known locations (each site's 5 x 5 cells that are pasture take the paint whatever the stop rule says), order near, to 60 % of the region's land; circle [52.75, -6.53, 10]; stop rule: terrain pasture, elevation_max 300, slope_min 0.01 | a large wood in 16th-17th-century accounts and maps; placed before the bogs (ck-manager ruling i9jc48iu) and grown from its centre over drier ground (slope at least 0.01), nearest first, to 60 % of the circle's land below 300 m; centre and size are a judgement | medium | 735 |
| `wood-wicklow-glens` | The Wicklow glens: Glenmalure, Glendalough, the Vale of Avoca | grow forest from forest and 1 known locations (each site's 5 x 5 cells that are pasture take the paint whatever the stop rule says), order near, to 60 % of the region's land; circle [52.95, -6.3, 12]; stop rule: terrain pasture, elevation_max 300, slope_min 0.01 | a large wood in 16th-17th-century accounts and maps; placed before the bogs (ck-manager ruling i9jc48iu) and grown from its centre over drier ground (slope at least 0.01), nearest first, to 60 % of the circle's land below 300 m; centre and size are a judgement | medium | 1065 |
| `wood-leix-offaly` | The Leix and Offaly woods, the Slieve Bloom foothills | grow forest from forest and 1 known locations (each site's 5 x 5 cells that are pasture take the paint whatever the stop rule says), order near, to 60 % of the region's land; circle [53.05, -7.5, 15]; stop rule: terrain pasture, elevation_max 300, slope_min 0.01 | a large wood in 16th-17th-century accounts and maps; placed before the bogs (ck-manager ruling i9jc48iu) and grown from its centre over drier ground (slope at least 0.01), nearest first, to 60 % of the circle's land below 300 m; centre and size are a judgement | medium | 1315 |
| `wood-aherlow` | The Glen of Aherlow | grow forest from forest and 1 known locations (each site's 5 x 5 cells that are pasture take the paint whatever the stop rule says), order near, to 60 % of the region's land; circle [52.42, -8.1, 8]; stop rule: terrain pasture, elevation_max 300, slope_min 0.01 | a large wood in 16th-17th-century accounts and maps; placed before the bogs (ck-manager ruling i9jc48iu) and grown from its centre over drier ground (slope at least 0.01), nearest first, to 60 % of the circle's land below 300 m; centre and size are a judgement | medium | 366 |
| `wood-killarney` | The Killarney oakwoods | grow forest from forest and 1 known locations (each site's 5 x 5 cells that are pasture take the paint whatever the stop rule says), order near, to 60 % of the region's land; circle [52.0, -9.52, 10]; stop rule: terrain pasture, elevation_max 300, slope_min 0.01 | a large wood in 16th-17th-century accounts and maps; placed before the bogs (ck-manager ruling i9jc48iu) and grown from its centre over drier ground (slope at least 0.01), nearest first, to 60 % of the circle's land below 300 m; centre and size are a judgement | medium | 558 |
| `wood-duhallow` | Duhallow | grow forest from forest and 1 known locations (each site's 5 x 5 cells that are pasture take the paint whatever the stop rule says), order near, to 60 % of the region's land; circle [52.15, -9.05, 12]; stop rule: terrain pasture, elevation_max 300, slope_min 0.01 | a large wood in 16th-17th-century accounts and maps; placed before the bogs (ck-manager ruling i9jc48iu) and grown from its centre over drier ground (slope at least 0.01), nearest first, to 60 % of the circle's land below 300 m; centre and size are a judgement | medium | 1029 |
| `wood-slieve-aughty` | Slieve Aughty | grow forest from forest and 1 known locations (each site's 5 x 5 cells that are pasture take the paint whatever the stop rule says), order near, to 60 % of the region's land; circle [52.98, -8.6, 14]; stop rule: terrain pasture, elevation_max 300, slope_min 0.01 | a large wood in 16th-17th-century accounts and maps; placed before the bogs (ck-manager ruling i9jc48iu) and grown from its centre over drier ground (slope at least 0.01), nearest first, to 60 % of the circle's land below 300 m; centre and size are a judgement | medium | 1434 |
| `wood-lough-erne` | The shores of Lough Erne | grow forest from forest and 1 known locations (each site's 5 x 5 cells that are pasture take the paint whatever the stop rule says), order near, to 60 % of the region's land; circle [54.4, -7.7, 15]; stop rule: terrain pasture, elevation_max 300, slope_min 0.01 | a large wood in 16th-17th-century accounts and maps; placed before the bogs (ck-manager ruling i9jc48iu) and grown from its centre over drier ground (slope at least 0.01), nearest first, to 60 % of the circle's land below 300 m; centre and size are a judgement | medium | 883 |
| `bog-seed-lake-edges-central` | Central Plain lake edges | paint bog; polygon of 10 points; stop rule: terrain pasture, elevation_min 35, elevation_max 140, slope_max 0.01, near_water_max 1 | raised bogs began as fen over lake basins; very flat low ground touching a lough seeds bog | medium | 95 |
| `bog-seed-lake-edges-ulster` | Ulster lowland lake edges | paint bog; polygon of 4 points; stop rule: terrain pasture, elevation_min 15, elevation_max 120, slope_max 0.01, near_water_max 1 | as bog-seed-lake-edges-central | medium | 30 |
| `bog-central-plain` | Central Plain raised bogs: Bog of Allen, Offaly, Westmeath, Longford, south Roscommon, east Galway, north Tipperary | grow bog from bog and 12 known locations (each site's 5 x 5 cells that are pasture/forest/marsh take the paint whatever the stop rule says), order wet, to 40 % of the region's land; polygon of 10 points; stop rule: terrain pasture, elevation_min 35, elevation_max 140, slope_max 0.02, relative_elevation_max 0, relative_radius 10 | raised bogs filled basins before cutting and reclamation (the Bog of Allen alone about 1,000 km2); grown from named bogs and lake edges into connected basin ground (flat, low, below its local mean), lowest first, to 40 % of the region's land | high on the region and scale; outlines follow the basins | 19149 |
| `bog-ulster-lowlands` | Ulster lowland raised bogs: the Lough Neagh basin and the Bann valley | grow bog from bog and 4 known locations (each site's 5 x 5 cells that are pasture/forest/marsh take the paint whatever the stop rule says), order wet, to 25 % of the region's land; polygon of 4 points; stop rule: terrain pasture, elevation_min 15, elevation_max 120, slope_max 0.02, relative_elevation_max 0, relative_radius 10 | cut and reclaimed since the 17th century; grown from named bogs and lake edges into connected basin ground to 25 % of the region's land | medium | 2774 |
| `bog-erris` | Western blanket bog: Erris and north-west Mayo | grow bog from bog and 12 known locations (each site's 5 x 5 cells that are pasture/forest/marsh take the paint whatever the stop rule says), order wet, to 66 % of the region's land, at most 20 cells from a seed; polygon of 4 points; stop rule: terrain pasture, elevation_max 300, slope_max 0.1, far_from_water_min 2 | lowland blanket bog covered most land below 300 m around these districts; grown from named bog places, lowest first, at most 20 cells (10 km) from a seed, not onto the coastal strip; at most two thirds of the region's land (a cap, not a target) | high on the district; seed places medium | 5448 |
| `bog-connemara` | Western blanket bog: Connemara | grow bog from bog and 13 known locations (each site's 5 x 5 cells that are pasture/forest/marsh take the paint whatever the stop rule says), order wet, to 66 % of the region's land, at most 20 cells from a seed; polygon of 4 points; stop rule: terrain pasture, elevation_max 300, slope_max 0.1, far_from_water_min 2 | lowland blanket bog covered most land below 300 m around these districts; grown from named bog places, lowest first, at most 20 cells (10 km) from a seed, not onto the coastal strip; at most two thirds of the region's land (a cap, not a target) | high on the district; seed places medium | 4389 |
| `bog-west-donegal` | Western blanket bog: west Donegal | grow bog from bog and 11 known locations (each site's 5 x 5 cells that are pasture/forest/marsh take the paint whatever the stop rule says), order wet, to 66 % of the region's land, at most 20 cells from a seed; polygon of 4 points; stop rule: terrain pasture, elevation_max 300, slope_max 0.1, far_from_water_min 2 | lowland blanket bog covered most land below 300 m around these districts; grown from named bog places, lowest first, at most 20 cells (10 km) from a seed, not onto the coastal strip; at most two thirds of the region's land (a cap, not a target) | medium | 4717 |
| `bog-uplands-rule` | Upland blanket bog island-wide (Wicklow, Slieve Bloom, the Sperrins, the Antrim plateau, Cuilcagh, the Ox Mountains, the Cork-Kerry hills and the others) | paint bog; box [51.25, 55.55, -10.9, -5.3]; stop rule: terrain pasture, elevation_min 150, slope_max 0.1 | blanket bog covered open hill ground and plateaus continuously; every pasture cell from 150 m with slope up to 0.10 is bog (a terrain rule, after the regional growth so it does not seed it) | high on the direction | 41408 |
| `tidy-bog` | The whole map | tidy bog: patches under 12 cells go back to pasture, enclosed holes of that size fill; never a patch that holds or touches a named site | no bog patch under 12 cells (3 km2); small pasture holes enclosed by bog become bog | n/a (method) | 4835 |
| `forest-steep-valley-sides` | Steep valley sides island-wide | paint forest; box [51.25, 55.55, -10.9, -5.3]; stop rule: terrain pasture, elevation_min 60, elevation_max 300, slope_min 0.06, far_from_water_min 3 | steep ground from 60 to 300 m, away from water, was left wooded when lowland was cleared: every such pasture cell is forest (a terrain rule) | medium | 25442 |
| `forest-dry-slopes` | Forest island-wide, onto drier sloping ground next to existing woods | grow forest from forest, order dry_slope, to 26 % of the region's land, at most 8 cells from a seed; box [51.25, 55.55, -10.9, -5.3]; stop rule: terrain pasture, elevation_min 20, elevation_max 300, slope_min 0.015, far_from_water_min 2 | woodland was far more extensive before the 17th-century clearances than the roughly 12 % quoted for about 1600; grows from existing forest up to 8 cells (4 km) out, steepest and highest first, to 26 % of land; flat, well-drained lowland stays mostly farmland | medium | 53549 |
| `tidy-forest` | The whole map | tidy forest: patches under 10 cells go back to pasture, enclosed holes of that size fill; never a patch that holds or touches a named site | no forest patch under 10 cells | n/a (method) | 7707 |
| `tidy-bog-again` | The whole map | tidy bog: patches under 12 cells go back to pasture, enclosed holes of that size fill; never a patch that holds or touches a named site | forest placement can cut bog patches; repeat the bog tidy | n/a (method) | 6 |

Final shares of land (not lake) after the edits, against the draft and the change list's rough targets:

| Share of land | The draft | After the edits | Rough target |
|---|---|---|---|
| pasture | 48.0 % | 41.1 % | about 40 % |
| forest | 21.3 % | 26.7 % | about 25 % |
| bog | 20.8 % | 22.1 % | about 23 % |
| marsh | 5.5 % | 5.7 % | about 7 % |
| mountain | 4.4 % | 4.4 % | unchanged |
| usable | 26.7 % | 25.4 % | the owner's "fine for now" |

Patch sizes after the edits (cells; draft 2 had 1310 bog patches with a median of 3): bog: 257 patches, smallest 12, median 46, largest 19063; forest: 239 patches, smallest 10, median 45, largest 25101.

Why so few cells change in some reclamations (the Wexford slobs, Lough Swilly, Strangford): at 500 m the elevation grid already holds much of
the slob land at 0 m (sea), and the cells are large; the edits change what the grid shows as land at the stated heights.

### Known limitation (deferred by the owner, 2026-11-05)

River courses are hand-anchored or Natural Earth lines snapped to a 500 m valley floor. Some run straight where the floor is flat, for
example the Shannon callows near Clonmacnoise; the owner, 2026-11-05: "the rivers look ok, some are too straight, but i think we can safely
defer that work". ck-manager tracks it.

Hand edits made after `edits.toml` go here, one row each (area, change, reason, who). None has been made.

### Edits r8 (CK-194), owner direction 2026-11-06

The owner directed, verbatim, "Make the islands land" (relayed by ck-manager). Edits r8 applies this to the one map cell containing
each monastery below. It changes only the terrain at those two cells; elevation and rivers are unchanged. The usable ranking is global,
so its output also changes at another cell; the CK-194 submission lists every changed usable cell and its before/after value.

| Edit | Area | Change | Reason | Confidence | Cells |
|---|---|---|---|---|---|
| `island-iniscealtra` | Inis Cealtra (Holy Island), Lough Derg, crop cell (325, 599) | paint pasture | The monastery of 768 is on an island smaller than this map's cell; the derivation made that cell lake. Elevation remains 30 m. | high on site, low on extent | 1 |
| `island-inisfallen` | Inis Faithlinn (Inisfallen), Lough Leane, crop cell (181, 788) | paint pasture | The monastery of 768 is on an island smaller than this map's cell; the derivation made that cell lake. Elevation remains 22 m. | high on site, low on extent | 1 |
