//! The decoder, its budgets and the assets rule, through the public API.
//!
//! What allocation a decode makes is measured in the `allocation` test binary, which is single-threaded.

mod assets;
mod budgets;
mod fixtures;
mod refusals;
mod reserving;

#[path = "../support/png.rs"]
mod png;
