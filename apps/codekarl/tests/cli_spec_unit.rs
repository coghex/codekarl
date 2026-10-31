//! Unit tests for the CLI specification runner's logic: the notice filter, the full-spec switch,
//! the deadline (a fixture child that outlives a short timeout), and the verdicts.

use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::OsStringExt,
    path::Path,
    process::Command,
    time::Duration,
};

use serde_json::json;

#[path = "cli_spec/support.rs"]
mod support;

use support::{
    Actual, CASE_TIMEOUT, Case, CaseSet, Mode, SET_FLAG, Verdict, evaluate, filter_notices, in_set,
    is_validation_notice, load_cases, requested_set, run_command, spec_path, summarize,
};

fn case(args: &[&str], code: i32, stdout: &str, stderr: &str, requires: &[&str]) -> Case {
    Case {
        args: args.iter().map(|arg| (*arg).to_owned()).collect(),
        code,
        stdout: stdout.to_owned(),
        stderr: stderr.to_owned(),
        requires: requires
            .iter()
            .map(|feature| (*feature).to_owned())
            .collect(),
    }
}

fn process_exists(pid: u32) -> bool {
    Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .map(|output| !output.stdout.is_empty())
        .expect("ps can be run")
}

#[test]
fn each_case_has_thirty_seconds() {
    assert_eq!(CASE_TIMEOUT, Duration::from_secs(30));
}

#[test]
fn the_bare_and_prefixed_notices_are_recognized() {
    for line in [
        "Metal API Validation Enabled",
        "Metal GPU Validation Enabled",
        "2026-10-16 03:04:05.678 codekarl[1234:56789] Metal API Validation Enabled",
        "2026-10-16 03:04:05.678 codekarl[1:2] Metal GPU Validation Enabled",
    ] {
        assert!(is_validation_notice(line), "{line}");
    }
}

#[test]
fn near_misses_are_kept() {
    for line in [
        "Metal API Validation Enabled!",
        "metal api validation enabled",
        " Metal API Validation Enabled",
        "Metal API Validation Disabled",
        "2026-10-16 03:04:05 codekarl[1234:56789] Metal API Validation Enabled",
        "2026-10-16 03:04:05.678 codekarl[1234] Metal API Validation Enabled",
        "2026-10-16 03:04:05.678 codekarl[x:5] Metal API Validation Enabled",
        "2026-10-16 03:04:05.678 [1234:5] Metal API Validation Enabled",
        "2026-10-16 03:04:05.678 otherprog[123:456] Metal API Validation Enabled",
        "2026-10-16 03:04:05.678 codekarl2[1:2] Metal API Validation Enabled",
        "note: Metal API Validation Enabled",
        "codekarl: unknown arguments; use --help",
        "",
    ] {
        assert!(!is_validation_notice(line), "{line:?}");
    }
}

#[test]
fn filtering_drops_only_notices_and_keeps_everything_else_byte_for_byte() {
    let stderr = "Metal API Validation Enabled\nreal line\n\
        2026-10-16 03:04:05.678 codekarl[1:2] Metal GPU Validation Enabled\n\
        Metal API Validation Enabled!\n";
    assert_eq!(
        filter_notices(stderr),
        "real line\nMetal API Validation Enabled!\n"
    );
    assert_eq!(filter_notices("Metal API Validation Enabled\n"), "");
    assert_eq!(filter_notices(""), "");
    // No terminator is added, none is converted, and a different process name is not a notice.
    assert_eq!(filter_notices("error"), "error");
    assert_eq!(filter_notices("error\r\n"), "error\r\n");
    assert_eq!(filter_notices("one\r\ntwo"), "one\r\ntwo");
    let other = "2026-10-16 04:00:00.123 otherprog[123:456] Metal API Validation Enabled\n";
    assert_eq!(filter_notices(other), other);
    assert_eq!(
        filter_notices("Metal API Validation Enabled\r\n"),
        "Metal API Validation Enabled\r\n"
    );
    // An unterminated final notice is still exactly a notice.
    assert_eq!(
        filter_notices("keep\nMetal GPU Validation Enabled"),
        "keep\n"
    );
}

/// Runs a fixture child that prints `text` to stdout (`printf '%s'`), exit code 0 and empty stderr,
/// against a case expecting `expected` on stdout.
fn stdout_verdict(text: &str, expected: &str) -> Verdict {
    evaluate(
        &case(
            &["-c", "printf '%s' \"$1\"", "sh", text],
            0,
            expected,
            "",
            &[],
        ),
        &[],
        Path::new("/bin/sh"),
        Duration::from_secs(10),
    )
}

const BARE_API: &str = "Metal API Validation Enabled";
const BARE_GPU: &str = "Metal GPU Validation Enabled";
const PREFIX: &str = "2026-10-16 03:04:05.678 codekarl[1234:56789] ";

#[test]
fn bare_notices_are_removed_from_stdout() {
    let text = format!("{BARE_API}\n{BARE_GPU}\nreal output\n");
    assert_eq!(stdout_verdict(&text, "real output\n"), Verdict::Passed);
}

#[test]
fn prefixed_notices_are_removed_from_stdout() {
    let text = format!("{PREFIX}{BARE_API}\nreal output\n{PREFIX}{BARE_GPU}\n");
    assert_eq!(stdout_verdict(&text, "real output\n"), Verdict::Passed);
}

#[test]
fn malformed_prefixes_are_kept_on_stdout() {
    for prefix in [
        // The wrong number of digits.
        "2026-10-16 03:04:05 codekarl[1234:56789] ",
        "2026-10-16 03:04:05.6789 codekarl[1234:56789] ",
        "26-10-16 03:04:05.678 codekarl[1234:56789] ",
        // A missing `codekarl[`.
        "2026-10-16 03:04:05.678 [1234:56789] ",
        "2026-10-16 03:04:05.678 codekarl 1234:56789] ",
        // A non-numeric pid or tid.
        "2026-10-16 03:04:05.678 codekarl[x:56789] ",
        "2026-10-16 03:04:05.678 codekarl[1234:y] ",
        // A missing space.
        "2026-10-16 03:04:05.678codekarl[1234:56789] ",
        "2026-10-16 03:04:05.678 codekarl[1234:56789]",
    ] {
        for notice in [BARE_API, BARE_GPU] {
            let line = format!("{prefix}{notice}\n");
            assert_eq!(stdout_verdict(&line, &line), Verdict::Passed, "{line:?}");
            // And a run that expected it gone fails: the line is real output.
            assert!(
                matches!(stdout_verdict(&line, ""), Verdict::Failed(_)),
                "{line:?}"
            );
        }
    }
}

#[test]
fn a_notice_with_extra_text_or_embedded_in_other_text_is_kept_on_stdout() {
    for line in [
        format!("{BARE_API} and more\n"),
        format!("{BARE_API}!\n"),
        format!("before {BARE_GPU}\n"),
        format!("before {BARE_API} after\n"),
        format!("{PREFIX}{BARE_API} trailing\n"),
        format!("note: {PREFIX}{BARE_GPU}\n"),
        format!("{BARE_API}{BARE_GPU}\n"),
    ] {
        assert_eq!(stdout_verdict(&line, &line), Verdict::Passed, "{line:?}");
    }
}

#[test]
fn lines_around_stdout_notices_keep_their_order() {
    let text = format!("first\n{BARE_API}\nsecond\n{PREFIX}{BARE_GPU}\nthird\n");
    assert_eq!(
        stdout_verdict(&text, "first\nsecond\nthird\n"),
        Verdict::Passed
    );
}

#[test]
fn an_unterminated_final_stdout_notice_is_removed_and_another_unterminated_line_is_kept() {
    let notice = format!("keep\n{PREFIX}{BARE_GPU}");
    assert_eq!(stdout_verdict(&notice, "keep\n"), Verdict::Passed);
    assert_eq!(stdout_verdict("keep\nlast", "keep\nlast"), Verdict::Passed);
    // The terminator is not added: a missing newline on the last real line is a mismatch.
    assert!(matches!(
        stdout_verdict("keep\nlast", "keep\nlast\n"),
        Verdict::Failed(_)
    ));
}

#[test]
fn a_foreign_notice_like_line_on_stdout_is_real_output() {
    let line = "2026-10-16 04:00:00.123 otherprog[123:456] Metal API Validation Enabled\n";
    assert_eq!(stdout_verdict(line, line), Verdict::Passed);
    assert!(matches!(stdout_verdict(line, ""), Verdict::Failed(_)));
}

#[test]
fn a_stderr_that_differs_only_by_a_missing_newline_fails_the_case() {
    // The fixture child prints `error` without a newline; the specification expects `error\n`.
    let child = ["-c", "printf error >&2"];
    let mismatch = evaluate(
        &case(&child, 0, "", "error\n", &[]),
        &[],
        Path::new("/bin/sh"),
        Duration::from_secs(10),
    );
    assert!(matches!(mismatch, Verdict::Failed(_)), "{mismatch:?}");
    let exact = evaluate(
        &case(&child, 0, "", "error", &[]),
        &[],
        Path::new("/bin/sh"),
        Duration::from_secs(10),
    );
    assert_eq!(exact, Verdict::Passed);
    // A line from another process that looks like a notice is real output, so the case fails.
    let other =
        "printf '2026-10-16 04:00:00.123 otherprog[123:456] Metal API Validation Enabled\\n' >&2";
    let foreign = evaluate(
        &case(&["-c", other], 0, "", "", &[]),
        &[],
        Path::new("/bin/sh"),
        Duration::from_secs(10),
    );
    assert!(matches!(foreign, Verdict::Failed(_)), "{foreign:?}");
}

#[test]
fn the_full_spec_variable_is_strict() {
    assert_eq!(Mode::from_os(None), Ok(Mode::Normal));
    assert_eq!(Mode::from_os(Some(OsStr::new("1"))), Ok(Mode::Full));
    let undecodable = OsString::from_vec(vec![0x66, 0xff]);
    let error =
        Mode::from_os(Some(&undecodable)).expect_err("a value that is not Unicode is refused");
    assert!(
        error.contains("CK_REQUIRE_FULL_SPEC") && error.contains("Unicode"),
        "{error}"
    );
    assert_eq!(Mode::from_value(None), Ok(Mode::Normal));
    assert_eq!(Mode::from_value(Some("0")), Ok(Mode::Normal));
    assert_eq!(Mode::from_value(Some("1")), Ok(Mode::Full));
    for other in ["yes", "true", "", "2", "01", " 1"] {
        let error = Mode::from_value(Some(other)).expect_err("a bad value is refused");
        assert!(error.contains("CK_REQUIRE_FULL_SPEC"), "{error}");
    }
}

#[test]
fn a_case_is_read_from_json_and_bad_cases_are_named() {
    let read = Case::from_json(&json!({
        "args": ["--metal-smoke"], "code": 0, "stdout": "ok\n", "stderr": "", "requires": ["metal"]
    }))
    .expect("the fixture is well formed");
    assert_eq!(read.requires, ["metal"]);
    assert_eq!(read.name(), "--metal-smoke");
    let plain = Case::from_json(&json!({"args": [], "code": 1, "stdout": "", "stderr": ""}))
        .expect("the fixture is well formed");
    assert!(plain.requires.is_empty());
    assert_eq!(plain.name(), "<no arguments>");
    for bad in [
        json!({"args": [], "stdout": "", "stderr": ""}),
        json!({"args": [], "code": "0", "stdout": "", "stderr": ""}),
        json!({"args": [1], "code": 0, "stdout": "", "stderr": ""}),
        json!({"args": [], "code": 0, "stdout": 5, "stderr": ""}),
        json!({"args": [], "code": 0, "stdout": "", "stderr": "", "requires": "metal"}),
    ] {
        assert!(Case::from_json(&bad).is_err(), "{bad}");
    }
}

#[test]
fn a_child_that_outlives_the_deadline_is_killed_reaped_and_reported_among_other_results() {
    // The fixture child is `sleep 30` with a 200 ms deadline.
    let slow = run_command(
        Path::new("/bin/sleep"),
        &["30".to_owned()],
        Duration::from_millis(200),
    )
    .expect("the fixture is well formed");
    assert!(slow.timed_out);
    assert_eq!(slow.code, None);
    assert!(
        !process_exists(slow.pid),
        "the timed-out child was not reaped"
    );
    let verdict = evaluate(
        &case(&["30"], 0, "", "", &[]),
        &[],
        Path::new("/bin/sleep"),
        Duration::from_millis(200),
    );
    assert!(matches!(verdict, Verdict::TimedOut(_)), "{verdict:?}");
    let results = vec![
        (
            "fast".to_owned(),
            evaluate(
                &case(&[], 0, "", "", &[]),
                &[],
                Path::new("/usr/bin/true"),
                Duration::from_secs(10),
            ),
        ),
        ("slow".to_owned(), verdict),
        (
            "wrong".to_owned(),
            evaluate(
                &case(&[], 3, "", "", &[]),
                &[],
                Path::new("/usr/bin/true"),
                Duration::from_secs(10),
            ),
        ),
    ];
    let summary = summarize(&results, &[], Mode::Normal);
    assert!(!summary.passed);
    let text = summary.lines.join("\n");
    assert!(text.contains("test fast ... ok"), "{text}");
    assert!(text.contains("test slow ... TIMED OUT"), "{text}");
    assert!(text.contains("test wrong ... FAILED"), "{text}");
    assert!(text.contains("timed out: slow"), "{text}");
    assert!(text.contains("failed: wrong"), "{text}");
}

#[test]
fn a_missing_feature_skips_the_case_and_full_mode_fails_on_the_skip() {
    let skipped = evaluate(
        &case(&["--metal-smoke"], 0, "", "", &["metal"]),
        &[],
        Path::new("/usr/bin/true"),
        Duration::from_secs(10),
    );
    assert!(matches!(skipped, Verdict::Skipped(_)), "{skipped:?}");
    let ran = evaluate(
        &case(&[], 0, "", "", &["metal"]),
        &["metal"],
        Path::new("/usr/bin/true"),
        Duration::from_secs(10),
    );
    assert_eq!(ran, Verdict::Passed);
    let results = vec![("metal".to_owned(), skipped), ("ok".to_owned(), ran)];
    let normal = summarize(&results, &[], Mode::Normal);
    assert!(normal.passed);
    assert!(normal.lines.join("\n").contains("test metal ... skipped"));
    let full = summarize(&results, &[], Mode::Full);
    assert!(!full.passed);
    assert!(
        full.lines
            .join("\n")
            .contains("skipped in full-spec mode: metal")
    );
}

#[test]
fn an_empty_specification_fails_only_in_full_mode() {
    assert!(summarize(&[], &[], Mode::Normal).passed);
    let full = summarize(&[], &[], Mode::Full);
    assert!(!full.passed);
    assert!(full.lines.join("\n").contains("no cases ran"));
}

#[test]
fn runner_checks_never_count_as_cases() {
    // The reviewer's composition: no cases, one passing runner check, full-spec mode. The check is
    // reported, but the specification still ran no case, so the run fails.
    let checks = vec![("parser".to_owned(), Verdict::Passed)];
    let full = summarize(&[], &checks, Mode::Full);
    let text = full.lines.join("\n");
    assert!(!full.passed, "{text}");
    assert!(text.contains("no cases ran in full-spec mode"), "{text}");
    assert!(text.contains("test parser ... ok"), "{text}");
    assert!(
        text.contains("0 case(s): 0 passed, 0 skipped; 1 runner check(s): 1 passed"),
        "{text}"
    );
    assert!(summarize(&[], &checks, Mode::Normal).passed);
}

#[test]
fn a_failing_runner_check_fails_the_run_in_both_modes() {
    let cases = vec![("a".to_owned(), Verdict::Passed)];
    let checks = vec![("parser".to_owned(), Verdict::Failed("broken".to_owned()))];
    for mode in [Mode::Normal, Mode::Full] {
        let summary = summarize(&cases, &checks, mode);
        assert!(!summary.passed);
        assert!(
            summary.lines.join("\n").contains("failed: parser (broken)"),
            "{mode:?}"
        );
    }
}

#[test]
fn the_specification_path_is_fixed_when_the_tests_are_compiled() {
    // `spec_path` takes no input: it is the manifest directory this crate was compiled in, so no
    // variable or argument at run time can redirect the real specification.
    let path = spec_path();
    assert!(path.starts_with(env!("CARGO_MANIFEST_DIR")), "{path:?}");
    assert!(path.ends_with("specs/bootstrap.json"), "{path:?}");
    let cases = load_cases(&path).expect("the real specification parses");
    assert!(cases.len() >= 6, "{} case(s)", cases.len());
    assert!(cases.iter().any(|case| case.args == ["--version"]));
}

#[test]
fn every_failure_is_reported_together() {
    let results = vec![
        ("a".to_owned(), Verdict::Failed("first".to_owned())),
        ("b".to_owned(), Verdict::Passed),
        ("c".to_owned(), Verdict::Failed("second".to_owned())),
    ];
    let summary = summarize(&results, &[], Mode::Normal);
    let text = summary.lines.join("\n");
    assert!(!summary.passed);
    assert!(
        text.contains("failed: a (first)") && text.contains("failed: c (second)"),
        "{text}"
    );
    assert!(
        text.contains("3 case(s): 1 passed, 0 skipped; 0 runner check(s): 0 passed; 2 problem(s)"),
        "{text}"
    );
}

#[test]
fn the_validation_layers_are_in_the_childs_environment() {
    // `env` prints its environment; the runner always sets both layers.
    let actual: Actual = run_command(Path::new("/usr/bin/env"), &[], Duration::from_secs(10))
        .expect("the fixture is well formed");
    assert!(
        actual
            .stdout
            .lines()
            .any(|line| line == "MTL_DEBUG_LAYER=1"),
        "{}",
        actual.stdout
    );
    assert!(
        actual
            .stdout
            .lines()
            .any(|line| line == "MTL_SHADER_VALIDATION=1")
    );
}

// ---------------------------------------------------------------------------------------------
// Case sets (CK-77): the plain cases and the GPU case run in separate check groups
// ---------------------------------------------------------------------------------------------

fn words(arguments: &[&str]) -> Vec<String> {
    arguments.iter().map(|word| (*word).to_owned()).collect()
}

#[test]
fn no_set_means_the_whole_specification_and_one_name_means_that_set() {
    assert_eq!(requested_set(&words(&[])), Ok(None));
    // Cargo's own arguments (a filter, `--nocapture`) and the runner's switch select nothing.
    assert_eq!(
        requested_set(&words(&["--nocapture", "--no-self-checks", "filter"])),
        Ok(None)
    );
    assert_eq!(
        requested_set(&words(&[SET_FLAG, "plain"])),
        Ok(Some(CaseSet::Plain))
    );
    assert_eq!(
        requested_set(&words(&["--no-self-checks", SET_FLAG, "gpu"])),
        Ok(Some(CaseSet::Gpu))
    );
}

#[test]
fn an_unknown_empty_repeated_valueless_or_misspelled_set_is_an_error() {
    for bad in [
        &[SET_FLAG][..],
        &[SET_FLAG, ""],
        &[SET_FLAG, "all"],
        &[SET_FLAG, "Plain"],
        &[SET_FLAG, "plain", SET_FLAG, "gpu"],
        &[SET_FLAG, "gpu", SET_FLAG, "gpu"],
        &["--set=gpu"],
        &["--sets", "gpu"],
    ] {
        assert!(requested_set(&words(bad)).is_err(), "{bad:?}");
    }
}

#[test]
fn the_real_specification_splits_into_a_plain_set_and_a_gpu_set_that_together_are_all_of_it() {
    let all = load_cases(&spec_path()).expect("the specification");
    let plain = in_set(all.clone(), Some(CaseSet::Plain)).expect("the plain set");
    let gpu = in_set(all.clone(), Some(CaseSet::Gpu)).expect("the gpu set");
    assert_eq!(in_set(all.clone(), None).expect("all").len(), all.len());
    assert_eq!(
        plain.len() + gpu.len(),
        all.len(),
        "nothing dropped or added"
    );
    assert!(plain.iter().all(|case| case.requires.is_empty()));
    assert_eq!(
        gpu.iter().map(Case::name).collect::<Vec<_>>(),
        ["--metal-smoke"],
        "the GPU case is the Metal smoke and nothing else"
    );
    for case in &all {
        let places = [&plain, &gpu]
            .iter()
            .filter(|set| set.iter().any(|member| member.name() == case.name()))
            .count();
        assert_eq!(places, 1, "{} is in exactly one set", case.name());
    }
}

#[test]
fn a_set_with_no_case_is_an_error_never_a_run_of_nothing() {
    let only_plain = vec![case(&["--version"], 0, "x\n", "", &[])];
    let error = in_set(only_plain, Some(CaseSet::Gpu)).expect_err("no gpu case");
    assert!(error.contains("gpu"), "{error}");
    assert!(in_set(Vec::new(), Some(CaseSet::Plain)).is_err());
    assert!(
        in_set(Vec::new(), None)
            .expect("no set: unchanged")
            .is_empty()
    );
}
