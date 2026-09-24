-- Test fixture: constant metrics, no network.
return {
  title = "Static",
  interval = 60,
  update = function()
    return {
      metrics = {
        { label = "answer", value = 42 },
        { label = "greeting", value = telemetrix.settings.greeting or "none" },
      },
    }
  end,
}
