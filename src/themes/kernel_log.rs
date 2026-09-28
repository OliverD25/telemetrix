use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout};
use ratatui::style::{Color, Modifier};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph};

use super::Theme;
use super::common::{self, Palette, fg, fit};
use crate::app::AppState;
use crate::config::{Config, TempUnit, ThemeOptions};
use crate::format;
use crate::metrics::{self, network};
use crate::plugins::{MetricStyle, PluginStatus};

pub const SCREEN: Color = Color::Rgb(0, 0, 0);
const TEXT: Color = Color::Rgb(190, 190, 190);
const DIM: Color = Color::Rgb(110, 110, 110);
const BRIGHT: Color = Color::Rgb(245, 245, 245);
const RULE: Color = Color::Rgb(75, 75, 75);
pub const OK: Color = Color::Rgb(80, 230, 90);
pub const WARN: Color = Color::Rgb(250, 220, 70);
pub const FAIL: Color = Color::Rgb(255, 80, 80);
/// The log keeps this many records; older ones are dropped.
pub const LOG_LINES: usize = 500;
/// Memory, swap and video memory have no threshold setting.
const FULL_WARN_PCT: f32 = 90.0;
/// From this width the status block stands to the right of the log.
const SIDE_BY_SIDE_W: u16 = 96;
/// Narrower logs leave the timestamps out, so the messages stay readable.
const TIMESTAMP_MIN_W: usize = 56;
/// The name column of the status block: a space, 7 characters, a space.
const NAME_W: usize = 9;

fn palette() -> Palette {
    Palette {
        bg: Some(SCREEN),
        border: RULE,
        title: BRIGHT,
        label: DIM,
        value: TEXT,
        bar: OK,
        bar_empty: RULE,
        spark: Color::Rgb(90, 170, 255),
        border_set: border::PLAIN,
        brackets: (" ", " "),
        warn: FAIL,
        rise: OK,
        muted: DIM,
        gauge: common::BLOCK_GAUGE,
        borders: Borders::TOP,
        levels: [OK, WARN, FAIL],
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    fn tag(self) -> &'static str {
        match self {
            Self::Ok => "  OK  ",
            Self::Warn => " WARN ",
            Self::Fail => "FAILED",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Ok => OK,
            Self::Warn => WARN,
            Self::Fail => FAIL,
        }
    }
}

struct Record {
    /// Seconds since the computer started, as `dmesg` counts.
    ts: f64,
    level: Level,
    text: String,
}

/// `52.0C`: the log has no room for the degree sign and its space.
fn short_temp(c: f32, unit: TempUnit) -> String {
    format::temperature(c, unit).replace(" °", "")
}

fn warn_if(bad: bool) -> Level {
    if bad { Level::Warn } else { Level::Ok }
}

/// One entry per source: a key, a level and the message. A source gets a
/// new log record when its message changes.
fn entries(state: &AppState) -> Vec<(String, Level, String)> {
    let cfg = &state.config;
    let th = &cfg.thresholds;
    let unit = cfg.units.temperature;
    let mut out = Vec::new();
    if let Some(s) = &state.snapshot {
        let mut cpu = format!("cpu0: load {:.0}%", s.cpu_usage);
        let mut hot = s.cpu_usage > th.cpu_warn_pct;
        if let Some(t) = s.cpu_temp {
            cpu += &format!(" temp {}", short_temp(t, unit));
            hot |= t > th.temp_warn_c;
        }
        out.push(("cpu".into(), warn_if(hot), cpu));
        let ram = s.ram_pct();
        let used = common::used_of(s.ram_used_bytes, s.ram_total_bytes, state);
        let text = format!("mem: {used} used ({ram:.0}%)");
        out.push(("mem".into(), warn_if(ram > FULL_WARN_PCT), text));
        if s.swap_total_bytes > 0 {
            let pct = metrics::pct(s.swap_used_bytes, s.swap_total_bytes);
            let used = common::used_of(s.swap_used_bytes, s.swap_total_bytes, state);
            let text = format!("swap: {used} used ({pct:.0}%)");
            out.push(("swap".into(), warn_if(pct > FULL_WARN_PCT), text));
        }
        if cfg.gpu.enabled {
            for (i, g) in s.gpus.iter().enumerate() {
                let mut text = format!("gpu{i}:");
                let mut hot = false;
                if let Some(load) = g.usage_pct {
                    text += &format!(" load {load:.0}%");
                }
                if let (Some(used), Some(total)) = (g.mem_used_bytes, g.mem_total_bytes) {
                    text += &format!(" vram {}", common::used_of(used, total, state));
                    hot |= metrics::pct(used, total) > FULL_WARN_PCT;
                }
                if let Some(t) = g.temp_c {
                    text += &format!(" temp {}", short_temp(t, unit));
                    hot |= t > th.temp_warn_c;
                }
                if let Some(w) = g.power_w {
                    text += &format!(" power {w:.1} W");
                }
                out.push((format!("gpu{i}"), warn_if(hot), text));
            }
        }
        for d in &s.disks {
            if network::is_hidden(&d.mount, &cfg.disks.hide) {
                continue;
            }
            let mount = network::normalize_mount(&d.mount);
            let pct = metrics::pct(d.used_bytes, d.total_bytes);
            let used = common::used_of(d.used_bytes, d.total_bytes, state);
            let text = format!("disk {mount} {pct:.0}% used ({used})");
            let level = warn_if(pct > th.disk_warn_pct);
            out.push((format!("disk {mount}"), level, text));
        }
    }
    if let Some(drives) = state.network.as_ref().filter(|_| cfg.disks.show_network) {
        for r in network::rows(drives, cfg.disks.group_network, &cfg.disks.hide) {
            let (level, text) = if r.online {
                let pct = metrics::pct(r.used_bytes, r.total_bytes);
                let warn = warn_if(pct > th.disk_warn_pct);
                (warn, format!("net {}: {pct:.0}% used", r.title))
            } else {
                (Level::Fail, format!("net {}: offline", r.title))
            };
            out.push((format!("net {}", r.title), level, text));
        }
    }
    for (id, card) in &state.plugins {
        let (level, text) = match &card.status {
            PluginStatus::Error(msg) => (Level::Fail, format!("plugin {id}: {msg}")),
            PluginStatus::Ok => {
                let metrics = &card.data.metrics;
                let bad = metrics
                    .iter()
                    .any(|m| m.bad || m.style == Some(MetricStyle::Bad));
                let parts: Vec<String> = metrics
                    .iter()
                    .map(|m| format!("{} {}", m.label, m.value).trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect();
                (warn_if(bad), format!("plugin {id}: {}", parts.join("; ")))
            }
        };
        out.push((format!("plugin {id}"), level, text));
    }
    out
}

/// A `name   value` row of the pinned status block.
fn status_row(name: &str, value: String, warn: bool, w: usize) -> Line<'static> {
    let color = if warn { WARN } else { TEXT };
    Line::from(vec![
        Span::styled(format!(" {:<7} ", fit(name, 7)), fg(DIM)),
        Span::styled(fit(&value, w.saturating_sub(NAME_W)), fg(color)),
    ])
}

/// Lines grouped by the card they stand for, so `hide` and `order` apply.
#[derive(Default)]
struct Sections(Vec<(String, Vec<Line<'static>>)>);

impl Sections {
    fn add(&mut self, id: &str, line: Line<'static>) {
        match self.0.last_mut() {
            Some((last, lines)) if last == id => lines.push(line),
            _ => self.0.push((id.to_string(), vec![line])),
        }
    }

    /// The lines of the shown cards, the ones in `order` first.
    fn lines(mut self, opts: &ThemeOptions) -> Vec<Line<'static>> {
        self.0.retain(|(id, _)| !opts.hidden(id));
        self.0.sort_by_key(|(id, _)| {
            opts.order
                .iter()
                .position(|o| o == id)
                .unwrap_or(usize::MAX)
        });
        self.0.into_iter().flat_map(|(_, lines)| lines).collect()
    }
}

/// The card a log source belongs to: `mem` is `ram`, `disk C:` is `disks`.
fn card_of(key: &str) -> &str {
    match key.split(' ').next().unwrap_or(key) {
        "mem" => "ram",
        "disk" => "disks",
        "net" => "network",
        "plugin" => key.split_once(' ').map_or(key, |(_, id)| id),
        k if k.starts_with("gpu") => "gpu",
        k => k,
    }
}

/// The current value of everything, like `systemctl status` for the machine.
fn status_lines(state: &AppState, pal: &Palette, w: usize, accent: Color) -> Vec<Line<'static>> {
    let cfg = &state.config;
    let th = &cfg.thresholds;
    let temp = |c: f32| format::temperature(c, cfg.units.temperature);
    let mut out = Sections::default();
    if let Some(s) = &state.snapshot {
        let mut cpu = format!("load {:.0}%", s.cpu_usage);
        let mut hot = s.cpu_usage > th.cpu_warn_pct;
        if let Some(t) = s.cpu_temp {
            cpu += &format!("  temp {}", temp(t));
            hot |= t > th.temp_warn_c;
        }
        out.add("cpu", status_row("cpu", cpu, hot, w));
        for (name, used, total) in [
            ("ram", s.ram_used_bytes, s.ram_total_bytes),
            ("swap", s.swap_used_bytes, s.swap_total_bytes),
        ] {
            let pct = metrics::pct(used, total);
            let text = format!("{}  {pct:.0}%", common::used_of(used, total, state));
            out.add(name, status_row(name, text, pct > FULL_WARN_PCT, w));
        }
        for g in s.gpus.iter().filter(|_| cfg.gpu.enabled) {
            out.add("gpu", status_row("gpu", g.name.clone(), false, w));
            let mut now = Vec::new();
            if let Some(pct) = g.usage_pct {
                now.push(format!("load {pct:.0}%"));
            }
            if let Some(t) = g.temp_c {
                now.push(format!("temp {}", temp(t)));
            }
            if let Some(watts) = g.power_w {
                now.push(format!("{watts:.1} W"));
            }
            if !now.is_empty() {
                let hot = g.temp_c.is_some_and(|t| t > th.temp_warn_c);
                out.add("gpu", status_row("", now.join("  "), hot, w));
            }
            if let (Some(used), Some(total)) = (g.mem_used_bytes, g.mem_total_bytes) {
                let text = format!("vram {}", common::used_of(used, total, state));
                out.add("gpu", status_row("", text, false, w));
            }
        }
        for d in &s.disks {
            if network::is_hidden(&d.mount, &cfg.disks.hide) {
                continue;
            }
            let pct = metrics::pct(d.used_bytes, d.total_bytes);
            let used = common::used_of(d.used_bytes, d.total_bytes, state);
            let label = d.label.as_deref().unwrap_or("");
            let text = format!("{pct:.0}%  {used}  {label}");
            let name = network::normalize_mount(&d.mount);
            let warn = pct > th.disk_warn_pct;
            out.add(
                "disks",
                status_row(&name, text.trim_end().to_string(), warn, w),
            );
        }
    } else {
        out.add(
            "cpu",
            status_row("cpu", "waiting for metrics".into(), false, w),
        );
    }
    if cfg.disks.show_network {
        match &state.network {
            None => out.add("network", status_row("net", "checking…".into(), false, w)),
            Some(drives) => {
                for r in network::rows(drives, cfg.disks.group_network, &cfg.disks.hide) {
                    let (text, warn) = if r.online {
                        let pct = metrics::pct(r.used_bytes, r.total_bytes);
                        let used = common::used_of(r.used_bytes, r.total_bytes, state);
                        let text = format!("{pct:.0}%  {used}  {}", r.title);
                        (text, pct > th.disk_warn_pct)
                    } else {
                        (format!("offline  {}", r.title), true)
                    };
                    out.add("network", status_row("net", text, warn, w));
                }
            }
        }
    }
    for card in common::plugin_cards(state, pal, w.saturating_sub(2)) {
        let color = card.title_color.unwrap_or(accent);
        out.add(
            &card.id,
            Line::styled(
                fit(&format!(" ▸ {}", card.title), w),
                fg(color).add_modifier(Modifier::BOLD),
            ),
        );
        for line in card.lines {
            let mut spans = vec![Span::raw("  ")];
            spans.extend(line.spans);
            out.add(&card.id, Line::from(spans));
        }
    }
    out.lines(common::options(state))
}

/// A `dmesg -w` view with a pinned status block: every change of a value
/// adds a log record, and the status block always shows the current values.
/// Static: it redraws only when data changes.
pub struct KernelLog {
    log: VecDeque<Record>,
    /// The last message of each source.
    last: BTreeMap<String, String>,
    /// When the first record was made, and the uptime at that moment.
    origin: Option<(Instant, f64)>,
}

impl KernelLog {
    pub fn new() -> Self {
        Self {
            log: VecDeque::new(),
            last: BTreeMap::new(),
            origin: None,
        }
    }

    fn push(&mut self, ts: f64, level: Level, text: String) {
        if self.log.len() == LOG_LINES {
            self.log.pop_front();
        }
        self.log.push_back(Record { ts, level, text });
    }

    /// Adds a record for every source whose message changed since the last call.
    pub fn ingest_at(&mut self, state: &AppState, ts: f64) {
        if self.log.is_empty() {
            self.push(ts, Level::Ok, "Started telemetrix live status log.".into());
        }
        let opts = common::options(state);
        for (key, level, text) in entries(state) {
            if opts.hidden(card_of(&key)) {
                continue;
            }
            if self.last.get(&key) != Some(&text) {
                self.last.insert(key, text.clone());
                self.push(ts, level, text);
            }
        }
    }

    fn ingest(&mut self, state: &AppState) {
        let (start, uptime) = *self
            .origin
            .get_or_insert_with(|| (Instant::now(), sysinfo::System::uptime() as f64));
        self.ingest_at(state, uptime + start.elapsed().as_secs_f64());
    }

    /// The newest records that fit in `rows`, oldest first.
    fn log_lines(&self, w: usize, rows: usize) -> Vec<Line<'static>> {
        let stamps = w >= TIMESTAMP_MIN_W;
        let skip = self.log.len().saturating_sub(rows);
        self.log
            .iter()
            .skip(skip)
            .map(|r| {
                let mut spans = Vec::new();
                if stamps {
                    spans.push(Span::styled(format!("[{:>12.6}] ", r.ts), fg(DIM)));
                }
                spans.push(Span::styled("[", fg(BRIGHT)));
                spans.push(Span::styled(
                    r.level.tag(),
                    fg(r.level.color()).add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled("] ", fg(BRIGHT)));
                let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
                spans.push(Span::styled(fit(&r.text, w.saturating_sub(used)), fg(TEXT)));
                Line::from(spans)
            })
            .collect()
    }
}

fn titled(title: &'static str, color: Color) -> Block<'static> {
    Block::new()
        .borders(Borders::TOP)
        .border_style(fg(RULE))
        .title(Span::styled(
            format!(" {title} "),
            fg(color).add_modifier(Modifier::BOLD),
        ))
}

impl Theme for KernelLog {
    fn name(&self) -> &'static str {
        "kernel-log"
    }

    fn frame_interval(&self, _cfg: &Config) -> Option<Duration> {
        None
    }

    fn draw(&mut self, frame: &mut Frame, state: &AppState) {
        self.ingest(state);
        common::fill_screen(frame, SCREEN);
        let pal = palette();
        let accent = common::accent(state).map_or(BRIGHT, |(r, g, b)| Color::Rgb(r, g, b));
        let body = common::body_area(frame.area(), state);
        let (log_area, status_area) = if body.width >= SIDE_BY_SIDE_W {
            let side = (body.width * 2 / 5).clamp(40, 60);
            let [log, _, status] = Layout::horizontal([
                Constraint::Fill(1),
                Constraint::Length(1),
                Constraint::Length(side),
            ])
            .areas(body);
            (log, status)
        } else {
            let want = status_lines(state, &pal, usize::from(body.width), accent).len() + 1;
            let cap = usize::from(body.height) * 3 / 5;
            let h = u16::try_from(want.min(cap)).unwrap_or(u16::MAX);
            let [status, log] =
                Layout::vertical([Constraint::Length(h), Constraint::Fill(1)]).areas(body);
            (log, status)
        };
        let status = titled("status", accent);
        let inner = status.inner(status_area);
        let lines = status_lines(state, &pal, usize::from(inner.width), accent);
        frame.render_widget(Paragraph::new(lines).block(status), status_area);
        let log = titled("dmesg --follow", accent);
        let inner = log.inner(log_area);
        let lines = self.log_lines(usize::from(inner.width), usize::from(inner.height));
        frame.render_widget(Paragraph::new(lines).block(log), log_area);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::AppEvent;
    use crate::ui::{self, TOO_SMALL, demo};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn text(buf: &Buffer) -> String {
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    fn render(theme: &mut KernelLog, state: &AppState, w: u16, h: u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| ui::draw(f, state, theme)).unwrap();
        terminal.backend().buffer().clone()
    }

    #[test]
    fn log_records_and_the_pinned_status() {
        let state = demo::state("kernel-log");
        let mut theme = KernelLog::new();
        theme.ingest_at(&state, 1_234.567_89);
        let t = text(&render(&mut theme, &state, 100, 30));
        for needle in [
            "[ 1234.567890] [  OK  ] Started telemetrix",
            "[  OK  ] cpu0: load 23% temp 52.0C",
            "[  OK  ] disk C: 44% used",
            "[  OK  ] gpu0: load 37%",
            "[  OK  ] plugin crypto: BTC 84,853 USDT; 7",
            " dmesg --follow ",
            " status ",
            " cpu     load 23%  temp 52.0 °C",
            " C:      44%  412.0 GiB / 931.0 GiB",
            " ▸ Crypto",
            "84,853 USDT",
            "+2.1%",
        ] {
            assert!(t.contains(needle), "{needle}");
        }
    }

    #[test]
    fn only_changes_add_records_and_the_log_is_bounded() {
        let mut state = demo::state("kernel-log");
        let mut theme = KernelLog::new();
        theme.ingest_at(&state, 1.0);
        let first = theme.log.len();
        theme.ingest_at(&state, 2.0);
        assert_eq!(theme.log.len(), first, "same values, no new records");
        let mut snap = demo::snapshot();
        snap.cpu_usage = 100.0;
        state.apply(AppEvent::Metrics(snap));
        theme.ingest_at(&state, 3.0);
        assert_eq!(theme.log.len(), first + 1, "only the CPU changed");
        let last = theme.log.back().unwrap();
        assert_eq!(last.level, Level::Warn);
        assert_eq!(last.text, "cpu0: load 100% temp 52.0C");
        for i in 0..600 {
            let mut snap = demo::snapshot();
            snap.cpu_temp = Some(40.0 + i as f32);
            state.apply(AppEvent::Metrics(snap));
            theme.ingest_at(&state, 4.0 + f64::from(i));
        }
        assert_eq!(theme.log.len(), LOG_LINES);
        let t = text(&render(&mut theme, &state, 100, 30));
        assert!(t.contains("[  603.000000]"), "the newest record is shown");
        assert!(
            !t.contains("[    1.000000]"),
            "the oldest ones scrolled away"
        );
    }

    #[test]
    fn warnings_and_failures_are_coloured() {
        let mut state = demo::state("kernel-log");
        let mut snap = demo::snapshot();
        snap.cpu_usage = 100.0;
        state.snapshot = Some(snap);
        if let Some(card) = state.plugins.get_mut("weather") {
            card.status = PluginStatus::Error("timed out".into());
        }
        let buf = render(&mut KernelLog::new(), &state, 100, 30);
        let t = text(&buf);
        assert!(t.contains("[ WARN ] cpu0: load 100%"), "{t}");
        assert!(t.contains("[FAILED] plugin weather: timed out"));
        let color_of = |needle: &str| {
            let cells = buf.content();
            let symbols: Vec<&str> = cells.iter().map(|c| c.symbol()).collect();
            let n = needle.chars().count();
            let pos = symbols
                .windows(n)
                .position(|w| w.concat() == needle)
                .unwrap();
            cells[pos].fg
        };
        assert_eq!(color_of(" WARN "), WARN);
        assert_eq!(color_of("FAILED"), FAIL);
        assert_eq!(color_of("load 100%  temp"), WARN, "the status row too");
    }

    #[test]
    fn narrow_screens_stack_status_over_log() {
        let state = demo::state("kernel-log");
        let t = text(&demo::render(&state, 40, 10));
        assert!(t.contains(" cpu ") && t.contains(" telemetrix "), "{t}");
        assert!(!t.contains(TOO_SMALL));
        let t = text(&demo::render(&state, 30, 8));
        assert!(t.contains("terminal too small"), "{t}");
        let t = text(&demo::render(&state, 50, 40));
        assert!(t.contains("[  OK  ] cpu0: load 23%"), "{t}");
        assert!(!t.contains("] [  OK  ]"), "no timestamps at this width");
        assert_eq!(KernelLog::new().frame_interval(&Config::default()), None);
    }
}
