-- The character screen's pop tab (CK-378 §1): the pop the character belongs to in the revision the screen shows, from
-- `game.character_pop(id, revision)` (its exact place and slot, and its live characters) and that slot of that revision's economy view
-- (`game.place(place, revision).pops[slot + 1]`), never another pop of the same kind and never a newer revision.
--
-- Four sections: the identity ("<kind> at <place>" with the culture and religion terms, and the open-location button at the right), the people
-- (workers, dependents and food days), the satisfaction (a bar on the game's -100..+100 scale with a tick at the pop's equilibrium and
-- "value/equilibrium", then the three needs met as the location panel's pops tab shows them) and the characters of the pop as portraits. Politics
-- is left out until the desired-laws model exists, and so is wealth: pops hold none yet.
--
-- The screen owns the window and the card stack; it gives this module the `kit` of its helpers and calls `sections(record, inner, kit)`. Every number
-- takes the location panel's tooltip for it, called with the tab's revision and the pop's slot.
local ui = require("lib.ui")
local theme = require("lib.theme")
local tooltip = require("lib.tooltip")
local draw = require("lib.draw")
local relations = require("lib.character_relations")

local pop_tab = {}

local P = ui.P
local CELL, UNDER = relations.CELL, relations.UNDER
local ROW = 10

local function signed(value)
  if value > 0 then return "+" .. value end
  return tostring(value)
end

-- A bar `width` wide with `part` of `whole` filled, to the nearest pixel.
local function fill_of(part, whole, width)
  local clamped = math.max(0, math.min(part, whole))
  return (width * clamped * 2 + whole) // (2 * whole)
end

-- The words of a glossary key: its title, or the key itself when the glossary has none.
local function title_of(key)
  local entry = glossary.entry(key)
  if entry ~= nil and entry.title ~= nil then return entry.title end
  return key
end

local function kind_colour(kind)
  local ok, colour = pcall(theme.colour, "pop_" .. tostring(kind))
  if ok and colour ~= nil then return colour end
  return theme.colour("text")
end

local function character_title(card)
  if card.epithet ~= "" then return card.name .. " " .. card.epithet end
  return card.name
end

-- The tip of a portrait: the name, the house line and the role line, nothing more.
local function portrait_tip(card)
  return {
    title = character_title(card),
    card.house ~= "" and card.house or "no house",
    card.role,
  }
end

-- The identity: the kind in its colour at the place in the term colour, the culture and religion as glossary terms, and the open-location button.
local function identity_section(data, inner, kit)
  local pop, place_name, place = data.pop, data.place_name, data.place
  local label = "open location"
  local button_width = 9 + 3 + draw.width(label, "small") + 8
  return relations.item_of(kit, 24, function(result, group, y, spots, hits)
    local kind = pop.kind
    local used = draw.text(result, group, 0, y + 1, kind, kind_colour(kind))
    local at = draw.text(result, group, used + 4, y + 1, "at", theme.colour("dim"))
    local room = inner - (kit.standalone and 0 or button_width + 4) - used - at - 8
    draw.text(result, group, used + at + 8, y + 1, draw.clip(place_name, room), theme.colour("term"))
    hits[#hits + 1] = {part = "kind", x = 0, y = 0, width = used, height = 12, content = tooltip.explanation(kind)}
    local culture_used = draw.text(result, group, 0, y + 13, title_of(pop.culture), theme.colour("term"), "small")
    local dot = draw.width(" · ", "small")
    draw.text(result, group, culture_used, y + 13, " · ", theme.colour("dim"), "small")
    draw.text(result, group, culture_used + dot, y + 13, title_of(pop.religion), theme.colour("term"), "small")
    hits[#hits + 1] = {part = "culture", x = 0, y = 12, width = culture_used, height = 11, content = tooltip.explanation(pop.culture)}
    hits[#hits + 1] = {
      part = "religion", x = culture_used + dot, y = 12, width = draw.width(title_of(pop.religion), "small"), height = 11,
      content = tooltip.explanation(pop.religion),
    }
    if kit.standalone then return end
    -- The button: a frame, the ledger's location icon and the words.
    local left = inner - button_width
    draw.frame(result, group, left, y, button_width, 13, theme.colour("tile"), theme.kind("social").outline)
    local icon = render.create_quad{
      texture = ui.texture("ui/theme/icon_location.png"), x = (left + 3) * P, y = (y + 1) * P, width = 9 * P, height = 11 * P,
      group = group, tint = theme.colour("text"),
    }
    result:add({destroy = function() render.destroy_quad(icon) end})
    draw.text(result, group, left + 3 + 9 + 3, y + 3, label, theme.colour("text"), "small")
    hits[#hits + 1] = {
      part = "open_location", x = left, y = 0, width = button_width, height = 13, content = {"open " .. place_name},
      open = function() kit.open_place(place, place_name) end,
    }
  end)
end

-- A hit whose content is made when it is hovered, from the location panel's class for this pop in the revision the tab shows.
local function class_spot(data, part, class, x, y, width, height)
  return {
    part = part, x = x, y = y, width = width, height = height,
    tip = function() return tooltip.content(class, {place = data.place, pop = data.slot + 1, revision = data.revision}) end,
  }
end

local function people_section(data, inner, kit)
  local pop = data.pop
  local half = inner // 2
  return relations.item_of(kit, 10 + 2 * ROW, function(result, group, y, _, hits)
    kit.heading(result, group, 0, y, inner, "people")
    local function figure(x, row, value, word, class, part)
      local top = y + 10 + row * ROW
      local used = draw.text(result, group, x, top + 1, tostring(value), theme.colour("number"), "small")
      local words = draw.text(result, group, x + used + 3, top + 1, word, theme.colour("text"), "small")
      hits[#hits + 1] = class_spot(data, part, class, x, 10 + row * ROW, used + 3 + words, ROW)
    end
    figure(0, 0, pop.workers, "workers", "pop_size", "workers")
    figure(half, 0, pop.dependents, "dependents", "pop_size", "dependents")
    figure(0, 1, pop.food_days, "food days", "pop_food", "food_days")
  end)
end

local NEEDS = {{key = "necessities", field = "necessities"}, {key = "niceties", field = "niceties"}, {key = "luxuries", field = "luxuries"}}

-- The satisfaction: its bar on -100..+100 with the equilibrium's tick and "value/equilibrium", then the three needs, each a bar and a percentage.
local function satisfaction_section(data, inner, kit)
  local pop = data.pop
  return relations.item_of(kit, 10 + 12 + 3 * ROW, function(result, group, y, _, hits)
    kit.heading(result, group, 0, y, inner, "satisfaction")
    local row = y + 11
    local label = signed(pop.satisfaction) .. "/" .. signed(pop.equilibrium)
    local label_width = draw.width(label, "small")
    local bar = inner - label_width - 8
    local function place(value) return fill_of(value + 100, 200, bar) end
    draw.rect(result, group, 0, row + 2, bar, 3, theme.colour("track"))
    draw.rect(result, group, 0, row + 2, place(pop.satisfaction), 3, theme.colour("text"))
    draw.rect(result, group, math.min(bar - 1, place(pop.equilibrium)), row, 1, 7, theme.colour("dim"))
    draw.text(result, group, bar + 6, row, {
      {text = signed(pop.satisfaction), colour = theme.colour("number")},
      {text = "/", colour = theme.colour("dim")},
      {text = signed(pop.equilibrium), colour = theme.colour("number")},
    }, theme.colour("number"), "small")
    hits[#hits + 1] = {part = "satisfaction", x = 0, y = 10, width = inner, height = 11, content = tooltip.explanation("satisfaction")}
    -- The three needs, as the pops tab of the location panel shows them: per mille to whole percent by floor division, the bar clamped to 0..1000.
    local label_x = 0
    local widest = 0
    for _, need in ipairs(NEEDS) do widest = math.max(widest, draw.width(need.key, "small")) end
    local bar_x = widest + 6
    local need_bar = inner - bar_x - 30
    for index, need in ipairs(NEEDS) do
      local top = y + 10 + 12 + (index - 1) * ROW
      local per_mille = pop[need.field]
      draw.text(result, group, label_x, top, need.key, theme.colour("dim"), "small")
      draw.rect(result, group, bar_x, top + 2, need_bar, 3, theme.colour("track"))
      -- Filled by the location panel's rule (floor), not the satisfaction bar's nearest pixel.
      draw.rect(result, group, bar_x, top + 2, draw.bar_fill(math.max(0, math.min(per_mille, 1000)), 1000, need_bar), 3, theme.colour("text"))
      draw.text(result, group, bar_x + need_bar + 5, top, (per_mille // 10) .. "%", theme.colour("number"), "small")
      hits[#hits + 1] = {
        part = need.key, x = 0, y = 10 + 12 + (index - 1) * ROW, width = inner, height = ROW, content = tooltip.explanation("needs"),
      }
    end
  end)
end

-- The characters of the pop: portraits seven to a row and two rows, then a "+N" chip listing the rest. The viewed character is framed.
local function characters_section(data, inner, kit)
  local people = data.characters
  local pitch, columns = relations.pitch_of(inner)
  local capacity = 2 * columns
  local shown, rest = {}, {}
  if #people <= capacity then
    shown = people
  else
    -- Past two rows the last place is the "+N" chip. The viewed character always has a portrait: the first places go to the most important, but when the
    -- viewed one is not among them it takes the last of them, and the portraits keep their importance order.
    local places = capacity - 1
    local viewed = kit.subject and kit.subject.id
    local viewed_at
    for index, card in ipairs(people) do
      if card.id == viewed then viewed_at = index end
    end
    for index, card in ipairs(people) do
      local visible = index <= places
      if viewed_at ~= nil and viewed_at > places then visible = index < places or index == viewed_at end
      if visible then shown[#shown + 1] = card else rest[#rest + 1] = card end
    end
  end
  local slots = #shown + (#rest > 0 and 1 or 0)
  local rows = math.max(1, (slots + columns - 1) // columns)
  return relations.item_of(kit, 10 + rows * (CELL + UNDER) - UNDER + 2, function(result, group, y, _, hits)
    kit.heading(result, group, 0, y, inner, "characters")
    for index, card in ipairs(shown) do
      local column, row = (index - 1) % columns, (index - 1) // columns
      local x, top = column * pitch, y + 10 + row * (CELL + UNDER)
      local current = kit.subject ~= nil and card.id == kit.subject.id
      if current then
        draw.frame(result, group, x - 1, top - 1, CELL + 2, CELL + 2, theme.colour("tile"), theme.kind("social").hot)
      end
      draw.frame(result, group, x, top, CELL, CELL, theme.colour("tile"), theme.kind("social").outline)
      draw.image(result, group, "unknown_card", x, top, CELL, CELL, theme.colour("dim"))
      hits[#hits + 1] = {
        part = "portrait", x = x, y = top - y, width = CELL, height = CELL, content = portrait_tip(card),
        open = (not current) and function() kit.open(card.id) end or nil,
      }
    end
    if #rest > 0 then
      local index = #shown
      local column, row = index % columns, index // columns
      local x, top = column * pitch, y + 10 + row * (CELL + UNDER)
      draw.frame(result, group, x, top, CELL, CELL, theme.colour("tile"), theme.kind("social").outline)
      local label = "+" .. #rest
      draw.text(result, group, x + (CELL - draw.width(label, "small")) // 2, top + (CELL - 7) // 2, label, theme.colour("text"), "small")
      local lines = {title = label .. " more"}
      for _, card in ipairs(rest) do lines[#lines + 1] = character_title(card) end
      hits[#hits + 1] = {part = "chip", x = x, y = top - y, width = CELL, height = CELL, content = lines}
    end
  end)
end

-- The sections of the pop tab. `data` = `{pop, place, place_name, slot, revision, characters}` where `pop` is the economy view's pop of that
-- revision and slot; `kit` = `{section, heading, open, open_place, subject}`.
function pop_tab.sections(data, inner, kit)
  return {
    identity_section(data, inner, kit),
    people_section(data, inner, kit),
    satisfaction_section(data, inner, kit),
    characters_section(data, inner, kit),
  }
end

return pop_tab
