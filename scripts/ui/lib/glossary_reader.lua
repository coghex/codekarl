-- The glossary reader (CK-300; docs/ui-design.md; look: mockups/glossary-reader-r1.html with the scroll bar of glossary-reader-r2).
--
-- The pause menu's "glossary" button opens it, above the menu and its backdrop as the settings window is (a modal window, lib/window.lua):
-- one library window of kind `window`, titled "Glossary", 400 by 300 UI pixels, centred in the view and centred again when the view
-- changes, not draggable, with its close "x". Its "x" and Escape (with no tooltip to close first) close it, back to the menu.
--
-- **The left column** is a scroll list (lib/list.lua, with its scroll bar) of every entry's title, sorted by `string.lower(title)`:
-- 13-pixel rows, the title 3 UI pixels in, as wide as the widest title plus 12 and the bar's room. The chosen row is filled with the kind's
-- `hot` (the list's `select`, its fill 2 UI pixels short of the row's right edge as drawn). The titles have no tooltip. A line in the
-- kind's `outline` divides the columns.
--
-- **The right column** is empty at opening. Choosing an entry (a click on its title, or on a linked word) shows, 8 UI pixels right of the
-- line: the title as a heading in the tooltip kind's `title_text` (gold), and 18 UI pixels below it the text in the panel font, wrapped at
-- the column's width in 13-pixel lines. Plain words are `text`, linked words `term`. A link reads as the tooltip reads it: `{key}` shows
-- the linked entry's title, `{key|words}` the words, `{{` and `}}` are braces.
--
-- **A linked word** has the tooltip of its term on hover (`tooltip.explanation`, as a panel's term has) and, clicked, opens its entry
-- here: it becomes the chosen entry and the list scrolls so that its row is in view. Each link has one hit region (a rectangle per line
-- it wraps over) under a key from `ui.key`; no key is built from a title or a glossary key, which are data.
--
-- The reader remembers nothing: it opens empty with the list at the top.
local ui = require("lib.ui")
local text = require("lib.text")
local theme = require("lib.theme")
local pointer = require("lib.pointer")
local tooltip = require("lib.tooltip")
local windows = require("lib.window")
local list = require("lib.list")
local scrollbar = require("lib.scrollbar")

local P = ui.P
local ROW = list.ROW
local OUTER_WIDTH, OUTER_HEIGHT = 400 * P, 300 * P
local ROWS = 21
local TITLE_X = 3 -- UI pixels from the row's left to its title
local TEXT_GAP = 8 -- UI pixels from the line to the right column
local RIGHT_MARGIN = 8 -- UI pixels from the right column's last pixel to the window's right edge
local HEADING_Y = 1 -- UI pixels down: the heading's top, as a row's text is
local BODY_Y = 18 -- UI pixels from the heading's top to the text's first line
local FILL_INSET = 2 -- UI pixels short of the row's right edge that the chosen row's fill stops
local WHITE = "ui/theme/white.png"

local glossary_reader = {}
local current

-- The entries in the list's order: by `string.lower(title)`, then by key.
local function sorted_entries()
  local entries = {}
  for _, key in ipairs(glossary.keys()) do
    local entry = glossary.entry(key)
    entries[#entries + 1] = {key = key, title = entry.title, text = entry.text, sort = string.lower(entry.title)}
  end
  table.sort(entries, function(a, b)
    if a.sort ~= b.sort then return a.sort < b.sort end
    return a.key < b.key
  end)
  return entries
end

-- An entry's text as pieces `{text, term, space, link}`: each word and each run of spaces, `term` the linked entry's key for a linked word
-- and `link` the number of the link it belongs to. Links read as `lib/tooltip.lua` reads them.
local function pieces_of(value)
  local chunks, index, links = {}, 1, 0
  while index <= #value do
    local open_at = value:find("[{}]", index)
    if open_at == nil then
      chunks[#chunks + 1] = {text = value:sub(index)}
      break
    end
    if open_at > index then chunks[#chunks + 1] = {text = value:sub(index, open_at - 1)} end
    local pair = value:sub(open_at, open_at + 1)
    if pair == "{{" then
      chunks[#chunks + 1] = {text = "{"}
      index = open_at + 2
    elseif pair == "}}" then
      chunks[#chunks + 1] = {text = "}"}
      index = open_at + 2
    elseif value:sub(open_at, open_at) == "}" then
      chunks[#chunks + 1] = {text = "}"}
      index = open_at + 1
    else
      local close_at = value:find("}", open_at + 1, true)
      if close_at == nil then
        chunks[#chunks + 1] = {text = value:sub(open_at)}
        break
      end
      local term, words = value:sub(open_at + 1, close_at - 1):match("^([^|]+)|?(.*)$")
      local entry = term ~= nil and glossary.entry(term) or nil
      links = links + 1
      chunks[#chunks + 1] = {text = words ~= "" and words or (entry ~= nil and entry.title or term or ""), term = entry ~= nil and term or nil,
        link = links}
      index = close_at + 1
    end
  end
  local pieces = {}
  for _, chunk in ipairs(chunks) do
    local cursor = 1
    while cursor <= #chunk.text do
      local split = chunk.text:find("%s+", cursor)
      if split == nil then
        pieces[#pieces + 1] = {text = chunk.text:sub(cursor), term = chunk.term, link = chunk.link}
        break
      end
      if split > cursor then pieces[#pieces + 1] = {text = chunk.text:sub(cursor, split - 1), term = chunk.term, link = chunk.link} end
      local finish = chunk.text:find("%S", split) or (#chunk.text + 1)
      pieces[#pieces + 1] = {text = chunk.text:sub(split, finish - 1), term = chunk.term, link = chunk.link, space = true}
      cursor = finish
    end
  end
  return pieces
end

-- The pieces wrapped at `room` UI pixels in the panel font: a sequence of lines, each a sequence of pieces with their `x` and `width`.
-- A space that would start a line is dropped, and so is a space that ends one.
local function wrap(pieces, room)
  local lines, line, used = {}, {}, 0
  local function flush()
    while #line > 0 and line[#line].space do line[#line] = nil end
    lines[#lines + 1] = line
    line, used = {}, 0
  end
  for _, piece in ipairs(pieces) do
    local width = font.layout(piece.text).width
    if used > 0 and used + width > room and not piece.space then flush() end
    if not (used == 0 and piece.space) then
      line[#line + 1] = {text = piece.text, term = piece.term, link = piece.link, space = piece.space, x = used, width = width}
      used = used + width
    end
  end
  if #line > 0 then flush() end
  return lines
end

-- The page of the right column: a group attached to the window, with its text lines and one hit region per link.
local function new_page(state)
  local page = {group = render.create_group(), owner = {}, keys = {}, lines = {}}
  page.item = {
    groups = {page.group},
    layout = function(_, ox, oy, clip)
      render.set_group_offset(page.group, ox + state.right_x, oy)
      render.set_group_clip(page.group, {
        x = clip.x + state.right_x, y = clip.y, width = math.max(P, clip.width - state.right_x), height = clip.height,
      })
    end,
    destroy = function()
      if page.destroyed then return end
      page.destroyed = true
      for _, key in ipairs(page.keys) do
        tooltip.detach(key)
        render.set_hit(key, nil, page.group)
      end
      pointer.unregister_owner(page.owner)
      render.destroy_group(page.group)
    end,
  }
  state.page = page
  state.window:attach(page.item)
  return page
end

local function clear_page(state)
  local page = state.page
  if page ~= nil then
    state.page = nil
    if not page.destroyed then
      state.window:detach(page.item)
      page.item.destroy()
    end
  end
end

-- One link's pointer widget: a press and release on it (a click) opens the linked entry here; the rebuild waits for the dispatch to end,
-- since it retires this widget.
local function link_widget(state, key, term)
  local pressed = false
  local function release()
    if pressed then
      pressed = false
      pointer.pressed(key, false)
    end
  end
  return {
    on_event = function(_, event, eligible)
      if event.kind == "down" then
        if not pressed then
          pressed = true
          pointer.pressed(key, true)
        end
      elseif event.kind == "up" then
        if pressed then
          release()
          if eligible ~= false and event.inside == true then
            pointer.defer(function()
              if current == state and not state.window.closed then state:choose(term, true) end
            end)
          end
        end
      elseif event.kind == "cancel" then
        release()
      end
    end,
    on_hover = function() end,
    on_overflow = release,
    functional = function() return true end,
  }
end

-- Draws the entry in the right column.
local function show_entry(state, entry)
  clear_page(state)
  local page = new_page(state)
  local colours = state.window.colours
  local heading = text.line(0, HEADING_Y * P, page.group, theme.kind("tooltip").title_text)
  text.set(heading, entry.title)
  local plain, linked = colours.text, theme.colour("term")
  local lines = wrap(pieces_of(entry.text), state.room)
  local rects = {} -- link number -> {term, rects}
  for number, line in ipairs(lines) do
    local top = HEADING_Y + BODY_Y + (number - 1) * 13
    -- Neighbouring pieces of one colour and one link are drawn as one run of text.
    local run
    local function finish()
      if run == nil then return end
      local drawn = text.line(run.x * P, top * P, page.group, run.term ~= nil and linked or plain)
      text.set(drawn, run.text)
      page.lines[#page.lines + 1] = drawn
      if run.term ~= nil then
        local kept = rects[run.link]
        if kept == nil then
          kept = {term = run.term, rects = {}}
          rects[run.link] = kept
        end
        kept.rects[#kept.rects + 1] = {x = run.x * P, y = (top - 1) * P, width = run.width * P, height = ROW}
      end
      run = nil
    end
    for _, piece in ipairs(line) do
      local same = run ~= nil and run.link == piece.link and run.term == piece.term
      if not same then finish() end
      if run == nil then
        run = {x = piece.x, text = piece.text, term = piece.term, link = piece.link, width = piece.width}
      else
        run.text = run.text .. piece.text
        run.width = run.width + piece.width
      end
    end
    finish()
  end
  local numbers = {}
  for number in pairs(rects) do numbers[#numbers + 1] = number end
  table.sort(numbers)
  for _, number in ipairs(numbers) do
    local kept = rects[number]
    local key = ui.key("glossary_link")
    page.keys[#page.keys + 1] = key
    render.set_hit(key, kept.rects, page.group)
    pointer.register(key, link_widget(state, key, kept.term), page.owner)
    local term = kept.term
    tooltip.attach(key, function() return tooltip.explanation(term) end)
  end
end

function glossary_reader.current()
  return current
end

local function centre(state)
  local width, height = state.window:size()
  local x, y = ui.anchor("centre", width, height, 0, 0, state.view.width, state.view.height)
  state.window:move_to(x, y)
end

-- Opens the reader in a view `view.width` by `view.height` (read again at each `relayout`), or raises it when it is open.
function glossary_reader.open(view)
  if current ~= nil and not current.window.closed then
    current.window:raise()
    return current.window
  end
  local entries = sorted_entries()
  local widest = 0
  local items, index_of = {}, {}
  for index, entry in ipairs(entries) do
    widest = math.max(widest, font.layout(entry.title).width)
    items[index] = {cells = {{text = entry.title, x = TITLE_X, align = "left"}}}
    index_of[entry.key] = index
  end
  local side = (widest + 12) * P + scrollbar.RESERVE -- the list column's width: the rows and the bar's room
  local state = {view = view, entries = entries, index_of = index_of}
  state.window = windows.new{
    title = "Glossary", kind = "window", draggable = false, modal = true, outer_width = OUTER_WIDTH, outer_height = OUTER_HEIGHT,
    on_close = function()
      clear_page(state)
      if current == state then current = nil end
    end,
  }
  local content_width, content_height = state.window.content_width, state.window.content_height
  state.right_x = side + TEXT_GAP * P
  -- The text's room in UI pixels: from the right column's left to 8 pixels short of the window's right edge (3 beyond the content).
  state.room = (content_width + 3 * P - RIGHT_MARGIN * P - state.right_x) // P
  state.list = list.new{
    window = state.window, x = 0, y = 0, width = side, rows = ROWS, items = items, selectable = true, selected_inset = FILL_INSET * P,
    on_select = function(index) state:choose(entries[index].key, false) end,
  }
  state.divider = render.create_quad{
    texture = ui.texture(WHITE), x = side, y = 0, width = P, height = content_height, group = state.window.content,
    tint = state.window.colours.outline,
  }
  function state:choose(key, scroll)
    local index = self.index_of[key]
    if index == nil or self.window.closed then return end
    self.chosen = key
    if scroll then self.list:scroll_to(index) end
    self.list:select(index)
    show_entry(self, self.entries[index])
  end
  function state:relayout(next_view)
    if next_view ~= nil then self.view = next_view end
    if not self.window.closed then centre(self) end
  end
  current = state
  centre(state)
  return state.window
end

function glossary_reader.relayout(view)
  if current ~= nil then current:relayout(view) end
end

function glossary_reader.close()
  if current ~= nil and not current.window.closed then current.window:close() end
end

return glossary_reader
