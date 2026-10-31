//! Presenter and texture-ledger fault hooks are exposed only through hidden test constructors and
//! reader methods. Nothing reachable from the `codekarl` binary names those entry points. There is
//! no Cargo feature, environment variable or flag that enables a hook (the full check builds the
//! workspace with `--all-features`); this test scans application sources for hook and test-reader
//! identifiers.

use std::fs;
use std::path::{Path, PathBuf};

fn rust_files(directory: &Path, found: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(directory).expect("a readable directory") {
        let path = entry.expect("a directory entry").path();
        if path.is_dir() {
            rust_files(&path, found);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            found.push(path);
        }
    }
}

#[test]
fn codekarls_sources_never_name_the_hidden_constructor_or_the_hooks() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../apps/codekarl");
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    rust_files(&root.join("tests"), &mut files);
    assert!(
        files.len() >= 5,
        "the scan found the application's sources: {files:?}"
    );
    for file in files {
        let text = fs::read_to_string(&file).expect("a readable source file");
        // The constructor, the hooks type, and the hooks the textured quad added (CK-91): each reaches the
        // presenter only through `TestHooks`, so none may appear in the application.
        for forbidden in [
            "start_with_test_hooks",
            "TestHooks",
            "fail_pipeline",
            "fail_texture_allocation_frames",
            "fail_quad_encode_frames",
            "retired_token_delay",
            "pause_after_reap_for_token",
            "forced_panic_payload",
            "nil_staging",
            "nil_command",
            "nil_encoder",
            "fail_commit",
            "error_completion",
            "completion_delay",
            "panic_worker",
            "block_worker",
            "panic_wake_ready",
            "panic_wake_completed",
            "panic_wake_room",
            "force_measured_size",
            "fail_texture_allocation_at",
            "force_estimate_at",
            "panic_worker_after_batch",
            "panic_worker_after_texture",
            "panic_worker_payload_drop_panics",
            "block_after_texture",
            "block_after_texture_at",
            "staging_reservation",
            "thread_observer",
            "TestThreadObservation",
            "FrameCommandBuffer",
            "test_reader",
            // The scene presenter's hooks (CK-114): its fault switches, its evidence logs and the carrier of the
            // store's hooks reach it only through `TestHooks`.
            "SceneSetup",
            "TicketEnding",
            "ticket_log",
            "release_log",
            "abandon_after_prepare",
            "skipped_frames",
            "skip_next_drawable",
            "device_identity",
            "queue_identity",
            "completion_arrived_unpolled",
            "completion_gate",
            "submit_pause",
            "envelope_ready_unsubmitted",
            // The instanced scene's hooks (CK-116): the checker upload's own fault switches (the store's hook type
            // carried by `SceneSetup`), and the off-screen capture of what a frame drew.
            "checker_hooks",
            "force_bindless_tier1",
            "fail_residency_creation",
            "residency_change_budget",
            "capture_frames",
            "CapturedFrame",
            "captured_frame",
            "readback_scene",
            // Bindless drawing (CK-128): the off-screen path chooser and the two presenter switches (a small per-draw limit and a
            // forced out-of-range slot); the ordinary per-texture path is reachable only through the hidden
            // `start_scene_ordinary_baseline` and a backend's `ordinary_baseline` hook, which the application never names.
            "start_scene_ordinary_baseline",
            "ordinary_baseline",
            "ReadbackPath",
            "readback_scene_counted",
            "draw_chunk_limit",
            "force_out_of_range_slot_at",
            "test_bind_checker",
            "test_bindless_entries",
            "test_table_views",
            "test_bindless_probe",
            "test_open_completion_gate",
            "test_native_slot",
            "test_residency_commit_count",
            "test_raw_handle",
            "test_table_resource_id",
            // Frame timing (CK-129): the observer of a frame's marks, the replacement of a timing result, the blocked
            // acquisition inside the step and the delayed adoption of a measurement window.
            "frame_marks",
            "timing_override",
            "hold_in_acquire",
            "hold_metrics_adoption",
            "hold_before_record",
            "hold_presentations",
            "hold_after_commit",
            "hold_before_drain",
            "MarksProbe",
            "TimingProbe",
        ] {
            assert!(
                !text.contains(forbidden),
                "{} names `{forbidden}`: a hook must not be reachable from the application",
                file.display()
            );
        }
    }
}

#[test]
fn ledger_and_presenter_never_wait_for_native_completion() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    for file in [
        root.join("src/ledger.rs"),
        root.join("src/bindless.rs"),
        root.join("src/textures.rs"),
        root.join("src/presenter.rs"),
        root.join("src/scene.rs"),
        root.join("src/frame.rs"),
        root.join("src/batch.rs"),
    ] {
        let text = fs::read_to_string(&file).expect("a readable implementation source");
        for forbidden in ["waitUntilCompleted", "waitUntilScheduled"] {
            assert!(
                !text.contains(forbidden),
                "{} names `{forbidden}` outside the test reader: owner operations must poll",
                file.display()
            );
        }
    }
    let file = root.join("native/textures.m");
    let source = fs::read_to_string(&file).expect("readable native texture source");
    let readback_start = source
        .find("int ck_mc_texture_read(")
        .expect("the test-only synchronous readback helper");
    let readback_end = source[readback_start..]
        .find("int ck_mc_measure_shared(")
        .map(|offset| readback_start + offset)
        .expect("the next production native function");
    let mut production = source;
    production.replace_range(readback_start..readback_end, "");
    for forbidden in ["waitUntilCompleted", "waitUntilScheduled"] {
        assert!(
            !production.contains(forbidden),
            "{} names `{forbidden}` outside the test-only texture readback helper: owner operations must poll",
            file.display()
        );
    }
}

#[test]
fn the_presenter_never_waits_for_a_device_or_queue_idle_or_a_completion_outside_its_readbacks() {
    // The render thread never calls a device or queue idle wait or `waitUntilCompleted`; the two synchronous
    // off-screen readback functions (used by the pixel tests and nothing else) are the only places that wait.
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("native/presenter.m");
    let source = fs::read_to_string(&file).expect("readable native presenter source");
    let readbacks_start = source
        .find("int ck_pr_readback(")
        .expect("the first readback helper");
    let readbacks_end = source[readbacks_start..]
        .find("CKPrObserver *ck_pr_observer_new(")
        .map(|offset| readbacks_start + offset)
        .expect("the function after the readbacks");
    assert!(
        source[readbacks_start..readbacks_end].contains("waitUntilCompleted"),
        "the scan sees the readback helpers it excludes"
    );
    let mut production = source;
    production.replace_range(readbacks_start..readbacks_end, "");
    for forbidden in [
        "waitUntilCompleted",
        "waitUntilScheduled",
        "waitForIdle",
        "waitIdle",
        "dispatch_semaphore_wait",
    ] {
        assert!(
            !production.contains(forbidden),
            "{} names `{forbidden}` outside the readback helpers: the render thread polls",
            file.display()
        );
    }
}

#[test]
fn the_application_does_not_depend_on_a_window_crate_through_the_presenter() {
    // The presenter crate has no normal dependency on any window crate (design §3): the window crates are
    // dev-dependencies of its tests only.
    let manifest = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"))
        .expect("the manifest");
    let normal = manifest
        .split("[dev-dependencies]")
        .next()
        .expect("a manifest head");
    assert!(
        !normal.contains("ck-window"),
        "no normal dependency on a window crate:\n{normal}"
    );
}

#[test]
fn native_sources_use_no_volatile_access_and_share_no_gate() {
    // The blocked-acquisition gate is an `AtomicI32` that only Rust code touches (the render thread loads
    // it with an atomic load before the native frame call). No native source reads memory Rust writes
    // atomically, so none may contain a `volatile` access or the old gate pointer.
    let native = Path::new(env!("CARGO_MANIFEST_DIR")).join("native");
    let mut checked = 0;
    for entry in fs::read_dir(&native).expect("the native directory") {
        let path = entry.expect("a directory entry").path();
        if path
            .extension()
            .is_some_and(|extension| extension == "m" || extension == "h")
        {
            let text = fs::read_to_string(&path).expect("a readable native source");
            for forbidden in ["volatile", "hold_next_drawable", "hold_limit"] {
                assert!(
                    !text.contains(forbidden),
                    "{} names `{forbidden}`: a native read of a Rust atomic is not a valid protocol",
                    path.display()
                );
            }
            checked += 1;
        }
    }
    assert!(checked >= 4, "the scan found the native sources: {checked}");
}
