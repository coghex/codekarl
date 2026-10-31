//! `ck-window` with fakes that model the real adapter contracts: a draining (GLFW-like) and a capped
//! (native-like) pump, sticky wakes, a presenter whose end can be scripted, and a UI link that can be
//! full, answering or disconnected.
//!
//! One test binary means one process-wide `ck_log` logger; every test that drives the loop takes
//! `scenario()`'s lock, so the records of a run can be read from the shared sink by offset. Every test
//! that drives the loop runs it on its own thread with an overall deadline (`fakes::bounded`), and every
//! fake wait is bounded.

#[path = "window/fakes.rs"]
mod fakes;

#[path = "window/close.rs"]
mod close;
#[path = "window/input.rs"]
mod input;
#[path = "window/late.rs"]
mod late;
#[path = "window/notifier.rs"]
mod notifier;
#[path = "window/records.rs"]
mod records;
#[path = "window/setup.rs"]
mod setup;
#[path = "window/slot.rs"]
mod slot;
#[path = "window/teardown.rs"]
mod teardown;
#[path = "window/update.rs"]
mod update;
#[path = "window/work.rs"]
mod work;
