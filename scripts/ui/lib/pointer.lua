-- The pointer dispatcher: gives the events of `frame_update.pointer` to the widgets that registered for their hit keys.
--
-- A widget implements `on_event(event, eligible)`, `on_hover(bool)` and `on_overflow()`. It reports pressed and released with
-- `pointer.pressed(key, bool)`. `eligible` is false for every event of an overflow packet: such a packet clicks nothing.
local pointer = {}

local widgets = {} -- key -> widget
local owners = {} -- key -> optional widget owner
local pressed_keys = {} -- key -> true, the widgets whose gesture is pressed
local last_over = false
local deferred = {} -- functions to run when the next dispatch ends

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

-- The widget registered under `key`, if any.
function pointer.widget(key)
  return widgets[key]
end

-- Whether a press on `key` performs a user action. Widgets without an explicit capability are passive.
function pointer.functional(key, event)
  local widget = widgets[key]
  if widget == nil or widget.functional == nil then return false end
  return widget:functional(key, event) == true
end

-- Runs `fn` when the running dispatch ends, or, when none is running, when the next one ends. A widget that retires a key (its row
-- scrolled out of view) unregisters it this way: the events the UI has not been given yet were routed against the frame the player was
-- pointing at, and the next packet may still name the key, so it still reaches the widget it was meant for. Nothing routed after the
-- retiring edit was published can name it.
function pointer.defer(fn)
  deferred[#deferred + 1] = fn
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
    if key and event.kind == "down" then
      -- A keyed press is classified against the widgets registered at this boundary, even if no widget owns its key.
      event.functional = pointer.functional(key, event)
    end
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
  local pending = deferred
  deferred = {}
  for _, fn in ipairs(pending) do
    fn()
  end
  return changed
end

return pointer
