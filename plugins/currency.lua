-- Hryvnia exchange rates: Monobank and PrivatBank (card rate) now, and the
-- official NBU rate of the last 30 days as 7-day and 30-day graphs.
-- Settings in [plugin.currency]:
--   currencies   = { "USD", "EUR", "GBP" }   (file only)
--   primary_bank = "mono" or "privat"       (shown first; the only one when compact)
--   show_month   = true                     (the 30-day graph)
--   compact      = false                    (true: only the primary bank, for narrow cards)
-- Monobank allows one request per 5 minutes. The plugin never asks it more
-- often, even when it runs sooner (a restart, a settings change).
-- A bank that fails keeps its last stored rates, marked stale.

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

local function trend_row(label, points)
  local a, b = points[1], points[#points]
  return { label = label, value = string.format("%+.2f%%", (b - a) / a * 100), trend = points }
end

local function bank_text(bank, r)
  if not r then
    return nil
  end
  if r.buy then
    return string.format("%s %.2f/%.2f", bank, r.buy, r.sell)
  end
  return string.format("%s x%.2f", bank, r.cross)
end

local function short_error(err)
  return #err <= 12 and err or err:sub(1, 11) .. "…"
end

local function currency_rows(cur, store, banks, compact, show_month)
  local parts, paired = {}, false
  for i, bank in ipairs(banks) do
    local b = store[bank] or {}
    local r = b.rates and b.rates[cur]
    local text = (i == 1 or not compact) and bank_text(bank, r)
    if text then
      if compact and b.err then
        text = text .. " (stale)"
      end
      parts[#parts + 1] = text
    end
    paired = paired or (r and r.buy ~= nil)
  end
  local points = store.nbu and store.nbu[cur]
  -- Without a buy/sell pair from any bank (GBP), the official rate helps.
  if not paired and points and #points > 0 then
    parts[#parts + 1] = string.format("NBU %.2f", points[#points])
  end
  local rows = { { label = cur, value = #parts > 0 and table.concat(parts, "  ") or "no rate" } }
  if points and #points >= 2 then
    rows[#rows + 1] = trend_row("7d", last_n(points, 7))
    if show_month then
      rows[#rows + 1] = trend_row("30d", last_n(points, 30))
    end
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
    show_month = { kind = "bool", label = "30-day graph", default = true },
    compact = { kind = "bool", label = "primary bank only", default = false },
  },
  update = function()
    local s = telemetrix.settings
    local currencies = currency_list(s.currencies)
    local banks = s.primary_bank == "privat" and { "privat", "mono" } or { "mono", "privat" }
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
    local metrics = {}
    for _, cur in ipairs(currencies) do
      for _, row in ipairs(currency_rows(cur, store, banks, s.compact, s.show_month)) do
        metrics[#metrics + 1] = row
      end
    end
    for i, bank in ipairs(banks) do
      local b = store[bank]
      if b.err and (i == 1 or not s.compact) then
        local value = b.ok
            and string.format("%s since %s (%s)", bank, os.date("%H:%M", b.ok), short_error(b.err))
          or string.format("%s: %s", bank, short_error(b.err))
        if not (s.compact and b.ok) then
          metrics[#metrics + 1] = { label = b.ok and "stale" or "no data", value = value }
        end
      end
    end
    return { metrics = metrics }
  end,
}
