-- The character screen (CK-375 §6): a window over the left slot (where the location panel is) that shows one character: a header (the
-- portrait, the name and epithet, the house line, "age N" and the role line), a strip of four tabs below it and the tab's content in a
-- scrolling stack. The first tab, "character", has eight sections, each headed by its word in the term colour with a rule to the right, every icon
-- naming its number in a tooltip (the glossary entry of its quantity). The "relations" tab (9b, `lib/character_relations.lua`) and the "pop" tab (9c,
-- `lib/character_pop.lua`) fill the stack with their own sections; "actions" says "nothing yet". A small button at the right of the name line, on every
-- tab, opens the life log window (9c, `lib/character_log.lua`).
--
-- **The data path.** The screen reads `game.character(id, revision)`, the immutable record of that character at a revision `game_time`
-- carried, and never anything live. It keeps its character's id and the **world** number it was opened in, and holds no revision of its own:
-- the ledger's `game_time` callback calls `screen:refresh(revision, world)` with each delivered snapshot. A different world closes it (an id
-- may now name someone else); the same world reads the character again, keeping the tab and the scroll, and the character no longer live
-- closes it. A read the cache refuses (a retired revision) changes nothing: the next snapshot is read instead.
--
-- **Opening another** character while one is open replaces it, so one close always returns to what was underneath.
local ui = require("lib.ui")
local windows = require("lib.window")
local tabs = require("lib.tabs")
local cards = require("lib.cards")
local scrollbar = require("lib.scrollbar")
local location = require("lib.location")
local theme = require("lib.theme")
local tooltip = require("lib.tooltip")
local draw = require("lib.draw")
local pointer = require("lib.pointer")
local relations = require("lib.character_relations")
local pop_tab = require("lib.character_pop")
local character_log = require("lib.character_log")
local button = require("lib.button")

local P = ui.P

local character_screen = {}
character_screen.__index = character_screen

-- The icons of the first tab, by the quantity they stand for (`assets/ui/characters/`; the set the owner approved, r85-r87).
local BODY = {
  {key = "height", icon = "height", unit = "cm"}, {key = "weight", icon = "weight", unit = "kg"},
  {key = "strength", icon = "strength"}, {key = "wit", icon = "quill"}, {key = "intellect", icon = "brain"},
  {key = "cunning", icon = "fox"}, {key = "longevity", icon = "heart"}, {key = "constitution", icon = "shield"},
  {key = "fecundity", icon = "egg"},
}
local HEALTH_ICONS = {cut = "cut", bruise = "bruise"}
local SKILL_ICONS = {
  leadership = "flag", craftsmanship = "hammer", seamanship = "anchor", scholarship = "book", horsemanship = "horse",
  martial_prowess = "sword", oratory = "speech", administration = "scroll", theology = "lamp", medicine = "mortar", husbandry = "cow",
}
local VIRTUE_ICONS = {
  pride_humility = "crown", greed_charity = "coins", lust_chastity = "rose", envy_kindness = "eye_green",
  gluttony_temperance = "drumstick", wrath_patience = "flame", sloth_diligence = "zz",
}
local HUMOUR_ICONS = {blood = "drop", yellow_bile = "sun", black_bile = "moon", phlegm = "waves"}
local MANNER_ICONS = {
  guarded_open = "door", deceitful_honest = "mask", suspicious_trusting = "lens", wary_daring = "bolt", traditional_curious = "question",
  yielding_stubborn = "stone", solitary_gregarious = "crowd", content_ambitious = "ladder", harsh_merciful = "hand",
}
local STANDING_ICONS = {popularity = "pair", fame = "star", honour = "laurel"}
local STRESS_ICON = "cloud"

-- Every icon the screen draws, for the build's check that each is in the approved set.
local function icon_names()
  local seen, names = {}, {}
  local function add(name)
    if not seen[name] then seen[name] = true; names[#names + 1] = name end
  end
  for _, item in ipairs(BODY) do add(item.icon) end
  for _, set in ipairs({HEALTH_ICONS, SKILL_ICONS, VIRTUE_ICONS, HUMOUR_ICONS, MANNER_ICONS, STANDING_ICONS}) do
    for _, name in pairs(set) do add(name) end
  end
  add(STRESS_ICON)
  add("scroll")
  for _, name in ipairs(relations.icon_names()) do add(name) end
  table.sort(names)
  return names
end
character_screen.ICONS = icon_names()
character_screen.PORTRAITS = {"unknown_card", "unknown_header"}

-- The four tabs and what the three that are not built yet say.
local TABS = {
  {key = "character", label = "character", short = "char", tiny = "c"},
  {key = "pop", label = "pop", short = "pop", tiny = "p"},
  {key = "relations", label = "relations", short = "rel", tiny = "r"},
  {key = "actions", label = "actions", short = "act", tiny = "a"},
}
local LATER = {actions = "nothing yet"}

-- Where the parts of the screen lie, in UI pixels from the window's content origin.
local PORTRAIT = 48
local TABS_Y = 52
local RULE_Y = 67
local STACK_Y = 70
local GAP = 3
-- The life-log button's width in UI pixels (an icon button: the icon and its padding).
local LOG_BUTTON = 13

local function signed(value)
  if value > 0 then return "+" .. value end
  return tostring(value)
end

-- A bar's fill: `value` of `top` over `width` pixels, to the nearest pixel.
local function filled(value, top, width)
  local part = math.max(0, math.min(value, top))
  return (width * part * 2 + top) // (2 * top)
end

-- The tip of the first hotspot (`{x, y, width, height, key}`) the point (x, y) lies in: the glossary entry of its quantity.
local function tip_of(hotspots, x, y)
  for _, spot in ipairs(hotspots) do
    if x >= spot.x and x < spot.x + spot.width and y >= spot.y and y < spot.y + spot.height then
      if spot.tip ~= nil then return spot.tip() end
      if spot.content ~= nil then return spot.content end
      return tooltip.explanation(spot.key)
    end
  end
  return nil
end

local function heading(result, group, x, y, width, word)
  local used = draw.text(result, group, x, y, word, theme.kind("place").divider, "small")
  draw.rect(result, group, x + used + 3, y + 4, math.max(1, width - used - 3), 1, theme.kind("window").outline)
end

-- A two-sided bar around a centre line: an icon, the bar, the signed number. `top` is the value at each end; the values beyond it are drawn at
-- the end and shown true.
local function two_sided(result, group, x, y, icon, value, top, bar, positive, negative)
  draw.image(result, group, icon, x, y - 1, 9, 9)
  local left, half = x + 12, bar // 2
  draw.rect(result, group, left, y + 3, half * 2, 3, theme.colour("track"))
  local length = filled(math.abs(value), top, half)
  if value > 0 then
    draw.rect(result, group, left + half, y + 3, length, 3, positive or theme.colour("text"))
  elseif value < 0 then
    draw.rect(result, group, left + half - length, y + 3, length, 3, negative or theme.colour("text"))
  end
  draw.rect(result, group, left + half, y + 1, 1, 7, theme.colour("dim"))
  draw.text(result, group, left + half * 2 + 4, y + 1, signed(value), theme.colour("number"), "small")
end

-- Each section is `{height, draw(group, x, y, width), tip(x, y)}` in the units the card stack gives: `draw` takes points, `tip` UI pixels
-- from the section's top-left. The sections are laid out for `inner` UI pixels (the content's width less the scroll bar's) whether or not the bar
-- shows, so that their heights never depend on it (lib/cards.lua).
local function section(height, paint)
  local spots = {}
  local item = {height = height}
  item.draw = function(group, _, y, _)
    for index = #spots, 1, -1 do spots[index] = nil end
    local result = draw.view()
    paint(result, group, y // P, spots)
    return result
  end
  item.tooltip = function(x, y) return tip_of(spots, x, y) end
  return item
end

local function body_section(record, inner)
  local pitch = inner // 4
  return section(10 + 3 * 13 - 2, function(result, group, y, spots)
    heading(result, group, 0, y, inner, "body")
    for index, entry in ipairs(BODY) do
      local value = record.body[index]
      local cx, cy = ((index - 1) % 4) * pitch, y + 10 + ((index - 1) // 4) * 13
      draw.frame(result, group, cx, cy, 13, 11, theme.colour("tile"), theme.kind("window").outline)
      draw.image(result, group, entry.icon, cx + 2, cy + 1, 9, 9)
      local used = draw.text(result, group, cx + 16, cy + 2, tostring(value.value), theme.colour("number"), "small")
      if entry.unit ~= nil then
        draw.text(result, group, cx + 16 + used + 1, cy + 2, entry.unit, theme.colour("dim"), "small")
      end
      spots[#spots + 1] = {x = cx, y = 10 + ((index - 1) // 4) * 13, width = pitch - 2, height = 11, key = entry.key}
    end
  end)
end

-- One row for each injury (`{kind, part, severity, healing}`): the kind's icon, the part, three pips for the severity and a healing bar with its
-- percent, or "lasting" when it does not heal. None are simulated yet, so the section says "none".
local function health_section(record, inner)
  local rows = math.max(1, #record.health)
  return section(10 + rows * 10, function(result, group, y, spots)
    heading(result, group, 0, y, inner, "health")
    if #record.health == 0 then
      draw.text(result, group, 0, y + 11, "none", theme.colour("dim"), "small")
      return
    end
    for index, injury in ipairs(record.health) do
      local row = y + 10 + (index - 1) * 10
      local icon = HEALTH_ICONS[injury.kind]
      if icon ~= nil then draw.image(result, group, icon, 0, row - 1, 9, 9) end
      draw.text(result, group, 12, row + 1, injury.part, theme.colour("text"), "small")
      for pip = 0, 2 do
        draw.rect(result, group, 60 + pip * 4, row + 2, 3, 4,
          pip < injury.severity and theme.colour("text") or theme.colour("track"))
      end
      if injury.healing == nil then
        draw.text(result, group, 78, row + 1, "lasting", theme.colour("dim"), "small")
      else
        draw.rect(result, group, 78, row + 3, 60, 3, theme.colour("track"))
        draw.rect(result, group, 78, row + 3, filled(injury.healing, 100, 60), 3, theme.colour("text"))
        draw.text(result, group, 142, row + 1, injury.healing .. "%", theme.colour("number"), "small")
      end
      spots[#spots + 1] = {x = 0, y = 9 + (index - 1) * 10, width = 11, height = 10, key = injury.kind}
    end
  end)
end

-- The two columns of a section of paired rows: the column's x and the bar's width.
local function columns(inner)
  local half = inner // 2
  return half, (half - 41) // 2 * 2
end

local function skills_section(record, inner)
  local half, bar = columns(inner)
  local rows = (#record.skills + 1) // 2
  return section(10 + rows * 10, function(result, group, y, spots)
    heading(result, group, 0, y, inner, "skills")
    for index, skill in ipairs(record.skills) do
      local cx, cy = ((index - 1) // rows) * half, y + 10 + ((index - 1) % rows) * 10
      local icon = SKILL_ICONS[skill.key]
      if icon ~= nil then draw.image(result, group, icon, cx, cy - 1, 9, 9) end
      draw.rect(result, group, cx + 12, cy + 3, bar, 3, theme.colour("track"))
      draw.rect(result, group, cx + 12, cy + 3, filled(skill.value, 20, bar), 3, theme.colour("text"))
      draw.text(result, group, cx + 12 + bar + 4, cy + 1, tostring(skill.value), theme.colour("number"), "small")
      spots[#spots + 1] = {x = cx, y = 9 + ((index - 1) % rows) * 10, width = half - 2, height = 10, key = skill.key}
    end
  end)
end

local function virtue_section(record, inner)
  local half, bar = columns(inner)
  local rows = (#record.virtue + 1) // 2
  return section(10 + rows * 10, function(result, group, y, spots)
    heading(result, group, 0, y, inner, "virtue")
    for index, pair in ipairs(record.virtue) do
      local cx, cy = ((index - 1) // rows) * half, y + 10 + ((index - 1) % rows) * 10
      two_sided(result, group, cx, cy, VIRTUE_ICONS[pair.key] or "crown", pair.value, 7, bar,
        theme.colour("virtue"), theme.colour("sin"))
      spots[#spots + 1] = {x = cx, y = 9 + ((index - 1) % rows) * 10, width = half - 2, height = 10, key = pair.key}
    end
  end)
end

-- The four humours as pip meters 0..10, two to a row, each humour's filled pips in its own colour and the empty ones in the track colour;
-- then the stress bar 0..100 with a tick at the threshold and "stress/threshold".
local function temper_section(record, inner)
  local half = inner // 2
  return section(10 + 2 * 10 + 11, function(result, group, y, spots)
    heading(result, group, 0, y, inner, "temper")
    for index, humour in ipairs(record.humours) do
      local cx, cy = ((index - 1) % 2) * half, y + 10 + ((index - 1) // 2) * 10
      draw.image(result, group, HUMOUR_ICONS[humour.key] or "drop", cx, cy - 1, 9, 9)
      local colour = theme.colour("humour_" .. humour.key)
      for pip = 0, 9 do
        local px = cx + 11 + pip * 5
        if pip < humour.value then
          draw.rect(result, group, px, cy + 1, 4, 6, theme.colour("dim"))
          draw.rect(result, group, px + 1, cy + 2, 2, 4, colour)
        else
          draw.rect(result, group, px + 1, cy + 2, 2, 4, theme.colour("track"))
        end
      end
      spots[#spots + 1] = {x = cx, y = 9 + ((index - 1) // 2) * 10, width = half - 2, height = 10, key = humour.key}
    end
    local row = y + 10 + 2 * 10 + 1
    local bar = inner - 51
    draw.image(result, group, STRESS_ICON, 0, row - 1, 9, 9)
    draw.rect(result, group, 12, row + 3, bar, 3, theme.colour("track"))
    draw.rect(result, group, 12, row + 3, filled(record.stress, 100, bar), 3, theme.colour("text"))
    draw.rect(result, group, 12 + filled(record.threshold, 100, bar), row + 1, 1, 7, theme.colour("dim"))
    local at = 12 + bar + 5
    at = at + draw.text(result, group, at, row + 1, tostring(record.stress), theme.colour("number"), "small")
    at = at + draw.text(result, group, at, row + 1, "/", theme.colour("dim"), "small")
    draw.text(result, group, at, row + 1, tostring(record.threshold), theme.colour("number"), "small")
    spots[#spots + 1] = {x = 0, y = 10 + 2 * 10, width = 11, height = 10, key = "stress"}
    spots[#spots + 1] = {x = 12 + bar + 5, y = 10 + 2 * 10, width = inner - 12 - bar - 5, height = 10, key = "stress_threshold"}
  end)
end

local function manner_section(record, inner)
  local half, bar = columns(inner)
  local rows = (#record.manner + 1) // 2
  return section(10 + rows * 10, function(result, group, y, spots)
    heading(result, group, 0, y, inner, "manner")
    for index, pair in ipairs(record.manner) do
      local cx, cy = ((index - 1) // rows) * half, y + 10 + ((index - 1) % rows) * 10
      -- The bar is drawn clamped to -20..+20; the number is the true one.
      two_sided(result, group, cx, cy, MANNER_ICONS[pair.key] or "door", pair.value, 20, bar)
      spots[#spots + 1] = {x = cx, y = 9 + ((index - 1) % rows) * 10, width = half - 2, height = 10, key = pair.key}
    end
  end)
end

local function standing_section(record, inner)
  local pitch = inner // 4
  return section(10 + 12, function(result, group, y, spots)
    heading(result, group, 0, y, inner, "standing")
    for index, entry in ipairs(record.standing) do
      local cx = (index - 1) * pitch
      draw.image(result, group, STANDING_ICONS[entry.key] or "pair", cx, y + 9, 9, 9)
      draw.text(result, group, cx + 12, y + 11, signed(entry.value), theme.colour("number"), "small")
      spots[#spots + 1] = {x = cx, y = 9, width = pitch - 2, height = 11, key = entry.key}
    end
  end)
end

-- The chips: the traits as dark chips and the attributes as light ones, each the words of its key and, for a graded key, its class letter. They
-- flow in rows across `inner` and wrap.
local CHIP_HEIGHT, CHIP_PITCH = 13, 15

local function chips_of(record, inner)
  local ordered = {}
  for _, attribute in ipairs({false, true}) do
    for _, chip in ipairs(record.traits) do
      if chip.attribute == attribute then ordered[#ordered + 1] = chip end
    end
  end
  local laid, x, row = {}, 0, 0
  for _, chip in ipairs(ordered) do
    local width = draw.width(chip.words .. (chip.letter ~= "" and (" " .. chip.letter) or ""), "small") + 8
    if x > 0 and x + width > inner then
      x, row = 0, row + 1
    end
    laid[#laid + 1] = {chip = chip, x = x, row = row, width = width}
    x = x + width + 3
  end
  return laid, row
end

local function traits_section(record, inner)
  local laid, last_row = chips_of(record, inner)
  local rows = #laid == 0 and 1 or last_row + 1
  return section(10 + (rows - 1) * CHIP_PITCH + CHIP_HEIGHT, function(result, group, y, spots)
    heading(result, group, 0, y, inner, "traits")
    if #laid == 0 then
      draw.text(result, group, 0, y + 11, "none", theme.colour("dim"), "small")
      return
    end
    for _, entry in ipairs(laid) do
      local chip, cy = entry.chip, y + 10 + entry.row * CHIP_PITCH
      local ink = chip.attribute and theme.colour("chip_light_text") or theme.colour("text")
      draw.frame(result, group, entry.x, cy, entry.width, CHIP_HEIGHT,
        chip.attribute and theme.colour("chip_light") or theme.colour("tile"), theme.kind("window").outline)
      local used = draw.text(result, group, entry.x + 4, cy + 2, chip.words, ink, "small")
      if chip.letter ~= "" then
        draw.text(result, group, entry.x + 4 + used + draw.width(" ", "small"), cy + 2, chip.letter,
          chip.attribute and ink or theme.colour("number"), "small")
      end
      spots[#spots + 1] = {
        x = entry.x, y = 10 + entry.row * CHIP_PITCH, width = entry.width, height = CHIP_HEIGHT,
        key = chip.attribute and "attribute" or "trait",
      }
    end
  end)
end

-- The sections of the first tab, in their order.
local function first_tab(record, inner)
  return {
    body_section(record, inner), health_section(record, inner), skills_section(record, inner), virtue_section(record, inner),
    temper_section(record, inner), manner_section(record, inner), standing_section(record, inner), traits_section(record, inner),
  }
end

-- A tab with nothing to show yet: one dim line.
local function later(word)
  return {section(13, function(result, group, y)
    draw.text(result, group, 0, y + 1, word, theme.colour("dim"), "small")
  end)}
end

-- ---------------------------------------------------------------------------------------------------------------------------------
-- The screen
-- ---------------------------------------------------------------------------------------------------------------------------------

-- The inner width the sections are laid out for: the content's, less the scroll bar's room.
local function inner_width(self)
  return self.window.content_width // P - scrollbar.RESERVE // P
end

local function title_of(record)
  if record.epithet ~= "" then return record.name .. " " .. record.epithet end
  return record.name
end

-- The header: the portrait, the name and epithet, the house line, "age N" and the role line.
local function draw_header(self, record)
  if self.header ~= nil then self.header:destroy() end
  local result = draw.view()
  local group = self.window.content
  local line = theme.kind("window").outline
  draw.frame(result, group, 0, 0, PORTRAIT, PORTRAIT, theme.colour("tile"), line)
  draw.image(result, group, "unknown_header", 0, 0, PORTRAIT, PORTRAIT, theme.colour("dim"))
  local x = PORTRAIT + 6
  local room = self.window.content_width // P - x - 2
  -- The life-log button stands at the right of the name line and the house line under it.
  local near = room - LOG_BUTTON - 3
  local text_colour = theme.colour("text")
  draw.text(result, group, x, 1, draw.clip(title_of(record), near), text_colour)
  if record.house ~= "" then
    draw.text(result, group, x, 14, draw.clip(record.house, near, "small"), theme.colour("term"), "small")
  else
    draw.text(result, group, x, 14, "no house", theme.colour("dim"), "small")
  end
  draw.text(result, group, x, 25, {
    {text = "age ", colour = text_colour}, {text = tostring(record.age), colour = theme.colour("number")},
  }, text_colour, "small")
  draw.text(result, group, x, 36, draw.clip(record.role, room, "small"), text_colour, "small")
  draw.rect(result, group, 0, RULE_Y, self.window.content_width // P, 1, theme.kind("window").hot)
  self.header = result
end

local function stack_height(self)
  return math.max(P, self.window.content_height - STACK_Y * P)
end

-- Reads the relations record of the character shown at the revision the screen holds. Returns whether it was read, and if not why.
local function read_relations(self)
  local ok, found = pcall(game.character_relations, self.character, self.revision)
  if ok and found ~= nil then
    self.relations = found
    return true
  end
  return false, found
end

-- The name of a place for the open-location button: the places list's.
local function name_of_place(id)
  for _, place in ipairs(game.places()) do
    if place.id == id then return place.name end
  end
  return id
end

-- Reads the pop the character belongs to at the revision the screen holds: its exact place and slot with its live characters
-- (`game.character_pop`), and that slot of that revision's economy view (`game.place`). Returns whether it was read, and if not why. A tab that cannot
-- read says "not available" and changes nothing else.
local function read_pop(self)
  local ok, found = pcall(game.character_pop, self.character, self.revision)
  if not ok then return false, found end
  local placed, place = pcall(game.place, found.place, self.revision)
  if not placed then return false, place end
  local pop = place.pops[found.slot + 1]
  if pop == nil then
    self.pop_data = nil
    return false, "no pop"
  end
  self.pop_data = {
    pop = pop, place = found.place, place_name = name_of_place(found.place), slot = found.slot, revision = self.revision,
    characters = found.characters,
  }
  return true
end

-- What the relations tab's sections need of the screen: its helpers, the character shown, and how a click on a live portrait opens that
-- character's screen. The click is acted on once the press that made it has been dispatched, because opening replaces the very stack it came from.
local function relations_kit(self)
  return {
    section = section, heading = heading, subject = self.record,
    open = function(id)
      pointer.defer(function()
        if self:is_open() then self:open(id, self.revision, self.world) end
      end)
    end,
    -- The open-location button closes the screen and shows the place under it, once the press that made the click has been dispatched.
    open_place = function(id)
      pointer.defer(function()
        if not self:is_open() then return end
        local name = name_of_place(id)
        self:close()
        if self.open_place ~= nil then self.open_place(id, name) end
      end)
    end,
  }
end

local function items_of(self)
  local sections
  if self.tab == "character" then
    sections = first_tab(self.record, inner_width(self))
  elseif self.tab == "relations" then
    if self.relations ~= nil then
      sections = relations.sections(self.relations, inner_width(self), relations_kit(self))
    else
      sections = later("not available")
    end
  elseif self.tab == "pop" then
    if self.pop_data ~= nil then
      sections = pop_tab.sections(self.pop_data, inner_width(self), relations_kit(self))
    else
      sections = later("not available")
    end
  else
    sections = later(LATER[self.tab])
  end
  return sections
end

-- Draws the tab's content in the stack, which is made when there is none; `keep` keeps the scroll.
local function fill(self, keep)
  local items = items_of(self)
  if self.stack ~= nil and not self.stack.closed then
    local scroll = self.stack:scroll_position()
    self.stack:set_items(items)
    self.stack:set_pos(keep and scroll or 0)
  else
    self.stack = cards.new{
      window = self.window, x = 0, y = STACK_Y * P, width = self.window.content_width, height = stack_height(self), gap = GAP, items = items,
    }
  end
end

-- Lays the whole screen out for the window it has: the header, the tabs and the stack.
local function build(self)
  draw_header(self, self.record)
  if self.tabs ~= nil then self.tabs:destroy() end
  self.tabs = tabs.new{
    window = self.window,
    items = TABS,
    selected = self.tab,
    body = {x = 0, y = TABS_Y * P, width = self.window.content_width},
    on_select = function(key)
      self.tab = key
      if key == "relations" then read_relations(self) end
      if key == "pop" then read_pop(self) end
      fill(self, false)
    end,
  }
  if self.log_button ~= nil then self.log_button:destroy() end
  self.log_button = button.new{
    x = self.window.content_width - LOG_BUTTON * P, y = 0, icons = {"scroll"}, icon = "scroll", tooltip = "life log", kind = "window",
    group = self.window.content, owner = self.window,
    on_click = function()
      if self:is_open() then self.log:open(self.character, self.record.name, self.revision, self.world) end
    end,
  }
  if self.stack ~= nil and not self.stack.closed then self.stack:destroy() end
  self.stack = nil
  fill(self, false)
end

-- `character_screen.new{view = {width, height}, top = function() return points end, open_place = function(id, name) end}`: the screen, closed.
-- `open_place` shows a place in the location panel once the screen has closed (the pop tab's open-location button).
function character_screen.new(spec)
  if type(spec) ~= "table" or type(spec.view) ~= "table" or type(spec.top) ~= "function" then
    error("character_screen.new: expected `view` and `top`", 2)
  end
  local self = setmetatable({view = spec.view, top = spec.top, open_place = spec.open_place, tab = "character"}, character_screen)
  self.log = character_log.new{geometry = function() return location.slot(self.view, ui.snap(self.top())) end}
  return self
end

function character_screen:is_open()
  return self.window ~= nil and not self.window.closed
end

-- The id of the character shown, or nil.
function character_screen:id()
  return self:is_open() and self.character or nil
end

-- The key of the tab showing.
function character_screen:current_tab()
  return self.tab
end

function character_screen:raise()
  if self:is_open() then
    self.window:raise()
    -- The life log window stands over the screen and stays there.
    if self.log:is_open() then self.log.window:raise() end
  end
end

-- Closes the screen: the location panel under it shows as it was.
function character_screen:close()
  if self:is_open() then self.window:close() end
end

-- Whether the life log window is open, and the id of the character whose log it shows.
function character_screen:log_id()
  return self.log:id()
end

local function geometry(self)
  return location.slot(self.view, ui.snap(self.top()))
end

-- Opens the screen on the character `id` read at `revision` in the world `world`, or shows that character in it when it is open (one close
-- returns to what was under it). The first tab shows. Returns whether the character was read: a refused read changes nothing.
function character_screen:open(id, revision, world)
  local ok, record = pcall(game.character, id, revision)
  if not ok then return false end
  self.character, self.world, self.record, self.tab = id, world, record, "character"
  self.revision, self.relations, self.pop_data = revision, nil, nil
  -- The log window belongs to the character it was opened for: another character's screen closes it.
  self.log:close()
  if self:is_open() then
    self.window:raise()
    build(self)
    return true
  end
  local top, width, height = geometry(self)
  local created
  created = windows.new{
    title = "character", kind = "window", outer_width = width, outer_height = height, x = 0, y = top, draggable = false,
    on_close = function()
      if self.window == created then
        self.log:close()
        self.window, self.header, self.tabs, self.stack, self.log_button = nil, nil, nil, nil, nil
        self.character, self.record, self.relations, self.pop_data = nil, nil, nil, nil
      end
    end,
  }
  self.window = created
  build(self)
  return true
end

-- A `game_time` delivered `revision` of `world`: a new world closes the screen, the same world reads the character again, keeping the tab and
-- the scroll, and a character no longer live closes it. A read that is refused for its revision changes nothing.
function character_screen:refresh(revision, world)
  if not self:is_open() then return end
  if world ~= self.world then
    self:close()
    return
  end
  local ok, record = pcall(game.character, self.character, revision)
  if not ok then
    if tostring(record):find("no character", 1, true) ~= nil then self:close() end
    return
  end
  if record == nil then return end
  self.record, self.revision = record, revision
  -- The tab showing is read again at the same revision: a read the cache refuses leaves what it shows as it was.
  if self.tab == "relations" or self.tab == "pop" then
    local read, why
    if self.tab == "relations" then read, why = read_relations(self) else read, why = read_pop(self) end
    if not read and tostring(why):find("no character", 1, true) ~= nil then
      self:close()
      return
    end
  end
  -- The life log window, when open, reads its lines again for the new revision.
  self.log:refresh(revision, world)
  draw_header(self, record)
  fill(self, true)
end

-- The place names show differently now (CK-423): the record, the tab showing and the log read again at the revision the screen holds, which
-- gives each place the name the choice shows. No new revision and no tick is needed.
function character_screen:relabel()
  if not self:is_open() or self.revision == nil then return end
  self:refresh(self.revision, self.world)
end

-- The window's size changed: fits the screen to the left slot again, keeping the tab and the scroll.
function character_screen:relayout()
  if not self:is_open() then return end
  local scroll = self.stack ~= nil and not self.stack.closed and self.stack:scroll_position() or 0
  local top, width, height = geometry(self)
  self.window:set_outer_size(width, height)
  self.window:move_to(0, top)
  build(self)
  if self.stack ~= nil and not self.stack.closed then self.stack:set_pos(scroll) end
  self.log:relayout()
end

return character_screen
