//! The harness's own logic that needs no device, checked on every run of the binary (the "not run" one included): the child's layer
//! environment, the reading of the loader's log against the job's validation state, K3's verdict and the captured frames kept by their
//! submission. Each check names what it covers; a failure is reported by name and fails the run.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::Path;

use std::collections::VecDeque;

use ck_gpu_vulkan::{DrawnRecord, FrameCapture, FrameCounters, vk};

use super::phases::{
    CAPS_MIN_TEXTURE_BYTES, CAPS_TEXTURE_BYTES, K3_IMAGE_BYTES, K3_IMAGES, POOL_BLOCK_BYTES,
    Window, caps_min_verdict, coverage_problem, expected_textures_created, k2_verdict, k3_verdict,
};
use super::rig::{Captures, ordinal_of};
use super::{Job, LAYER_NAME, LAYER_VARIABLES, VALIDATION_VARIABLE, job_command};
use super::{layer_in_loader_log, layer_problem};

type Check = fn() -> Result<(), String>;

/// How many checks there are.
pub(crate) const COUNT: usize = CHECKS.len();

const CHECKS: [(&str, Check); 16] = [
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
    (
        "a capture is tied to its committed frame by the submission, not by its place among the captures",
        a_capture_is_tied_to_its_committed_frame_by_the_submission,
    ),
    (
        "set A under the foreground is 32,768 + 1 images, plus the checker when it is resident",
        the_created_images_count_the_checker_when_it_is_resident,
    ),
    (
        "captures are read from a cursor on, once each, and earlier scenes are not in them",
        captures_are_read_from_a_cursor_on_once_each,
    ),
    (
        "more than eight captures between two reads are reported as lost, not forgotten",
        captures_gone_before_they_are_read_are_counted,
    ),
    (
        "the K2 window must be complete, begin after the baseline and reach both ends",
        the_k2_window_must_be_complete_and_bound_at_both_ends,
    ),
    (
        "K2 fails on a wrong frame in the window and on a window with a hole",
        k2_fails_on_a_wrong_frame_or_a_hole,
    ),
    (
        "K3's budget holds the pool's first block and some, not all, of K3's images",
        the_caps_budget_admits_some_but_not_all_of_k3,
    ),
    (
        "caps-min needs one refusal by the texture-bytes budget with the pool at its ceiling",
        caps_min_needs_the_refusal_at_the_ceiling,
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

fn the_created_images_count_the_checker_when_it_is_resident() -> Result<(), String> {
    // The first real run: 32,770 created = 32,768 + the foreground + the resident checker.
    if expected_textures_created(true) != 32_770 || expected_textures_created(false) != 32_769 {
        return Err(format!(
            "{} with the checker, {} without it",
            expected_textures_created(true),
            expected_textures_created(false)
        ));
    }
    Ok(())
}

fn a_capture_is_tied_to_its_committed_frame_by_the_submission() -> Result<(), String> {
    let record = |ordinal: u64, submission: u64| DrawnRecord {
        ordinal,
        generation: Some(7),
        submission,
        counters: FrameCounters::default(),
    };
    // Submissions 5 and 6 were submitted but never queued (no record); frames 1 to 3 were queued as submissions 4, 7 and 8.
    let records: VecDeque<DrawnRecord> = [record(1, 4), record(2, 7), record(3, 8)].into();
    if ordinal_of(&records, 7) != Some(2) || ordinal_of(&records, 8) != Some(3) {
        return Err("a queued frame's ordinal was not found by its submission".to_owned());
    }
    if ordinal_of(&records, 5).is_some() || ordinal_of(&records, 6).is_some() {
        return Err("a submission that was never queued was given an ordinal".to_owned());
    }
    Ok(())
}

fn numbers(captures: &Captures, from: u64) -> (u64, Vec<(u64, u64)>) {
    let (lost, kept) = captures.since(from);
    (
        lost,
        kept.iter()
            .map(|(number, frame)| (*number, frame.submission))
            .collect(),
    )
}

fn captures_are_read_from_a_cursor_on_once_each() -> Result<(), String> {
    let captures = Captures::default();
    // Scenes before the cursor (the controls: K1's foreground-only frames) are not in what is read from it.
    push(&captures, &[10, 11, 12, 13, 14]);
    let cursor = captures.cursor();
    if cursor != 5 {
        return Err(format!("the cursor after five captures is {cursor}, not 5"));
    }
    if captures.cursor() != 5 || numbers(&captures, cursor) != (0, Vec::new()) {
        return Err("a cursor with no capture after it read something".to_owned());
    }
    push(&captures, &[15, 16, 17]);
    let (lost, kept) = numbers(&captures, cursor);
    if lost != 0 || kept != [(5, 15), (6, 16), (7, 17)] {
        return Err(format!("from the cursor: lost {lost}, {kept:?}"));
    }
    // Reading again from the number after the last taken gives nothing, and the earlier ones are not given twice.
    if numbers(&captures, 8) != (0, Vec::new()) || numbers(&captures, 7).1 != [(7, 17)] {
        return Err("a capture was given twice or one after the last was invented".to_owned());
    }
    // Reading from the start of the run still has the earlier scenes: the cursor is what excludes them.
    if numbers(&captures, 0).1.len() != 8 {
        return Err("the eight kept captures were not all there from number 0".to_owned());
    }
    Ok(())
}

fn captures_gone_before_they_are_read_are_counted() -> Result<(), String> {
    let captures = Captures::default();
    let cursor = captures.cursor();
    // Twelve arrive between two reads: the last eight are kept, the first four are gone and said to be.
    push(&captures, &(100..112).collect::<Vec<_>>());
    let (lost, kept) = numbers(&captures, cursor);
    if lost != 4 || kept.len() != 8 || kept[0] != (4, 104) || kept[7] != (11, 111) {
        return Err(format!("twelve between two reads: lost {lost}, {kept:?}"));
    }
    // Having been told, the reader resumes after the gap and is not told again.
    if numbers(&captures, 12) != (0, Vec::new()) {
        return Err("a read after the gap was told of a loss".to_owned());
    }
    // Exactly eight between two reads loses none.
    push(&captures, &(200..208).collect::<Vec<_>>());
    let (lost, kept) = numbers(&captures, 12);
    if lost != 0 || kept.len() != 8 {
        return Err(format!(
            "eight between two reads: lost {lost}, {} kept",
            kept.len()
        ));
    }
    Ok(())
}

fn window(ordinals: &[u64]) -> Window<'_> {
    Window {
        lost: 0,
        unreadable: 0,
        unidentified: 0,
        ordinals,
        overflowed: false,
        baseline_index: 9,
        first_required: 12,
        last_required: 15,
    }
}

fn the_k2_window_must_be_complete_and_bound_at_both_ends() -> Result<(), String> {
    // A complete window: from the frame after the baseline, consecutive, covering 12 to 15.
    let complete = [10, 11, 12, 13, 14, 15, 16];
    if let Some(problem) = coverage_problem(&window(&complete)) {
        return Err(format!("a complete window was refused: {problem}"));
    }
    let tight = [12, 13, 14, 15];
    if let Some(problem) = coverage_problem(&window(&tight)) {
        return Err(format!(
            "a window of exactly the required frames was refused: {problem}"
        ));
    }
    let late_start = [13, 14, 15];
    let early_end = [10, 11, 12, 13, 14];
    let hole_inside = [10, 11, 13, 14, 15];
    let hole_at_the_required_start = [10, 11, 13, 14, 15, 16];
    let before_the_baseline = [8, 9, 10, 11, 12, 13, 14, 15];
    let at_the_baseline = [9, 10, 11, 12, 13, 14, 15];
    let out_of_order = [10, 11, 13, 12, 14, 15];
    let repeated = [10, 11, 11, 12, 13, 14, 15];
    let refused: [(&str, Option<String>); 11] = [
        (
            "the first required frame missing",
            coverage_problem(&window(&late_start)),
        ),
        (
            "the end frame missing",
            coverage_problem(&window(&early_end)),
        ),
        (
            "a frame missing in the middle",
            coverage_problem(&window(&hole_inside)),
        ),
        (
            "the first required frame missing inside the window",
            coverage_problem(&window(&hole_at_the_required_start)),
        ),
        (
            "frames before the baseline",
            coverage_problem(&window(&before_the_baseline)),
        ),
        (
            "the baseline frame itself",
            coverage_problem(&window(&at_the_baseline)),
        ),
        (
            "frames out of order",
            coverage_problem(&window(&out_of_order)),
        ),
        ("a frame taken twice", coverage_problem(&window(&repeated))),
        ("no frame", coverage_problem(&window(&[]))),
        (
            "a capture lost",
            coverage_problem(&Window {
                lost: 1,
                ..window(&complete)
            }),
        ),
        (
            "a capture unreadable",
            coverage_problem(&Window {
                unreadable: 1,
                ..window(&complete)
            }),
        ),
    ];
    for (what, problem) in refused {
        if problem.is_none() {
            return Err(format!("{what} was accepted"));
        }
    }
    if coverage_problem(&Window {
        unidentified: 1,
        ..window(&complete)
    })
    .is_none()
    {
        return Err("a capture with no committed record was accepted".to_owned());
    }
    if coverage_problem(&Window {
        overflowed: true,
        ..window(&complete)
    })
    .is_none()
    {
        return Err("a window that could not be retained was accepted".to_owned());
    }
    Ok(())
}

fn k2_fails_on_a_wrong_frame_or_a_hole() -> Result<(), String> {
    let complete = [10, 11, 12, 13, 14, 15];
    let none = coverage_problem(&window(&complete));
    if !k2_verdict(0, 0, 0, 0, true, none.as_deref()) {
        return Err("a clean transition with a complete window did not pass".to_owned());
    }
    let hole = [10, 11, 12, 14, 15];
    let hole = coverage_problem(&window(&hole));
    let refused: [(&str, bool); 6] = [
        (
            "a wrong cell in a decoded frame",
            k2_verdict(0, 0, 1, 0, true, none.as_deref()),
        ),
        (
            "a wrong cell in the end frame",
            k2_verdict(0, 0, 0, 1, true, none.as_deref()),
        ),
        (
            "a refused reload",
            k2_verdict(1, 0, 0, 0, true, none.as_deref()),
        ),
        (
            "a regressed shown revision",
            k2_verdict(0, 1, 0, 0, true, none.as_deref()),
        ),
        (
            "revisions not all shown",
            k2_verdict(0, 0, 0, 0, false, none.as_deref()),
        ),
        (
            "a correct window with a frame missing (a wrong one could be the missing one)",
            k2_verdict(0, 0, 0, 0, true, hole.as_deref()),
        ),
    ];
    for (what, accepted) in refused {
        if accepted {
            return Err(format!("{what} was accepted"));
        }
    }
    Ok(())
}

fn the_caps_budget_admits_some_but_not_all_of_k3() -> Result<(), String> {
    let room = CAPS_TEXTURE_BYTES
        .checked_sub(POOL_BLOCK_BYTES)
        .ok_or("the budget is below one pool block")?;
    let admitted = room / K3_IMAGE_BYTES;
    if admitted < 1 || admitted >= u64::from(K3_IMAGES) {
        return Err(format!(
            "{admitted} of {K3_IMAGES} images fit beside the first block, so K3 could not both admit and refuse one"
        ));
    }
    // The minimum run is CK-130's ceiling: the first block and nothing else.
    if CAPS_MIN_TEXTURE_BYTES != POOL_BLOCK_BYTES {
        return Err("caps-min is not the one-block ceiling".to_owned());
    }
    Ok(())
}

fn caps_min_needs_the_refusal_at_the_ceiling() -> Result<(), String> {
    let limit = CAPS_MIN_TEXTURE_BYTES;
    let at_ceiling = format!(
        "texture_bytes budget: 16777216 more bytes would pass the limit ({limit} of {limit} reserved)"
    );
    let below = format!(
        "texture_bytes budget: 16777216 more bytes would pass the limit (60 of {limit} reserved)"
    );
    let count = "allocation count limit: 4096 of 4096 memory allocations are in use".to_owned();
    if !caps_min_verdict(0, 1, std::slice::from_ref(&at_ceiling), limit) {
        return Err("a refusal at the ceiling was not accepted".to_owned());
    }
    let refused: [(&str, bool); 5] = [
        (
            "an image that became ready",
            caps_min_verdict(1, 0, &[], limit),
        ),
        ("a refusal with no text", caps_min_verdict(0, 1, &[], limit)),
        (
            "a refusal with the pool below its ceiling",
            caps_min_verdict(0, 1, &[below], limit),
        ),
        (
            "an allocation-count refusal",
            caps_min_verdict(0, 1, &[count], limit),
        ),
        (
            "two refusals for one request",
            caps_min_verdict(0, 2, &[at_ceiling.clone(), at_ceiling], limit),
        ),
    ];
    for (what, accepted) in refused {
        if accepted {
            return Err(format!("{what} was accepted"));
        }
    }
    Ok(())
}
