//! Real households (CK-369 §7), for slice 8: `complete_household(figure)` completes a **couple** (the figure and its named or generated
//! spouse) once and is **idempotent**: a household records that it is complete, and a second call for either spouse does nothing.
//!
//! - **Parents**: a missing parent is generated per §2, conditioned on the figure's birth, into the figure's house.
//! - **Spouse**: a figure of marriage age with no named spouse gets one with chance `1 - prod(1 - wish(age_y))` over its years from
//!   marriageable age to today (5c's wish curve per year): the marriage year is the first year whose wish draw succeeds (on the figure's
//!   stream, purpose `household.wish.<year>`) and the wedding date is the first date of that year on which **both** spouses are of
//!   marriageable age (1 January, or the later of their marriageable birthdays that fall in that year). A named spouse's wedding date is the
//!   first date on which both are of marriageable age.
//! - **Children**: the named children first, each at its recorded date, reserving the mother's window from nine months before its birth to
//!   `nursing_months` after it; then month by month from the wedding to today the conception rules of 5a run and skip any month whose
//!   conception would give a **generated interval** (its conception, its birth nine months later and its nursing after that) that overlaps a
//!   reserved window at any point. Named children count toward the children a couple wants; none are conceived before the wedding. The seed
//!   identity is the couple's lower figure id and the purpose `household.<n>`.
//!
//! One transaction. The `marriage` and `child_born` happenings, dated as the wedding and the births, follow the commit.
#![deny(clippy::float_arithmetic)]

use crate::{
    ArchiveSink, Character, CharacterStore, Error, PopLocator, Populations, Sex, UNKNOWN,
    character_stream,
    cycle::{CAUSE_CHILDBIRTH, first_after},
    events::{EventError, FirstError, LogSink, World, age_years},
    generator::{Birth, GenerateRequest, Txn, dates, figure::Home},
    genetics::{below, roll},
};
use ck_sim::GameTime;

/// The result of a household completion that committed.
#[derive(Debug)]
pub struct HouseholdOutcome {
    /// Whether the household was completed now (`false`: it was complete already and nothing was done).
    pub completed: bool,
    /// The parents made (live or archived), of the figure and of a named spouse.
    pub parents: Vec<u64>,
    /// The spouse of the couple, named or generated.
    pub spouse: Option<u64>,
    /// Whether the spouse was generated.
    pub spouse_generated: bool,
    /// The wedding date of the couple, when it has one by today.
    pub wedding: Option<GameTime>,
    /// The generated children, in the order of their births.
    pub children: Vec<u64>,
    /// A conception whose birth falls after today: the mother is pregnant now (her id).
    pub pregnant: Option<u64>,
    /// The day the generated spouse died, when the death curve or a delivery took it before the start (it is archived, dated and linked).
    pub spouse_died: Option<GameTime>,
    /// Every live character made, in order.
    pub created: Vec<u64>,
    /// Every dead ancestor made.
    pub archived: Vec<u64>,
    /// The first failure after the commit (a life-log line or a happening that could not run).
    pub output: Option<EventError>,
}

/// A happening to run after the commit.
struct Later {
    happening: &'static str,
    who: Vec<u64>,
    date: GameTime,
}

/// The age (whole years) at which a character of `c`'s sex is marriageable in the religion of its pop (CK-364 `[marriageable]`).
fn marriageable_age<P: Populations, A: ArchiveSink, L: LogSink>(
    world: &World<'_, P, A, L>,
    c: &Character,
) -> Result<u32, Error> {
    marriageable_age_in(world, &c.pop, c.sex)
}

/// The same for a character that is not made yet: of `sex`, in `pop`.
fn marriageable_age_in<P: Populations, A: ArchiveSink, L: LogSink>(
    world: &World<'_, P, A, L>,
    pop: &PopLocator,
    sex: Sex,
) -> Result<u32, Error> {
    let religion = world.populations.religion(pop).ok_or(Error::Lifecycle)?;
    let ages = world
        .lifecycle
        .marriageable
        .get(&religion)
        .ok_or(Error::Lifecycle)?;
    Ok(match sex {
        Sex::Male => ages.m,
        Sex::Female => ages.f,
    })
}

impl CharacterStore {
    /// Complete the household of a live character (CK-369 §7). See the module's description. One transaction.
    ///
    /// # Errors
    /// A refusal, with nothing changed: a figure that is not live, a pop with no room for what is generated, an archive that cannot append,
    /// data that cannot be resolved, arithmetic that does not fit. A failure of a happening or a line after the commit is in
    /// [`HouseholdOutcome::output`].
    pub fn complete_household<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        figure: u64,
        today: GameTime,
    ) -> Result<HouseholdOutcome, EventError> {
        let done = self.transact(world, |store, staged, txn| {
            store.household_in(staged, txn, figure, today)
        })?;
        let (mut outcome, later) = done.value;
        outcome.created = done.created;
        outcome.archived = done.archived;
        let mut first: FirstError = done.output;
        for happening in later {
            // A happening is for the living: a generated relative who died by the death curve has none.
            let who: Vec<u64> = happening
                .who
                .iter()
                .copied()
                .filter(|id| self.live.contains_key(id))
                .collect();
            self.emit(world, &who, happening.happening, happening.date, &mut first);
        }
        outcome.output = first;
        Ok(outcome)
    }

    /// The partner of `c` of the other sex: the first living spouse.
    pub(crate) fn partner_of(&self, c: &Character) -> Option<&Character> {
        c.spouses
            .iter()
            .filter_map(|spouse| self.live.get(spouse))
            .find(|spouse| spouse.sex != c.sex)
    }

    /// Generate the parents `person` lacks, into its house, conditioned on its birth (§7, parents), and link them.
    fn complete_parents<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        txn: &mut Txn,
        person: u64,
        today: GameTime,
        out: &mut Vec<u64>,
    ) -> Result<(), EventError> {
        let c = self
            .live
            .get(&person)
            .cloned()
            .ok_or(Error::MissingCharacter)?;
        let want = [c.father == UNKNOWN, c.mother == UNKNOWN];
        if !want[0] && !want[1] {
            return Ok(());
        }
        let mut identity = GenerateRequest::new(
            c.pop.clone(),
            c.sex,
            Birth::On(c.birth),
            &format!("household.{person}.parents"),
            today,
        );
        identity.house.clone_from(&c.life.house);
        let allocate = |wanted: bool, store: &mut Self| -> Result<Option<u64>, Error> {
            if !wanted {
                return Ok(None);
            }
            let id = store.next_id;
            store.next_id = id.checked_add(1).ok_or(Error::Overflow)?;
            Ok(Some(id))
        };
        let ids = [allocate(want[0], self)?, allocate(want[1], self)?];
        let made = self.make_ancestors(world, txn, &identity, ids, person, c.birth)?;
        txn.touch(self, person);
        self.invalidate_ancestry();
        if let Some(live) = self.live.get_mut(&person) {
            if let Some(father) = &made[0] {
                live.father = father.id;
            }
            if let Some(mother) = &made[1] {
                live.mother = mother.id;
            }
        }
        out.extend(made.iter().flatten().map(|parent| parent.id));
        Ok(())
    }

    /// The day `c` becomes marriageable.
    fn marriageable_day<P: Populations, A: ArchiveSink, L: LogSink>(
        world: &World<'_, P, A, L>,
        c: &Character,
    ) -> Result<GameTime, Error> {
        dates::birthday(c.birth, marriageable_age(world, c)?)
    }

    /// Whether the figure marries in a year of its life and the spouse it gets (§7, spouse): the first year, from the year it is
    /// marriageable to the last year whose birthday has come, whose wish draw succeeds. The year and the spouse's birth are returned.
    fn draw_marriage<P: Populations, A: ArchiveSink, L: LogSink>(
        world: &World<'_, P, A, L>,
        figure: &Character,
        today: GameTime,
    ) -> Result<Option<(u16, GameTime)>, Error> {
        let rules = &world.lifecycle;
        let marriageable = Self::marriageable_day(world, figure)?;
        let first_year = dates::date_of(marriageable)?.year;
        let last_year = dates::date_of(today)?.year;
        for year in first_year..=last_year {
            let age = u32::from(year - dates::date_of(figure.birth)?.year);
            let birthday = dates::birthday(figure.birth, age)?;
            if birthday > today {
                break;
            }
            let wish = Self::wish_ppb(world, figure, birthday)?;
            let mut stream = character_stream(
                world.seed,
                figure.id,
                &format!("household.wish.{year}"),
                figure.birth,
            );
            if !roll(&mut stream, wish)? {
                continue;
            }
            // The spouse's age within the free gap of the figure's, marriageable, and inside the ends of the fertile curve of its sex: read
            // on 1 January of that year and, when no age fits then (a figure who turns marriageable or of an age this year, with a narrow
            // gap), on the figure's birthday of that year, on which its wish succeeded.
            let gap = rules.marriage.score.age_gap_free;
            let (other, curve) = if figure.sex == Sex::Male {
                (Sex::Female, &rules.conception.women)
            } else {
                (Sex::Male, &rules.conception.men)
            };
            let religion = world
                .populations
                .religion(&figure.pop)
                .ok_or(Error::Lifecycle)?;
            let ages = rules.marriageable.get(&religion).ok_or(Error::Lifecycle)?;
            let (own, curve_ends) = (
                if other == Sex::Male { ages.m } else { ages.f },
                (
                    curve.first().map_or(0, |p| p.0),
                    curve.last().map_or(0, |p| p.0),
                ),
            );
            // The preferred age window first, then (when the figure is of an age at which none fits) the free gap alone, the fertile curve
            // being applied by the conceptions themselves; and when even that is empty, the youngest marriageable spouse: a marriage the wish
            // has made is kept whenever a marriageable spouse and a wedding date exist.
            let january = dates::new_year(year)?;
            let mut chosen = None;
            for (reference, fertile) in [(january, true), (birthday, true), (birthday, false)] {
                let figure_age =
                    age_years(dates::date_of(figure.birth)?, dates::date_of(reference)?);
                let mut low = figure_age.saturating_sub(gap).max(own);
                let mut high = figure_age + gap;
                if fertile {
                    low = low.max(curve_ends.0);
                    high = high.min(curve_ends.1);
                }
                if low <= high {
                    chosen = Some((reference, low, high));
                    break;
                }
            }
            let chosen = chosen.or(Some((birthday, own, own)));
            let Some((reference, low, high)) = chosen else {
                return Ok(None);
            };
            let mut spouse =
                character_stream(world.seed, figure.id, "household.spouse", figure.birth);
            let age = low
                + u32::try_from(below(&mut spouse, u64::from(high - low) + 1)?)
                    .map_err(|_| Error::Overflow)?;
            let latest = dates::years_before(reference, age)?;
            let earliest = dates::add_days(dates::years_before(reference, age + 1)?, 1)?;
            let days = dates::days_between(earliest, latest);
            let pick = below(
                &mut spouse,
                u64::try_from(days).map_err(|_| Error::Generator)? + 1,
            )?;
            let born =
                dates::add_days(earliest, i64::try_from(pick).map_err(|_| Error::Overflow)?)?;
            return Ok(Some((year, born)));
        }
        Ok(None)
    }

    /// Marry two live characters of one pop on `wedding` (state only; the `marriage` happening follows the commit): the wife takes her
    /// husband's house and a houseless husband his wife's (CK-366 §6).
    fn wed(
        &mut self,
        txn: &mut Txn,
        husband: u64,
        wife: u64,
        wedding: GameTime,
    ) -> Result<(), Error> {
        txn.touch(self, husband);
        txn.touch(self, wife);
        self.marry(husband, wife, wedding)?;
        let (h, w) = (
            self.live
                .get(&husband)
                .ok_or(Error::MissingCharacter)?
                .life
                .house
                .clone(),
            self.live
                .get(&wife)
                .ok_or(Error::MissingCharacter)?
                .life
                .house
                .clone(),
        );
        let house = if h.is_empty() && !w.is_empty() { w } else { h };
        self.set_house(husband, &house);
        self.set_house(wife, &house);
        Ok(())
    }

    /// The reserved windows of the named children of a couple: from nine months before each birth to `nursing_months` after it.
    fn reserved_windows<P: Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        wife: &Character,
    ) -> Result<Vec<(GameTime, GameTime)>, Error> {
        let nursing = world.lifecycle.conception.nursing_months;
        let mut windows = Vec::new();
        for id in self.children_of(&*world.archive, wife.id)? {
            let Some(child) = self.record(&*world.archive, id)? else {
                continue;
            };
            // The mother's window, whoever the father was: a woman cannot carry two pregnancies or nurse through one.
            if child.mother != wife.id {
                continue;
            }
            windows.push((
                dates::months_before(child.birth, 9)?,
                first_after(child.birth, nursing)?,
            ));
        }
        Ok(windows)
    }

    /// The nursing a mother is still in when the household is handed to play: the latest deadline of a named or a generated delivery, kept
    /// on her record so that the in-play conceptions respect it.
    fn hand_over_nursing(
        &mut self,
        txn: &mut Txn,
        wife: u64,
        windows: &[(GameTime, GameTime)],
        held_until: Option<GameTime>,
        today: GameTime,
    ) {
        let latest = windows
            .iter()
            .map(|(_, to)| *to)
            .chain(held_until)
            .max()
            .filter(|until| *until > today);
        if let Some(until) = latest {
            txn.touch(self, wife);
            if let Some(live) = self.live.get_mut(&wife) {
                live.life.nursing_until = Some(
                    live.life
                        .nursing_until
                        .map_or(until, |held| held.max(until)),
                );
            }
        }
    }

    /// The generated children of a couple, month by month from the wedding to today (§7, children).
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        reason = "the world, the transaction, the couple, the wedding and today, and the two outputs of the completion; the months of one couple's married years"
    )]
    fn conceive_children<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        txn: &mut Txn,
        pair: (u64, u64),
        wedding: GameTime,
        today: GameTime,
        out: &mut HouseholdOutcome,
        later: &mut Vec<Later>,
    ) -> Result<(), EventError> {
        let (husband_id, wife_id) = pair;
        let lower = husband_id.min(wife_id);
        // A generated spouse may have died by the death curve (CK-370 §5): its record is archived, and the window of conceptions ends with it.
        let husband = self.relative(&*world.archive, husband_id)?;
        let wife = self.relative(&*world.archive, wife_id)?;
        let curve = self.relatives_curve(world);
        let wife_generated = out.spouse_generated
            && out.spouse == Some(wife_id)
            && !self.start.slot_protected.contains(&husband_id);
        // The children of a character in a monk pop are generated relatives: they live beside the monk pop at the start (CK-370 §3 step 2), and
        // none can be made when the whole culture is abroad.
        let home = match self.relative_home(world, &husband.pop)? {
            Some(Home::Abroad) => return Ok(()),
            Some(Home::Pop(pop)) => Some(pop),
            None => None,
        };
        let windows = self.reserved_windows(world, &wife)?;
        let nursing = world.lifecycle.conception.nursing_months;
        let last_fertile = world.lifecycle.conception.women.last().map_or(0, |p| p.0);
        let mut month = {
            let date = dates::date_of(wedding)?;
            if date.day == 1 {
                wedding
            } else {
                first_after(wedding, 1)?
            }
        };
        let mut held_until: Option<GameTime> = None;
        let mut n = 0_u32;
        while month <= today {
            let index = n;
            n += 1;
            let at = month;
            month = first_after(month, 1)?;
            if held_until.is_some_and(|until| at < until) {
                continue;
            }
            let (Ok(husband), Ok(wife)) = (
                self.relative(&*world.archive, husband_id),
                self.relative(&*world.archive, wife_id),
            ) else {
                break;
            };
            if age_years(dates::date_of(wife.birth)?, dates::date_of(at)?) > last_fertile {
                break;
            }
            // Neither parent conceives after its death (a generated spouse's pending end is in the transaction), a father may die after conceiving
            // and the child is still born, and a mother who dies before the delivery loses the pregnancy: no birth.
            let ended = |id: u64| txn.relative_deaths.get(&id).map(|(day, _)| *day);
            if ended(wife_id).is_some_and(|died| at >= died)
                || ended(husband_id).is_some_and(|died| at >= died)
            {
                break;
            }
            let birth = first_after(at, 9)?;
            if ended(wife_id).is_some_and(|died| birth > died) {
                break;
            }
            let ends = first_after(birth, nursing)?;
            // Intervals are half-open: a conception may fall on the day a nursing period ends (5a), and a generated interval may end on the
            // day a named one begins.
            if windows.iter().any(|(from, to)| at < *to && ends > *from) {
                continue;
            }
            let chance = self.conception_ppb(&*world, &wife, &husband, at)?;
            let mut stream = character_stream(world.seed, lower, &format!("household.{index}"), at);
            if !roll(&mut stream, chance)? {
                continue;
            }
            if birth > today {
                // Pregnant at today: the in-play delivery takes it from here.
                txn.touch(self, wife_id);
                if let Some(live) = self.live.get_mut(&wife_id) {
                    live.life.pregnant_by = Some(husband_id);
                    live.life.due = Some(birth);
                }
                out.pregnant = Some(wife_id);
                break;
            }
            let count = Self::child_count(&*world, &wife, birth)?;
            for child in 0..count {
                let sex = if below(
                    &mut character_stream(
                        world.seed,
                        wife_id,
                        &format!("household.sex.{index}.{child}"),
                        at,
                    ),
                    2,
                )? == 0
                {
                    Sex::Male
                } else {
                    Sex::Female
                };
                let mut request = GenerateRequest::new(
                    home.clone().unwrap_or_else(|| husband.pop.clone()),
                    sex,
                    Birth::On(birth),
                    &format!("household.{lower}.{index}.{child}"),
                    today,
                );
                request.father = husband_id;
                request.mother = wife_id;
                request.relative_from = Some(birth);
                let id = self.generate_in(world, txn, &request)?;
                out.children.push(id);
                later.push(Later {
                    happening: "child_born",
                    who: vec![wife_id, husband_id],
                    date: birth,
                });
            }
            held_until = Some(ends);
            // A generated wife runs the risk of a delivery at each birth she gives (CK-370 §5): one who dies is archived on the day, and
            // there are no more conceptions.
            if curve
                && wife_generated
                && self.live.contains_key(&wife_id)
                && Self::relative_childbirth_death(world, &wife, birth)?
            {
                txn.relative_deaths
                    .insert(wife_id, (birth, CAUSE_CHILDBIRTH.to_owned()));
                break;
            }
        }
        self.hand_over_nursing(txn, wife_id, &windows, held_until, today);
        Ok(())
    }

    /// The record of `id`, live or archived: a generated relative that died by the death curve is archived, and is still the couple's.
    fn relative<A: ArchiveSink>(&self, archive: &A, id: u64) -> Result<Character, Error> {
        self.record(archive, id)?.ok_or(Error::MissingCharacter)
    }

    /// Move a generated relative made in this transaction from the living to the archive, dead on `day` of `cause` (CK-370 §5): its record is
    /// as it stands, children included, and it takes no head.
    fn archive_dead_relative<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        txn: &mut Txn,
        id: u64,
        day: GameTime,
        cause: &str,
    ) -> Result<(), EventError> {
        let mut record = self.take(id).ok_or(Error::MissingCharacter)?;
        record.death = Some(day);
        record.life.death_cause = Some(cause.to_owned());
        txn.created.retain(|made| *made != id);
        self.place(world, txn, record)
    }

    /// The completion of CK-369 §7 inside a transaction.
    #[expect(
        clippy::too_many_lines,
        reason = "the parts of one completion in the order of the spec: parents, spouse, wedding, children, the record"
    )]
    fn household_in<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        txn: &mut Txn,
        figure: u64,
        today: GameTime,
    ) -> Result<(HouseholdOutcome, Vec<Later>), EventError> {
        if !world.lifecycle.generator.loaded {
            return Err(Error::Generator.into());
        }
        let mut out = HouseholdOutcome {
            completed: false,
            parents: Vec::new(),
            spouse: None,
            spouse_generated: false,
            wedding: None,
            children: Vec::new(),
            pregnant: None,
            spouse_died: None,
            created: Vec::new(),
            archived: Vec::new(),
            output: None,
        };
        let mut later: Vec<Later> = Vec::new();
        let c = self
            .live
            .get(&figure)
            .cloned()
            .ok_or(Error::MissingCharacter)?;
        let named = self.partner_of(&c).map(|spouse| spouse.id);
        if c.life.household_complete
            || named
                .and_then(|spouse| self.live.get(&spouse))
                .is_some_and(|spouse| spouse.life.household_complete)
        {
            return Ok((out, later));
        }
        out.completed = true;
        // Parents: the figure's, and a named spouse's.
        self.complete_parents(world, txn, figure, today, &mut out.parents)?;
        if let Some(spouse) = named {
            self.complete_parents(world, txn, spouse, today, &mut out.parents)?;
        }
        // The spouse: named, or drawn and generated.
        let mut wedding: Option<GameTime> = None;
        let mut spouse = named;
        // A spouse generated for a character in a monk pop lives beside it (CK-370 §3 step 2); where the whole culture is abroad no spouse is made.
        let home = self.relative_home(world, &c.pop)?;
        let protected = self.start.slot_protected.contains(&figure);
        if let Some(spouse) = spouse.and_then(|id| self.live.get(&id)) {
            let day =
                Self::marriageable_day(world, &c)?.max(Self::marriageable_day(world, spouse)?);
            wedding = Some(day);
        } else if (protected || home != Some(Home::Abroad))
            && let Some((year, born)) = Self::draw_marriage(world, &c, today)?
        {
            // The spouse a slot names lives where the slot puts her, with her husband (r18 §3 steps 2 and 5): the relatives rule moves only the
            // relatives the household adds.
            let pop = match &home {
                Some(Home::Pop(pop)) if !protected => pop.clone(),
                _ => c.pop.clone(),
            };
            let sex = if c.sex == Sex::Male {
                Sex::Female
            } else {
                Sex::Male
            };
            // The first day of that year on which both are marriageable: 1 January, or the later marriageable birthday inside the year. It
            // comes first, because the spouse lives from the wedding to today by the death curve (CK-370 §5).
            let mut day = dates::new_year(year)?;
            for reaches in [
                Self::marriageable_day(world, &c)?,
                dates::birthday(born, marriageable_age_in(world, &pop, sex)?)?,
            ] {
                if dates::date_of(reaches)?.year == year {
                    day = day.max(reaches);
                }
            }
            let mut request = GenerateRequest::new(
                pop,
                sex,
                Birth::On(born),
                &format!("household.{figure}.spouse"),
                today,
            );
            request.house = String::new();
            request.spouse = Some(figure);
            request.defer_death = true;
            // The spouse a slot names is alive at the start (CK-370 §5): she was known before this step, and rolls nothing.
            // A spouse generated for the wedding exists because it happened: her natural-death rolls start on the 1 January after it (the wedding
            // year is survived).
            if !protected {
                request.relative_from = Some(dates::new_year(dates::date_of(day)?.year + 1)?);
            }
            let id = self.generate_in(world, txn, &request)?;
            if day <= today {
                let (husband, wife) = if c.sex == Sex::Male {
                    (figure, id)
                } else {
                    (id, figure)
                };
                if self.live.contains_key(&id) {
                    self.wed(txn, husband, wife, day)?;
                    later.push(Later {
                        happening: "marriage",
                        who: vec![husband, wife],
                        date: day,
                    });
                } else {
                    // The spouse died by the death curve after the wedding: archived, and still the figure's spouse.
                    txn.touch(self, figure);
                    if let Some(live) = self.live.get_mut(&figure) {
                        live.spouses.push(id);
                    }
                }
                wedding = Some(day);
                spouse = Some(id);
                out.spouse_generated = true;
            } else {
                // A wedding after today cannot arise (the year's wish draw needs the figure's birthday of the year, which has come); it
                // refuses the whole completion rather than leave a spouse who is not wed.
                return Err(Error::Generator.into());
            }
        }
        out.spouse = spouse;
        out.wedding = wedding.filter(|day| *day <= today);
        // Children: only of a man and a woman wed by today.
        if let (Some(spouse_id), Some(day)) = (spouse, out.wedding) {
            let (husband, wife) = if c.sex == Sex::Male {
                (figure, spouse_id)
            } else {
                (spouse_id, figure)
            };
            let sexes_differ = self
                .relative(&*world.archive, spouse_id)
                .is_ok_and(|spouse| spouse.sex != c.sex);
            if sexes_differ {
                self.conceive_children(
                    world,
                    txn,
                    (husband, wife),
                    day,
                    today,
                    &mut out,
                    &mut later,
                )?;
            }
        }
        // A generated spouse the death curve or a delivery took before the start is archived now, dated, at the earliest end the timeline found,
        // and takes no head; one who lives is charged her head now (a full pop is raised only for someone who is there at the opening).
        if let Some(id) = spouse.filter(|_| out.spouse_generated) {
            if let Some((died, cause)) = txn.relative_deaths.remove(&id) {
                self.start.pending.remove(&id);
                self.archive_dead_relative(world, txn, id, died, &cause)?;
                out.spouse_died = Some(died);
            } else if self.start.pending.contains(&id) {
                let pop = self
                    .live
                    .get(&id)
                    .map(|c| c.pop.clone())
                    .ok_or(Error::MissingCharacter)?;
                self.check_room(&pop, world.populations)?;
                self.start.pending.remove(&id);
            }
        }
        // The record: complete, for both.
        for id in [Some(figure), spouse].into_iter().flatten() {
            txn.touch(self, id);
            if let Some(live) = self.live.get_mut(&id) {
                live.life.household_complete = true;
            }
        }
        Ok((out, later))
    }
}
