-- One live list-look choice shared by Settings and every open location panel.
local state = {}
local value
local listeners = {}

local function current()
  if value == nil then
    value = settings ~= nil and settings.graphics ~= nil and settings.graphics().list_look or "cards"
  end
  return value
end

local function publish(next_value)
  if current() == next_value then return false end
  value = next_value
  for _, listener in ipairs(listeners) do listener(value) end
  return true
end
function state.get()
  return current()
end

function state.set(next_value)
  if current() == next_value then return false end
  if settings ~= nil and settings.set ~= nil then settings.set("ui.list_look", next_value) end
  if not publish(next_value) then return false end
  if game ~= nil and game.mark_unsaved ~= nil then game.mark_unsaved() end
  return true
end
function state.refresh()
  if settings == nil or settings.graphics == nil then return false end
  return publish(settings.graphics().list_look)
end

function state.subscribe(listener)
  listeners[#listeners + 1] = listener
  listener(current())
  return function()
    for index, candidate in ipairs(listeners) do
      if candidate == listener then table.remove(listeners, index); break end
    end
  end
end

return state
