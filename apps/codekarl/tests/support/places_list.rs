//! The places list's window witness (milestone 8, slice 6): the shipped `scripts/ui/main.lua` run unchanged over Ireland 768, with
//! observation wrappers installed before it and markers the native driver (`native/testing.m`, `ck_app_t_schedule_places_list`) polls.
//!
//! The driver clicks the `places` button, scrolls over the list, clicks a row, and closes the windows with Escape; this module only
//! prepares the script and judges the combined output. Geometry the driver relies on, all validated offline by the windowless case:
//! the time bar at the 8-point margin puts the `places` button's centre at (262, 27); the wrapper opens the Places window at (100, 100),
//! so its list's top-left corner is (106, 136), a row is 26 points high and the row under (126, 149) is the first, or, after a scroll
//! of `s` points, the one at `(13 + s) / 26`.
//!
//! **Markers are written one frame after the callback that made them returns**, so that the scene the callback edited has been
//! published (and the router holds its regions) before the driver acts on the marker.

use std::path::Path;

/// The prefix: wrappers over the libraries' module tables, installed before `main.lua` runs and so seen by it.
const PREFIX: &str = r"
do -- scoped: the fixture wrapper's own helpers (touch, wait_for) stay what they are
local directory = assert(
  os.getenv('CK_WINDOWED_TEST_MAP_SELECT') or os.getenv('CK_VULKAN_TEST_MAP_SELECT') or MARKER_DIR,
  'the places list marker directory is not configured'
)
local function touch(name)
  local file = assert(io.open(directory .. '/' .. name, 'w'))
  file:write('x')
  file:close()
end
-- Markers wait one frame: the scene a callback edited is published when the callback returns, and the driver acts on a marker.
local queued = {}
local function mark(name) queued[#queued + 1] = name end
local function flush()
  local ready = queued
  queued = {}
  for _, name in ipairs(ready) do touch(name) end
end

local windows_library = require('lib.window')
local lists_library = require('lib.list')
local button_library = require('lib.button')

local real_button_new = button_library.new
button_library.new = function(spec)
  local created = real_button_new(spec)
  if spec.text == 'places' then mark('places-button-ready') end
  return created
end

local observed = nil
local last_s = 0
local real_list_new = lists_library.new
lists_library.new = function(spec)
  observed = real_list_new(spec)
  log.info('places list opened ' .. #spec.items .. ' items ' .. spec.rows .. ' rows')
  mark('places-ready')
  return observed
end

local real_windows_new = windows_library.new
windows_library.new = function(spec)
  local places = spec.title == 'Places'
  if places then
    -- A known place for the driver's coordinates.
    spec.x, spec.y = 100, 100
  end
  local real_close = spec.on_close
  spec.on_close = function(closing)
    if places then
      log.info('places window closed')
      mark('places-closed')
    else
      log.info('place window closed')
      mark('place-closed')
    end
    if real_close ~= nil then real_close(closing) end
  end
  local created = real_windows_new(spec)
  if places then
    log.info('places window opened')
  else
    log.info('place window opened ' .. spec.title)
    -- What the click should have opened: the place under the driver's click point (LIST_POINT, 13 points below the list's top at
    -- no scroll) at the list's current scroll, whatever the backend's wheel delta made that.
    local index = math.floor((13 + observed.s) / lists_library.ROW) + 1
    log.info('place window expected ' .. game.places()[index].name)
    mark('place-ready')
  end
  return created
end

local real_on_update = frame.on_update
frame.on_update = function(callback)
  if callback == nil then return real_on_update(nil) end
  real_on_update(function(update)
    -- Markers queued by an earlier frame's callbacks go out first: their scenes are published by now.
    flush()
    callback(update)
    if observed ~= nil and not observed.closed and observed.s ~= last_s then
      last_s = observed.s
      log.info('places list s=' .. observed.s)
      if observed.s > 0 then mark('list-scrolled') end
    end
  end)
end
end
";

/// The shipped UI script behind the wrappers.
pub(crate) fn script() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/ui/main.lua");
    let main = std::fs::read_to_string(path).expect("the shipped UI script");
    format!("{PREFIX}\n{main}")
}

/// The configuration text of the witness, for `backend` (`native-metal` or `glfw-vulkan`): Ireland 768 with the shipped assets.
pub(crate) fn config_text(backend: &str, saves: &Path) -> String {
    let project = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    format!(
        "[window]\nbackend = \"{backend}\"\n\n[game]\nscenario = '{}'\nsaves = '{}'\n\n[render]\nassets = '{}'\n\n[ui]\npoints_per_pixel = 2\n\n[frame]\nupdates_per_second = 60\n",
        project.join("assets/scenarios/ireland-768").display(),
        saves.display(),
        project.join("assets").display()
    )
}

/// Judges the combined output (Lua log lines and the driver's sidecar): the windows opened and closed in order, the list scrolled (by a
/// positive whole number of UI pixels, whatever the backend's wheel delta made of the injected scroll), the click opened the place
/// window of the place under the click point at that scroll, and the map camera did not change between the driver's
/// `before-list-scroll` and `after-list-scroll`.
pub(crate) fn validate(output: &str) -> Result<(), String> {
    let lines: Vec<&str> = output.lines().collect();
    let mut start = 0;
    let mut next = |needle: &str| -> Result<usize, String> {
        let Some(offset) = lines[start..].iter().position(|seen| seen.contains(needle)) else {
            return Err(format!("{needle:?} is missing or out of order"));
        };
        start += offset + 1;
        Ok(start - 1)
    };
    next("lua.ui: places window opened")?;
    next("lua.ui: places list opened 94 items 12 rows")?;
    let scrolled = next("lua.ui: places list s=")?;
    let scroll: u32 = lines[scrolled]
        .rsplit_once("places list s=")
        .and_then(|(_, rest)| rest.trim().parse().ok())
        .ok_or("the scroll is not a number")?;
    if scroll == 0 || !scroll.is_multiple_of(2) {
        return Err(format!(
            "the list scrolled by {scroll}, not a positive whole number of UI pixels"
        ));
    }
    let opened = next("lua.ui: place window opened ")?;
    let expected = next("lua.ui: place window expected ")?;
    let title = lines[opened]
        .rsplit_once("place window opened ")
        .map(|(_, rest)| rest);
    let wanted = lines[expected]
        .rsplit_once("place window expected ")
        .map(|(_, rest)| rest);
    if title.is_none() || title != wanted {
        return Err(format!(
            "the click opened {title:?} but the place under it was {wanted:?}"
        ));
    }
    next("lua.ui: place window closed")?;
    next("lua.ui: places window closed")?;
    next("lua.ui: close requested by key")?;
    next("window: window closed")?;
    let before = lines
        .iter()
        .position(|line| *line == "before-list-scroll")
        .ok_or("the driver recorded no start of the list scroll")?;
    let after = lines
        .iter()
        .position(|line| *line == "after-list-scroll")
        .ok_or("the driver recorded no end of the list scroll")?;
    if before >= after {
        return Err("the scroll markers are out of order".to_owned());
    }
    if lines[before + 1..after]
        .iter()
        .any(|line| line.starts_with("camera "))
    {
        return Err(format!(
            "a wheel over the list changed the map camera: {:?}",
            &lines[before..=after]
        ));
    }
    Ok(())
}
