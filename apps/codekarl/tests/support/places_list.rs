//! The places list's window witness (milestone 8, slices 6 and 7c): the shipped `scripts/ui/main.lua` run unchanged over Ireland 768, with
//! observation wrappers installed before it and markers the native driver (`native/testing.m`, `ck_app_t_schedule_places_list`) polls.
//!
//! The driver clicks the `places` button, scrolls over the list, clicks a row (which opens the location panel, whose Location tab logs its
//! people line), closes the panel with Escape and the dropdown by its title. The wrapper publishes the real dropdown geometry so input follows the production point-based anchor.
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
local function mark(name, contents) queued[#queued + 1] = {name, contents} end
local emitted = {}
local function mark_once(name, contents)
  if emitted[name] then return end
  emitted[name] = true
  mark(name, contents)
end
local function flush()
  local ready = queued
  queued = {}
  for _, marker in ipairs(ready) do
    if marker[2] == nil then
      touch(marker[1])
    else
      local file = assert(io.open(directory .. '/' .. marker[1], 'w'))
      file:write(marker[2])
      file:close()
    end
  end
end

local windows_library = require('lib.window')
local lists_library = require('lib.list')
local button_library = require('lib.button')

local real_button_new = button_library.new
button_library.new = function(spec)
  local created = real_button_new(spec)
  if spec.text == 'places' then
    local width, height = created:size()
    mark('places-button-center', string.format('%.0f %.0f', spec.x + width / 2, spec.y + height / 2))
    mark_once('places-button-ready')
  end
  return created
end

local observed = nil
local last_s = 0
local real_list_new = lists_library.new
lists_library.new = function(spec)
  if spec.on_select == nil then
    -- The location panel's list (it only scrolls): not the places list this witness watches. Its first row is the Location tab's
    -- people line, for the place the click opened.
    log.info('location tab ' .. spec.items[1])
    return real_list_new(spec)
  end
  observed = real_list_new(spec)
  log.info('places list opened ' .. #spec.items .. ' items ' .. spec.rows .. ' rows')
  mark_once('places-ready')
  return observed
end

local real_windows_new = windows_library.new
windows_library.new = function(spec)
  local places = spec.title == 'places'
  local real_close = spec.on_close
  local real_title_click = spec.on_title_click
  if places then
    spec.on_title_click = function(clicked)
      log.info('places title clicked')
      mark_once('places-title-clicked')
      if real_title_click ~= nil then real_title_click(clicked) end
    end
  end
  spec.on_close = function(closing)
    if places then
      log.info('places window closed')
      mark_once('places-closed')
    else
      log.info('place window closed')
      mark_once('place-closed')
    end
    if real_close ~= nil then real_close(closing) end
  end
  local created = real_windows_new(spec)
  if places then
    local real_move_to = created.move_to
    function created:move_to(x, y)
      real_move_to(self, x, y)
      local left, top = self:position()
      local width, height = self:size()
      mark('places-window-geometry', string.format('%d %d %d %d %d', left, top, width, height, self.title_height))
      mark('places-title-center', string.format('%.0f %.0f', left + width / 2, top + self.title_height / 2))
    end
    log.info('places window opened')
  else
    log.info('place window opened ' .. spec.title)
    -- What the click should have opened: the place under the driver's click point (LIST_POINT, 13 points below the list's top at
    -- no scroll) at the list's current scroll, whatever the backend's wheel delta made that.
    local index = math.floor((13 + observed.s) / lists_library.ROW) + 1
    log.info('place window expected ' .. game.places()[index].name)
    mark_once('place-ready')
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
      if observed.s > 0 then mark_once('list-scrolled') end
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
    // The panel's Location tab was built from the clicked place's view of the real start: its people line is a count.
    let people = next("lua.ui: location tab people: ")?;
    let count = lines[people]
        .rsplit_once("people: ")
        .map(|(_, rest)| rest.trim());
    if count.is_none_or(|count| count.parse::<u64>().is_err()) {
        return Err(format!(
            "the Location tab's people line has no count: {:?}",
            lines[people]
        ));
    }
    next("lua.ui: place window closed")?;
    next("lua.ui: places title clicked")?;
    next("lua.ui: places window closed")?;
    next("lua.ui: close requested by key")?;
    next("window: window closed")?;
    if lines
        .iter()
        .filter(|line| line.contains("lua.ui: places window opened"))
        .count()
        != 1
        || lines
            .iter()
            .filter(|line| line.contains("lua.ui: places list opened"))
            .count()
            != 1
    {
        return Err("the second Places button click opened a duplicate window or list".to_owned());
    }
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
