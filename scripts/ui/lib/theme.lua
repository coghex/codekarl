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

-- A kind's colours with the defaults applied. The result is kept (the theme does not change during a run) and must not be edited.
function theme.kind(name)
  local kept = kinds[name]
  if kept ~= nil then
    return kept
  end
  local raw = loaded().kind(name)
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
