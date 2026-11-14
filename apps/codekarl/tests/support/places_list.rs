//! The ledger's locations-tab witness: the shipped `scripts/ui/main.lua` run unchanged over Ireland 768, with
//! observation wrappers installed before it and markers the native driver (`native/testing.m`, `ck_app_t_schedule_places_list`) polls.
//!
//! The driver clicks the locations icon, scrolls over the ledger, clicks a row (which opens the location panel, whose Location tab logs
//! its people line), closes the panel with Escape and the ledger with its close button. The wrapper publishes the real ledger geometry.
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
local function witness_log(message)
  if window.record ~= nil then window.record(message) else log.info(message) end
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
local function mark_now_once(name)
  if emitted[name] then return end
  emitted[name] = true
  touch(name)
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
local places_button = nil

-- The run's last Escape now opens the pause menu (CK-297) instead of ending the run: the witness leaves as the player does, through the
-- menu's own quit button.
local menu_library = require('lib.menu')
local real_menu_open = menu_library.open
menu_library.open = function(view)
  local created = real_menu_open(view)
  menu_library.current().buttons['quit to desktop'].on_click()
  return created
end
local view_width, view_height = nil, nil
-- Keep this in step with ck_app_map_select_list_point in native/testing.m.
local list_click_y = 45

local real_button_new = button_library.new
button_library.new = function(spec)
  if spec.text == 'x' and spec.owner ~= nil and spec.owner.title == 'ledger' then
    local real_click = spec.on_click
    spec.on_click = function(...)
      log.info('places title clicked')
      mark_once('places-title-clicked')
      if real_click ~= nil then return real_click(...) end
    end
  end
  local created = real_button_new(spec)
  if spec.icon == 'location' then
    places_button = created
    local function mark_center(button)
      local width, height = button:size()
      mark('places-button-center', string.format('%.0f %.0f %.0f %.0f',
        button.x + width / 2, button.y + height / 2, view_width, view_height))
    end
    local width, height = created:size()
    mark('places-button-created-center', string.format('%.0f %.0f', created.x + width / 2, created.y + height / 2))
  end
  return created
end

local observed = nil
local last_s = 0
local real_list_new = lists_library.new
lists_library.new = function(spec)
  if spec.window.title ~= 'ledger' then
    -- The location panel's list (it only scrolls): not the ledger list this witness watches. Its first row is the Location tab's
    -- people line, for the place the click opened.
    local first = spec.items[1]
    if type(first) == 'table' then
      local cells = {}
      for _, cell in ipairs(first.cells) do cells[#cells + 1] = cell.text end
      first = table.concat(cells)
    end
    log.info('location tab ' .. first)
    return real_list_new(spec)
  end
  local real_on_select = spec.on_select
  spec.on_select = function(index)
    local visible_row = math.floor((list_click_y - observed.y) / lists_library.ROW) + 1
    local place = game.places()[index - 1]
    assert(place ~= nil, 'the clicked list row has no place')
    witness_log('places list click row ' .. index .. ' visible ' .. visible_row)
    witness_log('place window expected ' .. place.name)
    if real_on_select ~= nil then return real_on_select(index) end
  end
  observed = real_list_new(spec)
  witness_log('places list opened ' .. #spec.items .. ' items ' .. spec.rows .. ' rows')
  witness_log('places list geometry y=' .. observed.y .. ' row_height=' .. lists_library.ROW .. ' click_y=' .. list_click_y)
  mark_once('places-ready')
  return observed
end

local real_windows_new = windows_library.new
windows_library.new = function(spec)
  local places = spec.title == 'ledger'
  local real_close = spec.on_close
  spec.on_close = function(closing)
    if places then
      log.info('places window closed')
      mark_now_once('places-closed')
    else
      log.info('place window closed')
      mark_now_once('place-closed')
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
      assert(view_width ~= nil and view_height ~= nil, 'places geometry needs a published view size')
      mark('places-window-geometry', string.format('%d %d %d %d %d %.0f %.0f',
        left, top, width, height, self.title_height, view_width, view_height))
      local close = self.close_button
      mark('places-title-center', string.format('%s %.0f %.0f %.0f %.0f', close.key,
        left + close.x + close.width / 2, top + close.y + close.height / 2, view_width, view_height))
    end
    witness_log('places window opened')
  else
    witness_log('place window opened ' .. spec.title)
    mark_once('place-ready')
  end
  return created
end

local real_on_update = frame.on_update
frame.on_update = function(callback)
  if callback == nil then return real_on_update(nil) end
  real_on_update(function(update)
    view_width, view_height = update.width, update.height
    -- Markers queued by an earlier frame's callbacks go out first: their scenes are published by now.
    flush()
    callback(update)
    -- Read the button only after the app's resize relayout has run. The native driver must click the
    -- center that corresponds to the published first-frame hit region, not its creation-time layout.
    if places_button ~= nil then
      local width, height = places_button:size()
      mark('places-button-center', string.format('%.0f %.0f %.0f %.0f',
        places_button.x + width / 2, places_button.y + height / 2, view_width, view_height))
      mark_once('places-button-ready')
    end
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
        "[window]\nbackend = \"{backend}\"\n\n[game]\nscenario = '{}'\nsaves = '{}'\n\n[render]\nassets = '{}'\n\n[frame]\nupdates_per_second = 60\n",
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
    next_line(&lines, &mut start, "lua.ui: places window opened")?;
    validate_list_layout(&lines, &mut start)?;
    let expected = next_line(&lines, &mut start, "lua.ui: place window expected ")?;
    let opened = next_line(&lines, &mut start, "lua.ui: place window opened ")?;
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
    let people = next_line(&lines, &mut start, "lua.ui: location tab people: ")?;
    let count = lines[people]
        .rsplit_once("people: ")
        .map(|(_, rest)| rest.trim());
    if count.is_none_or(|count| count.parse::<u64>().is_err()) {
        return Err(format!(
            "the Location tab's people line has no count: {:?}",
            lines[people]
        ));
    }
    next_line(&lines, &mut start, "lua.ui: place window closed")?;
    next_line(&lines, &mut start, "lua.ui: places title clicked")?;
    next_line(&lines, &mut start, "lua.ui: places window closed")?;
    next_line(&lines, &mut start, "lua.ui: close requested by key")?;
    next_line(&lines, &mut start, "window: window closed")?;
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

fn next_line(lines: &[&str], start: &mut usize, needle: &str) -> Result<usize, String> {
    let Some(offset) = lines[*start..]
        .iter()
        .position(|seen| seen.contains(needle))
    else {
        return Err(format!("{needle:?} is missing or out of order"));
    };
    *start += offset + 1;
    Ok(*start - 1)
}

fn validate_list_layout(lines: &[&str], start: &mut usize) -> Result<(), String> {
    let opened = next_line(lines, start, "lua.ui: places list opened ")?;
    let summary = lines[opened]
        .rsplit_once("places list opened ")
        .map(|(_, rest)| rest)
        .ok_or("the list summary is missing")?;
    let summary: Vec<&str> = summary.split_whitespace().collect();
    if summary.len() != 4 || summary[1] != "items" || summary[3] != "rows" {
        return Err(format!("the list summary is malformed: {summary:?}"));
    }
    let item_count: usize = summary[0]
        .parse()
        .map_err(|_| format!("the list item count is malformed: {summary:?}"))?;
    let visible_rows: usize = summary[2]
        .parse()
        .map_err(|_| format!("the visible row count is malformed: {summary:?}"))?;
    if item_count < 2 || visible_rows == 0 {
        return Err(format!(
            "the list has no place rows or visible rows: {summary:?}"
        ));
    }
    let geometry_line = next_line(lines, start, "lua.ui: places list geometry ")?;
    let geometry = lines[geometry_line]
        .rsplit_once("places list geometry ")
        .map(|(_, rest)| rest)
        .ok_or("the list geometry is missing")?;
    let field = |name: &str| -> Result<usize, String> {
        geometry
            .split_whitespace()
            .find_map(|field| field.strip_prefix(&format!("{name}=")))
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| format!("the list geometry has no integer {name}: {geometry:?}"))
    };
    let list_y = field("y")?;
    let row_height = field("row_height")?;
    let click_y = field("click_y")?;
    if row_height == 0 {
        return Err(format!("the list row height is invalid: {geometry:?}"));
    }
    let scroll_line = next_line(lines, start, "lua.ui: places list s=")?;
    let scroll: usize = lines[scroll_line]
        .rsplit_once("places list s=")
        .and_then(|(_, rest)| rest.trim().parse().ok())
        .ok_or("the scroll is not an integer")?;
    if scroll == 0 || !scroll.is_multiple_of(2) {
        return Err(format!(
            "the list scrolled by {scroll}, not a positive even whole number of UI pixels"
        ));
    }
    let click_line = next_line(lines, start, "lua.ui: places list click row ")?;
    let click = lines[click_line]
        .rsplit_once("places list click row ")
        .map(|(_, rest)| rest)
        .ok_or("the selected list row is missing")?;
    let click: Vec<&str> = click.split_whitespace().collect();
    if click.len() != 3 || click[1] != "visible" {
        return Err(format!("the selected list row is malformed: {click:?}"));
    }
    let list_row: usize = click[0]
        .parse()
        .map_err(|_| format!("the selected list row is malformed: {click:?}"))?;
    let visible_row: usize = click[2]
        .parse()
        .map_err(|_| format!("the visible list row is malformed: {click:?}"))?;
    let Some(click_offset) = click_y.checked_sub(list_y) else {
        return Err(format!("the click is above the list: {geometry:?}"));
    };
    let expected_visible = click_offset / row_height + 1;
    let Some(scrolled_offset) = click_offset.checked_add(scroll) else {
        return Err("the scrolled click coordinate overflowed".to_owned());
    };
    let expected_list = scrolled_offset / row_height + 1;
    if expected_visible > visible_rows
        || visible_row != expected_visible
        || expected_list < 2
        || expected_list > item_count
        || list_row != expected_list
    {
        return Err(format!(
            "the selected row {list_row} (visible {visible_row}) does not match the observed layout: click_y={click_y}, list_y={list_y}, row_height={row_height}, scroll={scroll}, rows={visible_rows}, items={item_count}"
        ));
    }
    Ok(())
}
