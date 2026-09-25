-- Weather from Open-Meteo (free, no key): now, tomorrow and the day after.
-- Settings in [plugin.weather]:
--   city = "Kyiv"          (type it in the s overlay; looked up once per change)
--   lat, lon, label        (file only, optional: an exact place and its name,
--                           used only when city is not set)
-- Without city and without lat/lon the card shows Kyiv.
-- Temperatures follow units.temperature in telemetrix.toml (°C or °F).
--
-- Weather data by Open-Meteo.com (CC BY 4.0). Free for non-commercial use:
-- 600 calls a minute, 5,000 an hour, 10,000 a day. The data license asks
-- for attribution, so the card ends with a "data: Open-Meteo.com" line.

local GEO_URL = "https://geocoding-api.open-meteo.com/v1/search?name=%s&count=1&language=en&format=json"
local FORECAST_URL = "https://api.open-meteo.com/v1/forecast?latitude=%s&longitude=%s"
  .. "&current=temperature_2m,wind_speed_10m,weather_code"
  .. "&daily=weather_code,temperature_2m_max,temperature_2m_min,"
  .. "precipitation_probability_max,wind_speed_10m_max"
  .. "&forecast_days=3&timezone=auto"
local WEEKDAYS = { "Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat" }
local DEFAULT_CITY = "Kyiv"
-- The last "which place" note, so it is logged once per change, not every update.
local last_note

local function urlencode(text)
  return (text:gsub("[^%w%-_.~]", function(c)
    return string.format("%%%02X", c:byte())
  end))
end

local function get_json(url)
  local body, status = telemetrix.http_get(url)
  if not body then
    return nil, status
  end
  if status ~= 200 then
    return nil, "HTTP " .. status
  end
  local data = telemetrix.json_decode(body)
  if type(data) ~= "table" then
    return nil, "not JSON"
  end
  return data
end

-- WMO weather codes as one or two words.
local function sky(code)
  if code == nil then
    return "?"
  elseif code == 0 then
    return "clear"
  elseif code == 1 then
    return "mostly clear"
  elseif code <= 3 then
    return "cloudy"
  elseif code == 45 or code == 48 then
    return "fog"
  elseif code >= 51 and code <= 57 then
    return "drizzle"
  elseif code >= 61 and code <= 65 then
    return "rain"
  elseif code == 66 or code == 67 then
    return "freezing rain"
  elseif code >= 71 and code <= 77 then
    return "snow"
  elseif code >= 80 and code <= 82 then
    return "showers"
  elseif code == 85 or code == 86 then
    return "snow showers"
  elseif code >= 95 and code <= 99 then
    return "storm"
  end
  return "code " .. code
end

-- The weekday of a "YYYY-MM-DD" date (Sakamoto's method), independent of
-- this computer's clock and time zone.
local function weekday(date)
  local y, m, d = date:match("^(%d+)-(%d+)-(%d+)")
  y, m, d = tonumber(y), tonumber(m), tonumber(d)
  if not (y and m and d) then
    return "?"
  end
  local t = { 0, 3, 2, 5, 0, 3, 5, 1, 4, 6, 2, 4 }
  if m < 3 then
    y = y - 1
  end
  return WEEKDAYS[(y + y // 4 - y // 100 + y // 400 + t[m] + d) % 7 + 1]
end

local function round(v)
  return string.format("%d", math.floor(v + 0.5))
end

-- The place for the city, from the store or from the geocoding service.
local function find_place(city, store)
  local cached = store.place
  if cached and cached.query == city then
    return cached
  end
  local data, err = get_json(string.format(GEO_URL, urlencode(city)))
  if not data then
    error("cannot look up " .. city .. ": " .. err, 0)
  end
  local hit = type(data.results) == "table" and data.results[1]
  local place = { query = city }
  if hit and hit.latitude and hit.longitude then
    place.name, place.country = hit.name or city, hit.country_code
    place.lat, place.lon = hit.latitude, hit.longitude
  end
  store.place = place
  local ok, store_err = telemetrix.store_set(store)
  if not ok then
    telemetrix.log("cannot save the store: " .. store_err)
  end
  return place
end

return {
  title = "Weather",
  interval = 600,
  settings_schema = {
    -- Empty means "not set", so lat/lon from old settings files can still apply.
    city = { kind = "text", label = "city", default = "" },
  },
  update = function()
    local s = telemetrix.settings
    local city = ((s.city or ""):gsub("^%s+", ""):gsub("%s+$", ""))
    local lat, lon = tonumber(s.lat), tonumber(s.lon)
    local use_coordinates = city == "" and lat and lon
    if s.lat ~= nil or s.lon ~= nil then
      local note
      if use_coordinates then
        note = string.format("using lat/lon %s, %s; city is not set", lat, lon)
      elseif city ~= "" then
        note = "using city " .. city .. "; lat/lon are ignored"
      else
        note = "lat/lon need both numbers; using " .. DEFAULT_CITY
      end
      if note ~= last_note then
        telemetrix.log(note)
        last_note = note
      end
    end
    if city == "" then
      city = DEFAULT_CITY
    end
    local title, metrics = nil, {}
    if use_coordinates then
      local where = string.format("%.2f, %.2f", lat, lon)
      title = "Weather · " .. (type(s.label) == "string" and s.label or where)
      metrics[1] = { label = "place", value = where .. " from lat/lon" }
    else
      local place = find_place(city, telemetrix.store_get() or {})
      if not place.lat then
        error("place not found: " .. city, 0)
      end
      lat, lon = place.lat, place.lon
      title = "Weather · " .. place.name .. (place.country and (", " .. place.country) or "")
    end

    local data, err = get_json(string.format(FORECAST_URL, lat, lon))
    if not data then
      error("Open-Meteo: " .. err, 0)
    end
    local now, daily = data.current, data.daily
    if type(now) ~= "table" or type(daily) ~= "table" or type(daily.time) ~= "table" then
      error("Open-Meteo sent no forecast", 0)
    end
    local fahrenheit = (telemetrix.units or {}).temperature == "fahrenheit"
    local unit = fahrenheit and "°F" or "°C"
    local function temp(c)
      return fahrenheit and c * 9 / 5 + 32 or c
    end
    metrics[#metrics + 1] = {
      label = "now",
      value = string.format("%.1f %s  wind %s km/h  %s",
        temp(now.temperature_2m or 0), unit, round(now.wind_speed_10m or 0), sky(now.weather_code)),
    }
    for i = 2, math.min(3, #daily.time) do
      local low, high = daily.temperature_2m_min[i], daily.temperature_2m_max[i]
      local rain = daily.precipitation_probability_max and daily.precipitation_probability_max[i]
      local value = string.format("%s..%s %s  %s", low and round(temp(low)) or "?",
        high and round(temp(high)) or "?", unit, sky(daily.weather_code[i]))
      if rain then
        value = value .. "  rain " .. round(rain) .. "%"
      end
      metrics[#metrics + 1] = { label = weekday(daily.time[i]), value = value }
    end
    metrics[#metrics + 1] = { label = "data: Open-Meteo.com", value = "", style = "dim" }
    return { title = title, metrics = metrics }
  end,
}
