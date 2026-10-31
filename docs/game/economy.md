# Codekarl vision: Economy and populations

Part of [the vision](../vision.md), moved verbatim from it on 2026-10-21. Owner decisions are marked **Firm** with their date; everything else is **Open** or a **Proposal**, as there.

## Economy and populations

- **Firm.** Economy, populations, resources and trade lead the design. Variety and depth of pop types and resources should go well beyond
  Victoria 2/3: the owner's aim is *at least twice* what Victoria 3 has (the owner's estimate: about a dozen pop types and three dozen
  resources). **Open:** exact counts and lists.
- **Firm.** Pop groups earn income and hold savings and wealth. Pop type and circumstances decide how much is spent (adding buy orders for goods
  in the market) and how much is saved. A pop may save towards a specific investment.
  - *Example:* labourers spend most of their income as they earn it, for instance on alcohol and meat. Aristocrats spend lavishly but also
    save. These are behaviours, not fixed rates.
- **Firm (the novel part).** Unlike Victoria 2/3, where the crown builds and pops inhabit, **populations drive most building and
  expansion**. The crown is expected to spend mostly on other things while populations become rich or poor. The owner expects crown
  building, such as a king building farms, to be usually not the best use of money, but it remains a valid sandbox strategy (see the Lowlands
  example below). It is neither capped as rare nor guaranteed unprofitable. When a pop has accumulated enough to meet an investment's
  requirements (see "Building and converting levels" below), it spends them on a new building or a new building level.
  - **Firm (owner, 2026-10-18 12:49Z).** It starts a construction that takes time (see "Construction, method changes and failing
    businesses").
- **Firm.** Enterprises such as farms are modelled as buildings, much as in Victoria 3:
  - a **level** that sets the employment limit;
  - **production methods** with inputs and outputs;
  - **owners**: owner pops (aristocrats, merchants, clergy, or even peasants), or the crown or country when the player builds it. The crown
    is an owner, not a pop;
  - **workers** in several roles, *for example* mostly labourers plus some engineers and merchants.
  - **Firm.** A farm offers a **wage** derived from the cost of its inputs and the value of its outputs, leaving an overhead for its owners.
    Labourers are hired from the peasant population.
- **Firm.** **Subsistence farms** exist from the start, are buildings too (with production methods, pops and wages), and can grow or shrink for
  a variety of reasons.
  - *Example (historical, needs research):* in medieval times all farming is subsistence. Commercial farms appear when agrarianism develops,
    for instance in the Low Countries around the 1400s.
- **Firm.** Crown investment is possible but is not the default growth mechanism. The game is also a sandbox, and exploiting the simulation
  is part of the fun.
  - *Example:* a Lowlands player with agricultural modifiers stacks ahistorical farm investment, crashes the global price of food while
    staying profitable, and starves foreign farmers. This is desired emergent possibility, not a scripted outcome or a chosen market model.

### Populations (owner, 2026-10-18 01:35Z, 02:05Z and 02:22Z)

The owner: pops are "the thing that really makes Victoria 2 my favorite game".

- **Firm. A pop is a group of people sharing the same qualities.** It can be one person or a million. (This widens the earlier "from one
  person to thousands" under "Building and converting levels".)
- **Firm.** Each pop has an occupation, location, culture, religion, wealth, literacy rate, needs, consumption and job.
  - Wealth is not a number but a collection of goods and their quantities (see "Wealth and stockpiles").
  - **Open:** how occupation (pop type) differs from a particular job.
- **Firm (owner, 2026-10-18 18:53Z). Literacy grows naturally for now.** Formal education is a very late-game feature the owner does not
  want to design now.
  - The main literacy growth rate is based on cultural values: some cultures should value things like literacy, while others prioritise
    other things. (See "Cultural values and traits" under "Diplomacy, war and peace".)
  - Universities, libraries and monasteries should all contribute. *Owner's example:* a monastery farm would have farmers with noticeable
    literacy advantages over normal peasants.
  - Schools would have a large effect, but they will not come into place until the late 1700s at least (a game intent, not a historical
    finding).
  - **Open:** which cultural values affect literacy and how; how much each institution contributes; and the education system.
- **Firm.** Peasants on subsistence farms get very little income. A subsistence farm should provide just enough for a peasant to take care of
  their dependents, with not much growth.
- **Firm.** One farm can have several pops working there, all peasants, with different cultures or religions.
- **Firm. Promotion.** When a commercial farm has open positions, peasants "promote" based on their literacy and standard of living,
  becoming farmers instead of peasants. (Standard of living is derived, not stored: see "Satisfaction" below.)
  - They first look for an existing group of the matching pop type in the farm. If workers of the same culture and religion are there, the
    promoted people join that pop group, whose numbers are averaged, weighted by the number of people added.
  - The old pop's population count shrinks, but its averaged traits stay the same, since the people who leave are assumed to be "average"
    (in the owner's words, "the old pop shrinks, but the numbers should stay the same").
  - **Open:** what happens when no matching group exists (presumably a new group); exactly which qualities must match for people to join a
    group; and how the goods a pop owns are divided when part of it leaves (a split must not duplicate people or goods).
- **Firm (owner, 02:05Z). Dependents go with the workers who move,** in proportion. Different ratios of dependents to workers are supported.
  - *Example:* a subsistence-farm pop of 500 has 100 workers. When 10 workers are promoted to farmers, they take 40 dependents with them, so
    the farmer group gains 50 people. The dependents are promoted with them.
  - At the farm, these dependents slowly become workers as the young come of age.
  - The owner is not sure how to accomplish this. **Open:** how the dependency ratio and age make-up are represented, how dependents differ
    from the unemployed, and how people are actually hired into jobs.
- **Firm. Migration.** Each pop has a satisfaction value (see "Satisfaction"). If it drops too low, the pop is up for migration and tries to leave, in groups, to
  various nearby locations. At each destination the movers join a matching pop and adjust its numbers, as in promotion.
  - **Open:** the exact threshold, how destinations are chosen, and any legal limits on moving.
- **Firm (owner, 02:05Z). Satisfaction is separate from needs being met.** The point of keeping it separate is to represent turmoil from
  politics, culture clashes, religious groups and treatment.
  - Satisfaction defaults to 0 and can go positive or negative. It is affected by in-game events and by ongoing conditions.
  - **Standard of living is not a separate stored value.** It is derived from needs met and satisfaction together. ("Scratch standard of
    living.")
  - **Firm (owner, 02:22Z, agreeing with ck-guide's 02:11Z suggestion).** Satisfaction has an actual value and a baseline, and the value
    drifts towards the baseline at a rate scaled by how far it is from it. ck-guide's suggestion (not stated by the owner) was that ongoing
    conditions set the baseline and events cause temporary changes that fade.
  - **Open:** what sets the baseline, how effects combine, the drift rate, and the standard-of-living formula.
- **Firm. Cultural and religious relations.** Each religion has an opinion of every other religion in the game, and each culture of every
  other culture. Some culture groups feel differently about certain other groups, so the opinions need not be mutual.
  - *Example:* a farm employs 500 people of religion A and 500 of religion B. Both pops experience some satisfaction loss, scaled by the
    relationship between the religions or cultures. If both groups view each other positively there is very little change; if both view each
    other negatively, satisfaction can drop.
  - **Open:** how mixed or one-sided opinions combine, and how opinions change over time.
- **Firm. Political awareness** is a separate stored value. Political rights affect satisfaction only for politically aware pops.
  - Over the game, all pops should slowly become more politically aware. Nobles should become aware the quickest. The intended general
    progression of history: first emperors and kings rule, then the nobility take much of that power, then the clergy, then the merchants,
    and these groups become politically aware in roughly that order.
  - The owner's framing at 02:05Z: "many medieval serfs had no concept that they were being mistreated"; "political rights didn't exist".
    This is recorded as the desired historical flavour and progression, not a verified historical claim (it needs research).
  - *ck-guide's 02:11Z suggestion (not an owner decision):* low awareness should not mean people cannot recognise mistreatment; medieval
    peasants could voice grievances, and the English revolt of 1381 included demands to end serfdom. Awareness could instead govern whether
    grievances become organised political demands.
  - **Owner's clarification (02:22Z).** Recognising mistreatment is different, and the owner doesn't know whether it should be tracked at
    all: the point is that serfs knew there was really nothing they could do about it. No separate mistreatment trait is requested.
- **Firm (owner, 02:22Z). Population size matters to political threat.** Serfs should pose a threat, not because of political awareness but
  because of their numbers.
  - *Example:* merchants at satisfaction -10 and 50% awareness could be bad, but peasants at -10 and 5% awareness could be worse, because the
    peasant population can dwarf the merchants, especially early in the game.
- **Firm (owner). Political radicalism** is a separate trait from political awareness, and the two work in tandem.
  - High awareness with low radicalism: pop groups strongly affected by the political system, with important interests, but no real drive
    "to go crazy about it".
  - High radicalism with low awareness: groups radicalised by their conditions without much awareness of the greater politics. *Example:*
    the situation ck-guide mentioned, where serfs demanded rights in a revolt against the king.
  - Something like a revolution needs enough pops with enough awareness and enough radicalism.
  - Awareness and radicalism are both percentages from 0 to 100. They should slowly accumulate, but can also rise and fall through
    percentage increases and decreases. The owner asked whether this sounds like a good system.
  - **Open:** how population size, satisfaction, awareness and radicalism combine into a threat; the thresholds; how a local revolt differs
    from a revolution (low awareness is not a hard bar on all unrest); whether awareness and radicalism have baselines; whether changes are
    in percentage points or proportional; and the update rules.
  - **Open:** how rights map onto satisfaction.
- **Firm. Other satisfaction factors** include taxation, both of money (or goods) and the levy of manpower. *Example:* if the player sends a
  missionary to convert a location, that might sharply lower the satisfaction of every pop that does not follow the target religion. The
  owner expects "there could be a lot" of such factors.
- **Firm.** Pops become owners by building things (see above) or by buying existing things.
- **Firm. Growth** is influenced by needs met, satisfaction, literacy, and perhaps also things such as the food and land available in the
  location. Wealth should not be a factor in growth.
  - **Open:** how growth is calculated. Whether needs can be met can still depend on what a pop can afford.
- **Firm. Decline.** Pops can starve when their food needs are not met. Soldier and officer pops can die in war. Civilians can also die on the
  battlefield or when armies raze.
  - **Open:** the birth and death mechanics.
- **Firm. Pop types** the owner wants, "at the very least": peasants, serfs, farmers, labourers, clergy, soldiers, officers, tribesmen,
  merchants, shopkeepers, bankers, doctors, lawyers and artisans. There could be many more, some specific to particular cultures or political
  systems. The owner has "many more ideas here".
  - This is not a complete list, and it does not decide which types come first.
- *ck-guide's 01:09Z summary of remaining gaps (not owner direction):* populations; production over time; political authority and ownership;
  characters and succession; warfare's economic effects; research and historical change; player information and control; and Lua, mods and
  multiplayer. ck-guide suggested populations next, which the owner took up.
- *ck-guide's 01:36Z suggestions (not owner decisions):* distinguish identity (culture, religion and pop type decide whether groups merge),
  averages (literacy, satisfaction and similar traits, population-weighted) and totals (people, goods and ownership, transferred without
  duplication); keep pop type and employment separate, so a farmer can lose a job without becoming a peasant; discuss demotion and
  retraining later; decide whether legal status (serf), occupation (farmer) and service (soldier) are pop types or separate characteristics.
- *ck-guide's 02:11Z suggestions (not owner decisions):* distinguish children growing into workers from other dependents, such as the
  elderly, and from unemployed adults; base cultural and religious effects on actual exposure and population proportions, so splitting one
  population into several groups does not multiply the penalty; treat the crown–nobility–clergy–merchant progression as a broad tendency that
  circumstances can change.
- *ck-guide's 02:22Z suggestions (not owner decisions):* do not simply multiply awareness × radicalism × population, since zero awareness would
  make even a desperate population harmless; let awareness shape organisation and objectives while radicalism and dissatisfaction can cause
  unrest at low awareness; numbers give mass participation while political position gives influence (merchants withholding finance, peasants
  disrupting production); distinguish adding percentage points (20% to 30%) from proportional increases (20% to 22%), noting that purely
  proportional growth cannot rise from zero. Worked drift example: satisfaction -20 with baseline -5, moving 10% of the remaining distance
  per update, gives -18.5, then -17.15.

### Political movements (owner, 2026-10-18 02:32Z and 02:40Z)

- **Firm. Pops have interests based on their class,** on all sorts of issues. The country has laws, and some laws are in the interest of
  particular groups.
- **Firm. A cause.** If a pop decides to revolt, it looks at the laws and picks the one most against its group's interest; that becomes the
  cause it champions.
- **Firm. The player sees it.** The player is notified when people start talking, with a progress bar, a population count and the movement.
- **Firm. Reaching 100%.** The movement grows. If it hits 100%, an event is triggered, and the game works out the proper event: it could be a
  revolution, or perhaps a country-specific or situation-specific change.
- **Firm. Concessions.** If the king capitulates and changes the law, the movement is dismissed, but that could start another pop group
  fomenting. If the law is changed back, the original group resumes at the level of agitation it had reached, which then falls off slowly.
- **Firm. Joining forces.** If two separate groups both want to agitate and share a common interest, they join forces towards it and switch
  movements. (Membership is not exclusive: see below.)
- **Firm. Momentum.** Movements have momentum. High momentum attracts new pops, and events or player actions can increase or decrease it.
- **Firm. No fixed allegiances, with exceptions.** Pops have no allegiances. The exceptions are religious or cultural movements, for example
  from an ethnic group's dissatisfaction: pops of other cultures or religions should not join them.
- **Firm.** Political movements will be a fairly in-depth feature. The owner asked "does that all make sense?"
- **Firm (owner, 02:40Z). Part of a pop joins by splitting the pop.** If 200 farmers in a pop of 1,000 want to revolt, the pop is split into
  200 and 800, and the 200 are put in the movement. Splitting and joining pops will happen often.
  - This replaces ck-guide's 02:33Z suggestion to let part of a pop support a movement without splitting it.
- **Firm (owner, 02:40Z). A pop can belong to several movements at once.** *Example:* with a minority uprising in the east and a merchant
  uprising in the capital, merchants of the minority group in either location would be in both movements.
  - This replaces ck-guide's 02:33Z suggestion that each person supports one movement at a time.
- *ck-guide's suggestions (not owner decisions):* at 02:33Z, keep support (how many take part), progress (how close to forcing an event) and
  momentum (gaining or losing strength) distinct; let groups join forces only for the same concrete change, not merely dislike of the same
  law; let momentum attract only eligible, sympathetic pops; and use dissatisfaction and radicalism to decide whether a pop starts agitating,
  then its interests to choose the demand, since choosing the worst law always finds something. At 02:41Z, after the owner's answers:
  splitting must conserve people, dependents and goods; merging must not erase or spread movement membership; and supporters shared by
  movements count once in a joint action (200 people, not 400).
- **Open:** how movement membership affects splitting and merging pops; how goods are divided in a movement split, and whether dependents
  follow the proportional rule under "Populations"; how overlapping supporters are counted and how their actions are divided between
  movements; whether joined groups must demand the same law change or can share a general opposition; exactly who is eligible for a cultural
  or religious movement; how progress and momentum relate; what happens to progress when groups switch movements; how long agitation is
  remembered and how fast it decays; how the triggered event is chosen; and grievances that are not about a law.

### Wealth and stockpiles

- **Firm.** Wealth is a **mixture**. Economically important stores (for example livestock, grain and coin) are held physically, as stockpiles
  of actual goods. Smaller household possessions and exchanges are abstracted. The owner chose this option (the guide's option 3, 2026-10-17
  19:20Z). The owner judged a single abstract wealth value, with all surplus converted to it, workable but less accurate than stockpiles.
  **Open:** exactly which goods are stockpiled and which are abstracted.
- **Firm.** Pops stockpile what they produce. Grain-farming pops stockpile oats or barley, for example, and livestock-farming pops stockpile
  cattle, pigs or sheep. Stockpiled goods can be used directly for an investment's starting goods, or sold at the market price.
- **Firm.** Taxation is collected periodically. Who collects it depends on the law (see "Authority, ownership, laws and taxation").
  **Open:** the form (coin or goods) in each case.
- **Firm.** Pops accumulate coins when they can obtain them.
  - *Example (needs research):* in Ireland at the start, coinage is nearly worthless and becomes more valuable as the game goes on. **Open:**
    the mechanism by which a coin's local usefulness and value grow.
  - **Open (unresolved between two owner examples):** whether Ireland at the start has **no** coinage (the barter example under "Crown,
    military, church and history") or **nearly worthless** coinage (this example). Both are examples; neither is settled.

### Subsistence farming

- **Firm.** Subsistence farms choose their most profitable production method, subject to the starting-goods requirement for converting (see
  "Building and converting levels"), so a farm need not always be on the current optimum. Production methods are distinct choices.
  - **Firm (owner, 2026-10-18 12:49Z).** A building's owner decides its production method, so unless the business is crown-owned, the AI
    handles it (see "Construction, method changes and failing businesses").
  - *Example:* in Ireland, most of the country starts on livestock, with cattle, pigs and sheep as three distinct methods. As livestock
    consume grain and drive up its price, a proportion of farms switch to oats or barley.
- **Firm.** Terrain grazing capacity offsets livestock feed, and grain makes up the remainder. Grassy areas therefore favour livestock, and
  richer soils less suited to grazing favour grain. Soil and grazing suitability and changing grain prices influence each farm's choice.
- *Example (proposed content, needs research):* wheat and rye existed in early Ireland but were too expensive for pops to use as
  production methods.
- **Firm.** Subsistence farms, castles and monasteries also produce small secondary outputs.
  - *Example:* peas, beans, onions, leeks, cabbage, carrots and parsnips.
  - *Example:* monasteries produce more distinctive goods, such as herb or hop orchards, honey, mead and beeswax.
  - **Open:** the actual goods lists, which are proposed content needing research.

### Building and converting levels

- **Firm.** Opening a new farm level requires **all three** of:
  1. **Land:** suitable, empty land. Land is a hard, finite limit. Early in the game it should not run out, but a farm expands only into free
     space, and each farm has a maximum size. **Open:** how land capacity and maximum size are calculated.
  2. **Workers:** enough available population to staff at least a minimum for the new level. Full employment is not required, but a level does
     not open without enough people to work it.
  3. **Starting goods:** a stockpile of the goods the desired production method requires (*example:* 100 of that resource; the number is
     illustrative). Costs are generalised across goods. The goods must be in hand before the upgrade starts.
- **Firm.** Pop groups vary in size, from one person to thousands. A group includes workers, their dependents, and stragglers or the
  unemployed, so total population is not the workforce.
  - *Example:* a fully employed farm needing 1000 workers has a worker group of more than 2000 people. This is illustrative, not a ratio
    rule. The group must exceed its current employment by enough to break off a chunk for the new jobs.
- **Firm (a change from Victoria and EU5).** A production-method change requires the destination method's starting goods to be acquired and
  spent, the same as for an upgrade; it is not a free switch. One or more levels of a farm may switch independently. **Firm (owner,
  2026-10-18 12:49Z):** the change disrupts production (see "Construction, method changes and failing businesses"). **Open:** whether the
  conversion also has its own duration once the goods are in hand.
  - *Example:* if demand for a good collapses, a farm can become unprofitable and stay stuck. It recovers when its owners exchange their
    excess goods for the inputs the new method needs.
- **Firm.** An owner with coin but no goods can buy the starting goods. To expand an existing farm the owner still needs the workers, and
  to build a new farm it must find them. Land remains a hard limit.
- **Firm (desired access).** The starting goods should be purchasable at some price, wherever the market is. **Open:** what this means for
  availability, transport and lead time.
- **Open:** exact costs; construction times and their modifiers, and conversion duration; which stockpile funds an investment, and how
  ownership is attributed; how workers are recruited; market access.

### Construction, method changes and failing businesses (owner, 2026-10-18 12:49Z, 14:14Z, 14:22Z and 14:35Z)

Answers to ck-guide's three questions of 12:39Z (at 12:49Z), its three follow-up questions of 12:49Z (at 14:14Z), its questions of
14:15Z (at 14:22Z) and of 14:23Z (at 14:35Z).

- **Firm. Construction will be implemented, and takes time.** Each defined building has a base construction time: "basically just a timer
  modified by terrain type, etc."
- **Firm (the owner's ideal).** A construction job starts and acts like its own building, hiring workers to build it and using up resources
  like lumber and masonry.
  - **Firm (owner, 14:14Z). Construction has no set price.** The cost of a building is just the construction and wage costs of building it.
    The building's owner puts up the cost. If funds run out during construction, the construction goes on hold until the owner has the
    money.
  - **Firm (owner, 14:14Z).** Resource shortages on a construction job fall under the shared resource-shortage penalty (see below).
  - **Open:** how this fits the earlier rule that a new level's starting goods must be in hand before the upgrade starts (see "Building and
    converting levels"); whether materials are bought up front or as work proceeds; what a shortage of construction workers does; whether a
    paused construction keeps its progress, and whether a construction can be cancelled. *ck-guide's 12:49Z suggestion (not an owner
    decision):* treat the base construction time as the duration with sufficient labour and supplies.
- **Firm (owner, 14:14Z). A resource-shortage penalty applies to all buildings,** not only construction. The owner's initial numbers, "for
  now": 50%, an extra 25% for two resource shortages, an extra 12.5% for three, and so on.
  - *ck-guide's 14:14Z reading (not an owner decision):* the penalties are cumulative, 50%, 75% and 87.5% for one, two and three shortages,
    leaving 50%, 25% and 12.5% of normal activity.
  - **Firm (owner, 14:22Z). When a shortage applies.** When buy orders exceed sell orders by 2x, the shortage modifier applies, and it then
    scales. With absolutely none of the desired resource, output would be very low, perhaps even 0.
  - **Firm (owner, 14:22Z). Trace supplies.** This is why buildings like subsistence farms should provide trace amounts of resources, so
    there are little bits here and there and the hard cap is not hit (compare their small secondary outputs under "Subsistence farming").
    Which buildings supply which goods, and how much, is open; not every farm is assumed to make every good.
  - **Firm (owner, 14:22Z, adopting ck-guide's 14:15Z suggestion). Indispensable and efficiency inputs differ.** Inputs physically turned
    into the outputs are distinguished from inputs that improve production. A butcher with no cattle produces 0. A butcher with plenty of
    cattle but no tools would produce "something like 25%" of what it should (an illustrative figure).
  - **Firm (owner, 14:35Z). Stored supply protects a building.** Answering ck-guide's 14:23Z question, the owner agreed ("that was short
    sighted of me"): the penalty should not apply unless the building's stockpile of that item is empty. This is a condition added to the
    2x market trigger above, not a replacement for it.
  - The earlier 50%, 25% and 12.5% figures stay provisional. How they, the scaling with scarcity and the 25% example fit into one rule is
    open.
  - **Open:** the exact threshold and curve; which market's buy and sell orders count; how the market ratio relates to a building's own
    deliveries, and how the market trigger and the empty-stockpile condition combine; what happens when stock runs out partway through a
    tick; how several shortages stack; timing and recovery; and whether goods are still consumed while a building is penalised.
- **Firm. Changing production method disrupts production.** It is especially noticeable on farms: a farm switching from wheat to barley in,
  say, March would grow its yield over only a few months and drastically underperform that season; a change made right after harvest would
  have little consequence. The destination method's starting goods are still required (see "Building and converting levels").
  - **Open:** the exact conversion duration, and how yield is handled when only some of a farm's levels switch.
- **Firm. The building's owner decides the production method.** Unless the business is crown-owned, the AI handles production methods; for a
  crown-owned business, that owner is the player. This refines the earlier wording that subsistence farms choose their most profitable
  method (see "Subsistence farming"): that choice is the building owner's, made by the AI. It does not add any other player controls.
- **Firm. Economic failures begin firing workers.**
- **Firm. Partial employment is not just scaled output.** Having too many extra empty levels should carry a penalty.
  - **Firm (owner, 14:14Z). The penalty is material upkeep.** Each extra level adds some maintenance cost, on top of increasing the input
    amounts, and this upkeep increases linearly. Maintenance is based on the building's **total** level. (Of ck-guide's 12:49Z options,
    this adopts upkeep; reduced efficiency for the remaining workers was not chosen.)
  - **Firm (owner, 14:14Z). Extra levels also give an economy-of-scale bonus,** based on the **occupied** level, not the total.
  - *Owner's illustration:* a massive factory missing some workers would not be a disaster, since the economy-of-scale bonus can offset some
    of its costs, but a low-level building might cost a lot to maintain while empty.
  - **Firm (owner, 14:22Z: "correct i think"). Occupied levels follow staffing proportionally,** as ck-guide put it at 14:15Z: 600 workers,
    where each level needs 100, means six occupied levels, even if the building has ten.
  - **Firm (owner, 14:35Z). The bonus gives more output for the same input.** That means more meat for the same amount of cattle, a
    deliberate abstraction. This replaces ck-guide's 14:23Z suggestion that, for processes such as butchery, the bonus save labour instead.
  - **Firm (owner, 14:35Z). The bonus differs by building type.** Tooling factories can have a large bonus, but agriculture should have a
    small one: "farms more than pastures".
  - **Open:** how worker roles count towards staffing; the bonus size and any cap for each building type; how it interacts with labour;
    which outputs it applies to, including by-products; the maintenance amounts; and how production inputs scale with level alongside
    maintenance.
- **Firm. Only the building's owner decides to destroy a level of it,** so the AI decides for non-crown properties. No automatic
  deterioration or demolition is assumed.
  - *ck-guide's 12:39Z suggestion (not an owner decision):* a failed building stays available, so another pop could buy and reopen it
    without rebuilding. **Open.**
- **Open:** how a failing business chooses whom to lay off, and whether and when it rehires.

### Production over time (owner, 2026-10-18 03:54Z, 04:02Z, 12:37Z and 12:39Z)

- **Firm. Harvests depend on the product.** Crops are seasonal, harvested in summer or autumn according to the specific good. Some
  agriculture, such as cattle, is regular, slow production.
- **Firm. Stockpiles carry people between harvests.** They need to be large enough to last a season, and peasants should not sell what they
  need to survive the rest of the time.
  - *ck-guide's 03:54Z reading (not an owner decision):* only what a peasant can spare enters sell orders, so a full granary after harvest
    is not necessarily a large market surplus.
- **Firm. Livestock is abstract.** A cattle farm takes inputs and produces a certain number of cows every month; individual animals are not
  simulated.
  - Cattle can be sold as cattle, or processed at a butcher into leather, meat, tallow and milk (the owner's abstraction, with milk among the
    butcher's outputs).
  - Different animals produce different products in different quantities.
  - *ck-guide's 03:54Z note (not an owner decision):* processing consumes the cattle and produces the listed goods together, so the same
    cattle cannot also remain for sale.
- **Firm. Spoilage is wanted eventually but deferred from the first slice.** For now, all meat is assumed salted and grain sits in a granary
  without rotting. (This does not add a salt input or a preservation industry now.)
- **Firm (owner, 04:02Z). Wages.** Since farmers are paid in grain, at least early in the game, they are paid all at once. Each farmer pop
  then either exchanges the grain for what it needs or keeps it all in its own personal storage. The owner is "not sure how to represent
  that yet".
  - The owner also said: every worker gets a monthly wage based on the production output; no output means no wage.
  - **Firm (owner, 12:39Z). How the two fit.** ck-guide asked at 12:38Z whether "monthly wages" means production is checked each month and
    wages are distributed only when output exists, so crop workers receive a large payment in the harvest month and nothing in between,
    while cattle workers can be paid every month. The owner replied "yes that is what i envisioned" (one typo corrected). No accrued or
    guaranteed wage is assumed.
- **Firm (owner, 12:37Z). Workers who leave before harvest are not paid** from that harvest, for now, to keep it simple. Perhaps in the
  future some sort of early severance system can be worked out. No accrual or entitlement for earlier work is added now.
- **Firm. Seed is not tracked,** at least not yet.
- **Firm. Farm inputs are tools and fertiliser.** Fertiliser can also be produced by cattle.
  - The inputs need to be available every simulation tick. A shortage of goods slows the growth of the yield, leading to a partial yield
    at harvest.
- **Firm. Yield grows through the season.** After harvest a farm starts at 0% yield, and the figure increases every month, based on
  conditions, towards 100%. Most yields should end somewhere in the middle, with exceptionally good land capped at 100%.
  - This answers ck-guide's 03:54Z question about inputs before harvest: inputs are needed through the season, and the yield builds up
    month by month. Whether labour is among the conditions is open.
  - Inputs are checked for availability each tick, while yield is updated monthly; the amount of tools and fertiliser consumed or worn out
    per tick or per month is not decided.
- **Firm (owner, 12:37Z). Yield already gained can be reduced.** Answering ck-guide's 04:02Z question whether accumulated yield can fall,
  the owner said yes: blight and frost (the owner's word was "frostbite") should both play a role in reducing yield. Maybe not in the first
  slice, so whether they are in it is undecided.
  - Drought and raids were in ck-guide's question but not named in the answer; whether they also reduce yield is open.
- **Open:** how personal storage is represented; the exact formula for an output-based wage; how large household reserves are, what they
  contain, and how they relate to goods offered for sale; whether yield growth is scaled to the length of each crop's growing season; when
  blight and frost arrive, how much yield they destroy, and whether drought or raids do the same; how labour affects yield; how inputs are
  consumed; how goods are consumed when processing yields several outputs; and which buildings produce fertiliser from cattle.

### Markets and trade (owner's proposed model, under discussion)

- **Firm (preference).** The owner dislikes the predefined geographic market regions of Victoria and EU5, while acknowledging they may be the
  only practical approach. **Open:** whether the model below is feasible.
- **Proposed by the owner (2026-10-17 19:31Z, asked "do you think that this would work?"):**
  - Every economic location on the map (for example a farm or a castle) has its own table of per-good **buy orders** and **sell orders**.
    Its local price is derived from those quantities, scaling a shared per-good base value (refined at 19:49Z and reaffirmed at 20:03Z; see
    "Value and prices" below).
  - Buy orders represent production-method inputs, population consumption and exports. Sell orders represent production outputs and imports.
    A farm, for example, announces what it produces as sell orders at its location.
  - A network of **merchants**, each with limited resources and a home base, looks at the buy and sell orders within a radius of that base. Each
    merchant chooses the most profitable trades, given the local prices and the cost of travel.
  - Goods move by land or sea. By land, they hop in steps through merchants trading in their own areas, so a good may change hands dozens of
    times before reaching its destination. By sea, goods can travel between distant places, even across the world, in a single transport leg.
    A single leg does **not** mean the trip is instantaneous.
- **Proposed by the owner (2026-10-17 19:49Z, asking whether any of it contradicts itself):**
  - **Value and prices.** Each good has two values:
    - an **objective value**, the same for everyone: a per-good base value scaled by local supply and demand;
    - a **subjective value**, specific to each pop type: how important the good is to that pop.
    - *Example:* amber may cost far more than grain, but a pop with no grain should buy grain even at an exorbitant price rather than take a
      bargain on amber.
  - **Building stock is separate from pop wealth.** Each building has its own stockpile, distinct from its pops' wealth stockpiles. This
    stock is what is available for sale, and it fills and drains with the building's outputs and inputs. Granaries, storehouses and similar
    buildings, built by the player or by pops, increase its capacity, which lets merchants take bigger orders and drain a larger pool.
  - **Marketplaces buy on delivery.** The same kind of pool exists where merchants sell. A merchant bringing cattle to a town is paid
    right away, the cattle go into the town marketplace's pool, and the pool drains as users buy from it. The owner prefers that a marketplace
    always buys at its price when it has room for that good (the owner's words: "if it has an empty pool of that resource"). If a merchant
    arrives with goods it cannot sell, the merchant pop takes the loss and keeps the goods in its personal wealth, to sell later when there is
    room.
  - **Demand travels outward through merchants.** A merchant that sees demand at a marketplace (*example:* a city wanting barley) and cannot
    source it sets that demand at each marketplace in its radius. On the next tick, merchants who have one of those intermediate marketplaces
    in range, but not the original, see the demand and pass it on to the markets in their own range, without duplicating it. It is slow by
    design. The demand weakens as the merchants' costs eat into it, so far away it may be too small to act on.
    - *Example:* barley should not travel across Europe, but silks and spices are valuable enough to come overland from India to Syria
      before shipping to England.
  - **Merchant coordination is an open choice.** The owner sees it as a classic game-theory problem with many solutions:
    - process merchants one at a time, each acting in a world where earlier trades are already made;
    - run subsets in parallel and move merchants between sets on certain conditions;
    - since the simulation has perfect information while the merchants model players with imperfect information, a function could allocate
      merchants centrally by their utility for each trade.
    - The owner likes the idea that a merchant may trade without a buyer lined up, but expects that to be work-intensive. This is why the owner
      prefers marketplaces that always buy.
- **Refined by the owner (2026-10-17 20:03Z, asking "does that all make sense? what am I still missing?"):**
  - **Needs are hierarchical.** Pops have needs ranked with food most important, then textiles, then luxury goods. This ranking has nothing
    to do with a good's shared base value, which mainly separates expensive goods from cheap ones. Affordable substitutes that meet a
    comparable need matter.
    - *Example:* a starving Irish population might be willing to pay anything for wheat but cannot afford it, so it spends what little it has
      on oats or barley, comparable in nutrition and more readily available. Barley is cheaper than amber, yet to a starving farmer it is worth
      more: the farmer would pay more for barley than for amber, and would never buy amber at all; amber is for nobles and merchants.
  - **A marketplace is a building**, like a farm. It employs far fewer labourers and includes a collection of merchants. The merchant pops
    own it collectively, and its purchases are paid from those pops' savings. Marketplaces do all the work of trading goods.
  - **Merchants buy their own consumption at their marketplace**, in effect paying themselves (*example:* a fishmonger sells fish and buys
    wheat). The merchants act as a collective that shares its goods. The owner describes this as showing no change in the marketplace's
    stockpile, calls it a simplification, and asks whether it is valid.
    - **Open:** what "no change" means: internal payments that cancel out, or also no physical depletion of goods the merchants consume.
      ck-guide noted that consumption must still deplete holdings; that is a suggestion, not an owner decision.
  - **No duplicated demand; a better offer replaces an existing one.** When a merchant has placed a buy demand at a marketplace, another
    merchant first checks whether it can beat the existing order. If it can fulfil the trade better, the old merchant's demand is removed and
    replaced with the new order.
    - **Open:** what "better" means (price, quantity, partial fulfilment); how the original demand is kept separate from competing offers;
      and where commitment begins, i.e. whether an offer can be replaced after cargo is bought or dispatched.
  - **Competition follows ownership.** Two different marketplaces compete with each other. A merchant pop that owns several levels of the
    same marketplace does not compete with itself. Different merchant pops owning different levels of marketplaces in the same city competing
    with each other is a possible further refinement, not selected.

  - **No choice is made here.** ck-guide's suggestions in the same discussion (for example, price as an adjustment of the previous price,
    funded versus unfunded demand, reserving goods in transit, merchants owning cargo and bearing resale risk, circumstance-dependent need
    urgency, sequential merchant decisions in a varying order, forwarded demand carrying a quantity and a maximum delivered price) are
    proposals, not owner decisions.
- **Open:** all of it as a mechanism, including:
  - the price function, and how the objective and subjective values combine;
  - the merchant radius and how routes are reachable;
  - scheduling, competition between merchants, and the final merchant allocation mechanism;
  - given the proposed building sale stock and marketplace pools: the exact rules for how orders are backed by them, funded, reserved and
    fulfilled;
  - marketplace funding beyond the owner's proposal (merchant pops' collective savings): whether a marketplace can run out of purchasing funds
    while it still has storage room;
  - barter details: exact matching of goods, whose location's prices value an exchange when transport is involved, and exchange quantities
    (the settlement form itself is settled; see "Money");
  - what "an empty pool" means compared with remaining storage capacity;
  - pricing per transaction lot;
  - how forwarded demand keeps its origin, conserves quantity and attenuates;
  - reserved and in-transit supply;
  - transport duration;
  - scalability.
- **Open (questions from ck-guide not yet answered by the owner):** whether an owner can always obtain startup goods for some price, or
  whether supply and transport can delay or prevent delivery; and whether expansion or conversion takes time once supplies arrive.

### Money

- **Firm (owner, 2026-10-17 20:09Z; ck-guide's option 1).** Before coinage, settlement is an **immediate exchange of actual goods**, not a
  claim redeemable later. A farmer who needs something first decides whether it can spare goods to sell, then exchanges them for goods of
  equal value based on its location's demands.
- **Firm (by design).** Barter is deliberately inconvenient. Coinage should make exchange run more smoothly, which gives the player a reason
  to introduce coinage.
- **Firm (owner, 2026-10-17 20:28Z). Coins are minted from actual metal.** A ruler with metal deposits in its territory can open a mine,
  employ workers, build a stockpile of copper or silver, and then choose to mint coins.
  - *Example (needs research):* Ireland has little metal, perhaps one silver mine in County Tipperary and some scattered copper mines. The
    copper mines had been abandoned since the Bronze Age, and the silver mines did not open until the 13th century.
- **Firm.** The crown introduces coins into circulation by buying from merchants with them, and by accepting them as taxation instead of the
  goods tax.
- **Firm. Coins are distinct inventory goods**, identified by metal, weight, place of origin (mint) and minting date, instead of abstract
  "ducats".
  - *Example (illustrative label; historical detail needs research):* "silver penny (24 grain) Dublin 997AD minting".
- **Firm.** Foreign trade brings foreign coins, with their own weights and stampings, so a country can hold a mix of domestic and foreign coin,
  even in the medieval era.
  - *Example (needs research):* a Spanish galleon buying Irish fish in the 1500s leaves silver reals behind.
- **Firm.** A coin trades with a base value tied to the base value of its metal (silver, for example). The owner asked "what do you think
  about that system?"
- **Firm (owner, 2026-10-17 20:41Z).** The owner said "I agree with the format you put it at" in reply to ck-guide's 20:29Z message, and
  likes introducing **purity**. Coins keep the owner's explicit fields (metal, weight, mint and minting date), now with purity.
  - **Open:** which parts of that message "the format" covers. Candidates are the identity / material / acceptance framework, grouping
    identical coins by minting issue in the inventory, and the weight × purity × metal value rule. None of them is recorded as adopted
    until the owner confirms.
- **Firm. The player controls weight and purity when minting.**
  - Purity is a continuous setting, a slider.
  - Weight is a choice among denominations: a generic penny, plus culture-specific coin types for flavour. Countries should mint their own
    culture's coin.
  - *Examples (proposed content; metals, weights, dates and cultural associations need research, and the metal of each type is not assumed to
    be silver):* an Iberian culture mints reals of about 50–60 grains; a southern German culture the thaler, about 400 grains; Italians the
    florin or ducat; the English the groat and farthing; Ireland the generic penny.
- **Firm (desired consequences).** Lowering purity stretches a silver stockpile into more coins, at the risk of devaluing everyone's existing
  money: the owner sees this as simulating inflation. Large imports of newly mined, full-purity coin should also be able to disrupt a local
  monetary economy.
  - *Example (desired emergent possibility, not a guaranteed outcome):* Spanish galleons bring back huge amounts of silver, mint too much at
    full purity and spend it in Ireland. A local Viking king's carefully balanced economy, built on a lower-purity silver currency, sees its
    currency's value tank. If the Spanish dump their coins across Europe, it could even wipe out the country's economy.
- **Firm (intended behaviour, owner 2026-10-17 20:52Z; an example, not equations).** Debasement lowers the crown's costs while pumping more
  currency into the economy, and that has unforeseen consequences.
  - *Example:* the king pays soldiers 10 silver pennies a week at 95% purity, then debases new minting to 90% and still pays 10 pennies a
    week.
    - The crown spends less silver per payment.
    - The new issue is worth less. Merchants accept it but value it lower, so the soldiers can buy less with the same coin and buy less.
      They cut luxuries first.
    - Luxury businesses lose income. *For example*, a jewellery shop, which itself needs silver as an input, sees its income cut.
    - The owner also expects the older, purer coins to lose a little value as more coins enter the market.
    - The owner asked whether this covers the inflation behaviour they want.
  - **Open (conditional):** the drop in value of older, purer issues. A count of coins is not a quantity of fine silver, and lowering a new
    issue's purity does not change the metal in old coins. Whether and how old issues lose value is undecided: for example, through demand and
    supply for silver, or through a common denomination being accepted at one value.
  - **Open:** how nominal wage contracts in a denomination, per-issue exchange valuation, prices quoted in current pennies, and the common
    purchasing power of silver relate. This example lets merchants recognise and discount the new issue; no hidden-purity discovery
    mechanic is chosen.
- **Open: the causal mechanics.** Lowering the purity of a new issue does not change the metal in coins already minted. How these effects
  arise depends on undecided mechanisms: face value and tax valuation of denominations, acceptance and trust, local silver supply against
  purchasing power, and how prices respond. ck-guide's explanations of these are suggestions, not owner decisions.
- **Open (also):** fineness and metal content versus gross weight; exchange rates between coins; reminting; whether a ruler can obtain minting
  metal by trade instead of mining. Inventories keep mixed issues with their exact metal and purity.
- **Firm (owner, 2026-10-17 23:56Z). "Inflation" means the price of money,** not the modern sense of simply "rising prices". As the coins
  in circulation increase, the value of a coin goes down; as the silver and gold in a market increase, the value of the coins also goes
  down. Buying less with the same coins is the consequence. In the owner's words, it is "more like bartering with gold and silver at market
  price".
  - *ck-guide's reading (not confirmed by the owner):* purchasing power emerges from trading, holding, producing and consuming, with no
    separate inflation percentage driving it.
- **Firm (owner).** Silver and gold have real uses, such as jewellery, so coinage competes with them for metal.
  - *The owner's proposed causal example (recorded as stated; not a validated rule):* if the king mints all the silver into coins, the demand
    for silver goes up and the value of the coins goes down, making a jeweller much less profitable.
  - **Open.** ck-guide's response is a suggestion, not an owner decision: more demand for silver would normally raise silver's value, not
    lower the value of silver coins; minting moves silver from bullion into coin and creates no new fine metal; a jeweller's input costs can
    rise while the metal content supports the coins' value; coins could become cheaper than bullion if demand for coin is weak, and whether
    coins can be melted and reminted connects the two markets again; newly mined silver differs, because it adds metal. How bullion and coin
    are priced relative to each other, conversion between them, and monetary acceptance are undecided.
- **Firm (owner). Reserves.** The treasury is coinage removed from circulation. Companies and individuals often hold reserves as well, and
  reserves affect inflation.
  - **Open:** how. ck-guide suggests (not an owner decision) that held coins still exist but are not being offered for goods, so the
    quantities offered or spent, against those withheld, matter: hoarding tax proceeds takes purchasing funds out of private hands, and
    spending the reserve returns them.
- **Firm (owner, aspiration). Debt against reserves.** Debt can be created against these reserves: the king could borrow from financiers,
  with the limit scaled from the reserves. The owner wants to model this. The immediate-exchange barter above is unchanged.
  - *ck-guide's illustration and preference at 23:57Z (proposals, not owner decisions):* a financier holding 1,000 pennies lends the king 400
    and then holds 600 pennies and a claim for repayment; no coins are created. ck-guide preferred actual coins changing hands for the first
    lending model, and asked whether financiers' promises should also circulate as payment. The owner's answer follows.
- **Firm (owner, 2026-10-18 00:15Z). A loan is a note.** The borrower receives actual coins (or goods) and the lender receives a note of a
  certain value, much like a treasury bond, so it is more than coins changing hands.
  - A loan can be taken out in a specific minted coin, or even in a good. *Example:* a loan of 100 sheep, repaid at a price-adjusted,
    interest-applied rate, just as a loan of silver coins would be. The owner clarified what this means at 00:29Z (below).
  - Bonds have terms. When the term is up, the lender can redeem the bond.
  - The repayment obligation, the bond, can be sold to another bank or tag, or the borrower can default on it.
  - Lending is general: pops lend to other pops. It is not a tag-to-tag mechanic; a player borrowing is one case of it, with the tag standing
    in for the pop.
  - *Example (the player borrowing):* the king opens a diplomatic interaction with another entity, a tag or a bank. Banks hold reserves of
    coin. The king puts up its reserves as collateral and gets the coins from the bank; the bank gets the country's note.
- **Firm (owner, 2026-10-18 00:15Z, refined at 00:29Z below).** If the player keeps taking loans and the debt-to-reserve ratio gets too
  large, the player faces bankruptcy, with drastic effects on the country, or selling existing assets to cover the balance. The player can
  also refuse to pay the lender ("tell the lender to pound sand"), and a country's credit rating must then be downgradable.
- **Firm (owner, 2026-10-18 00:29Z). Repayment is fixed in the unit borrowed.** A loan of 100 silver pennies is repaid as 100 silver
  pennies, and interest is also counted in pennies, not in real value. If the value of a penny has changed between issue and repayment, that
  can favour either side, depending on whether prices rose or fell. The same holds for goods: the value of 100 sheep at issue will differ
  from their value at repayment. "It's all about the relative value to whatever the loan is being repaid in."
  - This replaces ck-guide's 00:16Z candidates (a fixed monetary value, or sheep-indexed coin).
  - **Open:** whether a loan in pennies names a particular minting issue or any penny. ck-guide suggested (not an owner decision) that a debt
    in a particular issue should not automatically be repayable with the same count of newer, less pure coins.
- **Firm (owner). Reserves stay spendable.** The borrower should not spend the reserves it has put up, but it can. That is where the
  bankruptcy or credit consequences come in, and they should be very destructive: mainly a lack of foreign investment, or forced repossession
  of land and goods. Plenty of leaders in history spent their countries into debt and then spent their reserves.
  - This replaces ck-guide's 00:16Z suggestion that pledged collateral cannot be freely spent.
  - **Open:** how repossession and loss of foreign investment are enforced, and how lenders monitor collateral.
- **Firm (owner). No automatic bankruptcy.** ck-guide's point that a high debt-to-reserve ratio creates vulnerability, not automatic
  bankruptcy, is "right". The player can run a Ponzi scheme with lenders, and it can succeed if the loans are navigated well. *Example:* the
  player wins a big war and demands gold; timed right, the country could be far past the point where bankruptcy would happen.
- **Firm (owner). Bankruptcy is a player choice.** The country goes bankrupt only if the player presses the "go bankrupt" button. It means
  "we are trying to responsibly settle this debt in a way that both parties can agree to", and it is "actually the right thing to do".
- **Firm (owner). What really damages credit** is lenders redeeming their treasuries all at the same time. If that exceeds the country's
  liquid holdings, the player must decide: accept the consequences of not paying back a loan, or go bankrupt and lose a somewhat reduced
  amount of less-liquid capital, perhaps ownership in various buildings.
- **Firm (owner).** The crown should own some businesses. These are non-liquid assets, and their ownership can be exchanged in a bankruptcy
  deal. The owner asked "what do you think about all of that?"
- **Firm (owner, 2026-10-18 00:41Z). Bankruptcy begins negotiations.** The owner agreed with ck-guide's 00:30Z suggestion that the button
  begins negotiations rather than guaranteeing a settlement. The player then chooses the assets they are willing to give up, until the major
  parties agree. "We will have to figure all of that out."
- **Firm (owner). Selling a bond is the normal way for a creditor to get money before maturity.** The holder sells the treasury on the market
  to a lender willing to take on the debt. Its price could be calculated from the value of the goods at the time the bond changes hands; the
  bond's interest and value (its terms) do not have to change at all.
- **Firm (owner). Early redemption carries a penalty.** A creditor can demand early repayment, but only for a fraction of the original value:
  there must be a penalty for not waiting for maturity. Implementing early repayment could be considered later.
- **Firm (owner). Maturity does not force redemption.** A bond that reaches maturity is not automatically redeemed or sold. Banks, for
  example, would be wise to hold treasuries from successful countries. Bonds are redeemed only when the AI decides it needs the money for
  something, and redeemed early only when it needs the money badly.
  - *Desired outcome (owner):* if a bond is in a nation's own currency and that country handles its monetary policy well, both sides benefit:
    the bond's value goes up, while the reserve growth dwarfs the collateral it was borrowed against.
- **Firm (owner). Buybacks.** Before the term ends, the player can attempt a bond buyback, which the bank can accept or refuse. The owner
  asked "does that all sound right?"
- **Firm (owner, 2026-10-18 00:46Z). Interest continues after maturity,** agreeing with ck-guide's 00:43Z point that otherwise currency
  appreciation alone gives a holder no reason to keep the bond.
- **Firm (owner). Each borrower has a treasury interest rate based on its credit rating.** A bond is locked into that rate when the treasury is
  taken.
  - At 00:46Z the owner was unsure what happens to the rate once the bond has matured (a tentative idea: a fixed interest payment after the
    term, then scaled by the current treasury rate) and asked what the historical standard was. This was settled at 00:50Z (below).
  - *Research context from ck-guide (not selected contracts, and not evidence for eighth-century Irish institutions):* medieval perpetual
    rentes, redeemable by the issuer, unlike dated loans
    ([Munro](https://mpra.ub.uni-muenchen.de/10925/2/MPRA_paper_10925.pdf)); a 1648 Dutch water-board perpetual bond that still pays interest
    ([Yale](https://news.yale.edu/2015/09/22/living-artifact-dutch-golden-age-yale-s-367-year-old-water-bond-still-pays-interest)); modern
    fixed-rate gilts, which pay the principal and final coupon at maturity ([DMO](https://dmo.gov.uk/responsibilities/gilt-market/about-gilts/)).
- **Firm (owner, 2026-10-18 00:50Z; adopting ck-guide's 00:48Z recommendation). An existing bond's interest stays fixed** and continues
  until redemption. The borrower's current creditworthiness sets the rates on new borrowing. Changes to an existing bond come through
  negotiated refinancing or buybacks, not an automatic reset after the term. This replaces the owner's tentative 00:46Z scaling idea.
  - *ck-guide's illustration (not an owner decision):* the crown borrows 1,000 pennies at 8%, paying 80 pennies a year. If new lenders later
    accept 4%, the old bond still pays 80 and the crown has a reason to buy it back or replace it; if they demand 12%, the old bond still pays
    80, its resale price may fall, and an eligible holder may ask for repayment. ck-guide also suggested calling the date "redeemable from".
- **Firm (owner). Loans are written with different terms.** The different debt concepts are captured as terms of one loan system. The loan
  screen defaults to the most common type of loan, and the terms and rates depend on what the borrower wants and what the lender is willing to
  accept.
  - The mechanics already recorded (transferable notes, repayment in fixed units, bankruptcy negotiation, buybacks, the creditor's full
    redemption after the term) stay as the baseline unless the owner revises them. The historical arrangements are available concepts; which
    of them are needed first is open.
  - *ck-guide's proposals (not owner decisions):* the default offer is the arrangement above (a quantity of coins or goods, fixed interest
    until redemption, a minimum term before the creditor can demand full repayment). Negotiable terms could cover principal and denomination;
    interest and payment schedule; redemption (minimum term, notice, early-exit penalty); security (collateral and reserve commitments); and
    transfer and buyback rights. The lender evaluates the whole offer, using the credit rating as a starting point. The screen shows a
    complete default offer with an optional "change terms" section; the lender accepts, rejects or counteroffers, giving the main reason.
  - **Open:** the exact default contract, which variants are supported first, the screen's controls, how lenders price offers, and which
    combinations of terms are allowed.
- Three different transactions are recorded: a holder selling the bond to a third party, a creditor redeeming it from the borrower, and the
  borrower buying it back.
- **Open (lending):**
  - how the treasury rate follows from the credit rating, and the payment schedule;
  - the redemption process: notice, payment window and grace periods; whether early redemption is a right in every contract or needs
    agreement; the size of the early-redemption penalty;
  - how a bond's sale price is set, including the borrower's risk and how easily the bond can be sold;
  - how a bankruptcy negotiation works: who the "major parties" are, how they agree, what creditors must accept and what they recover;
    pressing the button does not guarantee a particular reduction;
  - how missed payments and default are handled and enforced, as distinct from a bankruptcy settlement;
  - what a lender loses on default or bankruptcy, and whether repayment can be made in a different coin or good;
  - whether reserves can be pledged to several lenders, and not counting pledged or lent wealth twice;
  - how credit ratings are computed, how severe the damage is, and what ratings affect.
  - A transferable note does not by itself make notes general means of payment or create spendable bank deposits; whether any note or
    promise circulates as money is undecided. The immediate-exchange barter before coinage is unchanged.
- The owner asked whether such an interconnected economic simulator is possible with this system. ck-guide's view is that it is; no design or
  mechanism is chosen.
- **Firm (aspiration).** Money is not necessarily one universal currency, such as EU4's ducats. The desired scope includes barter, minted
  coinage, possibly coinage issued by third parties such as banks, and currencies simulated as goods whose value depends on supply.
  Over-issuing should crash a currency's value, as over-bartering a good does.
  - **Open:** how this earlier over-issuing aspiration fits metal-backed minting. The owner's purity control (above) is one route; the
    mechanism is open. Coinage is not fiat-only.
  - **Open.** Whether that is feasible, and every mechanism: pricing units, credit and debt (see "Debt against reserves" above), banks, and
    how and where coin is accepted. Market
    geography is covered under "Markets and trade" above.

