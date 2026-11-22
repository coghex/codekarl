-- The ledger's Characters tab (CK-375 §2): a card for each live character, from `game.characters(revision)`. A card is a square portrait
-- on the left and three lines on the right: the name and epithet; the house line in the term colour (or "no house", dim); and "age N" with
-- the role line. The cards stand in the card stack (`lib/cards.lua`) with its scroll bar, and the list bar (`lib/listbar.lua`) filters and
-- sorts them; both come from the ledger, which owns the window.
--
-- The rows (`character_cards.rows`) carry what the list bar reads (`sort`, `filter`); the items (`character_cards.items`) are what the stack
-- draws. A click on a card calls `on_open(id)`; hovering one tells the name, the house line and the role line, nothing more.
local ui = require("lib.ui")
local theme = require("lib.theme")
local draw = require("lib.draw")

local P = ui.P

local character_cards = {}

-- The size of a card and of its portrait cell, and the gap between cards, in UI pixels.
character_cards.HEIGHT = 40
character_cards.GAP = 3
local PORTRAIT = 32
local TEXT_X = 4 + PORTRAIT + 6

-- What the bar sorts by. The importance is the order the rows come in (it is also the first choice), so a bar with no choice keeps it.
character_cards.SORTS = {
  {key = "importance", label = "importance", first = "ascending"},
  {key = "name", label = "name", first = "ascending"},
  {key = "age", label = "age", first = "descending"},
  {key = "house", label = "house", first = "ascending"},
}

-- The one group of the filter: a box for each of these a character is, of the ones some character is.
character_cards.GROUPS = {{key = "who", label = "who"}}

-- The words of a card: its name with its epithet.
local function title_of(character)
  if character.epithet ~= "" then return character.name .. " " .. character.epithet end
  return character.name
end

-- The rows of `list` (the table `game.characters` returned), most important first.
function character_cards.rows(list)
  local rows = {}
  for _, character in ipairs(list) do
    local who = {character.sex == "f" and "women" or "men"}
    if character.ruler then who[#who + 1] = "rulers" end
    if character.clergy then who[#who + 1] = "clergy" end
    if character.child then who[#who + 1] = "children" end
    rows[#rows + 1] = {
      character = character,
      sort = {
        importance = character.importance,
        name = character.name,
        age = character.age,
        -- No house sorts last, whichever way the sort goes.
        house = character.house ~= "" and character.house or nil,
      },
      filter = {who = who},
    }
  end
  table.sort(rows, function(a, b) return a.character.importance < b.character.importance end)
  return rows
end

-- One card, drawn at (x, y) points of `group` and `width` points wide.
local function draw_card(character, group, x, y, width)
  local result = draw.view()
  local dx, dy, wide = x // P, y // P, width // P
  local line = theme.kind("places_bar").outline
  draw.frame(result, group, dx, dy, wide, character_cards.HEIGHT, theme.colour("body"), line)
  draw.frame(result, group, dx + 4, dy + 4, PORTRAIT, PORTRAIT, theme.colour("tile"), line)
  draw.image(result, group, "unknown_card", dx + 4, dy + 4, PORTRAIT, PORTRAIT, theme.colour("dim"))
  local room = wide - TEXT_X - 4
  local text_colour = theme.colour("text")
  draw.text(result, group, dx + TEXT_X, dy + 3, draw.clip(title_of(character), room), text_colour)
  if character.house ~= "" then
    draw.text(result, group, dx + TEXT_X, dy + 16, draw.clip(character.house, room, "small"), theme.colour("term"), "small")
  else
    draw.text(result, group, dx + TEXT_X, dy + 16, "no house", theme.colour("dim"), "small")
  end
  local age = "age "
  local number = tostring(character.age)
  local used = draw.width(age .. number .. "  ", "small")
  draw.text(result, group, dx + TEXT_X, dy + 27, {
    {text = age, colour = text_colour},
    {text = number, colour = theme.colour("number")},
    {text = "  " .. draw.clip(character.role, room - used, "small"), colour = text_colour},
  }, text_colour, "small")
  return result
end

-- The items of the card stack for `rows` (as `rows` made them, in the order the bar passed them). `on_open(id, world)` is what a click does: it is
-- given the world the cards were made in, so that a click that outlives a load cannot open whoever has that number in the new world.
-- `tag_of(id)` gives the tag a card's region carries for a right-click (see `lib/go_to.lua`).
function character_cards.items(rows, on_open, world, tag_of)
  local items = {}
  for _, row in ipairs(rows) do
    local character = row.character
    items[#items + 1] = {
      height = character_cards.HEIGHT,
      on_click = function() on_open(character.id, world) end,
      -- What a right-click on the card means (CK-381): `tag_of(id)` is hung on the card's hit region (`render.set_hit_tag`).
      tag = tag_of ~= nil and tag_of(character.id) or nil,
      tooltip = function()
        return {title = title_of(character), character.house ~= "" and character.house or "no house", character.role}
      end,
      draw = function(group, x, y, width) return draw_card(character, group, x, y, width) end,
    }
  end
  return items
end

return character_cards
