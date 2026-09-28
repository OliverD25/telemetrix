# Writing telemetrix plugins

A plugin is one Lua file. It adds one card to the dashboard. You do not need
to build or restart anything.

## Where plugin files go

telemetrix looks for `*.lua` files in one folder:

1. the folder given with `--plugins-dir <dir>` (relative to the current
   folder), or
2. `general.plugins_dir` from `telemetrix.toml`, relative to the folder of
   the settings file, or
3. the **plugin home**: the `plugins` folder next to the settings file.
   That is `%APPDATA%\telemetrix\plugins` on Windows and
   `~/.config/telemetrix/plugins` on Linux. An empty `general.plugins_dir`
   (the default) and the old v0.1 value `"plugins"` both mean the home.

Run `telemetrix plugin list` to see the home, the folder in use, and
whether each plugin is `built-in`, `built-in-edited` or `yours`.

The default plugins are built into telemetrix and copied into the home on
every start. telemetrix never overwrites a built-in plugin you edited and
never restores one you deleted; `telemetrix plugin install --force <name>`
does both on request. So you can edit a built-in plugin in place, or copy
it under a new name and change the copy. Files with other names are always
yours; telemetrix never writes them.

To work on the plugins of this repository, start telemetrix from the
repository folder with `--plugins-dir plugins`.

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
| `run_key` | no | One key that runs `update()` at once, like `"g"`. See [A key that runs the plugin now](#a-key-that-runs-the-plugin-now). |
| `call_timeout` | no | Seconds one `update()` call may take, 1..60. Default: `plugins.call_timeout_s` (5). |
| `settings_schema` | no | The settings a user can change in the `s` box. See [Settings the user can change in the dashboard](#settings-the-user-can-change-in-the-dashboard). |
| `search` | only with a `search` setting | A function `search(query)` that returns choices for the `s` box's search list. See [A setting picked from a search list](#a-setting-picked-from-a-search-list). |

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

Every metric needs a `label` and a `value`. Keep rows short: a card is 30 to
45 characters wide, depending on the terminal. Do not use emoji. Many
terminals draw them two cells wide, and the card then looks broken. Block
characters and `°` are fine.

### A graph in a row

Add `trend`, a list of 2 to 400 numbers, and the row gets a small graph
between the label and the value:

```lua
{ label = "7d", value = "+0.8%", trend = { 44.1, 44.3, 44.2, 44.6, 44.5 } }
```

```
7d ▁▁▁▃▃▃▃▂▂▂▂██████▇▇▇▇ +0.8%
```

The graph fills the space between the label and the value. It is scaled to
its own lowest and highest number. The value is green when the last number
is higher than the first one, and red when it is lower. A `trend` that is
not a list of 2..400 numbers turns only that row into a red error line.

### How a row looks

Add `style` to a metric to change how its row looks:

| `style` | Looks like | Use it for |
|---|---|---|
| `"header"` | bold, in the label colour | a column header, like `buy  sell` above the rates |
| `"dim"` | grey, quieter than the other rows | extra details under a main row |
| `"good"` | the value in green | a value that is fine |
| `"bad"` | the value in red | a value that needs attention, like old data |
| `"bright"` | the label and the value in the value colour | text that must stand out |

With `good` or `bad` and an empty value, the label takes the colour, for
a one-line note like `stale since 14:32 (HTTP 429)`.

```lua
{ label = "", value = "     buy      sell", style = "header" },
{ label = "USD", value = "   44.63     45.03" },
{ label = "stale since 14:32 (HTTP 429)", value = "", style = "bad" },
```

- An unknown `style` is ignored, so the row looks normal.
- A `dim` row that is too narrow for both its label and its value shows
  only the label.
- A row with a `trend` graph ignores `style`.
- The value of a row is right-aligned. Give values the same width (pad them
  with spaces) and they line up as columns.

### Several colours in one row

`label` and `value` can also be a list of spans, `{ text = "...", style = "..." }`.
The texts are joined in order. Each span's `style` (one of the names above)
colours only its own text. A span without a `style`, or with an unknown one,
has the plain colour of its part: the label colour in a label, the value
colour in a value. The row's own `style` still decides what happens on a
narrow card, and colours the parts that are plain text.

```lua
{
  label = { { text = "USD" }, { text = "  44.63   45.03", style = "bright" } },
  value = { { text = " 7d ▂▃▅▆▇▇▆ ", style = "dim" }, { text = "+0.30%", style = "good" } },
  style = "dim",   -- on a narrow card the graph goes and the rates stay
},
{ label = "", value = { { text = "30d ▂▁▃▄▅▆█ ", style = "dim" }, { text = "+0.67%", style = "good" } },
  style = "dim", min_width = 37 },
```

```
USD  44.63   45.03   7d ▂▃▅▆▇▇▆ +0.30%
                    30d ▂▁▃▄▅▆█ +0.67%
```

A span without `text` is an error for the whole update. Spans are ignored on
a row with a `trend` graph; its text is still shown.

### Rows for wide cards only

Add `min_width`, a number of characters, and the row is left out of cards
that are narrower inside their frame. Use it for a detail row that only makes
sense next to another row's value, like the 30-day row above: the `USD` row
is 37 characters wide, so on a narrower card it drops its graph, and the
30-day row goes too. A `min_width` that is not a whole number of 0 or more
is ignored.

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
`telemetrix.settings` before every `update()`. When the plugin's own section
changes, the plugin runs again within a second (see
[Why update() runs](#why-update-runs)).

### Settings the user can change in the dashboard

Declare a setting in `settings_schema`, and it gets a row in the `s` box,
under the plugin's section after `enabled` and `interval`:

```lua
return {
  title = "Weather",
  settings_schema = {
    city  = { kind = "text", label = "city", default = "Kyiv" },
    units = { kind = "enum", label = "units", options = { "metric", "imperial" } },
    days  = { kind = "int", label = "days", min = 1, max = 7, step = 1, default = 3 },
    wind  = { kind = "bool", label = "show wind", default = true },
  },
  update = function()
    local city = telemetrix.settings.city   -- always set: the file value or the default
    ...
  end,
}
```

| Field | Kinds | Meaning |
|---|---|---|
| `kind` | all | `"text"`, `"search"`, `"enum"`, `"bool"` or `"int"` |
| `label` | all | The row name in the `s` box. Default: the key. |
| `default` | all | Used when the file has no value or a wrong one. Without it: `""`, the first option, `false`, or `min`. |
| `options` | enum | The choices, a list of text. |
| `min`, `max` | int | Required. |
| `step` | int | One press of `Left` or `Right`. Default 1. `Shift` moves ten steps. |

- Keys may use `a-z`, `A-Z`, `0-9`, `_` and `-`. `enabled` and `interval`
  cannot be schema keys.
- The rows are sorted by key.
- A broken schema (an unknown kind, a default outside `min..max`) stops the
  plugin from loading. `plugin check` shows the reason.
- telemetrix checks the file values against the schema before they reach
  `telemetrix.settings`. A wrong value (wrong type, out of range, not one of
  the options) gives one warning in the log, and the plugin sees the
  default. A missing value also becomes the default, without a warning.
- Keys that are not in the schema reach the plugin unchanged. Use them for
  things the `s` box cannot edit, like lists.

In the `s` box, `enum`, `bool` and `int` rows change with `Left`, `Right`
and `Enter`, like the other rows. On a `text` row, `Enter` opens an input
line with a cursor:

- type up to 64 characters; `Backspace`, `Delete`, `Left`, `Right`, `Home`
  and `End` work as usual;
- `Enter` saves the text to `[plugin.<id>]` in the settings file, and the
  plugin runs at once (its trigger is `"key"`); an empty text removes the
  key, so the default applies again (an empty text shows as `(not set)`);
- `Esc` closes the input and saves nothing.

While the input is open, every key goes to it: `q` does not quit and `t`
does not open the theme box. Only `Ctrl+C` still quits.

### A setting picked from a search list

Some values are hard to type exactly: a place name has official spellings,
and the program needs its coordinates too. A `search` setting lets the
user type a few letters and pick from a list that your plugin builds:

```lua
return {
  settings_schema = {
    city = { kind = "search", label = "city", default = "" },
  },
  search = function(query)
    -- query: what the user typed, 2 or more characters
    return {
      { label = "Lviv, Lviv Oblast, UA",
        values = { city = "Lviv", lat = 49.84, lon = 24.02, place = "Lviv, Lviv Oblast, UA" } },
      -- up to 8 choices; more are left out
    }
  end,
  update = function() ... end,
}
```

- The row shows the setting's value, like a text row. `Enter` on it opens
  the search box: a text line and a list under it.
- After 2 or more characters and a pause of 400 ms in typing, telemetrix
  asks the plugin's own thread to run `search(query)`. A spinner shows
  while it waits. `Up` and `Down` choose, `Enter` saves, `Esc` closes the
  box and saves nothing. As in the text input, every key goes to the box
  and only `Ctrl+C` quits.
- `Enter` saves **all** keys of the chosen `values` into `[plugin.<id>]`
  in one write, then the plugin runs at once (trigger `"key"`). Values may
  be text, numbers or `true`/`false`. `enabled` and `interval` cannot be
  among them. Keys outside the schema (like `lat` above) are fine.
- `search` has its own time limit of 5 seconds, and HTTP requests inside
  it are cut to what is left. An error (`error("message", 0)`) shows in
  the box. An empty list shows as "nothing found".
- `search` sees `telemetrix.settings` like `update()` does, so it can read
  other settings (the weather plugin reads its preferred `country`).
- A plugin that declares a `search` setting must return a `search`
  function, or it does not load.
- The same Lua state runs `search` and `update()`, one after the other. A
  search waits while `update()` runs.

Try it without the dashboard: `telemetrix plugin check <file> --search <text>`.

## Functions telemetrix gives you

All of them live in the global table `telemetrix`.

| Function | Returns | Notes |
|---|---|---|
| `http_get(url [, timeout_s])` | `body, status, headers` or `nil, error_text` | HTTP GET. A status like 404 is not an error: check `status` yourself. `headers` is a table of the response headers with lower-case names, like `headers["retry-after"]`. The body is limited to 1 MiB. The timeout is `plugins.http_timeout_s` unless you give a shorter one. |
| `json_decode(text)` | `table` or `nil, error_text` | JSON `null` becomes `nil`. A list with `null` holes then has gaps, so `#list` may be wrong. |
| `tcp_ping_ms(host, port [, timeout_ms])` | `milliseconds` or `nil, error_text` | Time to open a TCP connection. The default timeout is 2000 ms. |
| `run(name [, timeout_s])` | `stdout, exit_code, stderr` or `nil, error_text` | Runs the command the user listed as `name` in `[commands]`. See [Running commands](#running-commands). |
| `run_all({ name, ... } [, timeout_s])` | a list of `{ stdout, code, stderr }` or `{ error }` tables | Runs several listed commands at the same time; the results come in the same order. |
| `store_get()` | `table` or `nil` (nothing stored), or `nil, error_text` | What `store_set` saved last, also after a restart. |
| `store_set(table)` | `true` or `nil, error_text` | Saves the table. See [Remembering things](#remembering-things-between-runs). |
| `emit(card)` | `true`, or `false` when dropped | Shows a card at once, before `update()` returns. See [Showing progress](#showing-progress). |
| `trigger()` | text | Why this `update()` runs. See [Why update() runs](#why-update-runs). |
| `speed_multi{ ... }` | result or `nil, error_text` | A real speed test over parallel connections. See [Speed tests](#speed-tests). |
| `speed_download(url, max_bytes, max_seconds [, progress])` | result or `nil, error_text` | See [Speed tests](#speed-tests). |
| `speed_upload(url, bytes, max_seconds [, progress])` | result or `nil, error_text` | See [Speed tests](#speed-tests). |
| `log(text)` | nothing | Writes a line to the log. Press `l` in the dashboard to see it. A text that starts with `warning: ` is shown as a warning. |
| `now_ms()` | number | Milliseconds from a fixed start point. Use it to measure time, not as a clock. |
| `uptime_s()` | number | Seconds since this computer started. |
| `hostname()` | text | The name of this computer. |
| `settings` | table | Your `[plugin.<name>]` keys, see above. |
| `units` | table | The `[units]` settings, read-only: `temperature` is `"celsius"` or `"fahrenheit"`, `bytes` is `"binary"` or `"decimal"`. |

`print(...)` also writes to the log. It never writes to the screen.

### Running commands

A plugin can run programs on this computer, but only the ones the user
listed by name in their own `telemetrix.toml`:

```toml
[commands]
server-a-uptime = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "server-a", "uptime"]
```

```lua
local out, code, err = telemetrix.run("server-a-uptime", 20)
if not out then
  error("uptime: " .. code, 0)   -- code holds the error text here
end
```

**The security model: the user's file decides what can run, and a plugin
only picks a name.**

- `run` and `run_all` take a name and a time limit, nothing else. A plugin
  cannot pass arguments, change the program, or add to the list.
- Only `[commands]` in the user's settings file counts. The built-in
  defaults have no commands, and a plugin's own settings cannot add any.
- Each entry is a list: the program, then its arguments. It runs directly,
  never through a shell on this computer, so `;`, `|`, `>` and `$` in an
  argument are plain text. (A command string that `ssh` sends to a server
  is run by the server's shell; that is the server's business.)
- The time limit is 30 seconds unless the plugin asks for another, at most
  120 seconds, and never longer than what is left of the plugin's
  `call_timeout`. After it, the process is killed and `run` returns
  `nil, "... did not end within N s and was stopped"`.
- telemetrix keeps the first 64 KB of each output and drops the rest.
- On Windows the program starts without a console window, so nothing
  flashes up.
- A wrong entry in `[commands]` (not a list of text, or a name with other
  characters than `a-z`, `0-9`, `_` and `-`) is left out, and
  `config check` says so.
- `exit_code` is `nil` when the program ended without one (killed by a
  signal on Linux).
- `plugin check` prints one `log: run: <name>: exit 0 in 0.8 s, ...` line
  on standard error for every run, so you can see what ran.

The Lua sandbox is otherwise unchanged: there is still no `io`, no
`os.execute` and no other way to start a program.

### Units

`telemetrix.units` holds the user's `[units]` settings, refreshed before
every `update()` like `settings`:

```lua
local u = telemetrix.units
local f = u.temperature == "fahrenheit"            -- or "celsius"
local text = f and string.format("%.1f °F", c * 9 / 5 + 32) or string.format("%.1f °C", c)
-- u.bytes is "binary" (KiB, MiB) or "decimal" (kB, MB)
```

When the user changes `[units]` (in the `s` box or in the file), every
plugin runs again with the trigger `"settings"`, so a card redraws in the
new unit at once. Changing the table from Lua has no effect.

### Remembering things between runs

`store_set(table)` saves a table in a small JSON file, one per plugin.
`store_get()` reads it back, also after the dashboard restarts. Use it for
the last good values, a cache, or a history:

```lua
local store = telemetrix.store_get() or {}
store.count = (store.count or 0) + 1
local ok, err = telemetrix.store_set(store)
if not ok then
  telemetrix.log("cannot save: " .. err)
end
```

- The table may hold text, numbers, booleans and more tables. Functions
  cannot be stored.
- The file may be 64 KB at most. A bigger table is refused with an error.
- The file is written to a temporary name first and then renamed, so a crash
  never leaves half a file.
- The files live in `<data folder>/plugins/<id>.json`. The data folder is
  `--data-dir <dir>`, else `%LOCALAPPDATA%\telemetrix` on Windows and
  `$XDG_DATA_HOME/telemetrix` (or `~/.local/share/telemetrix`) on Linux.
- The store needs a plugin id of `a-z`, `0-9` and `_` only.

### A key that runs the plugin now

With `run_key = "g"`, pressing `g` in the dashboard runs `update()` at once,
without waiting for the interval. Its trigger is then `"key"`.

- It must be one printable character.
- Keys that telemetrix uses itself (`q`, `Q`, `t`, `T`, `s`, `l`, `r`, `u`,
  `v`, `?`, space, `+`, `=`, `-`) are refused, with a warning in the log.
- When two plugins want the same key, the first one keeps it and the other
  gets a warning.

### Longer work

One `update()` call may take `plugins.call_timeout_s` seconds (5 by
default). A plugin that needs longer, like a speed test, sets its own limit:

```lua
return { title = "Speed test", call_timeout = 60, update = ... }
```

The limit is 60 seconds at most. The time counts from the start of the call;
network requests are cut short when the time is up.

### Showing progress

`emit(card)` shows a card at once, while `update()` is still running. It
takes the same table that `update()` returns. The card that `update()`
returns at the end replaces it.

```lua
telemetrix.emit({ metrics = { { label = "testing download...", value = "312 Mbps" } } })
```

- At most 10 cards per second are shown. More are dropped, and `emit`
  returns `false`.
- A card that is not valid is an error, even when it would be dropped.

### Why update() runs

`trigger()` tells the plugin why this `update()` call happens:

| Value | When |
|---|---|
| `"start"` | The first call after the plugin started: the dashboard started, the file changed, or the plugin was turned on. `snapshot --plugins` and `plugin check` (without `--run`) also give `"start"`: they start the plugin fresh and run it once. |
| `"interval"` | A normal call when the interval is over. |
| `"key"` | The user pressed the plugin's `run_key`, or saved a text setting of this plugin in the `s` box. |
| `"manual"` | `telemetrix plugin check <file> --run`. |
| `"settings"` | The plugin's own `[plugin.<id>]` section or the `[units]` section changed, in the `s` box or in the file. The call comes about 0.3 seconds after the change. A change to another plugin's section does not run this plugin. |

Top-level code in the file (outside `update`) runs while the plugin loads;
`trigger()` gives `"start"` there.

Use it to skip expensive work when it is not wanted. The speed-test plugin
shows its stored result on `"start"` and `"settings"`, and tests only on
`"interval"`, `"key"` and `"manual"`:

```lua
local why = telemetrix.trigger()
if why == "start" or why == "settings" then
  return show_last_result()
end
```

### Speed tests

For a real speed test use `speed_multi`. One connection cannot fill a fast
line, and a test of a fixed size ends before the connection is up to speed.
`speed_multi` runs for a fixed time over several connections and leaves out
the first moments:

```lua
local r, err = telemetrix.speed_multi({
  direction = "down",          -- "down" or "up"
  url = "https://host:8080/download?size=25000000",
  streams = 4,                 -- parallel connections, 1..16
  seconds = 3,                 -- how long to measure
  warmup = 0.5,                -- the first 0.5 s is not counted
  piece_bytes = 25000000,      -- "up" only: bytes per upload request
  progress = function(mbps, seconds)
    telemetrix.emit({ metrics = { { label = "testing download...", value = string.format("%.0f Mbps", mbps) } } })
  end,
})
-- r = { mbps = 935.2, bytes = 292000000, seconds = 2.5 }
```

- Only `direction` and `url` matter for most tests; the rest have the
  defaults shown.
- Each connection repeats its request until the time is up. A download
  reads and throws away the body; `r=<number>` is added to each download
  URL so no cache answers it. An upload sends `piece_bytes` zero bytes per
  request, with their length declared (some servers refuse uploads of
  unknown length).
- `mbps` counts only the bytes after the warm-up. `progress` gets the rate
  so far, at most 4 times a second, after the warm-up.
- The call ends within about `seconds` + 1.5 s, even when a server hangs,
  and never runs past the plugin's `call_timeout`.
- Only `https://` addresses are accepted. The connections are closed when
  the call ends.
- The dashboard's memory-budget warning pauses while any speed-test
  function runs and for 10 seconds after it.
- An upload counts the bytes handed to the network connection. The
  computer may still hold a few of them in its send buffer, so an upload
  result can read a few percent high.

`speed_download` and `speed_upload` are simpler: one connection and a
fixed size. They suit a quick check, not a speed test. Neither keeps the
data in memory: the program reads or writes it 16 KB at a time.

- `speed_download(url, max_bytes, max_seconds [, progress])` downloads from
  `url`. It stops after `max_bytes` bytes or `max_seconds` seconds,
  whichever comes first. The server must answer HTTP 200.
- `speed_upload(url, bytes, max_seconds [, progress])` sends `bytes` zero
  bytes with an HTTP POST. It stops early when `max_seconds` runs out.

Both return a table `{ bytes = ..., seconds = ..., mbps = ... }`, or `nil`
and an error text. `progress`, if given, is called as
`progress(bytes, seconds)` at most 4 times per second, with the bytes moved
so far. An error inside `progress` stops the transfer.

```lua
local r, err = telemetrix.speed_download(
  "https://speed.cloudflare.com/__down?bytes=25000000", 25000000, 8,
  function(bytes, seconds)
    telemetrix.emit({ metrics = { { label = "down", value = string.format("%.0f Mbps", bytes * 8 / seconds / 1e6) } } })
  end)
```

All three count against `call_timeout`. Give the plugin enough time for both
directions plus a few seconds.

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
| Time for one `update()` call | `call_timeout_s` (or the plugin's own `call_timeout`) | 5 s |
| Memory for one plugin | `memory_limit_mb` | 8 MB |
| One HTTP request | `http_timeout_s` | 10 s |
| Plugins that run at the same time | `max_plugins` | 16 |

The time limit counts Lua work. A request that is waiting for the network does
not count as Lua work, but its timeout is cut to the time that is left. So one
call takes at most about the time limit. The one exception is a slow name
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
telemetrix plugin check plugins/hello.lua --run    # the same, with trigger() = "manual"
telemetrix plugin check plugins/weather.lua --search lvov   # run search("lvov"), print the list
telemetrix plugin list                             # every plugin with id, interval, state
telemetrix snapshot --json --plugins               # metrics and every plugin card, once
```

`plugin check` exits with code 1 when the plugin fails, and prints the error.
It uses the plugin's `[plugin.<name>]` settings from `telemetrix.toml`. It
also prints the plugin's `settings_schema`, one line per key. Cards sent
with `emit` appear as `progress:` lines, and `log` lines as `log:` lines,
both on standard error.

Without `--run` the trigger is `"start"`, so a plugin like the speed test
only shows its stored result. With `--run` it really runs.

## Notes for agents that write plugins

- Write the file to a temporary name first, then rename it to `<name>.lua`.
  The dashboard may read the folder at any moment and should never see half a
  file.
- Run `telemetrix plugin check <file>` after every change. Exit code 0 means
  the card works.
- Put anything a user may want to change (a city, a host, a list of coins) in
  `[plugin.<name>]` settings, not in the code. Declare it in
  `settings_schema` when the user should change it in the `s` box.
- Call `telemetrix.http_get` and the other functions at call time, not once
  at load time into a local variable. The plugin tests in this repository
  replace them with recorded answers.

## The default plugins

| File | What it shows | Settings | Network |
|---|---|---|---|
| `clock.lua` | local time and date, every second | none | no |
| `uptime.lua` | computer name and uptime | none | no |
| `network_ping.lua` | time to connect to a host | `host`, `port` | yes |
| `currency.lua` | hryvnia rates from one bank (Monobank or PrivatBank, the other as backup), NBU graphs | `primary_bank`; file only: `currencies` | yes |
| `crypto.lua` | coin prices from Binance, 7- and 30-day graphs | `quote`; file only: `coins` | yes |
| `weather.lua` | now, tomorrow and the day after (Open-Meteo) | `city` (a search list), `country`; set by the search: `lat`, `lon`, `place`; file only: `label` | yes |
| `speedtest.lua` | download, upload and ping against the nearest Ookla server (Cloudflare as backup), key `g` | `server`, `streams`, `seconds` | yes, about 700 MB per test at 1 Gbps |
| `hosts.lua` | the health, load, memory, disk, uptime and UPS of your servers, key `h`; off by default | file only: `hosts`, `timeout`; needs `[commands]` | only what your commands do |

These files live in `plugins/` in the repository and are built into the
program, which installs them into the plugin home. The README describes
each data source and its free limits.

- **currency:** a good example of `style`, spans and `min_width`. The
  `buy  sell` row is a `header`. Each currency has two rows: the code with
  its bright rates, and on the right the dim 7-day graph with its change
  in green or red; under it, the 30-day graph. A stale footer is `bad`. The
  small graphs are built in Lua from bar characters, so they keep a fixed
  width and line up; the engine's `trend` graph always fills the whole row.
  On a card narrower than 37 characters the 30-day rows go (`min_width`)
  and the first row drops its graph (`dim`), so only the rates stay.
- **weather:** Weather data by Open-Meteo.com (CC BY 4.0). The license
  requires attribution, so the card ends with a dim `data: Open-Meteo.com`
  line; keep it when you change the plugin. Temperatures follow
  `telemetrix.units.temperature`. The `city` row is a search list: picking
  a place saves `city`, `lat`, `lon` and `place` (the list label). The
  plugin decides in this order:
  1. `place` with `lat` and `lon`, and `place` starts with `city`: the
     picked place. The card title is the place name and country,
     `Weather · Lviv, UA`.
  2. Otherwise a `city`: looked up by name, with the same search as the
     list. A city typed by hand in the file, without `place` or different
     from it, lands here.
  3. Otherwise `lat` and `lon` alone: those coordinates, with `label` as
     the card title. Old v0.1 settings files have exactly that.

  In cases 2 and 3 the plugin writes one line to the log saying what it
  uses, and `config check` warns about `city` with `lat`/`lon` but no
  `place`. The city's schema default is empty, which the `s` box shows as
  `(not set)`; without a city and without coordinates the card shows Kyiv.

  The search: the geocoding service finds only the official spelling or
  its start, and Ukrainian Cyrillic only with `language=uk`. So the plugin
  also tries about 45 old or Russian names (`kiev` → Kyiv, `lvov` → Lviv,
  `киев` → Київ), fixes common Latin spellings (a leading `h` → `kh`,
  `-iy`/`-yy` endings → `-yi`, `c` → `ts`, `i` ↔ `y` in the middle) and
  Russian letters (`ы` → `и`, `-кий` → `-ький`), and at the end shorter
  prefixes (never below 4 letters). It stops as soon as a place in the
  preferred country turns up, and never sends more than 6 requests for
  one search. Airports and regions are left out. Places in `country`
  (default `"UA"`; empty = any) come first, then the biggest. Answers are
  kept for 10 minutes, so typing the same letters again sends nothing.
- **hosts:** a card for your own servers, filled by commands from
  `[commands]` (see [Running commands](#running-commands)). It is off by
  default (`enabled = false`); with no hosts listed it shows a dim
  `no hosts configured (see PLUGINS.md)`. Each host names up to three
  commands:

  ```toml
  [plugin.hosts]
  enabled = true
  hosts = [
    { name = "server-a", health = "server-a-health", stats = "server-a-stats", ups = "server-a-ups" },
  ]

  [commands]
  server-a-health = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "server-a", "/usr/local/bin/health"]
  server-a-stats = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "server-a", 'nproc; cat /proc/loadavg; grep -E "^(MemTotal|MemAvailable):" /proc/meminfo; df -B1 --output=size,used,avail / | tail -1; cat /proc/uptime']
  server-a-ups = ["ssh", "-o", "BatchMode=yes", "-o", "ConnectTimeout=8", "server-a", 'upsc ups@localhost 2>/dev/null | grep -E "^(battery.charge|battery.runtime|input.voltage|ups.load|ups.status):"']
  ```

  The long form `[[plugin.hosts.hosts]]` with `name = ...` lines works too.

  - `health`: a check script that prints `[ OK ]`, `[WARN]` and `[FAIL]`
    lines and a line `== summary: 46 ok, 1 warning(s), 0 failure(s) ==`,
    and exits with 0 (ok), 1 (warnings) or 2 (failures). The card shows
    `server-a  ok 46 · 1 warn` (green when all is well, red otherwise) and
    under it, dimmed, the first `[FAIL]` line, else the first `[WARN]` line.
  - `stats`: the output of the command above. The card shows
    `load 0.8/12  ram 37%  disk 7%  up 5d`: the 15-minute load average and
    the number of CPUs, the memory in use (total minus available), the root
    disk as `df` counts it, and the uptime.
  - `ups` (optional): `upsc` lines. The card shows
    `ups on mains 100% · 13 min` (charge and runtime left), in red
    `on battery` or `low battery`.

  All commands of all hosts run at the same time, each with `timeout`
  seconds (default 30, at most 55), so the card waits at most about a
  minute. When `ssh` cannot reach a host (exit code 255) or a command hits
  its time limit, the host shows `unreachable` in red, and its last values
  stay under it, dimmed, with a `last seen 14:32` line. The card never
  sends alerts. It checks every 5 minutes, and at once when you press `h`.
- **crypto:** old settings files list CoinGecko names (`"bitcoin"`); they
  still work. After an HTTP 429 from Binance the plugin sends no more
  requests until the next interval, because Binance bans addresses that
  keep asking. An HTTP 418 means such a ban: the plugin reads the
  `retry-after` header from `http_get` and waits that long, or else 10
  minutes, doubled for every 418 in a row up to 24 hours. The wait is kept
  in its store, so a restart does not end it.
