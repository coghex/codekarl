//! State-only mutators for fixtures (CK-363, feature `test-fixtures`, off by default).
//!
//! The game's mutations carry same-day consequences: a child is born, a spouse marries or dies, a vice is indulged, a character moves
//! and its attributes may fall (§§4 and 7). Those are the `*_with_events` methods, the only public mutation API. These forward to
//! the plain state changes for the fixtures that build characters without a world, and are not compiled into the game.
#![cfg(feature = "test-fixtures")]

use crate::{
    ArchiveSink, CharacterStore, Context, Error, LevelTrait, Lineage, NewCharacter, PopLocator,
    Populations,
};
use ck_economy::{Personality, PopGenetics};
use ck_sim::GameTime;

impl CharacterStore {
    /// `create_child` without the `child_born` happenings.
    ///
    /// # Errors
    /// As the state change.
    pub fn state_create_child(
        &mut self,
        ctx: &Context<'_>,
        new: NewCharacter,
        pop_genetics: &PopGenetics,
        lineage: &Lineage<'_>,
        populations: &impl Populations,
    ) -> Result<u64, Error> {
        self.create_child(ctx, new, pop_genetics, lineage, populations)
    }
    /// `die` without the `death_in_family` happenings.
    ///
    /// # Errors
    /// As the state change.
    pub fn state_die(
        &mut self,
        id: u64,
        day: GameTime,
        archive: &mut impl ArchiveSink,
    ) -> std::io::Result<()> {
        self.die(id, day, archive)
    }
    /// `die_of` without the `death_in_family` happenings.
    ///
    /// # Errors
    /// As the state change.
    pub fn state_die_of(
        &mut self,
        id: u64,
        day: GameTime,
        cause: Option<&str>,
        archive: &mut impl ArchiveSink,
    ) -> std::io::Result<()> {
        self.die_of(id, day, cause, archive)
    }
    /// `marry` without the `marriage` happenings.
    ///
    /// # Errors
    /// As the state change.
    pub fn state_marry(&mut self, a: u64, b: u64, day: GameTime) -> Result<(), Error> {
        self.marry(a, b, day)
    }
    /// `move_to` without precedence.
    ///
    /// # Errors
    /// As the state change.
    pub fn state_move_to(
        &mut self,
        id: u64,
        pop: PopLocator,
        day: GameTime,
        populations: &impl Populations,
    ) -> Result<(), Error> {
        self.move_to(id, pop, day, populations)
    }
    /// `indulge` without precedence.
    ///
    /// # Errors
    /// As the state change.
    pub fn state_indulge(&mut self, rules: &Personality, id: u64, pair: &str) -> Result<(), Error> {
        self.indulge(rules, id, pair)
    }
    /// `confess` without precedence.
    ///
    /// # Errors
    /// As the state change.
    pub fn state_confess(&mut self, rules: &Personality, id: u64) -> Result<(), Error> {
        self.confess(rules, id)
    }
    /// `set_trait_level` without precedence.
    ///
    /// # Errors
    /// As the state change.
    pub fn state_set_trait_level(
        &mut self,
        id: u64,
        which: LevelTrait,
        level: u8,
    ) -> Result<(), Error> {
        self.set_trait_level(id, which, level)
    }
}
