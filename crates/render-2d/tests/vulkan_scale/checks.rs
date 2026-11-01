//! The harness's own logic that needs no device, checked on every run of the binary (the "not run" one included): the child's layer
//! environment, the reading of the loader's log against the job's validation state, K3's verdict and the captured frames kept by their
//! submission. Each check names what it covers; a failure is reported by name and fails the run.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use ck_gpu_vulkan::{FrameCapture, vk};

use super::phases::k3_verdict;
use super::rig::Captures;
use super::{Job, LAYER_NAME, LAYER_VARIABLES, VALIDATION_VARIABLE, job_command};
use super::{layer_in_loader_log, layer_problem};

type Check = fn() -> Result<(), String>;

/// How many checks there are.
pub(crate) const COUNT: usize = CHECKS.len();

const CHECKS: [(&str, Check); 8] = [
    (
        "a timing job's child has no inherited loader layer variable and no implicit layer",
        a_timing_job_child_has_no_inherited_layer,
    ),
    (
        "a correctness job's child has the layer through the harness's variable alone",
        a_correctness_job_child_has_the_layer_through_its_variable_alone,
    ),
    (
        "the loader's log shows the layer inserted only on an insertion line",
        the_loader_log_shows_the_layer_only_on_an_insertion_line,
    ),
    (
        "a layer that is loaded in a timing job, or missing in a correctness job, is a problem",
        a_requested_and_actual_mismatch_is_a_problem,
    ),
    (
        "K3 needs every refusal to carry the texture-bytes reason",
        k3_needs_every_refusal_to_carry_the_texture_bytes_reason,
    ),
    (
        "a capture is found by its submission among the recent ones",
        a_capture_is_found_by_its_submission,
    ),
    (
        "a capture pushed out of the recent ones is told from one not yet taken",
        a_pushed_out_capture_is_told_from_one_not_yet_taken,
    ),
    (
        "the captures keep their order and count",
        the_captures_keep_their_order_and_count,
    ),
];

/// Runs every check; the failures, each with its name.
pub(crate) fn run() -> Vec<String> {
    CHECKS
        .iter()
        .filter_map(|(name, check)| check().err().map(|error| format!("{name}: {error}")))
        .collect()
}

fn job(validation: bool) -> Job {
    Job {
        name: "a job",
        worker: "a worker",
        validation,
        gated: true,
    }
}

/// What the command sets (`Some(value)`) or removes (`None`) in the child's environment.
fn child_environment(validation: bool) -> BTreeMap<OsString, Option<OsString>> {
    job_command(&job(validation), Path::new("/a/test/binary"))
        .get_envs()
        .map(|(name, value)| (name.to_owned(), value.map(OsString::from)))
        .collect()
}

fn expect(
    environment: &BTreeMap<OsString, Option<OsString>>,
    name: &str,
    want: Option<&str>,
) -> Result<(), String> {
    let got = environment.get(&OsString::from(name));
    let want = want.map(OsString::from);
    match got {
        Some(value) if *value == want => Ok(()),
        Some(value) => Err(format!(
            "{name}: expected {want:?}, the command sets {value:?}"
        )),
        None => Err(format!(
            "{name}: expected {want:?}, but the command leaves it alone (a child inherits it)"
        )),
    }
}

fn a_timing_job_child_has_no_inherited_layer() -> Result<(), String> {
    let environment = child_environment(false);
    for variable in LAYER_VARIABLES {
        if variable == "VK_LOADER_LAYERS_DISABLE" {
            continue;
        }
        // Removed explicitly: whatever the owner's environment says, the child does not inherit it.
        expect(&environment, variable, None)?;
    }
    expect(&environment, VALIDATION_VARIABLE, None)?;
    expect(&environment, "VK_LOADER_LAYERS_DISABLE", Some("~implicit~"))?;
    expect(&environment, "VK_LOADER_DEBUG", Some("layer"))
}

fn a_correctness_job_child_has_the_layer_through_its_variable_alone() -> Result<(), String> {
    let environment = child_environment(true);
    for variable in LAYER_VARIABLES {
        expect(&environment, variable, None)?;
    }
    expect(&environment, VALIDATION_VARIABLE, Some("1"))?;
    expect(&environment, "VK_LOADER_DEBUG", Some("layer"))
}

fn the_loader_log_shows_the_layer_only_on_an_insertion_line() -> Result<(), String> {
    let inserted = format!(
        "INFO | LAYER:  Insert instance layer \"{LAYER_NAME}\" (libVkLayer_khronos_validation.dylib)\n"
    );
    let found = format!(
        "INFO | LAYER:  Found manifest file, layer \"{LAYER_NAME}\" in /usr/local/share/vulkan/explicit_layer.d\n"
    );
    let other = "INFO | LAYER:  Insert instance layer \"VK_LAYER_KHRONOS_synchronization2\"\n";
    if !layer_in_loader_log(&inserted) {
        return Err("an insertion line was not recognised".to_owned());
    }
    if layer_in_loader_log(&found) {
        return Err("a manifest line that names the layer was taken for an insertion".to_owned());
    }
    if layer_in_loader_log(other) || layer_in_loader_log("") {
        return Err("another layer's insertion, or no text, was taken for this layer's".to_owned());
    }
    Ok(())
}

fn a_requested_and_actual_mismatch_is_a_problem() -> Result<(), String> {
    if layer_problem(Some(true), false).is_none() {
        return Err("the layer loaded in a timing job was not a problem".to_owned());
    }
    if layer_problem(Some(false), true).is_none() {
        return Err("the layer missing from a correctness job was not a problem".to_owned());
    }
    if layer_problem(Some(true), true).is_some() || layer_problem(Some(false), false).is_some() {
        return Err("a log that agrees with the job was a problem".to_owned());
    }
    // An unreadable log is a problem of its own (the stream error), not a verdict on the layer.
    if layer_problem(None, false).is_some() || layer_problem(None, true).is_some() {
        return Err("an unreadable log was read as a verdict".to_owned());
    }
    Ok(())
}

fn k3_needs_every_refusal_to_carry_the_texture_bytes_reason() -> Result<(), String> {
    let bytes =
        "texture_bytes budget: 16777216 more bytes would pass the limit (60 of 64 reserved)"
            .to_owned();
    let count = "allocation count limit: 4096 of 4096 memory allocations are in use".to_owned();
    let loose = "the texture bytes budget is spent".to_owned();
    if !k3_verdict(2, 3, &[bytes.clone(), bytes.clone(), bytes.clone()]) {
        return Err("three texture-bytes refusals were not accepted".to_owned());
    }
    let refused: [(&str, bool); 6] = [
        ("no refusal", k3_verdict(2, 0, &[])),
        (
            "no image ready",
            k3_verdict(0, 1, std::slice::from_ref(&bytes)),
        ),
        ("a refusal with no text", k3_verdict(2, 1, &[])),
        (
            "one text short",
            k3_verdict(2, 2, std::slice::from_ref(&bytes)),
        ),
        (
            "an allocation-count refusal among them",
            k3_verdict(2, 2, &[bytes.clone(), count]),
        ),
        (
            "a loosely worded byte/budget text",
            k3_verdict(2, 1, std::slice::from_ref(&loose)),
        ),
    ];
    for (what, accepted) in refused {
        if accepted {
            return Err(format!("{what} was accepted"));
        }
    }
    Ok(())
}

fn frame(submission: u64) -> FrameCapture {
    FrameCapture {
        generation: Some(7),
        submission,
        size: (1, 1),
        format: vk::Format::B8G8R8A8_UNORM,
        bytes: vec![0; 4],
    }
}

fn push(captures: &Captures, submissions: &[u64]) {
    let sink = captures.sink();
    let mut sink = sink.lock().expect("the capture sink");
    sink.extend(submissions.iter().copied().map(frame));
}

fn a_capture_is_found_by_its_submission() -> Result<(), String> {
    let captures = Captures::default();
    push(&captures, &[3, 4, 5]);
    let (found, newest) = captures.find(4);
    if found.map(|frame| frame.submission) != Some(4) {
        return Err("the capture of submission 4 was not found among 3, 4, 5".to_owned());
    }
    if newest != Some(5) {
        return Err(format!("the newest submission is 5, not {newest:?}"));
    }
    Ok(())
}

fn a_pushed_out_capture_is_told_from_one_not_yet_taken() -> Result<(), String> {
    let captures = Captures::default();
    // Nine captures: the recent ones are the last eight, so submission 1 is gone and submission 10 is not there yet.
    push(&captures, &(1..=9).collect::<Vec<_>>());
    let (gone, newest) = captures.find(1);
    if gone.is_some() || newest.is_none_or(|newest| newest <= 1) {
        return Err(format!(
            "submission 1 should be pushed out with a newer one seen: {:?}",
            (gone.is_some(), newest)
        ));
    }
    let (not_yet, newest) = captures.find(10);
    if not_yet.is_some() || newest.is_none_or(|newest| newest >= 10) {
        return Err(format!(
            "submission 10 should be absent with only older ones seen: {:?}",
            (not_yet.is_some(), newest)
        ));
    }
    Ok(())
}

fn the_captures_keep_their_order_and_count() -> Result<(), String> {
    let captures = Captures::default();
    push(&captures, &[1, 2]);
    push(&captures, &[3]);
    let (count, newest) = captures.poll();
    if count != 3 || newest.map(|frame| frame.submission) != Some(3) {
        return Err("three captures, the newest being submission 3, were expected".to_owned());
    }
    Ok(())
}
