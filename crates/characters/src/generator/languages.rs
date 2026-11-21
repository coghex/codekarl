//! Languages (CK-369 §5): the mother tongue of a culture, and second languages finalized once at the end of a generation and updated once
//! when a slot move places a fosterling or a hostage among another tongue. Every second language is a strength in thousandths, drawn
//! uniformly in the data's range on the stream of its source (purpose `language.<source>`); when two sources give one language the higher
//! strength stands.
#![deny(clippy::float_arithmetic)]

use crate::{
    ArchiveSink, Character, CharacterStore, Error, PopLocator, Populations, character_stream,
    events::{LogSink, World},
    genetics::below,
};
use ck_economy::events::{LATIN, StrengthRange};

/// The language key of the mother tongue of `c`: the one stored, else that of the culture of its pop.
fn tongue_of<P: Populations, A: ArchiveSink, L: LogSink>(
    world: &World<'_, P, A, L>,
    c: &Character,
) -> Option<String> {
    if !c.life.tongue.is_empty() {
        return Some(c.life.tongue.clone());
    }
    let culture = world.populations.culture(&c.pop)?;
    Some(
        world
            .lifecycle
            .generator
            .tongues
            .get(&culture)?
            .language
            .clone(),
    )
}

/// Give `record` the mother tongue of the culture of its pop: its language and dialect.
///
/// # Errors
/// Refuses a pop whose culture has no tongue.
pub(crate) fn set_tongue<P: Populations, A: ArchiveSink, L: LogSink>(
    world: &World<'_, P, A, L>,
    record: &mut Character,
) -> Result<(), Error> {
    let culture = world
        .populations
        .culture(&record.pop)
        .ok_or(Error::Culture)?;
    let tongue = world
        .lifecycle
        .generator
        .tongues
        .get(&culture)
        .ok_or(Error::Culture)?;
    record.life.tongue.clone_from(&tongue.language);
    record.life.dialect.clone_from(&tongue.dialect);
    Ok(())
}

/// Give a real figure the languages its data gives (CK-370 §1): the mother tongue (the dialect of the culture's tongue when it is the same
/// language, else none) and exactly the second languages it names, the mother tongue never being one of them.
///
/// # Errors
/// Refuses a pop whose culture has no tongue.
pub(crate) fn fix_languages<P: Populations, A: ArchiveSink, L: LogSink>(
    world: &World<'_, P, A, L>,
    record: &mut Character,
    fixed: &super::Fixed,
) -> Result<(), Error> {
    if !fixed.tongue.is_empty() {
        let culture = world
            .populations
            .culture(&record.pop)
            .ok_or(Error::Culture)?;
        let home = world
            .lifecycle
            .generator
            .tongues
            .get(&culture)
            .ok_or(Error::Culture)?;
        record.life.dialect = if home.language == fixed.tongue {
            home.dialect.clone()
        } else {
            String::new()
        };
        record.life.tongue.clone_from(&fixed.tongue);
    }
    record.life.languages.clear();
    for (language, strength) in &fixed.second_languages {
        if *language != record.life.tongue {
            let held = record.life.languages.entry(language.clone()).or_insert(0);
            *held = (*held).max(*strength);
        }
    }
    Ok(())
}

/// A strength drawn uniformly in `range` on the character's stream of purpose `language.<source>`.
fn strength(seed: u64, c: &Character, source: &str, range: StrengthRange) -> Result<u32, Error> {
    let span = u64::from(range.high - range.low) + 1;
    let pick = below(
        &mut character_stream(seed, c.id, &format!("language.{source}"), c.birth),
        span,
    )?;
    Ok(range.low + u32::try_from(pick).map_err(|_| Error::Overflow)?)
}

impl CharacterStore {
    /// Record the mother tongue of a live character made before the generator (one that holds none) as that of the culture of its pop
    /// **now**: before a move, so that a move among another tongue is measured against where it was raised.
    pub(crate) fn fix_tongue<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &World<'_, P, A, L>,
        id: u64,
    ) -> Result<(), Error> {
        let Some(c) = self.live.get(&id) else {
            return Ok(());
        };
        if !c.life.tongue.is_empty() {
            return Ok(());
        }
        let mut fixed = c.clone();
        set_tongue(world, &mut fixed)?;
        if let Some(live) = self.live.get_mut(&id) {
            live.life.tongue = fixed.life.tongue;
            live.life.dialect = fixed.life.dialect;
        }
        Ok(())
    }

    /// Add `language` at `strength` to a live character's second languages; the higher strength stands, and the mother tongue is never a
    /// second language.
    fn hold_language(&mut self, id: u64, language: &str, strength: u32) {
        if let Some(c) = self.live.get_mut(&id)
            && c.life.tongue != language
        {
            let slot = c.life.languages.entry(language.to_owned()).or_insert(0);
            *slot = (*slot).max(strength);
        }
    }

    /// Finalize the second languages of a generated character at the end of its generation (CK-369 §5): the Latin its role gives
    /// (`latin_by_office`), the Latin of a `latinist` gained in the past, and its mother's tongue when it differs from its own
    /// (`foreign_mother`).
    pub(crate) fn finalize_languages<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &World<'_, P, A, L>,
        id: u64,
        mother: Option<&Character>,
    ) -> Result<(), Error> {
        let rules = &world.lifecycle.generator.languages;
        let c = self.live.get(&id).ok_or(Error::MissingCharacter)?.clone();
        let mut offers: Vec<(String, u32)> = Vec::new();
        if let Some(range) = rules.latin_by_office.get(&c.life.role) {
            offers.push((
                LATIN.to_owned(),
                strength(world.seed, &c, "latin_by_office", *range)?,
            ));
        }
        if c.grade("latinist") > 0 {
            offers.push((
                LATIN.to_owned(),
                strength(world.seed, &c, "latinist", rules.latinist)?,
            ));
        }
        if let Some(mother) = mother
            && let Some(theirs) = tongue_of(world, mother)
            && theirs != c.life.tongue
        {
            offers.push((
                theirs,
                strength(world.seed, &c, "foreign_mother", rules.foreign_mother)?,
            ));
        }
        for (language, strength) in offers {
            self.hold_language(id, &language, strength);
        }
        Ok(())
    }

    /// Update the second languages of a character a slot move has placed in `court` (CK-369 §5), once: when the court's tongue is not the
    /// character's own, it gains it at the strength of the data's `source` (`fostered_among` for a fosterling, `hostage_raised` for a
    /// hostage).
    pub(crate) fn gain_court_tongue<P: Populations, A: ArchiveSink, L: LogSink>(
        &mut self,
        world: &World<'_, P, A, L>,
        id: u64,
        court: &PopLocator,
        source: &str,
    ) -> Result<(), Error> {
        let rules = &world.lifecycle.generator.languages;
        let range = match source {
            "fostered_among" => rules.fostered_among,
            "hostage_raised" => rules.hostage_raised,
            _ => return Err(Error::Slot),
        };
        let c = self.live.get(&id).ok_or(Error::MissingCharacter)?.clone();
        let culture = world.populations.culture(court).ok_or(Error::Culture)?;
        let theirs = world
            .lifecycle
            .generator
            .tongues
            .get(&culture)
            .ok_or(Error::Culture)?
            .language
            .clone();
        let own = tongue_of(world, &c).unwrap_or_default();
        if theirs != own {
            self.hold_language(id, &theirs, strength(world.seed, &c, source, range)?);
        }
        Ok(())
    }
}
