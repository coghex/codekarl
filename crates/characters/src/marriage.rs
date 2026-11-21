//! Marriage, the automatic part (CK-366, characters slice 5c): houses, the wish, the pool, scoring, acceptance and the marriage as one
//! transition.
//!
//! In the yearly birthday pass (after 5b's epithets) an unmarried character of marriageable age may seek a marriage: a roll on its
//! own stream (purpose `marriage.wish`) decides whether it does, the pool of candidates is built nearest first, the seeker scores
//! each candidate and proposes to the best, and the candidate's side accepts by chance. On acceptance one validated transition
//! (all or nothing) links the spouses, gives the wife her husband's house (a houseless husband takes his wife's), moves the head of the
//! spouse who moves from its pop to the other's, and then reports the `marriage` happening.
//!
//! Every chance is exact: a ratio of integers converted once to parts per billion, rounded down. A score is kept in nanopoints (a
//! billionth of a point), so that its terms, which come in thousandths and millionths, add exactly. No floating point.
#![deny(clippy::float_arithmetic)]

use crate::{
    ArchiveSink, Character, CharacterStore, Error, Office, Sex, UNKNOWN, character_stream,
    cycle::curve_ratio,
    events::{EventError, FirstError, LogSink, World, age_years, exact_ppb, keep},
    genetics::roll,
};
use ck_economy::{bigint::Big, genetics::PPB};
use ck_sim::GameTime;
use std::collections::BTreeSet;

/// The index of `lust_chastity` among the vice/virtue pairs.
const LUST_CHASTITY: usize = 2;
/// The index of `solitary_gregarious` among the manner pairs.
const SOLITARY_GREGARIOUS: usize = 6;
/// The factor the personality terms of the wish are held above: a tenth.
const PERSONALITY_FLOOR: (u128, u128) = (1, 10);
/// A billion: parts per billion in one, and nanopoints in one point.
const BILLION: i128 = 1_000_000_000;
/// A million: nanopoints in one thousandth of a point.
const MILLION: i128 = 1_000_000;
/// The lowest and highest chance a candidate's side accepts with, in ppb.
const ACCEPT_BOUNDS: (i128, i128) = (50_000_000, 950_000_000);

/// The result of a marriage that committed: the couple, and the first output failure after the commit (a life-log line that could
/// not be appended, or a happening that could not run), which does not undo it.
#[derive(Debug)]
pub struct Wedding {
    /// The husband.
    pub husband: u64,
    /// The wife.
    pub wife: u64,
    /// The first failure after the commit.
    pub output: Option<EventError>,
}

/// A character's age in whole years on `today`.
fn age(c: &Character, today: GameTime) -> Result<u32, Error> {
    Ok(age_years(
        c.birth.date().map_err(|_| Error::Event)?,
        today.date().map_err(|_| Error::Event)?,
    ))
}

/// The ids of the parents of `c` that are known.
fn parents(c: &Character) -> impl Iterator<Item = u64> {
    [c.father, c.mother]
        .into_iter()
        .filter(|parent| *parent != UNKNOWN)
}

/// Whether two characters share at least one known parent.
fn share_parent(a: &Character, b: &Character) -> bool {
    parents(a).any(|parent| parents(b).any(|other| other == parent))
}

impl CharacterStore {
    /// Whether `c` has no living spouse: a widow or widower is unmarried (CK-366 §2).
    #[must_use]
    pub fn unmarried(&self, c: &Character) -> bool {
        !c.spouses
            .iter()
            .any(|spouse| self.live.contains_key(spouse))
    }

    /// Whether `c` is of marriageable age by its pop's religion (CK-364 `[marriageable]`). A pop whose religion is unknown, or a
    /// religion the data gives no age, is not marriageable.
    ///
    /// # Errors
    /// Refuses a date outside the calendar.
    pub fn marriageable<P: crate::Populations, A: ArchiveSink, L: LogSink>(
        world: &World<'_, P, A, L>,
        c: &Character,
        today: GameTime,
    ) -> Result<bool, Error> {
        let Some(ages) = world
            .populations
            .religion(&c.pop)
            .and_then(|religion| world.lifecycle.marriageable.get(&religion))
        else {
            return Ok(false);
        };
        let wanted = match c.sex {
            Sex::Male => ages.m,
            Sex::Female => ages.f,
        };
        Ok(age(c, today)? >= wanted)
    }

    /// The factors whose product is the exact yearly chance that `c` seeks a marriage (CK-366 §2), before it is held to 1: the age
    /// curve for its sex (linear between points, the end values beyond); the personality factor
    /// `max(0.1, (1 + lust_per_point x lust_chastity) x (1 + gregarious_per_point x solitary_gregarious))`; the pop type's factor
    /// (a missing one is 1). Each factor is `(numerator, denominator)`.
    ///
    /// # Errors
    /// Refuses a pop type the engine cannot resolve and a date outside the calendar.
    pub fn wish_ratios<P: crate::Populations, A: ArchiveSink, L: LogSink>(
        world: &World<'_, P, A, L>,
        c: &Character,
        today: GameTime,
    ) -> Result<Vec<(u128, u128)>, Error> {
        let wish = &world.lifecycle.marriage.wish;
        let points = match c.sex {
            Sex::Male => &wish.men,
            Sex::Female => &wish.women,
        };
        let mut ratios = vec![curve_ratio(points, age(c, today)?)];
        // Both terms are `1 + per point (millionths) x value (thousandths)` over a billion; a negative term floors the product.
        let term = |per_point: i64, value: i64| {
            i128::from(per_point)
                .checked_mul(i128::from(value))
                .and_then(|moved| moved.checked_add(BILLION))
        };
        let lust = term(wish.lust_per_point, c.personality.vice[LUST_CHASTITY]);
        let gregarious = term(
            wish.gregarious_per_point,
            c.personality.manner[SOLITARY_GREGARIOUS],
        );
        let (lust, gregarious) = (
            lust.ok_or(Error::Lifecycle)?,
            gregarious.ok_or(Error::Lifecycle)?,
        );
        // The product of the two terms is floored, not each term: two negative terms make a positive product. A product that is
        // negative or zero is below the floor, whatever its terms.
        let (lust_size, gregarious_size) = (lust.unsigned_abs(), gregarious.unsigned_abs());
        let positive = (lust < 0) == (gregarious < 0) && lust != 0 && gregarious != 0;
        let floor = Big::new(u128::from(PPB) * u128::from(PPB) / 10);
        if positive
            && Big::new(lust_size)
                .mul(&Big::new(gregarious_size))
                .cmp(&floor)
                != std::cmp::Ordering::Less
        {
            ratios.push((lust_size, u128::from(PPB)));
            ratios.push((gregarious_size, u128::from(PPB)));
        } else {
            ratios.push(PERSONALITY_FLOOR);
        }
        let kind = world.populations.kind(&c.pop).ok_or(Error::Pop)?;
        if let Some(factor) = wish.pop_type.get(&kind) {
            ratios.push((u128::from(*factor), u128::from(PPB)));
        }
        Ok(ratios)
    }

    /// The yearly chance in ppb that `c` seeks a marriage: [`wish_ratios`](Self::wish_ratios), exact, **capped at 1** and rounded
    /// down once.
    ///
    /// # Errors
    /// As [`wish_ratios`](Self::wish_ratios).
    pub fn wish_ppb<P: crate::Populations, A: ArchiveSink, L: LogSink>(
        world: &World<'_, P, A, L>,
        c: &Character,
        today: GameTime,
    ) -> Result<u32, Error> {
        Ok(exact_ppb(PPB, &Self::wish_ratios(world, c, today)?))
    }

    /// The candidates `seeker` may be matched with (CK-366 §3), nearest first: live, of the other sex, with no living spouse, of
    /// marriageable age and of the seeker's religion; never a sibling (sharing at least one known parent), a parent or a child.
    /// The order is the same place first, then the distance between the places in whole metres (other places within `near_km`, then
    /// within `far_km`; a place farther than that or of unknown distance is out of reach), ties by id. Kinship is read from recorded
    /// parentage alone, never from a parent's list of children.
    ///
    /// # Errors
    /// Refuses a missing seeker, a pop whose religion the pool cannot resolve and a date outside the calendar.
    pub fn marriage_pool<P: crate::Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        seeker: u64,
        today: GameTime,
    ) -> Result<Vec<u64>, Error> {
        let s = self.live.get(&seeker).ok_or(Error::MissingCharacter)?;
        let religion = world.populations.religion(&s.pop).ok_or(Error::Lifecycle)?;
        let reach = world.lifecycle.marriage.pool.far_m;
        let mut pool: Vec<(bool, u64, u64)> = Vec::new();
        for c in self.live.values() {
            if c.sex == s.sex
                || c.id == s.id
                || !self.unmarried(c)
                || share_parent(s, c)
                || parents(s).any(|parent| parent == c.id)
                || parents(c).any(|parent| parent == s.id)
                || world.populations.religion(&c.pop).as_deref() != Some(religion.as_str())
                || !Self::marriageable(world, c, today)?
            {
                continue;
            }
            // The same place comes first whatever the distance of any other place: another place at no distance does not share its
            // tier.
            let same_place = c.pop.place == s.pop.place;
            let distance = if same_place {
                0
            } else {
                match world.populations.distance_m(&s.pop, &c.pop) {
                    Some(metres) if metres <= reach => metres,
                    _ => continue,
                }
            };
            pool.push((!same_place, distance, c.id));
        }
        pool.sort_unstable();
        Ok(pool.into_iter().map(|(_, _, id)| id).collect())
    }

    /// The record of `id`, live or archived; `None` for the unknown character and for one nobody holds.
    pub(crate) fn record<A: ArchiveSink>(
        &self,
        archive: &A,
        id: u64,
    ) -> Result<Option<Character>, Error> {
        if id == UNKNOWN {
            return Ok(None);
        }
        if let Some(live) = self.live.get(&id) {
            return Ok(Some(live.clone()));
        }
        archive.get(id).map_err(|_| Error::Archive)
    }

    /// The ancestors of `c` by generation, live or archived: the parents, the grandparents and the great-grandparents. A generation
    /// is empty when no record of its descendants is held; the unknown character is in none.
    pub(crate) fn ancestors<A: ArchiveSink>(
        &self,
        archive: &A,
        c: &Character,
    ) -> Result<[BTreeSet<u64>; 3], Error> {
        let mut generations: [BTreeSet<u64>; 3] = Default::default();
        let mut line: Vec<Character> = vec![c.clone()];
        for generation in &mut generations {
            let mut next = Vec::new();
            for descendant in &line {
                for parent in parents(descendant) {
                    if generation.insert(parent)
                        && let Some(record) = self.record(archive, parent)?
                    {
                        next.push(record);
                    }
                }
            }
            line = next;
        }
        Ok(generations)
    }

    /// The identifiers of the children of `id`, by recorded parentage alone: the live records that name it and the archived ones the
    /// archive's index of children by parent names. A parent's own list of children is not read: it can be stale after the child's
    /// parents were reassigned.
    pub(crate) fn children_of<A: ArchiveSink>(
        &self,
        archive: &A,
        id: u64,
    ) -> Result<BTreeSet<u64>, Error> {
        let mut found: BTreeSet<u64> = self
            .live
            .values()
            .filter(|c| c.father == id || c.mother == id)
            .map(|c| c.id)
            .collect();
        found.extend(archive.children_of(id).map_err(|_| Error::Archive)?);
        Ok(found)
    }

    /// The characters `seeker` is related to by marriage (CK-366 §4): a parent, child or sibling of a spouse of the seeker (living
    /// or dead), and a spouse (living or dead) of a sibling or child of the seeker.
    fn in_laws<A: ArchiveSink>(
        &self,
        archive: &A,
        seeker: &Character,
    ) -> Result<BTreeSet<u64>, Error> {
        let mut found = BTreeSet::new();
        for spouse in &seeker.spouses {
            let Some(spouse) = self.record(archive, *spouse)? else {
                continue;
            };
            found.extend(parents(&spouse));
            found.extend(self.children_of(archive, spouse.id)?);
            for parent in parents(&spouse) {
                found.extend(self.children_of(archive, parent)?);
            }
            found.remove(&spouse.id);
        }
        let mut relatives = self.children_of(archive, seeker.id)?;
        for parent in parents(seeker) {
            relatives.extend(self.children_of(archive, parent)?);
        }
        relatives.remove(&seeker.id);
        for relative in relatives {
            if let Some(record) = self.record(archive, relative)? {
                found.extend(record.spouses.iter().copied());
            }
        }
        found.remove(&UNKNOWN);
        found.remove(&seeker.id);
        Ok(found)
    }

    /// The kinship penalty of `candidate` to `seeker` in thousandths of a point (CK-366 §4), traced through parent links across live
    /// and archived records (dead ancestors count): **first cousins** share a known grandparent and no parent; **second cousins**
    /// share a known great-grandparent and no grandparent; **in-laws** as [`in_laws`](Self::in_laws) says. Where several apply only
    /// the most severe penalty counts. 0 for none.
    fn kinship<P: crate::Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        kin: &Kin,
        candidate: &Character,
    ) -> Result<i64, Error> {
        let theirs = self.ancestors(&*world.archive, candidate)?;
        let shared = |generation: usize| {
            kin.ancestors[generation]
                .intersection(&theirs[generation])
                .next()
                .is_some()
        };
        let penalties = &world.lifecycle.marriage.score.kin;
        let mut worst = 0_i64;
        let mut apply = |kind: &str| -> Result<(), Error> {
            worst = worst.min(*penalties.get(kind).ok_or(Error::Lifecycle)?);
            Ok(())
        };
        if shared(1) && !shared(0) {
            apply("first_cousin")?;
        }
        if shared(2) && !shared(1) {
            apply("second_cousin")?;
        }
        if kin.in_laws.contains(&candidate.id) {
            apply("in_law")?;
        }
        Ok(worst)
    }

    /// What `seeker` scores `candidate` (CK-366 §4), in nanopoints: `rank_gap x |rank(seeker) - rank(candidate)| + age_gap x max(0,
    /// |age difference| - age_gap_free) + opinion x opinion(seeker, candidate) + fame x candidate.fame + kinship`, ranks by pop type.
    ///
    /// # Errors
    /// Refuses a missing character, a pop type without a rank, arithmetic that does not fit and an archive failure.
    pub fn marriage_score<P: crate::Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        seeker: u64,
        candidate: u64,
        today: GameTime,
    ) -> Result<i128, Error> {
        let s = self.live.get(&seeker).ok_or(Error::MissingCharacter)?;
        let kin = Kin {
            ancestors: self.ancestors(&*world.archive, s)?,
            in_laws: self.in_laws(&*world.archive, s)?,
        };
        self.score(world, s, &kin, candidate, today)
    }

    fn score<P: crate::Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        s: &Character,
        kin: &Kin,
        candidate: u64,
        today: GameTime,
    ) -> Result<i128, Error> {
        let c = self.live.get(&candidate).ok_or(Error::MissingCharacter)?;
        let rules = &world.lifecycle.marriage.score;
        let rank = |who: &Character| -> Result<i64, Error> {
            let kind = world.populations.kind(&who.pop).ok_or(Error::Pop)?;
            rules.rank.get(&kind).copied().ok_or(Error::Pop)
        };
        let gap = i128::from(rank(s)?.abs_diff(rank(c)?));
        let difference = age(s, today)?.abs_diff(age(c, today)?);
        let excess = i128::from(difference.saturating_sub(rules.age_gap_free));
        let opinion = i128::from(crate::connections::opinion(
            world.personality,
            &world.lifecycle.connections,
            s,
            c,
            &crate::connections::PopTargets::of(world.populations, c),
        )?);
        let kinship = self.kinship(world, kin, c)?;
        let terms = [
            i128::from(rules.rank_gap).checked_mul(gap * MILLION),
            i128::from(rules.age_gap).checked_mul(excess * MILLION),
            i128::from(rules.opinion).checked_mul(opinion),
            i128::from(rules.fame).checked_mul(i128::from(c.personality.fame)),
            i128::from(kinship).checked_mul(MILLION),
        ];
        terms.into_iter().try_fold(0_i128, |sum, term| {
            sum.checked_add(term.ok_or(Error::Lifecycle)?)
                .ok_or(Error::Lifecycle)
        })
    }

    /// The candidate `seeker` proposes to and its score in nanopoints (CK-366 §4): the best score of the pool, ties by pool order.
    /// `None` when the pool is empty.
    ///
    /// # Errors
    /// As [`marriage_pool`](Self::marriage_pool) and [`marriage_score`](Self::marriage_score).
    pub fn marriage_proposal<P: crate::Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        seeker: u64,
        today: GameTime,
    ) -> Result<Option<(u64, i128)>, Error> {
        let pool = self.marriage_pool(world, seeker, today)?;
        if pool.is_empty() {
            return Ok(None);
        }
        let s = self.live.get(&seeker).ok_or(Error::MissingCharacter)?;
        let kin = Kin {
            ancestors: self.ancestors(&*world.archive, s)?,
            in_laws: self.in_laws(&*world.archive, s)?,
        };
        let mut best: Option<(u64, i128)> = None;
        for candidate in pool {
            let score = self.score(world, s, &kin, candidate, today)?;
            if best.is_none_or(|(_, top)| score > top) {
                best = Some((candidate, score));
            }
        }
        Ok(best)
    }

    /// The living member of `house` with the `ruler` marker of lowest id: the head of a ruling house (CK-366 §1, §5).
    #[must_use]
    pub fn house_head(&self, house: &str) -> Option<u64> {
        if house.is_empty() {
            return None;
        }
        // Among the house's members only (the lowest identifier first), never among everyone.
        self.by_house.get(house)?.iter().find_map(|id| {
            #[cfg(feature = "test-fixtures")]
            self.visited.0.set(self.visited.0.get() + 1);
            self.live
                .get(id)
                .filter(|c| c.office == Some(Office::Ruler))
                .map(|c| c.id)
        })
    }

    /// Whether `house` is a ruling house: it has a living member whose office marker is `ruler`.
    #[must_use]
    pub fn ruling_house(&self, house: &str) -> bool {
        self.house_head(house).is_some()
    }

    /// The chance in ppb that a candidate's side accepts a proposal of `score` nanopoints (CK-366 §5): `clamp(base + per_score x
    /// score, 0.05, 0.95)`, rounded down.
    ///
    /// # Errors
    /// Refuses arithmetic that does not fit.
    pub fn acceptance_ppb(
        rules: &ck_economy::lifecycle::MarriageAccept,
        score: i128,
    ) -> Result<u32, Error> {
        let moved = i128::from(rules.per_score_ppb)
            .checked_mul(score)
            .ok_or(Error::Lifecycle)?
            .div_euclid(BILLION);
        let chance = i128::from(rules.base_ppb)
            .checked_add(moved)
            .ok_or(Error::Lifecycle)?
            .clamp(ACCEPT_BOUNDS.0, ACCEPT_BOUNDS.1);
        u32::try_from(chance).map_err(|_| Error::Lifecycle)
    }

    /// The marriage wish of a character on its birthday (CK-366 §2-§5), the last stage of the pass: an unmarried character of
    /// marriageable age seeks a marriage with the yearly chance of [`wish_ppb`](Self::wish_ppb), proposes to the best candidate of
    /// its pool, and the candidate's side accepts by chance; a refusal ends the attempt for the year.
    ///
    /// # Errors
    /// A refusal of the engine or of the marriage's transition, with nothing changed by the failing step. A failure after the
    /// marriage committed is kept in `first`.
    pub(crate) fn seek_marriage<P: crate::Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        id: u64,
        today: GameTime,
        first: &mut FirstError,
    ) -> Result<(), EventError> {
        let Some(c) = self.live.get(&id) else {
            return Ok(());
        };
        if !self.unmarried(c) || !Self::marriageable(world, c, today)? {
            return Ok(());
        }
        let wish = Self::wish_ppb(world, c, today)?;
        if !roll(
            &mut character_stream(world.seed, id, "marriage.wish", today),
            wish,
        )? {
            return Ok(());
        }
        let Some((candidate, score)) = self.marriage_proposal(world, id, today)? else {
            return Ok(());
        };
        let chance = Self::acceptance_ppb(&world.lifecycle.marriage.accept, score)?;
        // A candidate of a ruling house is decided by the house's head, on the head's stream and for this pair alone; any other
        // candidate decides on its own stream, for the seeker alone.
        let house = self
            .live
            .get(&candidate)
            .map(|c| c.life.house.clone())
            .unwrap_or_default();
        let (decider, purpose) = match self.house_head(&house) {
            Some(head) => (head, format!("marriage.accept.{id}.{candidate}")),
            None => (candidate, format!("marriage.accept.{id}")),
        };
        if !roll(
            &mut character_stream(world.seed, decider, &purpose, today),
            chance,
        )? {
            return Ok(());
        }
        let wedding = self.arrange_marriage(world, id, candidate, today)?;
        if let Some(error) = wedding.output {
            keep(first, error);
        }
        Ok(())
    }

    /// The marriage of two live characters of the other sex, with no living spouse, as **one validated transition** (CK-366 §6):
    ///
    /// - the reciprocal spouse links (slice 1);
    /// - houses: the wife takes her husband's house and keeps her house of birth; a houseless husband takes his wife's house;
    /// - pops: the spouse who moves (the wife; a houseless husband marrying into a house moves instead) leaves its pop for the other
    ///   spouse's, and its own head moves with it by the cohort rule of a death (a minor a dependent if the source has one, else a
    ///   worker; an adult a worker if it has one, else a dependent; an employed worker's position is released), arriving in the
    ///   same cohort, the destination's genetics blending with one head of the source's. Both pops are checked after the whole
    ///   transition (each pop's heads still cover its characters); a head that cannot be taken or a failed check refuses the whole
    ///   marriage. Spouses already in one pop move no head;
    /// - then the `marriage` happening for both, with slice 4's precedence.
    ///
    /// # Errors
    /// A refusal, with nothing changed: a missing character, two of one sex, a spouse who is already married to a living one, a
    /// head that cannot be taken or a pop left without room. A failure after the commit is in [`Wedding::output`].
    pub fn arrange_marriage<P: crate::Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        a: u64,
        b: u64,
        today: GameTime,
    ) -> Result<Wedding, EventError> {
        let first = self.live.get(&a).cloned().ok_or(Error::MissingCharacter)?;
        let second = self.live.get(&b).cloned().ok_or(Error::MissingCharacter)?;
        let (husband, wife) = match (first.sex, second.sex) {
            (Sex::Male, Sex::Female) => (first, second),
            (Sex::Female, Sex::Male) => (second, first),
            _ => return Err(Error::InvalidRelationship.into()),
        };
        if !self.unmarried(&husband) || !self.unmarried(&wife) {
            return Err(Error::InvalidRelationship.into());
        }
        // A houseless husband marrying into a house moves to it; otherwise the wife moves to her husband.
        let husband_moves = husband.life.house.is_empty() && !wife.life.house.is_empty();
        let (mover, stays) = if husband_moves {
            (&husband, &wife)
        } else {
            (&wife, &husband)
        };
        let house = stays.life.house.clone();
        // The one step that can refuse and be undone: the head. Nothing else has changed yet.
        let staged = if mover.pop == stays.pop {
            None
        } else {
            let minor = Self::is_minor(world, mover, today)?;
            let undo = world.populations.move_head(&mover.pop, &stays.pop, minor)?;
            // Each pop's heads must still cover its characters once the mover has left one and joined the other.
            let leaves = self.pop_count(&mover.pop).saturating_sub(1);
            let joins = self.pop_count(&stays.pop) + 1;
            let enough = world
                .populations
                .heads(&mover.pop)
                .zip(world.populations.heads(&stays.pop))
                .is_some_and(|(source, destination)| {
                    source >= leaves as u64 && destination >= joins as u64
                });
            if !enough {
                world.populations.restore_move(&undo);
                return Err(Error::PopFull.into());
            }
            Some(undo)
        };
        if let Err(error) = self.marry(husband.id, wife.id, today) {
            if let Some(undo) = &staged {
                world.populations.restore_move(undo);
            }
            return Err(error.into());
        }
        // Everything is decided and checked: the writes below cannot fail.
        let destination = stays.pop.clone();
        for id in [husband.id, wife.id] {
            self.set_house(id, &house);
            if id == mover.id {
                self.relocate(id, destination.clone(), today);
            }
        }
        let mut output: FirstError = None;
        self.emit(
            world,
            &[husband.id, wife.id],
            "marriage",
            today,
            &mut output,
        );
        Ok(Wedding {
            husband: husband.id,
            wife: wife.id,
            output,
        })
    }
}

/// What the seeker's kinship to a candidate is read against, computed once for the seeker.
struct Kin {
    /// The seeker's parents, grandparents and great-grandparents.
    ancestors: [BTreeSet<u64>; 3],
    /// The characters related to the seeker by marriage.
    in_laws: BTreeSet<u64>,
}
