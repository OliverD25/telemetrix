# Changelog

All notable changes to telemetrix. Versions follow
[Semantic Versioning](https://semver.org/).

## Unreleased

### Added

- **nasa theme:** a mission control console. Every value is a row with a
  code, the value, its unit and a NOMINAL, CAUTION or WARNING status, in
  white and amber on black, under a line with the time since start (MET)
  and the UTC time (GMT). Static.
- **pip-boy theme:** a green wrist-computer screen in one hue: ASCII
  frames, bracketed tabs and `[▮▮▮▯▯]` gauges. Static.

## 0.2.0 (not released yet)

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
- **GPU card:** name, load, video memory and temperature, one card per
  GPU. `gpu.source = "auto"` reads the counters Task Manager uses on
  Windows (every GPU, about 1.3 MB of memory) and the amdgpu files on
  Linux; `gpu.source = "nvml"` reads NVIDIA's NVML library (loaded at run
  time, also gives the power in watts, adds about 24 MB). Off by default
  (`gpu.enabled`). `snapshot --json` lists the GPUs in `system.gpus`.
- **Three new themes:** `tokyo-night`, `crt-amber` (with scanlines) and
  `cyberpunk`. All three are static, so they use almost no CPU.
- **Screensaver command:** `telemetrix screensaver install [--idle-minutes N]`
  registers a small watcher as a logon task. It opens the dashboard full
  screen after N minutes without input and closes on any key. `uninstall`
  and `status` go with it, and `--dry-run` shows what would change. On Linux
  it prints the swayidle or xautolock line instead. The watcher runs from a
  copy of the program, so it never blocks `cargo install`;
  `telemetrix screensaver update` refreshes that copy after an update.
  `install` and `update` check that the watcher really runs, and the
  watcher writes a small log that `status` shows.
- **Hosts plugin** (off by default): the health result, load, memory,
  disk, uptime and UPS of your own servers, read by commands you list in
  the new `[commands]` table of `telemetrix.toml`. Plugins run them with
  `telemetrix.run(name)` or `telemetrix.run_all(names)`: only names from
  the user's file, no extra arguments, no shell, with a time limit and an
  output cap.
- **Search settings for plugins:** a `kind = "search"` setting and a
  `search(query)` function give a plugin a pick-from-a-list row in the `s`
  box. `plugin check <file> --search <text>` runs it without the dashboard.
- **Memory soak test:** `selftest --memory --soak <minutes>` checks that
  memory does not grow over a long run.

### Changed

- Crypto prices come from Binance instead of CoinGecko, with 7- and 30-day
  graphs.
- The weather card shows today, tomorrow and the day after. Temperatures
  follow `units.temperature`. Its city is picked from a search list in the
  `s` box, which also finds misspelled, old Russian and Cyrillic names;
  places in the `country` setting (default `UA`) come first.
- The Disks and Network cards list drives by letter, as Windows Explorer
  does; on Linux `/` comes first, then the other mount points in order.
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
