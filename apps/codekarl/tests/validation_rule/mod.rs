//! The output rule for Vulkan validation messages (milestone 5.5, issue 5), shared by `vulkan_app.rs` (a real window's output) and
//! the library's tests over the fake stack (supplied outputs).
//!
//! The application's run drains the instance's validation log into records. The one message the log tolerates (the bindless table
//! layout's, after the support query said no) is written as an `INFO` record carrying [`MARKER`]; the loader's own notice that the
//! environment adds the validation layer (manager ruling `3wzbqcc8`) is an `INFO` record carrying [`LOADER_MARKER`]; every other
//! message is a `warn` record `vulkan validation: ...`, and a dropped count is one more. A line of the output that names a
//! validation message or the loader's message is accepted **only** when it is one of the two marked records **and** carries their
//! fixed fields: the record's own level `INFO`; and the tolerated VUID, or the loader's exact words at its observed type and severity. Provenance comes from the
//! in-process scope that decided the flag when the message was recorded, never from comparing numbers: an unmarked line with the
//! same VUID (a message from another `vkCreateDescriptorSetLayout` call), an unmarked loader line, a marked record with another
//! VUID, type, severity or text, a `warn` validation record and a dropped-count record all fail, whatever the counts.

/// The words that mark a tolerated record (the application's `TOLERATED_MARKER`; the library's tests check they are equal).
pub(crate) const MARKER: &str =
    "vulkan validation (tolerated: MoltenVK #2573, bindless table layout)";

/// The one VUID a marked record may name.
pub(crate) const TOLERATED_VUID: &str = "VUID-vkCreateDescriptorSetLayout-support-09582";

/// The words that mark the loader's notice (the application's `LOADER_NOTICE_MARKER`).
pub(crate) const LOADER_MARKER: &str = "vulkan loader notice (not a validation message)";

/// The loader's words at their observed type and severity, for the two variables the loader names.
const LOADER_WORDS: [&str; 2] = [
    "[WARNING GENERAL] Loader Message: env var 'VK_INSTANCE_LAYERS' defined and adding layers",
    "[WARNING GENERAL] Loader Message: env var 'VK_LOADER_LAYERS_ENABLE' defined and adding layers",
];

/// Whether a line names a validation message or the loader's message at all.
fn names_validation(line: &str) -> bool {
    line.contains("Validation")
        || line.contains("VUID")
        || line.contains("vulkan validation")
        || line.contains("Loader Message")
}

/// Whether `line` is an `INFO` record whose text carries `marker`: the words of the log line before the marker (its timestamp when
/// there is one, then its level) name `INFO` and neither `WARN` nor `ERROR`. A marked record at another level is not accepted.
fn info_record_with(line: &str, marker: &str) -> bool {
    let Some(at) = line.find(marker) else {
        return false;
    };
    let mut level = line[..at].split_whitespace();
    level.clone().any(|word| word == "INFO") && !level.any(|word| word == "WARN" || word == "ERROR")
}

/// Whether the line is a tolerated layout record: an `INFO` record, marked, naming the tolerated VUID.
fn tolerated(line: &str) -> bool {
    info_record_with(line, MARKER) && line.contains(TOLERATED_VUID)
}

/// Whether the line is the loader's notice: an `INFO` record, marked, with the loader's exact words at the observed type and
/// severity.
fn loader_notice(line: &str) -> bool {
    info_record_with(line, LOADER_MARKER) && LOADER_WORDS.iter().any(|words| line.contains(words))
}

/// The lines of `output` that name a validation message without being one of the two accepted records.
pub(crate) fn offending_lines(output: &str) -> Vec<&str> {
    output
        .lines()
        .filter(|line| names_validation(line) && !tolerated(line) && !loader_notice(line))
        .collect()
}

/// `Ok` when every such line of `output` is an accepted record; else the offending lines.
pub(crate) fn check(output: &str) -> Result<(), String> {
    let offending = offending_lines(output);
    if offending.is_empty() {
        Ok(())
    } else {
        Err(format!("a validation message:\n{}", offending.join("\n")))
    }
}

/// The rule over supplied outputs (the same function every case's output goes through): the genuinely scoped records pass, and
/// every substitute fails, whatever the counts. Called by `vulkan_app.rs` and by the library's tests.
pub(crate) fn supplied_outputs_behave() {
    let notice = "INFO  render: bindless table of 131072 slots (vkGetDescriptorSetLayoutSupport: not supported although every update-after-bind limit passes; MoltenVK #2573, proceeding)";
    let marked = format!(
        "INFO  render: {MARKER}: [ERROR VALIDATION] {TOLERATED_VUID}: the layout is not supported"
    );
    let loader_words = LOADER_WORDS[0];
    let loader =
        format!("INFO  render: {LOADER_MARKER}: {loader_words} \"VK_LAYER_KHRONOS_validation\"");
    assert!(check("").is_ok());
    assert!(
        check(notice).is_ok(),
        "the notice names no validation message"
    );
    assert!(
        check(&format!("{notice}\n{marked}")).is_ok(),
        "the genuinely scoped record"
    );
    assert!(
        check(&format!("{notice}\n{loader}\n{loader}")).is_ok(),
        "the loader's notices"
    );
    // An equal-count substitution: the notice, no marked record, one unmarked line with the same VUID (a message from another
    // `vkCreateDescriptorSetLayout` call). A count rule would accept it.
    let unmarked = format!(
        "[ERROR VALIDATION] {TOLERATED_VUID}: from another vkCreateDescriptorSetLayout call"
    );
    assert!(
        check(&format!("{notice}\n{unmarked}")).is_err(),
        "equal-count substitution"
    );
    // A marked record naming another VUID, a marked record with an unmarked copy, a warn validation record, a dropped count.
    assert!(
        check(&format!(
            "INFO  render: {MARKER}: [ERROR VALIDATION] VUID-other: x"
        ))
        .is_err()
    );
    assert!(check(&format!("{marked}\n{unmarked}")).is_err());
    assert!(check("WARN  render: vulkan validation: [ERROR VALIDATION] VUID-other: x").is_err());
    assert!(
        check(
            "WARN  render: vulkan validation: 3 further messages were dropped beyond the log's bound"
        )
        .is_err()
    );
    assert!(check("Validation Error: [ VUID-other ]").is_err());
    // The loader's notice: an unmarked raw line fails; a marked one with another type, severity, variable or text fails.
    let raw = format!("{loader_words} \"VK_LAYER_KHRONOS_validation\"");
    assert!(check(&raw).is_err(), "an unmarked loader line");
    assert!(
        check(&format!(
            "WARN  render: vulkan validation: [WARNING GENERAL] Loader Message: {raw}"
        ))
        .is_err()
    );
    // The record's own level: the marked records at WARN or ERROR (a regression in the classification-to-level mapping) fail too.
    for level in ["WARN ", "ERROR", "DEBUG"] {
        assert!(
            check(&format!(
                "{level} render: {MARKER}: [ERROR VALIDATION] {TOLERATED_VUID}: x"
            ))
            .is_err(),
            "a tolerated record at {level}"
        );
        assert!(
            check(&format!(
                "{level} render: {LOADER_MARKER}: {loader_words} \"VK_LAYER_KHRONOS_validation\""
            ))
            .is_err(),
            "a loader record at {level}"
        );
    }
    assert!(
        check(&format!("2026-10-31T10:00:00.123Z INFO  render: {MARKER}: [ERROR VALIDATION] {TOLERATED_VUID}: x")).is_ok(),
        "the timestamped INFO form"
    );
    for wrong in [
        "[ERROR GENERAL] Loader Message: env var 'VK_INSTANCE_LAYERS' defined and adding layers",
        "[WARNING VALIDATION] Loader Message: env var 'VK_INSTANCE_LAYERS' defined and adding layers",
        "[WARNING GENERAL] Loader Message: env var 'PATH' defined and adding layers",
        "[WARNING GENERAL] Loader Message: the loader says something else",
    ] {
        assert!(
            check(&format!("INFO  render: {LOADER_MARKER}: {wrong}")).is_err(),
            "a marked loader record with other fields: {wrong}"
        );
    }
}
