-- The pause menu (CK-297): Escape, with nothing else left to close, opens a fixed window titled "Menu" over a dimmed view. Opening it
-- pauses the game and turns on the block on the game's own keys; closing it (the "return" button or Escape) turns the block off and
-- leaves the game paused.
--
-- The window is a library window of kind `window`, `fixed` (no close button) and not draggable, centred in the view and centred again
-- when the view's size changes. Under it a backdrop, the colour `body` at alpha one half, covers the whole view above every bar and
-- window; it is a hit region, so a press, a drag, a scroll or a hover there reaches nothing else. "glossary" and "save / load" are
-- greyed (inactive buttons) until their screens exist; "settings" opens the settings window (lib/settings_window.lua).
local ui = require("lib.ui")
local button = require("lib.button")
local windows = require("lib.window")
local theme = require("lib.theme")
local pointer = require("lib.pointer")

local P = ui.P
local GAP = 3 * P -- between buttons
local QUIT_GAP = 6 * P -- more, above "quit to desktop"
local SIDE = 16 -- UI pixels the widest label's button is wider than the label and the button's own padding

local menu = {}

menu.LABELS = {"return", "settings", "glossary", "save / load", "quit to desktop"}
-- The buttons no screen is built for yet.
local GREYED = {glossary = true, ["save / load"] = true}

local current = nil

-- The backdrop's colour: `body` at alpha one half, whatever alpha the theme's own value carries.
local function backdrop_colour()
  return theme.colour("body"):sub(1, 7) .. "80"
end

-- The button width in points: the widest label's text width, the button's padding on both sides (4 UI pixels) and the menu's 16.
local function button_width()
  local widest = 0
  for _, label in ipairs(menu.LABELS) do
    widest = math.max(widest, font.layout(label).width)
  end
  return (widest + 4 + SIDE) * P
end

local function centre(state)
  local width, height = state.window:size()
  local x, y = ui.anchor("centre", width, height, 0, 0, state.view.width, state.view.height)
  state.window:move_to(x, y)
end

local function release_game()
  if game ~= nil then
    game.set_modal(false)
  end
end

-- Takes the backdrop away. Runs when the window closes, however it was closed.
local function dismantle(state)
  require("lib.settings_window").close()
  windows.clear_modal_backdrop(state.backdrop_group)
  pointer.unregister(state.backdrop_key)
  render.set_hit(state.backdrop_key, nil, state.backdrop_group)
  render.destroy_group(state.backdrop_group)
  if current == state then
    current = nil
  end
  release_game()
end

-- A widget that takes every press, drag, scroll and hover over the backdrop and does nothing with them.
local function backdrop_widget()
  return {
    on_event = function() end,
    on_hover = function() end,
    on_overflow = function() end,
  }
end

function menu.is_open()
  return current ~= nil and not current.window.closed
end

-- The open menu's parts for a test: `window`, `buttons` (by label), `backdrop_key` and `backdrop`, the backdrop's quad.
function menu.current()
  return current
end

-- Opens the menu in a view `view.width` by `view.height` (the table is read again at each `relayout`), unless it is open: then it only
-- comes to the front. The game is paused (`game.pause`, which leaves a paused game as it is) and its keys are blocked.
function menu.open(view)
  if menu.is_open() then
    current.window:raise()
    return current.window
  end
  local state = {view = view, buttons = {}}
  state.backdrop_group = ui.create_group()
  windows.set_modal_backdrop(state.backdrop_group)
  state.backdrop = render.create_quad{
    texture = ui.texture("ui/theme/white.png"),
    x = 0,
    y = 0,
    width = view.width,
    height = view.height,
    group = state.backdrop_group,
    tint = backdrop_colour(),
  }
  state.backdrop_key = ui.key("menu_backdrop")
  render.set_hit(state.backdrop_key, {{x = 0, y = 0, width = view.width, height = view.height}}, state.backdrop_group)
  pointer.register(state.backdrop_key, backdrop_widget())

  local width = button_width()
  local height = select(2, button.measure("return"))
  local content_height = #menu.LABELS * height + (#menu.LABELS - 1) * GAP + QUIT_GAP
  -- Created after the backdrop, so it is above it.
  state.window = windows.new{
    title = "Menu",
    kind = "window",
    fixed = true,
    draggable = false,
    modal = true,
    content_width = width,
    content_height = content_height,
    on_close = function() dismantle(state) end,
  }
  local actions = {
    ["return"] = function() state.window:close() end,
    ["settings"] = function() require("lib.settings_window").open(state.view) end,
    ["quit to desktop"] = function() window.close() end,
  }
  local y = 0
  for _, label in ipairs(menu.LABELS) do
    if label == "quit to desktop" then
      y = y + QUIT_GAP
    end
    state.buttons[label] = button.new{
      x = 0,
      y = y,
      width = width,
      text = label,
      kind = "window",
      group = state.window.content,
      owner = state.window,
      inactive = GREYED[label] == true,
      on_click = actions[label],
    }
    y = y + height + GAP
  end
  current = state
  centre(state)
  if game ~= nil then
    game.pause()
    game.set_modal(true)
  end
  return state.window
end

-- The answer to a close request from `source` (`"key"` for Escape, `"button"` for the window's own close button). Escape follows
-- `window.close_request`: it closes the newest lockable tooltip, then the open list-bar picker, then the top window (the menu
-- itself, when it is the top one), one for each press, and declines each time. With nothing left to close it opens the menu and
-- declines too, so the run goes on. The window's close button still accepts: it ends the run, menu or no menu.
function menu.close_request(source, view)
  local accepted = windows.close_request(source)
  if accepted and source == "key" then
    menu.open(view)
    return false
  end
  return accepted
end

-- Closes the menu if it is open; the game stays paused.
function menu.close()
  if menu.is_open() then
    current.window:close()
  end
end

-- The view's size changed: the backdrop covers all of it and the window is centred in it.
function menu.relayout()
  if not menu.is_open() then
    return
  end
  local state = current
  render.set_rect(state.backdrop, 0, 0, state.view.width, state.view.height)
  render.set_hit(state.backdrop_key, {{x = 0, y = 0, width = state.view.width, height = state.view.height}}, state.backdrop_group)
  centre(state)
  require("lib.settings_window").relayout(state.view)
end

return menu
