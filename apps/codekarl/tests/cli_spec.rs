//! Runs every case of `specs/bootstrap.json` against the `codekarl` binary Cargo built for this test.
//!
//! This target has no libtest harness (`harness = false` in `Cargo.toml`), so its per-case lines and
//! its skipped cases appear in plain `cargo test` output. All cases run, all problems are reported
//! together, and the run then fails once. With `CK_REQUIRE_FULL_SPEC=1` a skipped case, a timeout or
//! an empty specification fails it too.
//!
//! The specification is always `specs/bootstrap.json` at a path fixed when this target is compiled
//! (`support::spec_path`): no environment variable and no argument can point the run at another file.

use std::{
    ffi::OsString,
    fs,
    os::unix::ffi::OsStringExt,
    path::PathBuf,
    process::{Command, ExitCode},
};

#[path = "cli_spec/support.rs"]
mod support;

use support::{
    CASE_TIMEOUT, CaseSet, FULL_SPEC_VARIABLE, Mode, SET_FLAG, Verdict, evaluate, in_set,
    requested_set, summarize,
};

/// The features this test, and so the binary under test, was built with.
const FEATURES: &[&str] = match (cfg!(feature = "metal"), cfg!(feature = "vulkan")) {
    (false, false) => &[],
    (true, false) => &["metal"],
    (false, true) => &["vulkan"],
    (true, true) => &["metal", "vulkan"],
};

/// An argument the self-checks pass to the executables they start, so those do not run the
/// self-checks again. It skips only the runner's own checks; the specification's cases always run.
const NO_SELF_CHECKS: &str = "--no-self-checks";

/// Variables a caller might try in order to point the run at a different specification. None has any
/// effect: the self-check sets all of them to a one-case file and requires the real case count.
const REDIRECT_ATTEMPTS: [&str; 5] = [
    "CK_CLI_SPEC_FILE",
    "CK_SPEC_FILE",
    "CK_CLI_SPEC",
    "CK_BOOTSTRAP_SPEC",
    "CK_SPECS",
];

/// Starts this same executable in full-spec mode (restricted to the same `set`) with every redirect
/// variable pointing at a file that holds a single case: it must still run the real specification's cases
/// of that set, all of them.
fn redirect_variables_change_nothing(set: Option<CaseSet>) -> Verdict {
    let file = std::env::temp_dir().join(format!("ck-cli-spec-one-{}.json", std::process::id()));
    let verdict = (|| {
        let real = in_set(support::load_cases(&support::spec_path())?, set)?;
        let one = r#"{"cases": [{"args": ["--version"], "code": 0, "stdout": "codekarl 0.1.0\n", "stderr": ""}]}"#;
        fs::write(&file, one).map_err(|error| error.to_string())?;
        let mut command = Command::new(std::env::current_exe().map_err(|error| error.to_string())?);
        command.arg(NO_SELF_CHECKS).env(FULL_SPEC_VARIABLE, "1");
        if let Some(set) = set {
            command.args([SET_FLAG, set.name()]);
        }
        for variable in REDIRECT_ATTEMPTS {
            command.env(variable, &file);
        }
        let output = command.output().map_err(|error| error.to_string())?;
        let text = String::from_utf8_lossy(&output.stdout);
        let expected = format!("cli_spec: {} case(s) against", real.len());
        if !text.starts_with(&expected) {
            return Err(format!(
                "a redirect variable changed the specification; expected {expected:?}, got {text:?}"
            ));
        }
        Ok(())
    })();
    let _ = fs::remove_file(&file); // a leftover temporary file is harmless
    match verdict {
        Ok(()) => Verdict::Passed,
        Err(message) => Verdict::Failed(message),
    }
}

/// Starts this same executable with a `CK_REQUIRE_FULL_SPEC` value that is not valid Unicode: it must
/// stop at once, naming the variable, instead of silently running in normal mode.
fn undecodable_value_is_rejected() -> Verdict {
    let output = std::env::current_exe().and_then(|exe| {
        Command::new(exe)
            .env(
                FULL_SPEC_VARIABLE,
                OsString::from_vec(vec![0x66, 0xff, 0x6f]),
            )
            .output()
    });
    match output {
        Ok(output)
            if !output.status.success()
                && String::from_utf8_lossy(&output.stderr).contains(FULL_SPEC_VARIABLE)
                && output.stdout.is_empty() =>
        {
            Verdict::Passed
        }
        Ok(output) => Verdict::Failed(format!(
            "expected an immediate failure naming {FULL_SPEC_VARIABLE}; got {:?}, stdout {:?}, stderr {:?}",
            output.status.code(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )),
        Err(error) => Verdict::Failed(format!("could not run this executable: {error}")),
    }
}

fn main() -> ExitCode {
    let value = std::env::var_os(FULL_SPEC_VARIABLE);
    let mode = match Mode::from_os(value.as_deref()) {
        Ok(mode) => mode,
        Err(message) => {
            eprintln!("cli_spec: {message}");
            return ExitCode::FAILURE;
        }
    };
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let set = match requested_set(&arguments) {
        Ok(set) => set,
        Err(message) => {
            eprintln!("cli_spec: {message}");
            return ExitCode::FAILURE;
        }
    };
    // The runner's own checks belong to the whole run and to the plain set: the gpu set is the GPU case.
    let self_checks =
        !arguments.iter().any(|argument| argument == NO_SELF_CHECKS) && set != Some(CaseSet::Gpu);
    let cases =
        match support::load_cases(&support::spec_path()).and_then(|cases| in_set(cases, set)) {
            Ok(cases) => cases,
            Err(message) => {
                eprintln!("cli_spec: {message}");
                return ExitCode::FAILURE;
            }
        };
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_codekarl"));
    println!(
        "cli_spec: {} case(s) against {} ({:?} mode, features: {FEATURES:?})",
        cases.len(),
        binary.display(),
        mode
    );
    let results: Vec<(String, Verdict)> = cases
        .iter()
        .map(|case| (case.name(), evaluate(case, FEATURES, &binary, CASE_TIMEOUT)))
        .collect();
    // The runner's own checks are reported with the cases but are not cases of the specification.
    let mut checks = Vec::new();
    if set != Some(CaseSet::Gpu) {
        checks.push((
            "full-spec variable that is not valid Unicode is rejected (this executable)".to_owned(),
            undecodable_value_is_rejected(),
        ));
    }
    if self_checks {
        checks.push((
            "no variable redirects the specification (this executable)".to_owned(),
            redirect_variables_change_nothing(set),
        ));
    }
    let summary = summarize(&results, &checks, mode);
    for line in &summary.lines {
        println!("{line}");
    }
    if summary.passed {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
