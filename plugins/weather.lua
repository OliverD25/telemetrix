-- Weather from Open-Meteo (free, no key): now, tomorrow and the day after.
-- Settings in [plugin.weather]:
--   city = "Kyiv"          (press Enter on the city row in the s box and pick
--                           a place from the list; that also saves lat, lon
--                           and place. Or type a city name in the file.)
--   country = "UA"         (the city search lists places in this country
--                           first; "" = no preference, biggest places first)
--   lat, lon, place        (the exact place the s box search saved)
--   lat, lon, label        (older files: an exact place and its name, used
--                           only when city is not set)
-- Without any of these the card shows Kyiv.
-- Temperatures follow units.temperature in telemetrix.toml (°C or °F).
--
-- Weather data by Open-Meteo.com (CC BY 4.0). Free for non-commercial use:
-- 600 calls a minute, 5,000 an hour, 10,000 a day. The data license asks
-- for attribution, so the card ends with a "data: Open-Meteo.com" line.
--
-- The geocoding service finds only the official spelling of a place, or
-- the start of it, and Ukrainian Cyrillic only with language=uk. The
-- search below also tries old or Russian names, common spelling variants
-- and shorter prefixes, at most MAX_QUERIES requests per search.

local GEO_URL = "https://geocoding-api.open-meteo.com/v1/search?name=%s&count=10&language=%s&format=json"
local FORECAST_URL = "https://api.open-meteo.com/v1/forecast?latitude=%s&longitude=%s"
  .. "&current=temperature_2m,wind_speed_10m,weather_code"
  .. "&daily=weather_code,temperature_2m_max,temperature_2m_min,"
  .. "precipitation_probability_max,wind_speed_10m_max"
  .. "&forecast_days=3&timezone=auto"
local WEEKDAYS = { "Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat" }
local DEFAULT_CITY = "Kyiv"
local MAX_QUERIES = 6
local MAX_PLACES = 8
local MIN_PREFIX = 4
local CACHE_MS = 10 * 60 * 1000
local CACHE_MAX = 64
-- Geocoding answers by URL, so typing the same letters again costs nothing.
local geo_cache, geo_cache_size = {}, 0
-- The last "which place" note, so it is logged once per change, not every update.
local last_note

-- Old or Russian names that the geocoding service does not know, by their
-- lower-case spelling.
local ALIASES = {
  kiev = "Kyiv", kharkov = "Kharkiv", odessa = "Odesa", lvov = "Lviv",
  nikolaev = "Mykolaiv", nikolayev = "Mykolaiv", zaporozhye = "Zaporizhzhia",
  zaporozhe = "Zaporizhzhia", dnepr = "Dnipro", dnepropetrovsk = "Dnipro",
  rovno = "Rivne", khmelnitsky = "Khmelnytskyi", khmelnitskiy = "Khmelnytskyi",
  khmelnitskij = "Khmelnytskyi", vinnitsa = "Vinnytsia", zhitomir = "Zhytomyr",
  chernigov = "Chernihiv", chernovtsy = "Chernivtsi", ternopol = "Ternopil",
  lugansk = "Luhansk", kirovograd = "Kropyvnytskyi", cherkassy = "Cherkasy",
  uzhgorod = "Uzhhorod", ["ivano-frankovsk"] = "Ivano-Frankivsk",
  ["krivoy rog"] = "Kryvyi Rih",
  ["киев"] = "Київ", ["харьков"] = "Харків", ["одесса"] = "Одеса",
  ["львов"] = "Львів", ["николаев"] = "Миколаїв", ["запорожье"] = "Запоріжжя",
  ["днепр"] = "Дніпро", ["днепропетровск"] = "Дніпро", ["ровно"] = "Рівне",
  ["хмельницкий"] = "Хмельницький", ["винница"] = "Вінниця",
  ["чернигов"] = "Чернігів", ["черновцы"] = "Чернівці", ["тернополь"] = "Тернопіль",
  ["луганск"] = "Луганськ", ["кировоград"] = "Кропивницький",
  ["черкассы"] = "Черкаси", ["ивано-франковск"] = "Івано-Франківськ",
  ["кривой рог"] = "Кривий Ріг",
}

local function urlencode(text)
  return (text:gsub("[^%w%-_.~]", function(c)
    return string.format("%%%02X", c:byte())
  end))
end

local function trim(text)
  return ((text or ""):gsub("^%s+", ""):gsub("%s+$", ""))
end

-- Lower case for Latin and Cyrillic letters (string.lower knows only ASCII).
local function lower(text)
  local out = {}
  for _, c in utf8.codes(text) do
    if c >= 0x41 and c <= 0x5A then
      c = c + 0x20
    elseif c >= 0x410 and c <= 0x42F then
      c = c + 0x20
    elseif c >= 0x400 and c <= 0x40F then
      c = c + 0x50
    elseif c == 0x490 then
      c = 0x491
    end
    out[#out + 1] = utf8.char(c)
  end
  return table.concat(out)
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

-- One geocoding request, answered from the cache when it is fresh.
local function geo_query(name, lang)
  local url = string.format(GEO_URL, urlencode(name), lang)
  local now = telemetrix.now_ms()
  local hit = geo_cache[url]
  if hit and now - hit.time < CACHE_MS then
    return hit.results, true
  end
  local data, err = get_json(url)
  if not data then
    return nil, err
  end
  local results = type(data.results) == "table" and data.results or {}
  if geo_cache_size >= CACHE_MAX then
    geo_cache, geo_cache_size = {}, 0
  end
  geo_cache[url] = { time = now, results = results }
  geo_cache_size = geo_cache_size + 1
  return results
end

-- Russian letters and endings that Ukrainian writes differently.
local function ru_to_uk(text)
  return (text:gsub("ы", "и"):gsub("э", "е"):gsub("ъ", "")
    :gsub("цкий$", "цький"):gsub("ский$", "ський"))
end

-- Common Latin spellings of Ukrainian names brought closer to the official
-- one: a leading h is kh, c is ts, -iy/-yy/-ij endings are -yi.
local function latin_base(text)
  local v = text:gsub("'", ""):gsub("’", ""):gsub("`", "")
  if v:sub(1, 1) == "h" and v:sub(1, 2) ~= "kh" then
    v = "k" .. v
  end
  v = v:gsub("c([^h])", "ts%1"):gsub("c$", "ts")
  v = v:gsub("[iy]y$", "yi"):gsub("ij$", "yi")
  return v
end

-- `from` becomes `to` everywhere but in the last two letters (the ending).
local function swap_middle(text, from, to)
  if #text <= 3 then
    return text
  end
  return text:sub(1, -3):gsub(from, to) .. text:sub(-2)
end

-- The first `n` characters of a UTF-8 text.
local function prefix(text, n)
  local stop = utf8.offset(text, n + 1)
  return stop and text:sub(1, stop - 1) or text
end

local function place_label(r)
  local parts = { r.name }
  if type(r.admin1) == "string" and r.admin1 ~= "" and r.admin1 ~= r.name then
    parts[#parts + 1] = r.admin1
  end
  if type(r.country_code) == "string" then
    parts[#parts + 1] = r.country_code
  end
  local label = table.concat(parts, ", ")
  if utf8.len(label) > 40 then
    label = label:gsub(" Oblast", " Obl"):gsub(" область", " обл.")
  end
  return label
end

-- Places (not airports or regions) for what someone typed, best first:
-- places in `country` first, then by population. Returns the geocoding
-- results, or nil and an error when every request failed.
local function find_places(query, country)
  local q = trim(query)
  if utf8.len(q) < 2 then
    return {}
  end
  local low = lower(q)
  local cyrillic = low:find("[\208-\211]") ~= nil
  local required, optional, seen = {}, {}, {}
  local function add(list, name, lang)
    if name and name ~= "" then
      local key = lower(name) .. "|" .. lang
      if not seen[key] then
        seen[key] = true
        list[#list + 1] = { name = name, lang = lang }
      end
    end
  end
  local best
  if cyrillic then
    add(required, q, "uk")
    add(required, ALIASES[low], "uk")
    best = ru_to_uk(low)
    add(optional, best, "uk")
    add(optional, q, "en")
  else
    add(required, q, "en")
    add(required, ALIASES[low], "en")
    best = latin_base(low)
    add(optional, best, "en")
    local y = swap_middle(best, "i", "y")
    add(optional, y, "en")
    add(optional, swap_middle(best, "y", "i"), "en")
    best = y
  end

  local pref = (country or ""):upper()
  local list, ids, used, failed, answered = {}, {}, 0, nil, false
  local function ask(step)
    local results, err = geo_query(step.name, step.lang)
    used = used + 1
    if not results then
      failed = err
      return
    end
    answered = true
    for _, r in ipairs(results) do
      local code = type(r.feature_code) == "string" and r.feature_code or "PPL"
      if r.id and r.latitude and r.longitude and code:sub(1, 3) == "PPL" and not ids[r.id] then
        ids[r.id] = true
        list[#list + 1] = { r = r, n = #list + 1 }
      end
    end
  end
  local function good_enough()
    for _, p in ipairs(list) do
      if pref == "" or (p.r.country_code or ""):upper() == pref then
        return true
      end
    end
    return false
  end

  for _, step in ipairs(required) do
    ask(step)
  end
  for _, step in ipairs(optional) do
    if good_enough() or used >= MAX_QUERIES then
      break
    end
    ask(step)
  end
  local n = utf8.len(best) or 0
  for _, len in ipairs({ n - 2, math.ceil(n / 2) }) do
    if #list > 0 or used >= MAX_QUERIES then
      break
    end
    len = math.max(len, MIN_PREFIX)
    if len < n then
      local step = { name = prefix(best, len), lang = cyrillic and "uk" or "en" }
      local key = lower(step.name) .. "|" .. step.lang
      if not seen[key] then
        seen[key] = true
        ask(step)
      end
    end
  end
  if not answered and failed then
    return nil, failed
  end

  table.sort(list, function(a, b)
    local pa = pref ~= "" and (a.r.country_code or ""):upper() == pref
    local pb = pref ~= "" and (b.r.country_code or ""):upper() == pref
    if pa ~= pb then
      return pa
    end
    local na, nb = tonumber(a.r.population) or 0, tonumber(b.r.population) or 0
    if na ~= nb then
      return na > nb
    end
    return a.n < b.n
  end)
  local out = {}
  for i = 1, math.min(#list, MAX_PLACES) do
    out[i] = list[i].r
  end
  return out
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

-- The place for a typed city, from the store or from the geocoding service.
local function find_place(city, country, store)
  local cached = store.place
  if cached and cached.query == city and (cached.pref or "") == country then
    return cached
  end
  local places, err = find_places(city, country)
  if not places then
    error("cannot look up " .. city .. ": " .. err, 0)
  end
  local hit = places[1]
  local place = { query = city, pref = country }
  if hit then
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

-- "Lviv, Lviv Oblast, UA" -> "Lviv, UA".
local function short_place(place)
  local first = place:match("^([^,]+)") or place
  local last = place:match(",%s*([^,]+)$")
  if last and last ~= first then
    return first .. ", " .. last
  end
  return first
end

-- A picked place still belongs to the city when the city was not typed
-- anew in the file: its label starts with the city name.
local function place_matches(place, city)
  if city == "" then
    return true
  end
  local name = place:match("^([^,]+)") or place
  return lower(trim(name)) == lower(city)
end

return {
  title = "Weather",
  interval = 600,
  settings_schema = {
    -- Empty means "not set", so lat/lon from old settings files can still apply.
    city = { kind = "search", label = "city", default = "" },
    country = { kind = "text", label = "country", default = "UA" },
  },
  search = function(query)
    local country = trim(telemetrix.settings.country)
    local places, err = find_places(query, country)
    if not places then
      error("Open-Meteo: " .. err, 0)
    end
    local out = {}
    for i, r in ipairs(places) do
      local label = place_label(r)
      out[i] = {
        label = label,
        values = { city = r.name, lat = r.latitude, lon = r.longitude, place = label },
      }
    end
    return out
  end,
  update = function()
    local s = telemetrix.settings
    local city = trim(s.city)
    local place = type(s.place) == "string" and trim(s.place) or ""
    local lat, lon = tonumber(s.lat), tonumber(s.lon)
    local picked = place ~= "" and lat and lon and place_matches(place, city)
    local use_coordinates = picked or (city == "" and lat and lon)
    if (s.lat ~= nil or s.lon ~= nil) and not picked then
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
    if picked then
      title = "Weather · " .. short_place(place)
    elseif use_coordinates then
      local where = string.format("%.2f, %.2f", lat, lon)
      title = "Weather · " .. (type(s.label) == "string" and s.label or where)
      metrics[1] = { label = "place", value = where .. " from lat/lon" }
    else
      local found = find_place(city, trim(s.country), telemetrix.store_get() or {})
      if not found.lat then
        error("place not found: " .. city, 0)
      end
      lat, lon = found.lat, found.lon
      title = "Weather · " .. found.name .. (found.country and (", " .. found.country) or "")
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
