-- The player's choice of which name of a place shows, `"english"` (the default: a place's English name, else its local one) or `"local"`
-- (CK-423). One live choice shared by Settings and every view that names a place from its id.
--
-- The shown name is read from `game.places()`, which applies the choice in the host, so a view that asks again after a change gets the
-- new name. Views that hold a name already (an open window's title, a row, a tooltip) register with `on_change` and read it again.
-- The choice is a setting of the player's, not part of a game, so changing it marks nothing unsaved and ticks nothing.
local state = {}
local value
local listeners = {}

local function current()
  if value == nil then
    value = settings ~= nil and settings.graphics ~= nil and settings.graphics().place_names or "english"
  end
  return value
end

local function publish(next_value)
  if current() == next_value then return false end
  value = next_value
  for _, listener in ipairs({table.unpack(listeners)}) do listener(value) end
  return true
end

-- The choice now: `"english"` or `"local"`.
function state.get()
  return current()
end

-- Chooses `"english"` or `"local"`, saves it with the settings and tells the listeners. Returns whether it changed.
function state.set(next_value)
  if next_value ~= "english" and next_value ~= "local" then error("place_names.set: expected english or local", 2) end
  if current() == next_value then return false end
  if settings ~= nil and settings.set ~= nil then settings.set("ui.place_names", next_value) end
  return publish(next_value)
end

-- Reads the choice from the settings again (after the settings window reset them) and tells the listeners if it changed.
function state.refresh()
  if settings == nil or settings.graphics == nil then return false end
  local choice = settings.graphics().place_names
  if choice ~= "english" and choice ~= "local" then return false end
  return publish(choice)
end

-- Calls `listener(choice)` after each change of the choice. Returns a function that removes it.
function state.on_change(listener)
  listeners[#listeners + 1] = listener
  return function()
    for index, candidate in ipairs(listeners) do
      if candidate == listener then table.remove(listeners, index); break end
    end
  end
end

-- The place `id` as `game.places()` lists it now (`name` is the shown name), or nil when the game has no such place.
function state.place(id)
  if game == nil or game.places == nil then return nil end
  for _, place in ipairs(game.places()) do
    if place.id == id then return place end
  end
  return nil
end

-- The name shown for the place `id`, or the id itself when there is no such place.
function state.name_of(id)
  local place = state.place(id)
  return place ~= nil and place.name or id
end

return state
