-- Demonstration: an error in the close callback logs its traceback, shuts down cleanly and exits 1.
-- Press Escape or the close button.
print("ui: loaded")
log.info("loaded")

window.open{title = "Codekarl (the close callback fails)", width = 800, height = 600, clear = {0.30, 0.10, 0.30}}

window.on_close_request(function(source)
  error("demonstration: the close callback failed (source " .. source .. ")")
end)

function startup()
  log.info("startup")
end

function shutdown()
  log.info("shutdown")
end
