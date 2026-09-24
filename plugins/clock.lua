-- Clock: local time and date. No network. The simplest possible plugin.
return {
  title = "Clock",
  interval = 1,
  update = function()
    return {
      metrics = {
        { label = "time", value = os.date("%H:%M:%S") },
        { label = "date", value = os.date("%Y-%m-%d") },
      },
    }
  end,
}
