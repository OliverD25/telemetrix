# telemetrix

**TermSaver**: a very light terminal screensaver and live system dashboard for
Windows 11 and Linux, written in Rust.

It shows CPU, memory, swap, disks and CPU temperature (where the computer
reports it), plus cards from small Lua plugins: exchange rates, crypto
prices, weather, an internet speed test, internet latency, a clock, uptime,
or anything you write yourself. It uses about 7.5 MB of memory without
plugins and about 12.5 MB with the seven default plugins, and almost no CPU
on the static theme. Settings live in a commented `telemetrix.toml` that a
wrong value can never break.

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
| `t` | open the theme box: `Up`/`Down` (or `t`/`T`) preview each theme live, `Enter` saves it, `Esc` goes back; `q` does not quit while the box is open |
| `s` | settings: every change is saved at once |
| `+` / `-` | more / fewer frames per second (saved) |
| `space` | pause the animation |
| `l` | log, with the program's own memory use at the top |
| `r` | look for new, changed and removed plugins now |
| `g` | run the speed test now (a plugin's own key) |
| `?` | help |

Inside the settings box: `Up`/`Down` move, `Enter` or `Right` pick the next
value, `Left` the previous one, `Shift` with `Left`/`Right` moves ten steps.
Paths are shown but can only be changed in the file.

Each plugin's section in the settings box also lists the plugin's own
settings, like the weather city or the main bank. On a text setting,
`Enter` opens an input line: type the text, `Enter` saves it and the plugin
runs at once, `Esc` cancels. While you type, every key goes to the input;
only `Ctrl+C` quits.

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
| `telemetrix plugin check <file> [--json] [--run]` | run a plugin once and print its card, its settings and any error; exit 1 on error. `--run` acts like the plugin's key, so the speed test really runs |
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

### Disks and network drives

The Disks card shows each local drive with its label, as Explorer does:
"System Disk (C:)". Long names are cut with "…" so the numbers stay
visible.

Mapped network drives get their own Network card under Disks. On Windows
they are asked on a separate thread every `disks.network_interval_s`
seconds; a drive that does not answer within `disks.network_timeout_s` is
shown as `offline` in red, and a stuck drive never slows the rest of the
dashboard. Drives on the same server with the same size and free space
(several shares of one NAS volume) share one row, for example
`nas  M: P: R: W: X:`; set `disks.group_network = false` to list them
one by one. Explorer "network locations" without a drive letter are not
shown. `disks.hide` hides letters or mount points in both cards.

On Linux, mounts of type nfs, nfs4, cifs, smb3, smbfs, fuse.sshfs and 9p go
to the Network card. A hung network mount can still delay the disk
readings there; protecting against that is planned after v0.1.

**Roadmap:** the original brief lists eight more themes. They are planned for
later versions.

## Plugins and where their data comes from

telemetrix comes with seven plugins. None needs an account or a key. The
network plugins stay far below the free limits of their services. When a
service does not answer, a card keeps its last values and marks them
stale. Rates, prices, the weather place and speed results are kept in a
small file per plugin (`%LOCALAPPDATA%\telemetrix\plugins` on Windows,
`~/.local/share/telemetrix/plugins` on Linux, or `--data-dir <dir>`), so
cards are not empty after a restart.

| Plugin | Shows | Data source | Free limit | How often telemetrix asks |
|---|---|---|---|---|
| Currency | USD, EUR, GBP to hryvnia from two banks, 7- and 30-day graphs | Monobank `api.monobank.ua/bank/currency`; PrivatBank card rate `api.privatbank.ua/p24api/pubinfo`; NBU official rate `bank.gov.ua/NBU_Exchange` | Monobank: 1 request per 5 minutes. PrivatBank and NBU publish no limit. | Monobank and PrivatBank every 5 minutes (Monobank never sooner, even after a restart). NBU history once a day, one request per currency. |
| Crypto | BTC, ETH, SOL in USDT, 7- and 30-day graphs | Binance `api.binance.com/api/v3/ticker/price` and `/klines` | Binance counts a request weight of 6000 per minute per IP address. | Prices every minute (one request). Daily history once an hour, one request per coin. |
| Weather | now, tomorrow and the day after | Open-Meteo `api.open-meteo.com` and its geocoding service | 10 000 requests per day for non-commercial use. | Every 10 minutes. The city is looked up once per change. |
| Speed test | download, upload, ping | Cloudflare `speed.cloudflare.com` | No published limit. | Every 6 hours, and when you press `g`. Never at start. |
| Internet latency | time to connect to 1.1.1.1:443 | a TCP connection, no service | none | every 30 seconds |
| Clock, Uptime | time and date; computer name and uptime | this computer | none | every second; every 30 seconds |

**The speed test uses data.** One run downloads 25 MB and uploads 10 MB with
the default settings, about 35 MB. At the 6-hour interval that is about
140 MB a day, or about 4.2 GB a month. On a metered connection, raise the
interval, lower `download_mb` and `upload_mb` in the `s` box, or set
`enabled = false` under `[plugin.speedtest]`.

Settings you can change in the `s` box:

- **Currency:** the primary bank (`mono` or `privat`), the 30-day graph on
  or off, and `compact`. The full row
  `USD  mono 44.80/45.20  privat 44.60/45.05` needs a card about 42
  characters wide; with `compact` on, the card shows only the primary bank.
  The list of currencies (`currencies = ["USD", "EUR", "GBP"]`) is set in
  the file.
- **Crypto:** the quote currency (default `USDT`). The coins
  (`coins = ["BTC", "ETH", "SOL"]`) are set in the file.
- **Weather:** the city. A city wins over `lat` and `lon`. The coordinates
  (with `label` as the card title) are used only when no city is set in the
  file. Settings files from v0.1 have `lat`, `lon` and `label` but no city,
  so they keep showing the same place until you type a city. Without a city
  and without coordinates the card shows Kyiv. The log says which one is
  used, and `config check` warns when a file has both. Temperatures are
  always °C.
- **Speed test:** `download_mb` (5..100), `upload_mb` (1..50) and
  `max_seconds` per direction (3..15).

To write your own plugin, see [PLUGINS.md](PLUGINS.md).

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
| `disks.show_network` | `true` | true \| false | yes | show mapped network drives in their own card |
| `disks.network_interval_s` | `60` | 10..3600 | yes | how often network drives are asked, 10..3600 |
| `disks.network_timeout_s` | `5` | 1..30 | yes | a drive that takes longer is shown offline, 1..30 |
| `disks.group_network` | `true` | true \| false | yes | one row for drives on the same server and volume |
| `disks.hide` | `[]` | list of texts | no, edit the file | letters or mount points to hide, like ["X:", "/boot"] |
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
- **Total** (the default setup, Matrix and the seven plugins): under
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
| 5 plugins, matrix (v0.1 default) | 11.0 MB | 3.3 MB |
| 7 plugins, matrix (v0.2 default, `selftest --memory`, 30 s) | 12.5 MB | 4.5 MB |

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
total     12.5 MB   12.6 MB    4.5 MB   13 MB  ok  (7 plugins)
core       7.6 MB    7.7 MB    2.0 MB   10 MB  ok  (0 plugins)
```

Run it before every release. Agents should run it after any change to the
code or the plugins.

## Linux

telemetrix builds and runs the same way on Linux. You need Rust 1.95 or newer
and a C compiler (gcc or clang) for the Lua interpreter.

```
cargo install --path .
```

This puts `telemetrix` in `~/.cargo/bin`. Copy the `plugins` folder next to
it, or set `general.plugins_dir` to an absolute path.

- **Settings file:** `~/.config/telemetrix/telemetrix.toml`, or
  `$XDG_CONFIG_HOME/telemetrix/telemetrix.toml` when that variable is set.
- **Start it automatically:** see the swayidle and xautolock recipes under
  "Starting it automatically".
- **Disks:** system and package mounts are not shown: snap images (`/snap`),
  and on WSL its own mounts (`/usr/lib/wsl`, `/usr/lib/modules`,
  `/mnt/wslg`, `/mnt/wsl`, `/init`). On WSL, the Windows drives at
  `/mnt/c`, `/mnt/d` and so on are local disks of the host and appear as
  "C: (/mnt/c)".
- **Network drives:** nfs, nfs4, cifs, smb3, smbfs, fuse.sshfs and 9p mounts
  go to the Network card. A hung network mount can still delay the disk
  readings in v0.1.
- **CPU temperature:** read from hwmon. telemetrix prefers the Intel
  package sensor (`coretemp`, "Package id 0"), then the AMD control
  temperature (`k10temp`, "Tctl"), then any other CPU sensor. WSL and most
  virtual machines have no sensors; the line is then hidden.
- **Containers:** sysinfo takes cgroup limits into account, so inside a
  container the total RAM can be the container's limit, not the machine's.
- **Signals:** SIGTERM, SIGHUP (the terminal was closed) and SIGINT end the
  dashboard cleanly and restore the terminal. If the terminal is already
  gone, the program still exits within about a second.
- **`selftest --memory`** needs the `script` command from util-linux (part
  of every common distribution); it gives each run a pseudo-terminal.

Measured in WSL 2 (Ubuntu 24.04, release build, 30 seconds after the start,
resident memory; Linux has no cheap "private bytes" figure):

| Setup | Resident memory |
|---|---|
| no plugins | 4.0 MB |
| 5 plugins, matrix (v0.1 default) | 5.6 MB |

These are WSL numbers: Linux shares library pages differently from Windows,
so they are not comparable with the Windows table above.

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
