-- The editing that the number field (lib/numberfield.lua, CK-306) and the text field (lib/textfield.lua, CK-303) share: one field edits at a
-- time, whatever its kind, and its ownership has one rule.
--
-- **One rule for the editing's ownership.** A transition (apply, cancel, start, destroy) finishes all of its own state before any caller code
-- runs: the field's `is_editing`, its caret and outline, `editing`, and who the typing function names (with text input on or off). The
-- caller's `on_apply` is always the last thing a transition does, so whatever it does (close the window, destroy fields, start or restart
-- editing on any field) meets a coherent state and is an ordinary transition on it.
--
-- A field takes part by having, besides `is_editing` (true while it edits) and `frame` (nil once destroyed), `field_edit = true` (on its class)
-- and these methods:
-- - `field:commit()` settles what was typed: it keeps the result as the field's value, shows it and returns the call that tells the caller
--   (a function, or nil when there is nothing to tell). The caller of `commit` makes that call last.
-- - `field:revert()` puts the old value back and shows it.
-- - `field:show_editing()` shows the editing: the outline in `hot` and the caret.
-- - `field:hide_caret()` takes the caret away.
-- and the typing function's `on_typed(entry)` (lib/typing.lua).
local typing = require("lib.typing")
local pointer = require("lib.pointer")

local fieldedit = {}

local editing = nil -- the field editing now

-- Ends the editing without telling anyone; `handoff` leaves the typing function's editor as it is, for the field that takes over.
function fieldedit.end_editing(self, handoff)
  if not self.is_editing then
    return
  end
  self.is_editing = false
  if editing == self then
    editing = nil
  end
  self:hide_caret()
  if not handoff then
    typing.stop(self)
  end
end

-- Settles what was typed and ends the editing. Returns the call that tells the caller (nil when there is nothing to tell); the caller of
-- `settle` makes it last. `handoff` leaves the typing function's editor for the field that takes over (`begin`).
local function settle(self, handoff)
  fieldedit.end_editing(self, handoff)
  return self:commit()
end

-- Applies what was typed and tells the caller once (Enter, Tab, a press elsewhere).
function fieldedit.apply(self)
  if not self.is_editing then
    return
  end
  local tell = settle(self, false)
  if tell ~= nil then tell() end
end

-- Puts the old value back and ends the editing, telling the caller nothing.
local function cancel(self)
  if not self.is_editing then
    return
  end
  fieldedit.end_editing(self, false)
  self:revert()
end

-- Starts `self`'s editing. The typing function is asked first: while a key capture owns the keyboard it refuses (`typing.start` returns
-- false) and then nothing changes: no field is applied, none edits, no caret or outline shows. Otherwise it has passed from the field that
-- was editing to this one (text input stays on); that field is settled and its caller told last, when this field already edits.
function fieldedit.begin(self)
  if self.is_editing or self.frame == nil then
    return
  end
  if not typing.start(self) then
    return
  end
  local tell
  if editing ~= nil then
    tell = settle(editing, true)
  end
  self.is_editing = true
  editing = self
  self:show_editing()
  if tell ~= nil then tell() end
end

-- The editor protocol of lib/typing.lua: `typing.capture` calls `editor:on_apply()` to apply and end the editing before it asks the window
-- to capture a key. A field's `on_apply` method is this: it applies what was typed and tells the caller, as Enter does (the caller's code
-- runs last), and then leaves no field editing: one that the caller's code started meanwhile is cancelled, since the keyboard is going to
-- the capture. When it returns no field edits, none has a caret and the typing function has no editor.
function fieldedit.capture_apply(self)
  if self.is_editing then
    fieldedit.apply(self)
  end
  if editing ~= nil then
    cancel(editing)
  end
end

-- The field editing now, or nil.
function fieldedit.current()
  return editing
end

-- Escape's first step: puts the old value back in the field that edits and ends its editing. True when there was one.
function fieldedit.cancel()
  if editing == nil then
    return false
  end
  cancel(editing)
  return true
end

-- A left press anywhere that is not a field applies the field that edits, before the press reaches its own widget; the press itself still
-- does what it does. A press on a field is that field's own business (`begin` applies the one that was editing).
pointer.observe_press(function(event)
  if editing == nil then
    return
  end
  local widget = event.key and pointer.widget(event.key) or nil
  if widget ~= nil and widget.field_edit then
    return
  end
  fieldedit.apply(editing)
end)

return fieldedit
