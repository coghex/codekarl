-- Shared tooltip definitions, hover targets, rendering and input. All geometry is in the UI units supplied by frame_update.
local ui = require("lib.ui")
local text = require("lib.text")
local panel = require("lib.panel")
local pointer = require("lib.pointer")
local theme = require("lib.theme")

local P = ui.P
local current_settings = settings.tooltips()
local tooltip = {}
local overlay
local definitions, attached, open = {}, {}, {}
local serial = 0
local next_span_id, span_ids_exhausted = "0", false
local last_hover
local view = {width = 0, height = 0}
local render_record, record_scroll
local close_key

local function fail(message, level)
  error("tooltip: " .. message, (level or 1) + 1)
end

-- Span keys have a fixed prefix and a decimal suffix whose length is explicitly capped at 35 digits (13 + 35 = 48).
-- The counter is a string so it remains exact beyond Lua's integer/number precision; exhaustion refuses before reuse.
local function increment_decimal(value)
  local digits, carry = {}, 1
  for index = #value, 1, -1 do
    local digit = value:byte(index) - 48 + carry
    if digit == 10 then
      digits[index] = "0"
    else
      digits[index] = string.char(48 + digit)
      carry = 0
    end
  end
  local result = table.concat(digits)
  return carry == 1 and ("1" .. result) or result
end

local function allocate_span_key()
  if span_ids_exhausted then fail("span hit key space exhausted", 2) end
  local id = next_span_id
  if #id > 35 then fail("span hit key space exhausted", 2) end
  next_span_id = increment_decimal(id)
  span_ids_exhausted = #next_span_id > 35
  return "tooltip_span_" .. id
end

function tooltip.define(class, fn)
  if type(class) ~= "string" or class == "" then fail("define: class must be a non-empty string", 2) end
  if type(fn) ~= "function" then fail("define: definition for `" .. class .. "` must be a function", 2) end
  if definitions[class] ~= nil then fail("class `" .. class .. "` is already defined", 2) end
  definitions[class] = fn
end

function tooltip.content(class, args)
  local fn = definitions[class]
  if fn == nil then fail("class `" .. tostring(class) .. "` is not defined", 2) end
  return fn(args or {})
end

local function resolve(source, x, y)
  if type(source) == "function" then return source(x, y) end
  if type(source) ~= "table" or type(source.class) ~= "string" then
    fail("source must be a function or {class, args}", 3)
  end
  return tooltip.content(source.class, source.args)
end

function tooltip.attach(key, source)
  if type(key) ~= "string" or type(source) ~= "table" and type(source) ~= "function" then
    fail("attach: expected a key and a source", 2)
  end
  local previous = attached[key]
  if previous ~= nil and previous.kind == "label" then close_key(key) end
  attached[key] = {source = source, kind = "lockable"}
end

function tooltip.label(key, words)
  if type(key) ~= "string" or type(words) ~= "string" or words == "" then
    fail("label: expected a key and non-empty words", 2)
  end
  local previous = attached[key]
  if previous ~= nil and previous.kind ~= "label" then close_key(key) end
  attached[key] = {kind = "label", words = words}
end

function tooltip.detach(key)
  local previous = attached[key]
  if previous ~= nil and previous.kind == "label" then close_key(key) end
  attached[key] = nil
end

function tooltip.map_hover(hover, source)
  last_hover = hover and {hover = hover, source = source} or nil
end

local function plain_span(value, font_name, colour)
  return {text = value, font = font_name or "small", colour = colour, plain = true}
end

local function normalize_line(line, title)
  if type(line) == "string" then
    return {spans = {plain_span(line, "small", title and theme.kind("tooltip").title_text or nil)}, title = title}
  end
  if type(line) ~= "table" then fail("content lines must be strings or span arrays", 3) end
  local spans = {}
  for _, value in ipairs(line) do
    if type(value) == "string" then
      spans[#spans + 1] = plain_span(value)
    elseif type(value) == "table" and type(value.text) == "string" then
      local font_name = value.font or "small"
      if font_name ~= "small" and font_name ~= "tiny" then fail("span font must be small or tiny", 3) end
      local term = value.term
      local class = value.class
      local fn = value.tooltip
      local over_limit = value.over_limit
      if over_limit ~= nil and type(over_limit) ~= "boolean" then fail("span over_limit must be a boolean", 3) end
      local links = (term ~= nil and 1 or 0) + (class ~= nil and 1 or 0) + (fn ~= nil and 1 or 0)
      if links > 1 then fail("a span may name only one of term, class or tooltip", 3) end
      if class ~= nil and type(class) ~= "string" then fail("span class must be a string", 3) end
      if fn ~= nil and type(fn) ~= "function" then fail("span tooltip must be a function", 3) end
      if term ~= nil and type(term) ~= "string" then fail("span term must be a string", 3) end
      spans[#spans + 1] = {text = value.text, font = font_name, term = term, class = class,
        args = value.args, tooltip = fn, linked = links > 0, over_limit = over_limit == true}
    else
      fail("span must be a string or a table with string text", 3)
    end
  end
  return {spans = spans, title = title}
end

local function normalize_content(content)
  if type(content) ~= "table" then fail("content must be a table of lines", 3) end
  local lines = {}
  if content.title ~= nil then
    if type(content.title) ~= "string" then fail("content title must be a string", 3) end
    lines[#lines + 1] = normalize_line(content.title, true)
  end
  for _, line in ipairs(content) do lines[#lines + 1] = normalize_line(line, false) end
  return lines
end

local function line_measure(line)
  local width, height = 0, 0
  for _, span in ipairs(line.spans) do
    local laid = font.layout(span.text, span.font)
    span.layout = laid
    if line.title then span.colour = theme.kind("tooltip").title_text
    elseif span.term ~= nil then span.colour = theme.colour("term")
    else span.runs = text.number_runs(span.text) end
    width = width + laid.width
    height = math.max(height, laid.height)
  end
  line.width = width
  line.height = height
  return width, height
end

local function measure(lines)
  local width, height = 0, 0
  for _, line in ipairs(lines) do
    local w, h = line_measure(line)
    width = math.max(width, w)
    height = height + h
  end
  return width, height
end

local function measurable_content(content)
  local normalized, visible = normalize_content(content), {}
  for _, line in ipairs(normalized) do
    local _, height = line_measure(line)
    if height > 0 then visible[#visible + 1] = line end
  end
  if #visible == 0 then return nil end
  return visible
end

local function destroy_visual(record)
  for _, line in ipairs(record.rendered or {}) do
    text.destroy(line)
  end
  record.rendered = {}
end

local function quad(record, name, colour, x, y, width, height)
  local state = record[name]
  if width <= 0 or height <= 0 then
    if state ~= nil then render.destroy_quad(state.quad); record[name] = nil end
    return
  end
  if state == nil then
    record[name] = {quad = render.create_quad{texture = ui.texture("ui/theme/white.png"), x = x, y = y,
      width = width, height = height, group = overlay, tint = colour}, x = x, y = y, width = width, height = height, colour = colour}
  else
    if state.x ~= x or state.y ~= y or state.width ~= width or state.height ~= height then
      render.set_rect(state.quad, x, y, width, height)
    end
    if state.colour ~= colour then render.set_tint(state.quad, colour) end
    state.x, state.y, state.width, state.height, state.colour = x, y, width, height, colour
  end
end

local function unregister_hit(key)
  pointer.unregister(key)
  render.set_hit(key, nil)
end

local function close_record(record)
  if record.closed then return end
  record.closed = true
  local children = {}
  for _, item in ipairs(open) do if item.parent == record then children[#children + 1] = item end end
  for i = #children, 1, -1 do close_record(children[i]) end
  for _, key in ipairs(record.hit_keys or {}) do attached[key] = nil; unregister_hit(key) end
  record.hit_keys = {}
  if record.bar_quad ~= nil then render.destroy_quad(record.bar_quad); record.bar_quad = nil end
  if record.track_quad ~= nil then render.destroy_quad(record.track_quad.quad); record.track_quad = nil end
  if record.thumb_quad ~= nil then render.destroy_quad(record.thumb_quad.quad); record.thumb_quad = nil end
  destroy_visual(record)
  if record.panel ~= nil then record.panel:destroy(); record.panel = nil end
  for index, item in ipairs(open) do
    if item == record then table.remove(open, index); break end
  end
end

local function close_children(record)
  local children = {}
  for _, item in ipairs(open) do if item.parent == record then children[#children + 1] = item end end
  for index = #children, 1, -1 do close_record(children[index]) end
end

local function close_all()
  while #open > 0 do close_record(open[#open]) end
end

-- Closes every open tooltip, locked or not: a source that follows a list row's position would otherwise show another item's data.
function tooltip.close_all()
  tooltip.pending = nil
  close_all()
end

close_key = function(key)
  if tooltip.pending ~= nil and tooltip.pending.candidate.key == key then tooltip.pending = nil end
  local snapshot = {}
  for _, record in ipairs(open) do
    if record.key == key then snapshot[#snapshot + 1] = record end
  end
  for index = #snapshot, 1, -1 do close_record(snapshot[index]) end
end

function tooltip.close_top()
  for index = #open, 1, -1 do
    if open[index].kind ~= "label" then
      close_record(open[index])
      return true
    end
  end
  return false
end

local function wraps(text_value, key, entries)
  local chunks, index = {}, 1
  while index <= #text_value do
    local open_at = text_value:find("[{}]", index)
    if open_at == nil then chunks[#chunks + 1] = {text = text_value:sub(index)}; break end
    if open_at > index then chunks[#chunks + 1] = {text = text_value:sub(index, open_at - 1)} end
    local pair = text_value:sub(open_at, open_at + 1)
    if pair == "{{" then
      chunks[#chunks + 1] = {text = "{"}; index = open_at + 2
    elseif pair == "}}" then
      chunks[#chunks + 1] = {text = "}"}; index = open_at + 2
    elseif text_value:sub(open_at, open_at) == "}" then
      chunks[#chunks + 1] = {text = "}"}; index = open_at + 1
    else
      local close_at = text_value:find("}", open_at + 1, true)
      if close_at == nil then fail("glossary entry `" .. key .. "` has unclosed markup", 3) end
      local body = text_value:sub(open_at + 1, close_at - 1)
      local term, words = body:match("^([^|]+)|?(.*)$")
      local entry = entries.entry(term)
      if entry == nil then fail("glossary entry `" .. key .. "` refers to missing term `" .. term .. "`", 3) end
      chunks[#chunks + 1] = {text = words ~= "" and words or entry.title, term = term}
      index = close_at + 1
    end
  end
  local words = {}
  for _, chunk in ipairs(chunks) do
    local cursor = 1
    while cursor <= #chunk.text do
      local split = chunk.text:find("%s+", cursor)
      if split == nil then words[#words + 1] = {text = chunk.text:sub(cursor), term = chunk.term}; break end
      if split > cursor then words[#words + 1] = {text = chunk.text:sub(cursor, split - 1), term = chunk.term} end
      local finish = chunk.text:find("%S", split) or (#chunk.text + 1)
      words[#words + 1] = {text = chunk.text:sub(split, finish - 1), term = chunk.term, space = true}
      cursor = finish
    end
  end
  local lines, current, current_width = {}, {}, 0
  local max_width = 160
  local function flush()
    lines[#lines + 1] = current
    current, current_width = {}, 0
  end
  for _, word in ipairs(words) do
    local width = font.layout(word.text, "small").width
    if current_width > 0 and current_width + width > max_width and not word.space then flush() end
    if word.text ~= "" then
      current[#current + 1] = word.term and {text = word.text, term = word.term} or word.text
      current_width = current_width + width
    end
  end
  if #current > 0 or #lines == 0 then flush() end
  return lines
end

local function term_content(key)
  local entry = glossary.entry(key)
  if entry == nil then return nil end
  local lines = wraps(entry.text, key, glossary)
  return {title = entry.title, table.unpack(lines)}
end

function tooltip.explanation(key)
  return term_content(key)
end

local function span_source(span)
  if span.term ~= nil then return function() return term_content(span.term) end end
  if span.class ~= nil then return {class = span.class, args = span.args} end
  if span.tooltip ~= nil then return span.tooltip end
  return nil
end

local function draw_span(record, span, x, y, title)
  local color = title and theme.kind("tooltip").title_text or span.colour
  -- A label, a title and a term are drawn whole, in one colour: only a lockable tooltip marks numbers.
  local whole = title or span.term ~= nil or record.kind == "label"
  local pieces = whole and {{text = span.text, number = false}} or (span.runs or {})
  local width = 0
  for _, piece in ipairs(pieces) do
    local ink = color or (piece.number and (span.over_limit and theme.colour("over_limit") or theme.colour("number"))
      or theme.kind("tooltip").text)
    record.render_cursor = record.render_cursor + 1
    local line = record.rendered[record.render_cursor]
    if line == nil or line.font_name ~= span.font then
      if line ~= nil then text.destroy(line) end
      line = text.line(x + width * P, y, overlay, ink, span.font)
      record.rendered[record.render_cursor] = line
    else
      text.move(line, x + width * P, y)
      text.set_colour(line, ink)
    end
    text.set(line, piece.text)
    width = width + line.width
  end
  return width
end

local function update_lock_bar(record)
  if record.kind == "label" then return end
  local fill = current_settings.lock == "timer" and math.min(1, record.lock_elapsed / current_settings.lock_timer) or 0
  if record.locked then fill = 1 end
  local width_px, height_px = record.width_px, record.height_px
  local run = math.floor((width_px - 2) * fill + 0.5)
  -- The lower-right cut corner remains outline, so the bar ends before its three cut pixels.
  run = math.min(run, width_px - 4)
  if current_settings.lock == "disabled" or current_settings.lock == "middle_mouse" and not record.locked then run = 0 end
  if run > 0 then
    local bar = record.bar_quad
    if bar == nil then
      record.bar_quad = render.create_quad{texture = ui.texture("ui/theme/white.png"), x = record.x + P,
        y = record.y + (height_px - 2) * P, width = run * P, height = P, group = overlay, tint = theme.colour("lock")}
      record.bar_x, record.bar_y, record.bar_width = record.x + P, record.y + (height_px - 2) * P, run * P
    else
      local x, y, width = record.x + P, record.y + (height_px - 2) * P, run * P
      if x ~= record.bar_x or y ~= record.bar_y or width ~= record.bar_width then
        render.set_rect(bar, x, y, width, P)
        record.bar_x, record.bar_y, record.bar_width = x, y, width
      end
    end
  elseif record.bar_quad ~= nil then
    render.destroy_quad(record.bar_quad); record.bar_quad = nil
    record.bar_x, record.bar_y, record.bar_width = nil, nil, nil
  end
end

local function stable_value(value)
  if type(value) ~= "table" then return type(value) .. ":" .. tostring(value) end
  local keys = {}
  for key in pairs(value) do keys[#keys + 1] = key end
  table.sort(keys, function(left, right) return tostring(left) < tostring(right) end)
  local parts = {}
  for _, key in ipairs(keys) do
    parts[#parts + 1] = stable_value(key) .. "=" .. stable_value(value[key])
  end
  return "{" .. table.concat(parts, ";") .. "}"
end

local function span_target(span)
  if span.term ~= nil then return "term:" .. span.term end
  if span.class ~= nil then return "class:" .. span.class .. ":" .. stable_value(span.args or {}) end
  return "tooltip:" .. span.text
end

local function draw_line(record, line, index, y, title)
  local x = record.x + 3 * P
  local total = 0
  for span_index, span in ipairs(line.spans) do
    local span_x = x + total * P
    local width = draw_span(record, span, span_x, y, title or line.title)
    if span.linked then
      local slot = index .. ":" .. span_index
      local hit = {slot = slot, signature = span_target(span), x = span_x, y = y,
        width = math.max(P, width * P), height = line.height * P,
        source = span_source(span), parent = record, anchor_x = span_x, anchor_y = y + line.height * P}
      record.spans[#record.spans + 1] = hit
    end
    total = total + width
  end
end

local function clear_hits(record)
  for _, key in ipairs(record.hit_keys or {}) do
    attached[key] = nil
    unregister_hit(key)
  end
  record.hit_keys = {}
  record.panel_hit = false
  record.span_slots = {}
end

local function register_hit_widget(record, hit)
  local widget = {}
  function widget:on_event() end
  function widget:on_hover() end
  function widget:on_overflow() end
  pointer.register(hit.key, widget)
  record.hit_keys[#record.hit_keys + 1] = hit.key
end

local function publish_span_hits(record)
  record.span_slots = record.span_slots or {}
  local visible = {}
  for _, hit in ipairs(record.spans) do
    visible[hit.slot] = true
    local slot = record.span_slots[hit.slot]
    if slot ~= nil and slot.signature ~= hit.signature then
      attached[slot.key] = nil
      unregister_hit(slot.key)
      record.span_slots[hit.slot] = nil
      slot = nil
    end
    if slot == nil then
      slot = {key = allocate_span_key(), signature = hit.signature}
      record.span_slots[hit.slot] = slot
      hit.key = slot.key
      register_hit_widget(record, hit)
    else
      hit.key = slot.key
    end
    render.set_hit(hit.key, {{x = hit.x, y = hit.y, width = hit.width, height = hit.height}}, overlay)
    attached[hit.key] = {source = hit.source, parent = record, anchor_x = hit.anchor_x, anchor_y = hit.anchor_y}
  end
  for key, slot in pairs(record.span_slots) do
    if not visible[key] then
      attached[slot.key] = nil
      unregister_hit(slot.key)
      record.span_slots[key] = nil
    end
  end
end

record_scroll = function(record, event)
  if not record.locked or not record.long or event.amount == nil then return end
  local delta = event.unit == "points" and -event.amount / (9 * P) or -event.amount
  record.carry = record.carry + delta
  local whole = record.carry >= 1 and math.floor(record.carry) or record.carry <= -1 and math.ceil(record.carry) or 0
  local last = math.max(1, #record.lines - 11)
  local next_first = math.max(1, math.min(last, record.first + whole))
  if next_first ~= record.first then
    record.first = next_first
    if next_first == 1 and delta < 0 or next_first == last and delta > 0 then record.carry = 0
    else record.carry = record.carry - whole end
    render_record(record, true)
  elseif whole ~= 0 then
    record.carry = 0
  end
end

render_record = function(record, keep_panel)
  if record.closed then return end
  local width, natural_height = measure(record.lines)
  record.long = #record.lines > 12
  local width_px = math.max(9, width + (record.long and 10 or 6))
  local height_px = record.long and 112 or natural_height + 4
  record.width_px, record.height_px = width_px, height_px
  if record.panel == nil then
    record.panel = panel.new{x = record.x, y = record.y, width = width_px * P, height = height_px * P,
      kind = "tooltip", cut = 3, group = overlay}
  elseif not keep_panel or record.panel.width ~= width_px * P or record.panel.height ~= height_px * P then
    record.panel:set_rect(record.x, record.y, width_px * P, height_px * P)
  end
  record.x, record.y = record.panel.x, record.panel.y
  record.render_cursor = 0
  record.spans = {}
  local last = record.long and math.min(#record.lines, record.first + 11) or #record.lines
  local y = record.y + 2 * P
  for index = record.first, last do
    local line = record.lines[index]
    draw_line(record, line, index, y, line.title)
    y = y + (record.long and 9 or line.height) * P
  end
  for index = record.render_cursor + 1, #record.rendered do
    text.destroy(record.rendered[index])
    record.rendered[index] = nil
  end
  if record.long then
    local track_top, track = 3, 105
    local thumb = math.max(3, math.floor(track * 12 / #record.lines + 0.5))
    local top = track_top + math.floor((track - thumb) * (record.first - 1) / (#record.lines - 12) + 0.5)
    quad(record, "track_quad", theme.colour("dim"), record.x + (width_px - 4) * P, record.y + track_top * P, P, track * P)
    quad(record, "thumb_quad", theme.kind("tooltip").text, record.x + (width_px - 5) * P,
      record.y + top * P, 3 * P, thumb * P)
  else
    quad(record, "track_quad", theme.colour("dim"), 0, 0, 0, 0)
    quad(record, "thumb_quad", theme.kind("tooltip").text, 0, 0, 0, 0)
  end
  if record.locked then
    local full_rect = {{x = record.x, y = record.y, width = width_px * P, height = height_px * P}}
    if not record.panel_hit then
      local widget = {}
      function widget:on_event() end
      function widget:on_hover() end
      function widget:on_overflow() end
      pointer.register(record.panel_key, widget)
      record.hit_keys[#record.hit_keys + 1] = record.panel_key
      record.panel_hit = true
    end
    render.set_hit(record.panel_key, full_rect, overlay)
    publish_span_hits(record)
  else
    clear_hits(record)
  end
  update_lock_bar(record)
end

local function place(record, candidate)
  local width, height = record.width_px * P, record.height_px * P
  local x, y
  if candidate.map then
    local hover = candidate.hover
    x = hover.right + 4
    if x + width > view.width and hover.left - 4 - width >= 0 then x = hover.left - 4 - width end
    y = hover.y - height / 2
  elseif candidate.anchor_x ~= nil then
    x, y = candidate.anchor_x, candidate.anchor_y + 2 * P
  else
    x, y = candidate.x + 6 * P, candidate.y + 12 * P
  end
  x = math.max(0, math.min(x, math.max(0, view.width - width)))
  y = math.max(0, math.min(y, math.max(0, view.height - height)))
  record.x, record.y = ui.snap(x), ui.snap(y)
  record.panel:set_rect(record.x, record.y, width, height)
end

local function open_tooltip(candidate, content)
  if type(content) == "table" and #content == 0 and content.title == nil then return nil end
  local lines = measurable_content(content)
  if lines == nil then return nil end
  if overlay == nil then
    overlay = ui.create_group()
    ui.set_top_group(overlay)
  end
  local record = {target = candidate.target, key = candidate.key, kind = candidate.kind or "lockable",
    words = candidate.words, parent = candidate.parent, source = candidate.source,
    map = candidate.map, hover = candidate.hover, anchor_x = candidate.anchor_x, anchor_y = candidate.anchor_y,
    source_x = candidate.x, source_y = candidate.y,
    lines = lines, locked = false, first = 1, carry = 0,
    hit_keys = {}, span_slots = {}, spans = {}, rendered = {},
    x = candidate.x + 6 * P, y = candidate.y + 12 * P}
  if record.kind ~= "label" then record.lock_elapsed = 0 end
  if serial >= 9007199254740991 then fail("tooltip panel key space exhausted", 2) end
  serial = serial + 1
  record.panel_key = "tooltip_panel_" .. serial
  local width, height = measure(record.lines)
  record.long = #record.lines > 12
  record.width_px = math.max(9, width + (record.long and 10 or 6))
  record.height_px = record.long and 112 or height + 4
  record.panel = panel.new{x = record.x, y = record.y, width = record.width_px * P,
    height = record.height_px * P, kind = "tooltip", cut = 3, group = overlay}
  place(record, candidate)
  open[#open + 1] = record
  render_record(record, true)
  return record
end

local function refresh_record(record)
  if record.closed then return true end
  if record.kind == "label" then
    local target = attached[record.key]
    if target == nil or target.kind ~= "label" then
      close_record(record)
      return false
    end
    if record.words ~= target.words then
      record.words = target.words
      record.lines = measurable_content({target.words})
      render_record(record, true)
    end
    return true
  end
  local content = resolve(record.source, record.source_x or 0, record.source_y or 0)
  if content == nil or type(content) == "table" and #content == 0 and content.title == nil then
    close_record(record)
    return false
  end
  record.lines = measurable_content(content)
  if record.lines == nil then
    close_record(record)
    return false
  end
  local max_first = math.max(1, #record.lines - 11)
  record.first = math.min(record.first, max_first)
  record.carry = 0
  render_record(record, true)
  return true
end

function tooltip.refresh()
  local snapshot = {}
  for _, record in ipairs(open) do snapshot[#snapshot + 1] = record end
  for _, record in ipairs(snapshot) do refresh_record(record) end
end

local function candidate_of(update)
  local key = update.pointer and update.pointer.over or false
  local x = update.pointer and update.pointer.x or 0
  local y = update.pointer and update.pointer.y or 0
  if key and attached[key] ~= nil then
    local target = attached[key]
    local content, part
    if target.kind == "label" then content = {target.words}
    elseif type(target.source) == "function" then content, part = target.source(x, y)
    else content = resolve(target.source, x, y) end
    if content == nil or type(content) == "table" and #content == 0 and content.title == nil then return nil end
    return {target = key .. "\0" .. tostring(part or ""), source = target.source, content = content,
      key = key, kind = target.kind or "lockable", words = target.words, part = part,
      parent = target.parent, x = x, y = y, anchor_x = target.anchor_x, anchor_y = target.anchor_y}
  end
  if not key and last_hover ~= nil and last_hover.source ~= nil then
    local hover = last_hover.hover
    local content = resolve(last_hover.source, x, y)
    if content ~= nil then return {target = "map\0" .. tostring(hover.id), source = last_hover.source,
      content = content, map = true, hover = hover, x = x, y = y} end
  end
  return nil
end

local function process_events(events)
  for _, event in ipairs(events or {}) do
    if event.kind == "map_down" then
      close_all()
    elseif event.kind == "button_down" and event.button == "middle" and current_settings.lock == "middle_mouse" then
      for index = #open, 1, -1 do
        if not open[index].locked and open[index].kind ~= "label" then
          open[index].locked = true
          open[index].lock_elapsed = current_settings.lock_timer
          render_record(open[index], true)
          break
        end
      end
    elseif event.kind == "down" then
      if event.functional == true then
        -- Functional actions keep every tooltip open.
      else
        local target_record
        for _, record in ipairs(open) do
          if event.key == record.panel_key then target_record = record end
          for _, hit in ipairs(record.spans) do if event.key == hit.key then target_record = record end end
        end
        if target_record ~= nil then close_children(target_record) else close_all() end
      end
    elseif event.kind == "scroll" then
      -- The locked panel/span widgets consume the scroll during pointer.dispatch.
    end
  end
end

function tooltip.update(update)
  if update.width ~= nil then view.width = update.width end
  if update.height ~= nil then view.height = update.height end
  local pointer_state = update.pointer or {}
  local events = pointer_state.overflow and {} or (pointer_state.events or {})
  process_events(events)
  for _, event in ipairs(events) do
    if event.kind == "scroll" then
      for _, record in ipairs(open) do
        local target = event.key == record.panel_key
        for _, hit in ipairs(record.spans) do target = target or event.key == hit.key end
        if target then record_scroll(record, event); break end
      end
    end
  end
  local candidate = candidate_of(update)
  if candidate ~= nil then
    local already_open = false
    for _, record in ipairs(open) do
      if record.target == candidate.target then already_open = true; break end
    end
    if not already_open and (tooltip.pending == nil or tooltip.pending.target ~= candidate.target) then
      current_settings = settings.tooltips()
    end
  end
  local active
  for _, record in ipairs(open) do if not record.locked then active = record end end
  if active ~= nil and (candidate == nil or candidate.target ~= active.target) then close_record(active); active = nil end
  for _, record in ipairs(open) do
    if not record.locked and candidate ~= nil and record.target == candidate.target then
      record.source_x, record.source_y = candidate.x, candidate.y
      if record.kind == "label" then
        if record.words ~= candidate.words then
          record.words = candidate.words
          record.lines = measurable_content({candidate.words})
          render_record(record, true)
        end
      else
        record.lock_elapsed = record.lock_elapsed + (update.dt or 0)
        if current_settings.lock == "timer" and record.lock_elapsed >= current_settings.lock_timer then
          record.locked = true
          render_record(record, true)
        else
          update_lock_bar(record)
        end
      end
      active = record
    end
  end
  if candidate == nil then tooltip.pending = nil; return end
  for _, record in ipairs(open) do if record.target == candidate.target then tooltip.pending = nil; return end end
  if active ~= nil and active.target == candidate.target then return end
  local target = candidate.target
  if target == nil then return end
  if tooltip.pending == nil or tooltip.pending.target ~= target then
    tooltip.pending = {target = target, candidate = candidate, elapsed = update.dt or 0}
  else
    tooltip.pending.elapsed = tooltip.pending.elapsed + (update.dt or 0)
    tooltip.pending.candidate = candidate
  end
  if tooltip.pending.elapsed >= current_settings.delay then
    local content = tooltip.pending.candidate.content
    if content ~= nil then open_tooltip(tooltip.pending.candidate, content) end
    tooltip.pending = nil
  end
end

return tooltip
