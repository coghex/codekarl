-- The pointer dispatcher: gives the events of `frame_update.pointer` to the widgets that registered for their hit keys.
--
-- A widget implements `on_event(event, eligible)`, `on_hover(bool)` and `on_overflow()`. It reports pressed and released with
-- `pointer.pressed(key, bool)`. `eligible` is false for every event of an overflow packet: such a packet clicks nothing.
local pointer = {}

local widgets = {} -- key -> widget
local owners = {} -- key -> optional widget owner
local pressed_keys = {} -- key -> true, the widgets whose gesture is pressed
local last_over = false

function pointer.register(key, widget, owner)
  widgets[key] = widget
  owners[key] = owner
  if owner ~= nil then
    owner._pointer_children = owner._pointer_children or {}
    owner._pointer_children[key] = true
  end
end

-- Removes the widget; if it was the hovered one the dispatcher forgets the hover, and it is no longer pressed.
function pointer.unregister(key)
  local owner = owners[key]
  if owner ~= nil and owner._pointer_children ~= nil then
    owner._pointer_children[key] = nil
  end
  widgets[key] = nil
  owners[key] = nil
  pressed_keys[key] = nil
  if last_over == key then
    last_over = false
  end
end

-- Destroys every pointer widget owned by a closing window; unregister removes it from this set.
function pointer.unregister_owner(owner)
  local keys = {}
  for key in pairs(owner._pointer_children or {}) do
    keys[#keys + 1] = key
  end
  table.sort(keys)
  for _, key in ipairs(keys) do
    local widget = widgets[key]
    if widget ~= nil and widget.destroy ~= nil then
      widget:destroy()
    else
      pointer.unregister(key)
    end
  end
end

function pointer.pressed(key, is_pressed)
  pressed_keys[key] = is_pressed and true or nil
end

-- Releases the widgets still pressed, in key order, each told once.
local function release_all()
  local keys = {}
  for key in pairs(pressed_keys) do
    keys[#keys + 1] = key
  end
  table.sort(keys)
  for _, key in ipairs(keys) do
    pressed_keys[key] = nil
    local widget = widgets[key]
    if widget ~= nil then
      widget:on_overflow()
    end
  end
end

-- Handles one `frame_update.pointer` (or nil). Returns true when the packet changed anything a widget could show: it has
-- overflow, it has events, or the hovered key changed.
function pointer.dispatch(p)
  if p == nil then
    return false
  end
  local events = p.events or {}
  local overflow = p.overflow and true or false
  local over = p.over or false
  local changed = overflow or #events > 0 or over ~= last_over

  -- An overflow packet clicks nothing: its events are replayed for the look only.
  local eligible = not overflow
  for _, event in ipairs(events) do
    local key = event.key
    if key then
      local widget = widgets[key]
      if widget ~= nil then
        local owner = owners[key]
        if event.kind == "down" and owner ~= nil and owner.on_child_down ~= nil then
          owner:on_child_down(key)
        end
        widget:on_event(event, eligible)
      end
    end
  end

  -- Hover last: the widget that lost the pointer hears it before the one that gained it.
  if over ~= last_over then
    local previous = last_over
    last_over = over
    if previous then
      local widget = widgets[previous]
      if widget ~= nil then
        widget:on_hover(false)
      end
    end
    if over then
      local widget = widgets[over]
      if widget ~= nil then
        widget:on_hover(true)
      end
    end
  end

  -- After an overflow packet every widget still pressed is released, whether or not the button is held.
  if overflow then
    release_all()
  end
  return changed
end

return pointer
