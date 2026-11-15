-- The fullscreen key (CK-326): the `window_fullscreen` action (F11 by default, rebindable on the input tab) switches the window mode.
--
-- A press while the window mode is `windowed` asks for `fullscreen`; a press in `fullscreen` or `windowed_fullscreen` asks for `windowed`.
-- The switch is `settings.set("window.mode", ...)`, the call the graphics tab makes, so it is saved the same way and
-- `settings.graphics().mode` reports it at once.
--
-- Like the game's own key actions the press does nothing while the pause menu (and the settings window it opens) is up, while a widget is
-- editing text, or while a key capture is waiting for its key, and not when the update that carries it ends that state (see `update`).
local menu = require("lib.menu")
local typing = require("lib.typing")

local fullscreen_key = {}

-- The mode a press asks for from `mode`.
function fullscreen_key.next_mode(mode)
  if mode == "windowed" then
    return "fullscreen"
  end
  return "windowed"
end

-- Whether a press would be ignored now.
function fullscreen_key.blocked()
  return menu.is_open() or typing.editing() ~= nil or typing.capturing()
end

-- What the last update left behind: whether it ended blocked, and the counts of the menu's openings and of the keyboard's changes of hands.
local was_blocked = false
local seen_opens = menu.opens()
local seen_changes = typing.changes()

-- Handles one update, after `typing.dispatch(update)` and the pointer dispatch. Those can end the very state that blocked this update's
-- press (an Enter that applies a field, a click on the menu's Return button), and the state can begin between two updates (Escape opening
-- the menu, a callback starting an editing), where no update sees it. So a press is ignored when the state was blocked at the end of the
-- last update, is blocked now, or began since that update ended: a press made under a block never acts over its end, and one that
-- arrives as a block starts is held back as well. A fresh press after an update that left everything unblocked and unchanged acts. A
-- key a capture claims never arrives as an action press at all. Call it on every update, so that the state is seen. Returns the mode
-- asked for, or nil when nothing was.
function fullscreen_key.update(update)
  local blocked = fullscreen_key.blocked()
  local opens, changes = menu.opens(), typing.changes()
  local held_back = blocked or was_blocked or opens ~= seen_opens or changes ~= seen_changes
  was_blocked, seen_opens, seen_changes = blocked, opens, changes
  local action = update.actions ~= nil and update.actions.window_fullscreen or nil
  if action == nil or not action.pressed or held_back then
    return nil
  end
  if settings == nil or settings.graphics == nil or settings.set == nil then
    return nil
  end
  local mode = fullscreen_key.next_mode(settings.graphics().mode)
  settings.set("window.mode", mode)
  return mode
end

return fullscreen_key
