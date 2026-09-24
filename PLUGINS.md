# Writing telemetrix plugins

A plugin is one Lua file. It adds one card to the dashboard. You do not need
to build or restart anything.

## Where plugin files go

telemetrix looks for `*.lua` files in the plugins folder:

1. the folder given with `--plugins-dir <dir>`, or
2. `general.plugins_dir` from `telemetrix.toml` (default `plugins`). A relative
   path is taken next to the executable when that folder exists, otherwise in
   the current folder.

Run `telemetrix plugin list` to see which folder is used and what it finds.

## The smallest plugin

```lua
-- plugins/hello.lua
return {
  title = "Hello",
  interval = 10,
  update = function()
    return {
      metrics = {
        { label = "greeting", value = "hi" },
        { label = "answer", value = 42 },
      },
    }
  end,
}
```

Save the file, then press `r` in the dashboard. telemetrix also looks for new,
changed and removed files every 60 seconds (`plugins.rescan_interval_s`).

## What the file must return

The file must return a table with these fields:

| Field | Required | Meaning |
|---|---|---|
| `update` | yes | A function with no arguments. telemetrix calls it on every interval. |
| `title` | no | The card title. Default: the id. |
| `id` | no | A unique name. Default: the file name without `.lua`. Keep it equal to the file name: telemetrix uses the file name to decide whether to start the plugin, and the id to find its settings. |
| `interval` | no | Seconds between two `update()` calls, 1 or more. Default: `plugins.default_interval_s` (60). |

`update()` must return a table:

```lua
{
  title = "Weather · Kyiv",   -- optional: replaces the title for this update
  metrics = {
    { label = "temp", value = "10.2 °C" },
    { label = "wind", value = 1.5 },       -- numbers and booleans become text
  },
}
```

Every metric needs a `label` and a `value`. Keep labels short: a card is about
30 characters wide. Do not use emoji. Many terminals draw them two cells wide,
and the card then looks broken. Block characters and `°` are fine.

## Settings for your plugin

Each plugin can have its own section in `telemetrix.toml`:

```toml
[plugin.hello]
enabled = true        # false = the plugin does not run
interval = 30         # overrides the interval in the file, 5 or more
name = "world"        # any other key goes to the plugin
```

Two keys are reserved: `enabled` and `interval`. Every other key reaches the
plugin as `telemetrix.settings.<key>`:

```lua
local name = telemetrix.settings.name or "nobody"
```

Always give a default with `or`: the key may be missing. telemetrix refreshes
`telemetrix.settings` before every `update()`, so a saved change to the file
reaches the plugin within a few seconds.

## Functions telemetrix gives you

All of them live in the global table `telemetrix`.

| Function | Returns | Notes |
|---|---|---|
| `http_get(url [, timeout_s])` | `body, status` or `nil, error_text` | HTTP GET. A status like 404 is not an error: check `status` yourself. The body is limited to 1 MiB. The timeout is `plugins.http_timeout_s` unless you give a shorter one. |
| `json_decode(text)` | `table` or `nil, error_text` | JSON `null` becomes `nil`. A list with `null` holes then has gaps, so `#list` may be wrong. |
| `tcp_ping_ms(host, port [, timeout_ms])` | `milliseconds` or `nil, error_text` | Time to open a TCP connection. The default timeout is 2000 ms. |
| `log(text)` | nothing | Writes a line to the log. Press `l` in the dashboard to see it. |
| `now_ms()` | number | Milliseconds from a fixed start point. Use it to measure time, not as a clock. |
| `uptime_s()` | number | Seconds since this computer started. |
| `hostname()` | text | The name of this computer. |
| `settings` | table | Your `[plugin.<name>]` keys, see above. |

`print(...)` also writes to the log. It never writes to the screen.

## What a plugin cannot do

Plugins run in a sandbox. They get the Lua libraries `string`, `table`,
`math` and `utf8`, and from `os` only `time`, `date`, `clock` and `difftime`.
These are removed: `io`, `require`, `load`, `loadstring`, `loadfile`,
`dofile`, and the rest of `os` (`execute`, `exit`, `remove`, `rename`,
`tmpname`, `getenv`, `setlocale`). Only Lua source text is accepted. A file
with precompiled Lua bytecode is refused.

Limits, all set in `telemetrix.toml` under `[plugins]`:

| Limit | Setting | Default |
|---|---|---|
| Lua time for one `update()` call | `call_timeout_s` | 5 s |
| Memory for one plugin | `memory_limit_mb` | 8 MB |
| One HTTP request | `http_timeout_s` | 10 s |
| Plugins that run at the same time | `max_plugins` | 16 |

The time limit counts Lua work. A request that is waiting for the network does
not count as Lua work, but its timeout is cut to the time that is left. So one
call takes at most about the Lua time limit. The one exception is a slow name
lookup (DNS) inside `tcp_ping_ms`, which the limit cannot cut short.

## When something goes wrong

If loading the file or `update()` fails, the card title turns red and shows
the error with its line number, for example
`Error: weather.lua:12: attempt to index a nil value`. The last good values
stay on the card, marked `(stale)`. The error is written to the log once, not
on every retry. telemetrix keeps calling `update()` on the normal interval, so
the card recovers when the problem goes away.

To report a problem on purpose, call `error("message", 0)`. The `0` leaves out
the file and line prefix.

## Checking a plugin

These commands work without the dashboard, so an agent can use them too:

```
telemetrix plugin check plugins/hello.lua          # run update() once, print the card
telemetrix plugin check plugins/hello.lua --json   # the same as JSON
telemetrix plugin list                             # every plugin with id, interval, state
telemetrix snapshot --json --plugins               # metrics and every plugin card, once
```

`plugin check` exits with code 1 when the plugin fails, and prints the error.
It uses the plugin's `[plugin.<name>]` settings from `telemetrix.toml`.

## Notes for agents that write plugins

- Write the file to a temporary name first, then rename it to `<name>.lua`.
  The dashboard may read the folder at any moment and should never see half a
  file.
- Run `telemetrix plugin check <file>` after every change. Exit code 0 means
  the card works.
- Put anything a user may want to change (a city, a host, a list of coins) in
  `[plugin.<name>]` settings, not in the code.

## The default plugins

| File | What it shows | Settings | Network |
|---|---|---|---|
| `clock.lua` | local time and date, every second | none | no |
| `uptime.lua` | computer name and uptime | none | no |
| `network_ping.lua` | time to connect to a host | `host`, `port` | yes |
| `crypto.lua` | coin prices in US dollars (CoinGecko) | `coins` | yes |
| `weather.lua` | temperature and wind (Open-Meteo) | `lat`, `lon`, `label`, `temperature_unit` | yes |
