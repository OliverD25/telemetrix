# telemetrix

**TermSaver**: a very light terminal screensaver and live system dashboard for
Windows 11 and Linux, written in Rust.

It shows CPU, memory, swap, disks and CPU temperature (where the computer
reports it), plus cards from small Lua plugins: weather, crypto prices,
internet latency, a clock, uptime, or anything you write yourself. It uses
about 7 MB of memory without plugins and about 11 MB with the five default
plugins, and almost no CPU on the static theme. Settings live in a commented
`telemetrix.toml` that a wrong value can never break.

## Build and run

You need Rust 1.95 or newer. On Windows you also need the Visual Studio C++
build tools, because the Lua interpreter is compiled from C source.

```
cargo build --release
```

The program is `target/release/telemetrix` (`telemetrix.exe` on Windows),
about 2.8 MB. Put the `plugins` folder next to it, or start it from the
project folder.

```
telemetrix                        # the dashboard
telemetrix --theme minimalist     # start with the quiet theme
telemetrix --exit-on-any-key      # screensaver mode: any key quits
```

## Keys

| Key | What it does |
|---|---|
| `q`, `Esc`, `Ctrl+C` | quit (`Esc` closes an open box first) |
| `t` / `T` | next / previous theme (saved in the settings file) |
| `s` | settings: every change is saved at once |
| `+` / `-` | more / fewer frames per second (saved) |
| `space` | pause the animation |
| `l` | log, with the program's own memory use at the top |
| `r` | look for new, changed and removed plugins now |
| `?` | help |

Inside the settings box: `Up`/`Down` move, `Enter` or `Right` pick the next
value, `Left` the previous one, `Shift` with `Left`/`Right` moves ten steps.
Paths are shown but can only be changed in the file.

## Flags

| Flag | Meaning |
|---|---|
| `--config <path>` | use this settings file |
| `--theme <name>` | `minimalist` or `matrix` |
| `--fps <n>` | frames per second for animated themes, 1..60 |
| `--plugins-dir <dir>` | folder with `.lua` plugins |
| `--no-plugins` | run no plugins |
| `--exit-on-any-key` | screensaver mode: any key quits |
| `--log <path>` | also write the log to this file |
| `-h`, `--help` / `-V`, `--version` | help / version |

Flags never write to the settings file. If you change a value in the
dashboard that a flag set, your choice wins from then on.

## Commands without the dashboard

These need no terminal window, so scripts and agents can use them.

| Command | What it does |
|---|---|
| `telemetrix snapshot [--json] [--plugins]` | read the metrics once (and run every plugin once) and print them |
| `telemetrix config init [--force]` | write the default settings file with comments |
| `telemetrix config path` | print where the settings file is and whether it exists |
| `telemetrix config check [--json]` | list every problem in the settings file, with line numbers; exit 1 if there is one |
| `telemetrix config show` | print every setting, its value and where it came from (default, file or flag) |
| `telemetrix config reference` | print the settings table below |
| `telemetrix plugin check <file> [--json]` | run a plugin once and print its card or its error; exit 1 on error |
| `telemetrix plugin list` | list the plugins that would run |
| `telemetrix themes` | list the theme names |
| `telemetrix selftest --memory [--seconds N] [--json]` | measure memory against the budgets; exit 1 if over (see below) |

## Themes

- **matrix** (the default): digital rain behind the cards, 15 frames per
  second. Density, speed and colour are in `[theme.matrix]`. On very large
  terminals (over 20 000 cells) it limits itself to 10 frames per second.
- **minimalist**: grey cards, colour only where a value is above its
  threshold. It redraws only when data changes.

Both themes lay the cards out in three columns on wide terminals, two on
medium ones and one on narrow ones. Below 40 by 10 cells the screen only says
"terminal too small".

**Roadmap:** the original brief lists eight more themes. They are planned for
later versions.

## Settings

The settings file is found in this order:

1. `--config <path>`
2. the environment variable `TELEMETRIX_CONFIG`
3. `telemetrix.toml` next to the program
4. Windows: `%APPDATA%\telemetrix\telemetrix.toml`;
   Linux: `$XDG_CONFIG_HOME/telemetrix/telemetrix.toml` or
   `~/.config/telemetrix/telemetrix.toml`

Without a file the defaults are used. The first change you make in the
dashboard creates the file, with a comment on every key.
`telemetrix.example.toml` in this repository is the same file.

A bad settings file never stops the program:

- A missing key uses its default.
- An unknown key is ignored, with a warning in the log.
- A wrong value (wrong type or out of range) uses the default for that key
  only, with a warning.
- A syntax error keeps the last good settings and shows a red line at the
  top of the screen with the line number.
- The running dashboard picks up a saved change within 2 seconds.

Each plugin can have its own `[plugin.<name>]` section; see
[PLUGINS.md](PLUGINS.md).

### Settings reference

This table is printed by `telemetrix config reference`. A test fails when it
no longer matches the program.

<!-- reference:start -->
| Key | Default | Allowed values | In the `s` overlay | Meaning |
|---|---|---|---|---|
| `schema` | `1` | 1..1 | no, edit the file | format version of this file, do not change |
| `general.theme` | `"matrix"` | minimalist \| matrix | yes | minimalist \| matrix |
| `general.fps` | `15` | 1..60 | yes | frames per second for animated themes, 1..60 |
| `general.exit_on_any_key` | `false` | true \| false | yes | true = screensaver mode: any key quits |
| `general.plugins_dir` | `"plugins"` | a file or folder path | no, edit the file | relative to the executable, or an absolute path |
| `general.log_file` | `""` | a file or folder path | no, edit the file | "" = off, otherwise a file path |
| `general.log_lines` | `200` | 50..2000 | yes | lines kept for the l overlay, 50..2000 |
| `units.temperature` | `"celsius"` | celsius \| fahrenheit | yes | celsius \| fahrenheit |
| `units.bytes` | `"binary"` | binary \| decimal | yes | binary = GiB, decimal = GB |
| `metrics.cpu_interval_ms` | `1000` | 200..60000 | yes | 200..60000 |
| `metrics.memory_interval_ms` | `1000` | 200..60000 | yes | 200..60000 |
| `metrics.temps_interval_ms` | `5000` | 1000..60000 | yes | 1000..60000 |
| `metrics.disks_interval_ms` | `30000` | 1000..600000 | yes | 1000..600000 |
| `metrics.history_len` | `240` | 10..3600 | yes | samples kept for sparklines, 10..3600 |
| `thresholds.cpu_warn_pct` | `80` | 1..100 | yes | highlight CPU above this, 1..100 |
| `thresholds.temp_warn_c` | `75` | 1..150 | yes | highlight temperature above this |
| `thresholds.disk_warn_pct` | `90` | 1..100 | yes | highlight disks fuller than this |
| `plugins.enabled` | `true` | true \| false | yes | false = run no plugins at all |
| `plugins.default_interval_s` | `60` | 5..86400 | yes | used when a plugin sets no interval, 5..86400 |
| `plugins.http_timeout_s` | `10` | 1..60 | yes | per request, 1..60 |
| `plugins.call_timeout_s` | `5` | 1..60 | yes | Lua time limit per update(), 1..60 |
| `plugins.memory_limit_mb` | `8` | 1..64 | yes | per plugin, 1..64 |
| `plugins.rescan_interval_s` | `60` | 0..86400 | yes | 0 = rescan only on the r key |
| `plugins.max_plugins` | `16` | 1..64 | yes | at most this many plugins run, 1..64 |
| `memory.budget_mb` | `13` | 5..1024 | yes | whole program, above it the status bar turns amber |
| `memory.plugin_budget_mb` | `1` | 1..64 | yes | Lua memory per plugin, above it one log warning |
| `theme.matrix.density` | `0.5` | 0.0..1.0 | yes | 0.0..1.0 |
| `theme.matrix.speed` | `1.0` | 0.1..5.0 | yes | 0.1..5.0 |
| `theme.matrix.color` | `"green"` | green \| amber \| cyan \| white \| #rrggbb | yes | green \| amber \| cyan \| white \| #rrggbb |
| `theme.minimalist.show_sparklines` | `true` | true \| false | yes | history graphs under CPU and RAM |
<!-- reference:end -->

## Memory and CPU

Keeping telemetrix small is a main goal. There are two budgets:

- **Core** (the dashboard without plugins): under **10 MB** working set. This
  limit is fixed.
- **Total** (the default setup, Matrix and the five plugins): under
  `memory.budget_mb`, **13 MB** by default. Each plugin gets an allowance of
  about 1 MB (`memory.plugin_budget_mb` for its Lua memory).

The dashboard reads its own memory every 5 seconds. The status bar shows it
("self 7.4 MB") and turns amber above the budget. The log (`l`) shows the
peak, the private bytes and each plugin's Lua memory. Going over a budget
only writes a warning to the log; nothing is stopped. The separate Lua limit
(`plugins.memory_limit_mb`) still stops a runaway plugin.

### Measured numbers

Windows 11, release build, 32-core PC, legacy console, 20 seconds after the
start. Working set is what Task Manager shows as memory; private bytes is the
memory that belongs to this program alone.

| Setup | Working set | Private bytes |
|---|---|---|
| no plugins, minimalist | 7.3 MB | 1.9 MB |
| no plugins, matrix | 7.3 MB | 2.0 MB |
| 2 offline plugins (clock, uptime) | 7.9 MB | 2.4 MB |
| 1 network plugin (weather) | 10.0 MB | 2.5 MB |
| 5 plugins, minimalist | 11.0 MB | 3.4 MB |
| 5 plugins, matrix (default) | 11.0 MB | 3.3 MB |

For comparison, a Rust program that does nothing but sleep uses 4.9 MB
working set on the same PC. Much of that comes from Windows itself and from
other software that loads into every process (here an antivirus and Microsoft
Defender).

CPU, measured over 60 seconds, as a share of one core:

| Setup | CPU |
|---|---|
| minimalist, no plugins | 0.18 % |
| matrix at 15 fps, 5 plugins | 1.45 % |

The first network request loads the TLS code, about 2 MB. That is why one
network plugin costs more than two offline ones.

What keeps it small on Windows:

- CPU usage comes from `GetSystemTimes`, not from Windows performance
  counters (PDH), which cost about 4 MB.
- Temperature sensors are read through WMI, which costs about 3.7 MB, and
  most Windows PCs report none. A short helper process asks first, and the
  dashboard loads WMI only if it will get a value.
- DLLs that are rarely needed are loaded on first use, not at start.
- All plugins share one HTTP client, which is dropped after a minute without
  requests.

### Before every release: `selftest --memory`

```
telemetrix selftest --memory
```

It starts the real dashboard twice in a hidden console: once with the default
settings and all plugins, then once with `--no-plugins`. After 30 seconds
(`--seconds N` to change) each run reports its peak and final working set.
The command prints both against their budgets and exits with code 1 if
either is over. Network plugins run for real; if the network is down they
fail, which is fine for this test.

```
memory selftest: 30 s per run, one run after the other, hidden consoles
            final      peak   private  budget  result
total     11.1 MB   11.1 MB    3.3 MB   13 MB  ok  (5 plugins)
core       7.4 MB    7.4 MB    2.0 MB   10 MB  ok  (0 plugins)
```

Run it before every release. Agents should run it after any change to the
code or the plugins.

## Guide for agents

- **Change the settings file safely.** Write the new content to a temporary
  file next to it, then rename it over `telemetrix.toml`. The dashboard reads
  the file at any moment and must never see half of it.
- **Check the settings after every change:** `telemetrix config check`.
  Exit code 0 means no problems; otherwise it lists each one with its line.
  `--json` gives the same list for a program.
- **Check a plugin after every change:** `telemetrix plugin check <file>`.
  Exit code 0 means the card works. See [PLUGINS.md](PLUGINS.md).
- **Read the numbers without a terminal:** `telemetrix snapshot --json
  --plugins`.
- **Keep memory in budget:** run `telemetrix selftest --memory` after a
  change. Exit code 1 means a budget is broken.

## Starting it automatically

### Windows Terminal profile

Add a profile to Windows Terminal's `settings.json` (Settings, then "Open
JSON file"), in the `profiles.list` array:

```json
{
    "name": "telemetrix",
    "commandline": "%USERPROFILE%\\.cargo\\bin\\telemetrix.exe",
    "closeOnExit": "always"
}
```

The path assumes you installed it with `cargo install --path .`. Start it
full screen with:

```
wt.exe -F -p telemetrix
```

### A keyboard shortcut on the desktop

1. Right-click the desktop, then New, then Shortcut.
2. As the location, enter `wt.exe -F -p telemetrix`.
3. Open the shortcut's Properties and click into "Shortcut key".
4. Press the keys you want, for example `Ctrl+Alt+T`.

Windows only honours such keys for shortcuts on the desktop or in the Start
menu.

### As a screensaver when the PC is idle (Task Scheduler)

This command (in PowerShell or Command Prompt) starts it after 10 minutes
without input, in screensaver mode, so any key closes it. Words after
`-p telemetrix` replace the profile's command, so the program name comes
first; `cargo install` puts it on your `PATH`.

```
schtasks /Create /TN "telemetrix screensaver" /SC ONIDLE /I 10 /TR "wt.exe -F -p telemetrix telemetrix.exe --exit-on-any-key"
```

Windows decides when the PC counts as idle, so the start can come a few
minutes late. Remove it with `schtasks /Delete /TN "telemetrix screensaver"`.

### Linux: swayidle (Sway, Wayland)

In your Sway config, start it after 5 minutes without input:

```
exec swayidle -w timeout 300 'foot telemetrix --exit-on-any-key'
```

Use your own terminal instead of `foot` if you like.

### Linux: xautolock (X11)

```
xautolock -time 5 -locker "xterm -fullscreen -e telemetrix --exit-on-any-key" &
```

## License

Either [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE), at your option.
