//! The simulation in the application (milestone 7, slice 7): the game's types, its owner thread, the controls that reach it,
//! saves and the snapshot it publishes to the UI.
//!
//! **All simulation code lives in this directory.** `build.rs` hashes every `.rs` file here, with `crates/sim` and
//! `crates/lua`, into the build id that saves carry (design §5.3): a change to any of them refuses older saves, and code
//! elsewhere in the application must not change simulation results.
//!
//! - [`core`]: the state machine, with no thread and no real clock. It is given a monotonic time and the controls that
//!   arrived, runs the due ticks and returns what to publish.
//! - [`owner`]: the thin thread shell, a `ck_runtime::Owner` called `sim`.
//! - [`controls`]: the input actions, and the presses that become controls.
//! - [`saves`]: the save header's identity, the quicksave file and the build id.
//! - [`hashes`]: the content hashes a save is checked against.
//! - [`slot`]: the latest-wins snapshot the owner publishes, and the terminal failure.
//! - [`setup`]: starting a new game or loading one, before anything runs.
//! - [`view`]: the immutable economy view the snapshot carries to the UI (milestone 9).
//! - [`character_view`]: the characters of the same view (slice 9, CK-375).
//! - [`character_relations`]: what the relations tab reads of that view (slice 9b, CK-377).

use ck_sim::{Canonical, CanonicalError, Reader};

pub(crate) mod character_relations;
mod character_storage;
pub(crate) mod character_view;
pub(crate) mod controls;
pub(crate) mod core;
pub(crate) mod economy;
pub(crate) mod events;
pub(crate) mod expedition_map;
pub(crate) mod expedition_panel;
pub(crate) mod expedition_route;
pub(crate) mod gameplay;
pub(crate) mod hashes;
mod lows;
pub(crate) mod owner;
pub(crate) mod saves;
pub(crate) mod setup;
pub(crate) mod slot;
mod stretch;
#[cfg(test)]
pub(crate) mod tests;
pub(crate) mod trade_panel;
pub mod travel;
pub(crate) mod view;
pub(crate) mod weather_trial;
mod wind;

/// The simulation state. M9 slice 2 stores its deterministic starting economy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GameState {
    /// Authored population, buildings, stores, employment and prices.
    pub(crate) economy: ck_economy::EconomyState,
    /// State of each physical land stretch, keyed by its stable id.
    pub(crate) stretches: std::collections::BTreeMap<u64, stretch::StretchState>,
    /// The list of manually placed lows and its next identifier.
    pub(crate) lows: lows::Lows,
    /// Live characters and their next identifier.
    pub(crate) characters: ck_characters::CharacterStore,
}

impl Canonical for GameState {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.economy.encode(out)?;
        self.characters.encode(out)?;
        self.stretches.encode(out)?;
        self.lows.encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            economy: ck_economy::EconomyState::decode(input)?,
            characters: ck_characters::CharacterStore::decode(input)?,
            stretches: std::collections::BTreeMap::decode(input)?,
            lows: lows::Lows::decode(input)?,
        })
    }
}

/// The simulation's commands. There are none in milestone 7; the first come with the economy.
///
/// Pause, speed, quicksave and quickload are controls, not commands: they are not simulation state and are not journalled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GameCommand {}

impl Canonical for GameCommand {
    fn encode(&self, _out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        match *self {}
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        // No byte names a command.
        Err(CanonicalError::BadTag {
            ty: "GameCommand",
            tag: u8::decode(input)?,
        })
    }
}

/// How the production game applies a command: there is nothing to apply.
pub(crate) const fn apply(_state: &mut GameState, command: GameCommand) {
    match command {}
}
