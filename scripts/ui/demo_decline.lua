-- Demonstration: the close callback declines the first close request (logging it) and accepts the next.
-- Try Escape and the close button, one after the other.
print("ui: loaded")
log.info("loaded")

window.open{title = "Codekarl (declines the first close)", width = 800, height = 600, clear = {0.45, 0.25, 0.10}}

local declined = false

window.on_opened(function()
  log.info("the window is open")
end)

window.on_close_request(function(source)
  if not declined then
    declined = true
    log.info("close requested by " .. source .. ": declined")
    return false
  end
  log.info("close requested by " .. source .. ": accepted")
  return true
end)

function startup()
  log.info("startup")
end

function shutdown()
  log.info("shutdown")
end
