-- Hosts: the state of your own servers, read by commands you list in
-- telemetrix.toml. The plugin runs nothing by itself: [commands] in your
-- settings file names each program and its arguments, and this plugin only
-- picks those names (see "Running commands" in PLUGINS.md).
--
-- Settings in [plugin.hosts]:
--   hosts = [
--     { name = "server-a", health = "server-a-health", stats = "server-a-stats",
--       ups = "server-a-ups" },
--   ]
--   Each entry names the host and, for each part, a command from [commands]:
--   health  a check script: "[ OK ]", "[WARN]" and "[FAIL]" lines and a
--           "== summary: 46 ok, 1 warning(s), 0 failure(s) ==" line; exit
--           code 0 = ok, 1 = warnings, 2 = failures
--   stats   the output of: nproc; cat /proc/loadavg;
--           grep -E "^(MemTotal|MemAvailable):" /proc/meminfo;
--           df -B1 --output=size,used,avail / | tail -1; cat /proc/uptime
--   ups     "key: value" lines from upsc (battery.charge, battery.runtime,
--           ups.status, ...); optional
--   timeout = 30    seconds for each command, 1..55
-- All commands of all hosts run at the same time. An ssh that cannot reach
-- its host ends with exit code 255; then the host shows as unreachable and
-- keeps its last values, dimmed, with their time.

local MAX_TEXT = 60

local function trim(text)
  return ((text or ""):gsub("^%s+", ""):gsub("%s+$", ""))
end

local function cut(text, n)
  if utf8.len(text) and utf8.len(text) > n then
    return text:sub(1, (utf8.offset(text, n) or n + 1) - 1) .. "…"
  end
  return text
end

-- health: counts from the summary line and the first problem line.
local function parse_health(out, code)
  local h = { code = code }
  local ok, warn, fail = out:match("==%s*summary:%s*(%d+)%s+ok,%s*(%d+)%s+warning%S*,%s*(%d+)%s+failure")
  h.ok, h.warn, h.fail = tonumber(ok), tonumber(warn), tonumber(fail)
  for line in out:gmatch("[^\r\n]+") do
    local text = line:match("^%s*%[FAIL%]%s*(.-)%s*$")
    if text and not h.first_fail then
      h.first_fail = text
    end
    text = line:match("^%s*%[WARN%]%s*(.-)%s*$")
    if text and not h.first_warn then
      h.first_warn = text
    end
  end
  return h
end

-- stats: the six lines of the stats command, found by their shape.
local function parse_stats(out)
  local s = {}
  for line in out:gmatch("[^\r\n]+") do
    line = trim(line)
    local l1, l5, l15 = line:match("^([%d.]+)%s+([%d.]+)%s+([%d.]+)%s+%d+/%d+")
    local size, used, avail = line:match("^(%d+)%s+(%d+)%s+(%d+)$")
    local up = line:match("^([%d.]+)%s+[%d.]+$")
    if l1 then
      s.load1, s.load5, s.load15 = tonumber(l1), tonumber(l5), tonumber(l15)
    elseif size then
      s.disk_size, s.disk_used, s.disk_avail = tonumber(size), tonumber(used), tonumber(avail)
    elseif up then
      s.uptime = tonumber(up)
    elseif line:match("^%d+$") and not s.cpus then
      s.cpus = tonumber(line)
    else
      local key, kb = line:match("^(%w+):%s+(%d+)%s*kB$")
      if key == "MemTotal" then
        s.mem_total = tonumber(kb)
      elseif key == "MemAvailable" then
        s.mem_avail = tonumber(kb)
      end
    end
  end
  return s
end

local function parse_ups(out)
  local u = {}
  for line in out:gmatch("[^\r\n]+") do
    local key, value = line:match("^%s*([%w._]+):%s*(.-)%s*$")
    if key then
      u[key] = value
    end
  end
  return u
end

local function duration(seconds)
  if seconds >= 86400 then
    return string.format("%dd", seconds // 86400)
  elseif seconds >= 3600 then
    return string.format("%dh", seconds // 3600)
  end
  return string.format("%dm", seconds // 60)
end

-- "load 0.8/12  ram 37%  disk 7%  up 5d". Disk as df counts it: used of
-- used + available, rounded up.
local function stats_line(s)
  local parts = {}
  if s.load15 then
    parts[#parts + 1] = string.format("load %.1f", s.load15) .. (s.cpus and ("/" .. s.cpus) or "")
  end
  if s.mem_total and s.mem_avail and s.mem_total > 0 then
    parts[#parts + 1] = string.format("ram %d%%", math.floor((s.mem_total - s.mem_avail) * 100 / s.mem_total + 0.5))
  end
  if s.disk_used and s.disk_avail and s.disk_used + s.disk_avail > 0 then
    parts[#parts + 1] = string.format("disk %d%%", math.ceil(s.disk_used * 100 / (s.disk_used + s.disk_avail)))
  end
  if s.uptime then
    parts[#parts + 1] = "up " .. duration(math.floor(s.uptime))
  end
  if #parts == 0 then
    return nil
  end
  return table.concat(parts, "  ")
end

-- "ups on mains 100% · 13 min"; bad on battery.
local function ups_line(u)
  local status = u["ups.status"] or ""
  local words = {}
  for w in status:gmatch("%S+") do
    words[w] = true
  end
  local state, bad
  if words.LB then
    state, bad = "low battery", true
  elseif words.OB then
    state, bad = "on battery", true
  elseif words.OL then
    state = "on mains"
  else
    state = status ~= "" and status or "status unknown"
  end
  local text = "ups " .. state
  if u["battery.charge"] then
    text = text .. " " .. u["battery.charge"] .. "%"
  end
  local runtime = tonumber(u["battery.runtime"])
  if runtime then
    text = text .. " · " .. math.floor(runtime / 60) .. " min"
  end
  return { label = text, value = "", style = bad and "bad" or nil }
end

-- The header value: "ok 46 · 1 warn", good or bad.
local function health_row(name, h)
  if not h.ok and not h.warn then
    local word = h.code == 0 and "ok" or ("exit " .. tostring(h.code))
    return { label = name, value = word, style = h.code == 0 and "good" or "bad" }
  end
  local value = "ok " .. (h.ok or 0)
  if (h.warn or 0) > 0 then
    value = value .. " · " .. h.warn .. " warn"
  end
  if (h.fail or 0) > 0 then
    value = value .. " · " .. h.fail .. " fail"
  end
  local bad = (h.warn or 0) > 0 or (h.fail or 0) > 0 or (h.code or 0) ~= 0
  return { label = name, value = value, style = bad and "bad" or "good" }
end

-- A command result that means the host did not answer: an error (the
-- time limit) or ssh's exit code 255.
local function unreachable(r)
  return r and (r.error or r.code == 255)
end

return {
  title = "Hosts",
  interval = 300,
  run_key = "h",
  call_timeout = 60,
  update = function()
    local s = telemetrix.settings
    local hosts = type(s.hosts) == "table" and s.hosts or {}
    if #hosts == 0 then
      return { metrics = { { label = "no hosts configured (see PLUGINS.md)", value = "", style = "dim" } } }
    end
    local timeout = math.min(math.max(tonumber(s.timeout) or 30, 1), 55)

    local names, where = {}, {}
    for i, h in ipairs(hosts) do
      for _, part in ipairs({ "health", "stats", "ups" }) do
        if type(h) == "table" and type(h[part]) == "string" and h[part] ~= "" then
          names[#names + 1] = h[part]
          where[#names] = { i, part }
        end
      end
    end
    local results = {}
    if #names > 0 then
      for n, r in ipairs(telemetrix.run_all(names, timeout)) do
        local i, part = where[n][1], where[n][2]
        results[i] = results[i] or {}
        results[i][part] = r
      end
    end

    local store = telemetrix.store_get() or {}
    store.last = type(store.last) == "table" and store.last or {}
    local metrics = {}
    for i, h in ipairs(hosts) do
      local name = type(h) == "table" and tostring(h.name or ("host " .. i)) or ("host " .. i)
      local r = results[i] or {}
      local down = unreachable(r.health) or unreachable(r.stats)
      if down then
        metrics[#metrics + 1] = { label = name, value = "unreachable", style = "bad" }
        local last = store.last[name]
        if type(last) == "table" and type(last.rows) == "table" then
          for _, row in ipairs(last.rows) do
            metrics[#metrics + 1] = { label = row, value = "", style = "dim" }
          end
          metrics[#metrics + 1] = {
            label = "last seen " .. os.date("%H:%M", last.time),
            value = "",
            style = "dim",
          }
        end
      else
        local rows = {}
        if r.health then
          local hh = parse_health(r.health.stdout or "", r.health.code)
          metrics[#metrics + 1] = health_row(name, hh)
          local problem = hh.first_fail or hh.first_warn
          if problem then
            metrics[#metrics + 1] = { label = cut(problem, MAX_TEXT), value = "", style = "dim" }
            rows[#rows + 1] = cut(problem, MAX_TEXT)
          end
        else
          metrics[#metrics + 1] = { label = name, value = "", style = "header" }
        end
        local line = r.stats and r.stats.code == 0 and stats_line(parse_stats(r.stats.stdout or ""))
        if line then
          metrics[#metrics + 1] = { label = line, value = "" }
          rows[#rows + 1] = line
        elseif r.stats then
          local why = r.stats.error or trim(r.stats.stderr) ~= "" and trim(r.stats.stderr) or ("exit " .. tostring(r.stats.code))
          metrics[#metrics + 1] = { label = "stats: " .. cut(why, MAX_TEXT), value = "", style = "dim" }
        end
        if r.ups then
          if r.ups.code == 0 then
            local row = ups_line(parse_ups(r.ups.stdout or ""))
            metrics[#metrics + 1] = row
            rows[#rows + 1] = row.label
          else
            local why = r.ups.error or ("exit " .. tostring(r.ups.code))
            metrics[#metrics + 1] = { label = "ups: " .. cut(why, MAX_TEXT), value = "", style = "dim" }
          end
        end
        store.last[name] = { time = os.time(), rows = rows }
      end
    end
    local ok, err = telemetrix.store_set(store)
    if not ok then
      telemetrix.log("cannot save the store: " .. tostring(err))
    end
    return { metrics = metrics }
  end,
}
