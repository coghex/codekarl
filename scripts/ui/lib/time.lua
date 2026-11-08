-- The time bar's date text. Transport state is shown by its pause/play icon and speed boxes.
local MONTHS = {"Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"}

local time = {}

function time.date(value)
  return string.format("%d %s %d · %02d:00", value.day, MONTHS[value.month], value.year, value.hour)
end

return time
