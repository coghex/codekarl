# Map design: Ireland, the look, the data and the renderer

**Status: technical design approved, 2026-11-02 (revision 2: ck-reviewer-2 and ck-reviewer-1 design re-reviews); revision 3 is editorial.** Written by ck-designer-1 (Class S,
`claude-opus-5-5`, high) from the map design session with the owner. The owner's answers are recorded verbatim, with timestamps, in
`.pipeline/coordination/map-design/session-log.md`. §2–§7 record what the owner has decided. §9 (technical design) and §10 (milestone) are
designer proposals, revised after design review 1 (`.pipeline/coordination/map-design/design-review-ck-reviewer-1.md` with its owner
addendum, and `design-review-ck-reviewer-2.md`) and the owner's decisions on it. Both reviewers approved revision 2
(`design-review-2-ck-reviewer-2.md`, `design-review-ck-reviewer-1-r2.md`). Revision 3 records two later owner answers as Firm and fixes two
non-blocking notes; it changes no design. Milestone 6 (§10) was then implemented and accepted. §12 records the milestone 9 land,
climate and look, which are accepted on native Metal (slices 9a, 10a and 10b: CK-234, CK-235 and CK-236); the Vulkan catch-up is
parked.

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

> Since M9, the default map draws land by fertility and hillside dither, not usable or unusable (§12.3). **Superseded since M9 (owner, 2026-11-09): see §12.**

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

> Since M9, fertility uses a blue-noise pattern and relief is a hillside dither from slope (§12.3). **Superseded since M9 (owner, 2026-11-09): see §12.**

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

- **Firm.** Each map pixel is 4 window points square, in logical units that do not depend on the display's scale: "i think 4 looks the
  best." The owner chose this in mock-up 2, which drew 4 CSS pixels per map pixel. After design review 1 the owner corrected the earlier
  wording "4 screen pixels": "i shuold never have asked for anything in exact screen pixels, we arent working in exact pixels, that's a
  mistake or oversight."
- **Firm (owner, 15:36Z).** The look must stay as in the mock-up: "i want what the mockup looked like, i thought that was perfect, if
  rounding looks the same or mostly similar then that is fine". §9.3 gives the rounding rule; on the owner's 2× display it is exactly 8
  drawable pixels per map pixel, as in the mock-up.

## 4. Zoom

- **Firm.** Zoom moves in steps, so pixels stay whole. There are enough steps to feel smooth, and stepping is fast: "it needs to be fast and
  natural, with lots of steps".
- **Firm.** Each step redraws the world at a finer or coarser resolution from the map data; this is why the map does not use textures: "it
  would look natural to see the pixels kind of change as we zoom in, that's why i didnt want to use textures."
- **Firm (owner, 15:36Z).** Milestone 6 controls are keyboard only: "controls are keyboard only for now". Mouse drag and wheel are
  later work.
- **Firm (owner, 15:36Z, confirmed later: "i meant 'cant'").** 60 frames per second is a hard bar: if it cannot be met while the map is
  being redrawn, the design is revisited.
- **Firm (owner, relayed by ck-manager after the re-review).** 60 fps applies "While moving": the map is redrawn at 60 fps during panning and
  zooming, and when the map changes. When nothing moves, it is idle and redraws only on demand.
- **Proposal (as in the mock-ups).** The on-screen map-pixel size stays fixed; each step changes how much ground a map pixel covers, 10 steps
  per doubling. Coast and terrain edges gain generated detail below the data resolution. This detail is decoration only. §9.3 gives the exact
  step table.

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
- **Firm.** A preview tool, outside the game, reads the data files and draws them, so authors can check the data before the engine
  can show it. The owner left its language to the designer (§9.8).
- **Open.** The sources for the medieval terrain layer and how guesswork is recorded so later research can correct it. Present-day land cover
  is a poor guide: medieval Ireland had far more forest and bog.

### 5.2 Cells

- **Firm.** The map grid is 500 m × 500 m cells (0.25 km²); the owner also calls them nodes. This is the smallest unit of land
  ownership. "As detailed as we can", balancing realism against computational pragmatism, which may need testing later.

### 5.3 Usable land

> Since M9, usable land is computed fertility, from the terrain, drainage, slope and a computed climate (§12.1–§12.2). **Superseded since M9 (owner, 2026-11-09): see §12.**

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
- **Firm (2026-11-09).** An elevation map mode shows actual height from black to white (§12.5).
- **Built in milestone 9:** the stockpile and yield modes (`docs/economy-design.md` §3.22) and the elevation mode, each chosen by a
  button on the map-mode bar; clicking the shown mode's button again returns to the geographic map.

## 7. Open questions carried forward

- Roads: drawn how, and authored or generated (the mock-up's generated tracks "need work").
- Detail at close zoom inside large usable areas, which currently reads flat.
- The full list of terrain types and their colours; whether lakes use the sea colour.
- How the map shows unmapped land (the vision's fog of war for land without maps).
- Mouse drag and wheel zoom, after milestone 6.
- **World scale (from design review 1).** Mercator cells shrink on the ground as latitude rises: a fixed cell at 60°N covers about 70 % of the
  ground of one at Ireland's latitude (ground area goes with cos² of latitude), and a quarter of one at the equator. Cells are gameplay units, so the world-map design must correct for
  it, for example by weighting resources by ground area or giving catchment radii in metres. Inside Ireland the variation is about ±5 %.
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

## 9. Technical design (Proposal, revision 2)

Everything in this section is a designer proposal unless it repeats a Firm item. Names are placeholders for the issue specs. Revision 2
answers design review 1: reviewer-2's D1–D3 and reviewer-1's F1–F3 (with its owner addendum), and folds in the owner's decisions on them.

**Firm — owner, on design review 1 (relayed by ck-manager):**

> we will need to implement gpu written images first then, that is a hard requirement. we need fast image generation occuring often, if the
> player is zoomming in and out and moving the window around, we will be generating the image each frame, so it has to be gpu, cpu rendering
> would be a waste of time.

So the production map image is written by the GPU every frame, and the engine support for GPU-written images comes first, on both backends.
A CPU rasterizer is not an implementation option. The CPU reference in §9.4 exists only for tests and the preview tool.

Wording: the map data lives on the GPU as **data grids** (numbers per cell, read exactly), not as pictures. Nothing on the map is a
pre-drawn picture that gets scaled, which keeps the owner's "that's why i didnt want to use textures" (§4).

### 9.1 Packages and ownership

- **`ck-map`** (new, `crates/map`), backend-neutral and windowless:
  - reads and validates the map files;
  - holds the cell grids and packs them for the GPU (§9.5);
  - owns the coordinate contract (§9.3) and the **reference colour function** (§9.4), a plain Rust implementation used by tests and the
    preview tool;
  - computes the river pixels once per zoom step (§9.4.7).
- **`ck-render-2d`** gains a **map lane** in its scene bridge, beside the scene snapshot. Through it the application publishes two immutable
  inputs:
  - the **map input**: the packed grids, the terrain palette and rule table, and a generation number. It is published once per loaded map.
  - the **camera snapshot**: the step, the top-left world pixel, the view size in map pixels, the visible river pixels (selected from the
    step's prepared coverage), and the metrics
    generation it was computed for.

  render-2d stays backend-neutral. It computes the enlarged map's destination rectangle with the accepted points-to-pixels rule (§9.3) and
  hands the GPU owner plain data.
- **The GPU owners** (`ck-gpu-metal`, `ck-gpu-vulkan`) hold everything native: the grid images, the low-resolution map target, the map
  pipeline and its encoding. These are the new engine work of §9.2.
- **The application** (`apps/codekarl`) owns map loading and the one camera (§9.6). No Lua map API is needed in milestone 6.

### 9.2 GPU-written images: the engine work that comes first

This extends the "later mechanism" that scene-design §5 promised and did not build. It is specific to the map. It adds no general
render-graph and no writable image for Lua.

1. **Grid images.** When a map input arrives, the GPU owner uploads its packed grids as immutable RGBA8 images, sampled only by exact
   integer reads (`texture.read` in Metal, `texelFetch` in Vulkan): never filtered or scaled. RGBA8 is the only image format both backends
   already create, so no new format support is needed (§9.5 gives the packing). A new map input replaces the grids. The old grid images are
   retired under each backend's existing render-owner ledger: a native resource stays owned until every frame that read it has a completion
   proof, and an unproven completion keeps it.
2. **The map target.** One RGBA8 image, the size of the camera's view in map pixels, usable as both a colour attachment and a sampled image.
   It is created at the first frame with a map, and replaced when the view size changes. The old target is retired under the same ledger rule.
   At any time there is one current target and at most a bounded number of retired ones awaiting proof. The bound is the presenter's
   frames-in-flight limit; Metal's presenter currently allows one.
3. **One frame, one submission.** Each frame's existing command buffer gains a pass before the window pass:
   1. **The map pass** writes the map target from the grid images and the camera snapshot: a full-target triangle for the terrain colour
      (§9.4), then the river pixels as one-map-pixel instanced quads.
   2. **The window pass**, as today: the clear, then **the enlarged map** as one nearest-sampled quad, then the legacy quad, then the scene's
      batches. The UI is drawn last, over the map, at full resolution.
4. **Ordering.**
   - **Metal:** the target is a hazard-tracked resource on the presenter's one queue. That orders the map pass's writes before the window
     pass's reads, and the next frame's writes after this frame's reads.
   - **Vulkan:** explicit barriers in the same command buffer. One at the start of the map pass orders the previous submission's shader reads
     before this pass's colour writes. One between the passes orders the colour writes before the shader reads, with the layout change to
     shader-read-only.
   - Neither backend waits on the CPU for this ordering.
5. **Zero size and minimizing.** No map pass is encoded while the drawable has no size, or while the presenter pauses for minimizing or loss
   of focus (the M5 choice). The current target is kept for the next visible frame, and replaced if the size changed meanwhile.
6. **Refusals.**
   - A grid image refused at map load is a named startup error. The application reports it and exits, as it does for configuration errors.
   - A target refused at resize fails that frame under the presenter's existing per-frame failure rule (reported, and retried on the next
     frame). It is never drawn with a stale or stretched target.
7. **Teardown.** The map target, retired targets and grid images join the existing shutdown proof. They are destroyed only after their last
   readers are proven complete, before the device. An unproven completion keeps them, as it does every other native resource.
8. **Readback fixtures.** These run off-screen through each backend's production map encoder (Metal's off-screen frame path and Vulkan's
   readback module already exist for the quad and scene):
   - the map target, read back, equals the expected colour function byte for byte, for a small asymmetric fixture map at two steps and an
     odd view size, including a pan away and back. In slice 1 the expected function is slice 1's own minimal rule (terrain index to
     palette), because the full reference arrives in slice 2; the comparison with the full reference (§9.4) belongs to slice 6;
   - the enlarged window pass equals the expected enlargement of that target under the fixture's metrics;
   - one resize case replaces the target and retires the old one only after its proof.

   No scale or performance campaign is part of this slice.

### 9.3 Coordinates and raster contract

One convention, used by the files, the loader, the reference function, the preview tool, both GPU passes, the camera and the river pixels.
**All map arithmetic is integer.** No step of the colour function uses floating point, so Metal and Vulkan cannot round differently.

- **The world grid.**
  - Spherical Mercator, as in web maps. Projected x runs east from the antimeridian. Projected y runs south from the top of the projection
    (about 85.05°N).
  - A cell is exactly **839 projected metres** square, which is 500 m on the ground at 53.4°N. Cell (i, j) covers projected
    [839·i, 839·(i+1)) east and [839·j, 839·(j+1)) south.
  - The whole world is about 47,800 cells wide, so cell indices fit easily in 32 bits.
  - A latitude/longitude position belongs to the cell its projected point falls in (floor), and the cell's centre is at (i + ½, j + ½).
- **The crop.** `map.toml` gives the crop's global origin cell (i₀, j₀) and its size in cells. Every image is exactly that size. Image pixel
  (x, y) is cell (i₀ + x, j₀ + y), so rows run south and columns east. The pilot's exact origin and size are fixed in the data issue; they are
  global indices, so Ireland's files carry over to the world map. Anything outside the crop is sea.
- **Fixed point.** Positions inside cells are counted in 1/32768 of a cell. A world-wide coordinate is then below 2³¹, and fits a signed
  32-bit integer on both GPUs.
- **Zoom steps.** Step s = 0 … 75. A map pixel at step s is P(s) = round(32768 × 6 × 2^(−s/10)) units.
  - Step 0 is 6 cells, about 3 km per map pixel. Step 75 is about 0.033 cells, about 17 m at Ireland's latitude.
  - The table of 76 integers is computed once and frozen in the spec, so no GPU ever evaluates 2^x.
  - Ten steps per doubling, as in the mock-ups.
- **World pixels.** At step s, world pixel (I, J) covers fixed-point x in [I·P(s), (I+1)·P(s)), and likewise for y.
  - Its **sample point** is its centre: X = I·P(s) + ⌊P(s)/2⌋.
  - The cell under it is (X >> 15, Y >> 15).
  - Dither, the coast test and river pixels all index world pixels (I, J), so they are pinned to the world at each step.
- **The camera.**
  - The camera is the step s and the integer world pixel (I_left, J_top) at the view's top-left. Panning moves it by whole world pixels, so
    the land never shimmers.
  - Zooming keeps the world point at the view's centre fixed: the new top-left is computed from the centre's fixed-point position.
- **View size and screen mapping (logical units, Firm §3.5).**
  - The view is ⌈window width in points / 4⌉ by ⌈window height in points / 4⌉ map pixels, anchored at the top-left. A partial map pixel at
    the right or bottom edge is cropped, never stretched.
  - The enlarged map is drawn as one quad of (view size × 4) points, through render-2d's accepted points-to-pixels rule: each edge lands at
    the rounded product of its position in points and the axis scale.
  - On a 2× display every map pixel is exactly 8 drawable pixels, as in the approved mock-up. At a fractional scale, neighbouring map pixels
    may differ by one drawable pixel.
  - There is no exact-native-pixel requirement (reviewer-1's addendum withdraws it).
  - On resize, the view size and the metrics generation are published together in the camera snapshot, so a frame never pairs a view size
    with metrics it was not computed for.

### 9.4 The colour function (the reference)

> Since M9, rules 4–6 (usable land, unusable land, snow caps) are replaced by §12.3–§12.4's rules. **Superseded since M9 (owner, 2026-11-09): see §12.**

The reference is a Rust function in `ck-map`. It takes the map, the step and a world pixel, and returns an RGBA8 colour. The GPU map pass
must give the same bytes for every pixel; that is what "the same picture on both backends" means. The readback fixtures in §9.2 check it on
each backend, and so Metal and Vulkan also match each other. The spec writes each rule out exactly. In outline:

1. **Cell lookup:** the cell under the world pixel's sample point (§9.3), after the sub-cell detail of rule 2. Outside the crop is sea.
2. **Sub-cell detail** (decoration only). The sample point is moved by a jitter of integer value noise from a frozen 32-bit hash, in three
   octaves taken from mock-up 2: lattices of 3, 0.7 and 0.18 cells, half-amplitudes of 0.9, 0.28 and 0.07 cell, starting at steps 14, 34
   and 53 (below 2.4, 0.6 and 0.16 cells per map pixel, the mock-up's thresholds). Elevation is interpolated between cell centres at the
   moved point, as in the mock-up. A pixel whose unmoved sample point is outside the crop is sea before any noise is evaluated. CK-178 holds
   the exact algorithm and test vectors. The result is the same at every visit and on every backend.
3. **Water.** Sea and lake are terrain types. Sea draws the shallow-sea tone where depth is under the shallow threshold and the Bayer value
   is below 8 × (threshold − depth) / threshold (integer division). Otherwise it draws the sea colour.
4. **Usable land** draws the terrain's solid colour, and its second tone where Bayer < level × mix / 100. Here level =
   clamp(16 × (elevation − relief₀) / (relief₁ − relief₀), 0, 16), and mix is the terrain's integer percentage.
5. **Unusable land** draws the terrain's dark colour, and its dark second tone where Bayer < level / 2.
6. **Snow caps** apply on mountain terrain where Bayer < clamp(16 × (elevation − cap₀) / (cap₁ − cap₀), 0, 16). Usable cells draw the cap
   colour; unusable cells draw the light-grey high-ground colour (Firm, §3.1).
7. **Rivers** are drawn after the terrain, in the river colour (Firm, §3.4). For each river line, the points are projected to fixed point at
   step s, and each consecutive pair of world pixels is joined by integer Bresenham. Every pixel visited is a river pixel. **Firm (owner: "CPU prep
   is fine").** `ck-map` computes this coverage on the CPU once per zoom step and keeps it until the step or the map changes; each camera
   snapshot selects the visible pixels from it without re-running the lines on every pan. The GPU draws each, every rendered frame, as a
   quad covering exactly one map pixel. The pixels are therefore
   identical on both backends, independent of GPU line rules.
8. **The coast outline:** a land pixel with any of its four neighbours (left, right, up, down, at the same step, by the same lookup) in sea
   or lake draws its terrain's solid colour (Firm, §3.2). It takes precedence over rules 4–6, and rivers draw over it.

- **Bayer** is the standard 4×4 ordered matrix with values 0–15, indexed by (I mod 4, J mod 4) of the world pixel.
- Colours are the palette's hex values written unchanged into the UNORM target, with alpha 255.
- Coarse steps sample the cell under each pixel's centre, as the mock-ups did. A feature smaller than a map pixel can appear and vanish
  between steps; a majority rule is a possible later refinement.

### 9.5 Files, loading and packing

> Since M9, `usable.png` is replaced by `fertility.png` and the climate layers, and grid A's packing changes (§12.6). **Superseded since M9 (owner, 2026-11-09): see §12.**

| File | Holds |
|---|---|
| `map.toml` | The crop's global origin cell and size, the layer file names. Projection and cell size are fixed by §9.3, and the file states them for checking |
| `terrain.png` | Terrain type per cell (RGB key; sea and lake are terrain types) |
| `usable.png` | Usable or not per cell (two keyed colours) |
| `elevation.png` | 16-bit greyscale: elevation + 11,000, in metres, so sea depth down to 11 km fits |
| `terrain.toml` | Terrain types: key colour, palette (solid, second tone, dark, dark second tone; caps and high ground for mountains), relief range, integer mix percentage, cap heights; the shallow-sea threshold |
| `rivers.toml` | Major rivers as lines of latitude/longitude points |

Later milestones add `rulers.png`, `counties.png`, `rulers.toml`, `units.toml`, `locations.toml` and `modes.toml`, as in §5. Milestone 6
neither needs nor reads them.

- **Strict validation.** Every pixel must be a colour in its key, every image must be the crop's size, and every river point must lie in the
  crop. A failure names the file, the pixel or entry, and the problem. This catches anti-aliased brush edges, colour-profile conversion and
  lossy formats.
- **A data-preserving PNG path.** `ck-map` decodes the map images itself (the `png` crate is already a dependency), not through the resource
  service's display decoder, which premultiplies and reduces 16-bit samples to 8. It reads raw sample values and ignores colour-management
  chunks (`iCCP`, `gAMA`, `sRGB`, `cHRM`), which image editors add.
- **GPU packing.** Two RGBA8 grids the crop's size, alpha always 255:
  - grid A: R is the terrain index, G bit 0 is usable;
  - grid B: R and G are the high and low bytes of the 16-bit elevation.

  Shaders read them exactly. A unorm byte read back as a float and multiplied by 255 rounds to the original integer on both backends.
- Ireland's crop is about 716 × 912 cells (about 650,000), a few megabytes in all. It is loaded whole, with no tiling or streaming.

### 9.6 The application path

- **Configuration.** A new `[map]` section in the existing configuration file names the map directory. Without it, the application behaves
  as today.
- **Loading.** At startup, before the window's first frame, the application loads and validates the map off the frame-critical path. A
  failure is a startup error that names the file and the problem, and exits as configuration errors do. It never opens a window with half a
  map.
- **Initial view.** The coarsest step at which the whole crop fits the view, centred.
- **One camera owner.** A map controller in the application holds the camera.
  - It reads new map actions from the existing configurable bindings (`[input.bindings]`). There are six actions: pan left, right, up and
    down, and zoom in and out.
  - Default keys are the arrow keys and `=` / `-`.
  - A held pan key moves a configurable number of map pixels per frame. A held zoom key repeats at a configurable rate.
  - On each change it computes the camera snapshot, with the visible river pixels selected from the step's prepared coverage, publishes it
    through the map lane, and wakes the renderer. While a key is held it updates every frame; when nothing moves, nothing is published and
    the renderer stays idle (Firm, §4).
  - The existing focus, minimize, close and lifecycle behaviour is unchanged.
- **Keyboard only** (Firm, §4). Mouse input is later work.

### 9.7 Performance

- **Firm (§4).** 60 frames per second while panning, zooming or when the map changes, on both backends; idle, on-demand redraw otherwise.
- **Proposal.** The map pass is small: about 120,000 map pixels on the owner's 3456 × 2234 display at 8 drawable pixels each, with a few
  exact reads per pixel. The evidence shows frames at 60 fps while zooming and panning, on both backends, at the owner's resolution,
  from the existing per-frame timing and presentation intervals only. There is no map-pass-specific GPU timing: the owner's rule is no
  engine debug counters. It also records load-and-validate time for the whole of Ireland, with 1 second as a provisional target.

### 9.8 The preview tool

The owner left the language to the designer. **Proposal:** a small Rust command-line tool in the workspace. It reads a map directory,
validates it with `ck-map`'s loader, and writes PNG images and an HTML page at chosen steps, using the reference colour function. Its pictures
are therefore exactly the game's, and authors see validation errors before the engine runs. It never opens a window and needs no GPU.

### 9.9 Later, not in milestone 6

- Locations and their sprites. **Proposal:** draw them into the map target in the map pass, so they share its grid (Firm §3.4) and each
  backend's pass keeps one composition.
- The political map, ownership state and changes (with partial grid updates), catchments, the highlight layer.
- Roads, fog of war, de facto display, the peace screen, the editor and location placer, mouse input, Lua control of the map.
- Streaming and tiles.

The file format and the map lane leave room for these.

## 10. Milestone proposal (Proposal, revision 2)

**Firm (owner, 15:15Z).** The first milestone is "Ireland on screen", terrain only, as one milestone on both backends, and acceptance is the
owner's reading of the running map plus windowless tests. **Firm (owner, 15:18Z).** Major rivers are included.

**Milestone 6, "Ireland on screen".** Every slice that touches the GPU lands on Metal and Vulkan together.

| # | Slice | Depends on | Delivers |
|---|---|---|---|
| 1 | **GPU-written map image** (engine) | — | §9.2: the map lane, grid images, the map target's lifecycle, the map pass with a minimal colour rule (terrain index to palette), the enlarged composition under the scene, and readback fixtures on both encoders against that minimal rule. It owns the packed-grid layout of §9.5, which slice 3 produces |
| 2 | **Coordinates and reference** | — | §9.3 and §9.4 in `ck-map`: the world grid, the crop, the step table, the reference colour function and the river pixel rule, with windowless tests on an asymmetric fixture |
| 3 | **Map loading** | 2 | §9.5: TOML and PNG reading, strict validation, the 16-bit path, GPU packing |
| 4 | **Preview tool** | 2, 3 | §9.8 |
| 5 | **Ireland's terrain data** | 4 | The files of §9.5 for Ireland, authored with the owner. It records sources, licences and required attribution: AWS Terrain Tiles combine several sources that ask for attribution; Natural Earth is public domain. It also records where medieval guesswork departs from the source data |
| 6 | **Full colour rules on the GPU** | 1, 2, 3 | Rules 1–6 and 8 of §9.4 in both map passes, matching the reference in the readback fixtures |
| 7 | **Application and camera** | 6 | §9.6 and §9.7: configuration, startup loading, the initial view, keyboard actions, zoom steps and panning, the performance evidence |
| 8 | **Rivers** | 6, 7 | Rule 7 of §9.4: `rivers.toml`, the river pixels prepared per zoom step and selected per snapshot, drawing them in both map passes, a readback fixture |

**Acceptance.**
- Both backends open the same authored map of Ireland.
- Tests drive the camera through injected actions, inside the owner-input layer.
- Both backends close normally.
- 60 frames per second is shown while zooming and panning.
- The owner reads the running map against mock-up 2.

## 11. Next steps (done)

Milestone 6 was filed slice by slice, reviewed and implemented after the owner's go; it is accepted. The original steps were: one issue
spec per §10 slice, filed by ck-manager, with implementation held until the owner said so ("File only").

## 12. Land, climate and the look since M9 (Firm: owner-signed, 2026-11-09)

The owner redesigned usable land, added a computed climate, and set the map's look in a series of mock-up sessions on 2026-11-09. The
signed records are in `.pipeline/coordination/m9-design/`:
- `usable-land-proposal-r4.md` (23221a44): fertility and climate;
- `map-look-design-r4.md` (33f0615e): the look, glaciation and the elevation mode;
- their mock-ups, in `mockups/`.

The owner's direction: the system "is meant to work around the globe", so every rule uses only what any map has (terrain, elevation,
latitude and a few per-map values), with geographical, not Ireland-specific, sources. The economy's use of fertility is in
`docs/economy-design.md` §3.24.

### 12.1 Fertility (per cell, in thousandths)

**Firm.** Each land cell has a fertility from 0 to 1000 ("a percentage to each spot with a formula"; river valleys near 100%, plain land
near 50%):

`fertility = clamp((base_t + valley_t × V ÷ 1000) × slope_factor × climate_factor, 0, 1000)`

- **The terrain values** are data:

  | Terrain | `base_t` | `valley_t` |
  |---|---|---|
  | pasture | 550 | 450 |
  | forest | 400 | 400 |
  | marsh | 350 | 450 |
  | bog | 50 | 150 |
  | mountain | 100 | 100 |

- **The valley score `V`** (0–1000) comes from the elevation alone (owner: valleys "from the elevation data", not drawn rivers):
  - fill the depressions (priority-flood);
  - drain each cell to its steepest-descent neighbour (D8), and accumulate the upstream area;
  - score 0 at ≤ 2 km², rising on a log scale to 1000 at ≥ 500 km²;
  - spread to cells within 2 cells and ≤ 20 m above the channel, losing 25% per ring.
- **The slope factor:** 1000 at ≤ 3% ground slope, falling to 0 at 20% (FAO agro-ecological zoning rates suitability by slope class).
- **The climate factor** = warmth × dryness × wetness:
  - **warmth:** 0 at ≤ 600 growing degree-days (above 5 °C), rising to 1000 at ≥ 1,400;
  - **dryness:** 0 at ≤ 250 mm a year, rising to 1000 at ≥ 500 mm;
  - **wetness:** 1000 at ≤ 900 mm, falling to 400 at ≥ 2,000 mm. Waterlogging limits tillage, but wet land still grazes.
- **Glacier cells** have fertility 0 (§12.4).
- **"Usable"** means fertility > 0.

### 12.2 Climate (computed from geography)

**Firm.** The owner chose to build climate now and to compute it ("computing it would be the right move"). It replaces the earlier
latitude ceiling. Weather, the year-to-year variation, comes later.

- **World tables by absolute latitude** (data):

  | Latitude | Sea-level mean °C | Half annual range, ocean | Half annual range, deep continent | Zonal rainfall (mm) |
  |---|---|---|---|---|
  | 0° | 27 | 1 | 1 | 2,000 |
  | 15° | 26 | 1.5 | 4 | 1,000 |
  | 30° | 20 | 2.5 | 10 | 300 |
  | 45° | 11 | 3.5 | 14 | 700 |
  | 55° | 5 | 4 | 17 | 900 |
  | 60° | 2 | 5 | 19 | 900 |
  | 70° | −8 | 7 | 22 | 500 |
  | 90° | −25 | 12 | 25 | 200 |

- **Prevailing wind by band:**

  | Band | Wind |
  |---|---|
  | 0–30° | easterly trades |
  | 30–60° | westerlies |
  | 60–90° | polar easterlies |

- **Per map:** the ocean temperature effect of its coasts, in °C. Ireland's North Atlantic is +4.2; a cold current is negative.
  - Western Europe's mildness comes mainly from westerly winds carrying ocean air, more than from the current's own heat (Seager et al.
    2002), so the wind is the main mechanism and the coast's value an adjustment.
- **Per cell:**
  - **maritime influence** `exp(−d ÷ 1,200 km)`, where `d` is the distance over land along the wind since the sea;
  - **mean temperature** = the zonal mean + ocean effect × maritime influence − 6.5 °C per km of height;
  - **the half range** moves from the ocean value to the continental value as maritime influence falls;
  - **months** follow a cosine, warmest in mid-July in the north and mid-January in the south;
  - **growing degree-days** = the sum of max(0, monthly mean − 5 °C) × 30.4;
  - **rainfall:**
    - moisture walks along the wind from the sea (recovering over water);
    - each land cell rains `zonal × moisture × (1.2 + min(1.6, 0.02 × uplift in m/km) + 1.5 × height in km)`;
    - the moisture is depleted by 0.0015 × rain × km, which makes rain shadows;
    - it is smoothed with a 7 × 7 box mean (edge replicated), exactly as the signed prototype (spec `m9-s9a-r3`).
- **The Ireland check (prototype):**

  | Station | Model | Approximate real |
  |---|---|---|
  | Valentia | 10.9 °C, 1,440 mm | 10.9 °C, ~1,550 mm |
  | Dublin | 9.4 °C, 700 mm | ~730 mm |
  | Shannon | 975 mm | ~980 mm |
  | Kilkenny | 840 mm | ~850 mm |

  The western mountains reach 2,000+ mm.
- **The generator** computes climate and fertility after the terrain edits. It writes `fertility.png` and the climate layers, reproduced
  byte for byte (slice 9a). The constants live in `edits.toml` `[settings.climate]` and `[settings.fertility]`.

### 12.3 The default map: fertility gradient and hillside dither

**Firm (owner choices on the mock-ups).**

- **Fertility is a darkness gradient:** "full color is 100% usable land, and the dark colors i have been using are the 0%".
  - **Four shades per terrain, two of them in between** (Firm, owner 2026-11-09, `owner-direction-map-look-r5.md`): `tone[k]`, from
    `dark` (k = 0) to full colour (k = 3) in thirds. With `level = ⌊256 × (fertility ÷ 1000)^2.5⌋` and `v = 3 × level`, a pixel takes
    shade `⌊v / 256⌋`, one higher when `blue16[wy mod 16][wx mod 16] < v mod 256`. So a dot steps only between neighbouring shades.
  - **Why (true scale):** the first relief mock-ups were drawn 8 times finer than the game draws (1 screen pixel per map pixel, against
    the game's 8 × 8, §3.5), so in the app a pasture of bright and dark dots read as "points instead of a surface". At true scale
    (`mockups/look-r5/`) the owner kept one dot per map pixel ("A looks the best for close up and for the mountains and woods, its just
    the pasture yellow that is too much like dots") and chose two in-between tones (F). With no in-between tones the rule is the earlier
    bright/dark one.
  - The curve is γ 2.5 ("gamma looks best at 2.5").
  - `blue16` is a fixed 16 × 16 void-and-cluster blue-noise ranking (Ulichney), committed as data and pinned to the world. It replaced
    the 4×4 ordered pattern, which formed a "plaid" ("that blue noise is a huge improvement").
- **Hillsides are a dither pattern from slope, not absolute height:** "i want hilsides to have a dither pattern, not absolute
  elevation".
  - **The light** comes from azimuth 337.5° (north-north-west), stored as the integer unit vector `[-3827, -9239]` (x east, y south, ×10⁴). The owner accepted the cartographic convention: light from below
    makes relief read inverted, and 337.5° inverts least (Biland and Çöltekin 2017).
  - **Per cell,** `d` is the rise away from the light, from the elevation gradient over ground distance.
  - **The soft curve** `shade = d ÷ (|d| + d₀)` has d₀ = 1/80, one value for every zoom step, in data ("i like soft 80x the most"). Grid A holds one strength
    per cell, so per-step d₀ is deferred until the owner asks to tune it. The strength is computed in integers (spec `m9-s10a-r2`). It is half strength
    at a 1.25% slope and never saturates, so steep ground keeps a dither gradient.
  - **The dither:** with `strength = ⌊16 × |shade|⌋`, a pixel takes the relief tone when `bayer4ᴮ[wy mod 4][wx mod 4] < strength`.
    `bayer4ᴮ` is the 4×4 ordered matrix, rotated and mirrored so it does not align with the blue noise.
  - **The relief tones:**
    - slopes facing away from the light take the shadow shade `shadow[k]`, from `dark` to the terrain's `second` tone in thirds (not at
      shade 0);
    - slopes facing the light take the lit shade `lit[k]`, from `dark_lit` to full colour in thirds (not at shade 3), with `dark_lit =
      dark + 2 × (dark_second − dark)`, as data ("2x looks the best");
    - so at full colour the shadow is `second`, and at `dark` the lit tone is `dark_lit`, as before.
- **Rivers, the coast outline, places and houses** draw over the land as before.
- **Removed:**
  - the usable/unusable split;
  - the old height-band relief (`relief_low`, `relief_high`, `mix`);
  - the height-based snow caps;
  - slice 9a's interim `display_usable_pm`.

### 12.4 Glaciation

**Firm.** "snow caps are not elevation based but climate based, this should put glaciers at sealevel at the poles, so its more than
just snow caps, but glaciation".

- **A cell is glacier** when its summer temperature (the warmest three months) is at or below the equilibrium-line temperature for its
  annual precipitation. That uses Ohmura's relation (Ohmura, Kasser and Funk 1992; Ohmura and Boettcher 2018),
  `P = 966 + 230 T + 5.87 T²` (P in mm, T in °C), as cited in the EGU 2020 cirque-glacier study.
  - About −3 °C at 300 mm, 0 °C at 1,000 mm and +7 °C at 3,000 mm.
  - The paper uses free-air June–August temperature; the model uses the warmest three months of the surface climate.
- **What it gives:**
  - wet maritime mountains hold glaciers in milder summers, and dry polar land needs colder ones;
  - at the poles the ice reaches sea level, consistent with Köppen's ice-cap climate (warmest month below 0 °C);
  - Ireland has none.
- **Glaciers** draw in the ice colour `#e8e9ed`, with its own second tone and the hillside dither, whatever the terrain beneath. Their
  fertility and building land are 0.
- **Seasonal snow** belongs to weather, later.

### 12.5 The elevation map mode

**Firm.** "a seperate elevation mode that uses black to white to represent actual elevation", with a square-root curve.

- **Land:** `grey = ⌊255 × √(max(0, elevation) ÷ 8,848)⌋`, the same fixed scale on every map. Ireland's highest ground is about grey 79.
- **Sea and lake** keep their colours. There is no dither.
- **Rivers and places** draw over it.
- It joins the stockpile and yield modes (`docs/economy-design.md` §3.22).

### 12.6 Data and packing

- **Files:**
  - `fertility.png`: 8-bit grey, the value × 4 = thousandths;
  - `climate_temperature.png`: 16-bit, °C × 10 + 1000;
  - `climate_gdd.png`: 16-bit;
  - `climate_rainfall.png`: 16-bit, mm;
  - `climate_glacier.png`: 1 bit.

  They replace `usable.png`, and `map.toml` `[layers]` names them.
- **Grid A:**
  - R is the terrain index;
  - G is fertility ÷ 4 (0–250), with 255 meaning glacier;
  - B is the signed hillside strength (−15…+15, offset by 128; `⌊16 × |shade|⌋` never reaches 16), precomputed per cell at load.
- **Grid B** keeps the elevation.
- **The reference colour function** in `ck-map` implements §12.3–§12.5, and native Metal matches it. Readback fixtures on Metal compare
  byte for byte.
- **Vulkan is compile-only for now** (Firm, owner 2026-11-09, `owner-direction-vulkan-r1.md`): until a native Linux machine with a real
  GPU is in the test loop, new render features land on native Metal only. A change to grid A's packing or another shared format updates
  the Vulkan code just enough to compile. The Vulkan port, with its own readback fixtures, is economy-design slice 11, parked until the
  owner says so.
- **Slices** (economy-design §4):
  - **9a:** the data and loading (CK-234, accepted);
  - **9:** the economy's use (CK-233, accepted);
  - **10a:** the default look (§12.3–§12.4), on native Metal (CK-235, accepted; the owner approved it in the running game);
  - **10b:** the elevation mode (§12.5), on native Metal (CK-236, accepted);
  - **11:** the Vulkan catch-up (CK-239, parked).

### 12.7 Sources

- **Agriculture and soils:**
  - [FAO/IIASA, GAEZ 2007](https://www.fao.org/docrep/pdf/010/a1075e/a1075e01.pdf);
  - [FAO Soils Portal: histosols](https://www.fao.org/soils-portal/soil-management/management-of-some-problem-soils/highly-organic-soils/en/);
  - [topographic wetness index, HESS 2014](https://hess.copernicus.org/articles/18/3623/2014/);
  - [Geographical: floodplains](https://geographical.co.uk/science-environment/why-do-we-live-on-floodplains).
- **Climate:**
  - [Seager et al. 2002](https://ocp.ldeo.columbia.edu/res/div/ocp/gs/pubs/Seager_etal_QJ_2002.pdf);
  - [Yamamoto et al. 2015](https://www.aoml.noaa.gov/ftp/phod/sklee/projects/amoc_postdocs/yamamoto_elal_2015_cd.pdf);
  - [Britannica: lapse rate](https://www.britannica.com/science/lapse-rate);
  - [Humboldt Current](https://en.wikipedia.org/wiki/Humboldt_Current);
  - [growing degree-day](https://en.wikipedia.org/wiki/Growing_degree-day);
  - [Rothamsted 2022 on excess water](https://repository.rothamsted.ac.uk/download/498a0c0c494497e0a746ace3bd0ab2cd32c6c4cf834b9a66291ea4cf3da70885/1419875/1-s2.0-S0308521X22001366-main.pdf);
  - [Climate of Ireland](https://en.wikipedia.org/wiki/Climate_of_Ireland), used as the check region.
- **Relief and glaciers:**
  - [Çöltekin et al. 2018](https://isprs-archives.copernicus.org/articles/XLII-4/101/2018/);
  - [Biland and Çöltekin 2017](https://intranet.inegi.org.mx/calidad/doc/2019/Indicadores_CyE/Guia_didactica/2017_Biland-NNW_CAGIS.pdf);
  - [EGU 2020, palaeo cirque glacier ELAs](https://presentations.copernicus.org/EGU2020/EGU2020-16981_presentation.pdf);
  - [Ohmura, Kasser and Funk 1992](https://www.cambridge.org/core/services/aop-cambridge-core/content/view/8DCF1C24E32A6781CC365853535CD16F/S0022143000002276a.pdf/climate-at-the-equilibrium-line-of-glaciers.pdf);
  - [Köppen climate classification](https://en.wikipedia.org/wiki/K%C3%B6ppen_climate_classification).

## Provenance

Drafted 2026-11-02 by ck-designer-1. Revision 2 the same day, after design review 1; revision 3 (editorial) after both re-reviews approved
it; revision 4 (editorial) aligns §9.4 rule 2 and §9.7 with the reviewed specs CK-178 and CK-183. Sources: the session log, both design reviews and the
reviewer-1 owner addendum, `docs/vision.md`, `docs/game/diplomacy.md`, `docs/scene-design.md` and `docs/render-design.md`, and reading of
`crates/gpu-metal` (the frame encoder and its off-screen path, the ledger), `crates/gpu-vulkan` (the render pass, readback and ledger),
`crates/render-2d` (the points-to-pixels rule), `crates/input` and `apps/codekarl`. No code, issues, builds or runs.

Revision 5, 2026-11-09, by ck-designer-1: §12 (land, climate and the look since M9), from the owner's signed records
(`usable-land-proposal-r4.md`, `map-look-design-r4.md`), with pointers at the passages it supersedes.

Revision 6, 2026-11-09, by ck-designer-1: §12.3's four shades per terrain, from the owner's choice at true scale
(`owner-direction-map-look-r5.md`, `mockups/look-r5/`).
