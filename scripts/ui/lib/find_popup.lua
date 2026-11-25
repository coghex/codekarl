-- The centred find popup (CK-385). Its editor owns the keyboard until the popup closes.
local ui = require("lib.ui")
local windows = require("lib.window")
local pointer = require("lib.pointer")
local textfield = require("lib.textfield")
local fieldedit = require("lib.fieldedit")
local list = require("lib.list")
local label = require("lib.label")
local theme = require("lib.theme")
local draw = require("lib.draw")
local model = require("lib.find_model")
local go_to = require("lib.go_to")
local typing = require("lib.typing")
local menu = require("lib.menu")
local fullscreen_key = require("lib.fullscreen_key")
local P = ui.P
local WIDTH, ROWS = 220 * P, 12
local icons = {location = "location", character = "characters", river = "river"}
local popup = {}
local current
local revision, world = nil, 0
local was_blocked, seen_opens, seen_changes = false, menu.opens(), typing.changes()

local function centre(state)
  local width, height = state.window:size()
  local x, y = ui.anchor("centre", width, height, 0, 0, state.view.width, state.view.height)
  state.window:move_to(x, y)
  render.set_hit(state.backdrop_key, {{x=0, y=0, width=state.view.width, height=state.view.height}}, state.backdrop)
end

local function show_hint(state, message)
  state.message = message
  state.hint:set_text(message)
end

local function selected(state, identity)
  local index = model.index_of(state.rows, identity)
  if index == nil then
    show_hint(state, "no longer there")
    return
  end
  local row = state.rows[index]
  if go_to.to(row.kind, row.id, state.revision) then
    state.window:close()
  else
    show_hint(state, "no longer there")
  end
end

local function refresh(state, changed)
  local query = table.concat(state.field.chars)
  local rows, more = model.search(state.entries, query)
  local old = state.highlight
  state.rows = rows
  state.lost = not changed and (state.lost or (old ~= nil and model.index_of(rows, old) == nil))
  local index = changed and (#rows > 0 and 1 or nil) or model.index_of(rows, old)
  state.highlight = index and rows[index] or nil
  local items = {}
  local row_width = list.row_width(WIDTH, ROWS, #rows) // P
  for i, row in ipairs(rows) do
    local shown = row.label or row.name
    local name = draw.clip(shown, row_width - 20)
    local cells = {{x=16, align="left", text=name}}
    if row.place ~= nil then
      local full = draw.clip(shown .. " · " .. row.place, row_width - 20)
      local prefix = draw.clip(shown, row_width - 20)
      if full:sub(1, #prefix) == prefix then
        cells[2] = {x=16 + font.layout(prefix).width, align="left", text=full:sub(#prefix + 1), colour=theme.colour("dim")}
      end
    end
    local tag = go_to.tag(row.kind, row.id, state.revision, state.world)
    items[i] = {icon={name=icons[row.kind], colour=theme.colour("text")}, cells=cells,
      tag=tag and ("f:" .. tag:sub(4)) or nil}
  end
  state.list:set_items(items)
  state.list:select(index)
  if index then state.list:scroll_to(index) end
  local hint = query:match("^%s*$") and "type to find" or (#rows == 0 and "nothing found" or "")
  if more > 0 then hint = tostring(more) .. " more: keep typing" end
  show_hint(state, hint)
end

function popup.current() return current end
function popup.close() if current then current.window:close() end end
function popup.relayout() if current then centre(current) end end

function popup.set_revision(next_revision, next_world)
  revision, world = next_revision, next_world
  local state = current
  if state == nil then return end
  if next_world ~= state.world then popup.close(); return end
  if next_revision == state.revision then return end
  local ok, entries = pcall(game.find_entries, next_revision)
  if not ok then popup.close(); return end
  state.entries, state.revision = entries, next_revision
  refresh(state, false)
end

-- The place names show differently now (CK-423): the open popup reads its entries again, at the revision it holds, and searches again.
function popup.relabel()
  local state = current
  if state == nil then return end
  local ok, entries = pcall(game.find_entries, state.revision)
  if not ok then popup.close(); return end
  state.entries = entries
  refresh(state, false)
end

function popup.open(view, next_revision, next_world)
  if current or not next_revision or next_revision == 0 then return end
  local ok, entries = pcall(game.find_entries, next_revision)
  if not ok then return end
  local state = {view=view, entries=entries, revision=next_revision, world=next_world, rows={}}
  state.backdrop = ui.create_group()
  state.backdrop_key = ui.key("find_backdrop")
  windows.set_modal_backdrop(state.backdrop)
  pointer.register(state.backdrop_key, {
    on_event=function(_, event, eligible) if eligible ~= false and event.kind == "down" then popup.close() end end,
    on_hover=function() end, on_overflow=function() end,
  })
  state.window = windows.new{title="find...", kind="window", modal=true, draggable=false,
    content_width=WIDTH, content_height=(15 + 3 + ROWS * 13 + 13) * P,
    on_close=function()
      windows.clear_modal_backdrop(state.backdrop)
      pointer.unregister(state.backdrop_key)
      render.set_hit(state.backdrop_key, nil, state.backdrop)
      render.destroy_group(state.backdrop)
      if current == state then current = nil end
      game.set_find_keyboard_block(false)
    end}
  state.field = textfield.new{x=0, y=0, width=WIDTH, greatest=100, group=state.window.content, owner=state.window}
  state.field.keep_editing = true
  state.hint = label.new{x=0, y=(18 + ROWS * 13) * P, text="type to find", group=state.window.content, colour=theme.colour("dim")}
  state.window:attach{groups={}, layout=function() end, destroy=function() state.field:destroy(); state.hint:destroy() end}
  state.list = list.new{window=state.window, x=0, y=18*P, width=WIDTH, rows=ROWS, items={}, tags=true, selectable=true,
    on_select=function()
      local identity = state.pending
      state.pending = nil
      if identity then selected(state, identity) end
    end,
    on_hover_row=function(index)
      if state.rows[index] then state.lost=false; state.highlight=state.rows[index]; state.list:select(index) end
    end}
  local original_typed = state.field.on_typed
  function state.field:on_typed(entry)
    if entry.kind == "enter" then
      if state.highlight then selected(state, state.highlight) elseif state.lost then show_hint(state, "no longer there") end
    elseif entry.kind == "up" or entry.kind == "down" then
      local index = model.index_of(state.rows, state.highlight)
      if #state.rows > 0 then
        if index == nil then index = entry.kind == "down" and 1 or #state.rows
        else index = math.max(1, math.min(#state.rows, index + (entry.kind == "down" and 1 or -1))) end
        state.lost = false
        state.highlight = state.rows[index]
        state.list:select(index); state.list:scroll_to(index)
      end
    elseif entry.kind ~= "tab" and entry.kind ~= "backtab" then
      local before = table.concat(self.chars)
      original_typed(self, entry)
      if table.concat(self.chars) ~= before then refresh(state, true) end
    end
  end
  current = state
  centre(state)
  game.set_find_keyboard_block(true)
  fieldedit.begin(state.field)
end

pointer.observe_tagged(function(event, eligible)
  local state = current
  if not state or not eligible or event.kind ~= "down" or not event.tag:match("^f:") then return end
  local kind, id, _, tagged_world = go_to.parse("go:" .. event.tag:sub(3))
  if kind == nil or tagged_world ~= state.world then return end
  local identity = {kind=kind, id=id}
  if model.index_of(state.rows, identity) == nil then
    state.pending = nil
    show_hint(state, "no longer there")
  else
    state.pending = identity
  end
end)

function popup.update(update, view)
  local blocked = fullscreen_key.blocked()
  local opens, changes = menu.opens(), typing.changes()
  local held_back = blocked or was_blocked or opens ~= seen_opens or changes ~= seen_changes
  was_blocked, seen_opens, seen_changes = blocked, opens, changes
  local action = update.actions and update.actions.game_find
  if action and action.pressed and not held_back then popup.open(view, revision, world) end
end

return popup
