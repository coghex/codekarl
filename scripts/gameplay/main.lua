-- The gameplay VM's skeleton: it only says that it ran. No real gameplay yet.
print("gameplay: loaded")
log.info("loaded")

function startup()
  print("gameplay: startup")
  log.info("startup")
end

function shutdown()
  print("gameplay: shutdown")
  log.info("shutdown")
end
