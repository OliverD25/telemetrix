-- Test fixture: counts its own runs in the plugin store.
return {
  title = "Store",
  update = function()
    local data = telemetrix.store_get() or { runs = 0 }
    data.runs = data.runs + 1
    assert(telemetrix.store_set(data))
    return { metrics = { { label = "runs", value = data.runs } } }
  end,
}
