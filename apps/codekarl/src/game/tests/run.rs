//! The game as a whole run: the configuration, `--load` and `--seed`, and the owner under the runtime.

use std::{
    fs,
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

use ck_window::Notifier;

use super::setup::{fixture, write_save};
use crate::{
    FAILED, Shared, cli::RunOptions, execute_with, game::tests::write_scenario, mode::Backend,
    testing::Dir, windowed::Plan,
};

fn shared() -> Shared {
    Shared {
        quit: Arc::new(AtomicBool::new(false)),
        notifier: Notifier::new(),
    }
}

fn options(config: &Path, headless: bool, load: Option<&Path>, seed: Option<u64>) -> RunOptions {
    RunOptions {
        config: Some(config.to_path_buf()),
        headless,
        backend: None,
        load: load.map(Path::to_path_buf),
        seed,
    }
}

/// A configuration beside the fixture scenario: `[game]` (and a native-metal window when `windowed`).
fn configuration(dir: &Dir, windowed: bool) -> std::path::PathBuf {
    let window = if windowed {
        "[window]\nbackend = \"native-metal\"\n"
    } else {
        ""
    };
    let path = dir.0.join("config.toml");
    fs::write(
        &path,
        format!("{window}[game]\nscenario = \"assets/scenarios/fixture\"\nsaves = \"saves\"\n"),
    )
    .unwrap();
    path
}

fn no_window(_: crate::windowed::Setup) -> Plan {
    panic!("a failed start-up must stop before any window plan");
}

#[test]
fn load_and_seed_need_a_configuration_with_a_game_table() {
    let dir = Dir::new();
    let config = dir.0.join("config.toml");
    fs::write(&config, "[map]\ndirectory = \"nowhere\"\n").unwrap();
    for options in [
        options(&config, true, Some(Path::new("save")), None),
        options(&config, true, None, Some(1)),
    ] {
        let status = execute_with(&options, shared(), &[Backend::NativeMetal], no_window);
        assert_eq!(status.code, FAILED.code);
    }
}

#[test]
fn a_game_run_without_a_window_starts_the_owner_and_stops_it_cleanly() {
    let (dir, _game) = fixture();
    let config = configuration(&dir, false);
    for seed in [Some(42), None] {
        let status = execute_with(
            &options(&config, true, None, seed),
            shared(),
            &[],
            no_window,
        );
        assert_eq!(status.code, 0);
        assert!(!status.owners_incomplete);
    }
}

#[test]
fn a_headless_load_checks_the_save_and_exits_0() {
    let (dir, game) = fixture();
    let (save, _) = write_save(&game, 24, 7);
    let config = configuration(&dir, false);
    let status = execute_with(
        &options(&config, true, Some(&save), None),
        shared(),
        &[],
        no_window,
    );
    assert_eq!(status.code, 0);
    assert!(!status.owners_incomplete);
}

#[test]
fn a_refused_load_exits_1_and_opens_no_window() {
    let (dir, game) = fixture();
    let (save, _) = write_save(&game, 24, 7);
    let config = configuration(&dir, true);
    let available = [Backend::NativeMetal];

    // One changed byte.
    let mut bytes = fs::read(&save).unwrap();
    bytes[0] ^= 1;
    fs::write(&save, bytes).unwrap();
    let status = execute_with(
        &options(&config, false, Some(&save), None),
        shared(),
        &available,
        no_window,
    );
    assert_eq!(status.code, FAILED.code);

    // A save that is not there.
    let missing = dir.0.join("missing.cksave");
    let status = execute_with(
        &options(&config, false, Some(&missing), None),
        shared(),
        &available,
        no_window,
    );
    assert_eq!(status.code, FAILED.code);
}

#[test]
fn an_invalid_start_date_is_a_configuration_error_before_any_window() {
    let dir = Dir::new();
    write_scenario(
        &dir.0.join("assets/scenarios/fixture"),
        ck_sim::Date {
            year: 769,
            month: 2,
            day: 29,
            hour: 0,
        },
    );
    let config = configuration(&dir, true);
    let status = execute_with(
        &options(&config, false, None, Some(1)),
        shared(),
        &[Backend::NativeMetal],
        no_window,
    );
    assert_eq!(status.code, FAILED.code);
}

#[test]
fn a_game_and_a_map_directory_together_are_a_configuration_error() {
    let (dir, _game) = fixture();
    let config = dir.0.join("config.toml");
    fs::write(
        &config,
        "[game]\nscenario = \"assets/scenarios/fixture\"\n[map]\ndirectory = \"assets/map/fixture\"\n",
    )
    .unwrap();
    let status = execute_with(
        &options(&config, true, None, None),
        shared(),
        &[],
        no_window,
    );
    assert_eq!(status.code, FAILED.code);
}
