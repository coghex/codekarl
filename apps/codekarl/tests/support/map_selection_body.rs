/// The shared map-selection UI body used by both window backends and the real-VM fixture test.
pub(crate) fn body() -> &'static str {
    r"
local windows = require('lib.window')
local label = require('lib.label')
local button = require('lib.button')
local pointer = require('lib.pointer')
local directory = assert(
  os.getenv('CK_WINDOWED_TEST_MAP_SELECT') or os.getenv('CK_VULKAN_TEST_MAP_SELECT'),
  'map selection marker directory is not configured'
)
local function touch(path)
  local file = assert(io.open(path, 'w'))
  file:write('x')
  file:close()
end
local function exists(path)
  local file = io.open(path, 'r')
  if file == nil then return false end
  file:close()
  return true
end
local place, help = nil, nil
local selection_count = 0
local frame_ready = false
local scroll_frame_observed = false
local place_closed_pending, help_closed_pending = false, false
game.on_select(function(value)
  selection_count = selection_count + 1
  log.info('game_select ' .. value.id)
  if place == nil then
    place = windows.new{title = value.name, content_width = 120, content_height = 48, x = 8, y = 60,
      on_close = function() log.info('place window closed'); place_closed_pending = true end}
    label.new{x = 0, y = 0, text = 'id: ' .. value.id, group = place.content, owner = place}
    label.new{x = 0, y = 22, text = 'kind: town', group = place.content, owner = place}
    label.new{x = 0, y = 44, text = 'size: 1', group = place.content, owner = place}
    log.info('place window opened')
    touch(directory .. '/place-ready')
  else
    place:set_title(value.name)
    place:raise()
  end
end)
button.new{x = 10, y = 10, text = '?', on_click = function()
  if help ~= nil then help:raise(); return end
  help = windows.new{title = 'Help', content_width = 120, content_height = 60, x = 240, y = 40,
    on_close = function() log.info('help window closed'); help_closed_pending = true end}
  label.new{x = 0, y = 0, text = 'drag: pan', group = help.content, owner = help}
  log.info('help window opened')
  touch(directory .. '/help-ready')
end}
window.on_close_request(function(source)
  log.info('on_close_request source=' .. source .. ' window_on_top=' .. tostring(windows.top() ~= nil))
  if source == 'key' and windows.top() ~= nil then return windows.close_request(source) end
  if source == 'key' then log.info('window: close requested'); touch(directory .. '/close-requested') end
  return true
end)
frame.on_update(function(f)
  if place_closed_pending then
    touch(directory .. '/place-closed')
    place_closed_pending = false
  end
  if help_closed_pending then
    touch(directory .. '/help-closed')
    help_closed_pending = false
  end
  if not frame_ready then
    touch(directory .. '/frame-ready')
    frame_ready = true
  end
  if not scroll_frame_observed and exists(directory .. '/scroll-injected') then
    touch(directory .. '/scroll-frame-observed')
    scroll_frame_observed = true
  end
  pointer.dispatch(f.pointer)
  if f.pointer ~= nil and #(f.pointer.events or {}) > 0 then
    touch(directory .. '/fixture-frame-pointer-events')
  end
  if place ~= nil and exists(directory .. '/map-marker-drag-injected') and not exists(directory .. '/drag-verified') then
    assert(selection_count == 1 and windows.top() == place, 'dragging a marker opens no additional place window')
    log.info('map marker drag opened nothing')
    touch(directory .. '/drag-verified')
  end
  if place ~= nil and help ~= nil and exists(directory .. '/place-click-injected') and windows.top() == place and not exists(directory .. '/place-raised') then
    log.info('place window raised')
    touch(directory .. '/place-raised')
  end
  if place ~= nil and exists(directory .. '/title-drag-injected') and not exists(directory .. '/title-moved') then
    local x, y = place:position()
    assert(x == 38 and y == 80, 'title drag moves the place window by its pointer delta')
    log.info('place window moved')
    touch(directory .. '/title-moved')
  end
end)
window.open{title = 'map selection', width = 400, height = 240, clear = {0, 0, 0}}
"
}
