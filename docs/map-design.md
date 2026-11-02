# Map design: Ireland, the look and the data

**Status: owner interview in progress, 2026-11-02.** Written by ck-designer-1 (Class S, `claude-opus-5-5`, high) from the map design session
with the owner. The owner's answers are recorded verbatim, with timestamps, in `.pipeline/coordination/map-design/session-log.md`. This page
records what is settled. It is not yet a technical design: storage, tiles, GPU layout and the milestone come in later sections, after the
owner has accepted the look and the data rules below.

**Firm** marks owner decisions, with their date. **Proposal** marks designer recommendations. **Open** marks decisions that still need an
answer. A Firm item that the owner chose by reacting to a mock-up says so.

This extends the "Map" section of `docs/vision.md`. It builds on `docs/scene-design.md` (live textures, reusable handles, bindless drawing,
tens of thousands of quads), which deliberately left the map representation open (its §14).

## 1. Owner direction

**Firm — owner, 2026-11-02, opening the session:**

> ok start the map design session for me, the first region is going to be ireland, the style of the map is going to be solid colors overlayed
> across a dark background, i have a unique vision in my head for how i want it to look.

**Firm — owner, 2026-11-02 12:47Z, on references:**

> dark black background like a arcade game, flat colors with two-tone dithering that contrast with that. low color count, minimalist, like a
> colored version of 'return of the obra dinn', but not as complicated as we wont be drawing this level of detail yet (although later we will
> when i implement story popups).

The owner asked for criticism and design help, and asked to see mock-ups before more design. Two interactive mock-ups were made on real
Irish elevation and coastline (§8). The owner's verdict on the second: "that looks good".

## 2. Region and extent

- **Firm (2026-11-02).** The first region is Ireland with its associated offshore islands. Owner: "we have settled on ireland as the pilot
  nation, since its an island in a relatively normal situation that doesnt have much outside contact but is still large enough to have some
  politics going on internally."
- **Firm (13:55Z).** Everything outside Ireland, including the nearby British coast, is hidden for now: "for now, just hide it, we are doing
  ireland only along with its associated offshore islands."

## 3. The look (default geographic map)

### 3.1 Ground and terrain colours

- **Firm.** The sea is a cool black. Land without a usable terrain is a warm black. Most of the screen is dark: "i want most of the screen
  to be dark typically".
- **Firm.** All land has a terrain type, and each type has a colour. Where the land is usable (§5.2), the terrain is drawn in its solid
  colour. Where it is not, it is drawn "like its greater type": a near-black tinted toward the type's colour. Owner: "the subtle hue is what
  i was thinking in general ... i would want the usable terrain to be the solid color, but all land shold have a color, unusable land should
  be like its greater type".
- **Firm.** Colours named so far: forest green; pasture brown-yellow; mountain grey with white caps; bog dark green; marsh dark brown. "there
  are lots of other terrain types we could do."
- **Firm (mock-up 2).** Mountains show white snow caps only where the mountain is usable, and a light grey on unusable high ground.
- **Firm.** A low colour count. Every colour comes from data, so it can be changed easily: "in general all the colors should be from data".
- **Firm (deferred).** Colour-blind options come later; for now the colours follow the owner's vision. (The designer had pointed out that
  forest green next to pasture brown-yellow is a red-green confusion pair.)
- **Firm (13:43Z, on mock-up 1).** The mock-up 1 colours were "close, perhaps a little too bright for some of the greens and yellows";
  mock-up 2 toned them down. Its palette is the starting point in §8.

### 3.2 Edges

- **Firm.** Edges are hard pixel steps: "it needs to look like a 2d pixel art game."
- **Firm (13:43Z).** Only the coast has an outline, drawn solid in the colour of the land behind it. Terrain areas inland are not outlined.
  (Mock-up 1 outlined every terrain area as well; the owner agreed it was too busy.)

### 3.3 Dithering

- **Firm.** Dithering is two-tone and marks features of the world, such as elevation; it is not a camera effect. It is pinned to the
  world, so it does not crawl when the view pans.
- **Proposal (as in the mock-ups).** Each terrain has a pair of colours. A continuous value (elevation within that terrain's range) sets
  how much of the second tone shows, through an ordered 4×4 Bayer pattern indexed by world pixel. The sea uses a lighter cool black to show
  the shallow shelf. Unusable land dithers between two near-blacks of its type, so hills still read in the dark.

### 3.4 Rivers

- **Firm (13:43Z).** Rivers have their own blue, drawn over everything, at every zoom.

### 3.5 Pixels on screen

- **Firm (13:43Z).** Each map pixel is 4 screen pixels square: "i think 4 looks the best."

## 4. Zoom

- **Firm.** Zoom moves in steps, so that pixels stay whole. There are enough steps to feel smooth, and stepping is fast: "yes, but it
  needs to be fast and natural, with lots of steps".
- **Firm.** The pixels change as the player zooms. Each step re-draws the world at a finer or coarser resolution from the map data; this is
  why the map does not use textures. Owner: "it would look natural to see the pixels kind of change as we zoom in, that's why i didnt want to
  use textures."
- **Firm.** Fully zoomed out, the map is a geographic picture only. Closer in, places appear as markers, and closer still they become
  houses. Owner, 13:55Z: "i like how you made the cities as markers until we have zoomed in enough, wherein they turn into houses, this looks
  good, but the roads need work, we can handle all of this during implementation."
- **Proposal (as in the mock-ups).** The on-screen pixel size stays fixed. Each step changes how much ground one pixel covers, 10 steps per
  doubling, from 3 km down to about 12 m per pixel (76 steps). Coast and terrain edges gain generated detail below the data resolution.

## 5. Map data

### 5.1 Authoring

- **Firm (13:55Z).** All map data, including usable land, is hand-authored in data files so that modders can change it.
- **Firm (13:55Z).** Elevation may start from the AWS terrain data used in the mock-ups, with guesswork to make the land closer to medieval
  times: "we can use the aws terrain data but we will have to do some guesswork to make it more accurate to medieval times."
- **Open.** The sources for the medieval terrain layer (forest, bog, marsh, pasture), and how the guesswork is recorded so later research
  can correct it. Present-day land cover is a poor guide: medieval Ireland had far more forest and bog.

### 5.2 Usable land

- **Firm.** Land is usable when it is open to development or resource extraction. Land that is not is the dark "wasteland": "it would really
  just by non-arible land that holds no resources."
- **Firm (13:55Z).** Usable land is part of the authored data, not computed at run time.
- **Firm (13:43Z, on mock-up 1).** The share of dark land in the mock-ups is "fine for now" (about a third of pasture usable, less for the
  other terrains).

### 5.3 Resolution and what is placed

- **Firm (13:55Z).** A 500 m data grid is acceptable. The aim is "as detailed as we can", balancing realism against computational
  pragmatism, which may need testing later.
- **Firm (13:55Z).** Villages and castles must be placed. Individual farms are wanted too, by plausible guesswork rather than a record of every
  historical farm.
- **Firm (13:55Z).** Fields and woods are not simulation objects unless a location exploits them. They are implied by a farmhouse's position
  and the terrain around it. Clicking a farm location could highlight the map area it draws its resources from.
- **Open.** How a location's exploited area is defined and stored (a radius, authored cells, or computed from terrain), and whether the map
  draws exploited land differently from unexploited usable land.

## 6. Map modes

- **Firm.** The geographic map described above is the default. Dozens of map modes are planned, each changing the appearance of the same
  rendered land.
- **Firm.** The political map shows all land, including wasteland, in one colour per country, with no terrain types. For now each holder
  has its own colour regardless of overlord, so a baron differs from his liege.
- **Open.** Whether the political map carries a faint dither hinting at the terrain underneath. The owner floated it ("we could implement
  dithering in this mode to give a hint as to the terrain underneath") but expects the player to switch back to the geographic map for
  terrain. Mock-up 2 has it as a button.
- **Proposal.** Every map mode is a colouring rule over the same world data, with its palette in data, so a new mode does not need a new
  renderer.

## 7. Open questions carried forward

- Roads: how they are drawn, and whether they are authored or generated (the mock-up's generated tracks "need work").
- Detail at close zoom inside large usable areas, which currently reads flat.
- The full list of terrain types and their colours.
- Lakes: whether they use the sea's cool black or their own colour.
- How the map shows unmapped land (the vision's fog of war for land without maps).
- The technical design (§9).

## 8. Mock-ups

Two interactive mock-ups were published to the owner as a private page; mock-up 2 is the current one. Real data: coast, elevation and sea
depth from AWS Terrain Tiles (terrarium encoding, zoom level 8, about 360 m per sample in Ireland, resampled to a 500 m grid); the larger
loughs and a few rivers from Natural Earth 1:10m. Invented: terrain placement, usable land, polities, tracks and houses. The generator,
page template and a saved copy of mock-up 2 are in `.pipeline/coordination/map-design/mockup/`.

**Proposal — mock-up 2 palette, the starting point for the data files:**

| Terrain | Solid (usable) | Second tone | Unusable | Unusable, second tone | Other |
|---|---|---|---|---|---|
| Pasture | `#a08944` | `#806c35` | `#15110a` | `#211a0f` | |
| Forest | `#367534` | `#285a27` | `#0a120a` | `#122012` | |
| Bog | `#2c4c2d` | `#203a22` | `#090f0a` | `#101a11` | |
| Marsh | `#5c3e25` | `#462e1b` | `#110b07` | `#1c130c` | |
| Mountain | `#7f8187` | `#606268` | `#101114` | `#1b1c20` | caps `#e8e9ed`, unusable high ground `#3c3e44` |

Sea `#05080e`, shallow sea `#0b1422`, lakes `#0b1422`, rivers `#3a78c2`.

## 9. Next steps

1. The owner reviews this page.
2. ck-designer-1 proposes the technical design here: the data file formats, how the GPU draws each zoom step from them, and how places and
   houses use the M5 scene. It goes to independent design review, as earlier designs did.
3. A milestone proposal follows once the owner is satisfied. ck-manager files it after the owner approves.

## Provenance

Drafted 2026-11-02 by ck-designer-1 from the session log, `docs/vision.md`, `docs/scene-design.md` and `docs/render-design.md`. No code,
issues or builds.
