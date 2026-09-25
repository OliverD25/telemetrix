-- Hryvnia exchange rates from one bank at a time: Monobank or PrivatBank
-- (card rate). Under each currency, one quiet line with the 7-day and
-- 30-day graphs of the official NBU rate.
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

local function period(label, points)
  local a, b = points[1], points[#points]
  return string.format("%s %s %+.2f%%", label, spark(points, SPARK_WIDTH), (b - a) / a * 100)
end

-- Two right-aligned columns of the same width, so rows line up.
local function columns(left, right)
  return string.format("%8s  %8s", left, right)
end

local function short_error(err)
  return #err <= 12 and err or err:sub(1, 11) .. "…"
end

local function currency_rows(cur, rates, points)
  local r = rates and rates[cur]
  local value
  if r and r.buy then
    value = columns(string.format("%.2f", r.buy), string.format("%.2f", r.sell))
  elseif r and r.cross then
    value = columns(string.format("%.2f", r.cross), "cross")
  elseif points and #points > 0 then
    value = columns(string.format("%.2f", points[#points]), "NBU")
  else
    value = "no rate"
  end
  local rows = { { label = cur, value = value } }
  if points and #points >= 2 then
    rows[2] = {
      label = "  " .. period("7d", last_n(points, 7)),
      value = period("30d", last_n(points, 30)),
      style = "dim",
    }
  end
  return rows
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
    local metrics = { { label = "", value = columns("buy", "sell"), style = "header" } }
    for _, cur in ipairs(currencies) do
      for _, row in ipairs(currency_rows(cur, b.rates, store.nbu and store.nbu[cur])) do
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
