//! The CLI specification runner's logic, shared by the runner (`cli_spec.rs`) and its unit tests
//! (`cli_spec_unit.rs`): case parsing, the validation-notice filter, the full-spec switch, a child
//! run with a deadline, and the per-case and overall verdicts.

use std::{
    ffi::OsStr,
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

/// The environment variable that turns on the full specification.
pub(crate) const FULL_SPEC_VARIABLE: &str = "CK_REQUIRE_FULL_SPEC";

/// How long one case may run before its child is killed and the case fails.
pub(crate) const CASE_TIMEOUT: Duration = Duration::from_secs(30);

/// The two lines Apple's validation layers print on start-up; only these are dropped.
const NOTICES: [&str; 2] = [
    "Metal API Validation Enabled",
    "Metal GPU Validation Enabled",
];

/// The one specification, at a path fixed when this crate's tests are compiled. Nothing in the
/// environment or the arguments can change it.
pub(crate) fn spec_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../specs/bootstrap.json")
}

/// Reads every case of the specification file at `path`; the error names what is wrong.
pub(crate) fn load_cases(path: &Path) -> Result<Vec<Case>, String> {
    let text =
        std::fs::read_to_string(path).map_err(|error| format!("{}: {error}", path.display()))?;
    let document: Value =
        serde_json::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
    document["cases"]
        .as_array()
        .ok_or_else(|| format!("{}: `cases` must be an array", path.display()))?
        .iter()
        .map(Case::from_json)
        .collect()
}

/// The argument that restricts a run to one case set.
pub(crate) const SET_FLAG: &str = "--set";

/// A part of the specification that a run may be restricted to. The cases split by what they need: a
/// `plain` case needs nothing but the binary; a `gpu` case (one that `requires` a feature, today only
/// `--metal-smoke`) runs the Metal GPU. With no `--set` the whole specification runs, as it always has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CaseSet {
    Plain,
    Gpu,
}

impl CaseSet {
    /// Both sets, by the name `--set` takes.
    pub(crate) const ALL: [(&'static str, Self); 2] = [("plain", Self::Plain), ("gpu", Self::Gpu)];

    /// The name `--set` takes.
    pub(crate) fn name(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(_, set)| *set == self)
            .map_or("?", |(name, _)| name)
    }

    /// The set a case belongs to: one that requires a feature runs the GPU.
    pub(crate) fn of(case: &Case) -> Self {
        if case.requires.is_empty() {
            Self::Plain
        } else {
            Self::Gpu
        }
    }
}

/// The set a run was restricted to (`None`: the whole specification). An unknown, empty, repeated or
/// valueless `--set`, or one written `--set=NAME`, is an error: never a run of nothing.
pub(crate) fn requested_set(arguments: &[String]) -> Result<Option<CaseSet>, String> {
    let mut named = None;
    let mut position = 0;
    while position < arguments.len() {
        let argument = &arguments[position];
        position += 1;
        if argument != SET_FLAG {
            if argument.starts_with(SET_FLAG) {
                return Err(format!(
                    "{argument:?} is not understood; use `{SET_FLAG} <plain|gpu>`"
                ));
            }
            continue;
        }
        let Some(value) = arguments.get(position) else {
            return Err(format!("{SET_FLAG} needs a value (plain or gpu)"));
        };
        position += 1;
        let Some((_, set)) = CaseSet::ALL.iter().find(|(name, _)| name == value) else {
            return Err(format!(
                "unknown case set {value:?}; the sets are plain and gpu"
            ));
        };
        if named.replace(*set).is_some() {
            return Err(format!("{SET_FLAG} was given more than once"));
        }
    }
    Ok(named)
}

/// The cases of `set` (all of them without one); a set with no case is an error.
pub(crate) fn in_set(cases: Vec<Case>, set: Option<CaseSet>) -> Result<Vec<Case>, String> {
    let Some(set) = set else {
        return Ok(cases);
    };
    let chosen: Vec<Case> = cases
        .into_iter()
        .filter(|case| CaseSet::of(case) == set)
        .collect();
    if chosen.is_empty() {
        Err(format!(
            "the {} set has no case in the specification",
            set.name()
        ))
    } else {
        Ok(chosen)
    }
}

/// One case of `specs/bootstrap.json`.
#[derive(Debug, Clone)]
pub(crate) struct Case {
    pub(crate) args: Vec<String>,
    pub(crate) code: i32,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) requires: Vec<String>,
}

impl Case {
    /// Reads one case; the error names what is wrong with it.
    pub(crate) fn from_json(value: &Value) -> Result<Self, String> {
        let text = |key: &str| {
            value[key]
                .as_str()
                .map(str::to_owned)
                .ok_or_else(|| format!("case {value}: `{key}` must be a string"))
        };
        let list = |key: &str| -> Result<Vec<String>, String> {
            match value.get(key) {
                None => Ok(Vec::new()),
                Some(Value::Array(items)) => items
                    .iter()
                    .map(|item| {
                        item.as_str()
                            .map(str::to_owned)
                            .ok_or_else(|| format!("case {value}: `{key}` must hold strings"))
                    })
                    .collect(),
                Some(_) => Err(format!("case {value}: `{key}` must be an array")),
            }
        };
        let code = value["code"]
            .as_i64()
            .and_then(|code| i32::try_from(code).ok())
            .ok_or_else(|| format!("case {value}: `code` must be an integer"))?;
        Ok(Self {
            args: list("args")?,
            code,
            stdout: text("stdout")?,
            stderr: text("stderr")?,
            requires: list("requires")?,
        })
    }

    /// The case as it appears in output: its arguments, or `<no arguments>`.
    pub(crate) fn name(&self) -> String {
        if self.args.is_empty() {
            "<no arguments>".to_owned()
        } else {
            self.args.join(" ")
        }
    }
}

/// Normal mode skips what the binary was not built for; full mode fails on any skip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Unset or `0`.
    Normal,
    /// `1`: the whole specification must run.
    Full,
}

impl Mode {
    /// Reads the variable as the OS holds it: absent is normal mode, a value that is not valid Unicode
    /// is an error naming the variable (it must never select normal mode), anything else goes through
    /// `from_value`.
    pub(crate) fn from_os(value: Option<&OsStr>) -> Result<Self, String> {
        match value.map(OsStr::to_str) {
            None => Ok(Self::Normal),
            Some(Some(text)) => Self::from_value(Some(text)),
            Some(None) => Err(format!(
                "{FULL_SPEC_VARIABLE} is not valid Unicode: {value:?}"
            )),
        }
    }

    /// Reads `CK_REQUIRE_FULL_SPEC`; any value but unset, `0` or `1` is an error naming the variable.
    pub(crate) fn from_value(value: Option<&str>) -> Result<Self, String> {
        match value {
            None | Some("0") => Ok(Self::Normal),
            Some("1") => Ok(Self::Full),
            Some(other) => Err(format!(
                "{FULL_SPEC_VARIABLE} must be unset, 0 or 1, not {other:?}"
            )),
        }
    }
}

/// True for a line (without its terminator) that is exactly one validation notice, bare or with the
/// process prefix `<YYYY-MM-DD HH:MM:SS.mmm> codekarl[<pid>:<tid>] `.
pub(crate) fn is_validation_notice(line: &str) -> bool {
    NOTICES.iter().any(|notice| {
        line.strip_suffix(notice)
            .is_some_and(|prefix| prefix.is_empty() || is_notice_prefix(prefix))
    })
}

fn is_notice_prefix(prefix: &str) -> bool {
    let Some((stamp, rest)) = prefix.split_at_checked(23) else {
        return false;
    };
    let digits = |text: &str, count: usize| {
        text.len() == count && text.bytes().all(|byte| byte.is_ascii_digit())
    };
    let stamp_ok = stamp.split_once(' ').is_some_and(|(date, time)| {
        let mut date = date.split('-');
        let (clock, millis) = time.split_once('.').unwrap_or(("", ""));
        let mut clock = clock.split(':');
        date.next().is_some_and(|part| digits(part, 4))
            && date.next().is_some_and(|part| digits(part, 2))
            && date.next().is_some_and(|part| digits(part, 2))
            && date.next().is_none()
            && clock.next().is_some_and(|part| digits(part, 2))
            && clock.next().is_some_and(|part| digits(part, 2))
            && clock.next().is_some_and(|part| digits(part, 2))
            && clock.next().is_none()
            && digits(millis, 3)
    });
    let Some(process) = rest
        .strip_prefix(' ')
        .and_then(|rest| rest.strip_suffix("] "))
    else {
        return false;
    };
    let Some(ids) = process.strip_prefix("codekarl[") else {
        return false;
    };
    let numeric = |text: &str| !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit());
    stamp_ok
        && ids
            .split_once(':')
            .is_some_and(|(pid, tid)| numeric(pid) && numeric(tid))
}

/// Removes only the lines that are exactly a recognized notice (with `\n`, or unterminated at the
/// end). Every other line is returned byte for byte, with the terminator it had (or none). The
/// runner applies it to both streams of a child.
pub(crate) fn filter_notices(output: &str) -> String {
    output
        .split_inclusive('\n')
        .filter(|segment| !is_validation_notice(segment.strip_suffix('\n').unwrap_or(segment)))
        .collect()
}

/// What one child run produced.
#[derive(Debug, Clone)]
pub(crate) struct Actual {
    /// The exit code, or `None` when the child was killed (by the deadline or by a signal).
    pub(crate) code: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) timed_out: bool,
    /// The child's process id (it has been reaped by the time this is returned).
    pub(crate) pid: u32,
}

fn drain<R: Read + Send + 'static>(stream: Option<R>) -> thread::JoinHandle<String> {
    thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut stream) = stream {
            // A read error leaves what arrived so far, which is what the report should show.
            let _ = stream.read_to_end(&mut bytes);
        }
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

/// Runs the binary with both Metal validation layers on, and waits at most `timeout`: a child
/// that outlives it is killed and reaped, and the run is reported as timed out.
pub(crate) fn run_command(
    binary: &Path,
    args: &[String],
    timeout: Duration,
) -> std::io::Result<Actual> {
    let mut child = Command::new(binary)
        .args(args)
        .env("MTL_DEBUG_LAYER", "1")
        .env("MTL_SHADER_VALIDATION", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let pid = child.id();
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let (status, timed_out) = loop {
        if let Some(status) = child.try_wait()? {
            break (status, false);
        }
        if Instant::now() >= deadline {
            child.kill()?;
            break (child.wait()?, true);
        }
        thread::sleep(Duration::from_millis(5));
    };
    Ok(Actual {
        code: if timed_out { None } else { status.code() },
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
        timed_out,
        pid,
    })
}

/// The result of one case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Verdict {
    Passed,
    /// The binary was not built with a feature the case requires.
    Skipped(String),
    /// The child outlived its deadline.
    TimedOut(String),
    Failed(String),
}

/// Runs one case, or skips it when the binary lacks a feature it requires.
pub(crate) fn evaluate(
    case: &Case,
    features: &[&str],
    binary: &Path,
    timeout: Duration,
) -> Verdict {
    if let Some(missing) = case
        .requires
        .iter()
        .find(|feature| !features.contains(&feature.as_str()))
    {
        return Verdict::Skipped(format!("requires the `{missing}` feature"));
    }
    let actual = match run_command(binary, &case.args, timeout) {
        Ok(actual) => actual,
        Err(error) => {
            return Verdict::Failed(format!("could not run {}: {error}", binary.display()));
        }
    };
    if actual.timed_out {
        return Verdict::TimedOut(format!(
            "timed out after {} s; child {} was killed and reaped",
            timeout.as_secs_f64(),
            actual.pid
        ));
    }
    // The executable routes stderr to stdout, so a validation notice may arrive on either stream.
    let stdout = filter_notices(&actual.stdout);
    let stderr = filter_notices(&actual.stderr);
    if actual.code == Some(case.code) && stdout == case.stdout && stderr == case.stderr {
        Verdict::Passed
    } else {
        Verdict::Failed(format!(
            "expected code {}, stdout {:?}, stderr {:?}; got code {:?}, stdout {:?}, stderr {:?}",
            case.code, case.stdout, case.stderr, actual.code, stdout, stderr
        ))
    }
}

/// Every case's result, the lines to print, and whether the run as a whole passes.
#[derive(Debug)]
pub(crate) struct Summary {
    pub(crate) lines: Vec<String>,
    pub(crate) passed: bool,
}

/// Reports every result together. `cases` are the cases of the specification; `checks` are the
/// runner's own self-checks, reported alongside but never counted as cases. A failure or a timeout
/// fails the run in both modes; a skip, and a specification with no cases, fail it only in full
/// mode, with the cases named.
pub(crate) fn summarize(
    cases: &[(String, Verdict)],
    checks: &[(String, Verdict)],
    mode: Mode,
) -> Summary {
    let mut lines = Vec::new();
    let mut problems: Vec<String> = Vec::new();
    for (name, verdict) in cases.iter().chain(checks) {
        match verdict {
            Verdict::Passed => lines.push(format!("test {name} ... ok")),
            Verdict::Skipped(why) => {
                lines.push(format!("test {name} ... skipped ({why})"));
                if mode == Mode::Full {
                    problems.push(format!("skipped in full-spec mode: {name} ({why})"));
                }
            }
            Verdict::TimedOut(why) => {
                lines.push(format!("test {name} ... TIMED OUT ({why})"));
                problems.push(format!("timed out: {name} ({why})"));
            }
            Verdict::Failed(why) => {
                lines.push(format!("test {name} ... FAILED ({why})"));
                problems.push(format!("failed: {name} ({why})"));
            }
        }
    }
    if cases.is_empty() && mode == Mode::Full {
        problems.push("no cases ran in full-spec mode".to_owned());
    }
    let skipped = cases
        .iter()
        .filter(|(_, verdict)| matches!(verdict, Verdict::Skipped(_)))
        .count();
    let passed = |results: &[(String, Verdict)]| {
        results
            .iter()
            .filter(|(_, verdict)| *verdict == Verdict::Passed)
            .count()
    };
    lines.push(format!(
        "{} case(s): {} passed, {skipped} skipped; {} runner check(s): {} passed; {} problem(s)",
        cases.len(),
        passed(cases),
        checks.len(),
        passed(checks),
        problems.len()
    ));
    if !problems.is_empty() {
        lines.push("problems:".to_owned());
        lines.extend(problems.iter().map(|problem| format!("  {problem}")));
    }
    Summary {
        lines,
        passed: problems.is_empty(),
    }
}
