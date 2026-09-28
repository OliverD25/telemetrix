# Changelog

All notable changes to telemetrix. Versions follow
[Semantic Versioning](https://semver.org/).

## 0.3.2 — 2026-09-28

### Added

- **1-year graphs on the Currency card:** a third row under each
  currency's 30-day graph shows the official NBU rate over the last year,
  with its change in green or red, lined up with the 7-day and 30-day
  parts. The daily NBU request now asks for 366 days instead of 31: still
  one request per currency and day, about 75 KB each. The store keeps only
  the 366 numbers (about 9 KB for three currencies). A store from 0.3.1
  holds 31 days, so the first update after the upgrade fetches the year
  at once. The new `show_year` setting (on by default, also in the `s`
  box) turns the year rows off. On a card narrower than 37 characters all
  graph rows go and the rates stay.

## 0.3.1 — 2026-09-28

### Changed

- **Currency card layout:** the buy and sell rates now stand right next
  to the currency code, in bright text. On the right, a dim column shows
  the 7-day graph on the currency's line and the 30-day graph on the line
  under it, so the labels, graphs and changes line up. Each change is
  green when the rate rose and red when it fell. On a card narrower than
  37 characters the 30-day lines go and only the rates stay; nothing
  wraps.

### Added

- **Several colours in one plugin row:** a metric's `label` and `value`
  can be a list of `{ text, style }` spans, and each span has its own
  style. A new style, `bright`, uses the value colour.
- **`min_width` on a metric:** the row is left out of cards narrower than
  this many characters.
- **Updates from the settings box:** the `s` box has an `update` group
  with this program's version, the latest release and when it was checked,
  and two rows that `Enter` runs in the background. `check now` asks
  GitHub and answers `up to date`, `v0.3.2 available` or `this build is
  newer`. `install now` appears when a newer release is known: it
  downloads with a percentage, checks the SHA-256 and installs the way
  `telemetrix update` does (including `screensaver update`), then the
  status bar offers `u` to restart. A failure changes nothing and shows
  its reason. The `auto` and `check_interval_h` rows are in the same group.
  The help (`?`) and the README describe it.

## 0.3.0 — 2026-09-28

### Added

- **Five new themes:**
  - `nasa`: a mission control console. Every value is a row with a code,
    the value, its unit and a NOMINAL, CAUTION or WARNING status, in white
    and amber on black, under a line with the time since start (MET) and
    the UTC time (GMT). Static.
  - `pip-boy`: a green wrist-computer screen in one hue: ASCII frames,
    bracketed tabs and `[▮▮▮▯▯]` gauges. Static.
  - `synthwave`: a neon sunset with a striped sun and a wireframe grid
    floor that moves toward you, and card frames that fade from purple to
    orange. Animated at `general.fps`, 10 frames per second at most on very
    large screens.
  - `kernel-log`: a scrolling boot log with `[  OK  ]`, `[ WARN ]` and
    `[FAILED]` records for every change, timestamps like `dmesg`, and a
    pinned status block with the current values. The log keeps 500
    records. Static: it redraws only on new data.
  - `ascii-dashboard`: large block-character charts of the CPU and RAM
    history, and horizontal block bars for disks, GPUs and swap that fill
    in eighths of a cell. Static.
- **Options per theme:** every theme has its own `[theme.<name>]` table:
  `cpu_view` and `ram_view` (a gauge, the tall history chart of
  ascii-dashboard, or both), `chart_height`, `hide` and `order` of the
  cards, and an `accent` colour. They are changed in the `t` box (`Right`
  or `o` opens the options of the highlighted theme, with a live preview
  and one save for that theme), in the first group of the `s` box, and
  with `v`, which cycles the CPU view. The defaults keep every theme's
  look.
- **Updates from GitHub Releases:** `telemetrix update [--check]
  [--dry-run] [--force]` downloads the program for this platform from the
  latest release of `OliverD25/telemetrix`, checks its SHA-256 against the
  release's `SHA256SUMS`, and replaces the installed program (on Windows
  the old file moves aside to `.old` and is deleted at the next start).
  The screensaver watcher checks once a day (`[update] auto`,
  `check_interval_h`) and installs by itself when no dashboard is open;
  otherwise it keeps a checked download ready. An open dashboard shows
  `update ready · u restart`, and `u` restarts into the new version in the
  same window.
- **Releases** carry the bare programs and a `SHA256SUMS` file next to
  the archives.
- **Hosts plugin** (off by default): the health result, load, memory,
  disk, uptime and UPS of your own servers, read by commands you list in
  the new `[commands]` table of `telemetrix.toml`. Plugins run them with
  `telemetrix.run(name)` or `telemetrix.run_all(names)`: only names from
  the user's file, no extra arguments, no shell, with a time limit and an
  output cap.
- **Search settings for plugins:** a `kind = "search"` setting and a
  `search(query)` function give a plugin a pick-from-a-list row in the `s`
  box. `plugin check <file> --search <text>` runs it without the dashboard.

### Changed

- **GPU card:** `gpu.source = "auto"` (the default) reads the counters
  Task Manager uses on Windows (every GPU, about 1.3 MB of memory) and the
  amdgpu files on Linux. NVIDIA's NVML library is used only with
  `gpu.source = "nvml"`; it also gives the power in watts and adds about
  24 MB. On Windows the card loads `gdi32.dll` at its first reading, so
  with the card off it costs no memory.
- **Screensaver:** the watcher runs from a copy of the program, so it
  never blocks `cargo install`; `telemetrix screensaver update` refreshes
  that copy. `install` and `update` check that the watcher really runs,
  and the watcher writes a small log that `status` shows. When Windows
  redirects AppData for the program that runs `install` (a packaged app),
  the task uses the folder where the copy really is.
- The weather city is picked from a search list in the `s` box, which also
  finds misspelled, old Russian and Cyrillic names; places in the
  `country` setting (default `UA`) come first.
- The Disks and Network cards list drives by letter, as Windows Explorer
  does; on Linux `/` comes first, then the other mount points in order.
- `u` and `v` are reserved keys now; a plugin cannot use them as its run
  key.
- The status bar lists `v view` among its hints.

### Fixed

- The status bar no longer cuts its key hints in the middle of a word when
  the theme name is long. It drops hints from the end, then shows the keys
  alone.

## 0.2.0 (2026-09-27)

### Added

- **New plugins:** currency (Monobank, PrivatBank and the official NBU
  rate, with 7- and 30-day graphs) and an internet speed test (the nearest
  Ookla server, Cloudflare as backup; key `g` runs it now).
- **Plugin engine:** trend graphs in plugin rows, row styles (`dim`,
  `header`, `good`, `bad`), a small stored file per plugin, a run-now key,
  a time limit per call, `emit()` for progress, settings that the `s` box
  shows (with text input), and `telemetrix.units`.
- **Built-in plugins:** the default plugins are built into the program and
  are installed next to the settings file on the first start.
  `telemetrix plugin install` restores or updates them.
- **GPU card** for NVIDIA GPUs: name, load, video memory, temperature and
  power, read through NVML (loaded at run time, no link-time dependency).
  Off by default (`gpu.enabled`), because NVML adds about 24 MB of memory.
  `snapshot --json` lists the GPUs in `system.gpus`.
- **Three new themes:** `tokyo-night`, `crt-amber` (with scanlines) and
  `cyberpunk`. All three are static, so they use almost no CPU.
- **Screensaver command:** `telemetrix screensaver install [--idle-minutes N]`
  registers a small watcher as a logon task. It opens the dashboard full
  screen after N minutes without input and closes on any key. `uninstall`
  and `status` go with it, and `--dry-run` shows what would change. On Linux
  it prints the swayidle or xautolock line instead.
- **Memory soak test:** `selftest --memory --soak <minutes>` checks that
  memory does not grow over a long run.

### Changed

- Crypto prices come from Binance instead of CoinGecko, with 7- and 30-day
  graphs.
- The weather card takes a city name typed in the `s` box and shows today,
  tomorrow and the day after. Temperatures follow `units.temperature`.
- The default total memory budget is 14 MB (was 13 MB); the core budget
  stays 10 MB.
- On Windows the program uses the segment heap, so private memory stays flat
  over hours.
- The crypto plugin stops asking Binance after an HTTP 429 answer until the
  next interval. After an HTTP 418 (the address is banned) it waits as long
  as Binance's `Retry-After` says, or else 10 minutes, doubled for every 418
  in a row up to 24 hours, and shows `paused by Binance until HH:MM`.
- `telemetrix.http_get` also returns the response headers.

## 0.1.0 (2026-09-25, not released)

The first working version.

- Dashboard with CPU, RAM, swap, disks, CPU temperature (where the system
  reports it) and a Network card for mapped drives.
- Two themes: **minimalist** (static) and **matrix** (digital rain). The `t`
  box picks a theme with a live preview.
- Settings in a commented `telemetrix.toml`. A wrong value never stops the
  program; changes in the file apply within 2 seconds. The `s` box changes
  and saves settings.
- Sandboxed Lua 5.4 plugins with five defaults: weather, crypto, internet
  latency, clock and uptime.
- Commands for scripts and agents: `snapshot [--json]`, `config
  init|path|check|show|reference`, `plugin check|list`, `themes`.
- Memory budgets, a memory number in the status bar and
  `selftest --memory`.
- Linux support, including clean exit on SIGTERM, SIGHUP and SIGINT.
