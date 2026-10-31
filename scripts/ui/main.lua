-- The ui VM: the common demo. It opens an 800 by 600 window with a solid clear colour, logs when the
-- window is open, and logs the source of a close request (Escape or the close button) and accepts it.
-- Run headless (--headless, or no [window] backend), `window.open` is ignored with one record and the rest
-- of the script runs unchanged.
print("ui: loaded")
log.info("loaded")

window.open{title = "Codekarl", width = 800, height = 600, clear = {0.10, 0.25, 0.45}}

window.on_opened(function()
  log.info("the window is open")
end)

window.on_close_request(function(source)
  log.info("close requested by " .. source)
  return true
end)

function startup()
  print("ui: startup")
  log.info("startup")
end

function shutdown()
  print("ui: shutdown")
  log.info("shutdown")
end
