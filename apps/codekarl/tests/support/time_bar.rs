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

/// The run's last Escape opens the pause menu (CK-297) instead of ending the run: the witness leaves as the player does, through the
/// menu's own "quit to desktop" button.
const QUIT_THROUGH_MENU: &str = r#"
local time_bar_menu = require("lib.menu")
local real_time_bar_menu_open = time_bar_menu.open
time_bar_menu.open = function(view)
  local created = real_time_bar_menu_open(view)
  time_bar_menu.current().buttons["quit to desktop"].on_click()
  return created
end
"#;

/// The shipped UI script with observation markers inside its real time and frame callbacks.
pub(crate) fn script() -> String {
    format!("{QUIT_THROUGH_MENU}{}", observed_script())
}

fn observed_script() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui/main.lua");
    let mut script = fs::read_to_string(path).expect("the shipped UI script");
    script.insert_str(
        0,
        r#"
local function time_bar_log(message)
  if window.record then window.record(message) else log.info(message) end
end
local time_bar_directory = os.getenv("CK_WINDOWED_TEST_TIME_BAR") or os.getenv("CK_VULKAN_TEST_TIME_BAR")
local time_bar_view_width, time_bar_view_height = nil, nil
local time_bar_markers = {}
local time_bar_emitted_markers = {}
local function time_bar_mark(name)
  if time_bar_directory ~= nil and not time_bar_emitted_markers[name] then
    time_bar_emitted_markers[name] = true
    time_bar_markers[#time_bar_markers + 1] = name
  end
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
        "local time_bar_initialized = false\nlocal time_bar_saw_speed5 = false\nlocal time_bar_speed_box_down = nil\nlocal time_bar_speed_box_dragged = false\n",
    );
    let time = r#"game.on_time(function(time)
    local state = time.paused and tostring(time.paused) or "running"
    time_bar_log(string.format("time-bar speed %d %s boxes %d", time.speed, state, time.speed))
    if time_bar_directory and time_bar_speed_box_dragged and time.speed == 4 and state == "running" then
      time_bar_mark("time-bar-speed4-after-drag")
    end
    if time_bar_directory and time.speed == 1 and state == "load" and not time_bar_initialized then
      time_bar_initialized = true
      game.control("time_faster")
      game.control("time_faster")
    elseif time_bar_directory and time.speed == 3 and state == "load" then
      time_bar_mark(time_bar_saw_speed5 and "time-bar-speed3-reset-paused" or "time-bar-speed3-paused")
    elseif time_bar_directory and time.speed == 5 and state == "load" then
      time_bar_saw_speed5 = true
      time_bar_mark("time-bar-speed5-paused")
    elseif time_bar_directory and time.speed == 4 and state == "load" then
      time_bar_mark("time-bar-speed4-paused")
    elseif time_bar_directory and time.speed == 4 and state == "running" then
      time_bar_mark("time-bar-speed4-running")
    end
"#;
    script = script.replace("game.on_time(function(time)\n", time);
    let frame = r#"frame.on_update(function(update)
  if update.width ~= time_bar_view_width or update.height ~= time_bar_view_height then
    time_bar_view_width, time_bar_view_height = update.width, update.height
    time_bar_log(string.format("time-bar UI extent %.17g %.17g", time_bar_view_width, time_bar_view_height))
    if time_bar_directory ~= nil then
      local path = time_bar_directory .. "/time-bar-ui-extent"
      local temporary = path .. ".tmp"
      local file = assert(io.open(temporary, "w"))
      file:write(string.format("%.17g %.17g\n", time_bar_view_width, time_bar_view_height))
      file:close()
      assert(os.rename(temporary, path))
    end
  end
  time_bar_flush_markers()
  for _, event in ipairs((update.pointer and update.pointer.events) or {}) do
    if event.kind == "down" and event.key and string.match(event.key, "^speed_[1-5]$") then
      time_bar_speed_box_down = event.key
      time_bar_speed_box_dragged = false
      time_bar_log("time-bar box down " .. event.key .. " inside " .. tostring(event.inside))
    elseif event.kind == "move" and time_bar_speed_box_down ~= nil and not event.inside then
      time_bar_speed_box_dragged = true
    elseif event.kind == "up" and event.key == time_bar_speed_box_down then
      time_bar_speed_box_down = nil
      if time_bar_speed_box_dragged then time_bar_mark("time-bar-box-drag-observed") end
      time_bar_log("time-bar box up " .. event.key .. " inside " .. tostring(event.inside))
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
        .find("lua.ui: time-bar box down speed_5 inside true")
        .ok_or("the first injected input (speed 5 click) is missing")?;
    for independent_startup_event in [
        "lua.ui: time-bar speed 1 load boxes 1",
        "lua.ui: time-bar icon play initial",
        "lua.ui: time-bar UI extent ",
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
        "lua.ui: time-bar speed 5 load boxes 5",
        "lua.ui: time-bar speed 3 load boxes 3",
        "lua.ui: time-bar speed 4 load boxes 4",
        "lua.ui: time-bar box down speed_3 inside true",
        "lua.ui: time-bar box up speed_3 inside false",
        "window: window closed",
    ];
    let sidecar = [
        "time-bar-speed5-click-injected",
        "time-bar-speed3-reset-injected",
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
    let up = output
        .find("lua.ui: time-bar box up speed_3 inside false")
        .ok_or("the speed-box drag must release outside speed_3")?;
    let closed = output
        .find("window: window closed")
        .ok_or("the fixture window did not close")?;
    if !output[up..closed].contains("lua.ui: time-bar speed 4 running boxes 4") {
        return Err("releasing outside speed_3 changed the selected speed".to_owned());
    }
    for independent_event in [
        "lua.ui: time-bar speed 4 running boxes 4",
        "lua.ui: time-bar icon pause after",
    ] {
        if !output.contains(independent_event) {
            return Err(format!("{independent_event:?} is missing"));
        }
    }
    let cameras: Vec<&str> = output
        .lines()
        .filter(|line| line.starts_with("camera "))
        .collect();
    // The 4x4 fixture crop is centred on world cell (23884, 23884), or fixed point
    // (782630912, 782630912); the speed-box capture must leave that initial camera unchanged.
    if cameras != ["camera 0 782630912 782630912"] {
        return Err(format!(
            "a bar-captured drag changed the map camera: {cameras:?}"
        ));
    }
    Ok(())
}

/// Exercises the revised assertion against the captured failure shape and its corrected output.
pub(crate) fn captured_failure_and_doctored_output_are_checked_offline() {
    // When set, this points to a retained run whose complete output is being checked offline. The
    // short fallback keeps the contract runnable in clean copies without pipeline evidence.
    let capture_path = std::env::var_os("CK_TIME_BAR_CAPTURE");
    let captured = capture_path
        .as_deref()
        .and_then(|path| fs::read_to_string(path).ok())
        .unwrap_or_else(|| {
            r"
INFO lua.ui: time-bar speed 1 load boxes 1
INFO lua.ui: time-bar icon play initial
INFO lua.ui: time-bar speed 3 load boxes 3
INFO lua.ui: time-bar speed 4 load boxes 4
INFO lua.ui: time-bar speed 4 running boxes 4
INFO lua.ui: time-bar icon pause after
INFO lua.ui: time-bar speed 4 running boxes 4
"
            .to_owned()
        });
    if capture_path.is_some() {
        validate(&captured).unwrap_or_else(|error| panic!("retained time-bar output: {error}"));
        return;
    }
    assert!(
        validate(&captured).is_err(),
        "the original failed capture must stay failing"
    );

    // Add the expected speed-box key and outside release to that same capture. Speed remains 4
    // after the up, and the camera line is unchanged by the captured drag.
    let mut doctored = captured;
    let first_input = doctored
        .find("lua.ui: time-bar speed 4 running boxes 4")
        .expect("the capture has the running time-bar state");
    doctored.insert_str(first_input, "INFO lua.ui: time-bar UI extent 1280 800\n");
    doctored.push_str(
        "\nINFO lua.ui: time-bar box down speed_5 inside true\nINFO lua.ui: time-bar box up speed_5 inside true\
         \ntime-bar-speed5-click-injected\nINFO lua.ui: time-bar speed 5 load boxes 5\
         \nINFO lua.ui: time-bar speed 3 load boxes 3\ntime-bar-speed3-reset-injected\
         \ntime-bar-fast-forward-injected\nINFO lua.ui: time-bar speed 4 load boxes 4\
         \ntime-bar-resume-injected\ntime-bar-box-drag-injected\
         \nINFO lua.ui: time-bar box down speed_3 inside true\
         \nINFO lua.ui: time-bar box up speed_3 inside false\
         \nINFO lua.ui: time-bar speed 4 running boxes 4\
         \ntime-bar-box-drag-observed-injected\nwindow: window closed\ncamera 0 782630912 782630912\n",
    );
    validate(&doctored).unwrap_or_else(|error| panic!("doctored capture: {error}"));
}
