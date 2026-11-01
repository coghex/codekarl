//! The test hooks reach the presenter only through `VulkanPresenter::start_with_test_hooks`, and nothing
//! reachable from the `codekarl` binary calls it. There is no Cargo feature, environment variable or flag
//! that enables a hook (the full check builds the workspace with `--all-features`, so a feature would reach
//! `codekarl` too); this test reads `codekarl`'s sources and fails if one names the hidden constructor or
//! the hooks type. It also checks the crate's dependency boundary (design §3) and that no source names a
//! dynamic-loader variable.

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

/// Test-only names distinctive enough to forbid anywhere in `codekarl`'s sources: the hidden constructors
/// and hook types, the log handle and the invalid-use helpers.
const DISTINCTIVE: [&str; 84] = [
    "start_with_test_hooks",
    "TestHooks",
    "readback_clear_with_test_hooks",
    "readback_quad_with_test_hooks",
    "ReadbackHooks",
    "create_instance_with_test_hooks",
    "InstanceTestHooks",
    "submit_test_warning",
    "leak_a_device_for_tests",
    "retained_readbacks",
    "LoaderWatch",
    "LedgerSnapshot",
    "SurfaceExtent",
    "take_for_test",
    "forced_panic_payload",
    "QuadFaults",
    "QuadStats",
    "QuadAllocation",
    "quad_faults",
    "quad_stats",
    "quad_slot",
    "forced_surface_formats",
    "quad_image_limit",
    "abandon_after_record",
    "FrameCapture",
    "FrameCaptures",
    "capture_frames",
    "PipelineFault",
    "new_for_tests",
    "TestFeatures",
    "lower_allocation_limit",
    "inject_submit_error",
    "submit_commands",
    "StallGate",
    "envelope_ready_unsubmitted",
    "test_wait_submitted",
    "lease_objects",
    "submitted_command_buffer",
    "held_images",
    "allocation_count",
    "readback_image",
    "test_submit_upload",
    "test_wait_upload",
    "test_revision_state",
    "test_live_revisions",
    "start_scene_with_test_hooks",
    "TicketEnding",
    "SceneSetup",
    "WaitKind",
    "WaitObserver",
    "BuildRecord",
    "DrawnRecord",
    "ledger_clock_ns",
    "SceneHooks",
    "readback_scene",
    "readback_scene_with_held",
    "HeldCompat",
    "CompatKind",
    "BindlessCaps",
    "BindlessDeviceLimits",
    "BindlessReport",
    "bindless_caps",
    "bindless_report",
    "LayoutScope",
    "submit_test_message",
    "bindless_layout_scope",
    "create_table_layout_for_tests",
    "BindingChoice",
    "with_binding_choice",
    "start_with_binding",
    "test_revision_slot",
    "test_table_entry",
    "test_table_wanted",
    "test_current_table",
    "test_table_pins",
    "test_slots_in_use",
    "test_table_version",
    "test_revision_pins",
    "timing_caps",
    "TimingCaps",
    "test_begin_frame_on",
    "SceneContent",
    "SceneReadback",
    "readback_slots",
];

/// Test-only names too common to forbid as words (the application has fakes of its own called `Gate`, say):
/// forbidden in every source of the application that names `ck_gpu_vulkan`.
const GENERIC: [&str; 4] = ["Observer", "Gate", "Pause", "Event"];

/// The names the library marks `#[doc(hidden)]` that are in neither list above, each with why: a method
/// of a forbidden type, or a name too common to forbid as a word.
const ALLOWED_HIDDEN: [(&str, &str); 11] = [
    (
        "watch",
        "a method of Loader, returning the forbidden LoaderWatch",
    ),
    ("holders", "a method of Instance, a count for the tests"),
    ("open", "a method of the forbidden Gate"),
    ("new", "the constructor of a forbidden type"),
    ("hold", "a method of the forbidden Pause"),
    ("release", "a method of the forbidden Pause"),
    ("reached", "a method of the forbidden Pause"),
    (
        "readback",
        "a method of PreparedTexture, which only the texture store's tests reach",
    ),
    (
        "device",
        "a method of VulkanContext for the tests' own commands, a common word",
    ),
    (
        "family",
        "a method of VulkanContext for the tests' own commands, a common word",
    ),
    (
        "memory_properties",
        "a method of VulkanContext for the tests' own buffers, a common word",
    ),
];

/// The names of the items after each `#[doc(hidden)]` in the library: `pub fn`, `struct`, `enum` or `type`.
fn hidden_items(source: &str) -> Vec<String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut found = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if line.trim() != "#[doc(hidden)]" {
            continue;
        }
        for next in &lines[index + 1..] {
            let text = next.trim();
            if text.starts_with("#[") || text.starts_with("///") || text.is_empty() {
                continue;
            }
            let words: Vec<&str> = text.split_whitespace().collect();
            if let Some(position) = words
                .iter()
                .position(|word| matches!(*word, "fn" | "struct" | "enum" | "type"))
                && let Some(name) = words.get(position + 1)
            {
                let name = name
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .next()
                    .unwrap_or_default();
                found.push(name.to_owned());
            }
            break;
        }
    }
    found
}

#[test]
fn every_hidden_item_of_the_library_is_either_forbidden_in_the_application_or_explained() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    let mut hidden = Vec::new();
    for file in &files {
        hidden.extend(hidden_items(
            &fs::read_to_string(file).expect("a source file"),
        ));
    }
    assert!(
        hidden.len() >= 15,
        "the scan found the hidden items: {hidden:?}"
    );
    for name in &hidden {
        assert!(
            DISTINCTIVE.contains(&name.as_str())
                || GENERIC.contains(&name.as_str())
                || ALLOWED_HIDDEN.iter().any(|(allowed, _)| allowed == name),
            "`{name}` is `doc(hidden)` in the library but neither forbidden in codekarl's sources nor \
             explained: add it to DISTINCTIVE, GENERIC or ALLOWED_HIDDEN in tests/hook_isolation.rs"
        );
    }
    for name in [
        "start_with_test_hooks",
        "readback_clear_with_test_hooks",
        "create_instance_with_test_hooks",
        "ReadbackHooks",
        "TestHooks",
    ] {
        assert!(
            hidden.iter().any(|seen| seen == name),
            "{name} is hidden: {hidden:?}"
        );
    }
}

#[test]
fn codekarls_sources_never_name_a_test_only_name() {
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
        for forbidden in DISTINCTIVE {
            assert!(
                !text.contains(forbidden),
                "{} names `{forbidden}`: a test-only name must not be reachable from the application",
                file.display()
            );
        }
        if text.contains("ck_gpu_vulkan") {
            for forbidden in GENERIC {
                assert!(
                    !text.contains(forbidden),
                    "{} uses ck_gpu_vulkan and names `{forbidden}`: a test-only name must not be reachable \
                     from the application",
                    file.display()
                );
            }
        }
    }
}

#[test]
fn the_library_has_no_normal_dependency_on_a_window_crate() {
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
    assert!(
        manifest.contains("[dev-dependencies]"),
        "the windowed tests' dependencies are dev-only"
    );
}

#[test]
fn no_source_names_a_dynamic_loader_variable_or_the_library_tree_holds_native_code() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    rust_files(&root.join("src"), &mut files);
    assert!(
        files.len() >= 8,
        "the scan found the library's sources: {files:?}"
    );
    for file in &files {
        let text = fs::read_to_string(file).expect("a readable source file");
        for forbidden in ["DYLD_", "LD_LIBRARY_PATH"] {
            let named = text
                .lines()
                .filter(|line| !line.trim_start().starts_with("//"))
                .any(|line| line.contains(forbidden));
            assert!(!named, "{} names `{forbidden}`", file.display());
        }
        assert!(
            !text.contains("target_os = \"macos\"") || file.ends_with("loader.rs"),
            "{} has macOS-only code (only the loader's file name differs by platform)",
            file.display()
        );
    }
    assert!(
        !root.join("native").exists(),
        "the library has no native code"
    );
}
