-- Test plugin: declares settings and shows why update() ran.
return {
  title = "Schema",
  interval = 60,
  settings_schema = {
    city = { kind = "text", label = "City", default = "Kyiv" },
    bank = { kind = "enum", options = { "mono", "privat" } },
  },
  update = function()
    local s = telemetrix.settings
    return {
      metrics = {
        { label = "trigger", value = telemetrix.trigger() },
        { label = "city", value = s.city },
        { label = "bank", value = s.bank },
      },
    }
  end,
}
