-- The save / load screen (CK-305; docs/ui-design.md §3.22; look: mockups/save-load-r1.html and -r2.html).
--
-- The pause menu's "save / load" button opens it, above the menu and its backdrop as the settings window and the glossary reader are (a
-- modal window, lib/window.lua): one library window of kind `window`, titled "Save / Load", 400 by 300 UI pixels, centred in the view and
-- centred again when the view changes, not draggable, with its close "x". Its "x" and Escape (with nothing above it) close it, back to the
-- menu.
--
-- From the top of its content: the name field (lib/textfield.lua, up to 48 characters) holding a generated name, a "new save" button, a line,
-- the saves as cards in a card stack (lib/cards.lua, with its scroll bar), and the list bar (lib/listbar.lua) in the bottom 19 pixels.
--
-- **The saves** are read with `game.saves()` when the screen opens and after every save and delete, and kept as they were read. A card shows
-- the save's name and in-game date, and the time it was written in the player's local time. A click chooses a card (one at a time); its
-- outline turns `hot` and its buttons appear: "load", "save" and "delete", or only "delete" for a save that cannot load (greyed).
--
-- **Asking first.** "save" on a card, and "new save" over a save of the same name, ask "Overwrite this save?"; "delete" asks "Delete this
-- save?"; "load" asks "Load this save?" only when the game answers a guarded load with `unsaved`. The screen keeps no copy of the unsaved
-- flag: a press of "load" always sends `game.load_save_guarded` and waits for the game's answer (`game.on_save_result`). From that press
-- until the answer nothing on the screen responds: the click that sends the load sets a pointer gate (lib/pointer.lua) at once, so a later
-- click of the same update (on the name field, on "x") reaches nothing, a hit region over the whole view takes the pointer after it, no field
-- is left editing, and Escape does nothing (the window's `keep_open`). The other buttons hold later events of their update the same way until
-- their action has run (a few lines below, `pressed`).
--
-- **The date** of the generated name is the latest `game_time` main.lua hands over (`save_screen.set_time`). **The scenario's id** comes from
-- `game.scenario()` (CK-320), called in one place (`scenario_id`).
local ui = require("lib.ui")
local text = require("lib.text")
local theme = require("lib.theme")
local frame = require("lib.frame")
local button = require("lib.button")
local pointer = require("lib.pointer")
local fieldedit = require("lib.fieldedit")
local windows = require("lib.window")
local textfield = require("lib.textfield")
local cards = require("lib.cards")
local listbar = require("lib.listbar")

local P = ui.P
local OUTER_WIDTH, OUTER_HEIGHT = 400 * P, 300 * P
local GREATEST = 48 -- the most characters a name may have (saves rule 1)
local FIELD_HEIGHT = 15 -- UI pixels
local FIELD_Y = 12 -- UI pixels: leave a line for the saved status above the name field
local FIELD_GAP = 4 -- UI pixels between the name field and "new save"
local REFUSAL_Y = 29 -- UI pixels: the refusal line's top, under the field
local REFUSAL_SHIFT = 13 -- UI pixels the line and the cards move down while the refusal line shows
local LINE_Y = 29 -- UI pixels: the line under the field
local CARDS_Y = 32 -- UI pixels: the first card's top
local CARD_HEIGHT = 28 -- UI pixels: a card's two lines
local CHOSEN_HEIGHT = 47 -- UI pixels: and its buttons
local CARD_GAP = 3
local TEXT_X = 3 -- UI pixels from a card's left to its text
local RIGHT_X = 4 -- UI pixels from a card's right to its time
local LINE_2 = 15 -- UI pixels from a card's top to its second line
local BUTTONS_Y = 29 -- UI pixels from a card's top to its buttons
local BUTTONS_X = 4
local BUTTON_GAP = 3
local VERSION_THIS, VERSION_OTHER = "this version", "other versions"
local MONTHS = {"Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"}
local MONTH_NUMBER = {}
for number, name in ipairs(MONTHS) do MONTH_NUMBER[name] = number end
local REASONS = {
  version = "made by another version: cannot load",
  content = "made with other game data: cannot load",
  unreadable = "cannot be read",
}
local WHITE = "ui/theme/white.png"

local save_screen = {}
local current
local latest_time -- {year, month, day} of the last `game_time`
local latest_unsaved = false

local function set_status(state, unsaved)
  local label = unsaved and "unsaved" or "saved"
  local colour = theme.colour(unsaved and "dim" or "saved")
  text.set(state.status_line, label)
  text.set_colour(state.status_line, colour)
end

-- ---------------------------------------------------------------------------------------------------------------------------------
-- Names, dates and times
-- ---------------------------------------------------------------------------------------------------------------------------------

-- The one place that asks the game for the scenario's id (CK-320).
local function scenario_id()
  return game.scenario()
end

local function date_text(time)
  return string.format("%d %s %d", time.day, MONTHS[time.month], time.year)
end

-- The generated name (rule 2): the scenario's id with `_` as spaces, a comma and the in-game date as the game shows it. It is never longer
-- than 48 characters: the date is kept whole and the scenario's part is cut to what is left (less 2 for the comma and its space) and loses
-- the spaces at its end. With no date known yet it is the scenario's part alone.
function save_screen.generated_name(id, time)
  local words = string.gsub(id, "_", " ")
  if time == nil then
    return (string.gsub(string.sub(words, 1, GREATEST), " +$", ""))
  end
  local date = date_text(time)
  local room = GREATEST - 2 - #date
  return string.gsub(string.sub(words, 1, room), " +$", "") .. ", " .. date
end

local function generated()
  return save_screen.generated_name(scenario_id(), latest_time)
end

-- Days from 1970-01-01 to a civil date.
local function days_from_civil(year, month, day)
  if month <= 2 then year = year - 1 end
  local era = (year >= 0 and year or year - 399) // 400
  local year_of_era = year - era * 400
  local day_of_year = (153 * (month + (month > 2 and -3 or 9)) + 2) // 5 + day - 1
  local day_of_era = year_of_era * 365 + year_of_era // 4 - year_of_era // 100 + day_of_year
  return era * 146097 + day_of_era - 719468
end

-- The seconds since the epoch of a UTC RFC 3339 time to the second, or nil.
local function epoch_of(written)
  if type(written) ~= "string" then return nil end
  local y, mo, d, h, mi, s = string.match(written, "^(%d%d%d%d)-(%d%d)-(%d%d)T(%d%d):(%d%d):(%d%d)Z$")
  if y == nil then return nil end
  y, mo, d, h, mi, s = tonumber(y), tonumber(mo), tonumber(d), tonumber(h), tonumber(mi), tonumber(s)
  if mo < 1 or mo > 12 or d < 1 or d > 31 or h > 23 or mi > 59 or s > 60 then return nil end
  return days_from_civil(y, mo, d) * 86400 + h * 3600 + mi * 60 + s
end

-- "today HH:MM", "yesterday HH:MM" or "D Mon YYYY", in the player's local time; nil for no time written.
local function written_text(epoch)
  if epoch == nil then return nil end
  local ok, then_at = pcall(os.date, "*t", epoch)
  if not ok or then_at == nil then return nil end
  local now = os.date("*t", os.time())
  local ago = days_from_civil(now.year, now.month, now.day) - days_from_civil(then_at.year, then_at.month, then_at.day)
  local clock = string.format("%02d:%02d", then_at.hour, then_at.min)
  if ago == 0 then return "today " .. clock end
  if ago == 1 then return "yesterday " .. clock end
  return string.format("%d %s %d", then_at.day, MONTHS[then_at.month], then_at.year)
end

-- A save's in-game date ("3 Mar 771") as a number that orders dates, or nil.
local function date_key(date)
  if type(date) ~= "string" then return nil end
  local day, month, year = string.match(date, "^(%d+) (%a+) (%d+)$")
  if day == nil or MONTH_NUMBER[month] == nil then return nil end
  return (tonumber(year) * 100 + MONTH_NUMBER[month]) * 100 + tonumber(day)
end

local function trimmed(value)
  return (string.gsub(string.gsub(value, "^ +", ""), " +$", ""))
end

-- ---------------------------------------------------------------------------------------------------------------------------------
-- The rows
-- ---------------------------------------------------------------------------------------------------------------------------------

-- The rows of the saves in the default order (newest written first, a save with no time written last, then by file name) for the list
-- bar: `sort` and `filter` as the bar reads them. A build that could not be read (`unknown`) counts as another version.
local function rows_of(entries)
  local rows = {}
  for _, entry in ipairs(entries) do
    local written = epoch_of(entry.written)
    rows[#rows + 1] = {
      entry = entry, written = written,
      sort = {name = entry.name, date = date_key(entry.date), written = written},
      filter = {version = entry.this_build == "yes" and VERSION_THIS or VERSION_OTHER},
    }
  end
  table.sort(rows, function(a, b)
    if (a.written == nil) ~= (b.written == nil) then return b.written == nil end
    if a.written ~= b.written then return a.written > b.written end
    return a.entry.filename < b.entry.filename
  end)
  return rows
end

-- ---------------------------------------------------------------------------------------------------------------------------------
-- A card
-- ---------------------------------------------------------------------------------------------------------------------------------

-- The card of `row`: a stack item. Its view holds the frame, the text lines and, for the chosen card, the buttons.
local function card_item(state, row)
  local entry = row.entry
  local chosen = state.chosen == entry.filename
  local fits = entry.fit == "fits"
  return {
    height = chosen and CHOSEN_HEIGHT or CARD_HEIGHT,
    filename = entry.filename, -- the save the card shows (for a test)
    on_click = function()
      pointer.defer(function()
        if current == state and not state.window.closed and not state:blocked() then state:choose(entry.filename) end
      end)
    end,
    draw = function(group, x, y, width)
      local kind = theme.kind("window")
      local ink = fits and theme.colour("text") or theme.colour("dim")
      local line = chosen and kind.hot or (fits and kind.outline or theme.colour("track"))
      local parts, buttons, by_label = {}, {}, {}
      local height = (chosen and CHOSEN_HEIGHT or CARD_HEIGHT) * P
      parts[#parts + 1] = frame.new{x = x, y = y, width = width, height = height, cut = 3, corners = "tl_br",
        fill = theme.colour("body"), line = line, group = group}
      local function put(left, top, content, colour)
        local drawn = text.line(left, top, group, colour)
        text.set(drawn, content)
        parts[#parts + 1] = drawn
        return drawn
      end
      put(x + TEXT_X * P, y + 2 * P, entry.name, ink)
      local second = fits and (entry.date or "") or (REASONS[entry.fit] or REASONS.unreadable)
      if second ~= "" then put(x + TEXT_X * P, y + LINE_2 * P, second, theme.colour("dim")) end
      local time = written_text(row.written)
      if time ~= nil then
        local drawn = put(0, y + LINE_2 * P, time, theme.colour("dim"))
        text.move(drawn, x + width - RIGHT_X * P - drawn.width * P, y + LINE_2 * P)
      end
      if chosen then
        local at = x + BUTTONS_X * P
        for _, label in ipairs(fits and {"load", "save", "delete"} or {"delete"}) do
          local made = button.new{
            x = at, y = y + BUTTONS_Y * P, text = label, kind = "window", group = group, owner = state.window,
            on_click = function() state:pressed(label, entry) end,
          }
          buttons[#buttons + 1] = made
          by_label[label] = made
          at = at + made:size() + BUTTON_GAP * P
        end
      end
      return {
        frame = parts[1], -- the card's frame (for a test)
        buttons = by_label, -- the chosen card's buttons by label (for a test)
        raise_hits = function() for _, made in ipairs(buttons) do made:raise_hit() end end,
        destroy = function()
          for _, made in ipairs(buttons) do made:destroy() end
          for _, part in ipairs(parts) do
            if part.destroy ~= nil then part:destroy() else text.destroy(part) end
          end
        end,
      }
    end,
  }
end

-- ---------------------------------------------------------------------------------------------------------------------------------
-- Questions
-- ---------------------------------------------------------------------------------------------------------------------------------

local function noop_widget()
  return {on_event = function() end, on_hover = function() end, on_overflow = function() end, functional = function() return false end}
end

local function centre(window, view)
  local width, height = window:size()
  local x, y = ui.anchor("centre", width, height, 0, 0, view.width, view.height)
  window:move_to(x, y)
end

-- A hit region over the whole view in a group of its own: for a backdrop, or the shield while a load is unanswered.
local function cover(view, prefix, colour)
  local made = {group = ui.create_group(), key = ui.key(prefix)}
  if colour ~= nil then
    made.quad = render.create_quad{texture = ui.texture(WHITE), x = 0, y = 0, width = view.width, height = view.height, group = made.group,
      tint = colour}
  end
  render.set_hit(made.key, {{x = 0, y = 0, width = view.width, height = view.height}}, made.group)
  pointer.register(made.key, noop_widget())
  return made
end

local function uncover(made)
  pointer.unregister(made.key)
  render.set_hit(made.key, nil, made.group)
  render.destroy_group(made.group)
end

local function fit_cover(made, view)
  if made.quad ~= nil then render.set_rect(made.quad, 0, 0, view.width, view.height) end
  render.set_hit(made.key, {{x = 0, y = 0, width = view.width, height = view.height}}, made.group)
end

-- Opens one small fixed window of kind `window` above the screen: `lines` in `text`, then `action` and "cancel" at the bottom right. Escape
-- and "cancel" close it and send nothing; `action` closes it and runs `run`.
local function ask(state, title, lines, action, run)
  local backdrop = cover(state.view, "save_ask_backdrop", theme.colour("body"):sub(1, 7) .. "80")
  local line_width, buttons_width = 0, 0
  for _, line in ipairs(lines) do line_width = math.max(line_width, font.layout(line).width * P) end
  for index, label in ipairs({action, "cancel"}) do
    buttons_width = buttons_width + button.measure(label) + (index > 1 and BUTTON_GAP * P or 0)
  end
  local width = math.max(line_width + 10 * P, buttons_width + 8 * P)
  local _, button_height = button.measure("cancel")
  local height = (#lines * 15 + 7) * P + button_height
  local question = {backdrop = backdrop}
  question.window = windows.new{
    title = title, kind = "window", fixed = true, draggable = false, modal = true, content_width = width, content_height = height,
    on_close = function()
      windows.clear_modal_overlay(question.window, backdrop.group)
      uncover(backdrop)
      if state.question == question then state.question = nil end
    end,
  }
  windows.set_modal_overlay(question.window, backdrop.group)
  for index, line in ipairs(lines) do
    local drawn = text.line(0, (index - 1) * 15 * P + P, question.window.content, theme.colour("text"))
    text.set(drawn, line)
  end
  local at = width - buttons_width
  question.buttons = {} -- by label (for a test)
  for index, label in ipairs({action, "cancel"}) do
    local made = button.new{
      x = at, y = height - button_height, text = label, kind = "window", group = question.window.content, owner = question.window,
      on_click = function()
        if index == 1 then
          question.window:close()
          run()
        else
          question.window:close()
        end
      end,
    }
    question.buttons[label] = made
    at = at + made:size() + BUTTON_GAP * P
  end
  state.question = question
  centre(question.window, state.view)
  return question
end

-- ---------------------------------------------------------------------------------------------------------------------------------
-- The screen
-- ---------------------------------------------------------------------------------------------------------------------------------

-- Lays the refusal line, the line under it and the card stack out for whether a refusal shows.
local function lay_out(state)
  local shift = state.refusal ~= nil and REFUSAL_SHIFT or 0
  local content_width, content_height = state.window.content_width, state.window.content_height
  render.set_rect(state.rule, 0, (LINE_Y + shift) * P, content_width, P)
  local top = (CARDS_Y + shift) * P
  local height = math.max(P, content_height - top - listbar.HEIGHT)
  local scroll = 0
  if state.cards ~= nil then
    if state.cards_top == top then return end
    scroll = state.cards:scroll_position()
    state.cards:destroy()
  end
  state.cards = cards.new{window = state.window, x = 0, y = top, width = content_width, height = height, gap = CARD_GAP, items = {}}
  state.cards_top = top
  state.scroll_wanted = scroll
end

local function show_refusal(state, message)
  state.refusal = message
  if message == nil then
    if state.refusal_line ~= nil then text.set(state.refusal_line, "") end
  else
    text.set(state.refusal_line, message)
  end
  lay_out(state)
  state:refresh(false)
end

local function new_state(view)
  local state = {view = view, entries = {}, rows = {}}
  state.window = windows.new{
    title = "Save / Load", kind = "window", draggable = false, modal = true, outer_width = OUTER_WIDTH, outer_height = OUTER_HEIGHT,
    keep_open = function() return state.busy ~= nil end,
    on_close = function() state:closed() end,
  }
  return state
end

function save_screen.current()
  return current
end

-- The pointer gate (lib/pointer.lua): from the click on one of the screen's buttons, every later keyed event of the same update is
-- swallowed until the button's action has run (`held`) or, for a load, until its answer (`busy`). A press of a button can be followed in one
-- update by a press on the name field or on "x"; those events have their keys already and would reach their widgets before a deferred
-- action ran. Nothing is left editing: a field that edits has its old text put back when the gate closes in.
local function close_in(state)
  if state.gated then return end
  state.gated = true
  pointer.set_gate(function() return true end)
  fieldedit.cancel()
end

local function open_out(state)
  if not state.gated then return end
  state.gated = false
  pointer.set_gate(nil)
end

-- True while something takes the screen's presses: a question is open, a button's action is waiting, or a load is unanswered.
local function blocked(state)
  return state.question ~= nil or state.busy ~= nil or state.held == true
end

local function build(state)
  local window = state.window
  local content_width = window.content_width
  local new_width = button.measure("new save")
  state.status_line = text.line(0, 0, window.content, theme.colour("saved"))
  set_status(state, latest_unsaved)
  state.field = textfield.new{
    x = 0, y = FIELD_Y * P, width = content_width - new_width - FIELD_GAP * P, value = generated(), greatest = GREATEST, group = window.content,
    owner = window,
    on_apply = function(value)
      if trimmed(value) == "" then state.field:set(generated()) end
    end,
  }
  state.new_button = button.new{
    x = content_width - new_width, y = FIELD_Y * P, text = "new save", kind = "window", group = window.content, owner = window,
    on_click = function() state:pressed("new save") end,
  }
  state.refusal_line = text.line(0, REFUSAL_Y * P, window.content, theme.colour("over_limit"))
  state.rule = render.create_quad{texture = ui.texture(WHITE), x = 0, y = LINE_Y * P, width = content_width, height = P,
    group = window.content, tint = window.colours.outline}
  lay_out(state)
  state.bar = listbar.new{
    window = window, kind = "window",
    sorts = {
      {key = "name", label = "name", first = "ascending"},
      {key = "date", label = "in-game date", first = "descending"},
      {key = "written", label = "written", first = "descending"},
    },
    groups = {{key = "version", label = "version"}},
    ticked = {{group = "version", value = VERSION_THIS}},
    on_change = function() state:refresh(false) end,
  }
end

function save_screen.open(view)
  if current ~= nil and not current.window.closed then
    current.window:raise()
    return current.window
  end
  local state = new_state(view)
  current = state
  state.blocked = blocked
  function state:closed()
    if self.question ~= nil and not self.question.window.closed then self.question.window:close() end
    if self.shield ~= nil then
      uncover(self.shield)
      self.shield = nil
    end
    self.busy, self.held = nil, nil
    open_out(self)
    if game ~= nil and game.on_save_result ~= nil then game.on_save_result(nil) end
    if current == self then current = nil end
  end
  -- Chooses the card of `filename`; the stack is drawn again with its buttons.
  function state:choose(filename)
    if self.chosen == filename then return end
    self.chosen = filename
    self:refresh(false)
  end
  -- Reads the saves again (`relist`) and shows the rows the list bar passes, in its order.
  function state:refresh(relist)
    if self.window.closed then return end
    if relist then
      local answer = game.saves()
      if answer.ok then
        self.entries = answer.entries
      else
        self.entries = {}
        self.refusal = "cannot list the saves: " .. tostring(answer.reason)
        text.set(self.refusal_line, self.refusal)
        lay_out(self)
      end
      self.rows = rows_of(self.entries)
    end
    local shown = self.bar:view(self.rows)
    local items, found = {}, false
    for _, row in ipairs(shown) do
      if row.entry.filename == self.chosen then found = true end
    end
    if not found then self.chosen = nil end
    for index, row in ipairs(shown) do items[index] = card_item(self, row) end
    self.cards:set_items(items)
    if self.scroll_wanted ~= nil then
      self.cards:set_pos(self.scroll_wanted)
      self.scroll_wanted = nil
    end
  end
  -- The click on one of the screen's buttons, at its place in the packet's order. A load starts at once: it is sent, and the wait begins (the
  -- gate and the shield), before any later event of the update. Every other button holds the later events and acts when the packet ends,
  -- as the cards are drawn again then.
  function state:pressed(label, entry)
    if self.window.closed or blocked(self) then return end
    if label == "load" then
      self:act(label, entry)
      return
    end
    self.held = true
    close_in(self)
    pointer.defer(function()
      self.held = nil
      open_out(self)
      if current == self and not self.window.closed then self:act(label, entry) end
    end)
  end
  -- What a press of one of the buttons does.
  function state:act(label, entry)
    if self.window.closed or blocked(self) then return end
    if label ~= "load" and self.refusal ~= nil then show_refusal(self, nil) end
    if label == "new save" then
      local name = trimmed(self.field:get())
      if name == "" then
        name = generated()
        self.field:set(name)
      end
      local match
      for _, candidate in ipairs(self.entries) do
        if candidate.name == name and candidate.fit == "fits" and (match == nil or candidate.filename < match.filename) then
          match = candidate
        end
      end
      if match ~= nil then
        self:ask_overwrite(match)
      else
        self:finish_save(game.save(name), "save")
      end
    elseif label == "save" then
      self:ask_overwrite(entry)
    elseif label == "delete" then
      ask(self, "Delete this save?", {'"' .. entry.name .. '" will be deleted.', "This cannot be undone."}, "delete", function()
        local answer = game.delete_save(entry.filename)
        if answer.ok then
          self.chosen = nil
          self:refresh(true)
        else
          show_refusal(self, "delete failed: " .. tostring(answer.reason))
        end
      end)
    elseif label == "load" then
      self.busy = {filename = entry.filename, entry = entry}
      self.shield = cover(self.view, "save_shield", nil)
      close_in(self)
      game.load_save_guarded(entry.filename)
      -- The refusal line goes with this action, but the cards are drawn again after the click, not under the button that sent it.
      if self.refusal ~= nil then
        pointer.defer(function() if current == self and not self.window.closed and self.refusal ~= nil then show_refusal(self, nil) end end)
      end
    end
  end
  function state:ask_overwrite(entry)
    ask(self, "Overwrite this save?", {'"' .. entry.name .. '" will be replaced', "by the game as it is now."}, "overwrite", function()
      self:finish_save(game.save_over(entry.filename), "save")
    end)
  end
  -- A save's answer: the saves are listed again and the name field gets a fresh generated name, or the screen says what failed.
  function state:finish_save(answer, what)
    if answer.ok then
      self.chosen = nil
      self.field:set(generated())
      self:refresh(true)
    else
      show_refusal(self, what .. " failed: " .. tostring(answer.reason))
    end
  end
  -- A load's answer, guarded (the screen's own press) or plain (a question's "load").
  function state:loaded(answer, guarded)
    if self.window.closed then return end
    local entry = self.busy ~= nil and self.busy.entry or nil
    if self.shield ~= nil then
      uncover(self.shield)
      self.shield = nil
    end
    self.busy = nil
    open_out(self)
    if answer.ok then
      self.window:close()
      require("lib.menu").close()
    elseif guarded and answer.reason == "unsaved" and entry ~= nil then
      ask(self, "Load this save?", {"The game has unsaved changes.", "They will be discarded."}, "load", function()
        self:loaded(game.load_save(entry.filename), false)
      end)
    else
      show_refusal(self, "load failed: " .. tostring(answer.reason))
    end
  end
  build(state)
  game.on_save_result(function(filename, answer)
    if current == state and state.busy ~= nil and state.busy.filename == filename then state:loaded(answer, true) end
  end)
  state:refresh(true)
  centre(state.window, view)
  return state.window
end

-- Hands over the latest `game_time` (its date is the generated name's).
function save_screen.set_time(time)
  latest_time = {year = time.year, month = time.month, day = time.day}
  latest_unsaved = time.unsaved == true
  if current ~= nil and not current.window.closed then set_status(current, latest_unsaved) end
end

function save_screen.relayout(view)
  local state = current
  if state == nil or state.window.closed then return end
  state.view = view
  centre(state.window, view)
  if state.question ~= nil then
    fit_cover(state.question.backdrop, view)
    centre(state.question.window, view)
  end
  if state.shield ~= nil then fit_cover(state.shield, view) end
end

function save_screen.close()
  if current ~= nil and not current.window.closed then current.window:close() end
end

return save_screen
