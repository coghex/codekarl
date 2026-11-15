-- The number field (CK-306; docs/ui-design.md): a cut-3 frame 15 UI pixels tall, as wide as `0000` plus 8, filled `body`, outlined in the
-- kind's `outline`, a whole number that is not negative right-aligned in `text` (it ends 4 UI pixels short of the right edge, 2 down).
--
-- **Editing.** A press on the field puts it into editing: the outline is the kind's `hot` and a 1-pixel caret, 11 tall and in `text`,
-- stands just right of the number. The number is kept; a digit is added at its end and Backspace takes the last digit away. A digit
-- typed into a full field and a Backspace in an empty one do nothing. One field edits at a time: a press on another field applies
-- the first and starts the second.
--
-- **Typing** comes from the library's typing function (lib/typing.lua) only: `typing.start(field)` when the editing begins and
-- `typing.stop(field)` when it ends, and the field's `on_typed(entry)` gets the update's entries in order. A character `0` to `9`
-- (the digit row or the number pad alike), `backspace`, `enter` and `tab` count; every other character, `left` and `right` are passed
-- over. No bound action carries typing.
--
-- **Applying** is Enter, Tab or a left press anywhere that is not this field (the map included; the press still does what it does):
-- the number is brought into the caller's range, the field shows the result, the editing ends and `on_apply(number)` is called once.
-- A field emptied and then applied puts its old number back, and a field whose number did not change calls nobody.
--
-- **Escape** while a field edits puts the old number back, ends the editing, tells the caller nothing and goes no further: the window
-- asks `numberfield.cancel()` first (lib/window.lua `close_request`).
--
-- **Closing** the window (or destroying the field) while it edits ends the editing and applies nothing.
--
-- **Key capture** (CK-309). Arming a capture (`typing.capture`) applies and ends the editing first, through the field's `on_apply()` editor
-- hook, and while it is armed no field starts editing (`typing.start` refuses, and a press on a field then changes nothing).
--
-- **Greyed** (CK-313). `field:set_greyed(true)` draws the field with its outline in `track` and its number in `dim`; it keeps its number and
-- still answers a press: the press makes the field live again, starts its editing and calls the caller's `on_enable(number)` once (the
-- frame-rate field: "none" is lit and the press turns the limit back on). A press that cannot start the editing (a key capture owns the
-- keyboard) changes nothing. The field is made live when the editing starts, before the previous field's apply callback runs (which may
-- close the window or destroy this field: it is then neither drawn nor reported); `on_enable` is told last, and only of a live field.
--
-- The editing's ownership (one field at a time, a number field or a text field alike) is lib/fieldedit.lua's; this file keeps what is the
-- number's: its buffer of digits, its look and its caret.
--
-- `numberfield.new{x, y, value, least, greatest, on_apply, on_enable = nil, greyed = false, digits = 4, kind = "window", group = nil, owner = nil}`: points, snapped; with a
-- `group` the quads and the hit key go into it and the position is local to it. A `digits` of 5 asks for a longer field.
local ui = require("lib.ui")
local text = require("lib.text")
local frame = require("lib.frame")
local theme = require("lib.theme")
local pointer = require("lib.pointer")
local fieldedit = require("lib.fieldedit")

local P = ui.P
local HEIGHT = 15 -- UI pixels
local CUT = 3
local PAD = 4 -- UI pixels between the number's end and the field's right edge; the field is wider than its digits by twice this
local TEXT_TOP = 2 -- UI pixels from the field's top to the text's
local CARET_TOP = 2
local CARET_HEIGHT = 11
local WHITE = "ui/theme/white.png"

local numberfield = {}
numberfield.__index = numberfield
numberfield.field_edit = true -- a field of lib/fieldedit.lua

local function draw(self)
  local width = self.width // P
  local inactive = self.inactive or self.greyed
  local ink = inactive and theme.colour("dim") or self.colours.text
  local outline = self.colours.outline
  if self.is_editing then
    outline = self.colours.hot
  elseif inactive then
    outline = theme.colour("track")
  end
  text.set(self.line, self.buffer)
  text.move(self.line, self.x + (width - PAD - self.line.width) * P, self.y + TEXT_TOP * P)
  text.set_colour(self.line, ink)
  self.frame:set_colours(self.colours.fill, outline)
end

local function show_caret(self)
  if self.caret == nil then
    self.caret = render.create_quad{
      texture = ui.texture(WHITE),
      x = self.x + (self.width // P - PAD + 1) * P,
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

local function clamp(value, least, greatest)
  return math.max(least, math.min(greatest, value))
end

-- The protocol of lib/fieldedit.lua.

-- Settles what was typed: the number is brought into the range and shown. Returns the call that tells the caller (nil when the field was
-- emptied, so its old number is back, or its number did not change).
function numberfield:commit()
  local old = self.value
  local result = old
  if self.buffer ~= "" then
    result = clamp(tonumber(self.buffer), self.least, self.greatest)
  end
  self.value = result
  self.buffer = tostring(result)
  draw(self)
  if result ~= old and self.apply_callback ~= nil then
    return function() self.apply_callback(result) end
  end
  return nil
end

function numberfield:revert()
  self.buffer = tostring(self.value)
  draw(self)
end

function numberfield:show_editing()
  -- The editing has started (a refused start never gets here): a greyed field is live from now on, and `on_event` tells the caller.
  if self.greyed then
    self.greyed = false
    self.enable_pending = true
  end
  draw(self)
  show_caret(self)
end

function numberfield:hide_caret()
  hide_caret(self)
end

local function publish_region(self)
  if self.inactive then
    render.set_hit(self.key, nil, self.group)
  else
    render.set_hit(self.key, {{x = self.x, y = self.y, width = self.width, height = self.height}}, self.group)
  end
end

function numberfield.new(spec)
  local digits = spec.digits or 4
  if math.type(digits) ~= "integer" or digits < 1 then
    error("numberfield.new: digits must be a whole number of at least 1", 2)
  end
  for _, name in ipairs({"value", "least", "greatest"}) do
    if math.type(spec[name]) ~= "integer" or spec[name] < 0 then
      error("numberfield.new: " .. name .. " must be a whole number that is not negative", 2)
    end
  end
  if spec.least > spec.greatest then
    error("numberfield.new: least is above greatest", 2)
  end
  if #tostring(spec.greatest) > digits then
    error("numberfield.new: greatest has more than " .. digits .. " digits; ask for a longer field with `digits`", 2)
  end
  local self = setmetatable({}, numberfield)
  self.least = spec.least
  self.greatest = spec.greatest
  self.value = clamp(spec.value, spec.least, spec.greatest)
  self.buffer = tostring(self.value)
  self.digits = digits
  self.apply_callback = spec.on_apply -- the caller's; `on_apply` as a method is the editor protocol of lib/typing.lua (below)
  self.enable_callback = spec.on_enable
  self.greyed = spec.greyed == true
  self.group = spec.group
  self.owner = spec.owner
  self.colours = theme.kind(spec.kind or "window")
  self.key = ui.key("numberfield")
  self.x = ui.snap(spec.x)
  self.y = ui.snap(spec.y)
  self.is_editing = false
  self.inactive = spec.inactive == true
  self.width = (font.layout(string.rep("0", digits)).width + 2 * PAD) * P
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
  self.line = text.line(self.x, self.y + TEXT_TOP * P, self.group, self.colours.text)
  draw(self)
  publish_region(self)
  pointer.register(self.key, self, self.owner)
  fieldedit.register(self)
  return self
end

-- The typing function's entries (lib/typing.lua), while this field edits.
function numberfield:on_typed(entry)
  local kind = entry.kind
  if kind == "char" then
    local digit = entry.text
    if #digit == 1 and digit >= "0" and digit <= "9" and #self.buffer < self.digits then
      self.buffer = self.buffer .. digit
      draw(self)
    end
  elseif kind == "backspace" then
    if self.buffer ~= "" then
      self.buffer = self.buffer:sub(1, -2)
      draw(self)
    end
  elseif kind == "enter" then
    fieldedit.apply(self)
  elseif kind == "tab" or kind == "backtab" then
    fieldedit.tab(self, kind == "backtab")
  end
end

-- The editor protocol of lib/typing.lua (`typing.capture` applies and ends the editing first): lib/fieldedit.lua's. The caller's own
-- callback is `apply_callback`.
function numberfield:on_apply()
  fieldedit.capture_apply(self)
end

-- A press on the field starts its editing (an event of an overflow packet starts nothing).
function numberfield:on_event(event, eligible)
  if event.kind == "down" and eligible ~= false and not self.inactive then
    fieldedit.begin(self)
    -- A greyed field that started editing was made live by `show_editing` (below), before the previous field's callback ran. The
    -- caller is told last, and only of a field that still exists: that callback may have closed the window or destroyed this field.
    if self.enable_pending then
      self.enable_pending = false
      if self.frame ~= nil and self.enable_callback ~= nil then self.enable_callback(self.value) end
    end
  end
end

function numberfield:on_hover() end
function numberfield:on_overflow() end

-- A press on the field does something: it starts the editing.
function numberfield:functional()
  return not self.inactive
end

-- Makes a resolution field visibly unavailable and removes its hit region until it is live again.
function numberfield:set_inactive(inactive)
  inactive = inactive == true
  if self.inactive == inactive then return end
  self.inactive = inactive
  publish_region(self)
  draw(self)
end

-- The number the field holds (what was last applied), in the range.
function numberfield:get()
  return self.value
end

-- Sets the number (clamped); a field that is editing goes back to showing it and keeps editing.
function numberfield:set(value)
  if math.type(value) ~= "integer" or value < 0 then
    error("numberfield.set: the number must be a whole number that is not negative", 2)
  end
  self.value = clamp(value, self.least, self.greatest)
  self.buffer = tostring(self.value)
  draw(self)
end

-- Greys the field (see the header) or makes it live again. A field that edits stops, applying nothing, and shows its number.
function numberfield:set_greyed(greyed)
  greyed = greyed == true
  if self.greyed == greyed then
    return
  end
  if greyed and self.is_editing then
    fieldedit.end_editing(self, false)
    self.buffer = tostring(self.value)
  end
  self.greyed = greyed
  draw(self)
end

function numberfield:is_greyed()
  return self.greyed
end

function numberfield:move(x, y)
  self.x = ui.snap(x)
  self.y = ui.snap(y)
  self.frame:set_rect(self.x, self.y, self.width, self.height)
  if self.caret ~= nil then
    render.set_position(self.caret, self.x + (self.width // P - PAD + 1) * P, self.y + CARET_TOP * P)
  end
  draw(self)
  publish_region(self)
end

-- The size in points.
function numberfield:size()
  return self.width, self.height
end

function numberfield:editing()
  return self.is_editing
end

-- Ends the editing and applies nothing; the caret goes with it. Safe to call twice.
function numberfield:destroy()
  if self.frame == nil then
    return
  end
  fieldedit.end_editing(self, false)
  fieldedit.unregister(self)
  pointer.unregister(self.key)
  render.set_hit(self.key, nil, self.group)
  self.frame:destroy()
  text.destroy(self.line)
  self.frame = nil
end

-- The field editing now (a number field or a text field), or nil.
function numberfield.current()
  return fieldedit.current()
end

-- Escape's first step: puts the old value back in the field that edits and ends its editing. True when there was one.
function numberfield.cancel()
  return fieldedit.cancel()
end

return numberfield
