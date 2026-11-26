-- The life log window (CK-378 §2): "life of <name>", a window that opens exactly over the character screen and lists every line of the character's life log,
-- oldest first, one line per moment with its date in the dim text colour before it, wrapped to the width. It scrolls with the shipped scroll bar
-- and opens at the end, where the newest line is. An empty log says "nothing yet" in the dim colour.
--
-- **The data path.** It reads `game.character_log(id, revision)`, the character's lines as of that revision's publication, when it opens and when the
-- screen tells it of a new revision (`refresh`), and never each frame. A new revision keeps the scroll position unless the window was at the end, where
-- it stays at the end. The character screen owns it: a world change, the character's death or another character's screen closes it with the screen,
-- and Escape closes the topmost window, so the log first.
local ui = require("lib.ui")
local windows = require("lib.window")
local cards = require("lib.cards")
local scrollbar = require("lib.scrollbar")
local theme = require("lib.theme")
local draw = require("lib.draw")

local P = ui.P

local character_log = {}
character_log.__index = character_log

local MONTHS = {"Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"}
-- The pitch of a row of text and the gap between two lines, in UI pixels.
local ROW = 10
local GAP = 3

local function date_text(line)
  return string.format("%d %s %d", line.day, MONTHS[line.month], line.year)
end

-- `text` cut into rows: the first `first` UI pixels wide and the rest `width`. A word wider than a row is cut between characters.
local function wrap(text, first, width)
  local rows, row, room = {}, "", first
  local function flush()
    rows[#rows + 1] = row
    row, room = "", width
  end
  for word in text:gmatch("%S+") do
    local joined = row == "" and word or (row .. " " .. word)
    if draw.width(joined, "small") <= room then
      row = joined
    else
      if row ~= "" then flush() end
      while draw.width(word, "small") > room do
        local count = utf8.len(word) or #word
        local cut = count - 1
        while cut > 1 and draw.width(word:sub(1, utf8.offset(word, cut + 1) - 1), "small") > room do cut = cut - 1 end
        row = word:sub(1, utf8.offset(word, cut + 1) - 1)
        word = word:sub(utf8.offset(word, cut + 1))
        flush()
      end
      row = word
    end
  end
  if row ~= "" or #rows == 0 then rows[#rows + 1] = row end
  return rows
end

-- The stack item of one line: its date, then its words wrapped under a hanging indent.
local function item_of(line, inner)
  local date = date_text(line)
  local indent = draw.width(date, "small") + 4
  local rows = wrap(line.text, inner - indent, inner - indent)
  return {
    height = #rows * ROW,
    draw = function(group, _, y, _)
      local result = draw.view()
      local top = y // P
      draw.text(result, group, 0, top + 1, {{text = date, colour = theme.colour("dim")}}, theme.colour("dim"), "small")
      for index, row in ipairs(rows) do
        draw.text(result, group, indent, top + 1 + (index - 1) * ROW, row, theme.colour("text"), "small")
      end
      return result
    end,
  }
end

local function empty_item()
  return {
    height = ROW + 3,
    draw = function(group, _, y, _)
      local result = draw.view()
      draw.text(result, group, 0, y // P + 1, "nothing yet", theme.colour("dim"), "small")
      return result
    end,
  }
end

local function inner_width(self)
  return self.window.content_width // P - scrollbar.RESERVE // P
end

local function items_of(self)
  local items = {}
  local inner = inner_width(self)
  for _, line in ipairs(self.lines) do items[#items + 1] = item_of(line, inner) end
  if #items == 0 then items[1] = empty_item() end
  return items
end

-- `character_log.new{geometry = function() return top, width, height end}`: the log window, closed. `geometry` is the character screen's, whose left slot
-- the window stands inside.
function character_log.new(spec)
  if type(spec) ~= "table" or type(spec.geometry) ~= "function" then
    error("character_log.new: expected `geometry`", 2)
  end
  return setmetatable({geometry = spec.geometry}, character_log)
end

function character_log:is_open()
  return self.window ~= nil and not self.window.closed
end

-- The id of the character whose log shows, or nil.
function character_log:id()
  return self:is_open() and self.character or nil
end

function character_log:close()
  if self:is_open() then self.window:close() end
end

-- The window stands exactly over the character screen: the screen's left slot.
local function frame_of(self)
  local top, width, height = self.geometry()
  return 0, top, width, height
end

-- Opens the window on the lines of `id` read at `revision` in the world `world`, named `name`; shows the newest at the bottom. Returns whether the log
-- was read: a refused read opens nothing.
function character_log:open(id, name, revision, world)
  local ok, lines = pcall(game.character_log, id, revision)
  if not ok then return false end
  if self:is_open() then self:close() end
  self.character, self.world, self.revision, self.lines = id, world, revision, lines
  local x, y, width, height = frame_of(self)
  local created
  created = windows.new{
    title = "life of " .. name, kind = "social", outer_width = width, outer_height = height, x = x, y = y, draggable = false,
    clip_title = true,
    on_close = function()
      if self.window == created then
        self.window, self.stack, self.character, self.lines = nil, nil, nil, nil
      end
    end,
  }
  self.window = created
  self.stack = cards.new{
    window = created, x = 0, y = 0, width = created.content_width, height = created.content_height, gap = GAP, items = items_of(self),
  }
  self.stack:scroll_to_end()
  return true
end

-- A new revision of `world`: the lines are read again, keeping the scroll unless the window was at the end, where it stays at the end. A read the
-- cache refuses changes nothing, and so does the revision already shown.
function character_log:refresh(revision, world)
  if not self:is_open() then return end
  if world ~= self.world then
    self:close()
    return
  end
  if revision == self.revision then return end
  local ok, lines = pcall(game.character_log, self.character, revision)
  if not ok then return end
  self.lines, self.revision = lines, revision
  local at_end, scroll = self.stack:at_end(), self.stack:scroll_position()
  self.stack:set_items(items_of(self))
  if at_end then self.stack:scroll_to_end() else self.stack:set_pos(scroll) end
end

-- The window's size changed: fits it inside the left slot again, keeping the lines and the scroll (at the end it stays at the end).
function character_log:relayout()
  if not self:is_open() then return end
  local at_end, scroll = self.stack:at_end(), self.stack:scroll_position()
  local x, y, width, height = frame_of(self)
  self.window:set_outer_size(width, height)
  self.window:move_to(x, y)
  self.stack:destroy()
  self.stack = cards.new{
    window = self.window, x = 0, y = 0, width = self.window.content_width, height = self.window.content_height, gap = GAP, items = items_of(self),
  }
  if at_end then self.stack:scroll_to_end() else self.stack:set_pos(scroll) end
end

return character_log
