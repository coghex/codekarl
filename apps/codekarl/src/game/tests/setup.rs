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
    let mut bytes = Vec::new();
    let mut meta = game_prepared.identity.meta(seed);
    meta.layers = ck_sim::MapLayers {
        rivers: true,
        roads: true,
    };
    simulation
        .save(meta)
        .write_with_sections(
            &mut bytes,
            &mut [
                (*b"ARCH", 0, &mut std::io::empty()),
                (*b"LOGS", 0, &mut std::io::empty()),
            ],
        )
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
    assert_eq!(
        loaded.layers,
        ck_sim::MapLayers {
            rivers: true,
            roads: true
        }
    );
    assert_eq!(loaded.simulation.seed(), 99);
    assert_eq!(loaded.simulation.time().hours(), 888 + loaded_start());
    assert_eq!(loaded.simulation.digest().unwrap(), digest);
    let snapshots = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let published = snapshots.clone();
    let _core = super::super::setup::test_core(
        loaded,
        game.saves.clone(),
        super::super::core::Outputs {
            log: Box::new(|_, _| {}),
            publish: Box::new(move |snapshot| published.lock().unwrap().push(snapshot)),
            fail: Box::new(|_| {}),
        },
    );
    let snapshots = snapshots.lock().unwrap();
    assert_eq!(snapshots.len(), 1);
    assert_eq!(
        snapshots[0].layers,
        ck_sim::MapLayers {
            rivers: true,
            roads: true
        }
    );
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
fn the_scenario_hash_covers_all_three_network_files_but_not_the_baseline() {
    let (_dir, game) = fixture();
    let scenario = &game.scenario;
    let without_network = hashes::scenario_hash(scenario).unwrap();
    fs::write(scenario.join("network.toml"), "hand list").unwrap();
    assert!(
        hashes::scenario_hash(scenario)
            .unwrap_err()
            .contains("roads.toml")
    );
    fs::write(scenario.join("roads.toml"), "stored network").unwrap();
    assert!(
        hashes::scenario_hash(scenario)
            .unwrap_err()
            .contains("sea_distance.png")
    );
    fs::write(scenario.join("sea_distance.png"), "sea distance layer").unwrap();
    let hash = || hashes::scenario_hash(scenario).unwrap();
    assert_ne!(hash(), without_network);
    each_file_matters(
        scenario,
        &["network.toml", "roads.toml", "sea_distance.png"],
        &hash,
    );
    let before = hash();
    fs::write(scenario.join("courses-approved.json"), "approved courses").unwrap();
    assert_eq!(hash(), before);
    fs::remove_file(scenario.join("network.toml")).unwrap();
    assert!(
        hashes::scenario_hash(scenario)
            .unwrap_err()
            .contains("network.toml")
    );
}

#[test]
fn a_refused_network_prevents_start_and_reports_its_file() {
    let (_dir, game) = fixture();
    fs::write(game.scenario.join("network.toml"), "hand list").unwrap();
    fs::write(game.scenario.join("roads.toml"), "format = 99\n").unwrap();
    fs::write(game.scenario.join("sea_distance.png"), "layer").unwrap();
    let message = prepared(&game, None, Some(42)).0.err().expect("refused");
    assert!(message.contains("roads.toml"), "{message}");
}

#[test]
fn a_network_changes_the_content_hash_but_not_the_simulation() {
    let (_dir, game) = fixture();
    let (save, _) = write_save(&game, 0, 42);
    let mut before = prepared(&game, None, Some(42)).0.unwrap();
    let old_hash = hashes::scenario_hash(&game.scenario).unwrap();
    let mut digests = vec![before.simulation.digest().unwrap()];
    for _ in 0..3 {
        before.simulation.step().unwrap();
        digests.push(before.simulation.digest().unwrap());
    }
    let crop = before.scenario.map.data.crop;
    // The fixture map's own files, as the stored network binds them (CK-401): terrain.png, rivers.toml and the layer beside the network.
    let mut roads = String::from(
        "format = 3\nhand_list_sha256 = \"cadb7f67e609caf88b363089d58964a35c2621a970915dc72fbd70449f1c4c4d\"\nbaseline_sha256 = \"unused\"\nnext_node_id = 3\nnext_stretch_id = 1\n\
         river_width_mm = 20000\n\
         terrain_sha256 = \"d11fe559195ce7d790dfd644e2bf9d8e82e9820e6564d710a694f0c0d1372b2f\"\n\
         rivers_sha256 = \"8898158a7e77c6ae6f7db850f2489486ed3b41dcc5153ac82fcdcdb1b5a819ff\"\n\
         sea_distance = \"sea_distance.png\"\n\
         sea_distance_sha256 = \"562db3788353cd656baca1ba6e8533ccf8e5fde34d7170ab7ca56d1d3e13567e\"\n\
         next_landing_id = 1\n",
    );
    for (index, location) in before.scenario.locations.iter().enumerate() {
        use std::fmt::Write as _;
        writeln!(
            roads,
            "[[node]]\nid = {}\nkind = \"place\"\ncell = [{}, {}]\nplace = \"{}\"",
            index + 1,
            location.cell.x - crop.origin.x,
            location.cell.y - crop.origin.y,
            location.id
        )
        .unwrap();
    }
    fs::write(game.scenario.join("network.toml"), "hand list").unwrap();
    fs::write(game.scenario.join("roads.toml"), roads).unwrap();
    fs::copy(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sea_distance.png"),
        game.scenario.join("sea_distance.png"),
    )
    .unwrap();
    assert_ne!(hashes::scenario_hash(&game.scenario).unwrap(), old_hash);
    let error = prepared(&game, Some(&save), None)
        .0
        .err()
        .expect("old save refused");
    assert!(error.contains("scenario hash"), "{error}");
    let mut after = prepared(&game, None, Some(42)).0.unwrap();
    assert!(after.scenario.network.is_some());
    assert_eq!(after.simulation.digest().unwrap(), digests[0]);
    for expected in &digests[1..] {
        after.simulation.step().unwrap();
        assert_eq!(after.simulation.digest().unwrap(), *expected);
    }
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
            "climate_half_range.png",
            "climate_sea_nearness.png",
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
    fs::write(game_rules.with_file_name("peoples.toml"), "registry").unwrap();
    write_genetics(&dir.0, "haplogroups", "traits");

    let before = hashes::rules_hash(&dir.0).unwrap();
    fs::write(&game_rules, "[footprint_m2]\nfarm = 2001\n").unwrap();
    assert_ne!(hashes::rules_hash(&dir.0).unwrap(), before);
}

fn write_genetics(root: &Path, haplogroups: &str, traits: &str) {
    fs::write(root.join("game/weather.toml"), "weather").unwrap();
    let genetics = root.join("game/genetics");
    fs::create_dir_all(&genetics).unwrap();
    fs::write(genetics.join("haplogroups.toml"), haplogroups).unwrap();
    fs::write(genetics.join("traits.toml"), traits).unwrap();
    // The rules hash also covers the personality data (CK-362); fixtures that do not edit it share one text.
    fs::write(root.join("game/personality.toml"), "personality").unwrap();
    // And the character event data (CK-363).
    fs::create_dir_all(root.join("game/events")).unwrap();
    fs::write(root.join("game/events/character-events.toml"), "events").unwrap();
    fs::write(
        root.join("game/events/attributes-skills.toml"),
        "attributes",
    )
    .unwrap();
    // And the life-cycle data (CK-364).
    fs::create_dir_all(root.join("game/lifecycle")).unwrap();
    fs::write(root.join("game/lifecycle/life-cycle.toml"), "life").unwrap();
    fs::write(root.join("game/lifecycle/names.toml"), "names").unwrap();
    // And the epithets (CK-365).
    fs::write(root.join("game/lifecycle/epithets.toml"), "epithets").unwrap();
    // And the connections and passions (CK-368).
    fs::write(root.join("game/lifecycle/connections.toml"), "connections").unwrap();
    // And the generator (CK-369).
    fs::write(root.join("game/lifecycle/generator.toml"), "generator").unwrap();
}

#[test]
fn editing_any_life_cycle_file_changes_the_rules_hash() {
    let dir = Dir::new();
    fs::create_dir_all(dir.0.join("game")).unwrap();
    fs::create_dir_all(dir.0.join("map/ireland")).unwrap();
    fs::write(dir.0.join("game/economy.toml"), "rules").unwrap();
    fs::write(dir.0.join("game/peoples.toml"), "registry").unwrap();
    fs::write(dir.0.join("map/ireland/terrain.toml"), "terrain").unwrap();
    write_genetics(&dir.0, "haplogroups", "traits");
    let before = hashes::rules_hash(&dir.0).unwrap();
    fs::write(dir.0.join("game/lifecycle/life-cycle.toml"), "life edited").unwrap();
    let life = hashes::rules_hash(&dir.0).unwrap();
    assert_ne!(life, before);
    fs::write(dir.0.join("game/lifecycle/names.toml"), "names edited").unwrap();
    let names = hashes::rules_hash(&dir.0).unwrap();
    assert_ne!(names, life);
    fs::write(
        dir.0.join("game/lifecycle/epithets.toml"),
        "epithets edited",
    )
    .unwrap();
    let epithets = hashes::rules_hash(&dir.0).unwrap();
    assert_ne!(epithets, names);
    fs::write(
        dir.0.join("game/lifecycle/connections.toml"),
        "connections edited",
    )
    .unwrap();
    let connections = hashes::rules_hash(&dir.0).unwrap();
    assert_ne!(connections, epithets);
    fs::write(
        dir.0.join("game/lifecycle/generator.toml"),
        "generator edited",
    )
    .unwrap();
    assert_ne!(hashes::rules_hash(&dir.0).unwrap(), connections);
}

#[test]
fn editing_the_personality_data_changes_the_rules_hash() {
    let dir = Dir::new();
    fs::create_dir_all(dir.0.join("game")).unwrap();
    fs::create_dir_all(dir.0.join("map/ireland")).unwrap();
    fs::write(dir.0.join("game/economy.toml"), "rules").unwrap();
    fs::write(dir.0.join("game/peoples.toml"), "registry").unwrap();
    fs::write(dir.0.join("map/ireland/terrain.toml"), "terrain").unwrap();
    write_genetics(&dir.0, "haplogroups", "traits");
    let before = hashes::rules_hash(&dir.0).unwrap();
    fs::write(dir.0.join("game/personality.toml"), "personality edited").unwrap();
    assert_ne!(hashes::rules_hash(&dir.0).unwrap(), before);
}

#[test]
fn editing_either_character_event_file_changes_the_rules_hash() {
    let dir = Dir::new();
    fs::create_dir_all(dir.0.join("game")).unwrap();
    fs::create_dir_all(dir.0.join("map/ireland")).unwrap();
    fs::write(dir.0.join("game/economy.toml"), "rules").unwrap();
    fs::write(dir.0.join("game/peoples.toml"), "registry").unwrap();
    fs::write(dir.0.join("map/ireland/terrain.toml"), "terrain").unwrap();
    write_genetics(&dir.0, "haplogroups", "traits");
    let before = hashes::rules_hash(&dir.0).unwrap();
    fs::write(
        dir.0.join("game/events/character-events.toml"),
        "events edited",
    )
    .unwrap();
    let events = hashes::rules_hash(&dir.0).unwrap();
    assert_ne!(events, before);
    fs::write(
        dir.0.join("game/events/attributes-skills.toml"),
        "attributes edited",
    )
    .unwrap();
    assert_ne!(hashes::rules_hash(&dir.0).unwrap(), events);
}

#[test]
fn editing_either_genetic_data_file_changes_the_rules_hash() {
    let dir = Dir::new();
    fs::create_dir_all(dir.0.join("game")).unwrap();
    fs::create_dir_all(dir.0.join("map/ireland")).unwrap();
    fs::write(dir.0.join("game/economy.toml"), "rules").unwrap();
    fs::write(dir.0.join("game/peoples.toml"), "registry").unwrap();
    fs::write(dir.0.join("map/ireland/terrain.toml"), "terrain").unwrap();
    write_genetics(&dir.0, "haplogroups", "traits");
    let before = hashes::rules_hash(&dir.0).unwrap();
    write_genetics(&dir.0, "haplogroups edited", "traits");
    let haplogroups = hashes::rules_hash(&dir.0).unwrap();
    assert_ne!(haplogroups, before);
    write_genetics(&dir.0, "haplogroups", "traits edited");
    let traits = hashes::rules_hash(&dir.0).unwrap();
    assert_ne!(traits, before);
    assert_ne!(traits, haplogroups);
}

#[test]
fn registry_edit_changes_rules_hash_and_refuses_existing_save() {
    let dir = Dir::new();
    let game = dir.0.join("game");
    let map = dir.0.join("map/ireland");
    fs::create_dir_all(&game).unwrap();
    fs::create_dir_all(&map).unwrap();
    fs::write(game.join("economy.toml"), "rules").unwrap();
    fs::write(game.join("peoples.toml"), "registry before").unwrap();
    fs::write(map.join("terrain.toml"), "terrain").unwrap();
    write_genetics(&dir.0, "haplogroups", "traits");
    let mut identity = super::identity("aa");
    identity.rules_hash = hashes::rules_hash(&dir.0).unwrap();
    let sim = Simulation::<super::World, GameCommand>::new(
        super::start(),
        42,
        super::World::default(),
        super::scheduler(super::Faults::default()),
        super::apply_nothing,
    );
    let bytes = sim.save(identity.meta(42)).to_bytes().unwrap();
    assert!(ck_sim::Save::<super::World>::from_bytes(&bytes, &identity.expected()).is_ok());
    fs::write(game.join("peoples.toml"), "registry after").unwrap();
    let changed = hashes::rules_hash(&dir.0).unwrap();
    assert_ne!(changed, identity.rules_hash);
    identity.rules_hash = changed;
    assert!(matches!(
        ck_sim::Save::<super::World>::from_bytes(&bytes, &identity.expected()),
        Err(ck_sim::SaveError::HashMismatch {
            which: ck_sim::HashKind::Rules,
            ..
        })
    ));
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
    let mut bytes = Vec::new();
    prepared
        .simulation
        .save(prepared.identity.meta(42))
        .write_with_sections(
            &mut bytes,
            &mut [
                (*b"ARCH", 0, &mut std::io::empty()),
                (*b"LOGS", 0, &mut std::io::empty()),
            ],
        )
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
        "crates/characters/Cargo.toml",
        "crates/map/Cargo.toml",
    ]
    .map(str::to_owned)
    .to_vec();
    for directory in [
        "crates/sim/src",
        "crates/lua/src",
        "crates/economy/src",
        "crates/characters/src",
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
    assert!(
        files
            .iter()
            .any(|file| file == "crates/characters/src/lib.rs")
    );
    let mut changed = framed.clone();
    let character_contents = fs::read(root.join("crates/characters/src/lib.rs")).unwrap();
    let offset = changed
        .windows(character_contents.len())
        .position(|window| window == character_contents)
        .unwrap();
    changed[offset] ^= 1;
    assert_ne!(fnv1a(&changed), fnv1a(&framed));
    assert!(files.iter().any(|file| file == "crates/map/src/lib.rs"));
}

#[test]
fn weather_is_ignored_when_economy_rules_are_absent() {
    let (_dir, game) = fixture();
    let root = game.scenario.parent().unwrap().parent().unwrap();
    let empty_hash = hashes::rules_hash(root).unwrap();
    fs::create_dir_all(root.join("game")).unwrap();
    fs::write(root.join("game/weather.toml"), "not = [valid").unwrap();
    assert_eq!(hashes::rules_hash(root).unwrap(), empty_hash);
    let game = prepared(&game, None, Some(42)).0.unwrap();
    let view = crate::game::setup::view_builder(game.view_wiring())(
        game.simulation.state(),
        date(768, 1, 1, 0),
    );
    assert!(view.places.iter().all(|place| place.temperature.is_none()));
}

#[test]
fn weather_map_changes_refuse_existing_saves_through_real_loader() {
    for (name, replacement) in [
        (
            "climate_half_range.png",
            include_bytes!("../../../tests/fixtures/map/climate_half_range_changed.png").as_slice(),
        ),
        (
            "climate_sea_nearness.png",
            include_bytes!("../../../tests/fixtures/map/climate_sea_nearness_changed.png")
                .as_slice(),
        ),
    ] {
        let (_dir, game) = fixture();
        let (save, _) = write_save(&game, 0, 42);
        fs::write(map_directory(&game).join(name), replacement).unwrap();
        let error = prepared(&game, Some(&save), None).0.err().unwrap();
        assert!(error.contains("map hash"), "{name}: {error}");
    }
    let (_dir, game) = fixture();
    let (save, _) = write_save(&game, 0, 42);
    let manifest = map_directory(&game).join("map.toml");
    let text = fs::read_to_string(&manifest).unwrap().replace(
        "warmest_month_phase_hundredths = 650",
        "warmest_month_phase_hundredths = 651",
    );
    fs::write(manifest, text).unwrap();
    let error = prepared(&game, Some(&save), None).0.err().unwrap();
    assert!(error.contains("map hash"), "{error}");
}

#[test]
fn the_shipped_scenario_starts_with_its_landings_and_its_straits() {
    let dir = Dir::new();
    let game = GameConfig {
        scenario: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenarios/ireland-768"),
        saves: dir.0.join("saves"),
    };
    let started = prepared(&game, None, Some(42))
        .0
        .expect("the shipped scenario starts");
    let network = started.scenario.network.as_ref().expect("a stored network");
    assert_eq!(network.landings.len(), 213);
    assert_eq!(network.straits.len(), 10);
    let landed: std::collections::BTreeSet<u64> = network.landings.iter().map(|l| l.node).collect();
    let places = network
        .nodes
        .iter()
        .filter(|n| n.kind == ck_map::LandNodeKind::Place && landed.contains(&n.id))
        .count();
    assert_eq!(places, 39);
}

#[test]
fn the_scenario_hash_changes_when_one_byte_of_the_sea_distance_layer_changes() {
    let game = GameConfig {
        scenario: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/scenarios/ireland-768"),
        saves: PathBuf::new(),
    };
    let dir = Dir::new();
    let copy = dir.0.join("assets/scenarios/ireland-768");
    fs::create_dir_all(&copy).unwrap();
    for entry in fs::read_dir(&game.scenario).unwrap() {
        let entry = entry.unwrap();
        if entry.file_type().unwrap().is_file() {
            fs::copy(entry.path(), copy.join(entry.file_name())).unwrap();
        }
    }
    let before = hashes::scenario_hash(&copy).unwrap();
    let mut layer = fs::read(copy.join("sea_distance.png")).unwrap();
    let last = layer.len() - 1;
    layer[last] ^= 1;
    fs::write(copy.join("sea_distance.png"), layer).unwrap();
    assert_ne!(hashes::scenario_hash(&copy).unwrap(), before);
}
