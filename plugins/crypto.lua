-- Crypto prices from Binance (free, no key): the price every minute and a
-- daily history for the 7-day and 30-day graphs, fetched at most once an hour.
-- Settings in [plugin.crypto]:
--   coins = { "BTC", "ETH", "SOL" }   (file only; old CoinGecko ids like "bitcoin" also work)
--   quote = "USDT"                    (the currency prices are in)
-- A failed request keeps the last prices, marked "(stale)".
-- Binance allows a request weight of 6000 per minute per IP address and
-- bans an address that keeps asking after HTTP 429 (2 minutes up to
-- 3 days). After a 429 the plugin sends nothing more until the next
-- interval. A 418 means the address is banned: the plugin waits as long as
-- Binance's Retry-After header says, or else 10 minutes, doubled for every
-- 418 in a row up to 24 hours. It shows the stored prices meanwhile.

local PRICE_URL = "https://api.binance.com/api/v3/ticker/price?symbols="
local KLINES_URL = "https://api.binance.com/api/v3/klines?symbol=%s&interval=1d&limit=31"
local HISTORY_EVERY = 3600
local BAN_FIRST = 600
local BAN_MAX = 86400
local COINGECKO = { bitcoin = "BTC", ethereum = "ETH", solana = "SOL" }

local function urlencode(text)
  return (text:gsub("[^%w%-_.~]", function(c)
    return string.format("%%%02X", c:byte())
  end))
end

-- 429: too many requests; 418: the address is banned for a while.
local SLOW_DOWN = { [429] = true, [418] = true }

-- The decoded answer, or nil, the error, whether Binance said slow down
-- and the Retry-After seconds of a 418.
local function get_json(url)
  local body, status, headers = telemetrix.http_get(url)
  if not body then
    return nil, status
  end
  if status ~= 200 then
    local retry = status == 418 and headers and tonumber(headers["retry-after"]) or nil
    return nil, "HTTP " .. status, SLOW_DOWN[status] or false, retry
  end
  local data = telemetrix.json_decode(body)
  if type(data) ~= "table" then
    return nil, "not JSON"
  end
  return data
end

local function coin_list(value)
  local out = {}
  if type(value) == "table" then
    for _, c in ipairs(value) do
      if type(c) == "string" and c ~= "" then
        out[#out + 1] = COINGECKO[c:lower()] or c:upper()
      end
    end
  end
  if #out == 0 then
    out = { "BTC", "ETH", "SOL" }
  end
  return out
end

local function grouped(digits)
  return (digits:reverse():gsub("(%d%d%d)", "%1,"):reverse():gsub("^,", ""))
end

local function money(v)
  if v >= 100 then
    return grouped(string.format("%.0f", v))
  elseif v >= 1 then
    return string.format("%.2f", v)
  end
  return string.format("%.4f", v)
end

-- Closing prices of the daily candles (the 5th field, a string).
local function closes(klines)
  local out = {}
  for _, k in ipairs(klines) do
    local close = type(k) == "table" and tonumber(k[5])
    if close then
      out[#out + 1] = close
    end
  end
  return out
end

local function last_n(points, n)
  local out = {}
  for i = math.max(1, #points - n + 1), #points do
    out[#out + 1] = points[i]
  end
  return out
end

-- The wait after the streak-th 418 in a row: Retry-After when Binance sent
-- it, else 10 minutes doubled per 418, at most 24 hours.
local function ban_seconds(streak, retry_after)
  if retry_after and retry_after > 0 then
    return math.ceil(retry_after)
  end
  return math.min(BAN_FIRST * 2 ^ (streak - 1), BAN_MAX)
end

local function clock(t, now)
  if t - now < 86400 then
    return os.date("%H:%M", t)
  end
  return os.date("%d %b %H:%M", t)
end

local function trend_row(label, points)
  local a, b = points[1], points[#points]
  return { label = label, value = string.format("%+.1f%%", (b - a) / a * 100), trend = points }
end

return {
  title = "Crypto",
  interval = 60,
  settings_schema = {
    quote = { kind = "text", label = "quote currency", default = "USDT" },
  },
  update = function()
    local s = telemetrix.settings
    local coins = coin_list(s.coins)
    local quote = (s.quote or "USDT"):upper()
    local store = telemetrix.store_get() or {}
    if store.quote ~= quote then
      store = { quote = quote, ban_until = store.ban_until, ban_streak = store.ban_streak }
    end
    store.prices, store.history = store.prices or {}, store.history or {}
    local now = os.time()

    local banned = store.ban_until and now < store.ban_until
    local symbols = {}
    for i, coin in ipairs(coins) do
      symbols[i] = '"' .. coin .. quote .. '"'
    end
    local list, err, slow, retry
    if banned then
      err, slow = "paused by Binance", true
    else
      list, err, slow, retry = get_json(PRICE_URL .. urlencode("[" .. table.concat(symbols, ",") .. "]"))
    end
    local fresh = {}
    for _, p in ipairs(list or {}) do
      local price = tonumber(p.price)
      if type(p.symbol) == "string" and price then
        fresh[p.symbol] = price
      end
    end
    local stale = next(fresh) == nil
    if not stale then
      store.prices = fresh
      store.ban_until, store.ban_streak = nil, nil
    end

    local http_418 = err == "HTTP 418"
    for _, coin in ipairs(coins) do
      local h = store.history[coin]
      if not slow and (not h or now - (h.ts or 0) >= HISTORY_EVERY) then
        local klines, kerr, told, kretry = get_json(string.format(KLINES_URL, coin .. quote))
        slow, retry = told, kretry
        http_418 = kerr == "HTTP 418"
        local points = klines and closes(klines)
        if points and #points >= 2 then
          store.history[coin] = { ts = now, closes = points }
        end
      end
    end
    if http_418 then
      store.ban_streak = (store.ban_streak or 0) + 1
      local wait = ban_seconds(store.ban_streak, retry)
      store.ban_until = now + wait
      telemetrix.log(string.format(
        "Binance answered HTTP 418 (this address is banned); no requests until %s (%s)",
        clock(store.ban_until, now),
        retry and ("Retry-After " .. retry .. " s") or ("418 number " .. store.ban_streak .. " in a row")
      ))
    elseif slow and not banned then
      telemetrix.log("Binance asked to slow down; no more requests until the next interval")
    end
    local paused = store.ban_until and now < store.ban_until
    local ok, store_err = telemetrix.store_set(store)
    if not ok then
      telemetrix.log("cannot save the store: " .. store_err)
    end

    local metrics = {}
    local pause_row = paused
      and { label = "paused by Binance until " .. clock(store.ban_until, now), value = "", style = "bad" }
    if not next(store.prices) then
      if pause_row then
        return { metrics = { pause_row } }
      end
      error("no prices yet: " .. (err or "Binance sent none"), 0)
    end
    for _, coin in ipairs(coins) do
      local price = store.prices[coin .. quote]
      local value = price and (money(price) .. " " .. quote) or "unknown coin"
      if price and stale then
        value = value .. " (stale)"
      end
      metrics[#metrics + 1] = { label = coin, value = value }
      local h = store.history[coin]
      if h and #h.closes >= 2 then
        local points = last_n(h.closes, 30)
        -- Today's candle closes at the current price, so the graph ends where the price is.
        if price then
          points[#points] = price
        end
        metrics[#metrics + 1] = trend_row("7d", last_n(points, 7))
        metrics[#metrics + 1] = trend_row("30d", points)
      end
    end
    metrics[#metrics + 1] = pause_row or nil
    return { metrics = metrics }
  end,
}
