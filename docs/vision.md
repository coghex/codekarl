# Codekarl vision (provisional: interview in progress)

> **Status: an ongoing owner interview. Not design-ready, not a backlog, and not a specification.** This page records what the owner has
> said so far (2026-10-17) and will change as the interview continues. Each item is labelled:
> - **Firm**: direction the owner has stated;
> - *Example*: an illustration of desired behaviour, not a commitment to a number, mechanism or historical claim;
> - **Open**: not yet decided.
>
> Nothing here settles an open decision, and nothing here is an issue or an implementation plan. Historical statements are the owner's
> examples and need research before they become game content.

## What the project is

- **Firm.** Codekarl is an ambitious side-project game, and a reusable collection of Rust/Metal packages for future Rust/Metal projects.
  It is a significant project in a new language and environment, by an experienced game developer. It is not a learning exercise.
- **Firm.** The game is a 2D pixel-art economic grand-strategy game inspired chiefly by Victoria 2, with novel mechanics of the owner's own.
  It is UI-heavy: many windows, panels, menus and UI elements.
- **Firm.** The experience centres on the economic simulation (exploiting resources, managing populations, trade), mixed with mild strategic
  land and naval warfare (moving armies and fleets on the map).
- **Firm.** Typical player activity: watching the market and the pops, queueing research or building, recruiting units, balancing a budget,
  and trading or making diplomatic deals with AI-driven nations. (Research: see "Research and technology".)
- **Firm (goal).** Responsiveness. UI code is kept truly separate from world rendering, and the game stays 2D, with the aim of avoiding
  the stutter and unresponsiveness the owner sees in the heavy 3D, script-laden engines of the games that inspire it. This is a design
  goal, not a claim that these choices alone remove script overhead or stutter.

## Player, tags and territory

- **Firm.** The player represents a crown or country, called a **tag**. The player handles laws, the budget, diplomacy, warfare, property, state
  functions and political goals.
- **Firm.** A tag has *de jure* claims and *de facto* territory and control.
- **Firm.** Major towns and cities are simulated individually. Every building belongs to a place on the map, even a single building in the
  countryside. Provinces or states may exist as a visual aid but are **not** assumed to be the fundamental simulation units.
- **Firm.** A full character system is desired, for example rulers and leaders of other nations who can be bribed or appear in events.
  See "Characters" for the owner's 2026-10-18 answers.

## Time

- **Firm.** The logical game tick is hourly. The economy and other systems run at coarser cadences, such as daily. The hourly resolution
  exists for more detailed combat.
- **Firm (owner, 2026-10-18 20:21Z: "yes").** In single-player, the player can pause freely, issue orders while paused and choose
  simulation speeds (as ck-guide's 20:18Z question put it).
- **Firm (owner, 20:21Z).** In multiplayer, time controls are "left to server settings on game creation", rather than one fixed rule such
  as everyone pausing or the host setting the speed.
- **Firm (owner, 20:21Z: "that would be the foundation"). Reproducible saves.** Loading the same save and repeating the same actions should
  produce the same outcomes, including random events and battles (ck-guide's 20:18Z recommendation, adopted). The owner added: "at some
  point i may make decisions that change this".
- **Open.** Exact game speeds, system ordering within a tick, which server settings exist, and how reproducibility is achieved.

## Setting and scope

- **Firm (aspiration).** A long horizon, from an early-medieval start towards the modern era, supported for as long as is feasible.
- **Open.** The start date. Two options have been raised: the coronation of Charlemagne (*example* date 9 October 768), or a later start around
  the early 1400s, as EU4 does. The owner considers this unimportant for the vision.
- **Open.** The first playable region. Ireland or Brittany has been suggested, to get the mechanics right before expanding to a global map.

## Game design (moved to docs/game/)

The detailed game design is part of this vision and keeps its Firm, Open and Proposal markings, but lives in its own documents so that a reader working on one system does not load them all. The sections were moved verbatim on 2026-10-21; nothing in them was reworded.

- [Economy and populations](game/economy.md)
- [Crown, military, church and history](game/crown-military-church.md)
- [Ships and maritime trade](game/maritime.md)
- [Research and technology](game/research.md)
- [Diplomacy, war and peace](game/diplomacy.md)

## Player information (owner, 2026-10-18 20:15Z and 20:16Z)

Answers to ck-guide's two questions of 20:11Z (at 20:15Z) and its two of 20:16Z (at 20:16:33Z).

- **Firm (owner: "ideally yes"). Generous economic information.** The player should get a much more detailed view than would be
  realistically possible for a real country's leader. (ck-guide's question offered inspecting every building, pop stockpile, market price
  and production figure worldwide, as against exact information only within the player's own country.)
- **Firm. Two different fogs of war:** one for land that has no maps, and one for naval and land units.
  - **Firm (owner, 20:16Z: "yes to all of that").** As ck-guide put it at 20:16Z, geography becomes known through the country's own
    expeditions, merchant voyages and sharing maps through treaties, and once mapped it remains known permanently, even without current
    military visibility there.
  - **Firm (owner, 20:16Z: "yes that is all right too").** Once an enemy army or fleet is visible, the player sees its exact composition,
    troop numbers, morale and supplies (ck-guide's 20:16Z preference, adopted); the uncertainty is mainly about where unseen forces are.
- Character skills and traits are visible (see "Characters").
- **Open:** exactly which economic information is shown, and how it relates to unmapped land; how units are spotted, from what and at what
  range; and what is shown for units that have been seen but are no longer visible. Map design itself remains reserved (see "Map").

## Missions

- **Firm.** Missions are EU4-style mission groups, assigned to categories of tags by culture and political circumstance. They offer
  historically inspired goals, rewards, unlocks further down the tree, and flavour events that name the actual in-game characters.
- **Firm.** Missions use strict, specific conditions. Meeting them makes a mission claimable, but the player chooses when to claim it.
  Conditions evaluated at claim time decide the effect, and the tooltip explains them, so the player can prepare first.
  - *Example:* an Anglo-Saxon kingdom (such as Mercia or East Anglia) that defeats a Norse country in the same region after a certain
    date can claim "form England". Only Anglo-Saxon kingdoms whose opinion of England meets a threshold at claim time are absorbed, so the
    player may first improve relations with the reluctant kings.
- **Firm.** Mission outcomes offer distinct, attractive, roughly balanced choices.
  - *Example:* "unite the Anglo-Saxon crowns", or "a display of dominance", which grants claims (casus belli) over all of England at a cost
    in lives and money, in exchange for force projection, legitimacy or political power. The historical leader in such text (Æthelstan) is a
    placeholder for whichever character the game produced.
- **Firm.** The AI has preset preferences for mission choices, so that history tends to follow its course when conditions allow, for example
  England or Germany eventually forming. These are preferences, not unconditional forced outcomes.

## Map (idea recorded; design reserved)

- **Firm.** Map design is reserved for a separate, fresh-context discussion with the owner. This page records the idea only.
- The idea: live-rendered pixels with sprites on top, and a moddable data format that captures terrain and resources. Zooming in reveals detail
  as new pixels come into place, with landscape cues from procedural dithering, colour and other features. The owner describes this as fast,
  live graphics memory updated as a function of zoom and the map data.
- **Open (deliberately).** Pixel storage, tiles, algorithms, rendering interfaces, geographic resolution and GPU layout.

## Engineering direction

- **Firm.** The first epic is the **runtime**. Its first observable output is proper logging to stdout; no window or rendering is needed for
  that milestone.
  - **Firm (owner, 2026-10-18 20:23Z). Two milestones.** Answering ck-guide's 20:21Z questions:
    1. The simple start: load configuration, initialise, log, then shut down cleanly. (ck-guide had also offered a small headless
       simulation loop; the owner chose the simple start.)
    2. "A lua vm would be the next step": the same lifecycle as the first milestone, "but using the full lua setup instead of just the
       logger".
  - **Open:** logging expectations (for example levels, categories, filtering, colour and file output) and what the "full lua setup" must
    demonstrate; ck-guide asked both at 20:24Z. The design is being drafted by ck-manager for independent review; no implementation is
    assigned.
- **Firm.** The project builds its own components. Small, well-established Rust libraries are acceptable. Nothing as large as a windowing
  library is adopted. A native windowing package is built in-house, reusable in other situations, and able to sit alongside a generic
  GLFW/Vulkan setup in the future. **Open:** whether the two are developed at the same time, which the owner has floated.
- **Firm.** Lua VM and scripting support come early, for the scripted political and game simulation. **Open:** what Lua is responsible for.
- **Firm (ambition).** Multiplayer and modding. **Open:** timing, trust model and architecture.
  - **Firm (owner, 2026-10-18 20:18Z: "yes to all").** Multiplayer covers, as ck-guide's 20:17Z question put it: each player controlling a
    different tag in the same world; several players cooperating on one country; and players joining an ongoing campaign. **Open:** how
    shared control, pausing, disconnects and saves work.
  - **Firm (owner, 20:18Z). As much as possible is moddable,** given reasonable performance considerations. (ck-guide's question listed map
    data, goods, cultures, religions, missions, laws and scripted behaviour as examples.) **Open:** what stays fixed in Rust for
    performance, and how mods are loaded and distributed.
- **Open (first-epic decisions):** the first package splits, for example whether logging is its own package, and whether a resource manager
  is needed now. The project's standing rule still applies: no window, ECS, renderer abstraction or map representation before its design is
  discussed with the owner.

## Relationship to Hetoimasia

Hetoimasia (`~/work/hetoimasia`, Haskell/Vulkan) is the owner's main project, for a different game. It was cited only as a reference for its
Lua scripting engine. Codekarl does not inherit its game vision, policies, isolation requirements, Haskell abstractions or engine
architecture. Ideas from it may be proposed, but not copied by default.

## Still to explore

- Construction labour and materials, paused and cancelled construction; conversion duration and costs; the shortage-penalty formula,
  occupied levels and the economy-of-scale bonus; land capacity; worker recruitment.
- Markets and trade: whether the per-location order-book model is feasible, and how it works (price function, merchant radius, transport).
- Which goods are stockpiled and which are abstracted; coin value over time; taxation form; reserves, lending, bonds, credit ratings and banks.
- Pops: merge keys, splits and ownership when people move, satisfaction, birth and death, cultural values (see "Populations").
- Research: penalty and bonus curves, facility sale and completion outcomes, input shortages (see "Research and technology").
- Battles and garrisons: several garrisons and control, sieges, prisoner actions, morale (see "Battles, garrisons and control").
- Levies: dividing losses on disbanding, mobilisation timing, combat losses and reinforcement (see "Levies, equipment and army
  supply").
- Characters: skills, experience, offices and appointment laws, elections (see "Characters").
- Diplomacy: casus belli sources, peace terms, treaty terms and enforcement, changing tag cultures, assimilation, religious spread (see
  "Diplomacy, war and peace").
- Ships: movement, wartime use of private ships, privateering permission, selective blockades, rentals (see "Ships and maritime trade").
- Political institutions; tag and character identity.
- The first regional scenario and its start date.
- Player controls, game speed and persistence; map discovery and unit visibility (see "Player information").
- Lua's responsibilities.
- Multiplayer: shared control, disconnects and saves; server time-control settings (see "Time").
- The runtime and logging contract.

Map design is discussed separately.

## Provenance

Owner messages of 2026-10-17 (16:43–23:56Z) and 2026-10-18 (00:15Z–20:23Z) in the ck-guide conversation `01a14231…`, summarised in ck-guide's brief
(`.pipeline/coordination/vision-interview/owner-brief.md`). Captured by ck-manager.
