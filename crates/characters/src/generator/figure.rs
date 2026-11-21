//! Real figures in the generator (CK-370, characters slice 8): the **fixed aspects** of a real figure, how an archived record ends, and where
//! the generated living relatives of clergy live at the 768 start.
//!
//! A real figure is made by the generator's own path (§2 of slice 7: parents, lineage, personality, body, orientation) **except** its fixed aspects,
//! which override what the recipe would give (real-characters-r4 rule 1): its name, its byname, its languages and its forced traits. Its
//! genetic baseline stays its parents' average, so its children regress toward its lineage and not toward its fixed values. The traits the
//! data excludes are never given to it.
#![deny(clippy::float_arithmetic)]

use crate::{
    ArchiveSink, Character, CharacterStore, Departure, Error, PopLocator, Populations,
    events::{LogSink, World},
};
use ck_sim::GameTime;

/// How an archived record ends: the end of a generated character that is not live (CK-370 §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// Dead on a day.
    Dead(GameTime),
    /// Dead, the date unknown ([`Departure::DeathUnknown`]).
    DeathUnknown,
    /// Abroad ([`Departure::Abroad`]): alive or dead unknown to the game.
    Abroad,
}

impl End {
    /// Give `record` this end: a death date, or the archive state that stands for none.
    pub(crate) fn apply(self, record: &mut Character) {
        match self {
            Self::Dead(day) => record.death = Some(day),
            Self::DeathUnknown => record.life.departure = Departure::DeathUnknown,
            Self::Abroad => {
                record.life.departure = Departure::Abroad;
                record.pop = no_pop();
            }
        }
    }
}

/// The fixed aspects of a real figure (CK-370 §1, §2): the part of a record that the data decides and the recipe does not.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Fixed {
    /// The authored key of a real figure or a dead parent (CK-375); kept on the record, so that the UI knows a real figure.
    pub key: String,
    /// The base name; never drawn from the pool, and never a namesake.
    pub name: String,
    /// The authored genitive of the name; empty for the pool's.
    pub genitive: String,
    /// The authored patronymic; empty for the pool's.
    pub patronymic: String,
    /// The byname, shown as an epithet is; empty for none.
    pub byname: String,
    /// The importance of the byname: the model's in `epithets.toml` where one matches, else 40.
    pub byname_importance: u32,
    /// The language key of the mother tongue; empty for the language of the culture of its pop.
    pub tongue: String,
    /// The second languages, each already rolled once on the figure's stream: language key and strength in thousandths.
    pub second_languages: Vec<(String, u32)>,
    /// The traits the sources give: key and level (1 for an ungraded key held or the lowest grade, up to 6).
    pub forced: Vec<(String, u8)>,
    /// The traits that are never given: key and the lowest level that is excluded (1 excludes the trait, 4 excludes grade S and above).
    pub excluded: Vec<(String, u8)>,
    /// The house the figure belongs to at the start: `Some("")` is none. `None` leaves the house its parents' gave.
    pub house: Option<String>,
    /// The house of birth where the data names one (empty: the parents' house, as the recipe gives it).
    pub house_of_birth: String,
    /// The spouses already made, linked from the figure's own record at creation (an archived record cannot be changed after it is appended).
    pub spouses: Vec<u64>,
    /// The authored strands of a figure that is **archived here**, written into its own record before it is appended (CK-370 §3 step 3): the
    /// partner's identifier, the kind and the strength. Its partner, live, keeps nothing of it: it is history, not a live bond. Empty for a
    /// live figure, whose strands are set after the burn-in.
    pub strands: Vec<(u64, String, u8)>,
    /// The authored **feelings others hold toward** a figure archived here (a live figure's love for it): the holder's identifier, the kind and
    /// the strength, kept in its snapshot as history, in the direction authored. Empty for a live figure.
    pub inbound: Vec<(u64, String, u8)>,
}

/// The year recorded as the last growth of a strand written into an archived record: the last year of the burn-in.
const ARCHIVED_STRAND_YEAR: u16 = 767;

/// Make the trait `key` show at `level` (the Mendelian conditions show or not; a graded trait or any other key takes the level).
fn force(c: &mut Character, key: &str, level: u8) {
    if let Some(condition) = c.genetics.mendelian.get_mut(key) {
        condition.shows = level > 0;
    } else {
        c.set_grade(key, level);
    }
}

/// Take the trait `key` down to below `from` when it stands at `from` or above: a condition stops showing, a graded trait falls to the
/// grade under the excluded one.
fn scrub(c: &mut Character, key: &str, from: u8) {
    if c.grade(key) < from {
        return;
    }
    if let Some(condition) = c.genetics.mendelian.get_mut(key) {
        condition.shows = false;
    } else {
        c.set_grade(key, from.saturating_sub(1));
    }
}

impl Fixed {
    /// Apply the fixed traits and the byname to a new record: the forced traits first, then the exclusions (a trait that is both is refused
    /// by the loader, so the order never decides).
    pub(crate) fn apply(&self, c: &mut Character) {
        c.life.figure.clone_from(&self.key);
        c.life.genitive.clone_from(&self.genitive);
        c.life.patronymic.clone_from(&self.patronymic);
        c.life.byname.clone_from(&self.byname);
        c.life.byname_importance = self.byname_importance;
        if let Some(house) = &self.house {
            c.life.house.clone_from(house);
        }
        if !self.house_of_birth.is_empty() {
            c.life.house_of_birth.clone_from(&self.house_of_birth);
        }
        for spouse in &self.spouses {
            if !c.spouses.contains(spouse) {
                c.spouses.push(*spouse);
            }
        }
        for (key, level) in &self.forced {
            force(c, key, *level);
        }
        self.scrub(c);
        for (holder, kind, strength) in &self.inbound {
            if let Some(at) = ck_economy::events::ALL_KINDS
                .iter()
                .position(|known| known == kind)
                .and_then(|at| u8::try_from(at).ok())
            {
                c.ties.inbound.push(crate::ties::InboundFeeling {
                    holder: *holder,
                    kind: at,
                    strength: *strength,
                });
            }
        }
        for (partner, kind, strength) in &self.strands {
            if let Some(at) = ck_economy::events::ALL_KINDS
                .iter()
                .position(|known| known == kind)
            {
                let link = c.ties.links.entry(*partner).or_default();
                link.strands[at].strength = *strength;
                link.strands[at].grew = ARCHIVED_STRAND_YEAR;
            }
        }
    }

    /// Take every excluded trait off `c` (a roll or the generated past may have given one).
    pub(crate) fn scrub(&self, c: &mut Character) {
        for (key, from) in &self.excluded {
            scrub(c, key, *from);
        }
    }
}

impl CharacterStore {
    /// Take the excluded traits off a live real figure again, after a generated past or a burn-in could have given one (CK-370 §1: if a roll or
    /// the generated past would give one, it is not given).
    pub(crate) fn scrub_excluded(&mut self, id: u64, fixed: &Fixed) {
        if let Some(c) = self.live.get_mut(&id) {
            fixed.scrub(c);
        }
    }
}

/// The pop of a record that has none on the map: an **abroad** relative (CK-370 §2). Its place is empty, so it names no pop of the economy.
#[must_use]
pub(crate) fn no_pop() -> PopLocator {
    PopLocator {
        place: String::new(),
        slot: 0,
    }
}

/// Where a generated living relative of a character lives (CK-370 §3 step 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Home {
    /// In this pop.
    Pop(PopLocator),
    /// Abroad: no pop, no head and no simulation.
    Abroad,
}

/// The pop kinds a relative of clergy may live in, in the order they are tried.
const BESIDE: [&str; 3] = ["peasant", "labourer", "noble"];

/// The nearest pop of each kind in turn (`BESIDE`'s order) among `pops`, by the distance the source reports from `from` (ties by place id,
/// then slot), or `None` when no pop of any kind has a measured distance.
fn nearest_by_kind<P: Populations>(
    populations: &P,
    from: &PopLocator,
    pops: &[(PopLocator, String)],
) -> Option<PopLocator> {
    BESIDE.into_iter().find_map(|kind| {
        pops.iter()
            .filter(|(_, of)| of == kind)
            .filter_map(|(locator, _)| {
                populations
                    .distance_m(from, locator)
                    .map(|metres| (metres, &locator.place, locator.slot, locator))
            })
            .min_by(|a, b| (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2)))
            .map(|(_, _, _, locator)| locator.clone())
    })
}

impl CharacterStore {
    /// Where the generated living relatives (parents, spouse, children) of a character in `pop` live (CK-370 §3 step 2), or `None` when no rule
    /// applies: the start's rule is off (play), or the character is lay and its place is not monastic (a relative of such a character lives in
    /// that character's pop). Anyone a slot names lives where the slot puts them: this moves only relatives the generator adds.
    ///
    /// - **A monastic place** (a monk pop and **no peasant pop and no noble pop**: a hermitage, an island monastery): the relatives of any
    ///   character placed there, figure or slot member, monk or lay, live at the **nearest** place (by the distance the pops report, ties by
    ///   place id, then slot) with a **peasant** pop of the character's culture, else the nearest **labourer** pop, else the nearest **noble**
    ///   pop, else **abroad**; the character stays where it is.
    /// - **A monk anywhere else**: in the place's **peasant** pop of the same culture, else its **labourer** pop, else its **noble** pop; with
    ///   none of these, at the nearest place that has a peasant pop of that culture, then a labourer pop, then a noble one; and **abroad** when
    ///   no non-monk pop of the culture exists on the map at all.
    ///
    /// # Errors
    /// A pop whose culture cannot be resolved, and non-monk pops of the culture that exist but are at no distance the source measures.
    pub(crate) fn relative_home<P: Populations, A: ArchiveSink, L: LogSink>(
        &self,
        world: &World<'_, P, A, L>,
        pop: &PopLocator,
    ) -> Result<Option<Home>, Error> {
        if !self.start.relatives_beside_monks {
            return Ok(None);
        }
        let monk = world.populations.kind(pop).as_deref() == Some("monk");
        let all = world.populations.all_pops();
        let here = |wanted: &str| {
            all.iter()
                .any(|(locator, kind, _)| locator.place == pop.place && kind == wanted)
        };
        let monastic = here("monk") && !here("peasant") && !here("noble");
        if !monastic && !monk {
            return Ok(None);
        }
        let culture = world.populations.culture(pop).ok_or(Error::Culture)?;
        let pops: Vec<(PopLocator, String)> = all
            .into_iter()
            .filter(|(_, kind, of)| *of == culture && BESIDE.contains(&kind.as_str()))
            .map(|(locator, kind, _)| (locator, kind))
            .collect();
        if pops.is_empty() {
            return Ok(Some(Home::Abroad));
        }
        if !monastic {
            // A monk outside a monastic place: the local pops of the culture first, in the order of `BESIDE`.
            for kind in BESIDE {
                if let Some((locator, _)) = pops
                    .iter()
                    .find(|(locator, of)| locator.place == pop.place && of == kind)
                {
                    return Ok(Some(Home::Pop(locator.clone())));
                }
            }
        }
        // Otherwise, and when the place has none of them, the nearest: peasant, then labourer, then noble.
        match nearest_by_kind(world.populations, pop, &pops) {
            Some(locator) => Ok(Some(Home::Pop(locator))),
            // Non-monk pops of the culture exist, but the source measures no distance to any of them: not abroad, and not placeable.
            None => Err(Error::Generator),
        }
    }
}
