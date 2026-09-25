-- Internet speed test against Cloudflare (speed.cloudflare.com, free, no key):
-- ping, download and upload, every 6 hours or when you press g.
-- Settings in [plugin.speedtest]: download_mb (5..100, default 25),
-- upload_mb (1..50, default 10), max_seconds per direction (3..15, default 8).
-- Data use: about 35 MB per run with the defaults, so about 140 MB a day at
-- the 6-hour interval. Starting the dashboard never runs a test: the card
-- shows the last stored result until the interval or the g key.

local HOST = "speed.cloudflare.com"
local DOWN_URL = "https://speed.cloudflare.com/__down?bytes="
local UP_URL = "https://speed.cloudflare.com/__up"
local KEEP = 30

local function mbps_text(mbps)
  return string.format(mbps < 10 and "%.1f Mbps" or "%.0f Mbps", mbps)
end

local function median(list)
  table.sort(list)
  local n = #list
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

local function result_rows(store)
  local last = store.last
  if not last then
    return { { label = "press g to test", value = "" } }
  end
  local rows = {
    { label = "down", value = mbps_text(last.down) },
    { label = "up", value = last.up and mbps_text(last.up) or ("failed: " .. (last.up_error or "?")) },
    { label = "ping", value = last.ping and string.format("%.0f ms", last.ping) or "no answer" },
    { label = "last", value = when(last.time) },
  }
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

local function progress(what, mbps)
  telemetrix.emit({
    metrics = { { label = "testing " .. what .. "...", value = mbps and mbps_text(mbps) or "" } },
  })
end

local function measure(s)
  progress("ping")
  local pings = {}
  for _ = 1, 5 do
    local ms = telemetrix.tcp_ping_ms(HOST, 443)
    if ms then
      pings[#pings + 1] = ms
    end
  end
  local max_seconds = s.max_seconds
  local down_bytes = s.download_mb * 1000000
  progress("download")
  local down, err = telemetrix.speed_download(
    DOWN_URL .. string.format("%d", down_bytes), down_bytes, max_seconds,
    function(bytes, seconds)
      progress("download", seconds > 0 and bytes * 8 / seconds / 1e6 or 0)
    end)
  if not down then
    error("download failed: " .. err, 0)
  end
  progress("upload")
  local up, up_err = telemetrix.speed_upload(UP_URL, s.upload_mb * 1000000, max_seconds,
    function(bytes, seconds)
      progress("upload", seconds > 0 and bytes * 8 / seconds / 1e6 or 0)
    end)
  return {
    down = down.mbps,
    up = up and up.mbps,
    up_error = up_err,
    ping = #pings > 0 and median(pings) or nil,
    time = os.time(),
  }
end

return {
  title = "Speed test",
  interval = 21600,
  run_key = "g",
  call_timeout = 40,
  settings_schema = {
    download_mb = { kind = "int", label = "download MB", min = 5, max = 100, step = 5, default = 25 },
    upload_mb = { kind = "int", label = "upload MB", min = 1, max = 50, default = 10 },
    max_seconds = { kind = "int", label = "max seconds", min = 3, max = 15, default = 8 },
  },
  update = function()
    local store = telemetrix.store_get() or {}
    local why = telemetrix.trigger()
    if why == "interval" or why == "key" or why == "manual" then
      local r = measure(telemetrix.settings)
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
