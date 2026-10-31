-- Demonstration of the live scene (milestone 5): a quad you move with the keys while twelve other textures load, with the
-- magenta checker shown for every texture that has not arrived. Run it with
--
--   tools/ck cargo run --features metal -- --config config/demo-scene.toml
--
-- What happens: the window opens at once and shows the gem (assets/demo-quad.png, 64 x 64 points) from the first frame
-- (or as a magenta checker for a moment if its pixels are not there yet). Twelve tiles are then admitted two per update,
-- so they appear one after the other as magenta checkers and turn into their images (assets/demo-scene/*.png) a moment
-- later. The twelfth names a file that does not exist: its checker stays for the whole run, and its status is logged once
-- as `failed`. The arrow keys and WASD move the gem (160 points per second, opposed keys cancel); R reloads every texture
-- (the quads do not change, the images are read and uploaded again); Escape closes the window. Each texture's first
-- `ready` is logged once, and nothing is logged per frame.
--
-- Run headless (--headless), the window and scene calls are validated and ignored with one record each, `startup()` runs,
-- no update is ever posted, and the run ends by itself.
print("ui: loaded")
log.info("loaded")

local SPEED = 160 -- window points per second
local WIDTH, HEIGHT = 800, 600
local PLAYER_SIZE = 64
local TILE_SIZE = 48
local TILES_PER_UPDATE = 2
local COLUMNS = 4

local files = {
  "demo-scene/tile-01-stripes-h.png",
  "demo-scene/tile-02-stripes-v.png",
  "demo-scene/tile-03-diagonal.png",
  "demo-scene/tile-04-checker.png",
  "demo-scene/tile-05-dots.png",
  "demo-scene/tile-06-cross.png",
  "demo-scene/tile-07-ring.png",
  "demo-scene/tile-08-frame.png",
  "demo-scene/tile-09-diamond.png",
  "demo-scene/tile-10-quarters.png",
  "demo-scene/tile-11-bars.png",
  "demo-scene/missing-tile.png", -- no such file: a checker for the whole run
}

local x, y = 360, 440
local player, player_quad
local tiles = {} -- admitted tiles: { name, texture, quad }
local named = {} -- every texture: { name, texture, logged }
local updates = 0

local function remember(name, texture)
  named[#named + 1] = { name = name, texture = texture, logged = false }
end

-- The statuses of every texture, once: the state the service reports at this point.
local function log_states(when)
  for _, entry in ipairs(named) do
    log.info(when .. " " .. entry.name .. " " .. texture.status(entry.texture).state)
  end
end

local function admit_tile(index)
  local column, row = (index - 1) % COLUMNS, (index - 1) // COLUMNS
  local name = files[index]
  local handle = texture.load(name)
  local quad = render.create_quad{
    texture = handle,
    x = 140 + column * 140,
    y = 60 + row * 110,
    width = TILE_SIZE,
    height = TILE_SIZE,
  }
  tiles[#tiles + 1] = { name = name, texture = handle, quad = quad }
  remember(name, handle)
end

-- Logs a texture's first `ready`, and the failure of one that cannot load, each once.
local function poll_states()
  for _, entry in ipairs(named) do
    if not entry.logged then
      local status = texture.status(entry.texture)
      if status.state == "ready" then
        entry.logged = true
        log.info("texture ready: " .. entry.name)
      elseif status.state == "failed" then
        entry.logged = true
        log.info("texture failed: " .. entry.name .. ": " .. tostring(status.error))
      end
    end
  end
end

local function on_update(f)
  updates = updates + 1
  -- Two more tiles per update until all twelve are in.
  for _ = 1, TILES_PER_UPDATE do
    if #tiles < #files then
      admit_tile(#tiles + 1)
    end
  end
  -- The gem moves with the held keys; held keys move it every update, opposed keys cancel.
  local dx = (f.actions.move_right.held and 1 or 0) - (f.actions.move_left.held and 1 or 0)
  local dy = (f.actions.move_down.held and 1 or 0) - (f.actions.move_up.held and 1 or 0)
  if dx ~= 0 or dy ~= 0 then
    x = math.max(0, math.min(WIDTH - PLAYER_SIZE, x + dx * SPEED * f.dt))
    y = math.max(0, math.min(HEIGHT - PLAYER_SIZE, y + dy * SPEED * f.dt))
    render.set_position(player_quad, x, y)
  end
  -- R reads and uploads every texture again; the quads stay as they are.
  if f.actions.reload_texture.pressed then
    for _, entry in ipairs(named) do
      texture.reload(entry.texture)
      entry.logged = false
    end
    log.info("reload requested")
  end
  poll_states()
end

function startup()
  log.info("startup")
  player = texture.load("demo-quad.png")
  player_quad = render.create_quad{
    texture = player,
    x = x,
    y = y,
    width = PLAYER_SIZE,
    height = PLAYER_SIZE,
  }
  remember("demo-quad.png", player)
  -- The point a headless run's statuses are read at: after this nothing more is admitted, and no update ever comes.
  log_states("startup status")
  frame.on_update(on_update)
  window.open{title = "Codekarl live scene", width = WIDTH, height = HEIGHT, clear = {0.10, 0.25, 0.45}}
end

window.on_opened(function()
  log.info("the window is open")
end)

window.on_close_request(function(source)
  log.info("close requested by " .. source)
  return true
end)

function shutdown()
  log.info("updates " .. updates)
  -- After the run's end the service is closed: both calls raise its Ended error and touch no file.
  local function first_line(text)
    return (tostring(text):match("^[^\n]*"))
  end
  local ok_status, status_error = pcall(texture.status, player)
  log.info("ended status raised=" .. tostring(not ok_status) .. " " .. first_line(status_error))
  local ok_load, load_error = pcall(texture.load, "demo-quad.png")
  log.info("ended load raised=" .. tostring(not ok_load) .. " " .. first_line(load_error))
  log.info("shutdown")
end
