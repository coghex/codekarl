//! The `render` API and `[render]` settings, end to end through the built binary without a window: the shipped
//! `config/demo-quad.toml` run headless (its image is decoded and validated, then ignored with one record), the
//! counts of ignored calls, a script whose `render.quad` fails, and the configuration errors of the `[render]`
//! table. Windowed behaviour, and the same scripts over fake backends, are in the executable's own tests.
//!
//! Every child has a whole-process deadline and is killed and reaped when it expires (the helpers are in
//! `common`).

use std::{
    ffi::OsStr,
    path::{Path, PathBuf},
};

#[expect(
    dead_code,
    reason = "this file uses the child runner and the scratch directory, not the full-pipe helper"
)]
mod common;

use common::{CHILD_DEADLINE, Run, Scratch, run_in};

fn run(args: &[&OsStr]) -> Run {
    let run = run_in(None, args, None, CHILD_DEADLINE);
    assert!(!run.timed_out, "the child outlived its deadline: {run:?}");
    run
}

fn config(text: &str) -> (PathBuf, Scratch) {
    let scratch = Scratch::new();
    let path = scratch.file("config.toml", text);
    (path, scratch)
}

/// How many times `needle` occurs.
fn count(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

/// The repository's own `assets` directory, absolute.
fn shipped_assets() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../assets")
        .canonicalize()
        .expect("the shipped assets directory")
}

#[test]
fn the_shipped_demo_runs_headless_with_one_record_per_ignored_api_and_no_error() {
    let demo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/demo-quad.toml");
    let result = run(&[
        OsStr::new("--config"),
        demo.as_os_str(),
        OsStr::new("--headless"),
    ]);
    assert_eq!(result.code, Some(0), "{result:?}");
    assert_eq!(result.stderr, "");
    assert_eq!(
        count(&result.stdout, "window: window.open ignored: headless run"),
        1,
        "{}",
        result.stdout
    );
    assert_eq!(
        count(&result.stdout, "render: scene calls ignored: headless run"),
        1,
        "{}",
        result.stdout
    );
    assert!(!result.stdout.contains("ERROR"), "{}", result.stdout);
    // No renderer exists in a headless run, so no Vulkan warning can appear.
    assert_eq!(count(&result.stdout, "textured quads on"), 0);
}

#[test]
fn a_headless_run_counts_the_accepted_render_calls_with_the_window_calls_in_one_debug_record() {
    let scratch = Scratch::new();
    let ui = scratch.file(
        "ui.lua",
        "window.open{title='t', width=200, height=100, clear={0, 0, 0}}\n\
         render.quad{image='demo-quad.png'}\n\
         render.quad{image='demo-quad.png', x=5, y=5}\n\
         render.clear_quad()\n\
         render.clear_quad()\n\
         window.set_clear(1, 1, 1)\n",
    );
    let text = format!(
        "[log]\nlevel = \"debug\"\ntimestamps = false\ncolour = \"never\"\n\n\
         [render]\nassets = '{}'\n\n[lua.ui]\nentry = '{}'\n",
        shipped_assets().display(),
        ui.display()
    );
    let path = scratch.file("config.toml", &text);
    let result = run(&[OsStr::new("--config"), path.as_os_str()]);
    assert_eq!(result.code, Some(0), "{result:?}");
    assert_eq!(
        count(&result.stdout, "render: scene calls ignored: headless run"),
        1,
        "{}",
        result.stdout
    );
    assert_eq!(
        count(
            &result.stdout,
            "window: 1 further window request(s) and 3 further render request(s) ignored: headless run"
        ),
        1,
        "{}",
        result.stdout
    );
}

/// The shipped live-scene demonstration run headless with `debug` resource records: the notice once, the script's
/// start-up statuses inside the vocabulary a headless run can reach, no update ever called, the ended calls raising the
/// service's `Ended` error, and the final record reconciled to zero. (The production headless run posts no update, so the
/// incremental tiles are exercised by the library's headless driver and the real-window cases.)
#[test]
fn the_shipped_scene_demo_runs_headless_finite_and_reconciles_its_resources() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../");
    let scratch = Scratch::new();
    let text = format!(
        "[log]\ntimestamps = false\ncolour = \"never\"\n\n[log.categories]\nresources = \"debug\"\n\n\
         [render]\nassets = '{}'\n\n[lua.ui]\nentry = '{}'\n\n[lua.gameplay]\nentry = '{}'\n",
        root.join("assets")
            .canonicalize()
            .expect("assets")
            .display(),
        root.join("scripts/ui/demo_scene.lua")
            .canonicalize()
            .expect("the demo script")
            .display(),
        root.join("scripts/gameplay/main.lua")
            .canonicalize()
            .expect("the gameplay script")
            .display(),
    );
    let path = scratch.file("config.toml", &text);
    let result = run(&[
        OsStr::new("--config"),
        path.as_os_str(),
        OsStr::new("--headless"),
    ]);
    assert_eq!(result.code, Some(0), "{result:?}");
    assert_eq!(result.stderr, "");
    let out = &result.stdout;
    assert!(!out.contains("ERROR"), "{out}");
    assert_eq!(
        count(out, "render: scene calls ignored: headless run"),
        1,
        "{out}"
    );
    assert_eq!(
        count(out, "window: window.open ignored: headless run"),
        1,
        "{out}"
    );
    // The read point is the end of `startup()`: a headless run reaches only these states (never `uploading` or `ready`).
    let line = out
        .lines()
        .find(|line| line.contains("startup status demo-quad.png "))
        .unwrap_or_else(|| panic!("no startup status in {out}"));
    let state = line.rsplit(' ').next().expect("a state");
    assert!(
        ["pending", "decoding", "cpu_ready", "failed"].contains(&state),
        "{line}"
    );
    // `frame.on_update` was registered and never called.
    assert_eq!(count(out, "lua.ui: updates 0"), 1, "{out}");
    // The ended calls raise the service's `Ended` error, separately.
    assert_eq!(
        count(
            out,
            "ended status raised=true runtime error: texture.status: the resource service has ended"
        ),
        1,
        "{out}"
    );
    assert_eq!(
        count(
            out,
            "ended load raised=true runtime error: texture.load: the resource service has ended"
        ),
        1,
        "{out}"
    );
    // The record reconciles: nothing live, nothing referenced, complete.
    let record = out
        .lines()
        .find(|line| line.contains("resources: final resources: "))
        .unwrap_or_else(|| panic!("no final record in {out}"));
    assert!(
        record.contains(" 0 live,") && record.contains(" 0 referenced at finish_teardown"),
        "{record}"
    );
    assert!(record.contains("gpu: no presenter"), "{record}");
    assert!(record.ends_with("complete"), "{record}");
    assert_eq!(count(out, "resources: final resources: "), 1, "{out}");
}

#[test]
fn a_headless_script_drives_the_texture_quad_and_frame_api_end_to_end_with_one_notice() {
    let scratch = Scratch::new();
    let ui = scratch.file(
        "ui.lua",
        "local t = texture.load('demo-quad.png')\n\
         assert(type(t) == 'string' and #t <= 64, 'a token string')\n\
         local before = texture.status(t)\n\
         assert(before.requested == 1 and before.shown == nil, 'one request, nothing shown')\n\
         local q = render.create_quad{texture = t, x = 1, y = 2, width = 3, height = 4}\n\
         render.set_position(q, 5, 6)\n\
         render.set_rect(q, 7, 8, 9, 10)\n\
         frame.on_update(function() end)\n\
         frame.on_update(nil)\n\
         local ok, err = pcall(render.set_position, q, 0/0, 1)\n\
         assert(not ok and tostring(err):find('render.set_position', 1, true), tostring(err))\n\
         render.destroy_quad(q)\n\
         texture.release(t)\n\
         local stale, why = pcall(texture.status, t)\n\
         assert(not stale and tostring(why):find('texture.status', 1, true), tostring(why))\n\
         log.info('the api ran')\n",
    );
    let text = format!(
        "[log]\ntimestamps = false\ncolour = \"never\"\n\n[render]\nassets = '{}'\n\n[lua.ui]\nentry = '{}'\n",
        shipped_assets().display(),
        ui.display()
    );
    let path = scratch.file("config.toml", &text);
    let result = run(&[
        OsStr::new("--config"),
        path.as_os_str(),
        OsStr::new("--headless"),
    ]);
    assert_eq!(result.code, Some(0), "{result:?}");
    assert!(result.stdout.contains("the api ran"), "{}", result.stdout);
    assert_eq!(
        count(&result.stdout, "render: scene calls ignored: headless run"),
        1,
        "{}",
        result.stdout
    );
    assert!(!result.stdout.contains("ERROR"), "{}", result.stdout);
    assert_eq!(result.stderr, "");
}

#[test]
fn a_failing_render_call_fails_the_ui_vm_naming_the_reason_in_headless_runs_too() {
    // The assets directory does not exist: it is named, on the first call.
    let (extra, expected) = (
        "[render]\nassets = '/nonexistent-codekarl-assets'\n",
        "/nonexistent-codekarl-assets",
    );
    let scratch = Scratch::new();
    let ui = scratch.file("ui.lua", "render.quad{image='demo-quad.png'}\n");
    let text = format!(
        "[log]\ntimestamps = false\ncolour = \"never\"\n\n{extra}\n[lua.ui]\nentry = '{}'\n",
        ui.display()
    );
    let path = scratch.file("config.toml", &text);
    let result = run(&[OsStr::new("--config"), path.as_os_str()]);
    assert_eq!(result.code, Some(1), "{result:?}");
    assert!(result.stdout.contains("render.quad"), "{}", result.stdout);
    assert!(result.stdout.contains(expected), "{}", result.stdout);
    assert_eq!(result.stderr, "");
}

#[test]
fn a_script_that_catches_a_failed_render_call_goes_on_and_the_run_is_clean() {
    let scratch = Scratch::new();
    let ui = scratch.file(
        "ui.lua",
        "local ok, err = pcall(render.quad, {image = '../escape.png'})\n\
         assert(not ok and string.find(tostring(err), 'escape.png', 1, true), tostring(err))\n\
         local ok2, err2 = pcall(render.quad, {image = 'demo-quad.png', width = 1/0})\n\
         assert(not ok2 and string.find(tostring(err2), 'finite', 1, true), tostring(err2))\n\
         log.info('caught both')\n",
    );
    let text = format!(
        "[log]\ntimestamps = false\ncolour = \"never\"\n\n[render]\nassets = '{}'\n\n[lua.ui]\nentry = '{}'\n",
        shipped_assets().display(),
        ui.display()
    );
    let path = scratch.file("config.toml", &text);
    let result = run(&[OsStr::new("--config"), path.as_os_str()]);
    assert_eq!(result.code, Some(0), "{result:?}");
    assert!(result.stdout.contains("caught both"), "{}", result.stdout);
    assert_eq!(
        count(&result.stdout, "scene calls ignored"),
        0,
        "nothing was accepted"
    );
}

#[test]
fn render_configuration_errors_are_one_config_error_with_the_path_and_the_line() {
    for (text, needle) in [
        ("[render]\nfilter = \"bilinear\"\n", "unknown filter"),
        ("[render]\nfilter = 3\n", "integer `3`"),
        ("[render]\nassets = true\n", "boolean"),
        ("[render]\nfiltre = \"linear\"\n", "filtre"),
        ("render = 3\n", "`render`"),
    ] {
        let (path, _scratch) = config(text);
        let result = run(&[OsStr::new("--config"), path.as_os_str()]);
        assert_eq!(result.code, Some(1), "{text:?}: {result:?}");
        assert_eq!(result.stderr, "");
        assert!(
            result
                .stdout
                .starts_with(&format!("ERROR config: {}: line ", path.display())),
            "{text:?}: {}",
            result.stdout
        );
        assert!(
            result.stdout.contains(needle),
            "{text:?}: {}",
            result.stdout
        );
        assert_eq!(result.stdout.lines().count(), 1, "{}", result.stdout);
    }
}

#[test]
fn a_valid_render_table_is_accepted_and_changes_nothing_for_a_run_that_never_renders() {
    for text in [
        "[render]\n",
        "[render]\nfilter = \"linear\"\n",
        "[render]\nfilter = \"nearest\"\nassets = \"no/such/dir\"\n",
    ] {
        let (path, _scratch) = config(&format!(
            "[log]\ntimestamps = false\ncolour = \"never\"\n{text}"
        ));
        let result = run(&[OsStr::new("--config"), path.as_os_str()]);
        assert_eq!(
            result.code,
            Some(0),
            "{text:?}: {result:?} (the assets directory is resolved at the first render.quad, not before)"
        );
    }
}
