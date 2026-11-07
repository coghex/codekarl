-- The gameplay VM's skeleton: it only says that it ran. No real gameplay yet.
-- Lua-held gameplay state is not saved in milestone 7, so persistence across calls is outside its guarantee.
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

if sim then
  sim.on_day(function(date)
    log.info(string.format("day %04d-%02d-%02d", date.year, date.month, date.day))
  end)

  sim.on_month(function(date)
    log.info(string.format("month %04d-%02d-%02d", date.year, date.month, date.day))
  end)
end
