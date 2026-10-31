//! The live-scene demonstration and the texture lifecycle through the **headless driver** (`testing::HeadlessDriver`),
//! milestone 5-8c.
//!
//! A production headless run is finite and posts no update, so it cannot create the demo's incrementally admitted tiles. The
//! driver composes exactly the headless composition (service, loader pool, bridge, both VMs), runs `startup()`, then posts the
//! UI's `frame_update` itself through the same posting path the window loop uses, with a fixed `dt`, and waits on the resource
//! service's own state with a whole-case deadline. Then the run ends normally. Each case names its negative control.

use std::path::PathBuf;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;

use ck_resources::JobGate;

use crate::testing::{Dir, HeadlessDriver, png};

/// The tile the held-job cases hold.
const HELD: &str = "demo-scene/tile-01-stripes-h.png";
/// The tile whose file does not exist.
const MISSING: &str = "demo-scene/missing-tile.png";

fn shipped_assets() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .canonicalize()
        .expect("the shipped assets directory")
}

/// The shipped demonstration script, read when the case runs (not at compile time: the toolchain's scratch copies hold no
/// `scripts/` directory and must still compile these tests).
fn demo_script() -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui/demo_scene.lua"),
    )
    .expect("the shipped demonstration script")
}

fn demo_driver(config: &str) -> HeadlessDriver {
    HeadlessDriver {
        assets: shipped_assets(),
        script: demo_script(),
        config: config.to_owned(),
    }
}

/// Every resource number zero after a reconciled end: the budgets, the registry's live entries and the referenced count.
fn assert_reconciled(service: &ck_resources::ResourceService) {
    let stats = service.stats();
    assert_eq!(stats.jobs.reserved, 0, "job slots");
    assert_eq!(stats.cpu_pixels.reserved, 0, "CPU pixel bytes");
    assert_eq!(stats.metadata.reserved, 0, "metadata bytes");
    assert_eq!(stats.registry.live, 0, "live entries");
    assert_eq!(stats.loaders.referenced_at_teardown, 0, "references");
}

#[test]
fn the_demo_admits_all_twelve_tiles_over_six_updates_and_every_texture_settles() {
    let run = demo_driver("").run(|drive| {
        for _ in 0..6 {
            drive.post_update();
        }
        // Two tiles per update: all twelve and the gem are admitted, no more.
        assert_eq!(drive.service.stats().registry.live, 13);
        drive.wait_until("every admitted job to finish", |drive| {
            drive.service.stats().jobs.reserved == 0
        });
        // One more update reads the statuses.
        drive.post_update();
    });
    assert_eq!(run.status.code, 0);
    assert!(!run.status.owners_incomplete);
    let latest = run.latest();
    assert_eq!(latest.len(), 13, "{latest:?}");
    let ready: Vec<&str> = latest
        .values()
        .filter(|reading| reading.state == "cpu_ready")
        .map(|reading| reading.name.as_str())
        .collect();
    assert_eq!(ready.len(), 12, "the gem and eleven tiles: {latest:?}");
    let missing = &latest[MISSING];
    assert_eq!(missing.state, "failed", "{missing:?}");
    assert!(
        !missing.error.is_empty(),
        "the missing tile's status carries its error: {missing:?}"
    );
    assert_reconciled(&run.service);
}

#[test]
fn without_any_update_only_the_gem_exists_so_the_completed_case_did_exercise_the_incremental_workload()
 {
    // The negative control of the case above: the same driver posting no update. The script's startup made the gem's
    // texture and quad and nothing else; none of the tiles was admitted (the script's tile counter is 0).
    let run = demo_driver("").run(|drive| {
        drive.wait_until("the gem's job to finish", |drive| {
            drive.service.stats().jobs.reserved == 0
        });
        assert_eq!(drive.service.stats().registry.live, 1);
    });
    assert_eq!(run.status.code, 0);
    assert!(run.readings.is_empty(), "no update ran, so nothing probed");
    assert_reconciled(&run.service);
}

#[test]
fn the_reload_binding_reloads_every_texture_of_the_demo_and_nothing_else_changes() {
    // `reload_texture` pressed: `texture.reload` on the gem and on every tile, the missing one included (its second request
    // fails again). Negative control: before the key, every texture has requested exactly one revision.
    let run = demo_driver("").run(|drive| {
        for _ in 0..6 {
            drive.post_update();
        }
        drive.wait_until("every admitted job to finish", |drive| {
            drive.service.stats().jobs.reserved == 0
        });
        drive.post_update();
        let before = drive.latest();
        assert!(
            before.values().all(|reading| reading.requested == 1),
            "{before:?}"
        );
        drive.post_update_with(&["reload_texture"]);
        let after = drive.latest();
        assert_eq!(after.len(), 13);
        assert!(
            after.values().all(|reading| reading.requested == 2),
            "every texture requested its second revision: {after:?}"
        );
        drive.wait_until("the reloads to finish", |drive| {
            drive.service.stats().jobs.reserved == 0
        });
        drive.post_update();
        let settled = drive.latest();
        assert_eq!(settled[MISSING].state, "failed", "{:?}", settled[MISSING]);
        assert_eq!(
            settled
                .values()
                .filter(|reading| reading.state == "cpu_ready")
                .count(),
            12
        );
        // A later update with no key requests nothing more.
        assert!(settled.values().all(|reading| reading.requested == 2));
    });
    assert_eq!(run.status.code, 0);
    assert_reconciled(&run.service);
}

/// Releases `gate` once the run's admission has closed, from a thread the case joins afterwards.
fn release_after_admission_closed(
    service: ck_resources::ResourceService,
    gate: Arc<JobGate>,
    slot: &Arc<Mutex<Option<thread::JoinHandle<()>>>>,
) {
    let releaser = thread::spawn(move || {
        let started = std::time::Instant::now();
        while !service.is_closed() {
            assert!(
                started.elapsed() < std::time::Duration::from_secs(10),
                "admission never closed"
            );
            thread::yield_now();
        }
        gate.release_all();
    });
    *slot.lock().unwrap_or_else(PoisonError::into_inner) = Some(releaser);
}

#[test]
fn an_unfinished_job_released_after_admission_closed_ends_discard_only_and_the_run_reconciles() {
    let gate = Arc::new(JobGate::new());
    gate.hold(HELD);
    let releaser = Arc::new(Mutex::new(None));
    let (drive_gate, drive_releaser) = (Arc::clone(&gate), Arc::clone(&releaser));
    let run = demo_driver("").run(move |drive| {
        drive.service.set_job_gate(Arc::clone(&drive_gate));
        for _ in 0..6 {
            drive.post_update();
        }
        drive.wait_until(
            "the held job to wait in the gate and the others to finish",
            |drive| drive_gate.is_waiting(HELD) && drive.service.stats().jobs.reserved == 1,
        );
        drive.post_update();
        let held = drive.latest()[HELD].state.clone();
        assert!(
            !matches!(held.as_str(), "cpu_ready" | "ready"),
            "the held tile is not shown: {held}"
        );
        // The run ends now: admission closes, then (inside the same shutdown budget) the gate is released.
        release_after_admission_closed(
            drive.service.clone(),
            Arc::clone(&drive_gate),
            &drive_releaser,
        );
    });
    releaser
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .expect("the releaser was started")
        .join()
        .expect("the releaser ended");
    // The end is finite and complete.
    assert_eq!(run.status.code, 0);
    assert!(!run.status.owners_incomplete);
    let latest = run.latest();
    assert!(
        !matches!(latest[HELD].state.as_str(), "cpu_ready" | "ready"),
        "the held job's late result was discarded: {:?}",
        latest[HELD]
    );
    assert_reconciled(&run.service);
}

#[test]
fn a_job_held_past_the_pool_deadline_makes_the_run_incomplete_and_the_gate_is_released_afterwards()
{
    // The variant: the gate stays held past the pool's shutdown deadline, so the loader is unfinished and the run takes
    // the incomplete status. Zero referenced is not asserted here.
    let gate = Arc::new(JobGate::new());
    gate.hold(HELD);
    let drive_gate = Arc::clone(&gate);
    let run = demo_driver("[runtime]\nstop_timeout = 0.5\n").run(move |drive| {
        drive.service.set_job_gate(Arc::clone(&drive_gate));
        for _ in 0..6 {
            drive.post_update();
        }
        drive.wait_until("the held job to wait in the gate", |_| {
            drive_gate.is_waiting(HELD)
        });
    });
    gate.release_all();
    assert_eq!(run.status.code, 1, "the stop phase failed");
    assert!(
        run.status.owners_incomplete,
        "the unfinished loader makes the shutdown incomplete"
    );
}

#[test]
fn a_loaded_texture_settles_cpu_ready_a_missing_one_failed_and_a_released_one_leaves_the_registry()
{
    let assets = Dir::new();
    std::fs::write(assets.0.join("good.png"), png(2, 2, &[7; 16])).expect("a good image");
    let script = "\
local good, missing\n\
local updates = 0\n\
function startup()\n\
  good = texture.load('good.png')\n\
  missing = texture.load('gone.png')\n\
  frame.on_update(function(f)\n\
    updates = updates + 1\n\
    if updates == 3 then texture.release(good) end\n\
  end)\n\
end\n";
    let run = HeadlessDriver {
        assets: assets.0.clone(),
        script: script.to_owned(),
        config: String::new(),
    }
    .run(|drive| {
        drive.post_update();
        drive.wait_until("both jobs to finish", |drive| {
            drive.service.stats().jobs.reserved == 0
        });
        drive.post_update();
        let latest = drive.latest();
        assert_eq!(latest["good.png"].state, "cpu_ready");
        assert_eq!(latest["gone.png"].state, "failed");
        assert!(latest["gone.png"].error.contains("gone.png"), "{latest:?}");
        assert_eq!(drive.service.stats().registry.live, 2);
        // The third update releases the good texture: its entry leaves the registry's live set.
        drive.post_update();
        drive.wait_until("the release to retire the entry", |drive| {
            drive.service.stats().registry.live == 1
        });
    });
    assert_eq!(run.status.code, 0);
    assert_reconciled(&run.service);
}

#[test]
#[should_panic(expected = "the frame_update callback returned")]
fn a_frame_callback_that_raises_fails_the_driver_instead_of_passing_silently() {
    // The driver's own negative control: it checks every answer, so a script whose update callback raises cannot pass.
    let assets = Dir::new();
    let _ = HeadlessDriver {
        assets: assets.0.clone(),
        script: "function startup() frame.on_update(function() error('boom') end) end\n".to_owned(),
        config: String::new(),
    }
    .run(crate::testing::Drive::post_update);
}
