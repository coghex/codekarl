-- The theme data the host loaded from `assets/ui/theme.toml` (milestone 8, slice 7b; docs/ui-design.md §3.7), with the defaults applied.
--
-- `theme.colour(name)` is a `[colour]` value and `theme.kind(name)` a kind's colours: `outline`, `hot`, `highlight_number`, `fill`,
-- `text`, `title_text`, `pressed_text`, `divider` (nil when the kind has none), `row_hover` and `row_pressed`. Every colour is the string as written
-- (`#rrggbb` or `#rrggbbaa`), the form `render.set_tint` and `tint =` take. `time_bar` also requires `speed_box` and `speed_fill`.
local host = theme -- the host's table; this module's own `theme` shadows it below

local theme = {}

local function loaded()
  if host == nil then
    error("no theme: assets/ui/theme.toml was not loaded", 3)
  end
  return host
end

function theme.colour(name)
  return loaded().colour(name)
end

local kinds = {}
local kind_bases = {window = "general", time_bar = "general", places_bar = "general", tooltip = "general",
  place = "location", social = "social", expedition = "expedition"}
local listeners = setmetatable({}, {__mode = "k"})
local current = nil

function theme.palette() return loaded().palette() end
function theme.row_ink(ink, fill, fallback)
  if host.row_ink == nil then return fallback end
  return host.row_ink(ink, fill, fallback)
end
function theme.bases()
  local result = loaded().bases()
  if settings ~= nil and settings.colours ~= nil then
    for key, value in pairs(settings.colours()) do result[key] = value end
  end
  return result
end

function theme.watch(owner, callback) listeners[owner] = callback end
function theme.unwatch(owner) listeners[owner] = nil end

function theme.refresh()
  if host.bases == nil then return end
  local bases = theme.bases()
  local changed = current == nil
  if current ~= nil then
    for key, value in pairs(bases) do if current[key] ~= value then changed = true end end
  end
  if not changed then return end
  current = bases
  for name, kept in pairs(kinds) do
    local base = kind_bases[name]
    if base ~= nil then
      local raw = loaded().kind(name, bases[base])
      for key, value in pairs(raw) do kept[key] = value end
      kept.row_hover, kept.row_pressed = kept.outline, kept.hot
    end
  end
  -- A callback may rebuild widgets, removing and adding watchers. Iterate a
  -- stable pass and leave new widgets (already built with these colours) alone.
  local snapshot = {}
  for owner, callback in pairs(listeners) do
    snapshot[#snapshot + 1] = {owner, callback}
  end
  for _, entry in ipairs(snapshot) do
    local owner, callback = entry[1], entry[2]
    if listeners[owner] == callback then callback(owner) end
  end
end

-- A kind's colours with the defaults applied. The kept table is updated in place when a base changes.
function theme.kind(name)
  local kept = kinds[name]
  if kept ~= nil then
    return kept
  end
  if current == nil and host.bases ~= nil then current = theme.bases() end
  local base = kind_bases[name]
  local raw = base ~= nil and current ~= nil and loaded().kind(name, current[base]) or loaded().kind(name)
  local text = loaded().colour("text")
  if name == "time_bar" then
    for _, key in ipairs({"speed_box", "speed_fill"}) do
      if raw[key] == nil then
        error("theme.kind: [kind.time_bar] missing `" .. key .. "`", 2)
      end
    end
  end
  kept = {
    outline = raw.outline,
    hot = raw.hot,
    fill = raw.fill or loaded().colour("body"),
    text = raw.text or text,
    title_text = raw.title_text or text,
    pressed_text = raw.pressed_text or text,
    highlight_number = raw.highlight_number or loaded().colour("number"),
    highlight_over_limit = raw.highlight_over_limit or loaded().colour("over_limit"),
    divider = raw.divider,
    row_hover = raw.row_hover or raw.outline,
    row_pressed = raw.row_pressed or raw.hot,
    speed_box = raw.speed_box,
    speed_fill = raw.speed_fill,
  }
  kinds[name] = kept
  return kept
end

return theme
