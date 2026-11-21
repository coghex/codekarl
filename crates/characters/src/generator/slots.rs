//! Slot filling (CK-369 §4): a slot is a court (a pop and its house), a kind (`fosterling`, `hostage`, `student`), a count and ages by sex.
//! Each place is filled from **people who exist first**; a child is generated only when none is found, and then into a real family chosen
//! under the same exclusions as a candidate. The chosen character **moves** to the court's pop (slice 1's move with 5c's head rule), keeping
//! its parents, house and lines, and records the slot (key and kind); a character with a slot is never chosen for another.
//!
//! A whole slot fill is one transaction (§8): a later refusal leaves nothing. Every draw is on the court's stream, purpose
//! `slot.<slot key>.<kind>.<n>` and its parts.
#![deny(clippy::float_arithmetic)]

use crate::{
    ArchiveSink, Character, CharacterStore, Error, Office, PopLocator, Populations, Sex, UNKNOWN,
    cycle::curve_ratio,
    events::{EventError, FirstError, LogSink, World, age_years},
    generator::{Birth, GenerateRequest, Txn, dates},
    genetics::below,
};
use ck_economy::events::SLOT_KINDS;
use ck_sim::{GameTime, Stream};

/// The ages of a slot by sex, both inclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotAges {
    /// The ages of a boy or man.
    pub male: [u32; 2],
    /// The ages of a girl or woman.
    pub female: [u32; 2],
}

/// A slot to fill.
#[derive(Clone, Debug)]
pub struct SlotRequest {
    /// The slot's key, in the purpose of every draw.
    pub key: String,
    /// The kind: `fosterling`, `hostage` or `student`.
    pub kind: String,
    /// The court's pop: the pop the places are in.
    pub court: PopLocator,
    /// The court's house, empty for none.
    pub house: String,
    /// How many places.
    pub count: u32,
    /// The ages by sex; the data's for the kind when `None`.
    pub ages: Option<SlotAges>,
    /// The day.
    pub today: GameTime,
}

/// A member of a slot other than a fosterling, a hostage or a student (a retainer, a monk, a woman of the household): generated into the
/// slot's pop with its role and a family of the same culture, the parents per §2.
#[derive(Clone, Debug)]
pub struct Member {
    /// The role, a key of the role vocabulary.
    pub role: String,
    /// The sex.
    pub sex: Sex,
    /// The birth.
    pub birth: Birth,
    /// The house, empty for none.
    pub house: String,
    /// The office marker, if any.
    pub office: Option<Office>,
    /// The key of the request, unique within the slot.
    pub key: String,
}

/// The result of a slot fill that committed.
#[derive(Debug)]
pub struct SlotOutcome {
    /// The characters that fill places, in the order of the places: those that existed and those generated.
    pub placed: Vec<u64>,
    /// Which of them were generated for the slot (they and their parents are new).
    pub generated: Vec<u64>,
    /// The places left unfilled, each with a warning.
    pub unfilled: u32,
    /// What slice 8 reports: a place that stayed unfilled and why.
    pub warnings: Vec<String>,
    /// Every live character made, in order.
    pub created: Vec<u64>,
    /// Every dead ancestor made.
    pub archived: Vec<u64>,
    /// The first failure after the commit.
    pub output: Option<EventError>,
}

/// The stream of a court for one purpose.
fn court_stream(seed: u64, court: &PopLocator, purpose: &str, today: GameTime) -> Stream {
    Stream::new(
        seed,
        format!("court:{}:{}:{purpose}", court.place, court.slot),
        today,
    )
}

/// What a slot places: a candidate's id and its weight in thousandths.
type Weighted = Vec<(u64, u64)>;

impl CharacterStore {
    /// Fill a slot (CK-369 §4). See the module's description. One transaction.
    ///
    /// # Errors
    /// A refusal, with nothing changed: a kind that is not a slot kind, a court that does not resolve, a move the pops refuse, a pop with no
    /// room for a generated family, an archive that cannot append, arithmetic that does not fit.
    pub fn fill_slot<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        request: &SlotRequest,
    ) -> Result<SlotOutcome, EventError> {
        let done = self.transact(world, |store, staged, txn| {
            store.fill_slot_in(staged, txn, request)
        })?;
        let mut outcome = done.value;
        outcome.created = done.created;
        outcome.archived = done.archived;
        outcome.output = done.output;
        Ok(outcome)
    }

    /// Generate the other members of a slot into its pop (CK-369 §4, last bullet): one transaction. The ids are in the order of `members`.
    ///
    /// # Errors
    /// As [`generate`](Self::generate); nothing is changed when any member is refused.
    pub fn fill_members<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        pop: &PopLocator,
        members: &[Member],
        today: GameTime,
    ) -> Result<Made, EventError> {
        let done = self.transact(world, |store, staged, txn| {
            let mut ids = Vec::with_capacity(members.len());
            for member in members {
                let mut request =
                    GenerateRequest::new(pop.clone(), member.sex, member.birth, &member.key, today);
                request.role = Some(member.role.clone());
                request.house.clone_from(&member.house);
                request.office = member.office;
                ids.push(store.generate_in(staged, txn, &request)?);
            }
            Ok(ids)
        })?;
        Ok(Made {
            ids: done.value,
            created: done.created,
            archived: done.archived,
            output: done.output,
        })
    }

    /// The age of `c` on `today`.
    fn age_of(c: &Character, today: GameTime) -> Result<u32, Error> {
        Ok(age_years(dates::date_of(c.birth)?, dates::date_of(today)?))
    }

    /// Whether `c` is the child of a living ruler (the office marker of slice 1).
    fn rulers_child(&self, c: &Character) -> bool {
        [c.father, c.mother].iter().any(|parent| {
            *parent != UNKNOWN
                && self
                    .live
                    .get(parent)
                    .is_some_and(|record| record.office == Some(Office::Ruler))
        })
    }

    /// Whether `c` is kin of the court's house: of it now or by birth.
    fn kin_of(c: &Character, house: &str) -> bool {
        !house.is_empty() && (c.life.house == house || c.life.house_of_birth == house)
    }

    /// The culture subgroup of a pop.
    fn subgroup_of<P: Populations, A: ArchiveSink, L: LogSink>(
        world: &World<'_, P, A, L>,
        pop: &PopLocator,
    ) -> Option<String> {
        let culture = world.populations.culture(pop)?;
        world.rules.culture_group.get(&culture).cloned()
    }

    /// Whether `pop` is within the data's reach of `court` (the same place is 0 metres; an unknown distance is out of reach).
    fn within_reach<P: Populations, A: ArchiveSink, L: LogSink>(
        world: &World<'_, P, A, L>,
        court: &PopLocator,
        pop: &PopLocator,
    ) -> bool {
        let reach = u64::from(world.lifecycle.generator.slots.reach_km) * 1000;
        world
            .populations
            .distance_m(court, pop)
            .is_some_and(|metres| metres <= reach)
    }

    /// The characters that may fill a place of `request`, with their weights (§4), existing characters first: without a slot, alive, in the
    /// age range of their sex, and by kind
    /// - **fosterling**: a noble child of the court's culture subgroup, its pop within reach; the child of a living ruler is never placed in
    ///   its own court or in a court whose house has a living ruler (it is fostered out); kin of the court's house weigh `kin`;
    /// - **hostage**: a male son or kinsman of a ruling house other than the court's, from any distance;
    /// - **student**: a child of a noble or monk family within reach; else a child of the place's peasant pops.
    fn slot_candidates<P: Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        request: &SlotRequest,
        ages: &SlotAges,
    ) -> Result<Weighted, Error> {
        let rules = &world.lifecycle.generator.slots;
        let court_group = Self::subgroup_of(world, &request.court);
        let court_ruled = self.ruling_house(&request.house);
        let mut primary: Weighted = Vec::new();
        let mut peasants: Weighted = Vec::new();
        for c in self.live.values() {
            if !c.life.slot_key.is_empty() {
                continue;
            }
            let range = match c.sex {
                Sex::Male => ages.male,
                Sex::Female => ages.female,
            };
            let age = Self::age_of(c, request.today)?;
            if age < range[0] || age > range[1] {
                continue;
            }
            let kind = world.populations.kind(&c.pop).unwrap_or_default();
            match request.kind.as_str() {
                "fosterling" => {
                    if kind != "noble"
                        || Self::subgroup_of(world, &c.pop) != court_group
                        || !Self::within_reach(world, &request.court, &c.pop)
                    {
                        continue;
                    }
                    if self.rulers_child(c) && (court_ruled || Self::kin_of(c, &request.house)) {
                        continue;
                    }
                    let weight = if Self::kin_of(c, &request.house) {
                        u64::from(rules.kin_milli)
                    } else {
                        1000
                    };
                    primary.push((c.id, weight));
                }
                "hostage" => {
                    if c.sex != Sex::Male
                        || c.office == Some(Office::Ruler)
                        || c.life.house.is_empty()
                        || c.life.house == request.house
                        || !self.ruling_house(&c.life.house)
                    {
                        continue;
                    }
                    primary.push((c.id, 1000));
                }
                "student" => {
                    if (kind == "noble" || kind == "monk")
                        && Self::within_reach(world, &request.court, &c.pop)
                    {
                        primary.push((c.id, 1000));
                    } else if kind == "peasant" && c.pop.place == request.court.place {
                        peasants.push((c.id, 1000));
                    }
                }
                _ => return Err(Error::Slot),
            }
        }
        Ok(if primary.is_empty() {
            peasants
        } else {
            primary
        })
    }

    /// One candidate by weight in identifier order, on the court's stream.
    fn pick_weighted(stream: &mut Stream, candidates: &Weighted) -> Result<Option<u64>, Error> {
        let total: u64 = candidates.iter().map(|(_, weight)| *weight).sum();
        if total == 0 {
            return Ok(None);
        }
        let mut pick = below(stream, total)?;
        for (id, weight) in candidates {
            if pick < *weight {
                return Ok(Some(*id));
            }
            pick -= *weight;
        }
        Ok(None)
    }

    /// Move `id` to the court's pop and record its slot (§4): its head moves by the economy's cohort rule (an undone move if the
    /// transaction fails), its pop and the dates it joined it change, its language is updated once for the tongue it is placed among, and
    /// the precedence check runs (a pop's type and luxuries are inputs of a condition).
    fn assign_slot<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        txn: &mut Txn,
        id: u64,
        request: &SlotRequest,
    ) -> Result<(), EventError> {
        let c = self.live.get(&id).cloned().ok_or(Error::MissingCharacter)?;
        txn.touch(self, id);
        // Its own tongue is fixed before it is placed among another (a character that holds none has that of where it was raised).
        self.fix_tongue(world, id)?;
        if c.pop != request.court {
            let minor = Self::is_minor(world, &c, request.today)?;
            let undo = world.populations.move_head(&c.pop, &request.court, minor)?;
            txn.moved(undo);
            // Each pop's heads must still cover its characters once the mover has left one and joined the other.
            let leaves = self.pop_count(&c.pop).saturating_sub(1);
            let joins = self.pop_count(&request.court) + 1;
            let enough = world
                .populations
                .heads(&c.pop)
                .zip(world.populations.heads(&request.court))
                .is_some_and(|(source, court)| source >= leaves as u64 && court >= joins as u64);
            if !enough {
                return Err(Error::PopFull.into());
            }
            self.relocate(id, request.court.clone(), request.today);
        }
        if let Some(live) = self.live.get_mut(&id) {
            request.key.clone_into(&mut live.life.slot_key);
            request.kind.clone_into(&mut live.life.slot_kind);
        }
        match request.kind.as_str() {
            "fosterling" => {
                self.gain_court_tongue(world, id, &request.court, "fostered_among")?;
            }
            "hostage" => {
                self.gain_court_tongue(world, id, &request.court, "hostage_raised")?;
            }
            _ => {}
        }
        let mut first: FirstError = None;
        self.precedence(world, id, request.today, &mut first)?;
        match first {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    /// The pops of a place in slot order (a slot that does not exist ends them).
    fn pops_of<P: Populations>(populations: &P, place: &str) -> Vec<PopLocator> {
        (0..64_u32)
            .map(|slot| PopLocator {
                place: place.to_owned(),
                slot,
            })
            .take_while(|locator| populations.heads(locator).is_some())
            .collect()
    }

    /// Whether a child may originate in `pop` for the slot (§4, none found): for a hostage any pop; for a fosterling or a student a **noble** pop
    /// of the court's culture subgroup within reach. One rule for the pop of a house's member and of a reused couple alike.
    fn source_fits<P: Populations, A: ArchiveSink, L: LogSink>(
        world: &World<'_, P, A, L>,
        request: &SlotRequest,
        pop: &PopLocator,
    ) -> bool {
        request.kind == "hostage"
            || (world.populations.kind(pop).as_deref() == Some("noble")
                && Self::subgroup_of(world, pop) == Self::subgroup_of(world, &request.court)
                && Self::within_reach(world, &request.court, pop))
    }

    /// The nearest house a generated child may come from (§4, none found): for a fosterling or a student the nearest **noble** house of the
    /// court's subgroup within reach, never the court's own; for a hostage the nearest ruling house other than the court's, from any
    /// distance. Nearest by the distance from the court to the house's living members' pops, ties by house key; the house's pop is that of
    /// its nearest qualifying member.
    fn permitted_house<P: Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        request: &SlotRequest,
    ) -> Option<(String, PopLocator)> {
        let mut best: Option<(u64, &String, PopLocator)> = None;
        for (house, members) in &self.by_house {
            if *house == request.house {
                continue;
            }
            let hostage = request.kind == "hostage";
            if hostage && !self.ruling_house(house) {
                continue;
            }
            let mut near: Option<(u64, PopLocator)> = None;
            for member in members.iter().filter_map(|id| self.live.get(id)) {
                if !Self::source_fits(world, request, &member.pop) {
                    continue;
                }
                let metres = world
                    .populations
                    .distance_m(&request.court, &member.pop)
                    .unwrap_or(u64::MAX);
                if near
                    .as_ref()
                    .is_none_or(|(best_metres, _)| metres < *best_metres)
                {
                    near = Some((metres, member.pop.clone()));
                }
            }
            if let Some((metres, pop)) = near
                && best.as_ref().is_none_or(|(least, _, _)| metres < *least)
            {
                best = Some((metres, house, pop));
            }
        }
        best.map(|(_, house, pop)| (house.clone(), pop))
    }

    /// The existing couples of `house` whose child born on `birth` would be eligible: a living husband and wife who are each other's
    /// spouse, the wife fertile at the birth, and (for a fosterling) not the house's ruler and spouse when the court's house has a living
    /// ruler, since a ruler's child is not fostered into such a court. Ordered by the husband's identifier.
    fn eligible_couples<P: Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        request: &SlotRequest,
        house: &str,
        birth: GameTime,
    ) -> Result<Vec<(u64, u64)>, Error> {
        let (women, men) = (
            &world.lifecycle.conception.women,
            &world.lifecycle.conception.men,
        );
        let court_ruled = self.ruling_house(&request.house);
        let mut couples = Vec::new();
        for id in self.by_house.get(house).into_iter().flatten() {
            // The child originates in the husband's pop, which must satisfy the source rules as a member's does.
            let Some(husband) = self
                .live
                .get(id)
                .filter(|c| c.sex == Sex::Male && Self::source_fits(world, request, &c.pop))
            else {
                continue;
            };
            for wife in husband
                .spouses
                .iter()
                .filter_map(|spouse| self.live.get(spouse))
                .filter(|wife| wife.sex == Sex::Female && wife.spouses.contains(id))
            {
                let age = age_years(dates::date_of(wife.birth)?, dates::date_of(birth)?);
                if curve_ratio(women, age).0 == 0 || wife.birth >= birth {
                    continue;
                }
                // The father too must have been a man of fertile age at the birth, and born before it.
                let father_age = age_years(dates::date_of(husband.birth)?, dates::date_of(birth)?);
                if husband.birth >= birth || curve_ratio(men, father_age).0 == 0 {
                    continue;
                }
                let ruler =
                    husband.office == Some(Office::Ruler) || wife.office == Some(Office::Ruler);
                if request.kind == "fosterling" && ruler && court_ruled {
                    continue;
                }
                couples.push((husband.id, wife.id));
            }
        }
        Ok(couples)
    }

    /// Generate the child of a place into a real family (§4), then place it: its family's house and pop are the nearest permitted ones; the
    /// parents are an existing couple with a fertile mother whose child would be eligible, else a generated couple placed in that house's pop;
    /// with no permitted house a fosterling or a student comes from a generated couple of a **new minor house** at the court's place, and a
    /// hostage's place stays unfilled with a warning.
    fn generate_for_slot<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        txn: &mut Txn,
        request: &SlotRequest,
        ages: &SlotAges,
        place: u32,
        out: &mut SlotOutcome,
    ) -> Result<(), EventError> {
        let seed = world.seed;
        let today = request.today;
        let purpose = format!("slot.{}.{}.{place}", request.key, request.kind);
        let mut stream = court_stream(seed, &request.court, &format!("{purpose}.child"), today);
        let sex = if request.kind == "hostage" || below(&mut stream, 2)? == 0 {
            Sex::Male
        } else {
            Sex::Female
        };
        let [low, high] = match sex {
            Sex::Male => ages.male,
            Sex::Female => ages.female,
        };
        let age = low
            + u32::try_from(below(&mut stream, u64::from(high - low) + 1)?)
                .map_err(|_| Error::Overflow)?;
        // The birth: a day of the year in which the child is `age`.
        let latest = dates::years_before(today, age)?;
        let earliest = dates::add_days(dates::years_before(today, age + 1)?, 1)?;
        let birth = {
            let days = dates::days_between(earliest, latest);
            let pick = below(
                &mut stream,
                u64::try_from(days).map_err(|_| Error::Slot)? + 1,
            )?;
            dates::add_days(earliest, i64::try_from(pick).map_err(|_| Error::Overflow)?)?
        };
        let mut generate = GenerateRequest::new(
            request.court.clone(),
            sex,
            Birth::On(birth),
            &format!("{}.{}.{place}", request.key, request.kind),
            today,
        );
        generate.role = Some(request.kind.clone());
        match self.permitted_house(world, request) {
            Some((house, family_pop)) => {
                generate.pop = family_pop;
                generate.house.clone_from(&house);
                let couples = self.eligible_couples(world, request, &house, birth)?;
                if !couples.is_empty() {
                    let at = below(
                        &mut court_stream(
                            seed,
                            &request.court,
                            &format!("{purpose}.family"),
                            today,
                        ),
                        couples.len() as u64,
                    )?;
                    let (father, mother) =
                        couples[usize::try_from(at).map_err(|_| Error::Overflow)?];
                    generate.father = father;
                    generate.mother = mother;
                    if let Some(husband) = self.live.get(&father) {
                        generate.pop = husband.pop.clone();
                    }
                }
            }
            None if request.kind == "hostage" => {
                out.unfilled += 1;
                out.warnings.push(format!(
                    "slot {}: no ruling house other than `{}` to take a hostage from",
                    request.key, request.house
                ));
                return Ok(());
            }
            None => {
                // A new minor house at the court's place: in a noble pop of the court's culture if there is one, else a peasant pop.
                let culture = world.populations.culture(&request.court);
                let pops = Self::pops_of(world.populations, &request.court.place);
                let pick = |kind: &str| {
                    pops.iter()
                        .find(|pop| {
                            world.populations.kind(pop).as_deref() == Some(kind)
                                && world.populations.culture(pop) == culture
                        })
                        .cloned()
                };
                generate.pop = pick("noble")
                    .or_else(|| pick("peasant"))
                    .ok_or(Error::Slot)?;
                generate.minor_house_at = Some(request.court.place.clone());
            }
        }
        let id = self.generate_in(world, txn, &generate)?;
        self.assign_slot(world, txn, id, request)?;
        out.placed.push(id);
        out.generated.push(id);
        Ok(())
    }

    /// The fill of CK-369 §4 inside a transaction: see [`fill_slot`](Self::fill_slot).
    fn fill_slot_in<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        txn: &mut Txn,
        request: &SlotRequest,
    ) -> Result<SlotOutcome, EventError> {
        let rules = &world.lifecycle.generator;
        if !rules.loaded {
            return Err(Error::Generator.into());
        }
        if !SLOT_KINDS.contains(&request.kind.as_str()) {
            return Err(Error::Slot.into());
        }
        let kind = *rules.slots.kinds.get(&request.kind).ok_or(Error::Slot)?;
        let ages = request.ages.unwrap_or(SlotAges {
            male: kind.age_m,
            female: kind.age_f,
        });
        world
            .populations
            .heads(&request.court)
            .ok_or(Error::MissingPop)?;
        let mut out = SlotOutcome {
            placed: Vec::new(),
            generated: Vec::new(),
            unfilled: 0,
            warnings: Vec::new(),
            created: Vec::new(),
            archived: Vec::new(),
            output: None,
        };
        for place in 0..request.count {
            let candidates = self.slot_candidates(world, request, &ages)?;
            let mut stream = court_stream(
                world.seed,
                &request.court,
                &format!("slot.{}.{}.{place}", request.key, request.kind),
                request.today,
            );
            if let Some(id) = Self::pick_weighted(&mut stream, &candidates)? {
                self.assign_slot(world, txn, id, request)?;
                out.placed.push(id);
            } else {
                self.generate_for_slot(world, txn, request, &ages, place, &mut out)?;
            }
        }
        Ok(out)
    }
}

/// The result of generating the other members of a slot.
#[derive(Debug)]
pub struct Made {
    /// The members, in the order asked.
    pub ids: Vec<u64>,
    /// Every live character made, in order (the members' living ancestors included).
    pub created: Vec<u64>,
    /// Every dead ancestor made.
    pub archived: Vec<u64>,
    /// The first failure after the commit.
    pub output: Option<EventError>,
}
