-- Uptime: how long this computer has been running, and its name. No network.
local function duration(seconds)
  local days = seconds // 86400
  local hours = seconds % 86400 // 3600
  local minutes = seconds % 3600 // 60
  return string.format("%dd %02d:%02d", days, hours, minutes)
end

return {
  title = "Uptime",
  interval = 30,
  update = function()
    return {
      metrics = {
        { label = "host", value = telemetrix.hostname() },
        { label = "up", value = duration(math.floor(telemetrix.uptime_s())) },
      },
    }
  end,
}
