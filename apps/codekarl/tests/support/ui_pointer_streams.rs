pub(crate) fn validate(lua_stdout: &str, sidecar: &str, backend: &str) -> Result<(), String> {
    validate_click_output(lua_stdout, sidecar, backend)?;

    let lines: Vec<_> = sidecar.lines().collect();
    let before = sidecar
        .lines()
        .find(|line| line.starts_with("camera "))
        .ok_or_else(|| format!("{backend} sidecar has no initial camera"))?;
    let drag = lines
        .iter()
        .position(|line| *line == "map-drag-dispatched")
        .ok_or_else(|| format!("{backend} sidecar has no dispatched drag"))?;
    let after = lines[drag + 1..]
        .iter()
        .find(|line| line.starts_with("camera "))
        .ok_or_else(|| format!("{backend} sidecar has no post-drag camera"))?;
    if before == *after {
        return Err(format!("{backend} outside drag did not pan"));
    }
    Ok(())
}

pub(crate) fn validate_combined_output(output: &str, backend: &str) -> Result<(), String> {
    // `Seen.stdout` concatenates the app's logs with the native observation sidecar. Recover each
    // source from its own record format, rather than splitting at markers whose order is under test.
    let lua_stdout = output
        .lines()
        .filter(|line| line.starts_with("INFO  lua.ui:") || line.starts_with("INFO  window:"))
        .collect::<Vec<_>>()
        .join("\n");
    let sidecar = output
        .lines()
        .filter(|line| !line.starts_with("INFO  "))
        .collect::<Vec<_>>()
        .join("\n");
    validate(&lua_stdout, &sidecar, backend)
}

pub(crate) fn validate_click_output(
    lua_stdout: &str,
    sidecar: &str,
    backend: &str,
) -> Result<(), String> {
    ordered(
        lua_stdout,
        &[
            "ui-hit-published",
            "ui-pointer down fixture_panel",
            "ui-pointer up fixture_panel",
            "window: window closed",
        ],
    )
    .map_err(|error| format!("{backend} Lua stdout: {error}"))?;
    let ordered_sidecar = if backend == "Vulkan" {
        [
            "ui-cursor-move-injected",
            "ui-click-injected",
            "ui-click-delivered",
            "ui-drag-cursor-move-injected",
            "map-drag-dispatched",
        ]
        .as_slice()
    } else {
        [
            "ui-click-injected",
            "ui-click-delivered",
            "map-drag-dispatched",
        ]
        .as_slice()
    };
    ordered(sidecar, ordered_sidecar).map_err(|error| format!("{backend} sidecar: {error}"))?;

    let lines: Vec<_> = sidecar.lines().collect();
    let click = lines
        .iter()
        .position(|line| *line == "ui-click-injected")
        .ok_or_else(|| format!("{backend} sidecar has no injected click"))?;
    let delivered = lines
        .iter()
        .position(|line| *line == "ui-click-delivered")
        .ok_or_else(|| format!("{backend} sidecar has no delivered click"))?;
    if lines[click + 1..delivered]
        .iter()
        .any(|line| line.starts_with("camera "))
    {
        return Err(format!("{backend} UI click moved the map"));
    }
    Ok(())
}

fn ordered(input: &str, needles: &[&str]) -> Result<(), String> {
    let mut offset = 0;
    for needle in needles {
        let Some(found) = input[offset..].find(needle) else {
            return Err(format!("missing or out of order {needle:?}"));
        };
        offset += found + needle.len();
    }
    Ok(())
}
