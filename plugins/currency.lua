-- Hryvnia exchange rates from one bank at a time: Monobank or PrivatBank
-- (card rate). Beside each currency, a quiet column with the 7-day graph
-- and, one line lower, the 30-day graph of the official NBU rate.
-- Settings in [plugin.currency]:
--   currencies   = { "USD", "EUR", "GBP" }   (file only)
--   primary_bank = "mono" or "privat"       (the bank the card shows)
-- The other bank is only a backup: it is shown when the primary bank fails
-- and has no rates from the last hour. Both banks are still asked on every
-- update, so the backup is ready.
-- Monobank allows one request per 5 minutes. The plugin never asks it more
-- often, even when it runs sooner (a restart, a settings change).

local MONO_URL = "https://api.monobank.ua/bank/currency"
local PRIVAT_URL = "https://api.privatbank.ua/p24api/pubinfo?json&exchange&coursid=11"
local NBU_URL = "https://bank.gov.ua/NBU_Exchange/exchange_site?start=%s&end=%s"
  .. "&valcode=%s&sort=exchangedate&order=asc&json"
local MONO_GAP = 300
local DAY = 86400
local UAH = 980
-- ISO 4217 numbers, as Monobank sends them.
local ISO = { USD = 840, EUR = 978, GBP = 826, PLN = 985, CHF = 756, CZK = 203, JPY = 392 }

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

local function parse_mono(list)
  local names = {}
  for name, code in pairs(ISO) do
    names[code] = name
  end
  local rates = {}
  for _, r in ipairs(list) do
    local name = names[r.currencyCodeA]
    if name and r.currencyCodeB == UAH then
      if r.rateBuy and r.rateSell then
        rates[name] = { buy = r.rateBuy, sell = r.rateSell }
      elseif r.rateCross then
        rates[name] = { cross = r.rateCross }
      end
    end
  end
  return rates
end

local function parse_privat(list)
  local rates = {}
  for _, r in ipairs(list) do
    local buy, sell = tonumber(r.buy), tonumber(r.sale)
    if type(r.ccy) == "string" and r.base_ccy == "UAH" and buy and sell then
      rates[r.ccy] = { buy = buy, sell = sell }
    end
  end
  return rates
end

local function parse_nbu(list)
  local points = {}
  for _, r in ipairs(list) do
    if type(r.rate) == "number" then
      points[#points + 1] = r.rate
    end
  end
  return points
end

-- Asks one bank, unless it was asked less than `gap` seconds ago.
local function refresh(b, now, url, parse, gap)
  if gap and b.tried and now - b.tried < gap then
    return
  end
  b.tried = now
  local data, err = get_json(url)
  local rates = data and parse(data)
  if rates and next(rates) then
    b.rates, b.ok, b.err = rates, now, nil
  else
    b.err = err or "no rates"
  end
end

local function refresh_history(store, currencies, now)
  local today = os.date("%Y-%m-%d", now)
  if store.history_date == today then
    return
  end
  local first, last = os.date("%Y%m%d", now - 31 * DAY), os.date("%Y%m%d", now)
  local complete = true
  store.nbu = store.nbu or {}
  for _, cur in ipairs(currencies) do
    local data = get_json(string.format(NBU_URL, first, last, cur:lower()))
    local points = data and parse_nbu(data)
    if points and #points >= 2 then
      store.nbu[cur] = points
    else
      complete = false
    end
  end
  if complete then
    store.history_date = today
  end
end

local function last_n(points, n)
  local out = {}
  for i = math.max(1, #points - n + 1), #points do
    out[#out + 1] = points[i]
  end
  return out
end

local BARS = { "▁", "▂", "▃", "▄", "▅", "▆", "▇", "█" }
local SPARK_WIDTH = 7
-- Rates older than this make the card switch to the backup bank.
local FRESH = 3600
local NAMES = { mono = "monobank", privat = "privatbank" }

-- `points` as `width` bar glyphs, scaled to their own lowest and highest value.
local function spark(points, width)
  local lo, hi = math.huge, -math.huge
  for _, p in ipairs(points) do
    lo, hi = math.min(lo, p), math.max(hi, p)
  end
  local out = {}
  for i = 1, width do
    local idx = #points
    if width > 1 then
      idx = ((i - 1) * (#points - 1) + (width - 1) // 2) // (width - 1) + 1
    end
    local level = hi > lo and math.floor((points[idx] - lo) / (hi - lo) * 7 + 0.5) + 1 or 4
    out[i] = BARS[level]
  end
  return table.concat(out)
end

-- Two right-aligned rate columns after the currency code, so rows line up.
local function rates_text(left, right)
  return string.format(" %6s  %6s", left, right)
end

local function short_error(err)
  return #err <= 12 and err or err:sub(1, 11) .. "…"
end

local function change(points)
  local a, b = points[1], points[#points]
  return (b - a) / a * 100
end

-- ` 7d ▂▃▅▆▇▇▆ +0.30%` as spans: a quiet graph and a coloured change. Every
-- change is padded to `change_w`, so the graphs of all rows line up.
local function period(label, points, change_w)
  local pct = change(points)
  local style = pct > 0 and "good" or pct < 0 and "bad" or nil
  return {
    { text = string.format("%3s %s ", label, spark(points, SPARK_WIDTH)), style = "dim" },
    { text = string.format("%" .. change_w .. "s", string.format("%+.2f%%", pct)), style = style },
  }
end

local function width(spans)
  local n = 0
  for _, s in ipairs(spans) do
    n = n + utf8.len(s.text)
  end
  return n
end

-- The rates of one currency: buy and sell, a cross rate, or the NBU rate.
local function rates_of(cur, rates, points)
  local r = rates and rates[cur]
  if r and r.buy then
    return rates_text(string.format("%.2f", r.buy), string.format("%.2f", r.sell))
  elseif r and r.cross then
    return rates_text(string.format("%.2f", r.cross), "cross")
  elseif points and #points > 0 then
    return rates_text(string.format("%.2f", points[#points]), "NBU")
  end
  return string.format(" %-14s", "no rate")
end

-- Two rows per currency: the rates and the 7-day graph, then the 30-day graph
-- under it. The row with the rates is `dim` only so that a narrow card drops
-- its graph; its spans set every colour. The 30-day row leaves a card too
-- narrow for the first row's graph.
local function currency_rows(cur, code_w, rates, points, change_w)
  local label = {
    { text = string.format("%-" .. code_w .. "s", cur) },
    { text = rates_of(cur, rates, points), style = "bright" },
  }
  if not (points and #points >= 2) then
    return { { label = label, value = "" } }
  end
  local week = period("7d", last_n(points, 7), change_w)
  local full = width(label) + 1 + width(week)
  return {
    { label = label, value = week, style = "dim" },
    { label = "", value = period("30d", last_n(points, 30), change_w), style = "dim", min_width = full },
  }
end

-- The widest change of all graphs, at least 6 characters (`+0.30%`).
local function change_width(currencies, nbu)
  local w = 6
  for _, cur in ipairs(currencies) do
    local points = nbu and nbu[cur]
    if points and #points >= 2 then
      for _, n in ipairs({ 7, 30 }) do
        w = math.max(w, #string.format("%+.2f%%", change(last_n(points, n))))
      end
    end
  end
  return w
end

local function currency_list(value)
  local out = {}
  if type(value) == "table" then
    for _, c in ipairs(value) do
      if type(c) == "string" then
        out[#out + 1] = c:upper()
      end
    end
  end
  if #out == 0 then
    out = { "USD", "EUR", "GBP" }
  end
  return out
end

-- The bank the card shows: the primary one, unless it failed and has no
-- rates from the last hour while the other bank has some.
local function shown_bank(store, primary, now)
  local other = primary == "mono" and "privat" or "mono"
  local p = store[primary]
  local usable = p.rates and (not p.err or now - (p.ok or 0) < FRESH)
  if usable or not store[other].rates then
    return primary, nil
  end
  local why = p.err or "no rates"
  if p.ok then
    why = why .. ", last rates " .. os.date("%H:%M", p.ok)
  end
  return other, why
end

-- The last "which bank" note, so the log gets one line per change.
local last_note

return {
  title = "Currency",
  interval = 300,
  settings_schema = {
    primary_bank = {
      kind = "enum",
      label = "primary bank",
      options = { "mono", "privat" },
      default = "mono",
    },
  },
  update = function()
    local s = telemetrix.settings
    local currencies = currency_list(s.currencies)
    local primary = s.primary_bank == "privat" and "privat" or "mono"
    local store = telemetrix.store_get() or {}
    store.mono, store.privat = store.mono or {}, store.privat or {}
    local now = os.time()
    refresh(store.mono, now, MONO_URL, parse_mono, MONO_GAP)
    refresh(store.privat, now, PRIVAT_URL, parse_privat)
    refresh_history(store, currencies, now)
    local ok, err = telemetrix.store_set(store)
    if not ok then
      telemetrix.log("cannot save the store: " .. err)
    end

    if not store.mono.rates and not store.privat.rates and not next(store.nbu or {}) then
      error("no rates yet: mono " .. (store.mono.err or "?")
        .. ", privat " .. (store.privat.err or "?"), 0)
    end
    local bank, why = shown_bank(store, primary, now)
    local note = why and string.format("using %s as the backup: %s %s", NAMES[bank], NAMES[primary], why)
    if note ~= last_note then
      if note then
        telemetrix.log(note)
      end
      last_note = note
    end

    local b = store[bank]
    local code_w = 3
    for _, cur in ipairs(currencies) do
      code_w = math.max(code_w, utf8.len(cur) or #cur)
    end
    local change_w = change_width(currencies, store.nbu)
    local metrics = {
      { label = string.rep(" ", code_w) .. rates_text("buy", "sell"), value = "", style = "header" },
    }
    for _, cur in ipairs(currencies) do
      local points = store.nbu and store.nbu[cur]
      for _, row in ipairs(currency_rows(cur, code_w, b.rates, points, change_w)) do
        metrics[#metrics + 1] = row
      end
    end
    if b.ok and b.err then
      metrics[#metrics + 1] = {
        label = string.format("stale since %s (%s)", os.date("%H:%M", b.ok), short_error(b.err)),
        value = "",
        style = "bad",
      }
    elseif b.ok then
      metrics[#metrics + 1] = { label = "updated " .. os.date("%H:%M", b.ok), value = "", style = "dim" }
    else
      metrics[#metrics + 1] = { label = NAMES[bank] .. ": " .. short_error(b.err or "no rates"), value = "", style = "bad" }
    end
    return {
      title = "Currency · " .. NAMES[bank] .. (why and " (backup)" or ""),
      metrics = metrics,
    }
  end,
}
