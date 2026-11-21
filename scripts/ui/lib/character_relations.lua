-- The character screen's relations tab (CK-377 §2-§3): portraits only, from `game.character_relations(id, revision)`.
--
-- The top line has "parents" over two portraits at the left (father then mother; the unknown shows as an empty dim cell) and the spouses at the
-- right ("wives", or "husbands" for a woman) in the stored order; then the children, one row by age; then the others row (lovers, rivals, friends,
-- foster kin, hostages held, then other living kin), which wraps once and past that ends in a "+N" chip; then the passions. A section with
-- nothing to show is left out. The dead are drawn dimmed and do nothing when clicked; a live portrait opens that character's screen.
--
-- Under each portrait stand up to two small icons: the formal tie's (kin a tree, foster kin a bowl, a hostage and its holder a lock) first and dim, then
-- the strands, strongest first (the bonds both share and this character's own feelings), and a "+" when more stand.
--
-- The screen owns the window and the card stack; it gives this module the `kit` of its section helpers and calls `sections(record, inner, kit)`.
-- Each section is an item of the stack (`lib/cards.lua`): `draw`, `tooltip(x, y)` and `on_click(x, y)`, the last two in UI pixels from the
-- item's top-left.
local ui = require("lib.ui")
local theme = require("lib.theme")
local tooltip = require("lib.tooltip")
local draw = require("lib.draw")

local relations = {}

-- The size of a portrait cell in UI pixels, the gap least wanted between two, and how many stand in a row.
local CELL = 24
local GAP = 3
local COLUMNS = 7
local MONTHS = {"Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"}
local ICON = 9
-- The room under a portrait for its icons, and between a portrait's icons and the next row.
local UNDER = 12

-- The icons, by what they stand for (designer rulings zixzsbi9ixzk8zjy5u9eu9inei and 692adpn7b2xjtd7dmuvkq27q7n; all of the approved set).
local TIE_ICONS = {kin = "tree", foster = "bowl", hostage = "lock"}
local STRAND_ICONS = {
  friend = "handshake", rival = "fist", lover = "lovers", love = "heart", respect = "star_pale", envy = "eye_green", fear = "ghost",
  hatred = "angry",
}
local PASSION_ICONS = {loyalty = "laurel", love = "heart", hatred = "angry", fear = "ghost"}

-- Every icon the tab draws, for the build's check that each is in the approved set.
function relations.icon_names()
  local seen, names = {}, {}
  for _, set in ipairs({TIE_ICONS, STRAND_ICONS, PASSION_ICONS}) do
    for _, name in pairs(set) do
      if not seen[name] then seen[name] = true; names[#names + 1] = name end
    end
  end
  table.sort(names)
  return names
end

-- The order of the strand kinds: the bonds, then the feelings (the simulation's order).
local KIND_ORDER = {friend = 1, rival = 2, lover = 3, love = 4, respect = 5, envy = 6, fear = 7, hatred = 8}

local function title_of(person)
  if person.epithet ~= "" then return person.name .. " " .. person.epithet end
  return person.name
end

local function first_name(name)
  return name:match("^%S+") or name
end

local function date_of(person)
  return string.format("%d %s %d", person.day, MONTHS[person.month], person.year)
end

-- How a person's end reads in a tooltip, or nil for a live one.
local function ending(person)
  if person.ended == "died" then return "died " .. date_of(person) end
  if person.ended == "before" then return "died before " .. person.year end
  if person.ended == "abroad" then return "abroad" end
  return nil
end

-- The pronouns of a person: he/him or she/her.
local function pronouns(person)
  if person.sex == "f" then return "she", "her" end
  return "he", "him"
end

-- The strands that stand under a portrait: the bonds and this character's own feelings, strongest first, then in the simulation's order.
local function badge_lines(person)
  local lines = {}
  for _, line in ipairs(person.lines) do
    if line.side == "both" or line.side == "mine" then lines[#lines + 1] = line end
  end
  table.sort(lines, function(a, b)
    if a.strength ~= b.strength then return a.strength > b.strength end
    return KIND_ORDER[a.kind] < KIND_ORDER[b.kind]
  end)
  return lines
end

-- The icons under a portrait, in order: `{icon, dim}`, at most two; and whether more stand.
local function badges(person)
  local items = {}
  if TIE_ICONS[person.tie] ~= nil then items[#items + 1] = {icon = TIE_ICONS[person.tie], dim = true} end
  for _, line in ipairs(badge_lines(person)) do
    items[#items + 1] = {icon = STRAND_ICONS[line.kind], dim = false, kind = line.kind}
  end
  local shown = {}
  for index = 1, math.min(2, #items) do shown[index] = items[index] end
  return shown, #items > 2
end

-- The tooltip of a portrait (§3): the name and epithet, the relation, then what is known of the two.
local function person_tip(subject, person)
  local lines = {title = title_of(person)}
  lines[#lines + 1] = person.relation
  local ended = ending(person)
  local me = first_name(subject.name)
  local he, him = pronouns(person)
  if ended == nil then
    lines[#lines + 1] = "age " .. person.age
    for _, line in ipairs(person.lines) do
      local label = {text = line.kind, term = "relationship"}
      if line.side == "both" then
        lines[#lines + 1] = {label, " " .. line.strength}
      elseif line.side == "mine" then
        lines[#lines + 1] = {label, string.format(" %d %s toward %s", line.strength, me, him)}
      else
        lines[#lines + 1] = {label, string.format(" %d %s toward %s", line.strength, he, me)}
      end
    end
    if person.opinion ~= nil then
      lines[#lines + 1] = string.format("opinion %d %s toward %s", person.opinion, me, him)
      lines[#lines + 1] = string.format("opinion %d %s toward %s", person.opinion_back, he, me)
    end
  else
    lines[#lines + 1] = ended
    local dim = theme.colour("dim")
    for _, line in ipairs(person.lines) do
      local words = line.kind .. " " .. line.strength
      if line.side == "mine" then
        words = words .. " " .. me .. " toward " .. him
      elseif line.side == "theirs" then
        words = words .. " " .. he .. " toward " .. me
      end
      lines[#lines + 1] = {{text = words .. " (history)", colour = dim}}
    end
  end
  return lines
end

-- A portrait cell at (x, y): the frame and the unknown portrait, the dead dimmed; an empty dim cell for `nil`.
local function paint_cell(result, group, x, y, person)
  draw.frame(result, group, x, y, CELL, CELL, theme.colour("tile"), theme.kind("window").outline)
  if person == nil then return end
  local ink = person.live and theme.colour("dim") or theme.colour("track")
  draw.image(result, group, "unknown_card", x, y, CELL, CELL, ink)
end

-- The icons under a portrait whose cell is at (x, y).
local function paint_badges(result, group, x, y, person)
  local shown, more = badges(person)
  local at = x
  for _, item in ipairs(shown) do
    draw.image(result, group, item.icon, at, y + CELL + 2, ICON, ICON, item.dim and theme.colour("dim") or nil)
    at = at + ICON + 2
  end
  if more then draw.text(result, group, at, y + CELL + 2, "+", theme.colour("text"), "small") end
end

-- The pitch between two portraits in a row and how many stand in a row, for `inner` UI pixels: seven where they fit, the gap shrinking to
-- three at most, and fewer in a window too narrow for seven side by side.
local function pitch_of(inner)
  local columns = COLUMNS
  if inner < columns * CELL then columns = math.max(1, inner // CELL) end
  if columns == 1 then return CELL, 1 end
  return math.min(CELL + GAP, (inner - CELL) // (columns - 1)), columns
end

-- An item whose click is told the point: `build(item)` fills `hits` with `{x, y, width, height, content, id}`.
local function item_of(kit, height, paint)
  local hits = {}
  local item = kit.section(height, function(result, group, y, spots)
    for index = #hits, 1, -1 do hits[index] = nil end
    paint(result, group, y, spots, hits)
  end)
  local function hit_at(x, y)
    for _, spot in ipairs(hits) do
      if x >= spot.x and x < spot.x + spot.width and y >= spot.y and y < spot.y + spot.height then return spot end
    end
    return nil
  end
  item.hits = hits
  -- What the stack asks when the pointer rests on the item at (x, y): the first region it lies in (an icon's before the portrait it stands under).
  item.tooltip = function(x, y)
    local spot = hit_at(x, y)
    if spot == nil then return nil end
    -- A spot may make its content when it is asked for (the pop tab's, which names the revision the tab shows).
    if spot.tip ~= nil then return spot.tip() end
    return spot.content
  end
  item.on_click = function(x, y)
    if x == nil or y == nil then return end
    local spot = hit_at(x, y)
    if spot ~= nil and spot.open ~= nil then spot.open() end
  end
  return item
end

-- Shared with the pop tab, whose portraits and button are laid out the same way.
relations.item_of = item_of
relations.pitch_of = pitch_of
relations.CELL = CELL
relations.UNDER = UNDER

-- Lays `people` out in rows of `columns` starting at column `first` of the first row, returning the cell of each: `{person, column, row}`.
local function cells_of(people, columns, first)
  local cells, column, row = {}, first, 0
  for _, person in ipairs(people) do
    if column >= columns then column, row = 0, row + 1 end
    cells[#cells + 1] = {person = person, column = column, row = row}
    column = column + 1
  end
  return cells, row
end

local function rows_height(rows)
  return rows * (CELL + UNDER) + (rows - 1) * 2
end

-- One portrait and what it is for the hit list: its tooltip and, when live, the click.
local function add_hit(hits, kit, person, x, y)
  local open = person.live and function() kit.open(person.id) end or nil
  -- A strand's icon names its kind, linking the shared `relationship` term; the tie's icon and the "+" say what the portrait does.
  local shown = badges(person)
  for index, item in ipairs(shown) do
    if item.kind ~= nil then
      hits[#hits + 1] = {
        part = "icon", x = x + (index - 1) * (ICON + 2), y = y + CELL + 2, width = ICON, height = ICON,
        content = {{{text = item.kind, term = "relationship"}}}, open = open,
      }
    end
  end
  hits[#hits + 1] = {
    part = "portrait", x = x, y = y, width = CELL, height = CELL + UNDER - 2, content = person_tip(kit.subject, person),
    open = open,
  }
end

-- The top line: "parents" at the left, the spouses at the right.
local function top_section(record, inner, kit)
  local pitch, columns = pitch_of(inner)
  local parents = {record.father, record.mother}
  local spouses = record.spouses
  -- The spouses stand at the right of the first row, as many as the room beside the parents allows; any more wrap to rows of their own.
  local beside = math.max(0, columns - 2)
  local first_row, more = {}, {}
  for index, person in ipairs(spouses) do
    if index <= beside then first_row[#first_row + 1] = person else more[#more + 1] = person end
  end
  local extra_rows = (#more + columns - 1) // columns
  local rows = 1 + extra_rows
  return item_of(kit, 10 + rows_height(rows), function(result, group, y, _, hits)
    kit.heading(result, group, 0, y, 2 * pitch, "parents")
    if #spouses > 0 then
      local right = (columns - #first_row) * pitch
      kit.heading(result, group, right, y, inner - right, record.spouse_word)
    end
    for index = 1, 2 do
      local person = parents[index]
      local x, cy = (index - 1) * pitch, y + 10
      paint_cell(result, group, x, cy, person)
      if person ~= nil then
        paint_badges(result, group, x, cy, person)
        add_hit(hits, kit, person, x, cy - y)
      end
    end
    for index, person in ipairs(first_row) do
      local x, cy = (columns - #first_row + index - 1) * pitch, y + 10
      paint_cell(result, group, x, cy, person)
      paint_badges(result, group, x, cy, person)
      add_hit(hits, kit, person, x, cy - y)
    end
    for index, person in ipairs(more) do
      local column, row = (index - 1) % columns, 1 + (index - 1) // columns
      local x, cy = column * pitch, y + 10 + row * (CELL + UNDER)
      paint_cell(result, group, x, cy, person)
      paint_badges(result, group, x, cy, person)
      add_hit(hits, kit, person, x, cy - y)
    end
  end)
end

-- A heading and rows of portraits.
local function grid_section(record, inner, kit, word, people)
  local pitch, columns = pitch_of(inner)
  local cells, last_row = cells_of(people, columns, 0)
  return item_of(kit, 10 + rows_height(last_row + 1), function(result, group, y, _, hits)
    kit.heading(result, group, 0, y, inner, word)
    for _, cell in ipairs(cells) do
      local x, cy = cell.column * pitch, y + 10 + cell.row * (CELL + UNDER)
      paint_cell(result, group, x, cy, cell.person)
      paint_badges(result, group, x, cy, cell.person)
      add_hit(hits, kit, cell.person, x, cy - y)
    end
  end)
end

-- The others row: two rows of portraits; past that the last slot is a "+N" chip whose tooltip lists the names that did not fit.
local function others_section(record, inner, kit)
  local pitch, columns = pitch_of(inner)
  local capacity = 2 * columns
  local people, rest = {}, {}
  if #record.others <= capacity then
    people = record.others
  else
    for index, person in ipairs(record.others) do
      if index < capacity then people[#people + 1] = person else rest[#rest + 1] = person end
    end
  end
  local cells, last_row = cells_of(people, columns, 0)
  local slot = #rest > 0 and #people or nil
  if slot ~= nil then last_row = math.max(last_row, slot // columns) end
  return item_of(kit, 10 + rows_height(last_row + 1), function(result, group, y, _, hits)
    kit.heading(result, group, 0, y, inner, "others")
    for _, cell in ipairs(cells) do
      local x, cy = cell.column * pitch, y + 10 + cell.row * (CELL + UNDER)
      paint_cell(result, group, x, cy, cell.person)
      paint_badges(result, group, x, cy, cell.person)
      add_hit(hits, kit, cell.person, x, cy - y)
    end
    if slot ~= nil then
      local x, cy = (slot % columns) * pitch, y + 10 + (slot // columns) * (CELL + UNDER)
      draw.frame(result, group, x, cy, CELL, CELL, theme.colour("tile"), theme.kind("window").outline)
      local label = "+" .. #rest
      local used = draw.width(label, "small")
      draw.text(result, group, x + (CELL - used) // 2, cy + (CELL - 7) // 2, label, theme.colour("text"), "small")
      local lines = {title = "+" .. #rest .. " more"}
      for _, person in ipairs(rest) do lines[#lines + 1] = title_of(person) .. " (" .. person.relation .. ")" end
      hits[#hits + 1] = {part = "chip", x = x, y = cy - y, width = CELL, height = CELL, content = lines}
    end
  end)
end

-- The name a passion's target reads as: a place or a house by the simulation's name; a culture or a religion by the title of its term in
-- the glossary; the key when nothing names it.
local function target_name(passion)
  if passion.name ~= "" then return passion.name end
  local entry = glossary.entry(passion.key)
  if entry ~= nil and entry.title ~= nil then return entry.title end
  return passion.key
end

local BAR = 90

-- The passions, one row each: the kind's icon, the kind word, the target, a strength bar out of 10 and the number.
local function passions_section(record, inner, kit)
  local rows = #record.passions
  return kit.section(10 + rows * 10, function(result, group, y, spots)
    kit.heading(result, group, 0, y, inner, "passions")
    local bar = math.min(BAR, inner // 3)
    for index, passion in ipairs(record.passions) do
      local row = y + 10 + (index - 1) * 10
      local icon = PASSION_ICONS[passion.kind]
      if icon ~= nil then draw.image(result, group, icon, 0, row - 1, ICON, ICON) end
      local used = draw.text(result, group, 12, row + 1, passion.kind, theme.colour("text"), "small")
      draw.text(result, group, 12 + used + 4, row + 1,
        draw.clip(target_name(passion), inner - 12 - used - 4 - bar - 22, "small"), theme.colour("term"), "small")
      local left = inner - bar - 14
      draw.rect(result, group, left, row + 3, bar, 3, theme.colour("track"))
      draw.rect(result, group, left, row + 3, (bar * passion.strength + 5) // 10, 3, theme.colour("text"))
      draw.text(result, group, left + bar + 4, row + 1, tostring(passion.strength), theme.colour("number"), "small")
      spots[#spots + 1] = {
        x = 0, y = 9 + (index - 1) * 10, width = inner, height = 10,
        content = {
          {{text = passion.kind, term = "passion"}, " " .. target_name(passion) .. " " .. passion.strength},
        },
      }
    end
  end)
end

-- The tab's sections for `record`, laid out for `inner` UI pixels. `kit` = `{section, heading, open, subject}`, where `subject` is the character shown (`name`).
function relations.sections(record, inner, kit)
  local sections = {top_section(record, inner, kit)}
  if #record.children > 0 then sections[#sections + 1] = grid_section(record, inner, kit, "children", record.children) end
  if #record.others > 0 then sections[#sections + 1] = others_section(record, inner, kit) end
  if #record.passions > 0 then sections[#sections + 1] = passions_section(record, inner, kit) end
  return sections
end

return relations
