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
-- its host ends with exit code 255; then the host shows as down and keeps
-- its last values, dimmed, with the time they were read. A health or stats
-- command stopped at the time limit shows the host as slow, in amber: with
-- fresh bars when only the health check was slow, else with the last ones.
--
-- Each host is a header (a coloured dot, the name, the health summary), bars
-- for load, memory, disk and UPS charge, the first problem line, and a dim
-- footer with the uptime and the time of the check. The values of all bars
-- have the same width, so the bars of the whole card line up.

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

-- "46 min", "5 h", "2 days".
local function duration(seconds)
  seconds = math.max(seconds, 0)
  if seconds >= 86400 then
    local days = seconds // 86400
    return string.format("%d %s", days, days == 1 and "day" or "days")
  elseif seconds >= 3600 then
    return string.format("%d h", seconds // 3600)
  end
  return string.format("%d min", seconds // 60)
end

-- One decimal below 10, a whole number from 10 up.
local function num(x)
  if x < 9.95 then
    return string.format("%.1f", x)
  end
  return string.format("%.0f", x)
end

-- Bytes as "810 GB" or, from 1024 GB, "1.1 TB".
local function size_text(bytes)
  local gb = bytes / 1073741824
  if gb >= 1024 then
    return num(gb / 1024) .. " TB"
  end
  return num(gb) .. " GB"
end

local function clock(time)
  local ok, text = pcall(os.date, "%H:%M", time)
  return ok and text or "?"
end

-- The bar rows of the stats: { label, gauge, value }. Disk as df counts it:
-- used of used + available, rounded up. Load without a CPU count has no
-- scale, so it is a plain row.
local function stat_rows(s)
  local rows = {}
  if s.load15 then
    if s.cpus and s.cpus > 0 then
      rows[#rows + 1] = {
        label = "  load",
        gauge = s.load15 / s.cpus * 100,
        value = string.format("%.1f of %d cpu", s.load15, s.cpus),
      }
    else
      rows[#rows + 1] = { label = string.format("  load %.1f", s.load15), value = "" }
    end
  end
  if s.mem_total and s.mem_avail and s.mem_total > 0 then
    local used = math.max(s.mem_total - s.mem_avail, 0)
    rows[#rows + 1] = {
      label = "  ram ",
      gauge = used * 100 / s.mem_total,
      value = string.format("%s of %s GB", num(used / 1048576), num(s.mem_total / 1048576)),
    }
  end
  if s.disk_used and s.disk_avail and s.disk_used + s.disk_avail > 0 then
    rows[#rows + 1] = {
      label = "  disk",
      gauge = math.ceil(s.disk_used * 100 / (s.disk_used + s.disk_avail)),
      value = size_text(s.disk_avail) .. " free",
    }
  end
  return rows
end

-- The UPS row and its style: bad on battery, warn on a weak battery on
-- mains, else good. Without a charge there is no bar, only text.
local function ups_row(u)
  local status = u["ups.status"] or ""
  local words = {}
  for w in status:gmatch("%S+") do
    words[w] = true
  end
  local state, style
  if words.LB then
    state, style = "low battery", "bad"
  elseif words.OB then
    state, style = "battery", "bad"
  elseif words.OL then
    state = "mains"
  else
    state = status ~= "" and status or "status unknown"
  end
  local charge = tonumber(u["battery.charge"])
  if state == "mains" and charge and charge < 50 then
    style = "warn"
  end
  local value = state
  local runtime = tonumber(u["battery.runtime"])
  if runtime then
    value = value .. " · " .. math.floor(runtime / 60) .. " min"
  end
  local row = { label = "  ups ", gauge = charge, value = value }
  if not charge then
    row.label, row.value = "  ups " .. value, ""
  end
  return row, style or "good"
end

-- The header's colour and value: "ok 46 · 1 warn" in good, warn or bad.
-- A failure or an exit code above 1 is bad; warnings are warn.
local function health_summary(h)
  if not h.ok and not h.warn then
    if h.code == 0 then
      return "good", "ok"
    end
    return "bad", "exit " .. tostring(h.code)
  end
  local value = "ok " .. (h.ok or 0)
  if (h.warn or 0) > 0 then
    value = value .. " · " .. h.warn .. " warn"
  end
  if (h.fail or 0) > 0 then
    value = value .. " · " .. h.fail .. " fail"
  end
  local style = "good"
  if (h.fail or 0) > 0 or (h.code ~= 0 and h.code ~= 1) then
    style = "bad"
  elseif (h.warn or 0) > 0 or h.code == 1 then
    style = "warn"
  end
  return style, value
end

-- "name (place)" shows as "name" and a dim " · place".
local function header(name, dot, value, style)
  local base, place = name:match("^(.-)%s*%(([^()]*)%)%s*$")
  place = place and trim(place) or ""
  if not base or base == "" or place == "" then
    base, place = name, nil
  end
  local label = { { text = "● ", style = dot }, { text = base } }
  if place then
    label[#label + 1] = { text = " · " .. place, style = "dim" }
  end
  return { label = label, value = value, style = style }
end

local function row_metric(g, style)
  return { label = g.label, value = g.value or "", gauge = g.gauge, style = style }
end

-- The bar rows saved by the last good reading; nil for a store of the old
-- format or a broken one.
local function stored_gauges(list)
  if type(list) ~= "table" then
    return nil
  end
  local out = {}
  for _, g in ipairs(list) do
    if type(g) == "table" and type(g.label) == "string" then
      out[#out + 1] = {
        label = g.label,
        gauge = tonumber(g.gauge),
        value = g.value ~= nil and tostring(g.value) or "",
      }
    end
  end
  return out
end

local TIME_LIMIT_ERROR = "did not end within"

-- A command stopped at the time limit: the host answers, but too slowly.
local function timed_out(r)
  return r and r.error and r.error:find(TIME_LIMIT_ERROR, 1, true) ~= nil
end

-- A command result that means the host did not answer: ssh's exit code 255,
-- or an error other than the time limit (a program that does not start).
local function unreachable(r)
  return r and (r.code == 255 or (r.error ~= nil and not timed_out(r)))
end

-- The text of a command that failed.
local function why_failed(r)
  return r.error or trim(r.stderr) ~= "" and trim(r.stderr) or ("exit " .. tostring(r.code))
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

    local now = os.time()
    local store = telemetrix.store_get()
    store = type(store) == "table" and store or {}
    store.last = type(store.last) == "table" and store.last or {}
    local metrics = {}
    for i, h in ipairs(hosts) do
      local name = type(h) == "table" and tostring(h.name or ("host " .. i)) or ("host " .. i)
      local r = results[i] or {}
      if i > 1 then
        metrics[#metrics + 1] = { label = "", value = "" }
      end
      local down = unreachable(r.health) or unreachable(r.stats)
      local slow = not down and (timed_out(r.health) or timed_out(r.stats))
      local slow_text = "slow, over " .. math.floor(timeout) .. " s"
      if down or (slow and timed_out(r.stats)) then
        local last = type(store.last[name]) == "table" and store.last[name] or nil
        local seen = last and tonumber(last.time)
        seen = seen and math.floor(seen)
        if down then
          local value = seen and ("down " .. duration(now - seen)) or "unreachable"
          metrics[#metrics + 1] = header(name, "bad", value, "bad")
        else
          metrics[#metrics + 1] = header(name, "warn", slow_text, "warn")
        end
        if last then
          local gauges = stored_gauges(last.gauges)
          if gauges then
            for _, g in ipairs(gauges) do
              metrics[#metrics + 1] = row_metric(g, "dim")
            end
          elseif type(last.rows) == "table" then
            -- The first versions of this plugin saved ready-made text.
            for _, row in ipairs(last.rows) do
              if type(row) == "string" then
                metrics[#metrics + 1] = { label = "  " .. row, value = "", style = "dim" }
              end
            end
          end
          if seen then
            metrics[#metrics + 1] = { label = "  last seen " .. clock(seen), value = "", style = "dim" }
          end
        end
      else
        -- Only the health check ran out of time: the stats are fresh.
        local health = not slow and r.health or nil
        local body, saved, uptime, stats_failed = {}, {}, nil, false
        if r.stats then
          local rows = {}
          if r.stats.code == 0 then
            local st = parse_stats(r.stats.stdout or "")
            rows, uptime = stat_rows(st), st.uptime
          end
          if #rows > 0 then
            for _, g in ipairs(rows) do
              body[#body + 1] = row_metric(g)
              saved[#saved + 1] = g
            end
          else
            stats_failed = true
            body[#body + 1] = { label = "  stats: " .. cut(why_failed(r.stats), MAX_TEXT), value = "", style = "dim" }
          end
        end
        if r.ups then
          if r.ups.code == 0 then
            local g, style = ups_row(parse_ups(r.ups.stdout or ""))
            body[#body + 1] = row_metric(g, style)
            saved[#saved + 1] = g
          else
            local why = r.ups.error or ("exit " .. tostring(r.ups.code))
            body[#body + 1] = { label = "  ups: " .. cut(why, MAX_TEXT), value = "", style = "dim" }
          end
        end

        local dot, value, style = stats_failed and "warn" or "good", "", nil
        local problem, problem_style
        if health then
          local hh = parse_health(health.stdout or "", health.code)
          style, value = health_summary(hh)
          dot = style
          if hh.first_fail then
            problem, problem_style = hh.first_fail, "bad"
          elseif hh.first_warn then
            problem, problem_style = hh.first_warn, "warn"
          end
        elseif slow then
          dot, value, style = "warn", slow_text, "warn"
        end
        metrics[#metrics + 1] = header(name, dot, value, style)
        for _, m in ipairs(body) do
          metrics[#metrics + 1] = m
        end
        if problem then
          metrics[#metrics + 1] = { label = "  ! " .. cut(problem, MAX_TEXT), value = "", style = problem_style }
        end
        metrics[#metrics + 1] = {
          label = uptime and ("  up " .. duration(math.floor(uptime))) or "",
          value = "checked " .. clock(now),
          style = "dim",
        }
        store.last[name] = { time = now, gauges = saved }
      end
    end

    -- Values of the same width keep the bars of all rows in one column.
    local width = 0
    for _, m in ipairs(metrics) do
      if m.gauge then
        width = math.max(width, utf8.len(m.value) or #m.value)
      end
    end
    for _, m in ipairs(metrics) do
      if m.gauge then
        m.value = m.value .. string.rep(" ", width - (utf8.len(m.value) or #m.value))
      end
    end

    local ok, err = telemetrix.store_set(store)
    if not ok then
      telemetrix.log("cannot save the store: " .. tostring(err))
    end
    return { metrics = metrics }
  end,
}
