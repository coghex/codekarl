use std::{num::NonZeroI32, process::Child};

/// Converts a child pid to a signalable target, refusing zero and out-of-range values.
pub(crate) fn target(pid: u32) -> Result<NonZeroI32, String> {
    let pid = i32::try_from(pid).map_err(|_| "child pid does not fit in i32".to_owned())?;
    NonZeroI32::new(pid).ok_or_else(|| "child pid must be positive".to_owned())
}

/// Returns a signal target only while the supervisor still owns that live direct child.
pub(crate) fn live_pid(child: Option<&mut Child>, expected_pid: u32) -> Result<NonZeroI32, String> {
    let child = child.ok_or_else(|| "the supervisor does not own a child".to_owned())?;
    if child
        .try_wait()
        .map_err(|error| format!("cannot check child before signal: {error}"))?
        .is_some()
    {
        return Err("the supervisor's child has already exited".to_owned());
    }
    if child.id() != expected_pid {
        return Err("the supervisor's child pid changed".to_owned());
    }
    target(expected_pid)
}
