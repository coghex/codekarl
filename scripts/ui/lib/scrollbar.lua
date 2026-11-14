-- The scroll bar of a scrolling list (CK-299, UI polish item 3; docs/ui-design.md §3.2): a thin bar beside the rows that shows where the
-- view is in the content and drags it. The scroll list, the card stack and the list bar's picker use this one piece.
--
-- **Look (B2).** A track 3 UI pixels wide at the right edge of the view and as tall as the view, in the theme's `track`; on it a thumb
-- of the same width in `dim`, and in `text` while the pointer is over it or it is dragged. Plain rectangles of `white.png`. The bar is
-- shown only while the content is taller than the view. Its rows stop 2 pixels short of the track, so while it is shown a row is 5
-- pixels narrower than the view: `scrollbar.row_width` is that width, and a caller that lays its own columns out asks for it.
--
-- **Thumb.** Its length is the shown share of the content, view * view / content, rounded to a whole pixel, at least 9 pixels and
-- never longer than the view. Its top is travel * scroll / greatest scroll, rounded to a whole pixel, where the travel is the view less
-- the thumb. A view too short to give the thumb any travel has a thumb that fills the track; nothing divides by the travel.
--
-- **Use.** Dragging the thumb keeps the point that was grabbed under the pointer, clamped to the ends; a press on the track above or
-- below the thumb moves one view that way; the wheel over the bar scrolls the owner. A drag ends at the release, at a `cancel`, at a
-- pointer overflow, and when the bar goes or is destroyed.
--
-- **Hit regions.** The bar has a group of its own that does not scroll, above the owner's rows. Its two keys come from `ui.key`: the
-- thumb's region is the thumb, and the track's regions are the parts of the track above and below it, so the two never overlap. A bar
-- that is not shown has no region, and neither does one whose view the window's content has cut away. Both widgets are functional.
--
-- The owner (`spec.owner`) gives `scroll()`, the displayed scroll in points, `set_scroll(pos)` and `wheel(event)`. It calls `update`
-- when the view, the content or the scroll changed, and `place` when its absolute position or the window's content clip did.
local ui = require("lib.ui")
local theme = require("lib.theme")
local pointer = require("lib.pointer")

local P = ui.P
local WHITE = "ui/theme/white.png"
local TRACK = 3 * P
local RESERVE = 5 * P
local LEAST = 9 * P

local scrollbar = {}
scrollbar.WIDTH = TRACK
scrollbar.RESERVE = RESERVE
scrollbar.LEAST = LEAST

local Bar = {}
Bar.__index = Bar

local function clamp(value, low, high)
  return math.max(low, math.min(value, high))
end

-- The width of the rows of a list `width` points wide whose view is `view` points high over `content` points of rows: the whole
-- width, or 5 pixels less while the content is taller than the view and the bar stands there.
function scrollbar.row_width(width, view, content)
  if content > view then
    return math.max(P, width - RESERVE)
  end
  return width
end

-- The thumb for a view over a taller content at `scroll`: its length, its top and the travel it has.
function scrollbar.thumb(view, content, scroll)
  local length = math.min(view, math.max(LEAST, ui.snap(view * view / content)))
  local travel = view - length
  local top = 0
  if travel > 0 then
    local greatest = content - view
    top = math.min(travel, ui.snap(travel * clamp(scroll, 0, greatest) / greatest))
  end
  return length, top, travel
end

local function look(self)
  if self.thumb_quad ~= nil then
    local lit = self.hover or self.dragging
    local colour = theme.colour(lit and "text" or "dim")
    if colour ~= self.shown_colour then
      render.set_tint(self.thumb_quad, colour)
      self.shown_colour = colour
    end
  end
end

local function end_drag(self)
  if self.dragging then
    self.dragging = false
    pointer.pressed(self.thumb_key, false)
    look(self)
  end
end

-- Hides the bar: its quads and regions go, and a drag in progress ends.
local function hide(self)
  end_drag(self)
  self.hover = false
  if self.track_quad ~= nil then
    render.destroy_quad(self.track_quad)
    render.destroy_quad(self.thumb_quad)
    self.track_quad, self.thumb_quad, self.shown_colour = nil, nil, nil
  end
  render.set_hit(self.track_key, nil, self.group)
  render.set_hit(self.thumb_key, nil, self.group)
  pointer.unregister(self.track_key)
  pointer.unregister(self.thumb_key)
  self.shown_rects = nil
  self.shown = false
  render.set_group_visible(self.group, false)
  self.group_visible = false
end

-- Puts the group where the owner's position and the window's clip put it. A view the clip leaves nothing of hides the group.
local function apply_place(self)
  if not self.shown or self.abs_x == nil then
    return
  end
  if self.abs_x ~= self.offset_x or self.abs_y ~= self.offset_y then
    render.set_group_offset(self.group, self.abs_x, self.abs_y)
    self.offset_x, self.offset_y = self.abs_x, self.abs_y
  end
  local left, top, right, bottom = self.abs_x, self.abs_y, self.abs_x + TRACK, self.abs_y + self.view
  local clip = self.content_clip
  if clip ~= nil then
    left, top = math.max(left, clip.x), math.max(top, clip.y)
    right, bottom = math.min(right, clip.x + clip.width), math.min(bottom, clip.y + clip.height)
  end
  if right <= left or bottom <= top then
    if self.group_visible ~= false then
      render.set_group_visible(self.group, false)
      self.group_visible = false
    end
    return
  end
  if left ~= self.clip_left or top ~= self.clip_top or right ~= self.clip_right or bottom ~= self.clip_bottom then
    render.set_group_clip(self.group, {x = left, y = top, width = right - left, height = bottom - top})
    self.clip_left, self.clip_top, self.clip_right, self.clip_bottom = left, top, right, bottom
  end
  if self.group_visible ~= true then
    render.set_group_visible(self.group, true)
    self.group_visible = true
  end
end

-- The scroll the thumb dragged to `y` (absolute points) stands for, given to the owner.
local function drag_to(self, y)
  if self.travel <= 0 then
    return
  end
  local want = clamp(y - self.abs_y - self.grab, 0, self.travel)
  self.owner.set_scroll(self.greatest * want / self.travel)
end

local function thumb_widget(self)
  local widget = {}
  function widget.on_event(_, event, eligible)
    if self.closed then
      return
    end
    local kind = event.kind
    if kind == "scroll" then
      self.owner.wheel(event)
    elseif kind == "down" then
      if self.shown and eligible ~= false and not self.dragging then
        self.dragging = true
        self.grab = event.y - ((self.abs_y or 0) + self.top)
        pointer.pressed(self.thumb_key, true)
        look(self)
      end
    elseif kind == "move" then
      if self.dragging and eligible ~= false then
        drag_to(self, event.y)
      end
    elseif kind == "up" then
      if self.dragging then
        if eligible ~= false then
          drag_to(self, event.y)
        end
        end_drag(self)
      end
    elseif kind == "cancel" then
      end_drag(self)
    end
  end
  function widget.on_hover(_, hovered)
    if self.closed then
      return
    end
    self.hover = hovered == true
    look(self)
  end
  function widget.on_overflow()
    end_drag(self)
  end
  function widget.functional()
    return true
  end
  return widget
end

local function track_widget(self)
  local widget = {}
  function widget.on_event(_, event, eligible)
    if self.closed then
      return
    end
    if event.kind == "scroll" then
      self.owner.wheel(event)
    elseif event.kind == "down" and self.shown and eligible ~= false then
      local scroll = self.owner.scroll()
      if event.y < (self.abs_y or 0) + self.top then
        self.owner.set_scroll(scroll - self.view)
      else
        self.owner.set_scroll(scroll + self.view)
      end
    end
  end
  function widget.on_hover() end
  function widget.on_overflow() end
  function widget.functional()
    return true
  end
  return widget
end

-- `scrollbar.new{window, owner = {scroll, set_scroll, wheel}}`. The bar starts hidden and has nothing registered; `bar.group` is its
-- group, which the owner adds to the groups it gives its window, after its rows' own. Its two hit keys (`bar.track_key` and
-- `bar.thumb_key`, nil until the bar is first shown) are made when it is first shown, and registered for as long as it is shown.
function scrollbar.new(spec)
  local owner = spec.owner
  local self = setmetatable({}, Bar)
  self.window = spec.window
  self.owner = owner
  self.closed = false
  self.shown = false
  self.view, self.content = 0, 0
  self.top, self.length, self.travel, self.greatest = 0, 0, 0, 0
  self.group = ui.create_group()
  self.groups = {self.group}
  render.set_group_visible(self.group, false)
  self.group_visible = false
  return self
end

-- Takes the view, the content (both in points) and the displayed scroll. True when the bar is shown. The bar appears and goes only
-- here: a content taller than the view shows it, and one that is not hides it.
function Bar:update(view, content, scroll)
  if self.closed then
    return false
  end
  if not (content > view) then
    if self.shown then hide(self) end
    self.view, self.content = view, content
    return false
  end
  local was_shown = self.shown
  if self.track_key == nil then
    self.track_key = ui.key("scroll_track")
    self.thumb_key = ui.key("scroll_thumb")
  end
  if not was_shown then
    pointer.register(self.track_key, track_widget(self), self.window)
    pointer.register(self.thumb_key, thumb_widget(self), self.window)
  end
  self.shown = true
  local view_changed = view ~= self.view
  self.view, self.content = view, content
  self.greatest = content - view
  self.length, self.top, self.travel = scrollbar.thumb(view, content, scroll)
  if self.track_quad == nil then
    self.track_quad = render.create_quad{texture = ui.texture(WHITE), x = 0, y = 0, width = TRACK, height = view, group = self.group,
      tint = theme.colour("track")}
    self.thumb_quad = render.create_quad{texture = ui.texture(WHITE), x = 0, y = self.top, width = TRACK, height = self.length,
      group = self.group, tint = theme.colour("dim")}
    self.shown_colour = theme.colour("dim")
    self.shown_rects = nil
  end
  local rects = table.concat({view, self.top, self.length}, ",")
  if rects ~= self.shown_rects then
    render.set_rect(self.track_quad, 0, 0, TRACK, view)
    render.set_rect(self.thumb_quad, 0, self.top, TRACK, self.length)
    -- The track's regions are the parts above and below the thumb; the thumb's own is the thumb.
    local track = {}
    if self.top > 0 then
      track[#track + 1] = {x = 0, y = 0, width = TRACK, height = self.top}
    end
    local below = view - self.top - self.length
    if below > 0 then
      track[#track + 1] = {x = 0, y = self.top + self.length, width = TRACK, height = below}
    end
    render.set_hit(self.thumb_key, {{x = 0, y = self.top, width = TRACK, height = self.length}}, self.group)
    render.set_hit(self.track_key, #track > 0 and track or nil, self.group)
    self.shown_rects = rects
  end
  look(self)
  if view_changed or not was_shown then
    self.clip_left = nil
  end
  apply_place(self)
  return true
end

-- Gives the absolute place of the track's top left corner and the window's content clip (nil: none to cut by).
function Bar:place(x, y, content_clip)
  if self.closed then
    return
  end
  self.abs_x, self.abs_y, self.content_clip = x, y, content_clip
  apply_place(self)
end

-- Whether `key` is one of the bar's keys, now or when it was last shown.
function Bar:owns(key)
  return key ~= nil and (key == self.track_key or key == self.thumb_key)
end

function Bar:destroy()
  if self.closed then
    return
  end
  end_drag(self)
  self.closed = true
  if self.shown then
    render.set_hit(self.track_key, nil, self.group)
    render.set_hit(self.thumb_key, nil, self.group)
    pointer.unregister(self.track_key)
    pointer.unregister(self.thumb_key)
  end
  render.destroy_group(self.group)
end

return scrollbar
