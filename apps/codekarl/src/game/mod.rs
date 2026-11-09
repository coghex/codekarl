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

use ck_sim::{Canonical, CanonicalError, Reader};

pub(crate) mod controls;
pub(crate) mod core;
pub(crate) mod economy;
pub(crate) mod gameplay;
pub(crate) mod hashes;
pub(crate) mod owner;
pub(crate) mod saves;
pub(crate) mod setup;
pub(crate) mod slot;
#[cfg(test)]
pub(crate) mod tests;

/// The simulation state. M9 slice 2 stores its deterministic starting economy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct GameState {
    /// Authored population, buildings, stores, employment and prices.
    pub(crate) economy: ck_economy::EconomyState,
}

impl Canonical for GameState {
    fn encode(&self, out: &mut Vec<u8>) -> Result<(), CanonicalError> {
        self.economy.encode(out)
    }

    fn decode(input: &mut Reader<'_>) -> Result<Self, CanonicalError> {
        Ok(Self {
            economy: ck_economy::EconomyState::decode(input)?,
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
