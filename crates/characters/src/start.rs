//! The 768 start (CK-370, characters slice 8 §3): the real figures, their families, the slots, the burn-in and the known strands, in the one
//! order the spec gives. Everything is deterministic from the master seed (`World::seed`) and the data; nothing reads the clock.
//!
//! 1. The **economy additions** are scenario data (`economy.toml`); the **safety net** is the `Populations::make_room` hook, which a
//!    [`Ledger`](crate::Ledger) built [`with_start_safety_net`](crate::Ledger::with_start_safety_net) answers for the start only.
//! 2. **Where generated living relatives live** is [`CharacterStore::begin_start`]'s rule (`generator::figure`): beside a monk pop, or abroad.
//! 3. **Ancestry, oldest first**: the figures and the dead parents are made so that every parent precedes its children (ties by birth, then
//!    key); a figure dead before the start is made last, so that the strands it holds name characters that exist.
//! 4. **Households**, the named spouses linked first, then slice 7's `complete_household` for every living figure.
//! 5. **Slots**, after every household exists: `lord` first, the other members, then the spouse places.
//! 6. **Generated pasts stop at 752** (`begin_start`'s `past_until`).
//! 7. **The burn-in, 753-767**: one yearly simulation for everyone, with no deaths and no births.
//! 8. **Known strands** between two live characters are set after the burn-in.
//! 9. The game starts on 9 October 768.
#![deny(clippy::float_arithmetic)]

use crate::{
    ArchiveSink, CharacterStore, Departure, Error, Office, PopLocator, Populations, UNKNOWN,
    connections::is_bond_kind,
    events::{EventError, FirstError, LogSink, World, conclude, keep},
    figures::{DeadParent, Figure, FigureData, FigureError},
    generator::{Birth, End, Fixed, GenerateRequest, HouseholdMinds, Member, SlotRequest, dates},
    genetics::below,
};
use ck_economy::EpithetScope;
use ck_sim::{GameTime, Stream};
use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

/// The last year of the generated pasts; the years after it, to the start, are the burn-in's.
const PAST_ENDS_IN: u16 = 752;
/// The years of the burn-in (753-767).
const BURN_IN_YEARS: u32 = 15;
/// The importance of a byname that matches no model of `epithets.toml`.
const DEFAULT_BYNAME_IMPORTANCE: u32 = 40;
/// The year recorded as the last growth of an authored strand between two live characters: the last year of the burn-in.
const STRAND_YEAR: u16 = 767;
/// How many draws a spouse place's generated spouse is tried on at each tier of her age window.
const CANDIDATE_ATTEMPTS: u32 = 4;
/// The tiers of her age window: the pool's own, without the fertile ages, and the marriageable minimum alone.
const CANDIDATE_RELAXATIONS: u8 = 3;

/// What the start made, for the start log (CK-370 §3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StartReport {
    /// Real figures made live.
    pub figures_live: usize,
    /// Real figures made archived (dead before the start).
    pub figures_archived: usize,
    /// Dead parents archived.
    pub dead_parents: usize,
    /// Characters live at the end of the start, generated ones included.
    pub live: usize,
    /// Archived records made by the figures, the households and the slots (dead figures, dead parents, generated dead ancestors and **abroad**
    /// relatives); the ancestors of a spouse generated for a spouse place are not counted.
    pub archived: usize,
    /// Of those, the generated relatives held **abroad**.
    pub abroad: usize,
    /// Households completed.
    pub households: usize,
    /// Slot places filled, by existing characters and by generated ones.
    pub slot_places: usize,
    /// Slot places left unfilled, and the warnings that say why.
    pub warnings: Vec<String>,
    /// The known strands set between two live characters.
    pub strands: usize,
    /// The dependents the safety net added, by pop (CK-370 §3 step 1).
    pub raised: Vec<(PopLocator, u64)>,
    /// The first failure after a commit (a life-log line that could not be appended): the start stands.
    pub output: Option<String>,
}

impl StartReport {
    /// The lines of the start log: one for the totals, one for each pop the safety net raised, and each warning.
    #[must_use]
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "characters start: {} real figures ({} dead before the start), {} dead parents, {} households, {} slot places; {} live characters, {} archived ({} abroad), {} known strands",
            self.figures_live + self.figures_archived,
            self.figures_archived,
            self.dead_parents,
            self.households,
            self.slot_places,
            self.live,
            self.archived,
            self.abroad,
            self.strands
        )];
        for (pop, count) in &self.raised {
            lines.push(format!(
                "characters start: pop {}/{} raised by {count} dependent(s) to hold its characters",
                pop.place, pop.slot
            ));
        }
        for warning in &self.warnings {
            lines.push(format!("characters start: {warning}"));
        }
        lines
    }
}

/// A refusal of the start: nothing of it is kept (the caller drops the store, the archive and the logs).
#[derive(Debug)]
pub enum StartError {
    /// The figures do not load, or do not join the scenario's pops.
    Data(FigureError),
    /// The engine refused a creation, a household, a slot or a year.
    Engine(EventError),
    /// A slot that cannot be filled as the data says (a spouse place with no subject).
    Slot(String),
}

impl std::fmt::Display for StartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Data(error) => write!(f, "{error}"),
            Self::Engine(error) => write!(f, "the start was refused: {error:?}"),
            Self::Slot(problem) => write!(f, "{problem}"),
        }
    }
}

impl std::error::Error for StartError {}

impl From<FigureError> for StartError {
    fn from(error: FigureError) -> Self {
        Self::Data(error)
    }
}
impl From<EventError> for StartError {
    fn from(error: EventError) -> Self {
        Self::Engine(error)
    }
}
impl From<Error> for StartError {
    fn from(error: Error) -> Self {
        Self::Engine(error.into())
    }
}

/// What a slot is filled from: the data, every pop of the map and the identifiers made so far.
#[derive(Clone, Copy)]
struct Joined<'a> {
    data: &'a FigureData,
    pops: &'a [(PopLocator, String, String)],
    ids: &'a BTreeMap<String, u64>,
}

/// A person the start makes from the data.
#[derive(Clone, Copy)]
enum Node<'a> {
    Figure(&'a Figure),
    Dead(&'a DeadParent),
}

impl Node<'_> {
    fn key(&self) -> &str {
        match self {
            Self::Figure(figure) => &figure.key,
            Self::Dead(parent) => &parent.key,
        }
    }
    fn born(&self) -> u16 {
        match self {
            Self::Figure(figure) => figure.born.year,
            Self::Dead(parent) => parent.born.year,
        }
    }
    fn parents(&self) -> Vec<&str> {
        match self {
            Self::Figure(figure) => [&figure.father, &figure.mother]
                .into_iter()
                .filter(|key| !key.is_empty())
                .map(String::as_str)
                .collect(),
            Self::Dead(_) => Vec::new(),
        }
    }
}

/// Every figure and dead parent in the order they are made (CK-370 §3 step 3): every parent before its children, ties by birth date, then
/// key. A figure dead before the start has no special place in it: its references to characters made later are resolved by the identifiers
/// [`predicted_ids`] gives them.
fn creation_order(data: &FigureData) -> Vec<Node<'_>> {
    let nodes: Vec<Node<'_>> = data
        .dead_parents
        .iter()
        .map(Node::Dead)
        .chain(data.figures.iter().map(Node::Figure))
        .collect();
    let index: BTreeMap<&str, usize> = nodes
        .iter()
        .enumerate()
        .map(|(at, node)| (node.key(), at))
        .collect();
    let mut waiting: Vec<usize> = nodes.iter().map(|node| node.parents().len()).collect();
    let mut children: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for (at, node) in nodes.iter().enumerate() {
        for parent in node.parents() {
            if let Some(parent) = index.get(parent) {
                children.entry(*parent).or_default().push(at);
            }
        }
    }
    let rank = |at: usize| (nodes[at].born(), nodes[at].key().to_owned(), at);
    let mut ready: BTreeSet<(u16, String, usize)> = (0..nodes.len())
        .filter(|at| waiting[*at] == 0)
        .map(rank)
        .collect();
    let mut order = Vec::with_capacity(nodes.len());
    while let Some(first) = ready.pop_first() {
        let at = first.2;
        order.push(nodes[at]);
        for child in children.get(&at).into_iter().flatten() {
            waiting[*child] -= 1;
            if waiting[*child] == 0 {
                ready.insert(rank(*child));
            }
        }
    }
    order
}

/// The identifier each figure and dead parent will have when it is made, in the order of [`creation_order`], from the store's next
/// identifier: the generator allocates one for each parent a figure lacks (a dead parent has none generated: its parents are the unknown
/// character) and then one for the figure. A reference to a character made later (a spouse, a strand's other end) is thus known when a record
/// is appended, and the archive never has to be changed afterwards.
fn predicted_ids(order: &[Node<'_>], next: u64) -> BTreeMap<String, u64> {
    let mut next = next;
    let mut ids = BTreeMap::new();
    for node in order {
        let missing = match node {
            Node::Figure(figure) => {
                u64::from(figure.father.is_empty()) + u64::from(figure.mother.is_empty())
            }
            Node::Dead(_) => 0,
        };
        let id = next + missing;
        ids.insert(node.key().to_owned(), id);
        next = id + 1;
    }
    ids
}

/// The first pop of a type at a place, in slot order.
fn first_pop(pops: &[(PopLocator, String, String)], place: &str, kind: &str) -> Option<PopLocator> {
    pops.iter()
        .filter(|(pop, of, _)| pop.place == place && of == kind)
        .map(|(pop, _, _)| pop)
        .min_by_key(|pop| pop.slot)
        .cloned()
}

/// The birth of the `n`th generated member of the slot `key` (CK-370 §5): a whole age drawn uniformly in the data's `[lo, hi]` of its role
/// (`other` for a role the table does not name), purpose `slot.age.<slot key>.<n>`, and the birth date the start date minus that age, on a day
/// of that year of age drawn on the same stream.
fn member_birth<P: Populations, A: ArchiveSink, L: LogSink>(
    world: &World<'_, P, A, L>,
    key: &str,
    n: u32,
    role: &str,
    start: GameTime,
) -> Result<GameTime, Error> {
    let [low, high] = world.lifecycle.generator.slots.ages_of(role);
    let mut stream = Stream::new(world.seed, format!("slot.age.{key}.{n}"), start);
    let age = low
        + u32::try_from(below(&mut stream, u64::from(high - low) + 1)?)
            .map_err(|_| Error::Overflow)?;
    let latest = dates::years_before(start, age)?;
    let earliest = dates::add_days(dates::years_before(start, age + 1)?, 1)?;
    let days =
        u64::try_from(dates::days_between(earliest, latest)).map_err(|_| Error::Generator)? + 1;
    let pick = i64::try_from(below(&mut stream, days)?).map_err(|_| Error::Overflow)?;
    dates::add_days(earliest, pick)
}

/// The first day of a calendar year.
fn first_of(year: u16) -> Result<GameTime, Error> {
    dates::new_year(year)
}

/// How a figure dead before the start ends (CK-370 §2): its date, else unknown.
fn end_of(figure: &Figure) -> Result<End, Error> {
    if let Some((year, month, day)) = figure.died_date {
        return Ok(End::Dead(dates::day_start(year, month, day)?));
    }
    // A known year is 1 January of it, whether or not the record also says the death was before the start; only a record with no date at all
    // is the explicit unknown.
    match figure.died {
        Some(died) => Ok(End::Dead(first_of(died.year)?)),
        None => Ok(End::DeathUnknown),
    }
}

/// The importance of a byname: that of the Irish (`gaelic`) model of `epithets.toml` whose name it is, else 40.
fn byname_importance<P: Populations, A: ArchiveSink, L: LogSink>(
    world: &World<'_, P, A, L>,
    byname: &str,
) -> u32 {
    let wanted = byname.to_lowercase();
    world
        .lifecycle
        .epithets
        .iter()
        .filter(
            |epithet| matches!(&epithet.scope, EpithetScope::Subgroup(group) if group == "gaelic"),
        )
        .find(|epithet| {
            let shown = epithet
                .shown
                .split(" (")
                .next()
                .unwrap_or_default()
                .to_lowercase();
            shown == wanted || epithet.key.replace('_', " ") == wanted
        })
        .map_or(DEFAULT_BYNAME_IMPORTANCE, |epithet| epithet.importance)
}

impl CharacterStore {
    /// Make the characters of a new game of the 768 scenario (CK-370 §3), on the economy, archive and logs of `world`, on the day `start`.
    /// The economy's start must already hold the additions of the scenario (§3 step 1), and the pops must report their cultures, kinds and
    /// distances; to have the safety net, the populations must answer [`make_room`](Populations::make_room).
    ///
    /// The store is left as the start made it: the caller saves it with the economy. On a refusal the store, the archive and the logs hold what
    /// was made before it, and the whole start is to be dropped.
    ///
    /// # Errors
    /// The data that does not join the pops, and any refusal of the engine: a pop that cannot hold a character, an archive that cannot
    /// append, a year that cannot be simulated.
    pub fn start_768<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        data: &FigureData,
        start: GameTime,
    ) -> Result<StartReport, StartError> {
        let past_until = first_of(PAST_ENDS_IN + 1)?;
        self.begin_start(past_until);
        let result = self.start_inner(world, data, start);
        self.end_start();
        result
    }

    #[expect(
        clippy::too_many_lines,
        reason = "the nine steps of the start in the order of the spec: each is a few lines over the engine's own operations"
    )]
    fn start_inner<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        data: &FigureData,
        start: GameTime,
    ) -> Result<StartReport, StartError> {
        let pops = world.populations.all_pops();
        data.check_pops(&pops)?;
        let mut report = StartReport::default();
        let mut first: FirstError = None;
        // Every archived record the start makes, to count the ones held abroad.
        let mut archived: Vec<u64> = Vec::new();
        self.fix_seed(world.seed);

        // Step 3: ancestry, oldest first.
        let mut ids: BTreeMap<String, u64> = BTreeMap::new();
        let mut fixed_of: BTreeMap<u64, Fixed> = BTreeMap::new();
        let parents_of: BTreeSet<&str> = data
            .figures
            .iter()
            .flat_map(|figure| [figure.father.as_str(), figure.mother.as_str()])
            .filter(|key| !key.is_empty())
            .collect();
        let order = creation_order(data);
        let predicted = predicted_ids(&order, self.next_id);
        for node in &order {
            let node = *node;
            let id = match node {
                Node::Dead(parent) => self.make_dead_parent(
                    world,
                    data,
                    &pops,
                    parent,
                    start,
                    (&mut first, &mut archived),
                )?,
                Node::Figure(figure) => {
                    let id = self.make_figure(
                        world,
                        data,
                        &pops,
                        figure,
                        (&ids, &predicted),
                        parents_of.contains(figure.key.as_str()),
                        start,
                        (&mut first, &mut archived),
                    )?;
                    if figure.alive_at_start {
                        report.figures_live += 1;
                    } else {
                        report.figures_archived += 1;
                    }
                    id
                }
            };
            if matches!(node, Node::Dead(_)) {
                report.dead_parents += 1;
            }
            if predicted.get(node.key()) != Some(&id) {
                return Err(StartError::Slot(format!(
                    "`{}` was made as {id}, not the {:?} its references were written for",
                    node.key(),
                    predicted.get(node.key())
                )));
            }
            ids.insert(node.key().to_owned(), id);
        }
        // The fixed aspects of the live figures, to take an excluded trait off again after the years (CK-370 §1).
        for figure in data.figures.iter().filter(|figure| figure.alive_at_start) {
            if let Some(id) = ids.get(&figure.key) {
                fixed_of.insert(*id, excluded_only(&figure.excluded));
            }
        }

        // The spouse a slot names is known before step 4 (CK-370 §5): the living ruler of a place with a `spouse_of_ruler` member has his spouse
        // protected, whether a household generates her now or a named wife is already his, so that she rolls neither natural nor childbirth death.
        for slot in &data.slots {
            if slot.members.iter().any(|row| row.role == "spouse_of_ruler")
                && let Some(ruler) = Self::figure_ruler_at(data, &ids, &slot.location)
            {
                self.start.slot_protected.insert(ruler);
            }
        }

        // Step 4: households. Named spouses are linked first (a living couple; a spouse who is archived is linked from its own record).
        for figure in data.figures.iter().filter(|figure| figure.alive_at_start) {
            if let (Some(a), Some(b)) = (ids.get(&figure.key), ids.get(&figure.spouse)) {
                let both_live = self.live.contains_key(a) && self.live.contains_key(b);
                let linked = self.live.get(a).is_some_and(|c| c.spouses.contains(b));
                if both_live && !linked {
                    self.marry(*a, *b, start)?;
                }
            }
        }
        for node in creation_order(data) {
            if let Node::Figure(figure) = node
                && figure.alive_at_start
                && let Some(id) = ids.get(&figure.key)
            {
                let done = self.complete_household(world, *id, start)?;
                if done.completed {
                    report.households += 1;
                }
                archived.extend(done.archived);
                if let Some(failure) = done.output {
                    keep(&mut first, failure);
                }
            }
        }

        // Step 5: slots, after every household exists.
        for slot in &data.slots {
            self.fill_start_slot(
                world,
                Joined {
                    data,
                    pops: &pops,
                    ids: &ids,
                },
                slot,
                start,
                &mut report,
                (&mut first, &mut archived),
            )?;
        }

        // Steps 6 and 7: the pasts stopped at 752 (`begin_start`); the burn-in runs 753-767 once, for everyone.
        self.run_start_burn_in(world, start)?;
        for (id, fixed) in &fixed_of {
            self.scrub_excluded(*id, fixed);
        }

        // Step 8: the known strands between two live characters.
        for strand in &data.strands {
            let (Some(a), Some(b)) = (ids.get(&strand.a), ids.get(&strand.b)) else {
                continue;
            };
            if self.live.contains_key(a) && self.live.contains_key(b) {
                self.set_authored_strand(*a, *b, &strand.kind, strand.strength, STRAND_YEAR)?;
                report.strands += 1;
            }
        }

        report.live = self.live.len();
        report.archived = archived.len();
        for id in &archived {
            if world
                .archive
                .get(*id)
                .map_err(EventError::Archive)?
                .is_some_and(|record| record.life.departure == Departure::Abroad)
            {
                report.abroad += 1;
            }
        }
        report.raised = world.populations.raised_at_start();
        report.output = first.map(|failure| format!("{failure:?}"));
        Ok(report)
    }

    /// The burn-in of the 768 start (CK-370 §3 step 7): the one yearly simulation of the years 753-767 for **everyone** alive at `start`, each
    /// character at its age in that year (those not yet born are skipped), slice 4's yearly stages (events, crafts, skills, standing) and slice 6's
    /// (strands, passions) together, dated in their years. **No deaths and no births**: monthly mortality is not run, a rule with a `die` effect
    /// is skipped as in a generated past, and no conception is run, so no character is added or removed.
    ///
    /// # Errors
    /// The first failure, once every year has been run.
    pub fn run_start_burn_in<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        start: GameTime,
    ) -> Result<(), EventError> {
        let mut minds: BTreeMap<u64, HouseholdMinds> = BTreeMap::new();
        self.burn_in_with(world, BURN_IN_YEARS, start, |store, world, id, day, age| {
            if let Entry::Vacant(slot) = minds.entry(id) {
                slot.insert(store.recorded_household(world, id)?);
            }
            let household = minds.get(&id).map_or(&[][..], Vec::as_slice);
            let mut soft: FirstError = None;
            store.past_year(world, id, day, age, household, &mut soft)?;
            conclude(soft)
        })
    }

    /// The fixed aspects of a figure, ready for the generator: its name and byname, its languages rolled once on its own stream, its traits.
    fn fixed_of_figure<P: Populations, A: ArchiveSink, L: LogSink>(
        world: &World<'_, P, A, L>,
        figure: &Figure,
        start: GameTime,
    ) -> Result<Fixed, Error> {
        let mut second_languages = Vec::new();
        for (language, range) in &figure.second_languages {
            let mut stream = Stream::new(
                world.seed,
                format!("figure:{}:language.{language}", figure.key),
                start,
            );
            let span = u64::from(range.high - range.low) + 1;
            let pick = below(&mut stream, span)?;
            second_languages.push((
                language.clone(),
                range.low + u32::try_from(pick).map_err(|_| Error::Overflow)?,
            ));
        }
        Ok(Fixed {
            name: figure.name.clone(),
            genitive: figure.genitive.clone(),
            patronymic: figure.patronymic.clone(),
            byname: figure.byname.clone(),
            byname_importance: if figure.byname.is_empty() {
                0
            } else {
                byname_importance(world, &figure.byname)
            },
            tongue: figure.mother_tongue.clone(),
            second_languages,
            forced: figure.forced.clone(),
            excluded: figure.excluded.clone(),
            house: Some(figure.house.clone()),
            house_of_birth: figure.house_of_birth.clone(),
            spouses: Vec::new(),
            strands: Vec::new(),
            inbound: Vec::new(),
        })
    }

    /// Make a real figure, live or archived (CK-370 §2, §3 step 3).
    #[expect(
        clippy::too_many_arguments,
        reason = "the world and the data, the pops, the figure, the ids made so far, whether it is a parent, the day and what the start keeps"
    )]
    fn make_figure<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        data: &FigureData,
        pops: &[(PopLocator, String, String)],
        figure: &Figure,
        (ids, predicted): (&BTreeMap<String, u64>, &BTreeMap<String, u64>),
        is_parent: bool,
        start: GameTime,
        (first, archived): (&mut FirstError, &mut Vec<u64>),
    ) -> Result<u64, StartError> {
        let pop = first_pop(pops, &figure.location, figure.pop_kind.key()).ok_or_else(|| {
            FigureError::Pop {
                entry: format!("figure.{}", figure.key),
                place: figure.location.clone(),
                kind: figure.pop_kind.key().to_owned(),
            }
        })?;
        let mut request = GenerateRequest::new(
            pop,
            figure.sex,
            Birth::On(first_of(figure.born.year)?),
            &figure.key,
            start,
        );
        request.father = ids.get(&figure.father).copied().unwrap_or(UNKNOWN);
        request.mother = ids.get(&figure.mother).copied().unwrap_or(UNKNOWN);
        request.office = figure.office;
        // A generated parent is born into the house the figure was born into.
        request.house = if figure.house_of_birth.is_empty() {
            figure.house.clone()
        } else {
            figure.house_of_birth.clone()
        };
        request.has_children = is_parent;
        let mut fixed = Self::fixed_of_figure(world, figure, start)?;
        if !figure.alive_at_start {
            request.end = Some(end_of(figure)?);
        }
        // A marriage with an archived spouse is linked from the record of **each** side as it is made, by the spouse's predicted identifier
        // (an archived record cannot be changed after it is appended, and the living one has to list it as every survivor does); a marriage of
        // two living figures is made once both exist.
        let alive = |key: &str| data.figure(key).is_none_or(|other| other.alive_at_start);
        let partners = data
            .figures
            .iter()
            .filter(|other| other.spouse == figure.key)
            .map(|other| other.key.as_str())
            .chain((!figure.spouse.is_empty()).then_some(figure.spouse.as_str()));
        for key in partners {
            if (!figure.alive_at_start || !alive(key))
                && let Some(id) = predicted.get(key)
                && !fixed.spouses.contains(id)
            {
                fixed.spouses.push(*id);
            }
        }
        // The authored strands of a figure archived here are written into its own record at its append, as history and never as a live bond: a
        // bond on the archived end, a feeling only when the archived figure is the one that holds it (the authored direction, `a` to `b`).
        if !figure.alive_at_start {
            for strand in &data.strands {
                // A one-way feeling toward this figure is kept as it was authored: inbound history on this record.
                if strand.b == figure.key
                    && !is_bond_kind(&strand.kind)
                    && let Some(holder) = predicted.get(&strand.a)
                {
                    fixed
                        .inbound
                        .push((*holder, strand.kind.clone(), strand.strength));
                    continue;
                }
                let partner = if strand.a == figure.key {
                    &strand.b
                } else if strand.b == figure.key && is_bond_kind(&strand.kind) {
                    &strand.a
                } else {
                    continue;
                };
                let id = predicted.get(partner).ok_or_else(|| {
                    StartError::Slot(format!(
                        "the strand between `{}` and `{}` names a character that is not in the data",
                        strand.a, strand.b
                    ))
                })?;
                fixed
                    .strands
                    .push((*id, strand.kind.clone(), strand.strength));
            }
        }
        request.fixed = Some(fixed);
        let made = self.generate(world, &request)?;
        archived.extend(made.archived.iter().copied());
        if let Some(failure) = made.output {
            keep(first, failure);
        }
        Ok(made.id)
    }

    /// Archive a dead parent (CK-370 §2): its dates, its house, no parents of its own and no head.
    fn make_dead_parent<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        data: &FigureData,
        pops: &[(PopLocator, String, String)],
        parent: &DeadParent,
        start: GameTime,
        (first, archived): (&mut FirstError, &mut Vec<u64>),
    ) -> Result<u64, StartError> {
        let pop = first_pop(pops, &parent.location, parent.pop_kind.key()).ok_or_else(|| {
            FigureError::Pop {
                entry: format!("dead_parent.{}", parent.key),
                place: parent.location.clone(),
                kind: parent.pop_kind.key().to_owned(),
            }
        })?;
        let mut request = GenerateRequest::new(
            pop,
            parent.sex,
            Birth::On(first_of(parent.born.year)?),
            &parent.key,
            start,
        );
        request.house.clone_from(&parent.house);
        request.bare = true;
        request.end = Some(match parent.died {
            Some(year) => End::Dead(first_of(year)?),
            None => End::DeathUnknown,
        });
        request.fixed = Some(Fixed {
            name: parent.name.clone(),
            genitive: parent.genitive.clone(),
            house: Some(parent.house.clone()),
            excluded: data.excluded.clone(),
            ..Fixed::default()
        });
        let made = self.generate(world, &request)?;
        archived.extend(made.archived.iter().copied());
        if let Some(failure) = made.output {
            keep(first, failure);
        }
        Ok(made.id)
    }

    /// The living ruler at a place: the figure with `office = ruler` there, the lowest key if several.
    fn figure_ruler_at(data: &FigureData, ids: &BTreeMap<String, u64>, place: &str) -> Option<u64> {
        data.figures
            .iter()
            .filter(|figure| {
                figure.alive_at_start
                    && figure.office == Some(Office::Ruler)
                    && figure.location == place
            })
            .min_by(|a, b| a.key.cmp(&b.key))
            .and_then(|figure| ids.get(&figure.key).copied())
    }

    /// Fill one slot (CK-370 §3 step 5): `lord` first, then the other members in file order, then the spouse places.
    #[expect(
        clippy::too_many_lines,
        reason = "one slot's rows in the order of the spec: each kind of row is a few lines over the engine's own fill"
    )]
    fn fill_start_slot<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        joined: Joined<'_>,
        slot: &crate::figures::SlotData,
        start: GameTime,
        report: &mut StartReport,
        (first, archived): (&mut FirstError, &mut Vec<u64>),
    ) -> Result<(), StartError> {
        let Joined { data, pops, ids } = joined;
        let court_house = Self::figure_ruler_at(data, ids, &slot.location)
            .and_then(|id| self.live.get(&id))
            .map(|ruler| ruler.life.house.clone())
            .unwrap_or_default();
        let pop_of = |kind: &str| -> Result<PopLocator, StartError> {
            first_pop(pops, &slot.location, kind).ok_or_else(|| {
                StartError::Data(FigureError::Pop {
                    entry: format!("slot.{}", slot.key),
                    place: slot.location.clone(),
                    kind: kind.to_owned(),
                })
            })
        };
        let mut lord: Option<u64> = None;
        let mut counter = 0_u32;
        // The generated members of the slot, numbered in the order they are made, for the stream of each one's age.
        let mut member_no = 0_u32;
        // `lord` first, then the others in file order; the spouse places come last.
        let rows: Vec<&crate::figures::SlotMember> = slot
            .members
            .iter()
            .filter(|row| row.role == "lord")
            .chain(
                slot.members
                    .iter()
                    .filter(|row| row.role != "lord" && !row.role.starts_with("spouse_of_")),
            )
            .chain(
                slot.members
                    .iter()
                    .filter(|row| row.role.starts_with("spouse_of_")),
            )
            .collect();
        for row in rows {
            counter += 1;
            let pop = pop_of(&row.pop)?;
            match row.role.as_str() {
                "fosterling" | "hostage" | "student" => {
                    let outcome = self.fill_slot(
                        world,
                        &SlotRequest {
                            key: format!("{}.{}.{counter}", slot.key, row.role),
                            kind: row.role.clone(),
                            court: pop,
                            house: court_house.clone(),
                            count: row.count,
                            ages: None,
                            today: start,
                            sex: Some(row.sex),
                            slot: slot.key.clone(),
                            noble_families_only: row.from_noble_families,
                        },
                    )?;
                    report.slot_places += outcome.placed.len();
                    archived.extend(outcome.archived);
                    report.warnings.extend(outcome.warnings);
                    if let Some(failure) = outcome.output {
                        keep(first, failure);
                    }
                }
                "spouse_of_lord" | "spouse_of_ruler" => {
                    let subject = if row.role == "spouse_of_lord" {
                        lord
                    } else {
                        Self::figure_ruler_at(data, ids, &slot.location)
                    };
                    let Some(subject) = subject else {
                        return Err(StartError::Slot(format!(
                            "slot {}: a `{}` place has no subject (no {} at {})",
                            slot.key,
                            row.role,
                            if row.role == "spouse_of_lord" {
                                "lord in the slot"
                            } else {
                                "living ruler"
                            },
                            slot.location
                        )));
                    };
                    for place in 0..row.count {
                        let filled = self.fill_spouse_place(
                            world, subject, &row.role, &slot.key, place, start,
                        )?;
                        report.slot_places += usize::from(filled.filled);
                        report.warnings.extend(filled.warning);
                        if let Some(failure) = filled.failure {
                            keep(first, failure);
                        }
                    }
                }
                role => {
                    let mut members: Vec<Member> = Vec::new();
                    for n in 0..row.count {
                        member_no += 1;
                        members.push(Member {
                            role: role.to_owned(),
                            sex: row.sex,
                            birth: Birth::On(member_birth(
                                world, &slot.key, member_no, role, start,
                            )?),
                            house: String::new(),
                            office: None,
                            key: format!("{}.{role}.{counter}.{n}", slot.key),
                            slot: slot.key.clone(),
                        });
                    }
                    let made = self.fill_members(world, &pop, &members, start)?;
                    archived.extend(made.archived.iter().copied());
                    report.slot_places += made.ids.len();
                    if role == "lord" && lord.is_none() {
                        lord = made.ids.first().copied();
                    }
                    if let Some(failure) = made.output {
                        keep(first, failure);
                    }
                }
            }
        }
        Ok(())
    }

    /// A spouse place (CK-370 §3 step 5): filled by the subject's **current** spouse, named or made by its household, and never a new person;
    /// a subject with no spouse gets one generated and married to it (slice 5c's transition), who fills it. She always exists: when no
    /// candidate fits his free age gap and the fertile ages the fertility limit is dropped (marriageable and within his free gap), and when
    /// that is empty her age is the marriageable minimum. The place is recorded on the spouse (the slot and its role as the
    /// kind), and a character with a slot is never chosen for another: a spouse who already fills a place is not another place filled. A
    /// spouse a slot names lives where the slot puts her, with her husband: the relatives rule of step 2 moves only relatives the generator adds.
    fn fill_spouse_place<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        subject: u64,
        role: &str,
        slot: &str,
        place: u32,
        start: GameTime,
    ) -> Result<SpousePlace, StartError> {
        let record = self
            .live
            .get(&subject)
            .cloned()
            .ok_or(Error::MissingCharacter)?;
        let mut failure = None;
        let mut generated = false;
        let spouse = if let Some(spouse) = self.partner_of(&record).map(|spouse| spouse.id) {
            spouse
        } else {
            generated = true;
            let mut candidate = None;
            'tiers: for relax in 0..CANDIDATE_RELAXATIONS {
                for attempt in 0..CANDIDATE_ATTEMPTS {
                    let index = usize::try_from(
                        (place * u32::from(CANDIDATE_RELAXATIONS) + u32::from(relax))
                            * CANDIDATE_ATTEMPTS
                            + attempt,
                    )
                    .unwrap_or(0);
                    candidate = self.generate_candidate(world, subject, start, index, relax)?;
                    if candidate.is_some() {
                        break 'tiers;
                    }
                }
            }
            let (made, output) = candidate.ok_or_else(|| {
                StartError::Slot(format!(
                    "slot {slot}: no spouse can be generated for the subject of a `{role}` place"
                ))
            })?;
            failure = output;
            let wedding = self.arrange_marriage(world, subject, made, start)?;
            if let Some(output) = wedding.output {
                keep(&mut failure, output);
            }
            made
        };
        let Some(c) = self.live.get_mut(&spouse) else {
            return Err(Error::MissingCharacter.into());
        };
        if !c.life.slot_key.is_empty() {
            let held = c.life.slot_key.clone();
            return Ok(SpousePlace {
                filled: false,
                warning: Some(format!(
                    "slot {slot}: the spouse of the subject of a `{role}` place already fills a place of `{held}`: the place stays unfilled"
                )),
                failure,
            });
        }
        slot.clone_into(&mut c.life.slot_key);
        role.clone_into(&mut c.life.slot_kind);
        // A spouse made for the place is made for its role; a named figure, or a spouse a household made, keeps its own life.
        if generated && c.life.role.is_empty() {
            role.clone_into(&mut c.life.role);
        }
        Ok(SpousePlace {
            filled: true,
            warning: None,
            failure,
        })
    }
}

/// What filling a spouse place came to.
struct SpousePlace {
    /// Whether a place was filled now (not one the spouse already held).
    filled: bool,
    /// Why it was not.
    warning: Option<String>,
    /// A failure after a commit.
    failure: Option<EventError>,
}

/// The fixed aspects that only keep a trait off: for the live figures, after the years.
fn excluded_only(excluded: &[(String, u8)]) -> Fixed {
    Fixed {
        excluded: excluded.to_vec(),
        ..Fixed::default()
    }
}
