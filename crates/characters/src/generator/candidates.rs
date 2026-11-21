//! Generated marriage candidates (CK-369 §6, 5c §3): when the pool of a seeker has fewer than `min_candidates`, candidates are generated
//! and **checked against the pool rules before they count** (other sex, marriageable age by religion, no living spouse, not kin). A
//! candidate's age is drawn within the seeker's free age gap **intersected with** its marriageable age and the ends of the fertile curve of
//! its sex. A noble seeker gets a minor noble in a generated family of a **new minor house** keyed `<father's base name>_<place id>`
//! (`_2`, `_3`... on a collision); any other a commoner of the seeker's culture and pop type at the seeker's place. Each takes an existing
//! head (§2): a pop with no spare head yields no candidate. Each candidate is one transaction.
#![deny(clippy::float_arithmetic)]

use crate::{
    ArchiveSink, CharacterStore, Error, Populations, Sex, character_stream,
    events::{EventError, LogSink, World, age_years, keep},
    generator::{Birth, GenerateRequest, dates},
    genetics::below,
};
use ck_sim::GameTime;

/// The result of topping up a pool.
#[derive(Debug)]
pub struct PoolTopUp {
    /// The candidates that were made.
    pub made: Vec<u64>,
    /// The first failure after a commit (a life-log line that could not be appended): the candidates stand.
    pub output: Option<EventError>,
}

impl CharacterStore {
    /// Top up the pool of `seeker` (CK-369 §6): when [`marriage_pool`](Self::marriage_pool) has fewer than the data's `min_candidates`,
    /// generate candidates until it has that many or no more can be made. Returns the candidates that were made and the first output
    /// failure after a commit.
    ///
    /// # Errors
    /// A refusal that is not a lack of room (a pop that does not resolve, data that cannot be read, an archive that cannot append). A
    /// candidate that cannot be made for want of a head or of an age to draw is not an error: the pool stays short.
    pub fn top_up_pool<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        seeker: u64,
        today: GameTime,
    ) -> Result<PoolTopUp, EventError> {
        let minimum = world.lifecycle.marriage.pool.min_candidates as usize;
        let mut made = Vec::new();
        let mut output = None;
        if minimum == 0 || !world.lifecycle.generator.loaded {
            return Ok(PoolTopUp { made, output });
        }
        let have = self.marriage_pool(world, seeker, today)?.len();
        for index in 0..minimum.saturating_sub(have) {
            match self.generate_candidate(world, seeker, today, index, 0) {
                Ok(Some((id, failure))) => {
                    made.push(id);
                    if let Some(failure) = failure {
                        keep(&mut output, failure);
                    }
                }
                Ok(None) | Err(EventError::Rules(Error::PopFull)) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(PoolTopUp { made, output })
    }

    /// One candidate for `seeker`, or `None` when no age can be drawn for it.
    ///
    /// `relax` is how far the age window is loosened (CK-370 §3 step 5, a spouse a slot always gets): 0 is the pool's own (within the seeker's
    /// free gap, marriageable and inside the fertile ages); 1 drops the fertile ages (`[max(M, A - G), A + G]`: A the seeker's age, G the free
    /// gap, M her marriageable minimum); 2 is her marriageable minimum alone, for the case where that window is empty.
    pub(crate) fn generate_candidate<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &mut World<'_, P, A, L>,
        seeker: u64,
        today: GameTime,
        index: usize,
        relax: u8,
    ) -> Result<Option<(u64, Option<EventError>)>, EventError> {
        let s = self
            .live
            .get(&seeker)
            .cloned()
            .ok_or(Error::MissingCharacter)?;
        let rules = world.lifecycle;
        let sex = if s.sex == Sex::Male {
            Sex::Female
        } else {
            Sex::Male
        };
        let religion = world.populations.religion(&s.pop).ok_or(Error::Lifecycle)?;
        let marriageable = rules.marriageable.get(&religion).ok_or(Error::Lifecycle)?;
        let curve = if sex == Sex::Male {
            &rules.conception.men
        } else {
            &rules.conception.women
        };
        let age = age_years(dates::date_of(s.birth)?, dates::date_of(today)?);
        let gap = rules.marriage.score.age_gap_free;
        let own = if sex == Sex::Male {
            marriageable.m
        } else {
            marriageable.f
        };
        let (low, high) = match relax {
            0 => (
                age.saturating_sub(gap)
                    .max(own)
                    .max(curve.first().map_or(0, |point| point.0)),
                (age + gap).min(curve.last().map_or(0, |point| point.0)),
            ),
            1 => (age.saturating_sub(gap).max(own), age + gap),
            // Empty (the youngest marriageable age is beyond his gap): her age is the marriageable minimum.
            _ => (own, own),
        };
        if low > high {
            return Ok(None);
        }
        let mut stream = character_stream(world.seed, seeker, &format!("candidate.{index}"), today);
        let drawn = low
            + u32::try_from(below(&mut stream, u64::from(high - low) + 1)?)
                .map_err(|_| Error::Overflow)?;
        let latest = dates::years_before(today, drawn)?;
        let earliest = dates::add_days(dates::years_before(today, drawn + 1)?, 1)?;
        let days =
            u64::try_from(dates::days_between(earliest, latest)).map_err(|_| Error::Generator)?;
        let born = dates::add_days(
            earliest,
            i64::try_from(below(&mut stream, days + 1)?).map_err(|_| Error::Overflow)?,
        )?;
        let noble = world.populations.kind(&s.pop).as_deref() == Some("noble");
        let mut request = GenerateRequest::new(
            s.pop.clone(),
            sex,
            Birth::On(born),
            &format!("candidate.{seeker}.{index}"),
            today,
        );
        if noble {
            request.minor_house_at = Some(s.pop.place.clone());
        }
        let done = self.transact(world, |store, staged, txn| {
            let id = store.generate_in(staged, txn, &request)?;
            // Checked against the pool rules before it counts: any refusal leaves nothing.
            let c = store
                .live
                .get(&id)
                .cloned()
                .ok_or(Error::MissingCharacter)?;
            let kin = [c.father, c.mother].contains(&s.id)
                || [s.father, s.mother].iter().any(|parent| {
                    *parent != crate::UNKNOWN && [c.father, c.mother].contains(parent)
                });
            if c.sex == s.sex
                || !store.unmarried(&c)
                || kin
                || !Self::marriageable(staged, &c, today)?
            {
                return Err(Error::InvalidRelationship.into());
            }
            Ok(id)
        });
        match done {
            Ok(done) => Ok(Some((done.value, done.output))),
            Err(EventError::Rules(Error::InvalidRelationship)) => Ok(None),
            Err(error) => Err(error),
        }
    }
}
