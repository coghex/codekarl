-- Typed text for an editing widget (CK-314; docs/ui-design.md): the library's one reader of the update's typed list.
--
-- The window gives each update `typed`, one ordered list of the characters the system committed and the edit commands, `typed_lost`
-- (entries were dropped at the bound) and `typed_epoch`. An entry is `{kind = "char", text = "á"}` (one Unicode scalar) or
-- `{kind = "backspace" | "enter" | "tab" | "left" | "right"}`. Nothing else reads the list, and no action is bound to a typing key.
--
-- **Sessions.** What an update's typing reaches is decided by the editing session that was under way when the update began, not by the widget's
-- table: when that session ends, or passes to another widget, the rest of the update (and its loss notice) reaches no one, even if the same
-- widget starts editing again at once (a Tab that applies a field and starts the next editing on it).
--
-- **One widget edits at a time.** A widget calls `typing.start(widget)` when its editing starts and `typing.stop(widget)` when it ends. The
-- library turns text input on (`window.set_text_input(true)`) when an editing starts with none under way, and off when the last one
-- ends; the change from one widget to another leaves it on. While a widget edits, `typing.dispatch(update)` gives it the update's
-- entries, in the list's order, through `widget:on_typed(entry)` (and `widget:on_typed_lost()` after them when entries were lost). With
-- no widget editing, the entries are dropped.
--
-- **Order with the pointer.** `main.lua` calls `typing.dispatch(update)` before the pointer dispatcher, so an update's typing is handled
-- first, all of it, and then its pointer events: typing goes to the widget that was editing when the update began, and when that widget
-- ends its editing part way (an Enter applied it) the update's later entries reach no widget.
--
-- **Which entries count.** Each call of `window.set_text_input` returns a generation, and an update's `typed_epoch` is the generation of the
-- latest call the main thread has applied. Entries of an update whose epoch is older than the library's latest call were typed under the
-- earlier setting (or before the main thread turned input on), so they are dropped: a stale entry never reaches a widget that started
-- after it was typed. A key pressed between a widget's start and the main thread turning input on is not typed.
local typing = {}

local editor = nil -- the widget editing now
local generation = 0 -- the generation of the latest window.set_text_input call
-- The editing session: one number that changes whenever an editing starts, ends or passes to another widget. It is the session that owns an
-- update's typing, not the widget's table: a widget that ends its editing and starts it again (in its own Tab handler, say) has a new session,
-- and what is left of the update it was handling belongs to the old one.
local session = 0

-- Starts `widget`'s editing: it takes the typing from any widget that was editing, and turns text input on when none was.
function typing.start(widget)
  if type(widget) ~= "table" or type(widget.on_typed) ~= "function" then
    error("typing.start: the widget needs an on_typed function", 2)
  end
  if editor == widget then
    return -- already editing: the same session goes on
  end
  if editor == nil then
    generation = window.set_text_input(true)
  end
  editor = widget
  session = session + 1
end

-- Ends `widget`'s editing, when it is the one editing; turns text input off. Another widget's call changes nothing.
function typing.stop(widget)
  if editor ~= nil and editor == widget then
    editor = nil
    session = session + 1
    generation = window.set_text_input(false)
  end
end

-- The widget editing now, or nil.
function typing.editing()
  return editor
end

-- Gives an update's typed entries to the widget that was editing when it began, in order. Called once per update, before the pointer.
function typing.dispatch(update)
  local target, mine = editor, session
  if target == nil then
    return
  end
  if (update.typed_epoch or 0) < generation then
    return
  end
  for _, entry in ipairs(update.typed or {}) do
    if session ~= mine then
      return
    end
    target:on_typed(entry)
  end
  if update.typed_lost and session == mine and target.on_typed_lost ~= nil then
    target:on_typed_lost()
  end
end

return typing
