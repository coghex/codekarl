-- The text field (CK-303; docs/ui-design.md): the number field's frame (a cut-3 frame 15 UI pixels tall, filled `body`, outlined in the kind's
-- `outline`, the outline `hot` and a 1-pixel caret while it edits) as wide as its caller asks, with its text left-aligned, 3 UI pixels from the
-- left edge and 2 down. It holds any text of the project font up to a greatest length, counted in characters.
--
-- **Characters, not bytes.** A character is a Unicode scalar value. The greatest length, the caret's place and what Backspace takes are
-- counted in characters, and the text is kept as a list of characters, so it is valid UTF-8 after every edit: 48 accented vowels fit a field of
-- 48, Left and Right step over one in a move and Backspace takes it whole. A character the project font has no glyph for (it would be drawn
-- as the replacement glyph) is not inserted, and neither is a control character.
--
-- **Editing.** A press on the field puts it into editing (lib/fieldedit.lua: one field edits at a time, a number field or this). The text is
-- kept and the caret starts at its end. A typed character goes in at the caret; Backspace takes away the character before it; Left and Right
-- move the caret one character. A character typed into a full field, Left at the start, Right at the end and Backspace at the start do
-- nothing. Text wider than the field scrolls, a character at a time, so that the caret stays in view; a field not editing shows the start.
--
-- **Typing** comes from the library's typing function (lib/typing.lua) only: its entries reach `on_typed(entry)` in order, a character and
-- the commands `backspace`, `left`, `right`, `enter` and `tab` together.
--
-- **Applying** is Enter, Tab or a left press anywhere that is not a field of this kind (the map included; the press still does what it does):
-- the field shows the text, the editing ends and `on_apply(text)` is called once, when the text is not the one it held. An empty text is the
-- caller's to decide: `on_apply("")` is called and the caller may `set` another text (the save screen puts back its generated name).
--
-- **Escape** while the field edits puts the old text back, ends the editing, tells the caller nothing and goes no further: the window asks
-- `numberfield.cancel()` first (lib/window.lua `close_request`), which is the same editing. **Closing** the window (or destroying the field)
-- while it edits ends the editing and applies nothing. **Key capture** (CK-309) applies and ends the editing first, as it does for the
-- number field.
--
-- `textfield.new{x, y, width, value = "", greatest, on_apply, kind = "window", group = nil, owner = nil}`: points, snapped; the width is the
-- whole field's, at least 16 UI pixels; `greatest` is the greatest length in characters. With a `group` the quads and the hit key go into it
-- and the position is local to it.
local ui = require("lib.ui")
local text = require("lib.text")
local frame = require("lib.frame")
local theme = require("lib.theme")
local pointer = require("lib.pointer")
local fieldedit = require("lib.fieldedit")

local P = ui.P
local HEIGHT = 15 -- UI pixels
local CUT = 3
local INSET = 3 -- UI pixels from the field's left edge to its text, and from its text's room to the right edge
local MIN_WIDTH = 16 -- UI pixels
local TEXT_TOP = 2 -- UI pixels from the field's top to the text's
local CARET_TOP = 2
local CARET_HEIGHT = 11
local WHITE = "ui/theme/white.png"

local textfield = {}
textfield.__index = textfield
textfield.field_edit = true -- a field of lib/fieldedit.lua

-- ---- Characters ----

-- The characters of a string, as a list of strings of one character each; nil when the string is not valid UTF-8.
local function split(value)
  if not utf8.len(value) then
    return nil
  end
  local list = {}
  for _, code in utf8.codes(value) do
    list[#list + 1] = utf8.char(code)
  end
  return list
end

local advances = {} -- a character's advance in font pixels, once measured

local function advance(character)
  local width = advances[character]
  if width == nil then
    width = font.layout(character).width
    advances[character] = width
  end
  return width
end

-- The width in font pixels of the characters `first` to `last` of the list.
local function span(chars, first, last)
  local width = 0
  for index = first, last do
    width = width + advance(chars[index])
  end
  return width
end

local supported = {} -- whether the font draws a character with a glyph of its own

-- A character is one the field takes when it is not a control character and the font has a glyph for it (the space draws nothing: the layout
-- has no entry for it, and the font has it). The replacement glyph is what the layout draws for a character the font does not have.
local function takes(character)
  local answer = supported[character]
  if answer == nil then
    local code = utf8.codepoint(character)
    if code < 0x20 or (code >= 0x7F and code < 0xA0) then
      answer = false
    elseif code == 0x20 then
      answer = true
    else
      local glyphs = font.layout(character).glyphs
      answer = #glyphs == 1 and not glyphs[1].texture:find("replacement", 1, true)
    end
    supported[character] = answer
  end
  return answer
end

-- ---- Drawing ----

-- Font pixels of room for the text, with one for the caret after it.
local function room(self)
  return self.width // P - 2 * INSET - 1
end

-- Brings the first visible character where the caret is in view, and the text fills the room when it can.
local function scroll(self)
  local chars, caret = self.chars, self.position
  local first = self.first
  if first > caret + 1 then
    first = caret + 1
  end
  local limit = room(self)
  while first <= caret and span(chars, first, caret) > limit do
    first = first + 1
  end
  while first > 1 and span(chars, first - 1, #chars) <= limit do
    first = first - 1
  end
  self.first = first
end

-- The characters from `first` that fit the room, as a string.
local function visible(self)
  local limit, width, last = room(self), 0, self.first - 1
  while last < #self.chars do
    width = width + advance(self.chars[last + 1])
    if width > limit then
      break
    end
    last = last + 1
  end
  return table.concat(self.chars, "", self.first, last)
end

local function caret_x(self)
  return self.x + (INSET + span(self.chars, self.first, self.position)) * P
end

local function draw(self)
  text.set(self.line, visible(self))
  text.move(self.line, self.x + INSET * P, self.y + TEXT_TOP * P)
  text.set_colour(self.line, self.colours.text)
  self.frame:set_colours(self.colours.fill, self.is_editing and self.colours.hot or self.colours.outline)
  if self.caret ~= nil then
    render.set_position(self.caret, caret_x(self), self.y + CARET_TOP * P)
  end
end

local function show_caret(self)
  if self.caret == nil then
    self.caret = render.create_quad{
      texture = ui.texture(WHITE),
      x = caret_x(self),
      y = self.y + CARET_TOP * P,
      width = P,
      height = CARET_HEIGHT * P,
      group = self.group,
      tint = self.colours.text,
    }
  end
end

local function hide_caret(self)
  if self.caret ~= nil then
    render.destroy_quad(self.caret)
    self.caret = nil
  end
end

-- ---- The protocol of lib/fieldedit.lua ----

-- Settles what was typed: the field holds and shows it, from its start. Returns the call that tells the caller (nil when the text is the one
-- the field held, or the field has no callback).
function textfield:commit()
  local old = self.value
  local result = table.concat(self.chars)
  self.value = result
  self.first = 1
  self.position = #self.chars
  draw(self)
  if result ~= old and self.apply_callback ~= nil then
    return function() self.apply_callback(result) end
  end
  return nil
end

function textfield:revert()
  self.chars = split(self.value)
  self.first = 1
  self.position = #self.chars
  draw(self)
end

function textfield:show_editing()
  self.position = #self.chars
  self.first = 1
  scroll(self)
  draw(self)
  show_caret(self)
end

function textfield:hide_caret()
  hide_caret(self)
end

local function publish_region(self)
  render.set_hit(self.key, {{x = self.x, y = self.y, width = self.width, height = self.height}}, self.group)
end

function textfield.new(spec)
  if type(spec.width) ~= "number" or ui.snap(spec.width) < MIN_WIDTH * P then
    error("textfield.new: width must be a number of at least " .. MIN_WIDTH * P .. " points", 2)
  end
  if math.type(spec.greatest) ~= "integer" or spec.greatest < 1 then
    error("textfield.new: greatest must be a whole number of at least 1", 2)
  end
  local value = spec.value or ""
  local chars = type(value) == "string" and split(value) or nil
  if chars == nil then
    error("textfield.new: value must be a string of valid UTF-8", 2)
  end
  if #chars > spec.greatest then
    error("textfield.new: value is longer than " .. spec.greatest .. " characters", 2)
  end
  for _, character in ipairs(chars) do
    if not takes(character) then
      error("textfield.new: value has a character the font cannot draw: " .. character, 2)
    end
  end
  local self = setmetatable({}, textfield)
  self.greatest = spec.greatest
  self.value = value
  self.chars = chars
  self.first = 1 -- the first character drawn
  self.position = #chars -- the characters before the caret
  self.apply_callback = spec.on_apply -- the caller's; `on_apply` as a method is the editor protocol of lib/typing.lua (below)
  self.group = spec.group
  self.owner = spec.owner
  self.colours = theme.kind(spec.kind or "window")
  self.key = ui.key("textfield")
  self.x = ui.snap(spec.x)
  self.y = ui.snap(spec.y)
  self.is_editing = false
  self.width = ui.snap(spec.width)
  self.height = HEIGHT * P
  self.frame = frame.new{
    x = self.x,
    y = self.y,
    width = self.width,
    height = self.height,
    cut = CUT,
    corners = "tl_br",
    fill = self.colours.fill,
    line = self.colours.outline,
    group = self.group,
  }
  self.line = text.line(self.x + INSET * P, self.y + TEXT_TOP * P, self.group, self.colours.text)
  draw(self)
  publish_region(self)
  pointer.register(self.key, self, self.owner)
  return self
end

-- The typing function's entries (lib/typing.lua), while this field edits.
function textfield:on_typed(entry)
  local kind = entry.kind
  if kind == "char" then
    local character = entry.text
    if type(character) == "string" and utf8.len(character) == 1 and #self.chars < self.greatest and takes(character) then
      table.insert(self.chars, self.position + 1, character)
      self.position = self.position + 1
      scroll(self)
      draw(self)
    end
  elseif kind == "backspace" then
    if self.position > 0 then
      table.remove(self.chars, self.position)
      self.position = self.position - 1
      scroll(self)
      draw(self)
    end
  elseif kind == "left" then
    if self.position > 0 then
      self.position = self.position - 1
      scroll(self)
      draw(self)
    end
  elseif kind == "right" then
    if self.position < #self.chars then
      self.position = self.position + 1
      scroll(self)
      draw(self)
    end
  elseif kind == "enter" or kind == "tab" then
    fieldedit.apply(self)
  end
end

-- The editor protocol of lib/typing.lua (`typing.capture` applies and ends the editing first): lib/fieldedit.lua's. The caller's own
-- callback is `apply_callback`.
function textfield:on_apply()
  fieldedit.capture_apply(self)
end

-- A press on the field starts its editing (an event of an overflow packet starts nothing).
function textfield:on_event(event, eligible)
  if event.kind == "down" and eligible ~= false then
    fieldedit.begin(self)
  end
end

function textfield:on_hover() end
function textfield:on_overflow() end

-- A press on the field does something: it starts the editing.
function textfield:functional()
  return true
end

-- The text the field holds (what was last applied or set).
function textfield:get()
  return self.value
end

-- Sets the text; a field that is editing goes back to showing it, with the caret at its end, and keeps editing. A text that is not valid
-- UTF-8, is longer than the greatest length or has a character the font cannot draw is an error.
function textfield:set(value)
  local chars = type(value) == "string" and split(value) or nil
  if chars == nil then
    error("textfield.set: the text must be a string of valid UTF-8", 2)
  end
  if #chars > self.greatest then
    error("textfield.set: the text is longer than " .. self.greatest .. " characters", 2)
  end
  for _, character in ipairs(chars) do
    if not takes(character) then
      error("textfield.set: the text has a character the font cannot draw: " .. character, 2)
    end
  end
  self.value = value
  self.chars = chars
  self.position = #chars
  self.first = 1
  if self.is_editing then
    scroll(self)
  end
  draw(self)
end

function textfield:move(x, y)
  self.x = ui.snap(x)
  self.y = ui.snap(y)
  self.frame:set_rect(self.x, self.y, self.width, self.height)
  draw(self)
  publish_region(self)
end

-- The size in points.
function textfield:size()
  return self.width, self.height
end

function textfield:editing()
  return self.is_editing
end

-- Ends the editing and applies nothing; the caret goes with it. Safe to call twice.
function textfield:destroy()
  if self.frame == nil then
    return
  end
  fieldedit.end_editing(self, false)
  pointer.unregister(self.key)
  render.set_hit(self.key, nil, self.group)
  self.frame:destroy()
  text.destroy(self.line)
  self.frame = nil
end

return textfield
