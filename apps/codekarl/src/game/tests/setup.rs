//! Starting a game: the scenario, the seed, the start date, `--load`, the content hashes and the build id.

use std::{
    fs,
    path::{Path, PathBuf},
};

use ck_log::Level;
use ck_sim::{GameTime, Scheduler, Simulation, fnv1a};

use super::{date, write_scenario};
use crate::{
    config::{GameConfig, VmConfig},
    game::{
        GameCommand, GameState, apply, hashes,
        saves::{build_id, quicksave_path},
        setup::{Prepared, Request, prepare},
    },
    testing::Dir,
};

fn game_config(directory: &Path) -> GameConfig {
    GameConfig {
        scenario: directory.join("assets/scenarios/fixture"),
        saves: directory.join("saves"),
    }
}

/// The fixture map's directory: `<root>/map/fixture`, beside the scenarios directory.
fn map_directory(game: &GameConfig) -> PathBuf {
    game.scenario
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("map/fixture")
}

/// A scenario directory and its game configuration.
pub(super) fn fixture() -> (Dir, GameConfig) {
    let dir = Dir::new();
    let game = game_config(&dir.0);
    write_scenario(&game.scenario, date(768, 1, 1, 0));
    (dir, game)
}

fn prepared(
    game: &GameConfig,
    load: Option<&Path>,
    seed: Option<u64>,
) -> (Result<Prepared, String>, Vec<(Level, String)>) {
    let mut records = Vec::new();
    let result = prepare(
        &Request {
            game,
            vms: &[],
            load,
            seed,
        },
        || Ok(0x1234_5678_9abc_def0),
        &mut |level, message| records.push((level, message)),
    );
    (result, records)
}

#[test]
fn a_new_game_starts_at_the_scenario_start_and_logs_the_seed_once() {
    let (_dir, game) = fixture();
    let (result, records) = prepared(&game, None, Some(42));
    let game = result.unwrap();
    assert_eq!(
        game.simulation.time(),
        GameTime::from_date(date(768, 1, 1, 0)).unwrap()
    );
    assert_eq!(game.simulation.seed(), 42);
    assert_eq!(game.scenario.id, "fixture_768");
    assert_eq!(
        records,
        [(Level::Info, "new game, seed 000000000000002a".to_owned())]
    );
}

#[test]
fn the_seed_at_both_ends_of_u64_is_sixteen_hex_digits() {
    let (_dir, game) = fixture();
    for (seed, text) in [(0, "0000000000000000"), (u64::MAX, "ffffffffffffffff")] {
        let (result, records) = prepared(&game, None, Some(seed));
        assert_eq!(result.unwrap().simulation.seed(), seed);
        assert_eq!(records, [(Level::Info, format!("new game, seed {text}"))]);
    }
}

#[test]
fn without_a_seed_the_operating_system_supplies_one_and_a_failure_is_a_start_up_error() {
    let (_dir, game) = fixture();
    let (result, records) = prepared(&game, None, None);
    assert_eq!(result.unwrap().simulation.seed(), 0x1234_5678_9abc_def0);
    assert_eq!(
        records,
        [(Level::Info, "new game, seed 123456789abcdef0".to_owned())]
    );

    let mut records = Vec::new();
    let refused = prepare(
        &Request {
            game: &game,
            vms: &[],
            load: None,
            seed: None,
        },
        || Err("no entropy".to_owned()),
        &mut |level, message| records.push((level, message)),
    );
    assert_eq!(refused.err(), Some("no entropy".to_owned()));
    assert!(records.is_empty());

    // The real source gives a seed (and two calls differ, with overwhelming probability).
    let first = crate::game::setup::operating_system_seed().unwrap();
    let second = crate::game::setup::operating_system_seed().unwrap();
    assert_ne!(first, second);
}

#[test]
fn an_invalid_start_date_is_a_configuration_error_naming_the_scenario() {
    let dir = Dir::new();
    let game = game_config(&dir.0);
    // 769 is not a Julian leap year.
    write_scenario(&game.scenario, date(769, 2, 29, 0));
    let (result, records) = prepared(&game, None, Some(1));
    let message = result.err().expect("refused");
    assert!(
        message.starts_with("[game] scenario `fixture_768`"),
        "{message}"
    );
    assert!(message.contains('2'), "{message}");
    assert!(records.is_empty());
}

#[test]
fn a_scenario_that_does_not_load_is_a_configuration_error() {
    let dir = Dir::new();
    let (result, _) = prepared(&game_config(&dir.0), None, Some(1));
    let message = result.err().expect("refused");
    assert!(message.starts_with("[game] "), "{message}");
    assert!(message.contains("scenario.toml"), "{message}");
}

/// Writes a save of a production game after `ticks` ticks and returns its path.
pub(super) fn write_save(game: &GameConfig, ticks: u64, seed: u64) -> (PathBuf, u64) {
    let (result, _) = prepared(game, None, Some(seed));
    let game_prepared = result.unwrap();
    let mut simulation = Simulation::<GameState, GameCommand>::new(
        game_prepared.simulation.time(),
        seed,
        GameState::default(),
        Scheduler::new(),
        apply,
    );
    for _ in 0..ticks {
        assert!(simulation.step().unwrap().is_empty());
    }
    let bytes = simulation
        .save(game_prepared.identity.meta(seed))
        .to_bytes()
        .unwrap();
    let path = quicksave_path(&game.saves);
    fs::create_dir_all(&game.saves).unwrap();
    fs::write(&path, bytes).unwrap();
    (path, simulation.digest().unwrap())
}

#[test]
fn a_loaded_game_is_the_save_exactly() {
    let (_dir, game) = fixture();
    let (path, digest) = write_save(&game, 888, 99);
    let (result, records) = prepared(&game, Some(&path), None);
    let loaded = result.unwrap();
    assert_eq!(loaded.simulation.seed(), 99);
    assert_eq!(loaded.simulation.time().hours(), 888 + loaded_start());
    assert_eq!(loaded.simulation.digest().unwrap(), digest);
    // A load logs the save, not a new seed.
    assert_eq!(records.len(), 1);
    assert!(records[0].1.starts_with("loaded "), "{records:?}");
    assert!(records[0].1.ends_with("at 0768-02-07 00:00"), "{records:?}");
}

fn loaded_start() -> u64 {
    GameTime::from_date(date(768, 1, 1, 0)).unwrap().hours()
}

#[test]
fn a_refused_save_is_a_configuration_error_with_the_named_error() {
    let (_dir, game) = fixture();
    let (path, _) = write_save(&game, 5, 1);
    let original = fs::read(&path).unwrap();

    // One changed byte at the end.
    let mut changed = original.clone();
    *changed.last_mut().unwrap() ^= 1;
    fs::write(&path, &changed).unwrap();
    let message = prepared(&game, Some(&path), None).0.err().expect("refused");
    assert!(message.contains("end checksum"), "{message}");
    assert!(message.contains(&path.display().to_string()), "{message}");

    // Not a save at all, and a file that is missing.
    fs::write(&path, b"hello").unwrap();
    let message = prepared(&game, Some(&path), None).0.err().expect("refused");
    assert!(message.contains("not a save file"), "{message}");
    let message = prepared(&game, Some(&path.with_extension("none")), None)
        .0
        .err()
        .expect("refused");
    assert!(message.contains("cannot read save"), "{message}");

    // The same save, but the scenario's content changed since: its hash is named.
    fs::write(&path, &original).unwrap();
    fs::write(game.scenario.join("locations.toml"), "# edited\n").unwrap();
    let message = prepared(&game, Some(&path), None).0.err().expect("refused");
    assert!(message.contains("scenario hash"), "{message}");
}

/// Every file in `files` changes `hash` when one byte is added to it, and the file is restored afterwards.
fn each_file_matters(directory: &Path, files: &[&str], hash: &dyn Fn() -> String) {
    let before = hash();
    for name in files {
        let path = directory.join(name);
        let original = fs::read(&path).unwrap();
        // One more byte, a newline, which every hashed file (TOML or PNG) can take without ceasing to be read.
        let mut changed = original.clone();
        changed.push(b'\n');
        fs::write(&path, &changed).unwrap();
        assert_ne!(hash(), before, "{name} is hashed");
        fs::write(&path, &original).unwrap();
        assert_eq!(hash(), before, "{name} restored");
    }
}

#[test]
fn the_scenario_hash_covers_the_scenario_file_and_its_locations_and_nothing_else() {
    let (_dir, game) = fixture();
    let scenario = game.scenario.clone();
    let hash = || hashes::scenario_hash(&scenario).unwrap();
    each_file_matters(&scenario, &["scenario.toml", "locations.toml"], &hash);
    let before = hash();
    fs::write(scenario.join("notes.txt"), "an unhashed file").unwrap();
    // Neither a new file beside it nor a change in the map directory touches it.
    let map_directory = map_directory(&game);
    fs::write(map_directory.join("terrain.png.bak"), "x").unwrap();
    assert_eq!(hash(), before);
    let mut map = fs::read(map_directory.join("map.toml")).unwrap();
    map.push(b'\n');
    fs::write(map_directory.join("map.toml"), map).unwrap();
    assert_eq!(hash(), before);
}

#[test]
fn the_map_hash_covers_the_map_file_every_layer_and_the_location_kinds() {
    let (_dir, game) = fixture();
    let map = map_directory(&game);
    fs::write(map.join("location_kinds.toml"), "# kinds\n").unwrap();
    let hash = || hashes::map_hash(&map).unwrap();
    each_file_matters(
        &map,
        &[
            "map.toml",
            "terrain.png",
            "fertility.png",
            "elevation.png",
            "climate_temperature.png",
            "climate_gdd.png",
            "climate_rainfall.png",
            "climate_glacier.png",
            "terrain.toml",
            "rivers.toml",
            "location_kinds.toml",
        ],
        &hash,
    );
    let before = hash();
    fs::write(map.join("notes.txt"), "an unhashed file").unwrap();
    fs::write(map.join("terrain.png.bak"), "x").unwrap();
    assert_eq!(hash(), before);
    // The location kinds file is optional; without it the hash is a different one.
    fs::remove_file(map.join("location_kinds.toml")).unwrap();
    assert_ne!(hash(), before);
}

#[test]
fn the_rules_hash_changes_when_a_building_footprint_changes() {
    let dir = Dir::new();
    let game_rules = dir.0.join("game/economy.toml");
    let terrain_rules = dir.0.join("map/ireland/terrain.toml");
    fs::create_dir_all(game_rules.parent().unwrap()).unwrap();
    fs::create_dir_all(terrain_rules.parent().unwrap()).unwrap();
    fs::write(&game_rules, "[footprint_m2]\nfarm = 2000\n").unwrap();
    fs::write(&terrain_rules, "[suitability]\n").unwrap();

    let before = hashes::rules_hash(&dir.0).unwrap();
    fs::write(&game_rules, "[footprint_m2]\nfarm = 2001\n").unwrap();
    assert_ne!(hashes::rules_hash(&dir.0).unwrap(), before);
}

#[test]
fn the_scripts_hash_is_the_gameplay_entry_script_or_nothing() {
    let dir = Dir::new();
    let entry = dir.0.join("gameplay.lua");
    fs::write(&entry, "-- first\n").unwrap();
    let first = hashes::scripts_hash(Some(&entry)).unwrap();
    fs::write(&entry, "-- second\n").unwrap();
    assert_ne!(hashes::scripts_hash(Some(&entry)).unwrap(), first);
    // No gameplay VM: no files, which hash to the FNV-1a of nothing.
    assert_eq!(hashes::scripts_hash(None).unwrap(), "cbf29ce484222325");
    assert!(hashes::scripts_hash(Some(&dir.0.join("missing.lua"))).is_err());

    // Only the VM named `gameplay` counts, and under the name `gameplay`, wherever the file is.
    let (_scratch, game) = fixture();
    let other = dir.0.join("ui.lua");
    fs::write(&other, "-- ui\n").unwrap();
    let hash_with = |vms: &[VmConfig]| {
        let mut records = Vec::new();
        prepare(
            &Request {
                game: &game,
                vms,
                load: None,
                seed: Some(1),
            },
            || Ok(1),
            &mut |level, message| records.push((level, message)),
        )
        .unwrap()
        .identity
        .scripts_hash
    };
    let vm = |name: &str, entry: &Path| VmConfig {
        name: name.to_owned(),
        entry: entry.to_path_buf(),
    };
    assert_eq!(hash_with(&[vm("ui", &other)]), "cbf29ce484222325");
    assert_eq!(
        hash_with(&[vm("ui", &other), vm("gameplay", &entry)]),
        hashes::scripts_hash(Some(&entry)).unwrap()
    );
}

#[test]
fn a_quickload_refuses_a_one_byte_changed_gameplay_script_by_scripts_hash() {
    let (_dir, game) = fixture();
    let entry = game.scenario.parent().unwrap().join("gameplay.lua");
    fs::write(&entry, "sim.on_day(function() end)\n").unwrap();
    let vms = [VmConfig {
        name: "gameplay".to_owned(),
        entry: entry.clone(),
    }];
    let mut records = Vec::new();
    let prepared = prepare(
        &Request {
            game: &game,
            vms: &vms,
            load: None,
            seed: Some(42),
        },
        || Ok(42),
        &mut |level, message| records.push((level, message)),
    )
    .expect("the original script prepares");
    let bytes = prepared
        .simulation
        .save(prepared.identity.meta(42))
        .to_bytes()
        .expect("the game save encodes");
    let path = quicksave_path(&game.saves);
    fs::create_dir_all(&game.saves).unwrap();
    fs::write(&path, bytes).unwrap();

    let mut changed = fs::read(&entry).unwrap();
    changed.push(b' ');
    fs::write(&entry, changed).unwrap();
    let refused = prepare(
        &Request {
            game: &game,
            vms: &vms,
            load: Some(&path),
            seed: None,
        },
        || Ok(0),
        &mut |level, message| records.push((level, message)),
    )
    .err()
    .expect("a save with different gameplay script bytes is refused");
    assert!(refused.contains("scripts_hash"), "{refused}");
}

#[test]
fn a_content_hash_is_the_fnv_of_the_sorted_name_and_content_framing() {
    let mut framed = Vec::new();
    for (name, contents) in [("a", &[2_u8, 3][..]), ("b", &[1][..])] {
        framed.extend_from_slice(&u32::try_from(name.len()).unwrap().to_le_bytes());
        framed.extend_from_slice(name.as_bytes());
        framed.extend_from_slice(&(contents.len() as u64).to_le_bytes());
        framed.extend_from_slice(contents);
    }
    let expected = format!("{:016x}", fnv1a(&framed));
    // The order the files are given in does not matter.
    let one = hashes::content_hash(vec![
        ("b".to_owned(), vec![1]),
        ("a".to_owned(), vec![2, 3]),
    ]);
    let two = hashes::content_hash(vec![
        ("a".to_owned(), vec![2, 3]),
        ("b".to_owned(), vec![1]),
    ]);
    assert_eq!(one, expected);
    assert_eq!(two, expected);
    // Moving a byte between the name and the content is a different framing.
    assert_ne!(
        hashes::content_hash(vec![("ab".to_owned(), vec![])]),
        hashes::content_hash(vec![("a".to_owned(), vec![b'b'])])
    );
}

/// The workspace root: two levels above the application.
fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn rust_files(root: &Path, relative: &str, found: &mut Vec<String>) {
    for entry in fs::read_dir(root.join(relative)).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name().into_string().unwrap();
        let child = format!("{relative}/{name}");
        if entry.file_type().unwrap().is_dir() {
            rust_files(root, &child, found);
        } else if Path::new(&name)
            .extension()
            .is_some_and(|extension| extension == "rs")
        {
            found.push(child);
        }
    }
}

#[test]
fn the_build_id_is_the_version_and_the_simulation_source_hash() {
    // The source set: the lock file and the toolchain, the manifests of `ck-sim` and `ck-lua`, every `.rs` under
    // `crates/sim/src`, `crates/lua/src` and `apps/codekarl/src/game`.
    let root = workspace();
    let mut files: Vec<String> = [
        "Cargo.lock",
        "rust-toolchain.toml",
        "crates/sim/Cargo.toml",
        "crates/lua/Cargo.toml",
        "crates/economy/Cargo.toml",
        "crates/map/Cargo.toml",
    ]
    .map(str::to_owned)
    .to_vec();
    for directory in [
        "crates/sim/src",
        "crates/lua/src",
        "crates/economy/src",
        "crates/map/src",
        "apps/codekarl/src/game",
    ] {
        rust_files(&root, directory, &mut files);
    }
    files.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    let mut framed = Vec::new();
    for file in &files {
        let contents = fs::read(root.join(file)).unwrap();
        framed.extend_from_slice(&u32::try_from(file.len()).unwrap().to_le_bytes());
        framed.extend_from_slice(file.as_bytes());
        framed.extend_from_slice(&(contents.len() as u64).to_le_bytes());
        framed.extend_from_slice(&contents);
    }
    let expected = format!("{:016x}", fnv1a(&framed));
    assert_eq!(env!("CK_SIM_SOURCES"), expected);
    assert_eq!(
        build_id(),
        format!("{}+{expected}", env!("CARGO_PKG_VERSION"))
    );
    // The id names the sources, not the executable: it has no feature or profile in it.
    assert_eq!(build_id().matches('+').count(), 1);
    assert!(
        files
            .iter()
            .any(|file| file == "apps/codekarl/src/game/core.rs")
    );
    assert!(files.iter().any(|file| file == "crates/sim/src/save.rs"));
    assert!(files.iter().any(|file| file == "crates/economy/src/lib.rs"));
    assert!(files.iter().any(|file| file == "crates/map/src/lib.rs"));
}
