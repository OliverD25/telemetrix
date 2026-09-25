-- Internet speed test: ping, download and upload against the nearest Ookla
-- speed-test server, with Cloudflare (speed.cloudflare.com) as the backup.
-- Every 30 minutes, or when you press g. Starting the dashboard never runs a
-- test: the card shows the last stored result until the interval or the key.
--
-- Settings in [plugin.speedtest]:
--   server  = ""   empty: the nearest Ookla server, chosen by ping and kept
--                  for 24 hours; "host:port" forces one; "cloudflare" uses
--                  Cloudflare only
--   streams = 2    parallel connections, 1..8 (2 keeps a test under the memory budget)
--   seconds = 3    per direction, 2..10; the first 0.5 s is not counted
--
-- Data use: each direction moves its speed times its seconds. At 935 Mbps
-- down and up with the defaults that is about 700 MB per test, and about
-- 34 GB a day at the 30-minute interval. On a metered connection raise the
-- interval, or lower streams or seconds.
--
-- The server list is Ookla's public one (www.speedtest.net/api/js/servers),
-- the same one the open-source speedtest-cli uses. It is not an official
-- API, which is why Cloudflare is the automatic backup.

local LIST_URL = "https://www.speedtest.net/api/js/servers?engine=js&limit=5"
local CLOUDFLARE = {
  host = "speed.cloudflare.com:443",
  label = "Cloudflare",
  -- Cloudflare refuses some sizes with HTTP 403 (15, 16 and 100 MB did,
  -- tested 2026-09-25); 10 MB is accepted.
  down = "https://speed.cloudflare.com/__down?bytes=10000000",
  up = "https://speed.cloudflare.com/__up",
}
local SERVER_DAYS = 1
local KEEP = 30
local LABEL_WIDTH = 34

local function mbps_text(mbps)
  return string.format(mbps < 10 and "%.1f Mbps" or "%.0f Mbps", mbps)
end

local function median(list)
  table.sort(list)
  local n = #list
  if n == 0 then
    return nil
  end
  if n % 2 == 1 then
    return list[(n + 1) // 2]
  end
  return (list[n // 2] + list[n // 2 + 1]) / 2
end

local function when(t)
  if os.date("%Y-%m-%d", t) == os.date("%Y-%m-%d") then
    return os.date("%H:%M", t)
  end
  return os.date("%Y-%m-%d %H:%M", t)
end

local function fit(text)
  if utf8.len(text) and utf8.len(text) > LABEL_WIDTH then
    return text:sub(1, utf8.offset(text, LABEL_WIDTH) - 1) .. "…"
  end
  return text
end

-- "host:port" → host, port (443 without a port).
local function split(hostport)
  local host, port = hostport:match("^(.-):(%d+)$")
  if host then
    return host, tonumber(port)
  end
  return hostport, 443
end

local function ping(hostport, times)
  local host, port = split(hostport)
  local list = {}
  for _ = 1, times do
    local ms = telemetrix.tcp_ping_ms(host, port, 1000)
    if ms then
      list[#list + 1] = ms
    end
  end
  return median(list)
end

local function progress(what, mbps)
  telemetrix.emit({
    metrics = { { label = "testing " .. what .. "...", value = mbps and mbps_text(mbps) or "" } },
  })
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

-- The nearest of Ookla's five closest servers, by the median of 3 pings.
local function nearest_server()
  local list, err = get_json(LIST_URL)
  if not list then
    return nil, "the Ookla server list failed: " .. err
  end
  local best
  for _, s in ipairs(list) do
    if type(s.host) == "string" and s.host ~= "" then
      local ms = ping(s.host, 3)
      if ms and (not best or ms < best.ping) then
        local sponsor = tostring(s.sponsor or ""):gsub('"', "")
        local label = sponsor .. (s.name and (", " .. s.name) or "")
        best = { host = s.host, label = label, ping = ms, chosen = os.time() }
      end
    end
  end
  if not best then
    return nil, "no Ookla server answered a ping"
  end
  return best
end

local function choose_server(s, store)
  local wanted = (s.server or ""):gsub("^%s+", ""):gsub("%s+$", "")
  if wanted:lower() == "cloudflare" then
    return nil, nil
  end
  if wanted ~= "" then
    return { host = wanted, label = wanted, forced = true }
  end
  local cached = store.server
  if cached and os.time() - (cached.chosen or 0) < SERVER_DAYS * 86400 then
    return cached
  end
  local best, err = nearest_server()
  store.server = best
  return best, err
end

local function direction(dir, url, s, what)
  return telemetrix.speed_multi({
    direction = dir,
    url = url,
    streams = s.streams,
    seconds = s.seconds,
    warmup = 0.5,
    progress = function(mbps)
      progress(what, mbps)
    end,
  })
end

-- Ping, download and upload against one server. A failed ping or download
-- returns nil and the reason; a failed upload is part of the result.
local function test_server(server, s, down_url, up_url)
  progress("ping")
  local ms = ping(server.host, 5)
  if not ms then
    return nil, "no ping answer from " .. server.host
  end
  local down, err = direction("down", down_url, s, "download")
  if not down then
    return nil, "download from " .. server.host .. " failed: " .. err
  end
  local up, up_err = direction("up", up_url, s, "upload")
  return {
    down = down.mbps,
    up = up and up.mbps,
    up_error = up_err,
    ping = ms,
    server = server.label,
    time = os.time(),
  }
end

local function measure(s, store)
  local server, why = choose_server(s, store)
  if server then
    local base = "https://" .. server.host
    local r
    r, why = test_server(server, s, base .. "/download?size=25000000", base .. "/upload")
    if r then
      return r
    end
    if not server.forced then
      store.server = nil -- choose again next time
    end
  end
  local backup = { host = CLOUDFLARE.host, label = CLOUDFLARE.label }
  if why then
    telemetrix.log("using Cloudflare as the backup: " .. why)
    backup.label = CLOUDFLARE.label .. " (backup)"
  end
  local r, err = test_server(backup, s, CLOUDFLARE.down, CLOUDFLARE.up)
  if not r then
    error(err .. (why and (" (after: " .. why .. ")") or ""), 0)
  end
  return r
end

local function result_rows(store)
  local last = store.last
  if not last then
    return { { label = "press g to test", value = "" } }
  end
  local rows = {
    { label = "down", value = mbps_text(last.down) },
    { label = "up", value = last.up and mbps_text(last.up) or ("failed: " .. (last.up_error or "?")) },
    { label = "ping", value = last.ping and string.format("%.0f ms", last.ping) or "no answer" },
  }
  if last.server then
    rows[#rows + 1] = { label = "server", value = fit(last.server) }
  end
  rows[#rows + 1] = { label = "last", value = when(last.time) }
  local history = store.history or {}
  if #history >= 2 then
    local sum = 0
    for _, v in ipairs(history) do
      sum = sum + v
    end
    rows[#rows + 1] = {
      label = #history .. " runs",
      value = "avg " .. mbps_text(sum / #history),
      trend = history,
    }
  end
  return rows
end

return {
  title = "Speed test",
  interval = 1800,
  run_key = "g",
  -- Server choice and pings, two directions of up to 10 s each, and
  -- possibly the same again against the backup.
  call_timeout = 60,
  settings_schema = {
    server = { kind = "text", label = "server", default = "" },
    streams = { kind = "int", label = "streams", min = 1, max = 8, default = 2 },
    seconds = { kind = "int", label = "seconds", min = 2, max = 10, default = 3 },
  },
  update = function()
    local store = telemetrix.store_get() or {}
    local why = telemetrix.trigger()
    if why == "interval" or why == "key" or why == "manual" then
      local r = measure(telemetrix.settings, store)
      store.last = r
      store.history = store.history or {}
      table.insert(store.history, r.down)
      while #store.history > KEEP do
        table.remove(store.history, 1)
      end
      local ok, err = telemetrix.store_set(store)
      if not ok then
        telemetrix.log("cannot save the store: " .. err)
      end
    end
    return { metrics = result_rows(store) }
  end,
}
