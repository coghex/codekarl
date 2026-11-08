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
        r#"
local function time_bar_log(message)
  if window.record then window.record(message) else log.info(message) end
end
local time_bar_directory = os.getenv("CK_WINDOWED_TEST_TIME_BAR") or os.getenv("CK_VULKAN_TEST_TIME_BAR")
local time_bar_markers = {}
local function time_bar_mark(name)
  if time_bar_directory ~= nil then time_bar_markers[#time_bar_markers + 1] = name end
end
local function time_bar_flush_markers()
  local ready = time_bar_markers
  time_bar_markers = {}
  for _, name in ipairs(ready) do touch(time_bar_directory .. "/" .. name) end
end
local time_bar_button = require("lib.button")
local real_time_bar_button_new = time_bar_button.new
time_bar_button.new = function(spec)
  local is_transport = spec.icons ~= nil and #spec.icons == 2 and spec.icons[1] == "pause" and spec.icons[2] == "play"
  local created = real_time_bar_button_new(spec)
  if is_transport then
    time_bar_log("time-bar icon " .. created.icon .. " initial")
    local real_set_icon = created.set_icon
    function created:set_icon(name)
      real_set_icon(self, name)
      if self.icon == name then time_bar_log("time-bar icon " .. name .. " after") end
    end
  end
  return created
end
"#,
    );
    script.insert_str(
        0,
        "local time_bar_initialized = false\nlocal time_bar_saw_box_down = false\n",
    );
    let time = r#"game.on_time(function(time)
    local state = time.paused and tostring(time.paused) or "running"
    time_bar_log(string.format("time-bar speed %d %s boxes %d", time.speed, state, time.speed))
    if time_bar_directory and time.speed == 1 and state == "load" and not time_bar_initialized then
      time_bar_initialized = true
      game.control("time_faster")
      game.control("time_faster")
    elseif time_bar_directory and time.speed == 3 and state == "load" then
      time_bar_mark("time-bar-speed3-paused")
    elseif time_bar_directory and time.speed == 4 and state == "load" then
      time_bar_mark("time-bar-speed4-paused")
    elseif time_bar_directory and time.speed == 4 and state == "running" then
      time_bar_mark("time-bar-speed4-running")
    end
"#;
    script = script.replace("game.on_time(function(time)\n", time);
    let frame = r#"frame.on_update(function(update)
  time_bar_flush_markers()
  for _, event in ipairs((update.pointer and update.pointer.events) or {}) do
    if event.key == "time_bar" and event.kind == "down" then
      time_bar_saw_box_down = true
      time_bar_log("time-bar box down")
    elseif event.key == "time_bar" and event.kind == "up" and time_bar_saw_box_down then
      time_bar_saw_box_down = false
      time_bar_mark("time-bar-box-drag-observed")
      time_bar_log("time-bar box up")
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
    let first_input = output
        .find("lua.ui: time-bar speed 3 load boxes 3")
        .ok_or("the first injected input (speed 3) is missing")?;
    for independent_startup_event in [
        "lua.ui: time-bar speed 1 load boxes 1",
        "lua.ui: time-bar icon play initial",
    ] {
        let at = output
            .find(independent_startup_event)
            .ok_or_else(|| format!("{independent_startup_event:?} is missing"))?;
        if at >= first_input {
            return Err(format!(
                "{independent_startup_event:?} must occur before the first injected input"
            ));
        }
    }
    let lua = [
        "lua.ui: time-bar speed 3 load boxes 3",
        "lua.ui: time-bar speed 4 load boxes 4",
        "lua.ui: time-bar speed 4 running boxes 4",
        "lua.ui: time-bar icon pause after",
        "lua.ui: time-bar box down",
        "lua.ui: time-bar box up",
        "window: window closed",
    ];
    let sidecar = [
        "time-bar-fast-forward-injected",
        "time-bar-resume-injected",
        "time-bar-box-drag-injected",
        "time-bar-box-drag-observed-injected",
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
