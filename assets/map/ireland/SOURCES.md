# Ireland's map data: sources, derivation and guesswork

This directory is Ireland and its associated offshore islands in the map format of `ck-map` (`docs/map-design.md` section 9.5), for
milestone 6, slice 5 (CK-181), updated by CK-194 for the Ireland-768 scenario and CK-234 for computed climate, fertility and glacier
layers. **It is a draft toward medieval Ireland, approved by the owner at three preview views on 2026-11-05 (below), with Ireland's water (rivers, loughs, estuaries) the owner-approved list of CK-350 ("Ireland's water", below); no generated layer
has a hand edit: terrain includes the r8 and CK-234 signed edits, and the climate, glacier and fertility layers are generated from that
terrain and elevation. The full draft-to-output chain is reproducible byte for byte by the scripts and `edits.toml`.** The elevation, the
coast and the larger loughs come from real data; terrain, climate, fertility and most river courses are modeled or estimated (see the
guesswork log).

| File | What it is | Kept by |
|---|---|---|
| `map.toml` | the crop and the layer names | `derive_elevation.py` |
| `elevation.png` | 16-bit grey, `elevation + 11000` in metres | `derive_elevation.py` (fully reproducible) |
| `terrain.toml` | the terrain types, their keys, palette and rules | by hand |
| `terrain.png` | the terrain type of each cell, as its key colour | `draft_terrain.py`, then `apply_edits.py` (edits r8, CK-234 and CK-350's mask and hand entries; no hand edit) |
| `fertility.png` | 8-bit grayscale fertility, stored as `min(250, floor(fertility_pm / 4))`; decoded per mille is the sample times four | `apply_edits.py` |
| `climate_temperature.png` | 16-bit grayscale, mean temperature in tenths of °C with a +1000 offset | `apply_edits.py` |
| `climate_gdd.png` | 16-bit grayscale growing degree days above 5 °C | `apply_edits.py` |
| `climate_rainfall.png` | 16-bit grayscale annual rainfall in mm | `apply_edits.py` |
| `climate_glacier.png` | 8-bit grayscale; 255 means glacier, 0 means no glacier | `apply_edits.py` |
| `rivers.toml` | the river network (format 2: named rivers with tiers, and their stretches with points, widths and ends) | `import_waters.py build` from `waters.toml` (CK-350); no hand edit |
| `edits.toml` | the owner-delegated corrections toward medieval Ireland: one entry per change, with its reason (cited by `id` in the guesswork log below) | by hand (ck-designer-1, owner-delegated); r8 adds the monastery islands, CK-234 two terrain corrections and CK-350 the mask's entries and the owner's hand entries for the stranded pools |
| `apply_edits.py` | applies `edits.toml` over the draft, in order, deterministically | |
| `edits.applied` | records that the edits were applied to these images (the SHA-256 of `edits.toml`) | `apply_edits.py` |
| `ireland_grid.py` | the world grid and tile arithmetic the scripts share | |
| `import_waters.py` | `fetch`, `propose` and `build` for the rivers, loughs and tidal outlines (CK-348; see "Water import" below) | |
| `waters.toml` | the owner-approved list of rivers, loughs, sea entries, reservoirs left out and corrections (CK-350) | by hand (ck-designer-7); each list approved by the owner |
| `water.png` | the mask of the cells that become sea or lake, or that a redrawn lough gives back to land (4-bit palette PNG in the terrain keys) | `import_waters.py build` from `waters.toml` (CK-350) |

**The committed `elevation.png` is the elevation draft with every edit applied; `terrain.png` is the terrain draft plus all of `edits.toml`
(r8, the two CK-234 corrections, and CK-350's mask and hand entries). The five computed climate, glacier and fertility layers are generated after
all edits, and `rivers.toml` and `water.png` are built by `import_waters.py` from `waters.toml`. Hand-kept inputs are listed in the guesswork log.**
Running `draft_terrain.py` again reproduces the first draft only and destroys those edits, so it refuses to overwrite its file unless `--force` is
given; `apply_edits.py` refuses to run twice over the same files (`edits.applied`) unless `--force` is given. `derive_elevation.py` can be run again
at any time, but its `--force` regenerates `elevation.png` and so removes the edits' sea paint: after it, run `draft_terrain.py --force` and
`apply_edits.py` again. The chain is: derive elevation, draft terrain, then apply every terrain edit followed by climate and fertility derivation.
`derive_rivers.py` is gone (CK-350): its output is no longer a file of the map, and running it would have replaced the network with the old format;
what it did is kept in "The first draft" below.

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

## Water import (CK-348, slice 3 of the rivers design; CK-354, the sea end and the two surveys; CK-355, the drained area)

`import_waters.py` turns the public river, lough and tidal surveys into drafts of Ireland's water data: a hand-kept list
(`waters.toml`), a `rivers.toml` in the new network format built from it, a mask of the cells that become sea and lough
(`water.png`), and a report (`waters-report.md`). **Nothing in the committed map data comes from it yet**: it changes none of
`rivers.toml`, `terrain.png`, `edits.toml` or the computed layers. Slice 4 has the owner approve the river and lough lists, applies
the mask with `apply_edits.py` and commits the result. The surveys are today's channels; only what `waters.toml` corrects is undone.

### Commands

From a checkout, with `numpy` and `Pillow` (`fetch` borrows `ireland_grid.download`, which needs numpy; the rules themselves need
neither and are tested in the project's Python by `tests/test_map_waters.py`):

```
python3 assets/map/ireland/import_waters.py fetch   --source ../ireland-source [--force]
python3 assets/map/ireland/import_waters.py propose --source ../ireland-source --out DIR [--map assets/map/ireland] [--force]
python3 assets/map/ireland/import_waters.py build   --source ../ireland-source --waters DIR/waters.toml --out DIR \
        [--locations assets/scenarios/ireland-768/locations.toml] [--map assets/map/ireland] [--force]
```

- `fetch` downloads the confirmed layers into the cache (not committed) and writes `waters-manifest.json`: each layer's address, the
  day, the number of features and the SHA-256 of what was stored. Northern Ireland's two downloads are converted from Irish Grid
  (EPSG:29902) to latitude and longitude as they are stored (the conversion agrees with the EPA's own reprojection to under a
  centimetre on shared vertices, and to under a metre in median over the 812 lake outlines' centres). A layer whose licence is not
  confirmed is not fetched, is named in the manifest and the report, and is a one-line switch in `LAYERS` (`confirmed=False`).
  It also fetches the Irish EPA's coastal water bodies (`epa-coastal`, 112 outlines), which a `[[sea]]` entry can take by code.
- `propose` writes a first `waters.toml` by the rule the mock-ups used: Irish rivers with 15 km or more at stream order 5 and above,
  each Northern Ireland water body with a segment of order 5 or above (named by hand: the survey has no names), tiers from the
  drained area (1,200 km2 and 300 km2), the loughs a proposed river touches, every lough that overlaps a lake cell the map has today (as `redrawn`, however small the overlap), Lough Neagh
  with `keep_existing` and the two island cells as `[[keep]]`, and the reservoirs "to be checked". It works on the lines of
  "One line in both surveys" below: a line of both surveys counts for the river of its Irish name where that river is proposed, and
  otherwise for its water body, and a water body left with no line of its own is not proposed. It also lists the rivers a
  proposed river ends at, so that the network reaches the sea, and marks each with why; and where a river ends at a line that has neither
  a name nor a water body (the Quoile's last reach, the tidal Foyle) it lists that line's segment code in the same river's `segments`, and goes on
  down the line it runs into, with a comment saying how many segments were added. It writes no `[[sea]]` entry. A river it cannot build
  (it has no way to the sea, below) is marked "not built".
- `build` reads `waters.toml`, the cache and the map directory's `map.toml`, `terrain.png` and `elevation.png`, and writes
  `rivers.toml`, `water.png` and `waters-report.md` to `--out` (never to the map directory unless it is named). The same inputs
  give the same bytes. `--locations` keeps each location's cell as it is and reports each location on or beside new water. The report
  names the cells themselves (`column,row` from the crop's corner, the pixel of `water.png`): each estuary's new sea cells with the terrain they
  were, the tidal cells left as river, the old sea cells cut off from the open sea, each lough's new cells, and the cells returned to land
  with the terrain each takes.
- `apply_edits.py` gains the region `{ mask = "water.png" }`: it covers the cells whose pixel is the key colour of the edit's
  `paint`, for any terrain, so slice 4 applies the mask with one edit for the sea, one for the lake and one for each land terrain.
  The image must be the crop's size and hold only black ("no change") and the terrain keys. Every other edit is unchanged.

### `waters.toml`

`[settings]` (`a`, `b`, `simplify_m`, `tier_steps`); `[[river]]` (`id`, `name`, `tier`, `source = { epa_names = [...], rwb_codes = [...], segments = [...] }`;
`segments` lists survey segment codes, for a line that has no name and no water-body code: the tidal Foyle, the Quoile's last reach);
`[[lough]]` (`name`, `source` = the survey's lake code, `mode` = `"added"` or `"redrawn"`, optional `keep_existing` with a `reason`);
`[[keep]]` (`id`, `reason`, `point`); `[[reservoir]]` (`name`, `source`, `reason`); `[[correction]]` (`id`, `reason`, `confidence`,
`river`, and one of `width_m` (one number or first and last) or `width_factor` (optionally for one `stretch`), `omit` (survey segment
codes) or `course` with `replaces` (a hand-drawn course in their place)); `[[sea]]` (`name`, `reason`, and exactly one of `source`, the code of
a coastal outline in the cache, or `points`, a list of `[latitude, longitude]`: the cells that hold them; how a mouth narrower than a cell is opened).
Unknown keys are refused, naming the entry. A river with an
empty name is refused: `rivers.toml` is built, not hand-edited, so D9's "corrected by hand" is done here, where a correction keeps its
reason through a rebuild. A rebuild replaces `rivers.toml` whole.

### Layers, licences and attribution (D8; each confirmed on the day read, 2026-11-16, and the coastal layer 2026-11-19)

Only a layer whose licence the publisher states is used. Both licences ask for attribution only (not share-alike).

| Layer (cache file) | Address | Licence evidence | Attribution |
|---|---|---|---|
| Irish EPA river network routes (`epa-rivers`, stream order 4 and above and every line through a lough or tidal water) | WFS `EPA:WATER_RIVNETROUTES`, <https://gis.epa.ie/geoserver/EPA/ows> | the EPA's own catalogue record `c4043e19-38ec-4120-a588-8cd01ac94a9c` (linked from the WFS capabilities): "Data is for public use under Creative Commons CC-By 4.0", CC BY 4.0 | Contains Environmental Protection Agency (Ireland) data licensed under CC BY 4.0. |
| Irish EPA lake water bodies (`epa-lakes`) | WFS `EPA:WFD_LAKEWATERBODIES_CYCLE3` | the EPA's record `00e92dc9-cb26-4858-889c-0b9b6ea9c9be`, same statement | as above |
| Irish EPA transitional water bodies (`epa-tidal`) | WFS `EPA:WFD_TRANSITIONALWATERBODIES_CYCLE3` | the EPA's record `5b3cd1d5-b2d7-4210-a821-0f6231129d58`, same statement | as above |
| Irish EPA coastal water bodies (`epa-coastal`; the outlines a `[[sea]]` entry takes by code, among them Cork Harbour `IE_SW_060_0000`, Outer Cork Harbour `IE_SW_050_0000`, Lough Swilly `IE_NW_220_0000` and Lough Foyle `GBNIIE6NW250`; Strangford Lough is not in it) | WFS `EPA:WFD_COASTALWATERBODIES_CYCLE3` | the EPA's own record `0b6c2e90-e56f-4cce-927e-57e86e9a60a3` (linked from the WFS capabilities; read 2026-11-19, sha256 of the record `388f60e8...`): "Data is for public use under Creative Commons CC-By 4.0" and the licence's address as its other constraint | as above |
| DAERA Lakes of Northern Ireland (`ni-lakes`) | ArcGIS service `Lakes_of_Northern_Ireland`, <https://services-eu1.arcgis.com/kswen6BYexuc1SUk/arcgis/rest/services> | the publisher's ArcGIS item `5acc99bfd09e432a9db6d72272b96174` (owner DAERA_OpenContent) names this exact service and states the Open Government Licence v3.0 (found by ck-reviewer-1; a copy is kept with the CK-348 spec reviews, sha256 `26e341c2...`) | DAERA NIEA, contains OSNI information licensed under OGL v3.0. |
| DAERA Northern Ireland River Segments (`ni-rivers`, stream order 4 and above) | download <https://www.daera-ni.gov.uk/sites/default/files/publications/doe/riversegmentgml.zip> (GML, Irish Grid), the 2010 edition | DAERA's own page <https://www.daera-ni.gov.uk/articles/wmu-spatial-datasets> links "Northern Ireland River Segments" to NIEA Water Management Unit's OpenDataNI record, whose licence is the UK Open Government Licence, and whose files are this download | Contains public sector information licensed under the Open Government Licence v3.0. Source: Northern Ireland Environment Agency Water Management Unit, DAERA. |
| DAERA Northern Ireland Transitional Waters 2009-2015 (`ni-tidal`) | download <https://admin.opendatani.gov.uk/dataset/42e0d654-e973-4c45-83a4-c19c57b33a90/resource/3dbb9f72-79db-41ee-afd5-cb5b935914db/download/transitionalwaterbodiesgml.zip> (GML, Irish Grid), the 2017 file | DAERA's page above links "Northern Ireland Transitional Waters (2009 - 2015)" to the OpenDataNI record, licence UK Open Government Licence, whose files include this download | Contains public sector information licensed under the Open Government Licence v3.0. Source: Northern Ireland Environment Agency, DAERA. |

The licence is OGL v3.0 (<https://www.nationalarchives.gov.uk/doc/open-government-licence/version/3/>) and CC BY 4.0
(<https://creativecommons.org/licenses/by/4.0/>). **The ArcGIS services for DAERA's river segments (`RiverSegments`) and transitional
waters (`TransitionalWaterBodiesNewURLs`, layer 1) are not used: their ArcGIS items carry no licence text.** The downloads are older
editions than those services, so their features differ (decision of ck-manager, 2026-11-16): River Segments has 257
water bodies at stream order 4 and above against 194 in the 2020 service (111 codes and 240 object ids in common), and Transitional
Waters has 7 estuaries (Quoile, Conns Water, Foyle and Faughan, Bann, Lagan, Newry, Roe) against 6 in the service (the Foyle split
into Upper Foyle and Foyle Harbour, the Roe absent). The owner approves the river and estuary lists in slice 4, where any difference
that matters is corrected by hand. OpenStreetMap stays unused.

**A DAERA coastal layer** was looked for (2026-11-19), as the tidal outlines were. DAERA's own datasets page links "Marine Water Bodies"
(<https://www.daera-ni.gov.uk/publications/marine-water-bodies>, coastal and transitional waters submitted for Water Framework Directive reporting 2016, an SHP
and a GML download) and "Coastal Interbasins" (an OpenDataNI record, not outlines of coastal waters). The Marine Water Bodies page states no licence and
OpenDataNI's catalogue could not be read from here (403), so no licence is confirmed and **the layer is not used** (import rule 3). The four inlets
the owner chose need none: Cork Harbour and Lough Swilly are in the EPA's coastal layer, and Lough Foyle's and Strangford Lough's mouths are opened by
`points`.

### What it cannot do, and what to know

- Northern Ireland's rivers have no names, so each is named by hand before `build` (a raw proposal is refused for its empty names).
- **Where the sea is (CK-354).** The end of a survey line that lies in an open-sea cell (a sea cell whose 5 x 5 box is all sea) is at the sea,
  whether or not the line stops there. Nothing else is a mouth for being tidal or near the coast, so the dead end of a small tidal side line is not
  a mouth and the line runs into its channel (unlisted, it is not built; listed by name, it ends at the channel). A piece of network that reaches
  no open sea takes one mouth: of its loose ends, the one with the shortest way to open sea (the run below), ties to the first by position;
  where none has one, the one nearest a sea cell within 30 cells. Joining pieces through a lough comes first.
- **The run to open sea (CK-354).** Where a stretch ends short of open sea with nothing onward, the tool adds: a first step to the centre of the
  nearest sea cell, when the last point's own cell is not sea (at most 3 cells, the larger of the row and column distances; ties by row, then
  column); then the shortest way through four-connected sea cells to the nearest open-sea cell, written as the centres of the cells where it turns and
  of its last cell. Thinning keeps the stretch's last survey point and every point the run adds, so a written leg of the run lies in sea cells (the
  first step alone may cross land). The report lists each run with its length. **A river with no such way is refused**, naming its last point and how
  many cells it is from the sea (a pool of sea with no open sea is 0): give it a hand-drawn course or the sea it lacks (`[[sea]]`); it is never sent
  over land. A course may be a river's last reach: where the segments it replaces met the rest of the river at one point only, the course starts
  there and its far end is free.
- **One line in both surveys (CK-354).** An Irish-survey segment and a DAERA segment with the same two end points (after the tool's rounding and
  snapping) and the same length to within 2 m or 1 % of the Irish line's length, whichever is larger, are one line. Pairing is one to one (the
  closest lengths first, then by code). The kept line has DAERA's points, water-body code and segment code, and the Irish survey's segment code, name,
  mark and lake code; **both codes name it** wherever a list gives a segment code (`segments`, a correction's `omit` and `replaces`). The order of claims:
  the river that lists it in `segments`; failing that, its Irish name; failing that, its water body; failing that, a river that runs on into it.
  A line listed in the `segments` of two rivers (by either code) is refused, naming both codes and both rivers; so is a code the survey does not have.
  The report lists, for each river, the lines of its entries another river has, and "Courses built twice": every pair of stretches within 25 m of each
  other for 300 m or more (local ground distance; for each segment of a stretch the part within 25 m of the other stretch's line segments is found exactly
  and summed, so vertices do not change a result; the lesser of what each runs beside the other). The tool does not reconcile lines the two surveys draw
  differently (a few metres apart, or cut at different points): the list takes one of them with `segments` and `omit`.
- **DAERA's marks (CK-354).** DAERA's `SEGMENT_TYPE` is read: `L` is a lough line and `T` a tidal one, as the Irish survey's marks are. `fetch` still
  leaves out DAERA's lines below stream order 4, marked or not.
- **`[[sea]]` (CK-354).** Its cells (those more than half inside the coastal outline, or those that hold its points) join the tidal cells and are decided
  with them by the same rule below: a cell that is not already sea or lake and is not kept becomes sea when it is joined to the map's edge through sea cells
  and other candidate cells. The report lists each entry: its cells that become sea (by the terrain they were), its cells that are not joined, its kept cells,
  and says where an entry changes nothing.
- A cell becomes sea only if more than half of it is inside a tidal outline (or a `[[sea]]` entry) and it is joined to the open sea (the sea cells that touch
  the crop's edge) through sea cells by their four neighbours. Narrow estuaries therefore stay river stretches.
- **The drained area of a stretch (CK-355).** Every cell that is not sea counts with its own area (lough cells too). It goes down the elevation grid (depressions
  filled from the sea and the crop's edge, then each cell's steepest neighbour, on the terrain with the new water) until it comes to a cell that holds a line of
  the survey's graph, or to the sea; a cell that holds several lines gives its area to the one of the highest stream order, then the first by segment code.
  Only lines of the graph collect: a hand-drawn course collects through its own cells, the lines it stands in for and the lines a correction leaves out collect
  nothing, and a run through sea cells collects nothing. Ground that leaves the crop's edge over land belongs to nothing. Along the survey, a line has what its cells
  collected plus what is sent into its upper end, and passes it on as the builder wrote the river: into the next line of its stretch, at the end of its stretch into
  the first line of the stretch it ends at (or to the sea); a line in no written stretch passes it to one line leaving its lower end, a written line before an
  unwritten one, then the one whose lower end is nearer a mouth, then the first by segment code. A stretch has, at its last point, the area at the lower end of its
  last line and, at its first, what is sent into its first line (a headwater: at most its own area and at least 1 km2). No stretch, id or course changes, so an
  area never falls along a stretch or to the stretch it ends at, and the report adds up the ground (not sea = to lines + straight to the sea + off the crop's edge).
  Tiers are the list's; `propose` suggests them from these areas. The areas are a surveyed-network reading of a 500 m grid and stay approximate: limestone country
  where water runs underground (the Fergus, the Cong) and the lough mazes (Lough Oughter, Upper Lough Erne) are wrong, and a hand width (`width_m`) is the
  remedy where it shows. `a = 2.3` and `b = 0.4` are starting values, to be fitted in slice 4.
- `water.png` has one specified encoding, written by the tool itself (not Pillow): a 4-bit palette PNG (black, then the seven terrain
  keys in the order sea, lake, pasture, forest, bog, marsh, mountain), filter 0 on every row and zlib *stored* blocks of up to 65,535
  bytes. The bytes are therefore the same on every machine, whatever Pillow or zlib is installed (`tests/test_map_waters.py` checks
  them against fixed bytes and decodes them with `zlib` only). It is about 0.36 MB.
- `build` checks the whole river format (CK-347 format rules 2 and 3) on the final terrain, after thinning, rounding and
  corrections, and refuses before it writes anything, naming each river and stretch at fault: a width not above 0 and at most
  10,000 m, a join that is not exact, a sea end that is not in open sea, and the rest. A stretch is kind `"lough"` only where the
  map has the lough under it (rule 7, r4): a stretch the survey marks as a lough line with no segment midpoint in a lake cell of the
  terrain the mask will make is written as `"river"` (its ends and id unchanged, not merged with its neighbours), so the built file
  holds no lough stretch that format rule 3 would refuse. The report lists each such line with its reason (four, spec r6): the survey gives it no
  lake outline (no lake code, or a code matching no outline); its lake has an outline that is not in the list (with the outline's area, so the owner can add that lough in slice 4);
  its lough is in the list and on the map, but no written segment midpoint of the stretch is in a lake cell (Upper Lough Erne's islands; the line may still touch a lake cell between midpoints, so the report does not say it crosses only land); or its lough is in the list,
  but no cell of it is lake on the map, with the number of cells its outline covers (a listed outline smaller than half a cell makes no lake; a lough
  is on the map when at least one cell its outline covers is a lake cell of the terrain the mask makes). A stretch whose segments carry several
  lake codes lists each with its own reason. Lake codes and names are given wherever the survey has them (spec r6, rule 11).

## Ireland's water (CK-350, slice 4 of the rivers design)

Ireland's rivers, loughs and estuaries are now data the owner approved, list by list, in ck-designer-7's tab (D14). `waters.toml` holds 106 rivers, 43 loughs, 11 sea entries, 2 kept cells, 14 reservoirs left out and 45 corrections.
`import_waters.py build` reads it with the frozen baseline and writes `water.png` and `rivers.toml`; `apply_edits.py` applies the mask and the hand entries
with the rest of `edits.toml` and regenerates the climate, glacier and fertility layers once over the finished terrain (CK-351's drainage rule). Nothing else of the
map changes by hand: no generated layer has a hand edit.

**The owner's approvals** (each recorded verbatim in `.pipeline/coordination/rivers-design/`, the first eight hex digits of its SHA-256):
  - `owner-direction-designer-r10.md` (`b4961e9b`): the four big inlets
  - `owner-direction-designer-r11.md` (`1b9fdf30`): Carlingford Lough and the Kenmare River opened as sea, Foyle mouth only
  - `owner-direction-designer-r12.md` (`fd75312f`): short Ulster rivers cut, rivers added
  - `owner-direction-designer-r13.md` (`9fb79b90`): the Cong
  - `owner-direction-designer-r14.md` (`808412e0`): the rivers, their names and tiers
  - `owner-direction-designer-r15.md` (`2ef1c1b1`): the sea cells and the hand-drawn last reaches
  - `owner-direction-designer-r16.md` (`941f67d5`): the loughs, with Lough Allua
  - `owner-direction-designer-r17.md` (`f16cc2d1`): the stranded pools: join the near ones, the rest to land
  - `owner-direction-designer-r18.md` (`4f03694b`): the stranded pools on the worker's build (the hand entries)
  - `owner-direction-designer-r19.md` (`b79dd767`): the island sites
  - `owner-direction-designer-r20.md` (`5217dc7a`): the reservoirs left out and the corrections (D13)
  - `owner-direction-designer-r21.md` (`d8060641`): the widths: a = 2.3 and b = 0.4 stand, the Shannon 1.4 times the rule above the tide
  - `owner-direction-designer-r22.md` (`b94789e0`): the Woodford's last reach: its last 1.5 km by three DAERA lines, running into the Erne through one lough line, so no reach is drawn twice
  - `owner-direction-designer-r23.md` (`41cfb630`): the before-and-after close-ups of the 27 added loughs: they stand

**Authored inputs** (kept and named; the scripts as accepted are the rest):
  - `edits.toml`: `567bb70a60b95086ead59c89bfebb4e832dd2ff937746d0d0ebfc92053164ccb` (the accepted entries, then the mask's 6, then the owner's 156 hand entries)
  - `waters.toml`: `18f2c99f24579ca50a69404f612436967f81f5f86b6b6f6af057bff6ed7142af`
  - `terrain.toml`: `cf4f8fca71ea6bbbde359bc117a10465ed9ff8da21abafada8a3d8a92abacd8d`, `map.toml`: `f8f4badf2adbf3533994e4026ad45736aa104b3c9f2ddd178196bb1a47018d26`
  - the accepted scenario's `assets/scenarios/ireland-768/locations.toml`, given to `--locations`: `3533c71164378dc9f2fbed33d6af57a807107a1de05eea046a2a1d2c278f59e2`

**Fetched inputs** (download caches, not committed; the manifest each script writes): `waters-manifest.json` of `import_waters.py fetch`:
  - `epa-rivers`: 25112 features, sha256 `4aa837df4032e1c194f6436bd09e0803302cc6e2ecb0222289c09d2d5c749940` (read 2026-11-19)
  - `epa-lakes`: 812 features, sha256 `5bb7066b50c044b07e9a84daa91440cf5dfa0425b66cfdf4cfcbe0804594fb53` (read 2026-11-19)
  - `epa-tidal`: 196 features, sha256 `c6b7e7eeeaa22eaf56b24cee7435aacdfb90ced7d0329dc65521d9360e05d6c9` (read 2026-11-19)
  - `epa-coastal`: 112 features, sha256 `79cfd6bb3e011b4622394040b86d5ca9b07496addc60fc8f96995223bf2ce06e` (read 2026-11-19)
  - `ni-rivers`: 2184 features, sha256 `fe788363279bd13b0bed77a7abe45b9d78caddc4595f65e15e357aaa5926043f` (read 2026-11-19)
  - `ni-lakes`: 345 features, sha256 `60005a72039fe8293374456e7b245ae6a9aab38018f04e9216cbe3e633dcaa60` (read 2026-11-19)
  - `ni-tidal`: 7 features, sha256 `afdfe98c63f8f7d177ad7c05750346ce749f9712cc83e9ec9f49b13359ba8067` (read 2026-11-19)
  - the elevation and Natural Earth sources of `derive_elevation.py` and `draft_terrain.py` as in "Sources" above.

**The frozen baseline** is `terrain.png` `cd471c15440715aa4d296e65a60164fd361cc5eaeeceb9565c4607602f052cc2` and `elevation.png` `6009b9991092749d59b82b887921cf3e421d8547063c46466308406b2d0e097f` as accepted when this slice started (after CK-351): the terrain with
every present edit applied and none of CK-350's. `build` reads it, never a terrain that already holds this slice's water, so the mask is not made from its own result.
The check before anything else: the same stage 2 with the accepted `edits.toml` alone reproduces the frozen baseline (`terrain.png`, `elevation.png`, the five computed layers
and `edits.applied`) byte for byte, so the chain is whole.

**Making it again**, from a checkout with Python 3.14.8, NumPy and Pillow as above, `S` the download cache and `BASE`, `STAGE1` and `OUT` empty scratch directories, in three steps. The frozen
baseline is recovered first (it is the map before this slice, which the committed files no longer are), then stage 1 reads it, then stage 2 makes the terrain and layers with stage 1's `water.png` beside the complete `edits.toml`.

Stage 1 also needs the **CK-350 locations file**, SHA-256 `3533c71164378dc9f2fbed33d6af57a807107a1de05eea046a2a1d2c278f59e2`, not the current r21 file. Recover it from the accepted CK-350 Pijul state as shown below and verify the hash before building (the retained checkpoint `1795124969055934000-combined` also contains that file). Inis Mac Saint (418, 258) and Loch Léin (182, 789) are r21 anchors on cells the CK-350 mask changes; using r21 would keep those cells and change the mask. Whether a future water rebuild should protect those anchors is left open.

```
# the frozen baseline: the chain with the accepted entries of edits.toml only (everything before the CK-350 section)
cp assets/map/ireland/terrain.toml BASE/
sed '/^# ---- Ireland.s water (CK-350/,$d' assets/map/ireland/edits.toml > BASE/edits.toml
python3 assets/map/ireland/derive_elevation.py --source S --out BASE --force
python3 assets/map/ireland/draft_terrain.py    --source S --out BASE --force
python3 assets/map/ireland/apply_edits.py      --out BASE        # BASE/terrain.png and BASE/elevation.png are the baseline named above
# recover CK-350's 94-location input and check it before building the mask
tools/ck pijul archive --state 35GNCYTJNTYYEGFMTVZHTLDBHHHKK3BTI52LKBE7YWNK3STBGNRQC -o BASE/ck350
tar -xOzf BASE/ck350.tar.gz assets/scenarios/ireland-768/locations.toml > BASE/locations-ck350.toml
printf '%s  %s\n' 3533c71164378dc9f2fbed33d6af57a807107a1de05eea046a2a1d2c278f59e2 BASE/locations-ck350.toml | shasum -a 256 -c
# stage 1: the built files (reads the frozen baseline, waters.toml and the survey cache)
python3 assets/map/ireland/import_waters.py build --source S --waters assets/map/ireland/waters.toml --map BASE \
        --locations BASE/locations-ck350.toml --out STAGE1   # STAGE1/water.png, STAGE1/rivers.toml, STAGE1/waters-report.md
# stage 2: the terrain and the layers, from the drafts, with stage 1's water.png beside the complete edits.toml
cp assets/map/ireland/terrain.toml assets/map/ireland/edits.toml STAGE1/water.png OUT/
python3 assets/map/ireland/derive_elevation.py --source S --out OUT --force
python3 assets/map/ireland/draft_terrain.py    --source S --out OUT --force
python3 assets/map/ireland/apply_edits.py      --out OUT         # OUT/terrain.png, elevation.png, the five layers, edits.applied
```

Each present edit and each new edit is applied exactly once, and the layers are computed once over the finished terrain. A second run of both stages from these inputs reproduced
`water.png`, `rivers.toml`, `terrain.png`, `elevation.png`, the five computed layers and `edits.applied` byte for byte (the hashes above). `SOURCES.md`'s own prose and the owner's records are written
by hand; no command makes them.

**Widths.** `rivers.toml`'s widths come from the drained areas (CK-355) by `a x area^b` metres with the values in `waters.toml` `[settings]`
(`a = 2.3`, `b = 0.4`), a tidal stretch at least as wide as its surveyed outline, and the `width_m` or `width_factor` corrections of `waters.toml`.

**Known limitations** (D13, deferred by the owner): the channels are today's. Drained and straightened channels (the Barrow and the Shannon below Limerick,
drained and canalised rivers elsewhere) stay as the surveys draw them and are logged here as a known limitation, with the confidence the terrain edits carry; modern reservoirs are left out
(below) and the Shannon is left as the survey draws it, in its old bed past Castleconnell. The drained areas behind the widths are a reading of a 500 m grid and
are wrong in limestone country where water runs underground (the Fergus, the Cong) and in the lough mazes (Lough Oughter, Upper Lough Erne).

## The derivation

Needs Python 3.11 or newer with `numpy` and `Pillow`. The CK-234 generation and reproduction check used Python 3.14.8, NumPy 2.3.4,
and Pillow 12.2.0. From a checkout:

```
python3 assets/map/ireland/derive_elevation.py --source ../ireland-source --force   # map.toml, elevation.png
python3 assets/map/ireland/draft_terrain.py    --source ../ireland-source --force   # terrain.png (the first draft only)
python3 assets/map/ireland/apply_edits.py                                           # edits.toml over the draft layers
```

The derivation order is elevation, terrain draft, then all terrain edits and the computed layers (the rivers are built before, from the frozen baseline: "Ireland's water"). The CK-234 correction was regenerated on
2026-11-09 from the accepted r8 `terrain.png` and `elevation.png`, applying only the two CK-234 owner-signed edits
(`glendalough-laragh-avonmore-pasture` and `rahan-clodiagh-dry-ground-pasture`) plus the climate and fertility settings. This is the
same final state as the full draft → r8 → CK-234 chain: r8 was already present in the accepted base and was not applied again. The
corrected files were reproduced by that procedure byte for byte. The terrain and computed-layer hashes below are current;
CK-351 replaces only the fertility hash (the CK-234 fertility hash was
`91c4c081ba0b581c854d03580c7145d0d0e48af57220e20fbff97e5d9c34e48c`):

**CK-350 replaces every hash below and adds the water files** (the accepted base, before CK-350, is the "frozen baseline" of "Ireland's water"):

| File | SHA-256 |
|---|---|
| `elevation.png` | `532fe15e0108cccc27ce17be34dfc0c7d9c91fc783ba659075313c8f48584cc8` |
| `terrain.png` | `e90fd4c6e3a87b74f73a85f99628c08dd4248a4729cfb691e539d972ef83d96e` |
| `fertility.png` | `deaf400a94af514e08ac60da1638b71285897a9ac23f0f22ac6b852a8a148adc` |
| `climate_temperature.png` | `368a68ea6b5429457e75165cf9335535ea9cae2eb171c25f1ffcf7330dd4275c` |
| `climate_gdd.png` | `2997b35ebef850eb96aed92830fb8763d1082c30ab8ba148d26172340d5e243f` |
| `climate_rainfall.png` | `af4f87ed4dca968a33719c58d292412d0763720f8059da944ad7d7f12c9ba3ac` |
| `climate_glacier.png` | `578726b5c37cdecbf847de48fb1d8d015ba631081ec9a8af9cf78a9d513ae512` |
| `edits.applied` | `fdee5d855f2a24c8bc93b51e885e30a4d73aa811b9a80919b4036ac8adb26211` |
| `edits.toml` | `567bb70a60b95086ead59c89bfebb4e832dd2ff937746d0d0ebfc92053164ccb` |
| `waters.toml` | `18f2c99f24579ca50a69404f612436967f81f5f86b6b6f6af057bff6ed7142af` |
| `water.png` | `80a0d67a93ef7a9fe4e6d1faf322eefd7fa8279ca380363cfc28f999f76ff64c` |
| `rivers.toml` | `e8959053e38c9a814e8a5fb2f1320352494f8f49900af250ea81dc67d1aa0495` |

(Before CK-350: `elevation.png` `6009b9991092749d59b82b887921cf3e421d8547063c46466308406b2d0e097f`, `terrain.png` `cd471c15440715aa4d296e65a60164fd361cc5eaeeceb9565c4607602f052cc2`, `fertility.png` `206a5a12747d4e95e9c35cb497423fb11fcbae16524460cd86fcba1a7c9529ea`.)

CK-351 changed the fertility valley drainage: elevation under 1 m ends the flow; all other cells, including loughs,
contribute their ground area and pass accumulated area to the steepest-descent neighbour after depressions are filled from
the sea and crop edge. The standard-library implementation is `drainage.py`, called by `apply_edits.py`. To regenerate
derived layers from the accepted terrain and elevation without applying `edits.toml` again, run
`python3 assets/map/ireland/apply_edits.py --layers-only`. The regenerated `fertility.png` SHA-256 is
`206a5a12747d4e95e9c35cb497423fb11fcbae16524460cd86fcba1a7c9529ea`; a second run in a scratch copy reproduced it
byte for byte. Terrain, elevation, climate, edit table and edit marker hashes above remain unchanged.

`edits.applied` retains the SHA-256 of the full `edits.toml`. The CK-234 reproduction check used the Python, NumPy and Pillow versions above.
`derive_elevation.py` retains its source-cache requirement and reproduces its own output from the cached source set.

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
- **Initial draft's former binary usable land (historical; removed by CK-234):** `0.62 x noise(10 cells) + 0.28 x noise(3.2 cells)` thresholded, for each terrain type separately, at the quantile that
  gives it the share of mock-up 2: pasture 32 %, forest 28 %, bog 20 %, marsh 15 %, mountain 8 %.
- **Lakes:** Natural Earth lake polygons drawn at 4 times the resolution; a cell is a lake where more than half of it is covered, on land
  only. Dropped as modern: Poulaphouca (Blessington) Reservoir, 1940 (and, by name, Inniscarra and Carrigadrohid, 1950s dams on the Lee,
  which Natural Earth does not have).
- **Rivers (the first draft; `derive_rivers.py`, removed by CK-350):** 15 rivers in this draft: Shannon, Erne, Upper Bann, Lower Bann, Foyle, Moy, Corrib,
  Boyne, Liffey, Barrow, Nore, Suir, Slaney, Munster Blackwater, Lee. Natural Earth gives the Shannon, the Lower Bann, the Barrow and the
  Suir, but its lines are coarse (straight stretches of 10 km or more), so their simplified vertices are used as anchors too; for the rest a few anchors per river (towns and bridges, approximate to a kilometre or two) are moved to the cell of the greatest
  drainage area within 4 cells, and the line between them follows the downhill path of the elevation grid (depressions filled), or, where
  that does not reach the next anchor, the cheapest path over the grid (a step costs less where the drainage area is greater). Lines are
  simplified with a tolerance of 0.6 cell.

### Computed climate, glaciers and fertility (`apply_edits.py`, CK-234)

After all entries in `edits.toml` have changed the terrain, `apply_edits.py` computes the map layers from the edited terrain and
`elevation.png`. The numeric tables and tunable values are in `[settings.climate]` and `[settings.fertility]` in `edits.toml`; the
algorithms are the Python implementations checked against the owner-reviewed prototypes in `m9-design/mockups/fertility-r3-flow.py`,
`m9-design/mockups/climate-r1/climate.py`, and `m9-design/mockups/fertility-with-climate.py`. The prototype copies are design references,
not runtime dependencies.

- **Climate:** monthly sea-level temperature and precipitation bands are interpolated by latitude; seasonal temperature uses the
  configured north/south phases, elevation lapse rate, and maritime influence. Westerly and easterly moisture walks use the configured
  wind direction and ocean-effect decay. Annual rainfall is accumulated from the configured monthly totals and smoothed with the
  configured box radius. Growing degree days sum monthly mean temperature above the base temperature. These are modeled fields, not
  station observations or a downscaled climate product.
- **Glaciers:** the equilibrium-line temperature solves the configured Ohmura quadratic from annual precipitation; elevation is compared
  to the configured snowline and uplift rules. The ELA relation is attributed to Ohmura, Kasser and Funk (1992), and Ohmura and
  Boettcher (2018); citations are listed in `docs/map-design.md` §12.7 and the owner-signed `m9-design/map-look-design-r4.md`. The
  `--self-test` mode runs three ELA vectors using only Python's standard library.
- **Fertility:** the configured climate, soil, slope, terrain, and distance-to-water factors combine into per-mille fertility. Surface
  land is usable for simulation when fertility is greater than zero. Water and glacier cells must have zero fertility. This continuous
  value is what the renderer draws: Grid A's green byte is fertility ÷ 4 (255 for a glacier), and the default look turns it into
  bright and dark pixels (milestone 9, slice 10a; `docs/map-design.md` §12).
- **Encoding and bounds:** fertility uses an 8-bit sample `min(250, floor(fertility_pm / 4))` and the loader decodes it as sample × 4;
  this deliberately distinguishes 0 from a positive value of 4. Temperature stores `round_half_away(celsius × 10) + 1000`, so the
  permitted -60..50 °C range is 400..1500. GDD is rounded to an integer in 0..12000, rainfall to an integer in 0..15000 mm, and glacier
  samples are exactly 0 or 255. Out-of-range values and inconsistent water/glacier fertility are rejected by `ck-map` with the layer and
  offending cell identified.
- **Shipped-map checks:** `crates/map/tests/ireland.rs` checks that at least 95% of land has nonzero fertility, 30–55% is at least half fertile, Emly and Tara reach 500 per mille, Valentia and Dublin temperatures stay within their named ranges, a Mayo/Galway
  upland cell exceeds 1,800 mm annual rainfall, Dublin stays below 900 mm, and Ireland contains no glacier cell. The two signed terrain
  corrections `glendalough-laragh-avonmore-pasture` and `rahan-clodiagh-dry-ground-pasture` are also pinned there.

For the checked CK-234 reproduction, see `.pipeline/coordination/runtime-m55/ck234-repro.log`; it records the SHA-256 of each generated
file. The cell-scale climate and fertility are estimates from the cited model and configured tables, not measured historical fields.

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

The original binary draft marked 26.7 % of land usable. CK-234 replaces that layer with modeled continuous fertility; the palette and
terrain rules remain in `terrain.toml` (mock-up 2's values).

## Guesswork log

**What is real:** the coast (at modern sea level, but for the reclamations below), the elevation and the sea depth (above) and the
larger loughs. **What is modeled or estimated:** terrain, climate, fertility, glacier extent, and the course of the rivers (Natural Earth's
vertices and hand-placed anchors, snapped to the valley floor of a 500 m grid). Each departure from a source is listed below, by area,
with its reason, so later research can correct it.

### The first draft (`derive_*.py`, `draft_terrain.py`, and `derive_rivers.py` until CK-350)

| Area | Departure from the sources | Reason |
|---|---|---|
| The whole map | Terrain, climate, fertility and glacier layers are modeled estimates; the initial binary usable layer was noise-based and has been removed | no medieval land-cover source or historical climate grid was available for this draft |
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
| pasture | 48.0 % | 41.2 % | about 40 % |
| forest | 21.3 % | 26.6 % | about 25 % |
| bog | 20.8 % | 22.1 % | about 23 % |
| marsh | 5.5 % | 5.7 % | about 7 % |
| mountain | 4.4 % | 4.4 % | unchanged |
| fertility > 0 (usable) | 26.7 % (legacy binary draft) | 98.3 % | at least 95 % (loader pin) |
| fertility ≥ 500‰ (display usable) | — | 40.2 % | 30–55 % (loader pin) |

Patch sizes after the edits (cells; draft 2 had 1310 bog patches with a median of 3): bog: 258 patches, smallest 12, median 46, largest 19063; forest: 253 patches, smallest 10, median 41, largest 25101.

Why so few cells change in some reclamations (the Wexford slobs, Lough Swilly, Strangford): at 500 m the elevation grid already holds much of
the slob land at 0 m (sea), and the cells are large; the edits change what the grid shows as land at the stated heights.

### Known limitation (deferred by the owner, 2026-11-05)

River courses are hand-anchored or Natural Earth lines snapped to a 500 m valley floor. Some run straight where the floor is flat, for
example the Shannon callows near Clonmacnoise; the owner, 2026-11-05: "the rivers look ok, some are too straight, but i think we can safely
defer that work". ck-manager tracks it.

Hand edits made after `edits.toml` go here, one row each (area, change, reason, who). None has been made.

### Edits r8 (CK-194), owner direction 2026-11-06

The owner directed, verbatim, "Make the islands land" (relayed by ck-manager). Edits r8 applies this to the one map cell containing
each monastery below. It changes only the terrain at those two cells; elevation and rivers are unchanged. At CK-194 the binary usable
ranking was global, so its output also changed at another cell; CK-234 replaced that layer with computed fertility. That historical
CK-194 binary output is no longer used by the map loader.

| Edit | Area | Change | Reason | Confidence | Cells |
|---|---|---|---|---|---|
| `island-iniscealtra` | Inis Cealtra (Holy Island), Lough Derg, crop cell (325, 599) | paint pasture | The monastery of 768 is on an island smaller than this map's cell; the derivation made that cell lake. Elevation remains 30 m. | high on site, low on extent | 1 |
| `island-inisfallen` | Inis Faithlinn (Inisfallen), Lough Leane, crop cell (181, 788) | paint pasture | The monastery of 768 is on an island smaller than this map's cell; the derivation made that cell lake. Elevation remains 22 m. | high on site, low on extent | 1 |

### Ireland's water (CK-350), owner direction 2026-11-19

The mask's entries and the owner's hand entries of `edits.toml` (cells are the cells each changed, from `apply_edits.py`); then each entry of `waters.toml` that corrects a source.

| Edit | Area | Change | Reason | Confidence | Cells |
|---|---|---|---|---|---|
| `ireland-water-sea` | water.png: cells that become sea (the estuaries and inlets the owner chose, and the tidal reaches joined to the open sea) | paint sea, depth 2 m; mask | Ireland's water of the owner-approved waters.toml, built by import_waters.py (CK-350; owner-direction-designer-r14 to r20) | medium | 2567 |
| `ireland-water-lake` | water.png: cells that become lake (the loughs added and redrawn from the surveys) | paint lake; mask | Ireland's water of the owner-approved waters.toml, built by import_waters.py (CK-350; owner-direction-designer-r14 to r20) | medium | 1144 |
| `ireland-water-pasture` | water.png: cells a redrawn lough gives back to land, as pasture | paint pasture; mask | Ireland's water of the owner-approved waters.toml, built by import_waters.py (CK-350; owner-direction-designer-r14 to r20) | medium | 660 |
| `ireland-water-forest` | water.png: cells a redrawn lough gives back to land, as forest | paint forest; mask | Ireland's water of the owner-approved waters.toml, built by import_waters.py (CK-350; owner-direction-designer-r14 to r20) | medium | 55 |
| `ireland-water-bog` | water.png: cells a redrawn lough gives back to land, as bog | paint bog; mask | Ireland's water of the owner-approved waters.toml, built by import_waters.py (CK-350; owner-direction-designer-r14 to r20) | medium | 49 |
| `ireland-water-marsh` | water.png: cells a redrawn lough gives back to land, as marsh | paint marsh; mask | Ireland's water of the owner-approved waters.toml, built by import_waters.py (CK-350; owner-direction-designer-r14 to r20) | medium | 220 |
| `stranded-pool-001-01` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 141, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-001-02` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 142, 6 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 6 |
| `stranded-pool-001-03` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 143, 2 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-001-04` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 143, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-001-05` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 144, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-001-06` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 145, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-001-07` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 146, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-001-08` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 147, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-001-09` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 148, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-001-10` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 149, 3 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 3 |
| `stranded-pool-001-11` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 150, 3 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 3 |
| `stranded-pool-001-12` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 151, 3 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 3 |
| `stranded-pool-001-13` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 152, 3 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 3 |
| `stranded-pool-001-14` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 153, 3 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 3 |
| `stranded-pool-001-15` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 154, 2 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-001-16` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 155, 2 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-001-17` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 156, 3 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 3 |
| `stranded-pool-001-18` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 157, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-001-19` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 158, 2 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-001-20` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 159, 2 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-001-21` | stranded sea near 54.906, -7.440: patch 1 of 85 (45 cells), row 160, 2 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-002-01` | stranded sea near 53.337, -9.682: patch 2 of 85 (17 cells), row 503, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-002-02` | stranded sea near 53.337, -9.682: patch 2 of 85 (17 cells), row 504, 4 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 4 |
| `stranded-pool-002-03` | stranded sea near 53.337, -9.682: patch 2 of 85 (17 cells), row 505, 4 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 4 |
| `stranded-pool-002-04` | stranded sea near 53.337, -9.682: patch 2 of 85 (17 cells), row 506, 4 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 4 |
| `stranded-pool-002-05` | stranded sea near 53.337, -9.682: patch 2 of 85 (17 cells), row 507, 3 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 3 |
| `stranded-pool-002-06` | stranded sea near 53.337, -9.682: patch 2 of 85 (17 cells), row 508, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-003-01` | stranded sea near 53.289, -9.624: patch 3 of 85 (8 cells), row 514, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-003-02` | stranded sea near 53.289, -9.624: patch 3 of 85 (8 cells), row 515, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-003-03` | stranded sea near 53.289, -9.624: patch 3 of 85 (8 cells), row 516, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-003-04` | stranded sea near 53.289, -9.624: patch 3 of 85 (8 cells), row 517, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-003-05` | stranded sea near 53.289, -9.624: patch 3 of 85 (8 cells), row 518, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-003-06` | stranded sea near 53.289, -9.624: patch 3 of 85 (8 cells), row 519, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-004-01` | stranded sea near 52.265, -9.804: patch 4 of 85 (7 cells), row 740, 3 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 3 |
| `stranded-pool-004-02` | stranded sea near 52.265, -9.804: patch 4 of 85 (7 cells), row 741, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-004-03` | stranded sea near 52.265, -9.804: patch 4 of 85 (7 cells), row 742, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-005-01` | stranded sea near 52.079, -7.593: patch 5 of 85 (7 cells), row 780, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-005-02` | stranded sea near 52.079, -7.593: patch 5 of 85 (7 cells), row 781, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-005-03` | stranded sea near 52.079, -7.593: patch 5 of 85 (7 cells), row 781, 2 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-005-04` | stranded sea near 52.079, -7.593: patch 5 of 85 (7 cells), row 782, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-005-05` | stranded sea near 52.079, -7.593: patch 5 of 85 (7 cells), row 782, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-006-01` | stranded sea near 55.171, -7.833: patch 6 of 85 (5 cells), row 87, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-006-02` | stranded sea near 55.171, -7.833: patch 6 of 85 (5 cells), row 88, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-006-03` | stranded sea near 55.171, -7.833: patch 6 of 85 (5 cells), row 89, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-006-04` | stranded sea near 55.171, -7.833: patch 6 of 85 (5 cells), row 90, 2 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-007-01` | stranded sea near 55.133, -8.153: patch 7 of 85 (5 cells), row 97, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-007-02` | stranded sea near 55.133, -8.153: patch 7 of 85 (5 cells), row 98, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-007-03` | stranded sea near 55.133, -8.153: patch 7 of 85 (5 cells), row 99, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-008-01` | stranded sea near 54.898, -8.347: patch 8 of 85 (5 cells), row 151, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-008-02` | stranded sea near 54.898, -8.347: patch 8 of 85 (5 cells), row 152, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-008-03` | stranded sea near 54.898, -8.347: patch 8 of 85 (5 cells), row 153, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-009-01` | stranded sea near 53.383, -9.827: patch 9 of 85 (4 cells), row 495, 3 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 3 |
| `stranded-pool-009-02` | stranded sea near 53.383, -9.827: patch 9 of 85 (4 cells), row 496, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-010-01` | stranded sea near 51.918, -10.275: patch 10 of 85 (4 cells), row 815, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-010-02` | stranded sea near 51.918, -10.275: patch 10 of 85 (4 cells), row 816, 3 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 3 |
| `stranded-pool-011-01` | stranded sea near 51.555, -9.705: patch 11 of 85 (4 cells), row 893, 2 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-011-02` | stranded sea near 51.555, -9.705: patch 11 of 85 (4 cells), row 894, 2 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-012-01` | stranded sea near 51.550, -9.667: patch 12 of 85 (4 cells), row 894, 2 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-012-02` | stranded sea near 51.550, -9.667: patch 12 of 85 (4 cells), row 895, 2 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-013-01` | stranded sea near 51.546, -9.720: patch 13 of 85 (4 cells), row 895, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-013-02` | stranded sea near 51.546, -9.720: patch 13 of 85 (4 cells), row 895, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-013-03` | stranded sea near 51.546, -9.720: patch 13 of 85 (4 cells), row 896, 2 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-014-01` | stranded sea near 54.574, -8.188: patch 14 of 85 (3 cells), row 226, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-014-02` | stranded sea near 54.574, -8.188: patch 14 of 85 (3 cells), row 227, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-015-01` | stranded sea near 53.963, -9.864: patch 15 of 85 (3 cells), row 365, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-015-02` | stranded sea near 53.963, -9.864: patch 15 of 85 (3 cells), row 366, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-016-01` | stranded sea near 53.567, -10.000: patch 16 of 85 (3 cells), row 454, 1 cell | paint forest; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-016-02` | stranded sea near 53.567, -10.000: patch 16 of 85 (3 cells), row 454, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-016-03` | stranded sea near 53.567, -10.000: patch 16 of 85 (3 cells), row 455, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-017-01` | stranded sea near 53.359, -9.668: patch 17 of 85 (3 cells), row 500, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-017-02` | stranded sea near 53.359, -9.668: patch 17 of 85 (3 cells), row 501, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-018-01` | stranded sea near 53.357, -9.648: patch 18 of 85 (3 cells), row 500, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-018-02` | stranded sea near 53.357, -9.648: patch 18 of 85 (3 cells), row 501, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-018-03` | stranded sea near 53.357, -9.648: patch 18 of 85 (3 cells), row 502, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-019-01` | stranded sea near 52.316, -9.834: patch 19 of 85 (3 cells), row 729, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-019-02` | stranded sea near 52.316, -9.834: patch 19 of 85 (3 cells), row 730, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-020-01` | stranded sea near 51.534, -9.429: patch 20 of 85 (3 cells), row 897, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-020-02` | stranded sea near 51.534, -9.429: patch 20 of 85 (3 cells), row 898, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-020-03` | stranded sea near 51.534, -9.429: patch 20 of 85 (3 cells), row 899, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-021-01` | stranded sea near 55.181, -7.801: patch 21 of 85 (2 cells), row 86, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-021-02` | stranded sea near 55.181, -7.801: patch 21 of 85 (2 cells), row 87, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-022-01` | stranded sea near 55.168, -7.688: patch 22 of 85 (2 cells), row 89, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-022-02` | stranded sea near 55.168, -7.688: patch 22 of 85 (2 cells), row 90, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-023-01` | stranded sea near 55.041, -7.134: patch 23 of 85 (2 cells), row 119, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-024-01` | stranded sea near 54.970, -8.404: patch 24 of 85 (2 cells), row 135, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-024-02` | stranded sea near 54.970, -8.404: patch 24 of 85 (2 cells), row 136, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-025-01` | stranded sea near 54.946, -7.391: patch 25 of 85 (2 cells), row 141, 2 cells | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-026-01` | stranded sea near 54.883, -8.367: patch 26 of 85 (2 cells), row 155, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-026-02` | stranded sea near 54.883, -8.367: patch 26 of 85 (2 cells), row 156, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-027-01` | stranded sea near 54.465, -5.638: patch 27 of 85 (2 cells), row 251, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-027-02` | stranded sea near 54.465, -5.638: patch 27 of 85 (2 cells), row 252, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-028-01` | stranded sea near 53.918, -9.927: patch 28 of 85 (2 cells), row 375, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-028-02` | stranded sea near 53.918, -9.927: patch 28 of 85 (2 cells), row 376, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-029-01` | stranded sea near 52.702, -8.909: patch 29 of 85 (2 cells), row 645, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-029-02` | stranded sea near 52.702, -8.909: patch 29 of 85 (2 cells), row 646, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-030-01` | stranded sea near 52.645, -8.695: patch 30 of 85 (2 cells), row 658, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-031-01` | stranded sea near 52.370, -6.976: patch 31 of 85 (2 cells), row 718, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-032-01` | stranded sea near 52.250, -9.803: patch 32 of 85 (2 cells), row 744, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-033-01` | stranded sea near 52.234, -6.799: patch 33 of 85 (2 cells), row 747, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-033-02` | stranded sea near 52.234, -6.799: patch 33 of 85 (2 cells), row 748, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-034-01` | stranded sea near 52.216, -6.588: patch 34 of 85 (2 cells), row 751, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-034-02` | stranded sea near 52.216, -6.588: patch 34 of 85 (2 cells), row 752, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-035-01` | stranded sea near 52.132, -10.281: patch 35 of 85 (2 cells), row 769, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-035-02` | stranded sea near 52.132, -10.281: patch 35 of 85 (2 cells), row 770, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-036-01` | stranded sea near 52.010, -7.850: patch 36 of 85 (2 cells), row 796, 2 cells | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 2 |
| `stranded-pool-037-01` | stranded sea near 51.649, -9.867: patch 37 of 85 (2 cells), row 873, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-037-02` | stranded sea near 51.649, -9.867: patch 37 of 85 (2 cells), row 874, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-038-01` | stranded sea near 51.597, -9.603: patch 38 of 85 (2 cells), row 884, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-038-02` | stranded sea near 51.597, -9.603: patch 38 of 85 (2 cells), row 885, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-039-01` | stranded sea near 55.287, -7.266: patch 39 of 85 (1 cells), row 62, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-040-01` | stranded sea near 55.183, -7.839: patch 40 of 85 (1 cells), row 86, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-041-01` | stranded sea near 55.115, -7.696: patch 41 of 85 (1 cells), row 102, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-042-01` | stranded sea near 55.089, -8.306: patch 42 of 85 (1 cells), row 108, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-043-01` | stranded sea near 55.067, -6.045: patch 43 of 85 (1 cells), row 113, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-044-01` | stranded sea near 54.968, -7.357: patch 44 of 85 (1 cells), row 136, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-045-01` | stranded sea near 54.963, -7.364: patch 45 of 85 (1 cells), row 137, 1 cell | paint forest; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-046-01` | stranded sea near 54.959, -7.643: patch 46 of 85 (1 cells), row 138, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-047-01` | stranded sea near 54.959, -7.372: patch 47 of 85 (1 cells), row 138, 1 cell | paint forest; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-048-01` | stranded sea near 54.955, -7.379: patch 48 of 85 (1 cells), row 139, 1 cell | paint forest; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-049-01` | stranded sea near 54.851, -7.462: patch 49 of 85 (1 cells), row 163, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-050-01` | stranded sea near 54.842, -7.470: patch 50 of 85 (1 cells), row 165, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-051-01` | stranded sea near 54.833, -5.782: patch 51 of 85 (1 cells), row 167, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-052-01` | stranded sea near 54.607, -5.917: patch 52 of 85 (1 cells), row 219, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-053-01` | stranded sea near 54.498, -5.669: patch 53 of 85 (1 cells), row 244, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-054-01` | stranded sea near 54.410, -5.638: patch 54 of 85 (1 cells), row 264, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-055-01` | stranded sea near 54.384, -5.616: patch 55 of 85 (1 cells), row 270, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-056-01` | stranded sea near 54.327, -5.736: patch 56 of 85 (1 cells), row 283, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-057-01` | stranded sea near 54.173, -10.070: patch 57 of 85 (1 cells), row 318, 1 cell | paint bog; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-058-01` | stranded sea near 54.018, -9.882: patch 58 of 85 (1 cells), row 353, 1 cell | paint bog; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-059-01` | stranded sea near 54.009, -9.882: patch 59 of 85 (1 cells), row 355, 1 cell | paint bog; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-060-01` | stranded sea near 53.501, -6.136: patch 60 of 85 (1 cells), row 469, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-061-01` | stranded sea near 53.384, -9.648: patch 61 of 85 (1 cells), row 495, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-062-01` | stranded sea near 53.353, -6.234: patch 62 of 85 (1 cells), row 502, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-063-01` | stranded sea near 53.330, -9.588: patch 63 of 85 (1 cells), row 507, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-064-01` | stranded sea near 53.317, -9.844: patch 64 of 85 (1 cells), row 510, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-065-01` | stranded sea near 53.312, -9.836: patch 65 of 85 (1 cells), row 511, 1 cell | paint bog; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-066-01` | stranded sea near 53.303, -9.633: patch 66 of 85 (1 cells), row 513, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-067-01` | stranded sea near 53.285, -9.648: patch 67 of 85 (1 cells), row 517, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-068-01` | stranded sea near 53.272, -9.693: patch 68 of 85 (1 cells), row 520, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-069-01` | stranded sea near 53.267, -8.947: patch 69 of 85 (1 cells), row 521, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-070-01` | stranded sea near 52.828, -8.955: patch 70 of 85 (1 cells), row 618, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-071-01` | stranded sea near 52.650, -9.550: patch 71 of 85 (1 cells), row 657, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-072-01` | stranded sea near 52.618, -8.789: patch 72 of 85 (1 cells), row 664, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-073-01` | stranded sea near 52.540, -6.219: patch 73 of 85 (1 cells), row 681, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-074-01` | stranded sea near 52.430, -6.565: patch 74 of 85 (1 cells), row 705, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-075-01` | stranded sea near 52.310, -7.003: patch 75 of 85 (1 cells), row 731, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-076-01` | stranded sea near 52.301, -7.010: patch 76 of 85 (1 cells), row 733, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-077-01` | stranded sea near 52.250, -9.829: patch 77 of 85 (1 cells), row 744, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-078-01` | stranded sea near 52.167, -7.116: patch 78 of 85 (1 cells), row 762, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-079-01` | stranded sea near 52.079, -7.613: patch 79 of 85 (1 cells), row 781, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-080-01` | stranded sea near 52.065, -7.621: patch 80 of 85 (1 cells), row 784, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-081-01` | stranded sea near 51.963, -7.854: patch 81 of 85 (1 cells), row 806, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-082-01` | stranded sea near 51.935, -7.877: patch 82 of 85 (1 cells), row 812, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-083-01` | stranded sea near 51.651, -9.814: patch 83 of 85 (1 cells), row 873, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-084-01` | stranded sea near 51.585, -9.618: patch 84 of 85 (1 cells), row 887, 1 cell | paint marsh; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |
| `stranded-pool-085-01` | stranded sea near 51.567, -9.701: patch 85 of 85 (1 cells), row 891, 1 cell | paint pasture; box; terrain ['sea'] | today's map draws this sea cut off from the open sea; the owner made each such patch land of the nearest terrain (owner-direction-designer-r18.md) | medium | 1 |

**The sea the owner added** (`[[sea]]`):

| Entry | Given by | Reason |
|---|---|---|
| Cork Harbour | survey outline IE_SW_060_0000 | Owner, 2026-11-19 (owner-direction-designer-r10.md): add Cork Harbour from the survey's coastal outline. Today's map draws it as land, so the Lee has no sea to end in. |
| Outer Cork Harbour | survey outline IE_SW_050_0000 | Part of the owner's Cork Harbour (r10): the survey draws the harbour as two outlines, and the inner one reaches the open sea only through this one. |
| Lough Swilly | survey outline IE_NW_220_0000 | Owner (r10): add Lough Swilly from the survey's coastal outline. Today's map has only its outer part. |
| Mouth of Lough Foyle | 21 points | Owner (r10): open Lough Foyle at its mouth. These 21 cells are land in today's map between the lough and today's sea, and are sea in the Irish survey's coastal outlines (20 in Portstewart Bay, GBNIIE6NB010; the one at the mouth itself in Lough Foyle, GBNIIE6NW250). Each point is the centre of one cell. |
| Strangford Narrows | 21 points | Owner (r10): open Strangford Lough at its mouth. A chain of 21 cells down the middle of the Narrows, from the lough past Portaferry to the notch of sea at its mouth; each point is the centre of one cell. No surveyed outline with a confirmed licence covers Strangford Lough, so the line is placed by hand from where Ballyhenry, the Portaferry ferry, Kilclief and Killard Point lie. |
| Passage West | 2 points | Part of the owner's Cork Harbour (r10), for the owner to confirm: the two cells of the channel at Passage West, narrower than a cell, which join Lough Mahon and the Lee's estuary to Cork Harbour; each point is the centre of one cell. |
| Carlingford Lough | survey outline GBNIIE6NB030 | Owner, 2026-11-19 (owner-direction-designer-r11.md): open Carlingford Lough from the survey's coastal outline. Today's map draws it as land. |
| Outer Kenmare River | survey outline IE_SW_190_0000 | Owner (r11): open the Kenmare River from the survey's coastal outline. Today's map draws its inner 22 km as land; the outline is the whole inlet, and 425 of its cells are land today. |
| Stranded sea joined through surveyed water | 63 points | Owner, 2026-11-19 (owner-direction-designer-r17.md): stranded sea that the survey draws as sea is joined to the open sea. These 63 cells are land in today's map, lie more than half in the Irish survey's tidal or coastal outlines, and are the one or two cells that join each of 46 patches of today's sea (inner Bantry Bay, Achill Sound, Westport Bay, Inver Bay and smaller ones). Each point is the centre of one cell (slice-4-checks/r2/poollinks.py). |
| Belfast Lough head | 4 points | Owner (r17): the sea cells of the owner's edit belfast-harbour-sea at the head of Belfast Lough are joined to the lough through these four cells, placed by hand; no surveyed outline covers them. |
| Ballyteige lagoon | 1 points | Owner (r17): the 21-cell lagoon behind Ballyteige Burrow near Kilmore Quay is joined to the sea through this one cell, placed by hand. That it was tidal before 19th-century reclamation is the designer's understanding, still to be cited. |

**Corrections** (`[[correction]]`): hand-drawn last reaches, the survey copies left out where both surveys draw a river, and the Shannon's widths (owner-direction-designer-r21.md):

| Correction | River | What | Reason | Confidence |
|---|---|---|---|---|
| `bann-tidal` | bann | hand-drawn course of 30 points (tidal) for UKGBNI8NB014.9835 | The Bann's survey line stops above Coleraine, 12 km of estuary short of the sea, and the estuary is narrower than a cell, so the tool cannot make it sea. Its last reach is drawn down the middle of DAERA's Bann Estuary outline (UKGBNI5NB010010) to the Barmouth, then in the Irish survey's Portstewart Bay outline (GBNIIE6NB010) to the first sea cell. The first 180 m are the survey segment it stands in for. | high |
| `lagan-tidal` | lagan | hand-drawn course of 24 points (tidal) for UKGBNI8NE023.17884 | The Lagan's survey line stops at Stranmillis. Its last reach is drawn down the middle of DAERA's Lagan Estuary outline (UKGBNI5NE100010) to the Lagan Weir, then by three hand-placed points along the harbour channel to today's sea in Belfast Lough, whose head today's map draws as land for 8 cells. No surveyed outline with a confirmed licence covers Belfast Lough. The first 188 m are the survey segment it stands in for. | low |
| `newry-tidal` | newry | hand-drawn course of 42 points (tidal) for UKGBNI1NB060604046.13608 | The Newry River's survey line stops in Newry, and its estuary down to Narrow Water is narrower than a cell, so the tool cannot make it sea. Its last reach is drawn down the middle of DAERA's Newry Estuary outline (UKGBNI5NB030010) to the first sea cell of Carlingford Lough, which the owner opened (r11). The first 1027 m are the survey segment it stands in for. | high |
| `quoile-tidal` | quoile | hand-drawn course of 10 points (tidal) for .12463 | The Quoile's survey lines stop at the barrage (1957). Before the barrages the river was tidal up to Downpatrick. Its last reach is a hand-placed line of two legs down the old estuary to Strangford Lough; no surveyed outline covers it. The first 252 m are the survey segment it stands in for. | low |
| `owenea-tidal` | owenea | hand-drawn course of 4 points (tidal) for 38_5368 | The Owenea's survey line stops at the head of its estuary, which today's map draws as land. Its last reach is drawn down the middle of the Irish survey's Owenea Estuary (IE_NW_110_0100) and Loughros Bay (IE_NW_110_0000) outlines to the first sea cell. It stands in for a 5 m connecting segment. | high |
| `inagh-tidal` | inagh | hand-drawn course of 7 points (tidal) for 28_1183 | The Inagh's survey line stops at the head of its estuary, narrower than a cell. Its last reach is drawn down the middle of the Irish survey's outlines of its estuary and of Liscannor Bay (IE_SH_100_0000) to the first sea cell. It stands in for a 5 m connecting segment. | high |
| `inny-kerry-tidal` | inny-kerry | hand-drawn course of 12 points (tidal) for 21_9166 | The Inny's survey line stops at the head of its estuary, narrower than a cell. Its last reach is drawn down the middle of the Irish survey's outlines of its estuary and of Ballinskelligs Bay (IE_SW_200_0000) to the first sea cell. It stands in for a 5 m connecting segment. | high |
| `blackwater-munster-tidal` | blackwater-munster | hand-drawn course of 4 points (tidal) for 18_3272 | The Blackwater's survey line ends at Youghal, three cells from today's sea, and the nearest sea cell is a single stranded one, so the tool finds no way out. Its last reach is drawn in the Irish survey's Youghal Harbour (IE_SW_020_0100) and Youghal Bay (IE_SW_020_0000) outlines to the first sea cell. It stands in for a 6 m connecting segment. | high |
| `mourne-daera-last-reach` | mourne | leaves out 2 survey segments | DAERA's last 2.5 km of the Mourne run on into a line of the Finn's water body, which is not in this list and ends 30 m from where the Irish survey's Foyle begins. The Irish survey's copy of the reach (in the river's segments) meets the Foyle, so DAERA's two lines are left out. | high |
| `erne-daera-below-belleek` | erne | leaves out 2 survey segments | DAERA's copy of the Erne from Belleek to Ballyshannon (10 km, in the Republic) does not meet the Irish survey's tidal lines at its lower end, so the tool wrote it as a branch flowing backwards. The Irish survey's lines carry this reach. | high |
| `arney-irish-copy` | arney | leaves out 6 survey segments | Both surveys draw this reach and the tool cannot pair these lines as one (their ends or lengths differ). DAERA's line is kept and the Irish survey's copy is left out, so the river is not built twice. Each line left out lies for half its length or more within 150 m of a DAERA line of this list (slice-4-checks/r2/dup.py). | high |
| `ballinamallard-irish-copy` | ballinamallard | leaves out 7 survey segments | Both surveys draw this reach and the tool cannot pair these lines as one (their ends or lengths differ). DAERA's line is kept and the Irish survey's copy is left out, so the river is not built twice. Each line left out lies for half its length or more within 150 m of a DAERA line of this list (slice-4-checks/r2/dup.py). | high |
| `colebrooke-irish-copy` | colebrooke | leaves out 3 survey segments | Both surveys draw this reach and the tool cannot pair these lines as one (their ends or lengths differ). DAERA's line is kept and the Irish survey's copy is left out, so the river is not built twice. Each line left out lies for half its length or more within 150 m of a DAERA line of this list (slice-4-checks/r2/dup.py). | high |
| `derg-irish-copy` | derg | leaves out 8 survey segments | Both surveys draw this reach and the tool cannot pair these lines as one (their ends or lengths differ). DAERA's line is kept and the Irish survey's copy is left out, so the river is not built twice. Each line left out lies for half its length or more within 150 m of a DAERA line of this list (slice-4-checks/r2/dup.py). | high |
| `erne-irish-copy` | erne | leaves out 21 survey segments | Both surveys draw this reach and the tool cannot pair these lines as one (their ends or lengths differ). DAERA's line is kept and the Irish survey's copy is left out, so the river is not built twice. Each line left out lies for half its length or more within 150 m of a DAERA line of this list (slice-4-checks/r2/dup.py). | high |
| `faughan-irish-copy` | faughan | leaves out 2 survey segments | Both surveys draw this reach and the tool cannot pair these lines as one (their ends or lengths differ). DAERA's line is kept and the Irish survey's copy is left out, so the river is not built twice. Each line left out lies for half its length or more within 150 m of a DAERA line of this list (slice-4-checks/r2/dup.py). | high |
| `finn-monaghan-irish-copy` | finn-monaghan | leaves out 38 survey segments | Both surveys draw this reach and the tool cannot pair these lines as one (their ends or lengths differ). DAERA's line is kept and the Irish survey's copy is left out, so the river is not built twice. Each line left out lies for half its length or more within 150 m of a DAERA line of this list (slice-4-checks/r2/dup.py). | high |
| `mourne-irish-copy` | mourne | leaves out 2 survey segments | Both surveys draw this reach and the tool cannot pair these lines as one (their ends or lengths differ). DAERA's line is kept and the Irish survey's copy is left out, so the river is not built twice. Each line left out lies for half its length or more within 150 m of a DAERA line of this list (slice-4-checks/r2/dup.py). | high |
| `roe-irish-copy` | roe | leaves out 14 survey segments | Both surveys draw this reach and the tool cannot pair these lines as one (their ends or lengths differ). DAERA's line is kept and the Irish survey's copy is left out, so the river is not built twice. Each line left out lies for half its length or more within 150 m of a DAERA line of this list (slice-4-checks/r2/dup.py). | high |
| `strule-irish-copy` | strule | leaves out 6 survey segments | Both surveys draw this reach and the tool cannot pair these lines as one (their ends or lengths differ). DAERA's line is kept and the Irish survey's copy is left out, so the river is not built twice. Each line left out lies for half its length or more within 150 m of a DAERA line of this list (slice-4-checks/r2/dup.py). | high |
| `swanlinbar-irish-copy` | swanlinbar | leaves out 3 survey segments | Both surveys draw this reach and the tool cannot pair these lines as one (their ends or lengths differ). DAERA's line is kept and the Irish survey's copy is left out, so the river is not built twice. Each line left out lies for half its length or more within 150 m of a DAERA line of this list (slice-4-checks/r2/dup.py). | high |
| `drowes-irish-copy` | drowes | leaves out 8 survey segments | The Irish survey's lough lines through Lough Melvin, copies of DAERA's lines of the same lough, which the tool cannot pair. DAERA's are kept. | high |
| `burn-dennet-irish-copy` | burn-dennet | leaves out 3 survey segments | Both surveys draw this reach and the tool cannot pair these lines as one (their ends or lengths differ). DAERA's line is kept and the Irish survey's copy is left out, so the river is not built twice. Each line left out lies for half its length or more within 150 m of a DAERA line of this list (slice-4-checks/r2/dup.py). | high |
| `glenelly-irish-copy` | glenelly | leaves out 1 survey segments | Both surveys draw this reach and the tool cannot pair these lines as one (their ends or lengths differ). DAERA's line is kept and the Irish survey's copy is left out, so the river is not built twice. Each line left out lies for half its length or more within 150 m of a DAERA line of this list (slice-4-checks/r2/dup.py). | high |
| `sillees-irish-copy` | sillees | leaves out 10 survey segments | Both surveys draw this reach and the tool cannot pair these lines as one (their ends or lengths differ). DAERA's line is kept and the Irish survey's copy is left out, so the river is not built twice. Each line left out lies for half its length or more within 150 m of a DAERA line of this list (slice-4-checks/r2/dup.py). | high |
| `woodford-irish-copy` | woodford | leaves out 5 survey segments | Both surveys draw the Woodford's last reach into Upper Lough Erne, and the tool cannot pair these lines. DAERA's are kept (three are in the river's segments, and the Erne's lough line carries on from them); the Irish survey's five are left out, so the reach is built once (owner, r22). | high |
| `shannon-width-01` | shannon | width times 1.4 on shannon-01 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-02` | shannon | width times 1.4 on shannon-02 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-03` | shannon | width times 1.4 on shannon-03 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-04` | shannon | width times 1.4 on shannon-04 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-05` | shannon | width times 1.4 on shannon-05 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-06` | shannon | width times 1.4 on shannon-06 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-07` | shannon | width times 1.4 on shannon-07 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-08` | shannon | width times 1.4 on shannon-08 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-09` | shannon | width times 1.4 on shannon-09 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-10` | shannon | width times 1.4 on shannon-10 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-11` | shannon | width times 1.4 on shannon-11 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-12` | shannon | width times 1.4 on shannon-12 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-13` | shannon | width times 1.4 on shannon-13 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-14` | shannon | width times 1.4 on shannon-14 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-15` | shannon | width times 1.4 on shannon-15 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-16` | shannon | width times 1.4 on shannon-16 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-17` | shannon | width times 1.4 on shannon-17 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-18` | shannon | width times 1.4 on shannon-18 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |
| `shannon-width-19` | shannon | width times 1.4 on shannon-19 | The Shannon is nearly level and lake-like and wider than the rule gives: the survey's outline at its head of tide at Limerick is 130 m across, the rule gives 93 m (widths-fit-r5.md). The owner widened it 1.4 times (r21). One measured place. | medium |

**Reservoirs left out** (`[[reservoir]]`, D13: the modern reservoirs are not lakes):

| Reservoir | Survey code | Reason, source and confidence |
|---|---|---|
| Assaroe Lake | IE_NW_36_717 | Made by the Erne hydro-electric scheme: the dam at Cathaleen's Fall, built from 1946 to 1955, its turbines commissioned in 1951 and 1952 (Wikipedia, Cathaleen's Fall Hydroelectric Power Station; ESB, Cathleen's Falls). Confidence high. |
| Ballinrees Reservoir | Lake_0088 | An impounding reservoir of NI Water near Coleraine (NI Water's reservoir improvement programme; photographed in 1952, britainfromabove.org.uk XAW045258). The year it was built is not found. Confidence medium. |
| Ben Crom Reservoir | Lake_0254 | Built 1953 to 1957 above the Silent Valley for Belfast's water supply (Wikipedia, Silent Valley Reservoir). Confidence high. |
| Carrigadrohid Lake | IE_SW_19_139 | Made by the ESB's Lee scheme: the dam was built from 1952 to 1957 (Wikipedia, Carrigadrohid hydroelectric power station). Confidence high. |
| Parteen Basin | IE_SH_25_191b | The basin above Parteen Weir, flooded by the Shannon Scheme, built from 1925 and opened on 22 July 1929 (Wikipedia, Shannon hydroelectric scheme). The survey marks it a heavily modified water body, apart from Lough Derg proper. Confidence high that it is modern; how the river ran here before is not checked. |
| Inniscarra Lake | IE_SW_19_138 | Made by the ESB's Lee scheme: the dam was built from February 1953 and completed in 1957 (Wikipedia, Inniscarra Dam). Confidence high. |
| Killylane Reservoir | Lake_0106 | An impounding reservoir of NI Water (NI Water's reservoir improvement programme; DAERA, Killylane Reservoir). A search summary of Lisburn Today, 'Killylane dam opens at Shane's Hill', gives its opening as June 1964 for the supply of Larne, Ballyclare, Antrim and Ballymena; the page itself could not be opened. Confidence high that it is modern. |
| Lough Island Reavy | Lake_0031 | A man-made reservoir (DAERA, Lough Island Reavy), one of two impoundments of the Bann Reservoir Company, set up by an Act of 1836 (Hansard, Lords, 11 December 1975). Whether a smaller natural lough lay here before is not established. Confidence medium. |
| Poulaphouca Reservoir | IE_EA_09_71 | Made by the ESB's Liffey scheme: created between 1937 and 1947, the flooding beginning on 3 March 1940 (Wikipedia, Poulaphouca Reservoir). Confidence high. |
| Seagahan Reservoir | Lake_0304 | Built in the 1950s for Armagh's water supply (Farrans, 'On site in the 1950s at Seagahan Dam'). The year is not found. Confidence medium. |
| Silent Valley Reservoir | Lake_0038 | Built 1923 to 1933 for Belfast's water supply (Wikipedia, Silent Valley Reservoir). Confidence high. |
| Spelga Reservoir | Lake_0036 | Built 1953 to 1957 (Wikipedia, Spelga Reservoir). Confidence high. |
| Stoneyford Reservoir | Lake_0035 | Built in the late 19th century for the Belfast Water Commissioners (geograph.org.uk photo 3465496; NI Water's reservoir improvement programme). The year is not found. Confidence medium. |
| Middle South Woodburn Reservoir | Lake_0147 | One of the Woodburn reservoirs of Belfast's water supply near Carrickfergus (DAERA, Middle South Woodburn Reservoir; NI Water's reservoir improvement programme). The year is not found. Confidence medium. |

### Sources read for Ireland's water (CK-350)

Pages opened and read in full on 2026-11-19 by ck-designer-7, for the reasons and sources of `waters.toml` (a search summary is said so):

One line each: what it supports; the page; the words relied on. Every page below was fetched and read on 2026-11-19
(copies not kept). Wikipedia pages were read as their source text.

#### Reservoirs left out (`[[reservoir]]`)

| Reservoir | Page | Words relied on |
|---|---|---|
| Assaroe Lake | Wikipedia, "Cathaleen's Fall Hydroelectric Power Station"; ESB, esb.ie/what-we-do/generation-and-trading/cathleen's-falls | "Constructed between 1946 and 1955"; "The first turbine was commissioned in 1951 and the second in 1952"; the dam meant "the destruction of Assaroe Falls". ESB: "Starting work in 1946". |
| Carrigadrohid Lake | Wikipedia, "Carrigadrohid hydroelectric power station" | "Construction of the reinforced concrete gravity dam started in 1952 and was complete in 1957." |
| Inniscarra Lake | Wikipedia, "Inniscarra Dam" | "Construction of the dam started in February 1953 and resulted in the formation of the Inniscarra Reservoir"; category "Dams completed in 1957". |
| Poulaphouca Reservoir | Wikipedia, "Poulaphouca Reservoir" | "It was created between 1937 and 1947, with flooding beginning at 10:00 on 3 March 1940 by damming the River Liffey at Poulaphouca". |
| Parteen Basin | Wikipedia, "Shannon hydroelectric scheme" | "The Shannon Scheme was officially opened at Parteen Weir on 22 July 1929"; the Shannon Electricity Act, 1925. |
| Silent Valley Reservoir | Wikipedia, "Silent Valley Reservoir" | "The reservoir was built between 1923 and 1933". |
| Ben Crom Reservoir | the same page | "Between 1953 and 1957, a second dam was built further up the Kilkeel river valley to create Ben Crom Reservoir." |
| Spelga Reservoir | Wikipedia, "Spelga Reservoir" | "The Spelga Reservoir itself was developed between 1953 and 1957." |
| Lough Island Reavy | DAERA, daera-ni.gov.uk/articles/lough-island-reavy; Hansard, Lords, 11 December 1975, "Bann Reservoir Company (Northern Ireland)" | DAERA: "a man-made freshwater reservoir near Kilcoo ... Constructed for water supply purposes". Hansard: "The Company was established by Private Act in 1836 ... two impoundments were constructed at Lough Island Reavy and Corbet Lough". |
| Seagahan Reservoir | Farrans, farrans.com/on-site-in-the-1950s-at-seagahan-dam/ | "On site in the 1950s at Seagahan Dam"; "The dam was constructed between Armagh and Markethill to supply water to the city." |
| Stoneyford Reservoir | geograph.org.uk/photo/3465496 | "Stoneyford reservoir dates from the late 19th century when it was built for the Belfast Water Commissioners to supply water to Belfast." |
| Ballinrees Reservoir | Britain from Above, image XAW045258; NI Water, news item 12039 (reservoir improvement programme) | "Ballinrees Reservoir, Macosquin, Coleraine, Northern Ireland, 1952" (an air photograph); listed among NI Water's impounding reservoirs. No year of building. |
| Middle South Woodburn Reservoir | DAERA, daera-ni.gov.uk/articles/middle-south-woodburn-reservoir; NI Water, news item 12039 | DAERA: "the largest of the Woodburn Reservoirs"; Woodburn is in NI Water's list of impounding reservoirs. No year of building. |
| Killylane Reservoir | DAERA, daera-ni.gov.uk/articles/killylane-reservoir; NI Water, news item 12039 | A reservoir; listed among NI Water's impounding reservoirs. **Its opening in June 1964 is NOT from a page could be opened:** it is a search summary of Lisburn Today, "Killylane dam opens at Shane's Hill", and that page refused me (403). |

Named in the list's reasons but **not opened** (search summaries only): the NPWS site synopsis 004063 (Poulaphouca, 1944),
buildingsofireland.ie 40852084 and S.I. 86 of 1945 (Assaroe), ESB's Lee pages, Lisburn Today (Killylane). `SOURCES.md`
should cite the pages in the table, not these.

#### The Cong (`[[river]] cong`, a known liberty)

- Wikipedia, "River Cong (Ireland)": "The springs are fed by underground flow through caverns in the limestone bedrock
  of the district from Lough Mask, a little over 4 mi to the north."
- Wikipedia, "Cong Canal": "It was abandoned unfinished in 1854 and is popularly known as 'The Dry Canal'"; the Board
  "failed to take account of the cavernous nature of the limestone in the district".
- The same page on the dates: the works were "carried out by ... the Board of Works ... over the period 1844 to
  1859", survey from June 1844. "Begun in 1848", which earlier drafts of the list gave, is not on this page (it came from a search summary) and is not in the list.
- Not opened: irishwaterwayshistory.com (named in my notes from a search summary).

#### Ballyteige lagoon (`[[sea]]` "Ballyteige lagoon")

- irishlagoons.com (page of the tag "Lineus ruber", entry "Ballyteige channels, Wexford"): "The drainage channels are
  artificial and were excavated to drain a lagoon and saltmarsh which were isolated behind an extensive dune system
  to the south and a sea wall to the west, constructed across the Cull Inlet in the mid 19th Century."
- irishhistory.com, "Castle, Ballyteige, Co. Wexford": "before Ballyteige Lough was reclaimed in the late 19th
  century, it would have overlooked the water from its eastern shore."

#### Tiers and widths

- Basin areas of 37 rivers: Wikipedia, "List of rivers of Ireland", table "Longest Irish Rivers (with Basin areas)"
  (it names the Ordnance Survey of Ireland's "Rivers and their Catchment Basins 1958" and R. A. Williams, "Notes on
  River Basins"). Second-hand.
- The Moyola's tier 2: Wikipedia, "River Moyola": "The catchment area to the gauging station is 304 km2".
- Widths: no published figures. Twelve head-of-tide widths measured from the surveys' own tidal outlines
  (`widths-fit-r5.md`, `slice-4-checks/widths/headtide.py`).
- The Shannon past Castleconnell: the Irish survey's river network itself (its line passes O'Briensbridge and
  Castleconnell; the head-race canal is not among its lines).

### CK-358 location kinds and island cells

The physical kinds follow `.pipeline/coordination/history-scratch/location-kinds-r1.md` (SHA-256 `a8895045b7562cd1b519751bd482356afb01ca8436cd73972f7f1d54515faa34`). The four glyphs and colours were approved by the owner in r189 from `mockups/kind-glyphs-r1.png`, drawn from plain shapes by `tools/kind_glyphs.py` in the history-scratch authoring area. Sizes, catchments and house numbers are designer values in `scenario/p2-economy-places-r1.toml` (SHA-256 `7fa590c19aaadbc0a16720b918e2aaeb6271d653f902f48d1fc00709a00ccfa3`). Seven one-cell island edits follow `scenario/ck358-dispositions-r1.toml` (SHA-256 `a3228302612021a485f2f57c08107692446d229fef862603198fcb4fea4adb52`) and `scenario/ck358-dispositions-r3.toml` (SHA-256 `72df6bb798a17ff822e1f66bf23fa1317f39fac9ed45f64bfb1d7056ca978c48`), under the owner's standing direction to make island monasteries land.


Current CK-358 outputs (the CK-350 table above records the earlier base):

| File | SHA-256 |
|---|---|
| `elevation.png` | `74a6d4b2361b26748ab97d1ea2e9e2056fa83a95a476ac95b5ef09e7df01ca0e` |
| `terrain.png` | `0c23fefb59ba8d408d2824e351da017d9688630a22555df8a8a6a947b1cd7e38` |
| `fertility.png` | `4dfc4669252926e80afb25e76560541fd1f806616d7d2eb852ed259ab76a4d75` |
| `climate_temperature.png` | `368a68ea6b5429457e75165cf9335535ea9cae2eb171c25f1ffcf7330dd4275c` |
| `climate_gdd.png` | `3fc60d590086ec8bbb8cff9682cccfc85ab370e97f6db4aaf77fedcdf034353d` |
| `climate_rainfall.png` | `d81638a7e6c4ffee46f324b8a1e23a615915575699dba6e2b2d179334587109d` |
| `climate_glacier.png` | `578726b5c37cdecbf847de48fb1d8d015ba631081ec9a8af9cf78a9d513ae512` |
| `edits.applied` | `8ef8531ce9095c9df81942c596a17c74f493c0572edba37bc23732655c35c03f` |
| `edits.toml` | `b7e78b9f4b52be5c756f92c9081446d5191e5b8d019ab415ce131180c8f82ee4` |
| `waters.toml` | `18f2c99f24579ca50a69404f612436967f81f5f86b6b6f6af057bff6ed7142af` |
| `water.png` | `80a0d67a93ef7a9fe4e6d1faf322eefd7fa8279ca380363cfc28f999f76ff64c` |
| `rivers.toml` | `e8959053e38c9a814e8a5fb2f1320352494f8f49900af250ea81dc67d1aa0495` |

Seven island edits each paint one cell:

| Edit | Cell | Source terrain |
|---|---|---|
| `island-inchbofin` | (396, 460) | lake |
| `island-inchcleraun` | (384, 451) | lake |
| `island-white-island` | (420, 246) | lake |
| `island-hare-island` | (394, 476) | lake |
| `island-inishmacsaint` | (418, 258) | lake |
| `island-inis-patraic` | (640, 450) | sea |
| `island-inishglora` | (102, 310) | sea |
