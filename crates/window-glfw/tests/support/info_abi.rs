//! The one Rust binding of the shared native test support's `CKGlfwTestInfo`, and the check that it matches the
//! native layout. `crates/window-glfw/tests/support/mod.rs` and `crates/gpu-vulkan/tests/support/mod.rs` both
//! include this file by path, so there is a single definition.
//!
//! **Why a check.** `ck_glfw_test_info` writes the whole native structure through the pointer it is given. A
//! binding that is smaller than the native structure (CK-82 added two fields to the native side) makes that call
//! write past its object. The native side therefore reports the layout it was compiled with
//! (`ck_glfw_test_info_layout`: size, alignment and the offset of each field), and [`read`], the only function
//! in the test trees that can call `ck_glfw_test_info` (its declaration is private to this module), compares it
//! with this binding first, once, and panics naming the first mismatched field instead of calling.

use std::mem::{align_of, offset_of, size_of};
use std::sync::OnceLock;

/// What `ck_glfw_test_info` reports (`CKGlfwTestInfo`). The fields are in the native declaration order.
#[repr(C)]
pub(crate) struct RawInfo {
    pub(crate) has_window: i32,
    pub(crate) title: [u8; 256],
    pub(crate) content_width: f64,
    pub(crate) content_height: f64,
    pub(crate) resizable: i32,
    pub(crate) visible: i32,
    pub(crate) is_key: i32,
    pub(crate) window_scale: f64,
    pub(crate) has_main_menu: i32,
    pub(crate) menu_items: u32,
    pub(crate) menu_key_equivalents: u32,
    pub(crate) window_count: u64,
    pub(crate) filtered_keys: u64,
    pub(crate) activation_policy: i32,
    pub(crate) is_active: i32,
}

unsafe extern "C" {
    /// Private on purpose: [`read`] is the only caller, and it checks the layout first.
    fn ck_glfw_test_info(out: *mut RawInfo);
    fn ck_glfw_test_info_layout(out: *mut u64, capacity: usize) -> usize;
}

/// The layout values of this binding, named: size, alignment, then the offset of each field in declaration
/// order, the order `ck_glfw_test_info_layout` reports them in.
fn binding_layout() -> Vec<(&'static str, u64)> {
    macro_rules! offsets {
        ($($field:ident),* $(,)?) => {
            [$((stringify!($field), offset_of!(RawInfo, $field) as u64)),*]
        };
    }
    let mut layout = vec![
        ("size", size_of::<RawInfo>() as u64),
        ("alignment", align_of::<RawInfo>() as u64),
    ];
    layout.extend(offsets![
        has_window,
        title,
        content_width,
        content_height,
        resizable,
        visible,
        is_key,
        window_scale,
        has_main_menu,
        menu_items,
        menu_key_equivalents,
        window_count,
        filtered_keys,
        activation_policy,
        is_active,
    ]);
    layout
}

/// Compares the native values with a binding's named ones: the first value that differs is named, and then a
/// different number of values is.
fn compare(native: &[u64], binding: &[(&str, u64)]) -> Result<(), String> {
    for (&(name, mine), &theirs) in binding.iter().zip(native) {
        if mine != theirs {
            return Err(format!(
                "CKGlfwTestInfo layout mismatch at `{name}`: the native side has {theirs}, this binding {mine}"
            ));
        }
    }
    if native.len() != binding.len() {
        return Err(format!(
            "CKGlfwTestInfo layout mismatch: the native side reports {} layout values, this binding describes {}",
            native.len(),
            binding.len()
        ));
    }
    Ok(())
}

/// The layout the native side was compiled with.
fn native_layout() -> Result<Vec<u64>, String> {
    let mut values = [0_u64; 64];
    // SAFETY: `values` is valid for the 64 writes `capacity` allows; the call writes at most that many, touches
    // no AppKit state and may be made from any thread.
    let count = unsafe { ck_glfw_test_info_layout(values.as_mut_ptr(), values.len()) };
    values.get(..count).map(<[u64]>::to_vec).ok_or_else(|| {
        format!(
            "the native layout has {count} values, more than {}",
            values.len()
        )
    })
}

/// Checks this binding against the native layout. Needs no window.
///
/// # Errors
///
/// The first mismatched field, or the number of values when they differ in count.
pub(crate) fn check_layout() -> Result<(), String> {
    compare(&native_layout()?, &binding_layout())
}

/// The check, made once per process; a mismatch panics (every time it is asked for) before any native call
/// that would write through the binding.
pub(crate) fn assert_layout() {
    static RESULT: OnceLock<Result<(), String>> = OnceLock::new();
    if let Err(message) = RESULT.get_or_init(check_layout) {
        panic!("{message}");
    }
}

/// The window's state as the native side reports it, read after the layout check. Main thread.
pub(crate) fn read() -> RawInfo {
    assert_layout();
    let mut raw = std::mem::MaybeUninit::<RawInfo>::zeroed();
    // SAFETY: the layout of `RawInfo` was just checked against the native structure, so `raw` is valid for the
    // whole write the native call makes (it also zeroes the structure first), on the main thread.
    unsafe { ck_glfw_test_info(raw.as_mut_ptr()) };
    // SAFETY: the native call initialised every field (and the zeroed value was valid anyway).
    unsafe { raw.assume_init() }
}

/// The comparison is not vacuous: against the native layout it rejects the 336-byte binding that CK-82's
/// two new fields outgrew (naming `size`), a binding that lost a field in the middle (naming the first field
/// after it, by offset) and one that merely lacks the last fields (by count), and it accepts the current
/// binding. Needs no window.
///
/// # Errors
///
/// What the comparison got wrong.
pub(crate) fn check_the_comparison_rejects_mismatched_bindings() -> Result<(), String> {
    let native = native_layout()?;
    let current = binding_layout();
    compare(&native, &current)
        .map_err(|error| format!("the current binding must match: {error}"))?;
    // The binding before CK-82: no `activation_policy`, no `is_active`, and the size 336.
    let mut old: Vec<(&str, u64)> = current[..current.len() - 2].to_vec();
    old[0].1 = 336;
    let message = compare(&native, &old)
        .err()
        .ok_or("the 336-byte binding must be rejected")?;
    if !message.contains("`size`") {
        return Err(format!("the size must be named first: {message}"));
    }
    // Same size, but without the two new fields: the count differs.
    old[0].1 = native[0];
    let message = compare(&native, &old)
        .err()
        .ok_or("a binding without the last fields must be rejected")?;
    if !message.contains("layout values") {
        return Err(format!("the count must be named: {message}"));
    }
    // A field moved: the first one that no longer lines up is named.
    let mut moved = current.clone();
    let at = moved
        .iter()
        .position(|(name, _)| *name == "window_scale")
        .ok_or("window_scale is described")?;
    moved[at].1 += 8;
    let message = compare(&native, &moved)
        .err()
        .ok_or("a moved field must be rejected")?;
    if !message.contains("`window_scale`") {
        return Err(format!("the moved field must be named: {message}"));
    }
    Ok(())
}

/// No other file of either test tree binds `ck_glfw_test_info`: a second declaration could call it without
/// the layout check. Reads the `.rs` files of `crates/window-glfw/tests` and `crates/gpu-vulkan/tests`.
///
/// # Errors
///
/// The files that call it, or why a directory could not be read.
pub(crate) fn check_nothing_else_binds_the_native_info() -> Result<(), String> {
    fn visit(directory: &std::path::Path, found: &mut Vec<String>) -> Result<(), String> {
        let entries = std::fs::read_dir(directory)
            .map_err(|error| format!("{}: {error}", directory.display()))?;
        for entry in entries {
            let path = entry.map_err(|error| error.to_string())?.path();
            if path.is_dir() {
                visit(&path, found)?;
            } else if path.extension().is_some_and(|extension| extension == "rs")
                && path.file_name().is_some_and(|name| name != "info_abi.rs")
            {
                let text = std::fs::read_to_string(&path)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                // The call or declaration (`ck_glfw_test_info(`), not the layout function or a mention.
                if text.contains(concat!("ck_glfw_test_info", "(")) {
                    found.push(path.display().to_string());
                }
            }
        }
        Ok(())
    }
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut found = Vec::new();
    let mut files_seen = 0;
    for tree in ["window-glfw/tests", "gpu-vulkan/tests"] {
        let directory = crates.join(tree);
        visit(&directory, &mut found)?;
        files_seen += std::fs::read_dir(&directory).map_or(0, Iterator::count);
    }
    if files_seen < 4 {
        return Err("the test trees were not found".to_owned());
    }
    if found.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "these files bind `ck_glfw_test_info` themselves, outside the layout check: {}",
            found.join(", ")
        ))
    }
}
