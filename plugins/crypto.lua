-- Crypto prices in US dollars from CoinGecko (free API, one request for all coins).
-- Settings in [plugin.crypto]: coins = list of CoinGecko ids,
-- default { "bitcoin", "ethereum", "solana" }.
-- On a failed request the card keeps the last prices, marked "(stale)".
local SYMBOLS = { bitcoin = "BTC", ethereum = "ETH", solana = "SOL" }

local function money(v)
  if v >= 1000 then
    local digits = string.format("%.0f", v)
    local grouped = digits:reverse():gsub("(%d%d%d)", "%1,"):reverse():gsub("^,", "")
    return "$" .. grouped
  end
  return string.format("$%.2f", v)
end

return {
  title = "Crypto",
  interval = 120,
  update = function()
    local coins = telemetrix.settings.coins or { "bitcoin", "ethereum", "solana" }
    local url = "https://api.coingecko.com/api/v3/simple/price?vs_currencies=usd&ids="
      .. table.concat(coins, ",")
    local body, status = telemetrix.http_get(url)
    if not body then
      error("request failed: " .. status, 0)
    end
    if status == 429 then
      error("rate limited by CoinGecko (HTTP 429)", 0)
    end
    if status ~= 200 then
      error("CoinGecko answered HTTP " .. status, 0)
    end
    local prices = telemetrix.json_decode(body)
    if type(prices) ~= "table" then
      error("CoinGecko sent something that is not JSON", 0)
    end
    local metrics = {}
    for _, id in ipairs(coins) do
      local entry = prices[id]
      local label = SYMBOLS[id] or id
      if entry and entry.usd then
        metrics[#metrics + 1] = { label = label, value = money(entry.usd) }
      else
        metrics[#metrics + 1] = { label = label, value = "unknown coin" }
      end
    end
    return { metrics = metrics }
  end,
}
