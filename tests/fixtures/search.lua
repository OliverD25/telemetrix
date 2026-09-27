-- A plugin with a search setting, for the runner tests.
return {
  settings_schema = { spot = { kind = "search", label = "spot" } },
  search = function(query)
    if query == "boom" then
      error("no service", 0)
    end
    return { { label = "Spot " .. query, values = { spot = query, rank = 1 } } }
  end,
  update = function()
    return { metrics = { { label = "spot", value = telemetrix.settings.spot } } }
  end,
}
