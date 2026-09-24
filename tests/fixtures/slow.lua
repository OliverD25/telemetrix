-- Test fixture: update() never returns, so the time limit must stop it.
return {
  title = "Slow",
  update = function()
    local n = 0
    while true do
      n = n + 1
    end
  end,
}
