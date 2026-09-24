-- Current weather from Open-Meteo (free, no API key).
-- Settings in [plugin.weather]: lat, lon, label (a place name for the title),
-- temperature_unit = "celsius" or "fahrenheit" (default "celsius").
local DIRECTIONS = { "N", "NE", "E", "SE", "S", "SW", "W", "NW" }

local function compass(degrees)
  return DIRECTIONS[math.floor((degrees % 360) / 45 + 0.5) % 8 + 1]
end

return {
  title = "Weather",
  interval = 600,
  update = function()
    local s = telemetrix.settings
    local lat, lon = s.lat or 50.45, s.lon or 30.52
    local unit = s.temperature_unit == "fahrenheit" and "fahrenheit" or "celsius"
    local url = string.format(
      "https://api.open-meteo.com/v1/forecast?latitude=%s&longitude=%s"
        .. "&current=temperature_2m,wind_speed_10m,wind_direction_10m"
        .. "&wind_speed_unit=ms&temperature_unit=%s",
      lat, lon, unit)
    local body, status = telemetrix.http_get(url)
    if not body then
      error("request failed: " .. status, 0)
    end
    if status ~= 200 then
      error("Open-Meteo answered HTTP " .. status, 0)
    end
    local data = telemetrix.json_decode(body)
    local now = type(data) == "table" and data.current
    if type(now) ~= "table" or not now.temperature_2m then
      error("Open-Meteo sent no current weather", 0)
    end
    local symbol = unit == "fahrenheit" and "°F" or "°C"
    return {
      title = s.label and ("Weather · " .. s.label) or "Weather",
      metrics = {
        { label = "temp", value = string.format("%.1f %s", now.temperature_2m, symbol) },
        { label = "wind", value = string.format("%.1f m/s", now.wind_speed_10m or 0) },
        { label = "direction", value = compass(now.wind_direction_10m or 0) },
      },
    }
  end,
}
