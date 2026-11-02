# Map design: Ireland, the look, the data and the renderer

**Status: owner interview in progress, 2026-11-02; technical design drafted, not yet reviewed.** Written by ck-designer-1 (Class S,
`claude-opus-5-5`, high) from the map design session with the owner. The owner's answers are recorded verbatim, with timestamps, in
`.pipeline/coordination/map-design/session-log.md`. §2–§7 record what the owner has decided. §9 (technical design) and §10 (milestone) are
designer proposals awaiting the owner's reading and independent design review.

**Firm** marks owner decisions, with their date. **Proposal** marks designer recommendations. **Open** marks decisions that still need an
answer. A Firm item that the owner chose by reacting to a mock-up says so. Times are UTC on 2026-11-02 unless stated.

This extends the "Map" section of `docs/vision.md`. It builds on `docs/scene-design.md` (live textures, reusable handles, bindless drawing,
tens of thousands of quads), which deliberately left the map representation open (its §14).

## 1. Owner direction

**Firm — owner, opening the session:**

> ok start the map design session for me, the first region is going to be ireland, the style of the map is going to be solid colors overlayed
> across a dark background, i have a unique vision in my head for how i want it to look.

**Firm — owner, 12:47Z, on references:**

> dark black background like a arcade game, flat colors with two-tone dithering that contrast with that. low color count, minimalist, like a
> colored version of 'return of the obra dinn', but not as complicated as we wont be drawing this level of detail yet (although later we will
> when i implement story popups).

The owner asked for criticism and design help, asked to see mock-ups before more design, and asked to be consulted on any decision that
would change the direction they have in mind. Two interactive mock-ups were made on real Irish elevation and coastline (§8). The owner's
verdict on the second: "that looks good".

## 2. Region and extent

- **Firm.** The first region is Ireland with its associated offshore islands. Owner: "we have settled on ireland as the pilot nation, since its
  an island in a relatively normal situation that doesnt have much outside contact but is still large enough to have some politics going on
  internally."
- **Firm (13:55Z).** Everything outside Ireland, including the nearby British coast, is hidden for now.
- **Firm.** Map images use a Mercator-family world projection, and Ireland's images are a crop of the future world map, so they carry over
  when the map grows. (The owner chose this over a local Irish projection that would need repainting later.)

## 3. The look (default geographic map)

### 3.1 Ground and terrain colours

- **Firm.** The sea is a cool black. Unusable land is a warm black. Most of the screen is dark: "i want most of the screen to be dark
  typically".
- **Firm.** All land has a terrain type, and each type has a colour. Usable land (§5.3) is drawn in the type's solid colour. Unusable land is
  drawn "like its greater type": a near-black tinted toward the type's colour.
- **Firm.** Colours named so far: forest green; pasture brown-yellow; mountain grey with white caps; bog dark green; marsh dark brown. "there
  are lots of other terrain types we could do."
- **Firm (mock-up 2).** Mountains show white snow caps only where the mountain is usable, and a light grey on unusable high ground.
- **Firm.** A low colour count. Every colour comes from data, so it can be changed easily.
- **Firm (deferred).** Colour-blind options come later. (The designer had pointed out that forest green next to pasture brown-yellow is a
  red-green confusion pair.)
- **Firm (on mock-up 1).** Those colours were "close, perhaps a little too bright for some of the greens and yellows"; mock-up 2 toned them
  down. Its palette is the starting point in §8.

### 3.2 Edges

- **Firm.** Edges are hard pixel steps: "it needs to look like a 2d pixel art game."
- **Firm.** Only the coast has an outline, drawn solid in the colour of the land behind it. Inland terrain areas are not outlined. (Mock-up 1
  outlined every terrain area; the owner agreed it was too busy.)

### 3.3 Dithering

- **Firm.** Dithering is two-tone and marks features of the world, such as elevation; it is not a camera effect. It is pinned to the
  world, so it does not crawl when the view pans.
- **Proposal (as in the mock-ups).** Each terrain has a pair of colours. Elevation within that terrain's range sets how much of the second
  tone shows, through an ordered 4×4 Bayer pattern indexed by world pixel. The sea uses a lighter cool black on the shallow shelf. Unusable
  land dithers between two near-blacks of its type, so hills still read in the dark.

### 3.4 Rivers, places and buildings

- **Firm.** Rivers have their own blue, drawn over everything, at every zoom. Only major, practically useful waterways are mapped, not every stream.
- **Firm (13:55Z).** Fully zoomed out the map is a geographic picture only. Closer in, places appear as markers; closer still they become
  houses: "i like how you made the cities as markers until we have zoomed in enough, wherein they turn into houses, this looks good, but the
  roads need work, we can handle all of this during implementation."
- **Firm.** The houses are decoration, and their number scales with the location's size. Notable buildings are always drawn when the location
  has them: "if a town has a castle, we should draw a castle somewhere in the town."
- **Open (owner, deferred).** How locations look. Generating houses from a location's id and size is "fine for now", but the owner wants to
  spend much more time on it later.
- **Firm.** Land, rivers, places and buildings share one grid of map pixels; the buildings in the mock-up "look right". The UI is completely
  separate from the map.

### 3.5 Pixels on screen

- **Firm.** Each map pixel is 4 screen pixels square: "i think 4 looks the best."

## 4. Zoom

- **Firm.** Zoom moves in steps, so pixels stay whole. There are enough steps to feel smooth, and stepping is fast: "it needs to be fast and
  natural, with lots of steps".
- **Firm.** Each step redraws the world at a finer or coarser resolution from the map data; this is why the map does not use textures: "it
  would look natural to see the pixels kind of change as we zoom in, that's why i didnt want to use textures."
- **Proposal (as in the mock-ups).** The on-screen pixel size stays fixed; each step changes how much ground a pixel covers, 10 steps per
  doubling, from 3 km to about 12 m per pixel (76 steps). Coast and terrain edges gain generated detail below the data resolution. This detail
  is decoration only.

## 5. Map data

### 5.1 Authoring

- **Firm.** All map data is hand-authored in data files so that modders can change it.
- **Firm.** Text data is TOML. Data that covers the land is stored in images, with the colour of each pixel as its value. A 6-digit hex
  colour gives 16,777,216 values, which the owner considers "more than enough".
- **Firm.** The owner and the designer author the initial data together, by hand in the data files; modders can change it later. Elevation
  starts from the AWS terrain data used in the mock-ups, with guesswork to make the land closer to medieval times.
- **Firm.** No procedural generation for gameplay-critical objects: "i dont want any procedural stuff in the generation of actual
  gameplay-critical objects". Every location and every border is authored. Decoration (dithering, coast detail, houses, implied fields)
  may be generated.
- **Firm.** A map editor comes eventually, not first. Placing locations in a tool is deferred; data is added by hand to begin with.
- **Firm.** A preview script, outside the game, reads the data files and draws them, so authors can check the data before the engine
  can show it.
- **Open.** The sources for the medieval terrain layer and how guesswork is recorded so later research can correct it. Present-day land cover
  is a poor guide: medieval Ireland had far more forest and bog.

### 5.2 Cells

- **Firm.** The map grid is 500 m × 500 m cells (0.25 km²); the owner also calls them nodes. This is the smallest unit of land
  ownership. "As detailed as we can", balancing realism against computational pragmatism, which may need testing later.

### 5.3 Usable land

- **Firm.** Land is usable when it is open to development or resource extraction; land that is not is the dark "wasteland". Usable land is
  authored, not computed at run time.
- **Firm (on mock-up 1).** The share of dark land in the mock-ups is "fine for now".

### 5.4 Land ownership

- **Firm.** Land ownership is separate from the ownership of anything in a location. Every land cell has a de jure ruler, and that is the
  colour the political map paints it. Political entities such as monarchs claim sets of cells, never locations: "political entities like
  monarchs will lay claim to entire sets of nodes, not any locations, locations have no owner."
- **Firm.** De facto rule is also kept per cell. It is rare in Ireland but common elsewhere. It also serves wartime occupation, which is
  temporary: an occupier may choose to collect taxes on the subjects of an occupied location.
- **Firm.** Ownership is kept cell by cell, and only changes from the starting state are saved.
- **Firm.** Political painting is based on cell ownership only and has nothing to do with counties or provinces, which are organisational.
- **Firm.** Starting de jure ownership is an authored image, with the ruler as the pixel colour. Unpainted land is terra nullius, which is
  also the only fallback; most of the world will be simulated as uninhabited. Ireland's bogs and mountains belong to the local monarch and
  are painted so. Borders need research, with educated guesses where the record is thin.
- **Firm.** Cores attach to land ownership, not to locations. (This corrects the per-location wording in `docs/game/diplomacy.md`, which
  carries a dated note.)
- **Firm.** A peace deal transfers exactly the cells the player chose, and only those cells change colour. The player paints cells by hand,
  picks a location's land, or picks an organisational unit (§5.6) in one click. Example: "i want munster, kilkenny, wexford, and this little
  chunk of land nodes in carlow."

### 5.5 Locations

- **Firm.** Every location is placed by hand. Locations have no owner; each building in a location has its own set of owners, and income goes
  to the building's owners. A political entity may own a building, which is separate from owning land. For warfare, a location goes with the
  cell it stands on.
- **Firm.** The skeleton comes first (towns, castles, abbeys, major settlements), then farms. Farms are consolidated to keep memory low:
  nearby farms count as one at a higher size level, and full farm detail means villages with farm buildings of various sizes attached, not
  one location per farm. A truly isolated farm is allowed only where there is evidence for it. Subsistence farms are most numerous early in
  the game and close as people move to towns.
- **Firm.** A location covers one or a few cells and draws resources from the cells within a radius. Land outside every radius belongs to no
  location. Where radii overlap, the nearest location takes the cell. All the labourers of a location count as working one farm, even when it
  stands for hundreds of farms.
- **Firm.** A building can use land across a border. Its owner then pays taxes to each land ruler in proportion to the land used from each.
- **Firm.** The map design covers where a location is and what it shows; what it holds for the economy belongs to the game design documents.

### 5.6 Organisational units

- **Firm.** Units nest from broad to small (counties are subsets of provinces): **region, area, province, county**. Example: British Isles, Ireland, the four provinces (Ulster,
  Connacht, Leinster, Munster), the modern counties (Kerry, Cork, Limerick, Clare, Galway, Mayo, ...). They have no rulers and no gameplay
  effect; they exist so a player can select land in one click. How deep nesting goes is open.
- Note: the counties are mostly Anglo-Norman and Tudor creations, chosen because players know them and their borders are well documented.

## 6. Map modes

- **Firm.** The geographic map is the default. Dozens of map modes are planned, each changing the appearance of the same rendered land.
- **Firm.** The political map paints all land, wasteland included, in the colour of its de jure ruler, with no terrain. Each ruler has its own
  colour regardless of overlord, so a baron differs from his liege.
- **Open.** A faint terrain-hint dither on the political map (the owner floated it; mock-up 2 has it as a button). How de facto rule and
  occupation are shown.
- **Proposal.** Every map mode is a colouring rule over the same world data, with its palette in data, so a new mode needs no new renderer.

## 7. Open questions carried forward

- Roads: drawn how, and authored or generated (the mock-up's generated tracks "need work").
- Detail at close zoom inside large usable areas, which currently reads flat.
- The full list of terrain types and their colours; whether lakes use the sea colour.
- How the map shows unmapped land (the vision's fog of war for land without maps).
- Starting de facto rule data, if any is needed for Ireland.

## 8. Mock-ups

Two interactive mock-ups were published to the owner as a private page; mock-up 2 is current. Real data: coast, elevation and sea depth from
AWS Terrain Tiles (terrarium encoding, zoom level 8, about 360 m per sample in Ireland, resampled to a 500 m grid); the larger loughs and a few
rivers from Natural Earth 1:10m. Invented: terrain placement, usable land, polities, tracks and houses. The generator, page template and a
saved copy of mock-up 2 are in `.pipeline/coordination/map-design/mockup/`.

**Proposal — mock-up 2 palette, the starting point for the data files:**

| Terrain | Solid (usable) | Second tone | Unusable | Unusable, second tone | Other |
|---|---|---|---|---|---|
| Pasture | `#a08944` | `#806c35` | `#15110a` | `#211a0f` | |
| Forest | `#367534` | `#285a27` | `#0a120a` | `#122012` | |
| Bog | `#2c4c2d` | `#203a22` | `#090f0a` | `#101a11` | |
| Marsh | `#5c3e25` | `#462e1b` | `#110b07` | `#1c130c` | |
| Mountain | `#7f8187` | `#606268` | `#101114` | `#1b1c20` | caps `#e8e9ed`, unusable high ground `#3c3e44` |

Sea `#05080e`, shallow sea `#0b1422`, lakes `#0b1422`, rivers `#3a78c2`.

## 9. Technical design (Proposal, unreviewed)

Everything in this section is a designer proposal unless it repeats a Firm item above. Names are placeholders for the issue specs.

### 9.1 Packages

- **`ck-map`** (new, `crates/map`), backend-neutral and windowless: reads and validates the data files, holds the cell grids and the run-time
  ownership state, computes location catchments, and answers queries (the cell at a point, a location's cells, the cells of a county, a
  selection). It knows nothing about the GPU.
- **`ck-render-2d`** gains one map pass, designed once and implemented on Metal and Vulkan together, fed from `ck-map`'s grids. Each
  slice lands on both backends at the same time and is tested on both (Firm, below). Places and buildings are sprites drawn
  through the existing M5 scene path (bindless textures, ordered quads).
- **Lua** gets a small surface later: camera, map mode, and highlight sets. The first milestone drives the camera from input bindings.

### 9.2 Data files

One directory per map, for example `data/map/`:

| File | Holds |
|---|---|
| `map.toml` | Projection (spherical Mercator), the crop's bounds, cell size, and the layer file names |
| `terrain.png` | Terrain type per cell (RGB key; sea and lake are terrain types) |
| `usable.png` | Usable or not per cell (two keyed colours) |
| `elevation.png` | 16-bit greyscale: metres above sea, with sea depth below a fixed offset |
| `rulers.png` | Starting de jure ruler per cell (RGB key; `#000000` is terra nullius) |
| `counties.png` | County per cell (RGB key). Only the finest level is painted |
| `terrain.toml` | Terrain types: key colour, palette, relief range, snow-cap heights |
| `rulers.toml` | Key colour to political entity id (the entities are defined in game data) |
| `units.toml` | The nesting: each county's province, each province's area, each area's region |
| `locations.toml` | `[[location]]`: id, name, kind, size level, position (latitude, longitude), optional radius, notable buildings |
| `rivers.toml` | River lines as latitude/longitude points |
| `modes.toml` | Map-mode colouring rules and palettes |

- Painting only counties keeps the nesting consistent: a province is the union of its counties, so no two layers can disagree. County
  painting is independent of ruler painting.
- Positions in text are latitude and longitude, so authors can look places up on any map. The engine projects them.
- **Validation is strict.** Every pixel must be a colour in its key, every image must have the crop's size, and every location must stand on
  land. A failure names the file, the pixel or entry, and the problem. This catches the usual image-editor damage: anti-aliased brush edges,
  colour-profile conversion and lossy formats.
- **Cell size in projection units.** Mercator stretches with latitude, so a fixed cell is 839 projected metres, which is 500 m on the ground at
  53.4°N. On the ground a cell is about 523 m at Ireland's south coast and 476 m at its north coast.

### 9.3 Run-time state

- Grids for terrain, usable land, elevation and county are read-only after loading.
- De jure and de facto rulers are grids of ruler ids, starting from `rulers.png` (de facto starts equal to de jure). Changes are recorded per
  cell and saved as the difference from the start.
- Catchments are computed at load from the authored locations: each location takes the cells within its radius (from its size level in data,
  or its own override), and the nearest location wins an overlap, with ties broken by location id. Cells outside every radius have no
  location. This is derived the same way every time from authored data; it creates no objects.
- Ireland's crop is about 716 × 912 cells (about 650,000), a few megabytes for all grids together. They are loaded whole: no tiling or streaming before the world
  map.

### 9.4 Drawing

1. **Map image.** Each frame the GPU fills a small image at map-pixel resolution (screen size divided by 4, about 130,000 pixels at 1080p)
   from the cell grids, which live on the GPU as textures. For each pixel it finds the cell, applies the decoration-only sub-cell detail at
   close zoom, picks the colour by the map mode's rule (terrain and usable land, elevation dither, snow caps, coast outline, shallow sea; or
   ruler colour), and writes it.
2. **Overlays in the same image:** rivers as one-pixel lines; places as markers when zoomed out, and houses plus notable buildings when zoomed
   in. Houses are generated from the location's id and size, so they are the same every time; notable buildings come from the location's
   data. A highlight layer marks a set of cells, for the peace screen and for a farm's catchment.
3. **Enlarge** the image four times, without smoothing, onto the window.
4. **The UI** is drawn after that, at full resolution, as a separate pass.

- Ownership changes upload only the changed part of the ruler texture.
- The camera moves on the map-pixel grid of the current step, so the land never shimmers while panning.
- **Firm (owner, 15:15Z).** The work is backend-agnostic and done once: Metal and Vulkan are implemented together and both are tested,
  never one after the other: "we shouldnt be implementing one and then the other, we are doing all the work at once now." Both must produce
  the same picture; Vulkan matters for Linux.

### 9.5 Preview script

A small Python script outside the game reads the data directory and writes a picture or an HTML page, using the same colour rules. It also
reports validation errors early, and can suggest starting borders from location positions for authors to paint over.

### 9.6 Not in the first milestone

Locations and their sprites; the political map and ownership changes; catchments; streaming and tiles; the map editor and location placer;
roads; fog of war; de facto display; the peace screen; Lua control of the map beyond the camera. §9.2–§9.4 still describe these, so the first
milestone's data format and pass leave room for them.

## 10. Milestone proposal (Proposal, unreviewed)

**Firm (owner, 15:15Z).** The first milestone is "Ireland on screen", terrain only: "we dont need locations settled yet, we dont need
political maps yet, just the terrain is fine for the first milestone." It is one milestone on both backends, not a Metal milestone followed
by a Vulkan one. Acceptance is the owner's reading of the running map plus windowless tests.

**Milestone 6, "Ireland on screen".** Proposed slices, each its own issue, each covering Metal and Vulkan together where it touches the GPU:

1. **Terrain data** for Ireland, authored with the owner: `map.toml`, `terrain.png`, `usable.png`, `elevation.png` (AWS data plus medieval
   guesswork), `terrain.toml`.
2. **Preview script** for that data.
3. **`ck-map` terrain loading**: the projection, the grids, strict validation and queries; windowless tests.
4. **The map pass on both backends**: terrain colours, usable and unusable land, elevation dither, snow caps, coast outline, sea shallows,
   zoom steps and panning, 4× enlargement, the UI kept separate.
5. **Rivers** (Firm, owner, 15:18Z: included). `rivers.toml` and the one-pixel line overlay, on both backends. Only major waterways that
   are practically useful: "we dont need to add in every single little revine and stream".

**Later milestones (sketch):** locations, markers, houses and notable buildings; the political map, ownership state and changes, catchments
and the highlight layer; then the peace screen's needs, roads and the editor.

Window tests stay inside the owner-input layer.

## 11. Next steps

1. The owner reads §9 and §10 and corrects them.
2. Independent design review of both brands, as for earlier designs.
3. ck-manager files the milestone after the owner approves.

## Provenance

Drafted 2026-11-02 by ck-designer-1 from the session log, `docs/vision.md`, `docs/game/diplomacy.md`, `docs/scene-design.md` and
`docs/render-design.md`. No code, issues or builds.
