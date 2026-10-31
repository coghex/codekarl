-- The milestone 5 scale acceptance's all-quads-moving profile (CK-130, M-lua; docs/scene-design.md section 11.3). MEASURED, NOT GATED.
--
-- Run by apps/codekarl/tests/scale_lua.rs through the application's own code in process (config/scale-lua.toml): the script loads set A
-- (32,768 identity PNGs, 128 per update), makes the full scene (65,536 quads, two per texture, painter-order overlap), and then moves
-- every quad with render.set_position on every update. The measured window is the application's: the script calls
-- diagnostics.metrics_mark() once every texture is ready and 5 s of `elapsed` have passed since (the mark's window starts 250 ms later),
-- and calls window.close() once 30.25 s of `elapsed` have passed since the mark. Time is the sum of the updates' `elapsed` (each is the time since the previous delivered update, from the monotonic clock),
-- never a count of callbacks, which coalescing makes variable.

local TEXTURES = 32768
local COLUMNS, ROWS = 256, 128
local CELL_W, CELL_H = 7.5, 8.4375          -- drawable pixels
local WARM_UP, SAMPLE = 5.0, 30.25          -- seconds of `elapsed`
local PER_UPDATE_LOADS = 128
local QUADS_PER_UPDATE = 4096
local CHECKS_PER_UPDATE = 2048

local tokens, quads = {}, {}
local loaded, made, checked = 0, 0, 0
local base_x, base_y = {}, {}
local scale
local ready_at, mark_at
local closing = false
local clock = 0.0                        -- seconds since the first update: the sum of each update's `elapsed` (which is the time since the previous one)

local function name_of(k)
  return string.format('a/%05d.png', k)
end

-- The pixel geometry of quad `n` (1-based) of the full scene, in drawable pixels, as the recipe freezes it.
local function geometry(n)
  local index = n - 1
  local pass, i = index // TEXTURES, index % TEXTURES
  local column, row = i % COLUMNS, i // COLUMNS
  local x, y = column * CELL_W, row * CELL_H
  if pass == 0 then
    return i, x, y
  end
  local dx = (column == COLUMNS - 1) and -CELL_W / 2 or CELL_W / 2
  local dy = (row == ROWS - 1) and -CELL_H / 2 or CELL_H / 2
  return i - (i % 2), x + dx, y + dy
end

function startup()
  frame.on_update(function(f)
    clock = clock + f.elapsed
    if not scale then
      scale = f.pixel_width / f.width
      log.info(string.format('scale-lua: drawable %dx%d, window %dx%d points, backing scale %.3f',
        f.pixel_width, f.pixel_height, f.width, f.height, scale))
    end

    -- 1. 128 loads per update.
    local until_load = math.min(loaded + PER_UPDATE_LOADS, TEXTURES)
    while loaded < until_load do
      tokens[loaded + 1] = texture.load(name_of(loaded))
      loaded = loaded + 1
    end

    -- 2. The quads, in chunks, once their textures are requested (a quad shows the checker until its texture is ready).
    if loaded == TEXTURES then
      local until_made = math.min(made + QUADS_PER_UPDATE, 2 * TEXTURES)
      while made < until_made do
        local n = made + 1
        local k, x, y = geometry(n)
        base_x[n], base_y[n] = x / scale, y / scale
        quads[n] = render.create_quad{texture = tokens[k + 1], x = base_x[n], y = base_y[n],
          width = CELL_W / scale, height = CELL_H / scale}
        made = n
      end
    end

    -- 3. Every texture ready: a rolling check (the next unchecked token must be ready for the pointer to move on).
    if made == 2 * TEXTURES and not ready_at then
      local limit = math.min(checked + CHECKS_PER_UPDATE, TEXTURES)
      while checked < limit and texture.status(tokens[checked + 1]).state == 'ready' do
        checked = checked + 1
      end
      if checked == TEXTURES then
        ready_at = clock
        log.info(string.format('scale-lua: all %d textures ready at elapsed %.3f', TEXTURES, ready_at))
      end
    end

    -- 4. Move every quad (once they exist): a small scripted circle that is the same for all of them.
    if made == 2 * TEXTURES then
      local angle = clock * 2.0
      local sx, sy = math.sin(angle) * 1.5, math.cos(angle) * 1.5
      for n = 1, 2 * TEXTURES do
        render.set_position(quads[n], base_x[n] + sx, base_y[n] + sy)
      end
    end

    -- 5. The mark, then the close.
    if ready_at and not mark_at and clock - ready_at >= WARM_UP then
      mark_at = clock
      diagnostics.metrics_mark()
      log.info(string.format('scale-lua: marked at elapsed %.3f', mark_at))
    end
    if mark_at and not closing and clock - mark_at >= SAMPLE then
      closing = true
      log.info(string.format('scale-lua: closing at elapsed %.3f (%.3f s after the mark)', clock, clock - mark_at))
      window.close()
    end
  end)
  window.open{title = 'Codekarl scale (Lua)', width = 960, height = 540, clear = {0.1, 0.2, 0.3}}
end

function shutdown()
  log.info('scale-lua: shutdown')
end
