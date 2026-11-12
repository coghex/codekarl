# The first economy slice: design (milestone 9)

**Status: accepted design, 2026-11-08.** The owner signed off r19 as the milestone 9 model ("i sign off"). ck-reviewer-1
approved its engineering (`economy-design-review-r19.md`, ee101993). A fresh viability, feasibility and fun review
(`design-fun-review-r1.md`, by ck-designer-4) was worked through with the owner in r17–r19. The design was written by ck-designer-1
(Class S, `claude-opus-5-5`, high).

- **Implemented:** every slice of §4 except slice 11 is accepted (CK-224 to CK-254, 2026-11-10); slice 11, the Vulkan catch-up, is
  parked. That includes:
  - the follow-up slices 4d, the storehouse fee in step 5 (CK-248); 4e, staffing shared across methods (CK-251); 3b, store capacity
    counting workers in the view (CK-253); and 8b, farmland quality on the yield map (CK-254);
  - the milestone 9 review notes (CK-246, CK-247 and CK-249) and ck-guide's findings: the location panel's monthly counters (CK-250)
    and the view's field room, which charged sea-method levels as land (CK-252);
  - the owner's UI feedback after running the game (2026-11-10, `owner-feedback-ui-r1.md`), slices 12a to 12f: the map-mode bar at the
    bottom centre (CK-255), the UI scale following the window (CK-256), the resizable 1280×800-pixel window (CK-257), the ledger (CK-258),
    pops and buildings as cards or rows (CK-259) and superscript glyphs (CK-260), with their owner records in
    `.pipeline/coordination/m9-design/owner-direction-ui-*.md`, `owner-direction-window-*.md` and `owner-direction-superscript-r1.md`; and a
    test fix for busy machines (CK-261). The UI side is described in `docs/ui-design.md` and the window in `docs/window-design.md`.
- **Later owner decisions** are folded into the sections that they change, with their date and record.
- **Source:** `.pipeline/coordination/m9-design/economy-design-r19.md` (sha256 `69772ff5…`), published unchanged below this header.
- **Revision history:** r1–r19 and their reviews live beside it in `.pipeline/coordination/m9-design/`.
- **Reading the section notes:** the "rN" notes inside the sections record when each rule was settled, and are kept for traceability.
  Where a later revision superseded an earlier rule, the later text is binding.

**Firm (owner, 2026-11-08)** marks the owner's decisions from that session. **Proposal** marks designer detail, **Research** findings
with sources, and **Open** what is still to decide.

## 1. The owner's decisions (Firm, 2026-11-08)

**Pops**
- **Multiple pops from the start,** so that the functionality exists from day one.
- **A pop's identity** is its pop type, culture and religion. Owners and workers of a building are the **same pop**: owners are a count
  inside its employment, and they share the workers' wage. The distinction exists so that entities can own other entities later; at the
  start it will rarely matter.
- **Pop types now:** peasants, monks and labourers.
  - **Monks** are a separate pop type, included now.
  - **Labourers** are hired at a monastery and work its direct holdings. They earn more than subsistence farmers, through the holdings'
    more profitable output, and so live better.
  - **Monks have no children.** Monks die off; as positions open, the monastery hires from the other classes. Monks have a wage even when
    it is not paid, so there is a demand to hire and a reason for anyone, even a noble, to enter the cloth.
  - **Recruits** are drawn from both of the source pop's pools equally, so the source's dependent share does not change.
- **Dependents** are a share of the population, stored per pop.
  - The base is about 40%: a pop of 100 is 60 workers, and it eats for 100.
  - Dependents are the children, the elderly and the injured or sick. They cannot reproduce and are more susceptible to disease.

**Religion and culture**
- **Allocated by research.** Ireland in 768 is Christian and Irish with **some exceptions,** and representing them is a good test of
  the system.
- **Minority pagan pockets** are wanted.
- **The Norse** arrive later, through an event the owner will design with the event system.

**Buildings**
- **The subsistence farm** is an early-medieval peasant farm with **one production method that produces cattle, oats and other goods
  together.** These are not separate oats and cattle farms; those come later. This refines `docs/game/economy.md` ("Subsistence farming"),
  which describes livestock and grain as distinct methods.
- **Owner coverage.** Owners must cover the workers, regardless of the building's maximum:
  - one owner per 10 workers on a farm. A farm with 3 owners can have up to 30 workers, and needs a fourth owner to grow past 30;
  - at a monastery, one priest covers 5 monks or 10 labourers;
  - the ratio may differ by building;
  - when coverage falls short, output and efficiency lose a scaled percentage.
- **Monasteries** produce all the goods, including specialty goods (ale, fruit, herbs) in more limited quantities.
  - Some monasteries have no peasants.
  - Others share their land with peasants, who have a subsistence farm there too.
  - Specific monastic production methods can wait.

**Prices, needs and wages**
- **Prices** were first fixed per good at a base price for the first milestone. **Superseded by the owner's later decision below:**
  local prices from buy and sell orders. Markets hold a large stockpile and employ merchants and labourers, but trade does not need
  them.
- **Needs** come in three tiers: necessities, niceties and luxuries ("water, ale and wine").
  - Each good provides different benefits: a hunger value, and contributions to the tiers. Oats satisfy necessities; cattle satisfy
    necessities and niceties; fruit has a low hunger value but a high luxury value and satisfies all three.
  - Each tier is a value from 0 to 100%, with extra negative or positive effects at certain levels.
  - Each pop class has expectations that its satisfaction is checked against. A peasant expects no niceties or luxuries; a labourer
    expects some niceties and no luxuries.
  - Standard of living is affected by these values, among other things.
- **Pops are paid in goods, not coin.** This is how a farm's food reaches its pops.

**Stores**
- **Every good has its own unit;** oats are counted in bushels.
- **Pop stores** are sized by the pop's people, and are where it eats from. They are large enough to last a while. Undersupply or a
  blight can deplete them, which does not kill at once: it slowly drains the needs values, and the death rate starts rising once they
  fall far enough.
- **Building stores** are larger than a pop's on a normal farm. A subsistence farm's can be low, and a monastery's holds a lot more.
- **The flow:**
  1. wages go into the pops' stores;
  2. overflow goes into the building's store;
  3. when the building is full, the overflow goes to the pops: owner pops first, then the other workers;
  4. what still does not fit is lost. Production keeps going; it is capped at what can be held.

**Buildings, land and fishing (owner, 2026-11-08, answering review r7)**
- **Fish** is added (with seabirds).
- **Locations have no strict rules on which buildings are built there.** The scenario sets each place up however we want.
- **A farm's maximum size** is set by the usable land in the location's radius. On the islands that is 0. (Owner, 2026-11-09: this caps
  the farming levels; the farm's coastal levels have their own limit, §3.4.)
- **A monastery's holdings** are sized by its land in the same way.
- **Island monasteries** have monastery levels but no farm levels. They use a **separate production method for fishing,** which takes up
  less space. When a monastery's holdings are larger than its land allows a farming monastery to be, the farming method is forbidden,
  and it supplies only fish and seabirds. **Refined by the owner during review r9, below:** a monastery fishes only when that is truly all it has (it cannot hold
  one farming level, §3.4), and its fishing includes a garden. **Superseded by the owner on 2026-11-09:** a monastery fishes when that
  is the more profitable method, and an island may have coastal farm levels where the scenario authors them (§3.4, §3.13).
- **Monks:** only Christian pops can be recruited, and every monk pop is Christian. Culture does not matter.
- **Starting stockpiles** are set up at the beginning, and wages are drawn from them until the first production is paid.

**Storage, herds and fishing (owner, 2026-11-08, during review r9)**
- **Goods are stored in real units,** such as bushels, each with a realistic weight and cargo size.
- **Fishing monasteries have a garden** for herbs and vegetables.
- **A monastery switches from farming to fishing only when it is profitable to switch** (owner, 2026-11-09, replacing r9's "only when
  that is truly all it has: no usable land"). Methods are chosen by value, never authored per place, each within its own limit (§3.4).
- **Fishing grounds are not yet designed.** The ocean could provide a resource area with its own fishing radius, and other coastal
  buildings could have other, more profitable coastal methods later.
- **Herds are tracked with the buildings:**
  - a building's herd has a maximum scaled by its levels, and a starting value that is somewhere reasonable;
  - the herd has its own population growth;
  - if pops move, the herd stays with the building;
  - a herd that has lost animals is replenished when there are extra goods to sell, or extra cattle in the stockpile.
- **Live animals and their meat are separate goods.** This lets farms supply cattle to butchers later, and sell cattle to buildings with
  room for more herd.
- **Surplus animals** are butchered automatically, until meat stores are full. Excess meat beyond a full store is destroyed. There is
  no seasonal cull yet.
- **A starving pop** eats its food first. When the food is gone, it butchers its livestock.
- **Dairy** is set by the size of the herd, not by the workers or the size of the building.
- **The herd per level** is based on research, with each head of household (an owner of the subsistence farm) holding the herd. The
  owner approved: 12 cattle, 8 pigs and 7 sheep per level; a start at 75%; seasonal dairy; and grain recalculated so that a level still
  just feeds its people.

**Methods, markets and barter (owner, 2026-11-08, after review r12)**
- **Every building picks its production method by an estimate of the value of its outputs minus wages.** A building with access to
  several methods splits its levels between them, by what is most valuable:
  - a monastery with sea and land would naturally take one level of fishing, because fish and seabirds have the higher base value;
  - for its second level, land goods are worth more than more fish, which already has a supply. And so on: a natural balance.
- **Sizes are limited only by the available resources:** farming levels by the usable land in the radius, and fishing levels by the fish
  available.
- **Unproductive monasteries** come later. For now, all are assumed productive in some way.
- **Market mechanics come in with the goods.** Price can be as simple as this:
  - demand from the local pops gives buy orders;
  - the supply available gives sell orders;
  - the base price is changed by a value scaled by the difference.
- **Exchange:** local barter now (the designer's recommendation, which the owner agreed). Merchants come in the next milestone.
  **Superseded (owner, r17):** barter is deferred to the merchants milestone.

**Eating, wages, payday and barter (owner, 2026-11-08, answering review r13)**
- **Eating:** a pop eats an equal share of all the food available in its stockpile.
- **Zero-hunger goods** (herbs, wool, beeswax) have a **ticking consumption** that benefits the pop's needs while depleting the good.
  **Since (owner, 2026-11-09):** only herbs and beeswax tick, at a rate per pop type. Wool, hides and linen are consumed by a later
  clothing need (§3.7 step 6).
- **Diet shares:**
  - the subsistence farm keeps one production method, but the diet share sets how much of each good it produces;
  - the diet share sets the peasant class's (each class's) demand values;
  - the start-up heuristic uses them.
- **Value after labour and inputs:** a method's output must have the cost of labour subtracted. A farm producing 1 oats yields only 1
  minus what is paid to the workers. Comparing methods compares values after the workers are paid, even as an estimate, because that is
  the true value of the farm. Inputs must be subtracted too.
  - **Inputs** are not yet designed. They are an empty list by design, so that they can be implemented later and their cost is already
    used in the calculations.
- **The overhead share** does not matter much now. It will be defined by building type later; 10% (subsistence farm) and 30% (monastic
  holdings) are fine for now.
- **The farm's stockpile:**
  - peasants always keep what they have, unless they are full; they give nothing to the building;
  - the exception is payday, when the excess goes into the building's stockpile.
- **Barter:** the initiator bears the rounding loss (the designer's recommendation, accepted).
- **Buildings barter only with what is in their storehouse.** Barter happens before any output is registered, so the stockpile always
  shows filled. **Moot since r17,** because barter is deferred.

**Eating and storage limits (owner, 2026-11-08, answering review r14)**
- **Eating is an exact equal share** of what remains, scaled by the pop's stockpile of each good (the designer's option B, chosen by the
  owner). It is not the "most plentiful first" approximation.
- **Storage differs by good:** a pop can store a lot of grain but not much fruit, so it draws on grain more.

**After the fresh review (owner, 2026-11-08)**
- **C1, eating and herds.**
  - Consumption is set by the food a pop holds, not by forecasting. Pops predicting the harvest "would be cool", but it is not a
    requirement.
  - About 80% of needs supplied is the ideal. In famine it can safely fall to 50%, which slows growth a lot and raises deaths only a
    little. Below 50% the real penalties apply. A surplus pushes towards 100%, which gives the most growth.
  - At the minimum, people eat what is left ("people gotta eat"), and then the needs drop precipitously.
  - **Herd animals are rationed:** there is a breeding floor.
- **Seasons for every good:**
  - the grain harvest is in September;
  - fruit can come from August through November;
  - herds produce in winter as in summer, at a lesser rate;
  - fish may have a season too.
- **C2, owners and alms.**
  - Owners, the monks in the owner pool (the earlier word "priest" was only the owner's shorthand), **collect no wages**. They live
    communally on what the pop and the building hold, and the farms' surplus replenishes it.
  - Buildings only **predict** an owner's wage, so that they can hire.
  - **Any excess goes to the labourers.** Merchants trading oats for ale will help later.
- **C3, positions and movement.**
  - Each building defines how it fills its positions. The holdings have 5 monks and 5 labourers per level.
  - Open monk positions fill as needed, and labourers prefer to become monks when there is room.
  - **Every kind of hiring happens at a rate,** never all at once.
  - Peasants fill open labourer positions slowly, for the better wage. In its employment phase a pop transfers people, averages the
    values at the destination, and takes a share of its goods, rounded down. This is on the hot path.
- **C4, islands:**
  - island houses recruit from nearby populations;
  - the subsistence farm gains a **coastal production method**, so island houses can have peasants;
  - a monastery with truly no peasants draws recruits from nearby places, through the high income of its fishing.
- **Storehouses** (owner idea, adjusted by research; see §2):
  - simple local buildings at farms and monasteries, owned by the pops they serve;
  - depositors are tracked;
  - **alms, then the storehouse, then loss.**
  - Public granaries come later.
- **C6:** barter is **deferred** to the merchants milestone.
- **C7, the location panel and map modes.**
  - The place window only selects a place.
  - A **new location panel** fills the left third of the screen, with tabs for the location, its buildings and its pops.
  - **Map modes are a must, as a slice of this milestone:**
    - **stockpile**, stored against the maximum, from red (empty) to green (full);
    - **expected yearly yield**, relative across the map, from red at the minimum to green at the maximum.

    Places are points, so the map is blended. Maps that include consumption come later.

**Storehouse accounts (owner, 2026-11-08, answering review r17)**
- **Labourers' surplus:** after alms, it goes into the storehouse under the **labourers' own account**.
- **Movers' accounts:** movers **take their share of their old pop's account with them, as goods**, up to what their new store can
  hold. What does not fit stays with the old pop.

**Starting levels and growth of buildings**
- **A building's starting levels** are set from guesses at how big that building was, not derived from its population.
- **A pop can outgrow its jobs.** Adding a level needs the location to invest in it, which is **construction**: a separate process,
  not yet discussed, and out of scope here.

**Time and growth**
- **The daily tick runs at noon.**
- **Births and deaths are separate rates,** each driven by its own conditions. The design is "complex, but not complicated": it accounts
  for everything available, and stays cheap enough for the hot path. The first slice works these out with the owner.

## 2. Research (2026-11-08)

- **Religion in 768.**
  - Public worship of the old gods by high-status people had probably ended by the mid-to-late 500s. Marginal non-Christian practice
    seems to have continued until about the turn of the eighth century, when druids disappear from the law texts.
  - The penitentials, and the Church's hostility to the *fían* bands (*díberga*, "marauders"), record what it saw as pagan survivals.
  - No source locates remnant paganism in 768, so the pockets in §3.2 are a **marked invention.**
- **The English at Mayo.** Maigh Eo ("Mayo of the Saxons"), our location `mayo`, was founded about 668 by Colmán, formerly of Lindisfarne,
  for English monks. Over a hundred monks lived there by 700, and it went on attracting English students. Colmán's Irish monks stayed on
  Inishbofin (`inishbofin`). Rath Melsigi (Carlow) and Tullylease (Cork) also had English monks; neither is among the 94 places.
- **The Cruthin.** A people of the north-east (Dál nAraidi in Antrim, Uí Echach Cobo in Down), distinguished from the Ulaid in early
  sources although inside the Ulaid kingdom. The annals stop using the name around 773. Our `connor` and `antrim` lie in their country.
  Whether the difference was ethnic or only political is disputed.
- **Population.** A 2019 Queen's University Belfast radiocarbon study (Hannah and McLaughlin) puts early medieval Ireland at several
  million, perhaps over three million at a late-7th-century peak, and already declining by 800. The 94 places hold only part of that
  until villages and the countryside exist (map-design §5.5).
- **Diet and produce.**
  - Hulled barley and oats were the staple crops, with wheat secondary. Oats were eaten as porridge, and barley as bread and ale.
  - Cereals dominated winter and spring. Dairy, the "white meats", dominated from May on.
  - Cattle were the measure of wealth, valued for dairy more than for meat.
- **Hops.** No source puts hops in Irish brewing in 768. English evidence begins in the 9th–10th century, and the firm Irish evidence is
  16th-century imports from Flanders. Monastic ale is plain, or flavoured with herbs.
- **Storage (r17).**
  - Grain was dried in **corn-drying kilns** before it would keep over winter in the damp climate, and stored at each farmstead, often
    in **souterrains** (underground passages) beside ringforts. At Ballyvass (Kildare), a kiln was built into a souterrain.
  - Lords and churches held more, filled by clients' **food-renders**.
  - Communal granaries with deposits are not attested for 768.
  - Sources: TII (Doyle, Long), M28 kiln excavations, Charles-Edwards, *Early Irish and Welsh Kinship*.
- **Demography (general, not Ireland-specific).** Pre-industrial populations with high birth and death rates had roughly 35–40% children,
  which supports the owner's 40% base.

The sources are listed at the end. Kelly's *Early Irish Farming* and Lucas's work are the scholarly references to check next.

## 3. The model (Proposal unless marked Firm)

### 3.1 Goods, units and prices

Each good is a row in `assets/game/economy.toml`:
- its unit (Firm: each good has its own);
- its **base price**, in a notional value unit since there is no coin. It anchors the local price (§3.16; r13 replaces "fixed for M9");
- its **hunger** value, in person-days per unit;
- its **tier values** for necessities, niceties and luxuries;
- its stores.

The numbers are placeholders for tuning.

| Good | Unit | Hunger | Necessities | Niceties | Luxuries | From |
|---|---|---|---|---|---|---|
| oats | bushel | high | ✓ | | | farm, holdings |
| barley | bushel | high | ✓ | | | farm, holdings |
| cattle, pigs, sheep (live) | head | – | | | | the herds (§3.14); not eaten except by emergency butchering |
| beef, pork, mutton | pound (salted) | medium | ✓ | ✓ | | the herds' slaughtered surplus (§3.14) |
| dairy (butter, cheese) | pound | medium | ✓ | ✓ | | the cattle herd, by season (§3.14; Research: the main value of cattle; butter keeps) |
| peas and beans | bushel | medium | ✓ | | | farm (small) |
| vegetables (onions, leeks, cabbage) | bushel | low | ✓ | a little | | farm (small) |
| ale | gallon | low | | ✓ | | holdings |
| honey | pound | low | | ✓ | ✓ | holdings |
| mead | gallon | low | | ✓ | ✓ | holdings |
| apples | bushel | low | ✓ | ✓ | ✓ (high) | holdings |
| herbs | pound | none | | ✓ | ✓ | holdings |
| wine | gallon | low | | ✓ | ✓ (high) | none in Ireland: defined, waiting for trade |
| fish | pound (dried or salted) | low | ✓ | a little | | holdings, fishing method (Firm) |
| (garden) vegetables, herbs | as above | | | | | the fishing method's garden (Firm) |
| seabirds | bird | low | ✓ | a little | | holdings, fishing method (Firm) |
| beeswax | pound | none | | ✓ (small) | | holdings: ticked (§3.7 step 6) |
| wool, hides | pound, hide | none | | | | farm, holdings: held for the later clothing need (§3.7 step 6) |

Wheat waits (vision: too dear for ordinary pops). Hops are left out (§2).

### 3.2 Religions and cultures (data, with the start allocation)

- **Religions:**
  - Christianity;
  - Irish paganism, held at the start by a few small **pagan pockets**: peasant pops in remote places (bog and mountain catchments in the
    west), marked as invention in the guesswork log;
  - Norse paganism and Anglo-Saxon paganism, defined with no holders until events bring them.
- **Cultures:**
  - an Irish culture group with regional cultures: Ulaid, Cruthin, the Northern Uí Néill, Airgíalla, Mide and Brega, Laigin, Mumu and
    Connachta. Each place takes the culture of its region, with mixed places where the sources support it;
  - **English** (Anglo-Saxon, Christian), present at the start only at Mayo.
- **The day-one tests:**
  - Mayo has English monks beside Irish pops;
  - the north-east has Cruthin and Ulaid;
  - the pagan pockets share a subsistence farm with Christian peasants at the same place.

### 3.3 Pops

| Field | Meaning |
|---|---|
| type, culture, religion | the identity (Firm). Two pops at a place merge only when all three match |
| `workers`, `dependents` | **authoritative integer head counts** (Proposal, owner: no objection; review r7 Q3). `size = workers + dependents`, and the whole `size` eats |
| dependent share | **derived**, `dependents × 1000 / size` ‰, never stored. It is the owner's "% dependents" (Firm, a share of the population) |
| `needs_pm`, `niceties_pm`, `luxuries_pm` | the three tiers, 0–1000‰ (Firm 0–100%), stored and slow-moving (§3.7) |
| `satisfaction` | stored, default 0, drifting towards a baseline (vision, Firm) |
| `literacy_pm` | stored, shown. Nothing changes it in M9 |
| `store` | goods by good, in each good's unit (Firm). Capacity in §3.6 |
| `pantry_hunger[g]` | per food good: hunger already taken out of opened units of `g` and not yet eaten, in hundredths of a person-day (§3.7, r15) |
| `pantry_needs`, `pantry_niceties`, `pantry_luxuries` | the three tier balances, in hundredths of a person-day (§3.7) |
| `employment` | per building: `(owners, workers)` from this pop (§3.5) |
| carries | the remainders of §6 |

**The pop types:**

| Type | Dependent base | Births | Expectations (necessities / niceties / luxuries) |
|---|---|---|---|
| peasant | 400‰ (Firm, about 40%) | yes | 100% / 0% / 0% (Firm: no niceties or luxuries expected) |
| labourer | 400‰ | yes | 100% / some, about 30% / 0% (Firm: some niceties, no luxuries) |
| monk | low, about 100–150‰ (the old and sick only) | none (Firm) | 100% / some / a little (Proposal) |

### 3.4 Buildings

**No fixed building rules per kind (Firm).** The scenario sets each place up however we want. A place may have a subsistence farm, a
monastery's holdings, both, or neither. The kind-and-size tables of §3.12–3.13 are authoring defaults only.

**The subsistence farm.**
- **At most one per place,** as map-design §5.5 has it: all of a location's labourers work one farm.
- **Two methods, chosen per level by value (Firm, r17):** early-medieval mixed farming, and **coastal subsistence** (fish and
  seabirds, worked by peasants). Coastal levels draw on the same fish pool as the holdings' fishing (below). Mixed farming's outputs per
  level are data:
  - oats and barley at harvest;
  - cattle, dairy, pigs and sheep through the year;
  - small amounts of peas and beans, vegetables, wool and hides.
- **Ownership:** owned by its workers, who keep all of its output (`docs/game/crown-military-church.md`). Its owners and workers are the
  peasant pops working it. Several peasant pops of different cultures and religions can work one farm (vision, Firm).

**The monastic holdings.**
- **At most one per place,** owned by its monks.
- **Employees:** monks (with the priests among them as owners) and hired labourers.
- **Two methods, mixed level by level (Firm, r13):**
  - **monastic farming:** all of the farm's goods, plus ale, honey, mead, apples, herbs and beeswax in more limited quantities;
  - **monastic fishing:** fish and seabirds, and a **garden** of herbs and vegetables (Firm). It takes up less space. Until fishing
    grounds are designed (Open), it uses the catchment's **sea** cells as a stand-in.

**Maximum sizes from land (Firm).**
- A building's maximum levels are derived at load from its place's catchment, as land counts are (r5's adapter, hashed through the build
  id, never saved):
  - **subsistence farm, farming:** `max = free usable cells / 2`, floored. **Since slice 9, "usable cells" are fertility sums in
    cell-equivalents (§3.24).** Its coastal levels share the sea budget with the holdings'
    fishing (below). The farm's size is the sum of the two (owner, 2026-11-09);
  - **holdings, farming:** `max = usable cells / 2`, floored. Farming levels of the holdings and the farm together cannot exceed the
    land;
  - **fishing, holdings and the farm's coastal method together:** `max = sea cells in the catchment / 4`, floored, shared by both
    buildings (r17) (Proposal: 4 sea cells per level; "less space", Firm). This
    is a stand-in until fishing grounds exist: a sea resource area with its own fishing radius (Open). **Replaced in slice 9 (§3.23,
    owner 2026-11-09):** fishing is limited by building footprint, not fish, and needs at least one sea cell in the catchment.
- **The land order.** The holdings take their land first; the farm takes what is left.
- **Choosing methods per level (Firm, r13; Proposal mechanism).** At the start, each building assigns its authored levels **one at a
  time**. Each level takes the method with the highest **estimated net value** that still has room under its limit:
  - **the estimate is the value after labour and inputs (Firm, r14),** over **one year**:
    `estimate(m) = (Σ_g annual_output_g(m) × projected_price(g) − annual input cost(m)) × overhead_pm / 1000`.
    This is the value left after the workers' wage pool (§3.6). Inputs are empty in M9, so their cost is 0, but the term is in the
    formula;
  - **the annual outputs** at the start: crops per year at the land's suitability; herd products at the starting herd's yearly yield
    (births at the starting size, dairy and wool for a year); fish and the garden per year;
  - **`projected_price(g)`** comes from the pure start-up primitives of slice 2 (§3.17):
    - `target_price(base, buy_value, sell_value)`, which §3.16 reuses daily;
    - **buy:** the place's starting pops' **diet profiles × people × 365**, fixed for the whole choice;
    - **sell:** the annual output of the levels **already assigned** at the place, cumulative, starting from nothing;
    - so the second fishing level sees the first level's fish already supplied, and a lower price;
  - **the order:** places in file order; within a place, the holdings, then the farm; levels one at a time. Each assigned level's
    annual output joins the sell side before the next choice. No eating planner and no live orders are used, so nothing is circular;
  - ties go in method order (farming before fishing).
- **The result** is a count of levels per method in each building, which is saved. Each farming level of the holdings has its herd
  (§3.14), and fishing levels have none.
- **Staffing per method (r14, exact):** there is no integer `staffed_m`. Each method's output takes its share of staffing exactly
  inside the numerator, each month: `portion(annual_g × staffed × levels_m × season_pm_g[month], suitability_g × cov, levels × workers_per_level × 10⁹, carry_g)` (r17: by season, §3.19).
  - Staffing is conserved and never rounded; only whole goods are floored.
  - Coverage applies to the whole building.
- **Chosen once in M9.** Changing a level's method mid-game is the vision's conversion, and comes with construction. It is out of
  scope. The owner has set its rules for later (2026-11-09):
  - **cost:** the level loses the goods of its old method, and the new method produces nothing for its first month;
  - **lock:** a switch starts a countdown that locks the level's method until the end of the year. This matters for the future real
    farms: a fishing village must not switch to oats in September, collect the harvest, and switch back;
  - **room:** a switch must fit the target method's limit, so it can never put a building over a cap. A monastery with no land can
    never switch to farming. Owner: "if a monastary has no land, it shouldnt ever be over the cap, so that rule would never apply to a
    switch, only newly built farm";
  - so the cut below applies only to authored levels at load, and the room check to newly built levels. Neither ever deletes an
    existing level.
- **Limits:** farming levels by usable land (fertility, §3.24), and fishing levels by building footprint, with at least one sea cell in
  the catchment (§3.23, slice 9; the sea-cell stand-in above is withdrawn). Authored levels that fit under no method's limit are cut, and the load report lists them.
- **Every monastery is productive** (Firm, for now). The load report names any place where no method can hold a level, so that the
  data is fixed.

**Coverage (Firm ratios).**
- `owners_needed = ⌈workers / 10⌉` on the farm, and `⌈monks / 5⌉ + ⌈labourers / 10⌉` priests at the holdings. The ratio is per
  building type, in data.
- `coverage = min(1000, owners × 1000 / owners_needed)`, in ‰. With no workers, coverage is 1000.
- Below 1000, output is multiplied by `1000 − penalty_pm × (1000 − coverage) / 1000`, in ‰. `penalty_pm` is data; at 500, an unmanaged
  share of the workforce produces half (Firm: a scaled loss).

**Owners are an exclusive role (Firm, relayed in review r7).** An employed person is either an owner or a worker, and is counted once.
- Owners are unpaid (r17) and do not produce.
- Workers produce.
- A farm level of 10 workers and 1 owner therefore has **10 paid and 10 producing**. Owners are unpaid and live communally (r17, Firm).
- Owners and workers come from the same pops. Priests come from the monk pops, and labourers are never owners.

### 3.5 Employment, recruitment and movement (Firm, r17; Proposal rates)

**The positions.** Each building type defines its positions per level, in data (Firm):
- **subsistence farm:** 10 workers per level (peasants);
- **monastic holdings:** **5 monks and 5 labourers** per level;
- **storehouse:** 2 workers per level, from its owner pops (§3.18);
- **owners on top, by coverage:**
  - 1 per 10 farm workers;
  - 1 per 5 monks or 10 labourers at the holdings, where the owners are monks in the owner pool.

**An open position** is a slot of its kind without a person in it. An open monk position is a monk slot without a monk (Firm: "as
monks die").

**Hiring, each month in the employment phase, always at a rate (Firm):**
- a building fills at most `max(1, ⌊open × hire_pm / 1000⌋)` open positions of each kind, with `hire_pm = 100` (data);
- a source pop gives up at most `⌊workers × leave_pm / 1000⌋` people a month, with `leave_pm = 50` (data);
- **the order:**
  1. owners, as coverage needs;
  2. then workers of each kind, from that kind's own pops at the place;
  3. then **movement** (below).

**Movement between pops (Firm, r17).** An open position draws people from other pops when they are paid less than its posted wage:
- **labourer positions** draw from **peasants** at the place (the vision's "labourers are hired from the peasant population");
- **monk positions** draw from **Christian** pops (Firm), and labourers prefer them when there is room (Firm). Culture does not matter;
- **sources:** poorest first, by wage per head, with ties in pop order. A pop's wage per head is
  `⌊Σ_b workers_p,b × posted_b / workers_p⌋` over the buildings where it works, so its owners and its unemployed count as 0. A position
  draws from a source only when that is below the position's posted wage;
- **nearby places (Firm, C4):** when a position has no eligible source at its own place, it draws from nearby places, nearest first,
  at the same rates. The pull is the posted wage: a profitable fishing house attracts people.
  - **The range is per building, in data** (`recruit_range_m`): **monastic holdings 100 km** (owner, 2026-11-10: "all monastaries
    could use a recruitment range of 100 km", for all their positions; `owner-direction-monastery-range-r1.md`), and 30 km for every
    other building (Proposal); slice 6b implements it. Only monk and labourer positions move today, so in practice the range is the holdings'.
  - **Distances are on the ground,** not in map metres: `⌊√(dx² + dy²)⌋` cells × 839 m × the cosine of the latitude of the row midway
    between the two places (Web Mercator cells are about 500 m on the ground in Ireland). So Skellig Michael reaches Inisfallen
    (75 km) and Ardfert (81 km). Its food is left alone until trade (owner: "we need to leave it alone until we implement trade");
- **The counts (E3):**
  - `J` is the people a position takes this month. It is capped by the building's hiring rate and by the source's **monthly leave
    budget**, `⌊workers × leave_pm / 1000⌋`, shared across every destination;
  - destinations are served in building order, then position kind (owners, monks, labourers, workers);
  - places at equal distance are taken in file order.
- **Who moves:**
  - **for a labourer or worker position,** `J` workers plus `⌊J × dependents / workers⌋` dependents move;
  - **for a monk position (Firm, unchanged),** `J` recruits are removed from the source's workers and dependents **in proportion**,
    largest remainder, so the source's share is unchanged. **All of them join as working monks** with no dependents.
- **The transfer:**
  - movers join the destination pop of their culture, religion and new type, or start one;
  - its literacy, satisfaction and the three needs become the people-weighted averages;
  - they take their share of the source pop's goods, good by good, `⌊units × movers / size⌋`, rounded down (Firm). The remainder stays,
    so nothing is duplicated. Pantries and the tier balances move the same way. Carries stay with the source;
  - **account rights (Firm, r18):** the movers' share of the source's storehouse account, `⌊account_g × movers / size⌋`, is withdrawn
    as goods and moves with them;
  - **destination caps:** everything moved is limited by the destination store's volume and per-good caps. What does not fit **stays
    with the source**, and is never lost;
  - **one commit (E3):** each move commits the source and destination places together, or neither. On a checked-arithmetic refusal,
    both are left as they were, and the per-place rollback rule extends to the pair.
- **the cost:** a few multiplications per pop a month (hot path, Firm).

**Releases** use r5's capacity-bounded largest-remainder rule when a pop shrinks.

### 3.6 Wages and the flow of goods

- **The wage pool (Firm, r14; the vision's rule):** "a wage derived from the cost of its inputs and the value of its outputs, leaving an
  overhead for its owners".
  - `value = Σ units × local price` (§3.16), for the period's output;
  - `wage_pool = (value − input cost) × (1000 − overhead_pm) / 1000`;
  - `overhead = value − input cost − wage_pool`;
  - `overhead_pm` is 100 on the subsistence farm and 300 at the holdings (Firm placeholders, by building type later);
  - input cost is 0 in M9 (empty input lists, Firm by design).
- **Who gets what:**
  - **the wage pool** is shared equally per head among the **non-owner workers** only (Firm, r17: owners collect no wages);
  - **owners live communally** on what their pop and the building hold, and the overhead replenishes the building's stockpile. Each
    building computes a **predicted owner wage** (its overhead per owner) for hiring only;
  - **the overhead has one path (E2):** all of it goes into the **building's stockpile**, the owners' communal store. Owners draw from
    it (below). Nothing goes straight to an owner pop's store;
  - **the whole-good split (R14-F2):** for each good `g` of the period's output, in table order:
    - `w_ppm = ⌊(value − input cost) × (1000 − overhead_pm) × 1000 / value⌋`, frozen for this payday. It is `(1000 − overhead_pm) × 1000`
      when inputs are 0: 900,000 on a farm, 700,000 at the holdings;
    - `wage_units_g = portion(units_g, w_ppm, 10⁶, wcarry_g)`, with a per-building, per-good carry in millionths of a unit;
    - `overhead_units_g = units_g − wage_units_g`, so nothing is duplicated;
    - **wage units** go to the **non-owner workers**, each pop by its employed workers, by largest remainder in pop order. Leftover
      whole units go to the building's `pay_carry_g` for the next payday. **Overhead units** go to the building's stockpile;
    - **no paid workers:** the wage units wait in `pay_carry_g`;
    - **no output:** when `value = 0` there is nothing to split, and `w_ppm` is not computed;
  - on a subsistence farm, owners and workers are the same peasant pops, so the wage reaches their stores and the overhead their
    communal stockpile. At the holdings, labourers and working monks share the wage, and the overhead fills the monastery's stockpile.
- **The period** follows the vision's Firm timing: crop outputs at harvest, and livestock products, fish and the rest monthly.
- **The posted wage** is the hiring signal (Firm, for monks). **It is the average monthly pay per paid head over the last 12 months**
  (Firm, owner 2026-11-09, `owner-direction-posted-wage-r1.md`), so a farm paid mostly at the September harvest compares fairly with a
  monastery paid every month:
  - each building saves `wage_history`, 12 values in thousandths of a value unit per paid head, one per calendar month;
  - at each monthly payday, with `P` its paid heads (non-owner workers) and `W` the value of this payday's wage units at local prices,
    the month's entry becomes `⌊W / P⌋`. A month with workers and no output records 0. **With no paid heads the entry is kept** (r19's
    "kept even when nothing is paid");
  - `posted = ⌊Σ wage_history / 12⌋`.
- **Before any history (Firm: starting stockpiles):**
  - each building starts with an authored stockpile (§3.10);
  - every month's entry starts at the value of that stockpile per paid head, spread over the months to the next harvest:
    `⌊V₀ / (P₀ × M)⌋`, with `M` the months from the start month to the first month with a grain harvest (11 for an October start).
    Real months replace these entries as paydays happen;
  - monthly wages are paid from it.
- **Store capacities are volumes** (Firm; §3.15). `Σ units × volume ≤ capacity`, in millilitres, shared by every stored good. A unit
  fits only whole, and a unit that does not fit is lost (Firm). Live animals live in herds (§3.14).
- **Payday (Firm, r14):**
  1. the wages go into the workers' own stores, up to their volume and per-good caps;
  2. the overhead, and any wages that do not fit, go into the **building's stockpile**, up to its capacity;
  3. **alms (Firm, r17):** what still does not fit goes to the **labourers** at the place, then to any other hungry pop there, hungriest
     (fewest days of food) first, each up to its room. **Hungry** means less than 60 days of food, the drawing threshold below,
     counting the pop's store and its storehouse account (owner, 2026-11-10, `owner-direction-ck227-notes-r1.md`);
  4. **the storehouse** (§3.18). It is credited to the account of the pop the goods came from: the worker pop for its wages, including
     labourers (Firm, r18), and the building's communal account for overhead;
  5. the rest is lost.
- **Outside payday, pops keep what they hold** (Firm). Nothing moves from a pop to a building.
- **Drawing (E1):** when a pop's own food (store plus pantries) falls below 60 days, it moves real goods into its store, until it is
  back at 60 days or nothing is left, in this order:
  1. its own storehouse account;
  2. if it has owners at a building, that building's stockpile and the building's communal account.

  Labourers draw only from their own account.

### 3.7 Eating and needs, daily at noon

- **Four benefit balances** (Proposal, owner: no objection; review r7 Q2): hunger, necessities, niceties and luxuries. Opening one unit of
  a good adds **all** of its values to the balances, and nothing left over is lost.
- **The scale (r11: hundredths, replacing r9's tenths).** Every benefit balance, good value, need and reserve test is in **hundredths
  of a person-day**, which represents every value in §3.12 exactly (pork niceties 0.35 is 35).
  - A pop's daily need for hunger and for each tier is `need = 100 × size`.
  - "60 days of food" is `6000 × size`.
  - Goods' values are stored ×100 (oats: 1800).
  - Store capacities are volumes (§3.15), not person-days.
- **Units and benefits are separate.** Stores, volumes, prices, wages and the goods ledger count **whole units** of each good. Benefits
  exist only in the four balances, and enter them only when a whole unit is opened.
- **Eating, each day at noon (Firm, r15: an exact equal share of what remains, scaled by each good's stockpile):**
  0. **If `size = 0`,** the pop skips eating entirely, and nothing is divided.
  1. **What is held and what is owed (E1):**
     - `H_g = units_g × hunger_g + pantry_hunger[g]` is the food physically in the pop's store, after today's drawing (§3.6), and
       `H = Σ H_g`;
     - `A` is the food left in its storehouse account, a claim it can draw later;
     - today's need is `need = 100 × size`.
  2. **Emergency butchering** runs when `H < ⌊need × 500 / 1000⌋`, the 500‰ intake, and the pop has herd access (below). It stops when
     `H` reaches that threshold, or at the breeding floor. Then `H` is recomputed.
  3. **Today's total, scaled by days of food held (Firm, r17):**
     - `days = (H + A) / need`, the days of full eating it owns, with `need > 0` by step 0;
     - the intake `c` is read from a curve: ≥ 240 days → 1000‰, 120 → 800‰, ≤ 30 → 500‰, linear between (data);
     - `T = min(⌊need × c / 1000⌋, H)`. It is eaten only from what is held. At the 500‰ floor the pop keeps eating 500‰ until the food
       is gone ("people gotta eat"), then eats what is left;
     - if `T = 0` (no people, or no food even after butchering), nothing is eaten and nothing is divided.
     - So stores fall through the year, intake eases to about 80% before harvest, tightens towards 50% in a bad year, and recovers when
       the September harvest arrives. There is no forecasting.
  4. **One joint apportionment (R15-F1)**, exact in `u128`. With `T ≤ H` it never overdraws:
     - each good first gets `f_g = ⌊T × H_g / H⌋`. Since `T ≤ H`, `f_g ≤ H_g`;
     - the leftover `R = T − Σ f_g` is fewer than the number of goods. It is given one hundredth at a time to the goods with a non-zero
       remainder `T × H_g mod H`, largest remainder first, ties in table order;
     - such a good has `f_g < T × H_g / H ≤ H_g`, so `f_g + 1 ≤ H_g`, and no good is overdrawn;
     - `eat_g` is the result: exactly `T` in all, each within one hundredth of its exact proportional share. When `T = H`, every
       `eat_g = H_g`, and everything held is eaten;
     - there is no carry and no saved state for the split.
  5. **Opening and eating:** for each `g`, open `⌈(eat_g − pantry_hunger[g]) / hunger_g⌉` units at once (one division, r17; C9),
     when that is positive. The result is the same as opening one at a time. Its hunger goes to
     `pantry_hunger[g]`, and its tier values to the tier balances. Then eat `eat_g` from `pantry_hunger[g]`. Because `eat_g ≤ H_g`, the
     units always suffice.
  6. **Ticking goods (Firm, owner-signed 2026-11-09):** herbs and beeswax only. Each is used at a fixed rate **per person per day, by pop
     type** (data, in millionths of a unit), `portion(size, rate_g,type, 10⁶, carry_g)` whole units, while it lasts. Its tier values enter
     the balances.

     | Good | Monk | Labourer | Peasant | Niceties per unit | Luxuries per unit |
     |---|---|---|---|---|---|
     | herbs | 6,000 (2.2 lb a year) | 2,000 (0.73 lb) | 1,000 (0.37 lb) | 10 | 5 |
     | beeswax | 4,000 (1.46 lb a year) | 500 (0.18 lb) | 250 (0.09 lb) | 5 | – |

     - "A monk would use more beeswax than a peasant" (owner): altar and liturgical candles for monks, while peasants lit with tallow
       and rushlights; herbs for the kitchen and infirmary.
     - A holdings level (about 6 monk-people and 8 labourer-people) uses about 10 of its 10 lb of beeswax and 18 of its 20 lb of herbs a
       year.
     - Sources: Sapoznik (2019), wax bought by great English houses (200–1,400 lb a year) and a parish church's minimum (about 15 lb).
       Record: `m9-design/ticking-goods-proposal-r1.md`, `owner-direction-clothing-r2.md`.
     - **Wool and hides do not tick** and have no tier values: "wool and linen dont tick, they are consumed" (owner). A pop holds them,
       within its storage limits, until **a later clothing slice** (after M9): clothing becomes a separate need beside food and housing,
       filled by wool, linen (a flax crop on the farm) and hides, each giving a different amount of rudimentary clothing. Tailors later
       make clothing that fulfils niceties. A shortfall raises deaths in cold months, by the place's computed monthly temperature, and
       lowers satisfaction.
  7. **Tiers:** for each tier, take up to `need` from its balance. `taken × 1000 / need` ‰ is today's fulfilment, and 1000 when `size` is
     0.
- **Example:** a pop holding 300 bu of oats and 100 bu of barley (with oats 18 and barley 28 person-days a bushel) eats about 66% of
  its hunger from oats and 34% from barley today. As the barley runs down, its share falls.
- **Why grain dominates (Firm):** per-good storage limits (§3.15) let a pop keep many bushels of grain but little fruit, so most of
  every day's share comes from grain.
- **What eating no longer does:** the cheapest-first rule and the 60-day gate on niceties are withdrawn (r14). A pop eats what it has,
  in proportion. What it has comes from its wages, alms, drawing and movement (§3.5, §3.6).
- **Example:** a gallon of mead adds 1.5 hunger, 4 niceties and 4 luxuries. Whatever is not eaten today stays for tomorrow.
- **Emergency butchering (Firm; r17 floor; E1 threshold).** It runs only when the pop cannot keep up even the 500‰ intake, as in step
  2. It never takes a species
  below its **breeding floor** of 2 head per level (data), unless the pop's necessities are below 300‰ (real starvation). When a pop's
  hunger balance and stored food cannot cover that intake, it butchers livestock. It uses
  the herds of the buildings where it has owners, one head at a time, until `H` reaches `⌊need × 500 / 1000⌋`, the breeding floor
  stops it, or the herds are empty:
  - **the order** (Proposal): pigs, then sheep, then cattle. The cows that give dairy go last;
  - **each head** yields whole units of meat (and hides), as slaughter does (§3.14). **One path (r11):**
    1. the units go into the **pop's store**, up to its volume;
    2. units that do not fit are destroyed and counted as lost;
    3. eating then continues as normal: it opens units from the store into the balances (step 1 of eating).

    So the goods ledger records them as produced, then stored or lost, then opened. The benefit ledger counts only opened units.
    Butchering stops once the store plus the balance reach that 500‰ threshold, or when the store is full;
  - labourers have no herd, so they cannot butcher.

  It eats into future production: a realistic famine trap.
- **The needs values move slowly** (Firm). Each tier value moves a data fraction of the way towards today's fulfilment each day. A full
  store holds them up, and a drained store lets them sink over weeks.
- **Satisfaction.** The gap between each tier and the pop type's expectation sets the satisfaction baseline (Firm: an expectation check).
  Data thresholds add extra effects at certain levels: for example necessities below 50%, or luxuries above expectation. Satisfaction
  drifts towards that baseline, as the vision's Firm rule has it.
- **Standard of living** is derived from the tiers and satisfaction, never stored (vision, Firm).

### 3.8 Births and deaths, monthly (Firm separate rates; Proposal curves)

Every factor is a small **response curve** in data: about five points, read by integer linear interpolation. So a pop costs about six
lookups and a few multiplications a month, and every curve can be tuned without code.

```
births   = workers    × birth_base[type] × f_food(N) × f_crowd(C) × f_satisfaction(S) × f_literacy(Lit)
deaths_w = workers    × death_base[type]     × g_hunger(N) × g_comfort(W, L) × g_crowd(C)
deaths_d = dependents × dep_death_base[type] × g_hunger(N) × g_comfort(W, L) × g_crowd(C)²
```

**The inputs:**
- N, W and L are the stored tier values;
- S is satisfaction;
- Lit is literacy;
- C is **each pop's own crowding,** against the place's **housing** (below). It replaces r17's capacity (usable land × 8 people a cell,
  plus the fishing levels' food), which is withdrawn.

**Crowding as living space (Firm, owner-signed 2026-11-09).** Signed text: `crowding-design-r3.md` (sha256 e03651ac).
- **The owner's decisions:**
  - crowding is living space: "crowding is supposed to be living space";
  - "housing should be a number provided by the buildings";
  - housing is a soft cap: "excess pops over capacity could have negative effects instead of just not allowing people to live there";
  - pops "naturally choose the best quality housing that they can afford".
- **Housing per building level** (data, hard values, not computed):

  | Building | Housing per level | Class |
  |---|---|---|
  | subsistence farm (any method, coastal included) | 17 | basic |
  | monastic holdings (any method) | 14 | basic |
  | storehouse | 3 | basic |

  - A level counts whenever it exists, staffed or not.
  - Today's values follow the owner's rule, "room for a full staff with an average amount of dependents" (workers per level ÷ (1 − the
    base dependent share)). Future buildings need not follow it.
- **Natural shelter** (makeshift class) = `⌊Σ over the catchment's land cells of shelter_t ÷ 1000⌋` people. It is "homeless straglers
  that live off the land", weighted by terrain, in thousandths of a person per cell:

  | Terrain | pasture | forest | mountain (caves) | marsh | bog | glacier |
  |---|---|---|---|---|---|---|
  | `shelter_t` | 50 | 100 | 100 | 20 | 10 | 0 |

- **Housing classes,** in quality order: makeshift, then basic. Neither charges rent. Rented classes above basic come later.
- **The monthly fill,** on the 1st, first in §3.9 step 5, before births and deaths, in whole people, per place:
  1. rented classes (later), to the pops that can afford them;
  2. **basic, workers first:** each building's housing goes to the pops employed there, each worker bringing their pop's average
     dependents;
  3. **spare basic, pooled:** what is left goes to every pop's still-unhoused people, so "a monastary will house the homeless until it is
     full";
  4. **makeshift:** natural shelter takes the rest;
  5. **overflow:** once both are full, the remaining people are spread across both classes in proportion to their capacities.
- **Class crowding:** `c_k = occupants_k × 1000 ÷ capacity_k`, overflow included.
- **Each pop's crowding:** `C_pop = Σ_k people_pop,k × c_k ÷ size`. Births and deaths use the pop's own `C_pop` on the curves below. A
  place with no housing at all gives every pop 2000, and beyond 2000 the curves hold their last point.
- **The place's crowding** (the panel, a future map mode) is all its people × 1000 ÷ all its housing, shelter included.
- **No links and no save:** the fill is recomputed from the current pops and buildings, keeps no pointers either way, and is not saved.
  A pop that dies off gets nothing, and its space goes to the others.
- **Development** raises capacity only indirectly: it raises the buildable maximum (§3.23), so more housing can be built. M9 builds
  nothing, so development changes no crowding in M9.
- **Later (recorded, not M9):** rental buildings with no production methods, owned by a landowner class; masonry for upkeep; quality
  feeding comfort; per-building visuals; crowding driving migration.
- **The integer rules** (claims, rounding and attribution) are frozen in CK-229's spec.

**The curves:**
- **Hunger:**
  - births collapse when necessities fall below about 50%;
  - deaths rise steeply below the threshold (Firm: a drained store raises deaths only once the needs value is low).
- **Comfort:** niceties and luxuries lower deaths a little.
- **Crowding:**
  - it lowers births;
  - it stands in for disease in deaths;
  - dependents feel it squared, being the most susceptible (Firm).
- **Satisfaction and literacy** start as flat curves: inputs that are wired in, but tuned later.

**The rest:**
- **The rates, applied once:** the data holds both rates for each pop type, `worker_deaths_pm_year` and `dependent_deaths_pm_year`,
  and the dependent rate in data is authoritative for `deaths_d` (CK-229). The shipped dependent rates (60‰ and 75‰, §3.12) were set
  as the worker rate times a frailty of 3; no separate frailty value exists in data or code.
- **Deaths are capped at the pool** (review r7 E1). `deaths_w ≤ workers` and `deaths_d ≤ dependents`. Whole deaths beyond an empty
  pool are discarded, and each carry keeps only its own remainder. At the extremes (hunger 0, crowding 2000), the dependents' monthly
  rate exceeds 1, and a whole pool can die in a month.
- **Monks:** `birth_base` is 0 (Firm).
- **The dependent share:** births enter as dependents, and deaths leave each part as above. The share then drifts a data fraction back
  towards its type's base (children grow up, the sick recover).
- **Order:** births, deaths, then the drift. Every fraction is carried (§6).

### 3.9 The clock (review r7 E2)

- **Each coarse cadence gains an hour of day:** `Yearly { hour }`, `Monthly { hour }` and `Daily { hour }`, from 0 to 23.
  - **The defaults are 0,** so existing registrations, and their accepted tests, keep their midnight behaviour.
  - `Hourly` is unchanged and runs on every tick.
- **The hour is available to every coarse cadence,** but the economy uses only **one `Daily { 12 }` system** (R14-F4), with its
  monthly work inside it on the 1st (below). The gameplay callbacks are also `Daily { 12 }` systems registered **after** it: `on_month`
  (which fires only on the 1st), then `on_day`.
- **Dispatch:** on any tick, the systems due are those whose cadence and hour match. They run coarser first (yearly, monthly, daily,
  hourly), and in registration order within a cadence. That is the accepted order, applied only to what is due.
- **A save at 11:00 or 12:00** rebuilds the same schedule, because registration is code, which the build id covers. The tests cover
  ordinary noon, midnight systems unchanged, month and year boundaries at noon, and a save-load-continue across 11:00 and 12:00.
- **The economy is one daily system at 12:00 (r14). Its steps, in order (E6, the single order):**
  1. orders and prices (§3.16); until slice 7 lands, prices stay at base;
  2. **on the 1st:**
     - production by the season tables (§3.19);
     - herds (§3.14);
     - payday (§3.6), with alms and the storehouse;
  3. drawing (§3.6);
  4. eating and needs (§3.7);
  5. **on the 1st:**
     - births, deaths and drift (§3.8);
     - hiring and movement (§3.5);
     - the storehouse fee (§3.18);
     - the monthly satisfaction drift.

  **Step 0, on the 1st, before step 1:** the counter rollover of §3.11.
  *(In the accepted code the rollover runs on the 1st **after** step 1's price update and before the month's production: it is the
  first act of step 2. Nothing showed the difference until CK-274, whose values use the prices of that moment, §3.11.)*

  r14's barter step is removed (barter is deferred).
- **The gameplay callbacks** register as daily systems at 12:00 **after** the economy: `on_month` (it fires only on the 1st), then
  `on_day`. Lua therefore always sees the day's finished state.

### 3.10 The starting state (authored, deterministic)

For each place:
- **its pops:** type, culture, religion and size, from defaults by kind and size, with per-place overrides:
  - Mayo's English monks;
  - the pagan pockets;
  - monasteries without peasants;
- **its buildings, levels and land** (§3.4), with the levels authored by guess (§3.13), not derived from the pops;
- **its employment:** owners first, then workers;
- **its needs (Firm, owner 2026-11-09, ruling on CK-228 B1, "for now"):** every pop starts a new game with necessities 1000‰,
  niceties 0 and luxuries 0. The starting values are data per pop type;
- **its stores (Firm: starting stockpiles):** authored per pop and per building, by default enough food to reach the next harvest,
  since the start falls after it. The buildings' stores pay monthly wages until production does (§3.6).

No random draw is used (map-design §5.1). Every number goes into the scenario's guesswork log.

### 3.11 The place window and the location panel (Firm, r17)

- **The place window** only selects a place.
- **The location panel** fills the left third of the screen, with tabs (Firm):
  - **Location:** people (workers and dependents), food in days, storage used against capacity, prices against base (§3.16), and last
    month's losses to overflow;
  - **Pops:** each pop's type, culture, religion, workers and dependents, employed and unemployed, food in days, the three needs,
    satisfaction, and last month's births and deaths as a trend arrow;
  - **Buildings:** levels by method (for example "farming 3, fishing 2"), positions filled against open, herds per species against
    their maximum, the store's fill, and the posted wage.
- **The data path** is r5's revision-pinned view (§6). The panel reads the view, never live state.
- **Monthly counters (E5; r19 buckets):** births, deaths and overflow losses are counted in two saved buckets per pop and per place:
  `current` (accumulating) and `last` (the last completed month, which the panel shows).
  - **The rollover** is the first act on the 1st, step 0 of §3.9: `last ← current`, then `current ← 0`. The 1st's own events count in
    the new month.
  - Both buckets are saved and loaded, so a load shows the same last month.
  - They are display only; no rule reads them.
  - **Per good, for the place** (CK-274; for the owner's location tooltip, `.pipeline/coordination/ui-polish-design/`
    `owner-direction-tooltips-manager-r4.md` and `-designer-r8.md`: the owner chose the value of what was produced and consumed
    **"Last month"**, shown to them as "counted through the month, shown for the last complete month … each number breaks down by
    good", and overflow **"Per good, units"**). A place's two buckets also count whole units per good, in the goods' order; a pop's
    buckets do not:
    - **`produced`:** every unit made at the place: what building methods and herds yield on the 1st (slaughter and dairy included)
      and what pops' butchering yields, counted when made, whether it is then stored or lost;
    - **`used_by_pops`:** the units pops use up in the day's consumption: those opened into their balances and the zero-hunger goods
      used up by ticking (§3.7);
    - **`lost`:** the units lost to overflow, by good. Their sum is the existing overflow total, which stays.
    - **There is no count of building inputs.** Buildings take no goods as inputs today, so what a place consumed is what its pops
      used; an input count comes with inputs, a later game-model change (the owner's answer was "Yes, both": `-designer-r9.md`).
  - **Valued at the rollover.** When the month closes, `last` also keeps each good's value of `produced` and `used_by_pops`:
    units × the place's price of that good, in thousandths. The price is the one in force when the rollover runs, the 1st's after
    that day's price update (§3.9). So a shown value does not drift as prices move during the next month. Twelve oats at 96.5 are
    1,158.
  - **`closed`:** each pair of buckets has a saved flag, false in a new game and set by every rollover. It tells a completed month
    in which nothing happened (the panel can show 0) from a game that has not closed a month yet (nothing to show).
  - The 1st's own production and consumption count in the new month, as its births and deaths do.
  - All of this is saved with the buckets and loads back the same. As with any change to the economy's code, a save from an earlier
    build does not load (`docs/places-time-design.md`, the build id).
- **Land in the view** (CK-274; the owner chose **"Usable land"**, shown to them as "usable-land cells × 0.25", broken down by
  terrain, with "the total incl. bog, mountain and sea": `-designer-r8.md`). The land counts a place gets from the map at load
  (§3.24; not saved) now also say how many of its cells have fertility above zero **by terrain**; their sum is the place's count
  of fertile cells. Nothing that already read the land counts changes.
- **What the view gives the UI** (`game.place`, CK-274), beside the fields above:
  - `land = {usable = {<terrain> = cells}, land = {<terrain> = cells}, sea = cells}`: usable cells, all land cells (glaciers left
    out) and the catchment's sea cells. They are counts of map cells; a cell is 0.25 km² (`docs/map-design.md` §5.2), and the UI
    does the multiplying;
  - `last_month = {closed, produced, used_by_pops, lost}`: each a list of `{good, units, value_milli}` for the goods with units
    above zero, in the goods' order (`lost` has no value). While `closed` is false the lists are empty. Integers too large for Lua
    are given as its largest integer;
  - nothing in the UI shows these yet: the location tooltip and its breakdowns are later slices.
- **Widgets:** it uses the M8 theme's windows and lists, with tabs as title buttons. The tab widget is the one new UI piece.
- **Since the owner's UI feedback (2026-11-10, CK-258, CK-259; `owner-direction-ui-ledger-lists-r1.md`):** the pops and buildings tabs
  show their entries as **cards** (the default) or one-line **rows** that open, switched by two buttons at the bottom of the panel; the
  places list is replaced by the **ledger**, a window docked on the right third with a locations tab and a tab of every pop. Details in
  `docs/ui-design.md`.

### 3.12 The starting numbers (Firm: approved by the owner, 2026-11-08, as placeholders for tuning)

**Units:**
- **hunger and tier values** are in person-days: how many people one unit covers for one day. Each person needs 1 person-day of each
  tier each day, and fulfilment is the share covered;
- **hunger** is derived from about 2,000 kcal per person per day;
- **prices** are in a notional value unit;
- **storage:** hunger and tier values are stored in hundredths of a person-day (r11), and ‰ multipliers in integers.

| Good | Unit | Weight | Volume | Hunger | Necessities | Niceties | Luxuries | Price |
|---|---|---|---|---|---|---|---|---|
| oats | bushel | 14.5 kg | 35 L | 18 | 18 | – | – | 100 |
| barley | bushel | 21.8 kg | 35 L | 28 | 28 | – | – | 120 |
| peas and beans | bushel | 27 kg | 35 L | 46 | 46 | – | – | 120 |
| vegetables | bushel | 20 kg | 35 L | 3 | 3 | 1 | – | 40 |
| apples | bushel | 18 kg | 35 L | 4.7 | 4.7 | 10 | 10 | 150 |
| dairy (butter, cheese) | pound | 0.45 kg | 0.5 L | 1 | 1 | 1 | – | 40 |
| beef | pound (salted) | 0.45 kg | 0.5 L | 0.6 | 0.6 | 0.3 | – | 10 |
| pork | pound (salted) | 0.45 kg | 0.5 L | 0.7 | 0.7 | 0.35 | – | 9 |
| mutton | pound (salted) | 0.45 kg | 0.5 L | 0.6 | 0.6 | 0.1 | – | 8 |
| fish | pound (dried or salted) | 0.45 kg | 1 L | 0.6 | 0.6 | 0.2 | – | 30 |
| seabirds | bird | 1 kg | 3 L | 0.5 | 0.5 | 0.2 | – | 20 |
| ale | gallon | 4.6 kg | 5 L | 1 | – | 4 | – | 60 |
| mead | gallon | 5 kg | 5 L | 1.5 | – | 4 | 4 | 300 |
| wine | gallon | 4.6 kg | 5 L | 1.5 | – | 4 | 8 | 600 |
| honey | pound | 0.45 kg | 0.35 L | 0.7 | – | 2 | 2 | 120 |
| herbs | pound (dried) | 0.45 kg | 2 L | – | – | 10 | 5 | 100 |
| wool | pound (fleece) | 0.45 kg | 3 L | – | – | – | – | 40 |
| beeswax | pound | 0.45 kg | 0.5 L | – | – | 5 | – | 150 |
| hides | hide (dried) | 10 kg | 30 L | – | – | – | – | 200 |
| cattle (live) | head | 250 kg | (herd) | – | – | – | – | 2,400 |
| pigs (live) | head | 80 kg | (herd) | – | – | – | – | 600 |
| sheep (live) | head | 35 kg | (herd) | – | – | – | – | 300 |

- **Weights and volumes** are standard approximations for research: a Winchester bushel is 35.2 L, and an ale gallon is about 4.6 L.
  Medieval measures varied.
- **Prices are r7's ×10**, so that the meats and the cheap goods have integer prices. The relative values are unchanged.
- **Live animals have no volume,** because they are not stored (§3.14). Their weight and size are for cargo later. Their price is the
  value of the live animal, which is above its meat.
- **Eating** is in proportion to what a pop holds (§3.7). r7's 60-day gate on niceties is withdrawn (r14).
- **Pop stores** hold 720 L per person: about 450 days of a normal diet at about 1.6 L per person-day (§3.15).

**The pop types:**

| Type | Dependent base | Births per worker per year | Deaths per year (workers / dependents) | Expects (N / W / L, %) |
|---|---|---|---|---|
| peasant | 400‰ | 67‰ | 20‰ / 60‰ | 100 / 0 / 0 |
| labourer | 400‰ | 67‰ | 20‰ / 60‰ | 100 / 30 / 0 |
| monk | 120‰ | 0 | 25‰ / 75‰ | 100 / 50 / 10 |

- **These rates** are about 40 births and 36 deaths per thousand people a year: about +0.4% a year when well fed at normal crowding,
  and decline under strain.
- **Dependents** die at 3 times the workers' rate, stored as `dependent_deaths_pm_year` (§3.8).
- **The dependent share** drifts back 10‰ of its gap a month, by moving whole people between the counts (§6.1).

**The response curves** (multipliers in ‰, linear between points):

| Curve | Points |
|---|---|
| births by food, N (r17) | 300→0, 500→200, 800→700, 1000→1000 |
| births by crowding, C | 0→1100, 1000→1000, 1500→700, 2000→400 |
| births by satisfaction, S | −100→850, 0→1000, +100→1100 |
| births by literacy | flat 1000 |
| deaths by hunger, N (r17) | ≥800→1000, 500→1200, 300→5000, 0→100000 |
| deaths by comfort, `min(1000, W + L/2)` | 0→1000, 1000→900 |
| deaths by crowding, C (squared for dependents) | 0→900, 1000→1000, 1500→1300, 2000→1800 |

- **Crowding** is each pop's crowding against housing (§3.8; owner-signed 2026-11-09). The old `C = people × 1000 / (usable cells × 8)`
  is withdrawn. With these housing values, place crowding starts at a median of about 1,170‰ (prototype estimate). Most of it falls on
  the unemployed, since the scenario has more peasants than farm jobs.
- **Needs** move 50‰ of the gap towards today's fulfilment each day. A dry store halves necessities in about 2 weeks, and deaths start
  rising around day 10.
- **The satisfaction baseline:**
  - −10 per 100‰ of necessities below expectation;
  - −3 per 100‰ of niceties below, and +2 per 100‰ above;
  - +3 per 100‰ of luxuries above;
  - −20 extra below 500‰ necessities, and +10 extra at 200‰ or more of luxuries above expectation.
- **Satisfaction** moves 100‰ of the gap to its baseline each month.

**The buildings:**

| | Subsistence farm | Monastic holdings |
|---|---|---|
| a level employs | 10 workers, plus their owner | 10 employees (monks or labourers), plus the priests |
| land per level | 2 usable cells | 2 usable cells |
| coverage | 1 owner per 10 workers | 1 priest per 5 monks or 10 labourers |
| `penalty_pm` | 500 | 500 |
| store | 100 L per employee (about 60 days) | 1,170 L per employee (about 730 days) |
| herd per level (§3.14) | 12 cattle, 8 pigs, 7 sheep | 12 cattle, 8 pigs, 7 sheep (farming method); none (fishing) |

**Output per level per year, at suitability 1000:**
- **The farm** (r10, recalculated with herds):
  - **crops:** oats 300 bu, barley 50 bu, peas and beans 6 bu, and vegetables 32 bu: about 7,170 person-days;
  - **the herd at its maximum** (§3.14):
    - beef about 1,100 lb, pork about 970 lb and mutton about 165 lb;
    - dairy about 860 lb;
    - wool about 10 lb and hides about 5;
    - in all, about 2,240 person-days, or about a quarter of the food, as the research suggests.

  That is about 9,400 person-days against about 6,700 needed by a level's people. Middling land (suitability 600–700) feeds them with
  little to spare (vision: "just enough, not much growth").
- **The holdings:** the same staples, plus ale 400 gal, honey 40 lb, mead 40 gal, apples 30 bu, herbs 20 lb and beeswax 10 lb. That is
  more value per head than a farm level. r7's "1,770 against 1,320" is stale (C5); the soak tool (§3.21) gives the real figures.
- **The fishing holdings** (Firm: fish, seabirds and a garden; Proposal numbers): fish 8,000 lb, seabirds 1,500, vegetables 20 bu and
  herbs 10 lb per level a year at suitability 1000. That is about 5,600 person-days against a 10-monk level's need of about 4,500. Sea cells count as suitability 1000 for
  fishing in M9.
- **Timing:** crops are paid at harvest. Fish and the rest are paid monthly, at a twelfth of the yearly figure. Herd products follow
  §3.14.

**The starting population by kind and size** (defaults; per-place overrides win):

| Kind | Size | Peasants | Monks | Labourers |
|---|---|---|---|---|
| monastery | 1 / 2 / 3 / 4 | 150 / 300 / 600 / 1,000 (some have none) | 20 / 50 / 100 / 200 | 20 / 40 / 80 / 150 |
| royal site | 1 / 2 / 3 | 300 / 600 / 1,000 | – | – |
| ringfort | 1 / 2 | 80 / 150 | – | – |
| crannog | 1 / 2 | 60 / 120 | – | – |

- **In all:** about 36,500 people across the 94 places (r17: recounted from the actual places, C5).
- **Pagan pockets:** 4 remote places, each with a pagan peasant pop of 10–20% of the place's peasants.
- **Mayo:** its monk pop is English.

### 3.13 Starting levels (Firm: guesses at building size, not derived from population)

**The levels** are authored per building, in the scenario's `economy.toml`. Kind and size give the defaults, per-place entries can set
any building (Firm: no strict rules), and every value goes into the guesswork log.
- **Island houses** (`skellig_michael`, `inishmurray` and any other with no usable land) have holdings levels. Their holdings use
  fishing (§3.4). An island may also have coastal farm levels where the scenario authors them (owner, 2026-11-09).
  - **Starting data fixed for housing** (owner, 2026-11-09: "Fix the starting data"):
    - Skellig Michael: monks 10 and labourers 10, fitting its 2 holdings levels (housing 28);
    - Inishmurray: a coastal subsistence farm of 3 levels (housing 51), 50 peasants, and monks 10 and labourers 10 on its 2 holdings
      levels. This replaces the earlier "Inishmurray authors none".
- **Farms:** where the guesses do not fit the land, the maximum of §3.4 cuts them.

| Kind | Size | Subsistence farm levels | Holdings levels | Storehouse levels (§3.18) |
|---|---|---|---|---|
| monastery | 1 / 2 / 3 / 4 | 8 / 15 / 30 / 50 (0 where there are no peasants) | 2 / 5 / 10 / 20 | 1 / 2 / 4 / 8 |
| royal site | 1 / 2 / 3 | 15 / 30 / 50 | – | 1 / 2 / 4 |
| ringfort | 1 / 2 | 4 / 8 | – | 1 / 1 |
| crannog | 1 / 2 | 3 / 6 | – | 1 / 1 |

- **Pops and levels do not have to agree.** A pop larger than its jobs has unemployed workers, and a building larger than its pop has
  open positions. Both are legitimate starting states.
- **Limits still bind.** Each method's levels must fit that method's own limit (§3.4): farming by usable land, and coastal and fishing
  levels by the shared sea budget. Authored levels that fit no method's limit are cut at load, and the load report lists them (owner,
  2026-11-09).
- **No new levels in M9.** A level is added only through construction, funded by the location's investment. That is a separate process,
  not yet designed, and out of scope (§5).

### 3.14 Herds (Firm, owner 2026-11-08; Proposal numbers)

**Herds belong to buildings.** Each subsistence farm and farming holdings keeps a head count per species: cattle, pigs and sheep. If
pops move, the herd stays. These are counts, not individual animals, so livestock stays abstract as the vision requires.

| | Cattle | Pigs | Sheep |
|---|---|---|---|
| maximum per level (Firm) | 12 | 8 | 7 |
| start (Firm) | 75% of the maximum | 75% | 75% |
| births, ‰ of the herd per year | 500 | 1,500 | 800 |
| deaths, ‰ of the herd per year | 80 | 100 | 80 |
| slaughter yield per head | 220 lb beef, 1 hide | 88 lb pork | 33 lb mutton |
| herd products | dairy 72 lb per head per year, by season | – | wool 1.5 lb per head per year, in June: `portion(sheep × 15, cov, 10 000, carry)` whole pounds |

- **Research:** *Críth Gablach* gives the *ócaire*, the smallest free farmer, 7 cows and a bull, 7 pigs and a sow, and 7 sheep. The
  per-level herd is a little above that.
- **Monthly, at noon on the 1st, per building and species:**
  1. **Births** are `portion(herd, birth_pm_year × graze_pm × season_pm[month], 10⁹, carry)`, by the season table (§3.19). Births scale
     with the land's grazing suitability for that species (§3.21).
  2. **Deaths** are `portion(herd, death_pm_year, 12 000, carry)`, capped at the herd.
  3. **The surplus above the maximum** is slaughtered at once (Firm). Its meat and hides are output of the building, and are paid out as
     wages in goods (§3.6). They follow the flow, so meat beyond full stores is lost (Firm).
  4. **There is no seasonal cull** (Firm).
- **Dairy** is set by the herd's size, not by workers or levels (Firm). Each month it is
  `portion(cattle × 72, season_pm[month] × cov, 10⁶, carry)` whole pounds, where `cov` is the coverage multiplier in ‰.
  - The carry is in millionths of a pound.
  - Because the season's shares sum to 1000 over the year, a constant herd of one cow at full coverage gives exactly 72 lb a year.
  - r10's denominator of 12,000 was a defect that gave 6 lb. The seasonal table (‰ of the year) follows the research (dairy from May):

  | Jan | Feb | Mar | Apr | May | Jun | Jul | Aug | Sep | Oct | Nov | Dec |
  |---|---|---|---|---|---|---|---|---|---|---|---|
  | 20 | 20 | 40 | 80 | 130 | 150 | 150 | 130 | 110 | 80 | 50 | 40 |

  Each month's figure is that month's share of the year, and the shares sum to 1000.
- **Every product is whole units.** Slaughter yields whole pounds and hides per head. Dairy and wool go through `portion`, with their
  fractions in saved carries, so a store only ever receives whole units.
- **Coverage applies to herd products too,** because an unmanaged herd is poorly kept (Proposal): births, dairy and wool are multiplied
  by the coverage multiplier.
- **Replenishing** (Firm): when a herd is below its maximum, live animals held by the building or its owner pops return to it. In M9
  the surplus is always slaughtered and nothing can be bought, so no live animals are ever held and herds recover through births only.
  Buying animals with spare goods comes with markets.
- **Later:** butchers turn live cattle into more meat than slaughter does, and buildings with room in their herds buy animals (Firm
  direction; out of scope).

### 3.15 Storage in real units (Firm: real units, weight and cargo size; Proposal: capacities by volume)

- **Every good has a unit, a weight and a cargo volume** (§3.12), stored as integers in grams and millilitres. Stores count units.
  Hunger and tier values are the food value of one unit; they are not the unit of storage.
- **Capacities are volumes:**
  - a pop's store, 720 L per person;
  - a farm's store, 100 L per employee;
  - the holdings' store, 1,170 L per employee.

  These are r7's 450, 60 and 730 days of a normal diet, at about 1.6 L per person-day, mostly oats.

  **An employee here is a worker,** not an owner (owner, 2026-11-10, `owner-direction-ck227-notes-r1.md`): a building's volume and its
  per-good limits count the workers it employs now, without the farm's owners or the holdings' priests.
- **Per-good limits (Firm, r15):** on top of the volume, each good has a storage limit **per person** for a pop, in its unit (data).
  - **Starting values:**
    - grain: oats 20 bu, barley 8 bu;
    - peas and beans 3 bu, vegetables 4 bu (r17: root crops kept in clamps; C5), apples 0.5 bu;
    - dairy 40 lb, and each meat 30 lb;
    - fish 30 lb, and seabirds 5;
    - ale 5 gal, mead and wine 2 gal each;
    - honey 5 lb, herbs 1 lb, wool 5 lb, beeswax 2 lb and hides 1.
  - **Representation (R15-F3):** each limit is a `u64` in **thousandths of a unit per person** (apples 500, oats 20,000).
  - **The caps:**
    - **a pop:** `cap_g = ⌊limit_m × size / 1000⌋` whole units;
    - **a building:** `cap_g = ⌊limit_m × employees × days_b / (450 × 1000)⌋`, with the **exact** ratio `days_b / 450`: 60 for a farm
      and 730 for the holdings. r15's 0.15 and 1.6 were rounded wrongly;
    - computed in `u128` and checked, with limits at most 10⁹ milli-units.
  - **Every receipt checks both the good's cap and the store's volume.** That covers wages, overhead, payday overflow, alms, the
    storehouse, drawing, movement, and butchering.
  - **What a refusal does:**
    - **drawing and movement** only shrink to what fits, or are skipped. They never destroy goods: what cannot be received stays where
      it was;
    - **payday overflow and butchering** past every cap are lost, as the owner decided (§3.6, §3.14).
- **Weight** has no use in M9. It is there for carts, merchants and ships later.

### 3.16 The local market (Firm direction, owner 2026-11-08; Proposal mechanism)

**Each place is its own market** in M9. No goods move between places until merchants exist.

**Every day at noon, as steps 1 and 2 of the economy's daily system (§3.9),** before any output is registered:
1. **orders** are computed, and **prices** move.

**Orders (derived each day, never saved):**
- **Buy orders, per pop** (Firm r14: from diet profiles):
  - **food (E6: one basis, the 240-day target):** `buy_g = max(0, ⌈share_g × need × 240 / (1000 × hunger_g)⌉ − holding_g)` units, where
    `holding_g` counts the store and the pop's account. `share_g` is the pop type's profile normalised for the place (§3.17), and `need`
    and `hunger_g` are in hundredths. r14's 30-day rule is removed;
  - **niceties and luxuries** (Firm, owner 2026-11-09, `owner-direction-tier-orders-r1.md`): the tier profile's goods, up to the
    type's expectation over the same 240 days, minus holdings. Exactly, for each tier `T` and each good `g` in the type's tier profile:
    - `G_T = ⌊exp_T × need × 240 / 1000⌋`, with `exp_T` the expectation in ‰ (§3.12);
    - `u_T,g = ⌈s_T,g × G_T / (1000 × v_T,g)⌉` units, with `s_T,g` the good's tier share in ‰ (§3.17) and `v_T,g` its tier value
      (×100); 0 when any factor is 0;
    - **a unit gives both tiers, so the target is the larger want:** `t_g = max(u_niceties,g, u_luxuries,g)`, and
      `buy_g = max(0, t_g − holding_g)`. A good also in the diet buys up to the larger of its food target and `t_g`;
    - sell orders are unchanged.
- **Sell orders (r17, C6: one basis with buying):**
  - **a pop (Firm, owner 2026-11-09, `owner-direction-market-notes-r1.md`):** **per food**, its holding above that good's target
    `T_g = max(food target, comfort target)`, the same target its buy order uses. So a pop never buys and sells one good on the same day,
    and a pop short of food overall still offers a food it holds more of than its diet wants. Goods with no hunger value are not sold by
    pops. (r17's "above a 240-day total, in proportion" is replaced.);
  - **a building:** its stockpile plus its communal account, above its owner pops' 60-day draw. **The draw covers each owner pop's whole
    size** (Firm, owner 2026-11-09), apportioned by largest remainder across the buildings where it holds owner positions, by those
    positions.
- **Each good is counted once (r19):**
  - the participants are **pops**, counting their store plus their own account, and **buildings**, counting their stockpile plus
    their communal account;
  - **the storehouse's physical stock is not a participant**: it is exactly the sum of the accounts, which are already counted by
    their owners.
- **One basis (r17):** buy orders are the shortfall below the same 240-day target, by diet profile. A place in balance sits near base,
  and the seasonal swing is real but bounded.
- **`k` starts at 500‰** (data, C5), so a fully one-sided market moves the target to 50% or 150% of base, well inside the clamps.

**Prices (saved, per place and good; r14 representation):**
- **Representation:** a price is a `u64` in **thousandths of a value unit** (`price_m`), always at least 1.
  - Values are `units × price_m`, in `u128`.
  - Quantities stay in units, and values stay in milli-units. They are never mixed.
- **Bounds:** base price 1 to 10⁶ units; `k` 0 to 10,000‰. Every intermediate is checked in `i128` or `u128`, and an overflow is a named
  refusal.
- **The target, in a frozen order:**
  1. `B` and `S` are the buy and sell values at the current price. If both are 0, the target is the current price.
  2. `r = ⌊(B − S) × 10⁶ / (B + S)⌋`, in `i128`, rounding towards negative infinity, so `|r| ≤ 10⁶`.
  3. `target = ⌊base_m × (10⁹ + k × r) / 10⁹⌋`, with `base_m = 1000 × base`.
  4. Clamp to `[max(1, ⌊base_m × 200 / 1000⌋), ⌊base_m × 5000 / 1000⌋]`.

  Example: base 120 with buy:sell 8:7 gives `r = 66,666` and a target of 127,999.
- **Smoothing:** the price moves towards its target by signed drift (§6.1), 100‰ of the gap a day, with a saved carry.
- **The start:** every price starts at base. `target_price` is the slice-2 primitive (§3.4), reused here.

**Barter is deferred to the merchants milestone (Firm, r17, C6).**
- r13–r16's barter rules are withdrawn, and so are the building buy orders that served barter.
- Alms and storehouses do the local work barter was meant to do.
- **Prices are still computed and shown** (§3.11, §3.22).

**What prices drive in M9:** method choice (§3.4), wage valuation and the posted wage (§3.6), and recruitment's comparison (§3.5).
Eating no longer uses prices (r14).

### 3.17 Diet profiles (Firm, owner 2026-11-08; Proposal shares)

**Each pop type has a diet profile** (data, in `economy.toml`): its share of food demand by good, in ‰ summing to 1000, and a tier
profile of the goods it wants for niceties and luxuries.

| Good | Peasant | Labourer | Monk |
|---|---|---|---|
| oats | 400 | 350 | 350 |
| barley | 150 | 150 | 150 |
| dairy | 200 | 200 | 200 |
| meat (beef, pork, mutton, equally) | 100 | 150 | 50 |
| peas and beans | 70 | 70 | 70 |
| vegetables | 50 | 50 | 80 |
| fish (coastal places only; elsewhere renormalised away) | 30 | 30 | 100 |

**Tier profiles:** labourers ale and honey; monks ale, mead, apples and herbs; peasants none. Each tier's goods have **fixed shares**
in ‰ (Firm, owner 2026-11-09; data, tunable), started at a holdings level's yearly output × the tier value, so a balanced place sits
near base. Equal shares were rejected: labourers would want about 18 lb of honey per 240 days against about 3 lb made a head a year.

| Type | Niceties | Luxuries |
|---|---|---|
| labourer | ale 952, honey 48 | honey 1000 (expectation 0, so no orders) |
| monk | ale 708, mead 71, apples 133, herbs 88 | mead 286, apples 536, herbs 178 |

The shares are from the research (cereals the bulk, oats before barley, dairy large, then meat). Monks eat more fish (fast days) and
labourers more meat.

**The profiles drive three things (Firm):**
1. **What the subsistence farm grows.**
   - Its mixed-farming method's annual crop food is a total in person-days per level, scaled by suitability.
   - That total is split among the crops it can grow (oats, barley, peas and beans, vegetables), by its worker pops' profiles
     renormalised over those crops, weighted by workers. The result is `cshare_g` in ‰, summing to 1000 by largest remainder in table
     order.
   - **Units (R14-F3; r17 by season):** each month,
     `units_g = portion(F × staffed × levels_m × cshare_g × season_pm_g[month], suitability_g × cov, levels × workers_per_level × 10⁹ × 1000 × hunger_g, carry_g)`,
     where `F` is the annual crop food per level in hundredths of a person-day.
     - Because a good's season shares sum to 1000 over the year, a full year at full staffing, suitability 1000 and full coverage gives
       `F × cshare_g / (1000 × hunger_g)` units: the whole crop, paid in its months.
     - The denominator is fixed per building and good.
   - The split is recomputed at each harvest from the pops then employed.
   - Herd products follow the herd (§3.14), not the profile.
2. **Demand:** the buy orders of §3.16.
3. **The start-up projection** of §3.4.

**Normalisation (R14-F3):**
- **Meat:** the profile's meat share is split equally between beef, pork and mutton, `⌊share / 3⌋` each, with the remainder to beef.
- **Inland places** have no fish share. The other shares are scaled up to sum to 1000 by largest remainder, in table order.

### 3.18 Storehouses (Firm, r17)

- **A simple local building at farms and monasteries,** owned by the pops of the building it serves.
  - It is the kiln and souterrain or barn of §2: worker-owned, like the subsistence farm.
  - Every farm and monastery has one at the start, with levels guessed like other buildings.
- **Levels and methods.** Each level is one kind of storage. Storage has no output, so §3.4's value score does not apply (r19):
  - **grain:** oats, barley, peas and beans;
  - **cellar:** dairy, salted meat, dried fish, apples, vegetables;
  - **drink:** ale, mead, wine.

  - **The choice (r19):**
    - storehouse levels are assigned at the start, **after** every producing building at the place has its methods, so there is no
      cycle;
    - each level, one at a time, goes to the kind with the largest remaining **projected annual volume**: the served buildings'
      projected output of that kind's goods, in litres, minus the capacity already assigned to that kind;
    - ties go grain, then cellar, then drink.

  A level adds storage capacity for its kind (volume, and the per-good limits of §3.15), proposed as 50 people's worth of a pop
  store.
- **Staff:** 2 workers per level from its owner pops. They are paid a small share of what is stored, 10‰ a month (data), in those
  goods. They are filled only from its owner pops, so a storehouse is never a movement destination (§3.5). **Its posted wage** (§3.6)
  records that fee as the month's pay per worker, and starts at 0, since its stock belongs to its depositors (CK-242).
- **The fee, one ledger (E2):** on the 1st, for each good and each account, `fee = portion(account_g, 10, 1000, fcarry)`. It is
  debited from both the account and the physical stock, then credited once to the storehouse workers' stores, through payday's overflow
  path. Physical stock always equals the sum of the accounts.
- **Accounts:** owner pops, the building's communal account, and **labourer pops** (Firm, r18) each have an account per good. Excess is
  credited to the pop whose excess it was (§3.6). A pop draws its own
  goods back when its food falls below 60 days, and its account counts towards its days of food (§3.7).
- **Public granaries,** owned by labourers and taking deposits from anyone, come later (owner).

### 3.19 Seasons (Firm, r17; Proposal tables)

Every product has a monthly table: its share of the year in ‰, summing to 1000. Pay follows production, and there is no special
harvest rule. r16's `season_months` is withdrawn: crops are paid in their months through the table.

| Product | Season |
|---|---|
| oats, barley, peas and beans | September 1000 (Firm: the harvest is in September) |
| vegetables | July to October, 250 each |
| apples | August to November, 250 each (Firm) |
| dairy | §3.14's table: all year, low but not zero in winter |
| herd births | March 233, April 234, May 233; January 34, February 34, June 34; July–December 33 each (sum 1000; Firm: herds produce in winter too, at a lesser rate) |
| wool | June 1000 |
| fish | May–September 160 each; January–April 29 each; October–December 28 each (sum 1000) |
| seabirds | May 334, June 333, July 333 |
| ale, mead | January–April 84 each, May–December 83 each (sum 1000) |
| honey, beeswax | August 500, September 500 |
| herbs | June–September 250 each |

### 3.20 Hot-path note for movement (C3)

Movement runs once a month per pop, after hiring. It visits only the pops with an open position to fill, so its cost is bounded by
open positions, not by pops.

### 3.21 Suitability and grazing (C5; Proposal values for tuning)

Per terrain, in the shared rules (`assets/game/economy.toml`, `[suitability.*]` and `[grazing.*]`; `rules_hash` covers that file and
the map's `terrain.toml`):

| Terrain | Crops | Cattle and sheep grazing | Pig forage |
|---|---|---|---|
| pasture | 700 | 1000 | 400 |
| forest | 300 | 300 | 1000 |
| bog | 150 | 400 | 200 |
| marsh | 200 | 600 | 300 |
| mountain | 150 | 500 (sheep), 200 (cattle) | 100 |

- **The cells a building holds** are the per-terrain counts it was assigned at the start, by the land order of §3.4 (holdings first,
  then the terrain with the highest score for the level's method).
  - **The score (r19)** is one number per method and terrain: the sum, over the method's outputs, of each output's base-price value
    share at full suitability (in ‰) × that output's suitability on the terrain, divided by 1000.
  - It is computed once from data. Ties go to the lower terrain index. Its suitability for a product is the cell-weighted mean over those counts, floored to ‰.
- Fishing uses 1000 until fishing grounds exist.
- **The coastal method's output per level, per year:** fish 8,000 lb and seabirds 1,500, at the season tables.
- **Island capacity for crowding** is withdrawn: crowding is housing (§3.8), and islands house people in their holdings and coastal farm
  levels.
- **A soak tool (C5, E5 contract):** one headless binary.
  - **Inputs:** the real scenario setup and the real economy systems, with no copy of the model, a fixed seed and N years.
  - **Output:** a CSV row per place and pop at noon on the 1st: size, workers and dependents, needs, food days, herds and prices.
  - **Determinism:** the same build, seed and N give byte-identical files.
  - **Staging:** it arrives with slice 4, when production and payday exist and nothing is eaten yet. Its columns grow as slices 5–7
    land, and the full loop exists after slice 6.
  - It is not an engine counter (owner: external debug tools).

### 3.22 Map modes (Firm, r17; a slice)

- **Stockpile:** each place's stored goods (pop stores, building stockpiles and storehouses) against their capacity, by volume, from
  red at 0% to green at 100% (Firm).
- **Expected yearly yield:** each place's projected annual food production, relative across the map: red at the minimum place and
  green at the maximum (Firm). Maps that weigh consumption come later.
  - **What counts as yield (Firm, owner 2026-11-09, rulings on CK-231 F3):** "herd is not food; herd can become food", so herds stay
    out of the yield calculation, with one refinement: **dairy counts, meat does not.**
    - **Counted:** each method's food output at the place's land suitability, at full staffing and coverage, plus dairy at
      `cattle × 72 lb a year` (§3.14) × dairy's hunger value. **Farming methods also take the place's farmland quality** (§3.24),
      `⌊annual × suitability × quality ÷ (1000 × 550)⌋` units per good, floored once, as production does (owner, 2026-11-10,
      `owner-direction-yield-fertility-r1.md`; CK-254). Sea methods and dairy take no quality factor, and a place with no land data
      counts at the reference quality. Dairy does not scale with land suitability, and a method's own dairy
      output is not counted a second time.
    - **Not counted:** beef, pork and mutton, and the herds' non-food goods.
    - **Later:** butcher shops, with constant production methods, will add meat to the yield map.
- **Values (E5), per place, in ‰:**
  - **stockpile:** `stored × 1000 / capacity`, by volume. A place with **zero capacity is uncoloured**;
  - **yield:** `(y − min) × 1000 / (max − min)` across the coloured places. If `max = min`, every place is **500**.
- **The reference rule (E5):**
  - the index grid is `Scenario::catchment_owners` as a `u16` per crop cell (`0xFFFF` for none). It is uploaded once per loaded map;
  - the value table is a `u16` per place, published with a **revision** in an immutable snapshot each time the values change (daily at
    most) or the mode changes, as the camera snapshot is;
  - **blending:** a cell's value is the floored integer mean of the values of the owned cells in its 5 × 5 box. Unowned cells and
    uncoloured places are excluded. A cell with no counted neighbour keeps its terrain colour;
  - **colour:** linear from red (0) through yellow (500) to green (1000), in integers;
  - the reference colour function gains this rule, so readback fixtures compare byte for byte on both backends.
- **The renderer:** the map pass reads the index grid and the value table. This landed on Metal and Vulkan together (CK-231), with
  readback tests, before the owner made Vulkan compile-only (§4). Later map modes land on native Metal, and Vulkan catches up in slice 11.
- **Switching:** three map-mode buttons on the UI, stockpile, yield and elevation (`docs/map-design.md` §12.5), select a mode; clicking
  the shown mode's button again turns it off. They sit in one row at the bottom centre of the screen (owner, 2026-11-10, CK-255).

### 3.23 Buildable land, footprints and development (Firm, owner-signed 2026-11-09; a follow-up slice)

**Status:**
- This is the design for **slice 9**, a follow-up slice. It replaces §3.4's sea-cell fishing cap (`sea cells / 4`).
- Slices 3–8 stay as written.
- Signed text: `land-footprint-design-r2.md` (sha256 186187ef), with the owner's answers.
- How development's parts combine stays the owner's. Crowding is now housing (§3.8, owner-signed 2026-11-09).
- **Since signed (owner, 2026-11-09):**
  - usable land is now computed fertility, with a computed climate (§3.24, and `docs/map-design.md` §12);
  - clearing land applies to buildings only;
  - prepared land and starting over the cap are below, under **Load and later rules**;
  - the calibration below was measured on the old usable layer.

**Fields and footprints are two limits (Firm).**

- **Fields** stay as they are. They are the usable cells around the place, and farming levels need them (§3.4). A larger radius at
  higher development is later.
- **Footprint** is a second, separate limit, on buildings:
  - every building level has a footprint, in square metres;
  - a place's footprints add up to at most its **maximum buildable land**;
  - it is a **hard cap**. Once a place is full, a building must be destroyed before a new one is built. Construction is designed later;
  - it applies at land and coastal places alike.
- **A farm level** takes many fields and a little footprint. **Set to fishing,** it keeps the same footprint and needs no fields.
- **Fishing is limited by footprint,** meaning housing, boats and a larger pier, not by fish. Early medieval fishing was far too small to
  overfish. The sea-cell cap (`sea cells / 4`, §3.4) and fishing grounds as a limit are withdrawn.
  - **Fishing requires fishing waters nearby** (owner, Q5): a fishing method, on the holdings or the farm's coastal method, needs at
    least one sea cell in the catchment. This decides eligibility only, never size.

**Footprint per level (Proposal).**

| Building level | Footprint | Basis |
|---|---|---|
| subsistence farm (mixed farming or coastal) | 2,000 m² | a farmstead: see below |
| monastery: its holdings level (farming or fishing) | 4,000 m² | Firm: more room than a farm (owner, Q3); twice a farm level |
| storehouse | 200 m² | Guess |

- **Subsistence farm.**
  - About 19 people per level (150 peasants on 8 levels).
  - A ringfort's interior is about 30 m across, and with its bank and ditch about 45 m, so about 1,600 m².
  - Deer Park Farms had double-walled round houses about 7.5 m across, several in one rath.
  - Rounded up for yards and outbuildings.
- **The monastery.**
  - **Owner, Q3:** "a monastary should take more room than a subsistance farm, the monsastic holdings should be wrapped up in the
    monstary building as a whole i think, unless they own other buildings, but that hasnt been implemented yet".
  - So the holdings are the monastery building, and its footprint covers the church, cells, workshops and enclosure.
  - Buildings a monastery owns elsewhere are not implemented.
  - **Check, Armagh:** its default levels give 18 ha (50 farm at 2,000 m², 20 holdings at 4,000 m²). That is a little above the
    largest early church enclosures, about 400 m across, which fits the largest church settlement.
  - **Check, Skellig Michael:** its 2 levels give 0.8 ha for its terraces, cells and gardens, on an island of 21.9 ha.
- **Storehouse:** a kiln, barn or souterrain.

**Maximum buildable land (Firm form; Proposal numbers).**

`max_buildable = development × (base + Σ over the catchment's land cells of b_terrain) / 1000`, in m², with development in thousandths.

- **`base` = 20,000 m² (2 ha)** at every place: the settlement core, so that islands and small catchments can always hold their house.
  - That is an enclosure about 160 m across, mid-range for early church enclosures (about 30–400 m; Leo Swan's aerial survey found one
    of about 110 m at Clogher, Co. Clare).
  - It was raised from r1's 1 ha so that the larger monastery footprint fits at the start (owner, Q3).
- **`b_terrain`** is the buildable allowance per land cell, of about 24 ha ground:

  | Terrain | m² per cell | Basis |
  |---|---|---|
  | pasture | 300 | see below |
  | forest | 250 | Guess: buildable once cleared |
  | marsh | 150 | Guess: flood-prone land; a Clare study finds few sites on alluvium |
  | bog | 150 | Guess: monasteries such as Clonmacnoise and Durrow stand on dry ridges among bogs |
  | mountain | 75 | almost no ringforts stand on high ground (the Clare study) |
  | sea, lake | 0 | |

- **Basis for pasture.**
  - Ireland averages just over one ringfort per 2 km², at about 1,600 m² each. That is about 190 m² per cell nationally.
  - Ringforts cluster on good grazing soils (Stout), so good land gets about 1.5 times the national figure.
- **Derived at load** from the catchment, like the land counts. It is never saved.

**Calibration: today's starting state against this cap (Guess).**

These are approximate figures from my own script over `terrain.png`, `usable.png` and the scenario's levels, with farm levels after the
fields cut. The engine's load report is the authority.

- **Occupancy** (starting footprints ÷ max buildable, at development 1000):

  | Statistic | Occupancy |
  |---|---|
  | median | 45% |
  | highest (glendalough) | 96% |
  | places over 90% | 4 |
  | places over 100% | 0 |

  So no starting level is cut by footprint.
- **Examples:**

  | Place | Occupancy |
  |---|---|
  | Armagh | 64% |
  | Clonmacnoise | 93% |
  | Glendalough | 96% |
  | Annaghdown | 78% |
  | Tara | 40% |
  | Cashel | 41% |
  | Skellig Michael | 40% |
  | Inishmurray | 40% |

- **Tuning (owner, Q2):** these values are a starting point, to be tuned once a run shows how the cap plays. Scaling the base and every
  `b_terrain` moves the median directly.

**Development (Firm hooks; parts reserved for the owner).**

- **The value:**
  - per place, in thousandths, starting at 1000 everywhere for now;
  - it scales the maximum buildable land (above), and so enters crowding only through the housing that can be built (§3.8).
- **Its parts:** the number of building levels at the place, plus a time part that **slowly drifts up**. Events can lower it.
  - Reserved for the owner: how the parts combine, how fast it drifts, and which events lower it.
- **Engineering placement** (ck-manager's notes):
  - the value is **saved**, because its drift and event losses cannot be recomputed;
  - buildable land and footprint totals are derived at load;
  - a **floor** keeps capacity from reaching zero;
  - diminishing returns from levels arrive with construction, so levels, development, capacity and people cannot run away.

**Load and later rules.**

- **At load,** authored farming levels must fit their fields (§3.4), and a level that does not fit is cut and listed.
- **Footprint never cuts** (owner, 2026-11-09: "being over the limit should be allowed, just not building over that limit"). A place may
  start over its building cap; the load report lists it.
- **Prepared land** (owner, 2026-11-09):
  - a place's building cap is `min(prepared, max_buildable)`;
  - `prepared` starts at the built footprint plus the 2 ha base, and is **saved**;
  - clearing raises it, using labour and time through a temporary building (construction, later). Pasture clears quickly, and bog is
    arduous and slow.
- **Newly built levels** (construction, later) are placed only if `footprint + new ≤ cap`. A place over its cap builds nothing until it
  is back under. Existing levels are never deleted to make room. A method switch keeps the footprint, so it never changes the total
  (§3.4's conversion rules).
- **The starting method choice ignores footprint.** A level's footprint is the same whatever its method.

### 3.24 Land fertility and climate in the economy (Firm, owner-signed 2026-11-09; slices 9a and 9)

- **The map side** is `docs/map-design.md` §12: the fertility formula, the computed climate, glaciation and the look.
- **Signed text:** `usable-land-proposal-r4.md` (23221a44) and `map-look-design-r4.md` (33f0615e).
- **The owner's decisions:**
  - a percentage per cell from a formula, with "river valleys ... closer to 100% and the more plain land ... closer to 50";
  - the fertility does "both" jobs: farm room and yield;
  - climate is computed now, and weather comes later.
- **Farm room:**
  - a place's field room per terrain is the sum of its land cells' fertility ÷ 1000, in cell-equivalents. It replaces the usable cell
    counts of §3.4;
  - a farming level on terrain *t* uses 2 cell-equivalents of *t*, the same `land_per_level` as before;
  - holdings still take land before the farm;
  - fishing and coastal levels need sea, not fields, and are limited by footprint (§3.23).
- **Farmland quality and yield:**
  - `quality‰ = ⌊Σ fertility ÷ cells with fertility > 0⌋` per place, derived at load;
  - farming output is multiplied by `quality ÷ 550` inside the exact production `portion`, so an average place yields what the earlier
    tuning gave;
  - the per-crop suitability (§3.21) is unchanged: it says which crops suit the ground, and fertility says how good the ground is;
  - fishing is unaffected.
- **Glaciers:** fertility 0 and building land 0 (`b_ice = 0`, §3.23).
- **Expected on the Ireland map** (prototype):
  - farmland quality runs from 14% to 78% across places, with a median of 51%;
  - Tara, Cashel and Emly are among the richest, and the bog-ringed Clonmacnoise and Durrow yield less per farm;
  - only Glendalough and Rahan run short of fields, and the signed terrain edits fix both;
  - the islands fish.
- **Not yet:** weather (year-to-year harvest variation) and per-crop climate limits.

## 4. Still open (next sessions)

- **Known limits (C8):**
  - population is capped by fixed jobs until construction exists;
  - wealth is capped by store volume until storehouses can grow, and public granaries arrive.
- **Construction:** how a location funds and builds a new level (owner, 2026-11-08: "a whole other process that we havent discussed").
  Already fixed (owner, 2026-11-09): a new level is placed only if its method has room, and existing levels are never deleted to make
  room. Conversion's cost, lock and room rules are in §3.4.
- What else feeds satisfaction (the vision's culture and religion relations come later).
- The literacy rule.
- How a monastery's specialty goods are produced (methods later, Firm).
- **Lua's role** (r5's Q1): which rules, if any, are written in Lua.
- **The slice plan** (dependency order, review r7 E2). Every slice is accepted except 11, which is parked:

  | Slice | Contents | Depends on |
  |---|---|---|
  | 1 | the cadence hour in `ck-sim` (§3.9) | – |
  | 2 | data, validation, maximum sizes and the starting state with its stockpiles; `GameState`, saves and `rules_hash` | – |
  | 3 | the location panel and its starting view: the first visible result | 2 |
  | 3b | the view's farm and holdings store capacity counts workers, as the receipts do (§3.15) | 3, 4 |
  | 4 | production by season, herds, wages, the payday flow with alms and storehouses; the soak tool | 1, 2 |
  | 4b | the posted wage: a 12-month payday history, seeded from the starting stockpile, saved and shown (§3.6) | 4 |
  | 4c | a storehouse's posted wage is its fee (§3.18) | 4b |
  | 4d | the storehouse fee runs on the 1st in step 5 of §3.9, after hiring and movement, not at payday | 4c, 6 |
  | 4e | each method takes its share of the building's staffing (§3.4), not staffed in method order | 4 |
  | 5 | eating with the intake curve, needs and satisfaction | 4 |
  | 6 | housing and crowding (§3.8), births, deaths, drift, hiring and movement (C9: before the market) | 5, 9 |
  | 6b | the recruitment range per building, monastic holdings 100 km, in ground distance (§3.5) | 6 |
  | 7 | local prices, computed and shown (no barter) | 5 |
  | 7b | market orders: one target per good, and the owner pop reserve (§3.16) | 7 |
  | 8 | the two map modes, on both backends (landed before the Vulkan direction) | 4 |
  | 8b | the yield map takes farmland quality for farming methods (§3.22, §3.24) | 8, 9 |
  | 9a | the computed climate and fertility map data, and glaciation (`docs/map-design.md` §12) | 2 |
  | 9 | buildable land, footprints, development fixed at 1000 (§3.23), and fertility's farm room and yield (§3.24); before CK-229 (slice 6) | 9a |
  | 10a | the map look: the fertility gradient, the hillside dither and glaciers (map-design §12.3–§12.4, §12.6), on native Metal | 9a |
  | 10b | the elevation map mode (map-design §12.5), on native Metal | 8, 10a |
  | 11 | the Vulkan catch-up: slices 10a and 10b, and any other milestone 9 render work, ported to Vulkan, with the Vulkan window tests repaired. **Parked until the owner says so**; not a milestone 9 gate | 10a, 10b |
  | 12a–12f | the owner's UI feedback: the map-mode bar at the bottom (12a), the window (12b) and UI scale (12c), the ledger (12d), cards and rows (12e), superscripts (12f) | 3, 8 |

  Slice 2 includes the starting method choice, with its two pure primitives (`target_price` and the diet-profile projection, §3.4)
  frozen there and reused by slice 6. Slices 1 and 2 can run together, and so can 3 and 4. Each slice carries the §6 coverage for what it adds.

  **Render work lands on native Metal only** (Firm, owner 2026-11-09, `owner-direction-vulkan-r1.md`). Until a native Linux machine with
  a real GPU is in the test loop, Vulkan only has to compile: a change to a shared interface or GPU data format updates the Vulkan code
  just enough for that. The milestone's Vulkan port is slice 11, one catch-up issue. The owner has parked Linux and slice 11 "until i say
  so" (`owner-direction-linux-r1.md`).

## 5. Out of scope for M9

- **Merchants and marketplaces:** trade between places, the marketplace building with its large stockpile, merchant pops, radius and
  transport. These are **the next milestone** (Firm, owner 2026-11-08), designed in their own session.
- Coin, taxes and tithes.
- Method conversion mid-game (its rules are recorded in §3.4), and unproductive monasteries (owner: later).
- Specific production methods, and method changes.
- Construction and expansion: new levels (the owner's separate process, §3.13).
- General migration. M9 moves people only to fill open positions, including from nearby places (§3.5).
- Political awareness, radicalism and movements.
- Literacy growth.
- Spoilage, blight and frost. Blight is an event later; the stores already let it bite.
- The Norse (the owner's event, later).
- **Fishing grounds:** a sea resource area with a fishing radius, and coastal buildings with other methods. Owner, Open; M9 uses the
  catchment's sea cells as a stand-in.
- Butchers, buying and selling live animals, and herds recovering through purchase.
- Villages and countryside farms.
- Any player command.

## 6. Engineering contracts

### 6.1 New in r8 (review r7 E1–E3)

- **The dependent share moves people between the counts** (R9-F2, r10). `workers` and `dependents` stay the authoritative counts.
  - **Each month,** the gap in thousandths of a person is `g = dependents × 1000 − size × base_pm`, signed, in `i128`.
  - **The step** is `t = g × drift_pm + carry`, with `drift_pm = 10`, and `step = t / 10⁶`, truncated towards zero. Then
    `carry′ = t − step × 10⁶`.
  - **The carry** is signed, in millionths of a person, and saved. A change of the gap's sign resets it to 0.
  - **The move:**
    - a positive step moves `min(step, dependents)` people from dependents to workers (children growing up, the sick recovering);
    - a negative step moves `min(−step, workers)` from workers to dependents.
  - **Small maturation is kept:** a gap of 6 people (6,000 thousandths) gives 0.06 a month in the carry, so one person moves about every
    17 months. Nothing is rounded away.
  - Births, deaths and recruitment change the counts directly (§3.5, §3.8). The share is always derived from the counts.
- **Signed drift** for needs and satisfaction:
  - `drift(value, target, numerator, denominator, carry)`;
  - with `d = target − value`, compute `t = d × numerator + carry` in `i128`. The step is `t / denominator`, truncated towards zero, and
    `carry′ = t − step × denominator`;
  - a reversal (the sign of `d` changes) resets the carry to 0;
  - the result is clamped to the value's range: tiers 0–1000‰, satisfaction −100 to +100;
  - each drifting value has its own carry, in units of `1 / denominator`.
- **Output arithmetic, per building and good.** `staffed = min(workers, limit)` (owners do not produce), and `cov` is the coverage
  multiplier in ‰:
  - **every good, each month (r17):** as §3.4, `portion(annual × staffed × levels_m × season_pm_g[month], suitability_g × cov, levels × workers_per_level × 10⁹, carry_g)`.
    The season shares sum to 1000, so a full year gives `annual` (r16's `season_months` is withdrawn);
  - each carry has a fixed unit for the building's life;
  - annual figures are data in each good's units (§3.12).
- **Curves:**
  - x values strictly increasing, and y from 0 to 100,000‰;
  - x outside the range is clamped to the end points (Proposal, owner: no objection);
  - interpolation is in `i64`, rounding towards negative infinity;
  - **rates, an exact rational with no saturation (R9-F1, r10 and r12):**
    - The factors are each curve's value in ‰, with crowding twice for dependents; dependents start from their own base rate, with no
      frailty factor. There are at most 5, and validation keeps each at or below 100,000‰.
    - `num = base_pm_year × Π factors`, exact in `u128`. It is at most 1000 × 10²⁵ = 10²⁸.
    - `den = 12 × 1000^(k+1)` for `k` factors. It is at most 1.2 × 10¹⁹.
    - **The people:** `whole = ⌊(pool × num + carry) / den⌋` and `carry′ = (pool × num + carry) mod den`, with `carry < den`, computed
      without overflow by splitting `num = q × den + r`:
      - `whole = pool × q + ⌊(pool × r + carry) / den⌋`;
      - `carry′ = (pool × r + carry) mod den`;
      - `pool × r + carry < 1.85 × 10¹⁹ × 1.2 × 10¹⁹ + 1.2 × 10¹⁹ ≈ 2.2 × 10³⁸`, inside `u128` (about 3.4 × 10³⁸);
      - `pool × q` and the sum are checked (§3.13); an overflow is a named refusal, never a wrap.
    - This is the same `portion` as everywhere else, evaluated exactly. There is **no saturation** and no floor before the final one, so
      the error is the single floor, and the fraction stays in the carry.
    - **Deaths are capped at the pool after `portion` (§3.8).** Whole deaths beyond the pool are discarded, and the carry keeps its
      fraction. Example: a true rate of 1.62 on a pool of 1 gives 1 death, and the carry keeps 0.62.
    - **Births are not capped** (r12 withdraws r11's implied one-birth-per-worker cap). Births use the same exact computation, and an
      overflow is a checked refusal like any other.
- **The conservation ledger, per good in its unit:**
  - initial units plus units produced (`portion`'s whole results) equal
  - pop stores, plus building stores, plus unpaid carries, plus units opened into the balances, plus units lost to overflow.

  Benefits, per balance:
  - units opened × each value equal
  - what remains in the balance plus what was eaten.

  "Potential" output is not "produced": units lost to overflow were produced, and then lost. Each system returns a ledger of produced,
  opened and lost units for its run. Tests sum it; it is not saved or shown, and it is not an engine counter.
- **Transfers in the ledger:** drawing, alms, the storehouse fee and movement move whole units between stores, so the per-good totals
  are unchanged. Units refused for lack of room are never moved. (Barter is deferred, r17.)
- **Herds in the ledger.** Per building and species: the start, plus births, minus deaths, minus slaughter (surplus and emergency)
  equals the herd now. The slaughtered heads × yield are counted as units produced for beef, pork, mutton and hides, so the goods ledger
  still balances.
- **The saved state:**
  - **every pop (r15 adds the per-good hunger pantries and the ticking carries; r16 has no eating carry):** head counts, the dependent-drift
    carry, the four balances, the three tiers, satisfaction, literacy, its store, its
    employment and every carry;
  - **every building (r15 adds the wage-split carries per good):** method, levels, store, herds per species with their birth, death and product carries, each
    output carry, the pay carries and the posted wage;
  - **recruitment** carries;
  - **every place:** its price per good, with each price's drift carry;
  - **every building:** its levels per method;
  - **every storehouse:** its accounts per owner pop and good.

  Orders are derived each day and are never saved.

  The maximum sizes, land counts and views are derived, never saved. Every data file used goes into `rules_hash` (shared rules) or the
  scenario hash (`economy.toml`, cultures, religions and starting entries).
- **Views:**
  - the UI view is built after **all** of that tick's systems, so at 12:00 it reflects that day's economy;
  - the callback views are built at their scheduler point;
  - the starting view comes from the starting state, before any tick, so slice 3 needs no live rules.
- **Save-load proofs** now include non-empty building stores, partly opened goods (non-zero balances), a kept posted wage, multiple pops
  per place and a monk pop of a second culture.

### 6.2 Kept from r5 (review-approved)

- **The remainder rule:** `portion(base, numerator, denominator, carry)`, with one fixed carry unit for each use, and every rate named
  with its period (r5 §3.1).
- **Exact staffing** in production, with no floor before the carry (r5 §3.5).
- **Conservation,** per good in its units and for food in person-days (r5 §3.12).
- **Data bounds and checked arithmetic,** with an all-or-nothing commit per place and an error pause (r5 §3.13).
- **Saves:**
  - `GameState` holds the economy, and a `rules_hash` joins the save header;
  - the build id gains `crates/economy` and `crates/map` (sources and manifests);
  - the code that derives any unsaved gameplay input is part of the build id (r5 §3.8).
- **The view:**
  - gameplay callbacks read a view built immediately before each call;
  - the UI reads `game.place(id, revision)` through a cache bounded by `UI_CALL_LIMIT + 1` views;
  - `economy_revision` rises on every new view, including a load (r5 §3.11).

## Sources

- [Princeton University Press: "Hidden Beginnings: From Cult to Conversion"](https://assets.press.princeton.edu/chapters/s10827.pdf)
- [Bernadette Filotas, pastoral literature and popular belief (PIMS)](http://www.pims.ca/wp-content/uploads/pdf/st151.pdf)
- [Penitential of Cummean](https://en.wikipedia.org/wiki/Penitential_of_Cummean)
- [Séamus Mac Mathúna, "Paganism and Society in Early Ireland"](http://www.ricorso.net/rx/library/celtica/M-Mathuna_S/Paganism.htm)
- [Fian](https://Www.wikipedia.org/wiki/Fian), and [The Heroic Age on fían and díberg](https://jemne.org/issues/10/johnson.html)
- [Kim McCone, *Pagan Past and Christian Present*](https://kimmccone.org/?p=51)
- [History Ireland: "Maigh Eo na Sacsan"](https://historyireland.com/?p=2709)
- [Mayo of the Saxons](https://www.mayo-ireland.ie/en/towns-villages/mayo-abbey/history/monastic-settlement-mayo-of-the-saxons.html)
- [Ní Mhaonaigh, "Of Saxons, a Viking and Normans"](https://www.repository.cam.ac.uk/items/46ac96bd-d487-4220-b357-d23a00642d60/full)
- [Rath Melsigi](https://en.wikipedia.org/wiki/Rath_Melsigi)
- [Cruthin](https://en.wikipedia.org/wiki/Cruthin), and [Kings of Dál nAraidi](https://en.wikipedia.org/wiki/Kings_of_D%C3%A1l_nAraidi)
- [The Conversation: Viking migration and Ireland's population](https://theconversation.com/viking-migration-left-a-lasting-legacy-on-irelands-population-122148)
- [Medievalists.net: population decline before the Vikings](https://www.medievalists.net/2019/08/irelands-population-declined-before-the-vikings-arrived-researchers-find)
- [Hannah and McLaughlin (2019)](https://pureadmin.qub.ac.uk/ws/files/172198429/Hannah_and_McLaughlin_PURE_version_1_.pdf)
- [Britannica video: early Irish foods](https://www.Britannica.com/video/183291/Saint-Patrick-discussion-foods-contemporaries-Ireland)
- [Waterford Treasures: food in medieval Ireland](https://waterfordtreasures.com/bringing-home-the-bacon-food-in-medieval-ireland-2/)
- [A food history of Maynooth, part 2](https://jpmcmahon.substack.com/p/a-food-history-of-maynooth-part-2)
- [UCD: Jessica Gleman, ale in early medieval Ireland](https://www.ucd.ie/archaeology/research/phd/gleman_jessica)
- [Trinity College Dublin FoodCult](https://tcd.ie/trinitylongroomhub/news/2024/foodcultthe-conversation-)
- [Searching for medieval ale (BYO)](https://byo.com/articles/searching-for-medieval-ale/)
- [Raths, Crannogs, and Cashels (encyclopedia.com)](https://encyclopedia.com/humanities/encyclopedias-almanacs-transcripts-and-maps/raths-crannogs-and-cashels)
- [Ringfort](https://en.wikipedia.org/wiki/Ringfort)
- [Stout, *The Irish Ringfort*, reviewed in History Ireland](https://historyireland.com/irish-ringforts-matthew-stout-four-courts-pressgroup-for-the-study-of-irish-historic-settlement-19-95-isbn-1-85182-300-x/)
- [Clare County Library: ringfort distribution in Bunratty Lower](https://clarelibrary.ie/eolas/coclare/archaeology/ryan/part1_ringforts_stoneforts_dist_factors.htm)
- [Deer Park Farms (encyclopedia.com)](https://encyclopedia.com/humanities/encyclopedias-almanacs-transcripts-and-maps/deer-park-farms), and [Excavations 1986](https://excavations.ie/report/1986/Antrim/0000631/)
- [Irish Field Monuments (National Monuments Service)](https://archaeology.ie/app/uploads/2025/01/irish-field-monuments.pdf)
- [Clogher, Co. Clare ecclesiastical enclosure](https://www.irishhistory.com/places/ecclesiastical-enclosure-clogher-co-clare/)
- [UNESCO: Skellig Michael](https://whc.unesco.org/document/154192)
- [Montgomery et al. 2015: Lough Swilly fish traps](https://pure.ulster.ac.uk/ws/files/11494816/Montgomery_et_al_2015.pdf)
- [FAO/IIASA, GAEZ 2007](https://www.fao.org/docrep/pdf/010/a1075e/a1075e01.pdf)
- [Seager et al. 2002: Europe's mild winters](https://ocp.ldeo.columbia.edu/res/div/ocp/gs/pubs/Seager_etal_QJ_2002.pdf)
- [Ohmura, Kasser and Funk 1992: climate at the equilibrium line of glaciers](https://www.cambridge.org/core/services/aop-cambridge-core/content/view/8DCF1C24E32A6781CC365853535CD16F/S0022143000002276a.pdf/climate-at-the-equilibrium-line-of-glaciers.pdf)

## Provenance

- 2026-11-09: §3.6's posted wage and §3.5's wage comparison (`owner-direction-posted-wage-r1.md`), and §3.16's per-food sells, single
  target per good and owner-pop reserve (`owner-direction-market-notes-r1.md`), the owner's choices with the designer.
- 2026-11-09: §3.16's tier buy orders and §3.17's tier shares, the owner's choices with the designer (`owner-direction-tier-orders-r1.md`,
  for CK-230, ck-manager `2k8btgqz6cgyebcpyvhwu2hu4a`).
- 2026-11-09: the owner's rulings, relayed by ck-manager: §3.22's yield definition (CK-231 F3, `sn2ctfm4skwzufr9ufhu8jnd9s` and
  `puf5ybyn8vj3zyrke5xnrgtqca`) and §3.10's starting needs (CK-228 B1, `tp3pvvw3a6d9dszdvhahifek9e`).
- 2026-11-09: the owner's Vulkan direction (`owner-direction-vulkan-r1.md`, relayed by ck-manager, `jryeeakppnwyirhsfet4ku97ia`): §4's
  slices 10a, 10b and 11 and the Metal-only rule, and §3.22's renderer note. Then `owner-direction-linux-r1.md`: slice 11 parked.
- 2026-11-09: §3.24 land fertility and climate (`usable-land-proposal-r4.md`, 23221a44; `map-look-design-r4.md`, 33f0615e), and §3.23's
  load and prepared-land rules (`usable-land-proposal-r2.md`, 5b2faf29), the owner's sign-offs with the designer. Slice 9a added.
- 2026-11-09: §3.23 buildable land, footprints and development (`land-footprint-design-r2.md`, 186187ef), the owner's
  sign-off with the designer; slice 9 added.
- r19, 2026-11-08, for review r18's five remaining items (`economy-design-review-r18.md`, 0ab757f5).
- r18, 2026-11-08: review r17's E1–E6, and the owner's two account answers.
- r17, 2026-11-08: the owner's session on the fresh review (`design-fun-review-r1.md`, 2d526c24). It covers C1–C10, storehouses, the
  location panel and map modes, and defers barter.
- r16, 2026-11-08, for design review r15's findings F1–F3.
- r15, 2026-11-08: the owner chose exact equal-share eating with per-good storage limits; review r14's four findings answered.
- r14, 2026-11-08: the owner's answers to review r13's model questions, and review r13's six engineering gaps.
- r13, 2026-11-08: the owner's decisions after review r12. These are methods by value per level, local prices and local barter, with
  merchants next.
- r12, 2026-11-08, for ck-reviewer-1's last R9-F1 check `rjjss6nf2uckyst9ydr66ybhks`.
- r11, 2026-11-08, for ck-reviewer-1's r10 checks `duspybsibgy9zxt95kpgufhyhs`.
- r10, 2026-11-08: the owner's decisions during review r9 (real units, herds, fishing gardens and the fishing rule, fishing grounds
  Open), and R9-F1 and R9-F2 (`economy-design-review-r9.md`, 561d36c2; verdict `gv8ijci35mzjc7ad3anvraxjj2`).
- r9, 2026-11-08, for ck-reviewer-1's r8 consistency checks `fym93ajy8x73f4fq5s5xnd27yw`.
- r8, 2026-11-08: the owner's answers to review r7's model questions, and review r7's engineering contracts (E1–E3).
- r7, 2026-11-08: the owner approved the numbers and set the starting levels as guesses at building size. Submitted for review.
- r6, 2026-11-08: the owner's direct session with ck-designer-1. It replaces r5's model and keeps r5's review-approved engineering
  contracts.
- r1–r5: see `economy-design-r5.md`.
