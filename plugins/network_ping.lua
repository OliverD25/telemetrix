-- Internet latency: time to open a TCP connection to a host.
-- Settings in [plugin.network_ping]: host (default "1.1.1.1"), port (default 443).
return {
  title = "Internet Latency",
  interval = 30,
  update = function()
    local host = telemetrix.settings.host or "1.1.1.1"
    local port = math.tointeger(telemetrix.settings.port) or 443
    local ms, err = telemetrix.tcp_ping_ms(host, port, 2000)
    if not ms then
      error("no connection to " .. host .. ":" .. port .. " (" .. err .. ")", 0)
    end
    return {
      metrics = {
        { label = host .. ":" .. port, value = string.format("%.0f ms", ms) },
      },
    }
  end,
}
