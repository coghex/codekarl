-- Going to a thing on the map (CK-381): the camera moves to it and, when the thing has a window of its own, that window opens for the same
-- thing. The find popup (the next issue) calls the same function.
--
-- Every call names the thing and the economy `revision` the caller holds (the one `game_time` last carried, or the one a ledger row was drawn
-- from): the camera and the window both read that revision, so they cannot disagree. The engine (`game.go_to`) refuses a revision that is
-- retired or of a replaced world, an unknown thing, a pop slot the place does not hold and a character who is not live there; this module
-- then opens nothing.
--
-- A kind of thing is one entry of `KINDS`: `open` names the opener the caller registers with `go_to.set_openers` (a kind with none, a river,
-- is the camera only). An opener is called as `opener(id, revision)` after the camera has moved.
local go_to = {}

local KINDS = {
  location = {open = "location"},
  pop = {open = "pop"},
  character = {open = "character"},
  river = {},
}

local openers = {}

-- Registers the openers by name (`location`, `pop`, `character`); a later call replaces earlier ones of the same name.
function go_to.set_openers(table_of_openers)
  for name, opener in pairs(table_of_openers) do openers[name] = opener end
end

-- The identity `game.go_to` takes for a pop: its place and its zero-based slot.
function go_to.pop_id(place, slot)
  return {place = place, slot = slot}
end

-- Moves the camera to the thing without zooming and opens nothing: the ledger's right-click. Returns true when the camera moved, or false
-- and the engine's reason.
function go_to.camera(kind, id, revision)
  if KINDS[kind] == nil then return false, "unknown kind " .. tostring(kind) end
  if revision == nil or revision == 0 then return false, "no revision" end
  local ok, reason = pcall(game.go_to, kind, id, false, revision)
  if not ok then return false, reason end
  return true
end

-- Moves the camera to the thing with the go-to zoom and, only when that succeeded, opens the thing's own window for the same thing in the
-- same revision. Returns true when the camera moved, or false and the reason (nothing opened).
function go_to.to(kind, id, revision)
  local entry = KINDS[kind]
  if entry == nil then return false, "unknown kind " .. tostring(kind) end
  if revision == nil or revision == 0 then return false, "no revision" end
  local ok, reason = pcall(game.go_to, kind, id, true, revision)
  if not ok then return false, reason end
  local opener = entry.open ~= nil and openers[entry.open] or nil
  if opener ~= nil then opener(id, revision) end
  return true
end

-- The tag a ledger row or card hangs on its hit region (`render.set_hit_tag`) to say what a right-click on it means: the thing, the
-- revision the row was drawn from and the world, as one string (`go:<kind>:<id>:<revision>:<world>`, a pop's id `<place>#<slot>`). The
-- input link copies it into the button events routed to the region, so a right-click keeps its target however the rows change before it
-- is dispatched. Nil when the thing cannot be named (an unknown kind, an id that is not printable ASCII without spaces, no revision).
function go_to.tag(kind, id, revision, world)
  if KINDS[kind] == nil or math.type(revision) ~= "integer" or revision <= 0 or math.type(world) ~= "integer" then return nil end
  local name = id
  if kind == "pop" then
    if type(id) ~= "table" or type(id.place) ~= "string" or math.type(id.slot) ~= "integer" or id.slot < 0 then return nil end
    name = id.place .. "#" .. id.slot
  elseif kind == "character" then
    if math.type(id) ~= "integer" or id < 0 then return nil end
    name = tostring(id)
  end
  if type(name) ~= "string" or not name:match("^[%w_#%.%-]+$") then return nil end
  local tag = string.format("go:%s:%s:%d:%d", kind, name, revision, world)
  if #tag > 128 then return nil end
  return tag
end

-- What a tag of `go_to.tag` names: `kind, id, revision, world` (id as `go_to` takes it), or nil for a tag that is not one.
function go_to.parse(tag)
  local kind, name, revision, world = tostring(tag):match("^go:(%a+):([^:]+):(%d+):(%d+)$")
  if kind == nil or KINDS[kind] == nil then return nil end
  local id = name
  if kind == "pop" then
    local place, slot = name:match("^(.+)#(%d+)$")
    if place == nil then return nil end
    id = {place = place, slot = math.tointeger(slot)}
  elseif kind == "character" then
    id = math.tointeger(name)
    if id == nil then return nil end
  end
  return kind, id, math.tointeger(revision), math.tointeger(world)
end

-- What a right-click does: moves the camera, without zooming, to the thing a tag names, when the tag was made in `current_world`.
-- A tag that is not a go-to tag, a tag of another world and a thing the engine refuses do nothing. Returns true when the camera moved.
function go_to.camera_from_tag(tag, current_world)
  local kind, id, revision, world = go_to.parse(tag)
  if kind == nil or world ~= current_world then return false end
  return go_to.camera(kind, id, revision)
end

return go_to
