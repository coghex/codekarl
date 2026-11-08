use std::fs;
use std::path::Path;

/// Copies the shipped Lua helpers and the theme into the fixture roots used by the real application.
pub(crate) fn install_fixture_assets(ui_root: &Path, assets_root: &Path) {
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let library = project.join("scripts/ui/lib");
    let destination = ui_root.join("lib");
    fs::create_dir_all(&destination).expect("the fixture UI library directory");
    for entry in fs::read_dir(library).expect("the shipped UI library") {
        let entry = entry.expect("a UI library entry");
        fs::copy(entry.path(), destination.join(entry.file_name())).expect("copy a UI helper");
    }
    install_theme(assets_root);
}

/// Copies the shipped theme (`ui/theme.toml` and the frame art `ui/theme/*.png`) into an assets root: a run with a scenario reads the
/// theme at start-up, and the library loads the art.
pub(crate) fn install_theme(assets_root: &Path) {
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/ui");
    let destination = assets_root.join("ui/theme");
    fs::create_dir_all(&destination).expect("the fixture theme art directory");
    fs::copy(
        shipped.join("theme.toml"),
        assets_root.join("ui/theme.toml"),
    )
    .expect("copy the shipped theme");
    for entry in fs::read_dir(shipped.join("theme")).expect("the shipped theme art") {
        let entry = entry.expect("a theme art entry");
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "png")
        {
            fs::copy(entry.path(), destination.join(entry.file_name()))
                .expect("copy a theme image");
        }
    }
}

/// The shipped UI script with observation markers inside its real time and frame callbacks.
pub(crate) fn script() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui/main.lua");
    let mut script = fs::read_to_string(path).expect("the shipped UI script");
    script.insert_str(
        0,
        "local function time_bar_log(message)\n  if window.record then window.record(message) else log.info(message) end\nend\n",
    );
    let time = r#"game.on_time(function(time)
    local state = time.paused and tostring(time.paused) or "running"
    time_bar_log(string.format("time-bar speed %d %s", time.speed, state))
    local dir = os.getenv("CK_WINDOWED_TEST_TIME_BAR") or os.getenv("CK_VULKAN_TEST_TIME_BAR")
    if dir and time.speed == 1 and state == "load" then touch(dir .. "/time-bar-initial") end
    if dir and time.speed == 3 and state == "load" then touch(dir .. "/time-bar-speed3-applied") end
    if dir and time.speed == 3 and state == "running" then touch(dir .. "/time-bar-running") end
    if dir and time.speed == 3 and state == "player" then touch(dir .. "/time-bar-paused-again") end
"#;
    script = script.replace("game.on_time(function(time)\n", time);
    let frame = r#"frame.on_update(function(update)
  local dir = os.getenv("CK_WINDOWED_TEST_TIME_BAR") or os.getenv("CK_VULKAN_TEST_TIME_BAR")
  local saw_down = false
  if dir then
    local down = io.open(dir .. "/time-bar-drag-down", "r")
    saw_down = down ~= nil
    if down then down:close() end
  end
  for _, event in ipairs((update.pointer and update.pointer.events) or {}) do
    if event.key == "time_bar" and event.kind == "down" then
      saw_down = true
      if dir then touch(dir .. "/time-bar-drag-down") end
      time_bar_log("time-bar gap down")
    elseif event.key == "time_bar" and event.kind == "up" and saw_down then
      if dir then touch(dir .. "/time-bar-drag-observed") end
      time_bar_log("time-bar gap up")
    end
  end
  pointer.dispatch(update.pointer)
"#;
    script = script.replace(
        "frame.on_update(function(update)\n  pointer.dispatch(update.pointer)\n",
        frame,
    );
    script
}

/// Validates the sidecar and Lua log order from a time-bar window witness.
pub(crate) fn validate(output: &str) -> Result<(), String> {
    let lua = [
        "lua.ui: time-bar speed 1 load",
        "lua.ui: time-bar speed 3 load",
        "lua.ui: time-bar speed 3 running",
        "lua.ui: time-bar speed 3 player",
        "lua.ui: time-bar gap down",
        "lua.ui: time-bar gap up",
        "window: window closed",
    ];
    let sidecar = [
        "time-bar-speed3-injected",
        "time-bar-pause1-injected",
        "time-bar-pause2-injected",
        "time-bar-gap-drag-injected",
        "time-bar-gap-drag-observed",
    ];
    for (stream, expected) in [("Lua", &lua[..]), ("sidecar", &sidecar[..])] {
        let lines: Vec<&str> = output.lines().collect();
        let mut start = 0;
        for line in expected {
            let Some(offset) = lines[start..].iter().position(|seen| seen.contains(line)) else {
                return Err(format!("{stream} {line:?} is missing or out of order"));
            };
            start += offset + 1;
        }
    }
    let cameras: Vec<&str> = output
        .lines()
        .filter(|line| line.starts_with("camera "))
        .collect();
    if cameras != ["camera 0 782598144 782598144"] {
        return Err(format!(
            "a bar-captured drag changed the map camera: {cameras:?}"
        ));
    }
    Ok(())
}
