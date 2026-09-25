-- Crypto prices from Binance (free, no key): the price every minute and a
-- daily history for the 7-day and 30-day graphs, fetched at most once an hour.
-- Settings in [plugin.crypto]:
--   coins = { "BTC", "ETH", "SOL" }   (file only; old CoinGecko ids like "bitcoin" also work)
--   quote = "USDT"                    (the currency prices are in)
-- A failed request keeps the last prices, marked "(stale)".

local PRICE_URL = "https://api.binance.com/api/v3/ticker/price?symbols="
local KLINES_URL = "https://api.binance.com/api/v3/klines?symbol=%s&interval=1d&limit=31"
local HISTORY_EVERY = 3600
local COINGECKO = { bitcoin = "BTC", ethereum = "ETH", solana = "SOL" }

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
      store = { quote = quote }
    end
    store.prices, store.history = store.prices or {}, store.history or {}
    local now = os.time()

    local symbols = {}
    for i, coin in ipairs(coins) do
      symbols[i] = '"' .. coin .. quote .. '"'
    end
    local list, err = get_json(PRICE_URL .. urlencode("[" .. table.concat(symbols, ",") .. "]"))
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
    end

    for _, coin in ipairs(coins) do
      local h = store.history[coin]
      if not h or now - (h.ts or 0) >= HISTORY_EVERY then
        local klines = get_json(string.format(KLINES_URL, coin .. quote))
        local points = klines and closes(klines)
        if points and #points >= 2 then
          store.history[coin] = { ts = now, closes = points }
        end
      end
    end
    local ok, store_err = telemetrix.store_set(store)
    if not ok then
      telemetrix.log("cannot save the store: " .. store_err)
    end

    if not next(store.prices) then
      error("no prices yet: " .. (err or "Binance sent none"), 0)
    end
    local metrics = {}
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
    return { metrics = metrics }
  end,
}
