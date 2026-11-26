local directory = assert(os.getenv('CK_WINDOWED_TEST_KEY_FILES'))
local settings_window = require('lib.settings_window')
local typing = require('lib.typing')
local pointer = require('lib.pointer')
local theme = require('lib.theme')
local listbar = require('lib.listbar')
local P = require('lib.ui').P
local calls = {}
local actual_set = settings.set
settings.set = function(name, value)
  if name:match('^ui%.colours%.') then calls[#calls+1] = value end
  return actual_set(name, value)
end
local phase, ready, state, row, choice = 0, false, nil, nil, nil
local function mark(name, value)
  local path = directory .. '/' .. name
  local temporary = path .. '.tmp'
  local f = assert(io.open(temporary, 'w'))
  f:write(value or 'ok'); f:close()
  assert(os.rename(temporary, path))
end
local function field_at()
  local ox, oy = state.window:content_origin()
  return string.format('%g %g', ox + state.page.x + row.field.x + row.field.width/2, oy + row.field.y + row.field.height/2)
end
local function opener_at()
  local ox, oy = state.window:content_origin()
  return string.format('%g %g', ox + state.page.x + row.opener.x + row.opener.width/2, oy + row.opener.y + row.opener.height/2)
end
local function defaults_at()
  local ox, oy = state.window:content_origin()
  local button = state.controls.defaults
  return string.format('%g %g', ox + state.page.x + button.x + button.width/2, oy + button.y + button.height/2)
end
local function typed() return table.concat(row.field.chars) end
function startup()
  window.on_opened(function() wait_for(HOLD); ready = true end)
  frame.on_update(function(update)
    if not ready then return end
    typing.dispatch(update)
    pointer.dispatch(update.pointer)
    theme.refresh()
    settings_window.update(update)
    listbar.update(update)
    if phase == 0 then
      settings_window.open({width=640,height=400})
      state = settings_window.current(); state:show('colours'); row = state.controls.colours.general
      phase = 1
    elseif phase == 1 then
      mark('field-ready', field_at()); phase = 2
    elseif phase == 2 and row.field:editing() then
      phase = 3
    elseif phase == 3 then
      mark('field-focused'); phase = 4
    elseif phase == 4 and typed() == '#111111' then
      mark('draft-ready', opener_at()); phase = 5
    elseif phase == 5 and row.opener.pressed then
      assert(not row.field:editing() and #calls == 0)
      mark('opener-down'); phase = 6
    elseif phase == 6 and row.picker.picker ~= nil then
      local picker = row.picker.picker
      picker:set_pos(100000)
      for _, item in ipairs(picker.rows) do if item.entry.value == '#ece3cb' then choice = item end end
      assert(choice ~= nil); phase = 7
    elseif phase == 7 then
      local picker = row.picker.picker
      local px, py = picker:geometry()
      local x = picker.origin_x + px + 5 * P
      local y = picker.origin_y + py + 3 * P + choice.y - picker.s + 6 * P
      mark('palette-ready', string.format('%g %g', x, y)); phase = 8
    elseif phase == 8 and row.picker.picker.pressed_row ~= nil then
      assert(#calls == 0); mark('palette-down'); phase = 9
    elseif phase == 9 and #calls == 1 then
      assert(calls[1] == '#ece3cb' and row.field:get() == '#ece3cb')
      mark('picked', field_at()); phase = 10
    elseif phase == 10 and row.field:editing() then phase = 11
    elseif phase == 11 then mark('second-focused'); phase = 12
    elseif phase == 12 and #calls == 2 then
      assert(calls[2] == '#abcdef' and row.field:get() == '#abcdef')
      mark('mixed-applied', field_at()); phase = 13
    elseif phase == 13 and row.field:editing() then phase = 14
    elseif phase == 14 then mark('third-focused'); phase = 15
    elseif phase == 15 and not row.field:editing() then
      assert(row.field:get() == '#abcdef' and #calls == 2)
      mark('invalid-refused', field_at()); phase = 16
    elseif phase == 16 and row.field:editing() then phase = 17
    elseif phase == 17 then mark('fourth-focused'); phase = 18
    elseif phase == 18 and typed() == '#111111' then mark('map-draft'); phase = 19
    elseif phase == 19 and #calls == 3 then
      assert(calls[3] == '#111111' and row.field:get() == '#111111')
      for _, name in ipairs({'location', 'social', 'expedition'}) do settings.set('ui.colours.' .. name, '#123456') end
      mark('map-applied', defaults_at()); phase = 20
    elseif phase == 20 and state.controls.colours.general.field:get() == '#6e130f' then
      assert(theme.bases().general == '#6e130f' and theme.bases().location == '#4fb3c8'
        and theme.bases().social == '#6e130f' and theme.bases().expedition == '#c9932e')
      mark('defaults-applied'); log.info('native colour interactions complete'); window.close(); phase = 21
    end
  end)
  window.open{title='colour settings', width=640, height=400, clear={0,0,0}}
end
