//! The optional gameplay profile: remove host-dependent facilities and make table order and
//! value-to-text conversions independent of Lua's hash seed and allocation addresses.

use mlua::{Function, Lua, LuaString};

/// Installs the deterministic script surface and returns the protected-call helper. It runs after
/// the host surface and extensions are installed, but before any user chunk.
pub(crate) fn install(lua: &Lua) -> mlua::Result<Function> {
    let byte_less = lua.create_function(|_, (left, right): (LuaString, LuaString)| {
        let left = left.as_bytes();
        let right = right.as_bytes();
        Ok(left.as_ref() < right.as_ref())
    })?;
    lua.globals().set("__ck_internal_byte_less", byte_less)?;
    let protected: Function = lua
        .load(BOOTSTRAP)
        .set_name("=ck-lua deterministic profile")
        .eval()?;
    Ok(protected)
}

const BOOTSTRAP: &str = r#"
local globals = _G
local native = {
  next = next, pairs = pairs, tostring = tostring, print = print,
  setmetatable = setmetatable, getmetatable = debug.getmetatable,
  load = load, xpcall = xpcall, type = type,
  rawget = rawget, rawset = rawset,
  sort = table.sort, pack = table.pack, unpack = table.unpack,
  concat = table.concat, format = string.format,
  byte_less = __ck_internal_byte_less,
  stdout = io.stdout, write = io.stdout.write,
  traceback = debug and debug.traceback,
}

local function stable_tostring(value)
  local kind = native.type(value)
  local mt = native.getmetatable(value)
  if mt ~= nil and mt.__tostring ~= nil then
    return native.tostring(value)
  end
  if kind == 'table' or kind == 'function' or kind == 'thread' or kind == 'userdata' then
    return kind
  end
  return native.tostring(value)
end
globals.__ck_internal_byte_less = nil
globals.tostring = stable_tostring

globals.print = function(...)
  local n = select('#', ...)
  local values = {}
  for i = 1, n do values[i] = stable_tostring(select(i, ...)) end
  native.write(native.stdout, native.concat(values, '\t'))
  native.write(native.stdout, '\n')
end

local key_order = { number = 1, string = 2 }
globals.pairs = function(value)
  local mt = native.getmetatable(value)
  if mt ~= nil and mt.__pairs ~= nil then
    return mt.__pairs(value)
  end
  if native.type(value) ~= 'table' then
    error("bad argument #1 to 'pairs' (table expected)", 2)
  end
  local keys = {}
  for key in native.next, value do
    local kind = native.type(key)
    if kind == 'number' then
      local number_kind = math.type(key)
      if number_kind ~= 'integer' then
        error('unsupported non-integral numeric key: ' .. stable_tostring(key), 2)
      end
    elseif kind ~= 'string' then
      error('unsupported table key (' .. kind .. '): ' .. stable_tostring(key), 2)
    end
    keys[#keys + 1] = key
  end
  native.sort(keys, function(a, b)
    local ta, tb = native.type(a), native.type(b)
    if ta ~= tb then return key_order[ta] < key_order[tb] end
    if ta == 'string' then return native.byte_less(a, b) end
    return a < b
  end)
  local index = 0
  local function iterate()
    while true do
      index = index + 1
      local key = keys[index]
      if key == nil then return nil end
      local current = native.rawget(value, key)
      if current ~= nil then return key, current end
    end
  end
  return iterate, value, nil
end

local function stable_sort(value, comp)
  if native.type(value) ~= 'table' then
    error("bad argument #1 to 'sort' (table expected, got " .. native.type(value) .. ")", 2)
  end
  if comp ~= nil and native.type(comp) ~= 'function' then
    error("bad argument #2 to 'sort' (function expected, got " .. native.type(comp) .. ")", 2)
  end
  local count = #value
  if count < 2 then return end

  local function before(left, right)
    if comp ~= nil then return comp(left, right) end
    local left_type, right_type = native.type(left), native.type(right)
    if left_type ~= right_type or (left_type ~= 'number' and left_type ~= 'string') then
      error('attempt to compare ' .. left_type .. ' with ' .. right_type, 3)
    end
    if left_type == 'string' then return native.byte_less(left, right) end
    return left < right
  end

  -- One reusable run buffer keeps the merge schedule independent of time and allocation history.
  local buffer = {}
  local width = 1
  while width < count do
    local first = 1
    while first <= count do
      local middle = math.min(first + width - 1, count)
      local last = math.min(first + width * 2 - 1, count)
      if middle < last then
        local length = last - first + 1
        for offset = 1, length do buffer[offset] = value[first + offset - 1] end
        local left = 1
        local left_end = middle - first + 1
        local right = left_end + 1
        local output = first
        while output <= last do
          if left <= left_end and (right > length or not before(buffer[right], buffer[left])) then
            value[output] = buffer[left]
            left = left + 1
          else
            value[output] = buffer[right]
            right = right + 1
          end
          output = output + 1
        end
      end
      first = last + 1
    end
    width = width * 2
  end
end

table.sort = stable_sort

local native_setmetatable = native.setmetatable
globals.setmetatable = function(value, mt)
  if mt ~= nil and native.type(mt) == 'table' and
      (native.rawget(mt, '__gc') ~= nil or native.rawget(mt, '__mode') ~= nil) then
    error("deterministic profile refuses metatable fields '__gc' and '__mode'", 2)
  end
  return native_setmetatable(value, mt)
end

globals.load = function(chunk, name, _mode, env)
  return native.load(chunk, name, 't', env)
end

-- `%s` normally calls luaL_tolstring directly and includes an allocation address. Replace only
-- those arguments after formatting has validated the format's conversion sequence.
string.format = function(format_text, ...)
  local args = native.pack(...)
  local arg = 1
  local i = 1
  while i <= #format_text do
    if format_text:sub(i, i) == '%' then
      if format_text:sub(i + 1, i + 1) == '%' then
        i = i + 2
      else
        local j = i + 1
        while j <= #format_text and format_text:sub(j, j):match('[-+ #0]') do j = j + 1 end
        while format_text:sub(j, j):match('%d') do j = j + 1 end
        if format_text:sub(j, j) == '.' then
          j = j + 1
          while format_text:sub(j, j):match('%d') do j = j + 1 end
        end
        while j <= #format_text and not format_text:sub(j, j):match('[cdiouxXeEfgGqsaAp]') do j = j + 1 end
        local conversion = format_text:sub(j, j)
        if conversion == '' then break end
        if conversion == 'p' then error("string.format does not support '%p' in deterministic profile", 2) end
        if conversion == 's' then args[arg] = stable_tostring(args[arg]) end
        arg = arg + 1
        i = j + 1
      end
    else
      i = i + 1
    end
  end
  return native.format(format_text, native.unpack(args, 1, args.n))
end

local log = globals.log
for _, name in ipairs({'error', 'warn', 'info', 'debug', 'trace'}) do
  local emit = log[name]
  log[name] = function(value) return emit(stable_tostring(value)) end
end

globals.os, globals.io, globals.debug = nil, nil, nil
globals.loadfile, globals.dofile, globals.require, globals.package = nil, nil, nil, nil
globals.collectgarbage, globals.next = nil, nil
math.random, math.randomseed = nil, nil
string.dump = nil

local function normalize_error(value)
  local kind = native.type(value)
  if kind == 'string' or kind == 'number' then return value end
  local mt = native.getmetatable(value)
  local method = mt and mt.__tostring
  if native.type(method) == 'function' then
    local ok, result = pcall(method, value)
    if ok and native.type(result) == 'string' then return result end
  end
  return '(error object is a ' .. kind .. ' value)'
end

local function message_handler(value)
  local message = normalize_error(value)
  if native.traceback then return native.traceback(message, 2) end
  return message .. '\nstack traceback:\n\t[C]: in ck-lua (no Lua frame was running)'
end

-- The helper keeps Lua values intact for successful calls. On failure it returns a normalized
-- string with Lua's traceback, so mlua never formats a non-string error object itself.
return function(fn, ...)
  local result = native.pack(native.xpcall(fn, message_handler, ...))
  if not result[1] then return false, result[2] end
  return true, native.unpack(result, 2, result.n)
end
"#;
