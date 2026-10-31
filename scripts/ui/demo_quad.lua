-- Demonstration: the window shows a 32 x 32 pixel-art image (assets/demo-quad.png) scaled to 256 x 256 points
-- and centred, drawn with nearest sampling and blended over the clear colour. Escape or the close button
-- closes the window. Run headless (--headless), `window.open` and `render.quad` are validated and ignored,
-- with one record each, and the rest of the script runs unchanged.
print("ui: loaded")
log.info("loaded")

window.open{title = "Codekarl quad", width = 800, height = 600, clear = {0.10, 0.25, 0.45}}
render.quad{image = "demo-quad.png", x = 272, y = 172, width = 256, height = 256}

window.on_opened(function()
  log.info("the window is open")
end)

window.on_close_request(function(source)
  log.info("close requested by " .. source)
  return true
end)

function startup()
  log.info("startup")
end

function shutdown()
  log.info("shutdown")
end
