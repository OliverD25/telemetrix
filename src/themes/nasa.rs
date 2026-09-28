use std::time::{Duration, SystemTime};

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Borders, Paragraph};

use super::Theme;
use super::common::{self, Card, Group, Palette, fg, fit};
use crate::app::AppState;
use crate::config::Config;
use crate::format;
use crate::metrics::{self, network};

pub const SCREEN: Color = Color::Rgb(0, 0, 0);
pub const AMBER: Color = Color::Rgb(255, 176, 0);
const WHITE: Color = Color::Rgb(235, 235, 235);
const GREY: Color = Color::Rgb(125, 125, 125);
pub const RED: Color = Color::Rgb(255, 64, 48);
/// A value is a caution above this share of its warning threshold.
const CAUTION_AT: f32 = 0.85;
/// Memory, swap and video memory have no threshold setting.
const FULL_WARN_PCT: f32 = 90.0;
const CODE_W: usize = 7;
const UNIT_W: usize = 4;
const STATUS_W: usize = 7;

pub fn palette() -> Palette {
    Palette {
        bg: Some(SCREEN),
        border: Color::Rgb(90, 90, 90),
        title: AMBER,
        label: AMBER,
        value: WHITE,
        bar: AMBER,
        bar_empty: Color::Rgb(60, 60, 60),
        spark: Color::Rgb(200, 140, 0),
        border_set: border::PLAIN,
        brackets: (" ", " "),
        warn: RED,
        rise: Color::Rgb(255, 230, 160),
        muted: GREY,
        gauge: common::BLOCK_GAUGE,
        borders: Borders::TOP,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Nominal,
    Caution,
    Warning,
}

impl Status {
    pub fn of(value: f32, warn: f32) -> Self {
        if value > warn {
            Self::Warning
        } else if value > warn * CAUTION_AT {
            Self::Caution
        } else {
            Self::Nominal
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Nominal => "NOMINAL",
            Self::Caution => "CAUTION",
            Self::Warning => "WARNING",
        }
    }

    fn color(self) -> Color {
        match self {
            Self::Nominal => WHITE,
            Self::Caution => AMBER,
            Self::Warning => RED,
        }
    }
}

fn value_width(w: usize) -> usize {
    w.saturating_sub(CODE_W + UNIT_W + STATUS_W + 3)
}

/// `CPU-LD     23.0 %    NOMINAL`: code, value, unit and status in fixed columns.
fn row(code: &str, value: &str, unit: &str, status: Option<Status>, w: usize) -> Line<'static> {
    let vw = value_width(w);
    let value_color = match status {
        Some(Status::Warning) => RED,
        _ => WHITE,
    };
    let mut spans = vec![
        Span::styled(format!("{:<CODE_W$} ", fit(code, CODE_W)), fg(AMBER)),
        Span::styled(format!("{:>vw$}", fit(value, vw)), fg(value_color)),
        Span::styled(format!(" {:<UNIT_W$} ", fit(unit, UNIT_W)), fg(GREY)),
    ];
    if let Some(s) = status {
        let mut style = fg(s.color());
        if s != Status::Nominal {
            style = style.add_modifier(Modifier::BOLD);
        }
        spans.push(Span::styled(s.word(), style));
    }
    Line::from(spans)
}

fn heading(w: usize) -> Line<'static> {
    let vw = value_width(w);
    let text = format!(
        "{:<CODE_W$} {:>vw$} {:<UNIT_W$} {:<STATUS_W$}",
        "CODE", "VALUE", "UNIT", "STATUS"
    );
    Line::styled(fit(&text, w), fg(GREY).add_modifier(Modifier::UNDERLINED))
}

/// `18.3 GiB` → (`18.3`, `GiB`), so the unit gets its own column.
fn split_unit(text: &str) -> (String, String) {
    match text.rsplit_once(' ') {
        Some((v, u)) => (v.to_string(), u.to_string()),
        None => (text.to_string(), String::new()),
    }
}

fn amount_row(code: &str, bytes: u64, state: &AppState, w: usize) -> Line<'static> {
    let (v, u) = split_unit(&format::bytes(bytes, state.config.units.bytes));
    row(code, &v, &u, None, w)
}

fn pct_row(code: &str, pct: f32, warn: f32, w: usize) -> Line<'static> {
    row(
        code,
        &format!("{pct:.1}"),
        "%",
        Some(Status::of(pct, warn)),
        w,
    )
}

fn temp_row(code: &str, c: f32, state: &AppState, w: usize) -> Line<'static> {
    let cfg = &state.config;
    let (v, u) = split_unit(&format::temperature(c, cfg.units.temperature));
    let status = Status::of(c, cfg.thresholds.temp_warn_c);
    row(code, &v, &u, Some(status), w)
}

/// `DSK-C` for a drive letter, else `DSK-2` by position.
fn drive_code(prefix: &str, mount: &str, index: usize) -> String {
    let m = network::normalize_mount(mount);
    match m.strip_suffix(':') {
        Some(letter) if letter.len() == 1 => format!("{prefix}-{letter}"),
        _ => format!("{prefix}-{}", index + 1),
    }
}

fn system_cards(state: &AppState, w: usize) -> Vec<Card> {
    let Some(s) = &state.snapshot else {
        let wait = Line::styled("AWAITING TELEMETRY", fg(GREY));
        return vec![Card::new("CPU", vec![wait])];
    };
    let th = &state.config.thresholds;
    let mut cpu = vec![pct_row("CPU-LD", s.cpu_usage, th.cpu_warn_pct, w)];
    if let Some(t) = s.cpu_temp {
        cpu.push(temp_row("CPU-TMP", t, state, w));
    }
    let swap_pct = metrics::pct(s.swap_used_bytes, s.swap_total_bytes);
    let mem = vec![
        pct_row("MEM-US", s.ram_pct(), FULL_WARN_PCT, w),
        amount_row("MEM-USD", s.ram_used_bytes, state, w),
        amount_row("MEM-TOT", s.ram_total_bytes, state, w),
        pct_row("SWP-US", swap_pct, FULL_WARN_PCT, w),
        amount_row("SWP-USD", s.swap_used_bytes, state, w),
        amount_row("SWP-TOT", s.swap_total_bytes, state, w),
    ];
    vec![Card::new("CPU", cpu), Card::new("MEMORY", mem)]
}

fn gpu_cards(state: &AppState, w: usize) -> Vec<Card> {
    let gpus = match &state.snapshot {
        Some(s) if state.config.gpu.enabled => &s.gpus,
        _ => return Vec::new(),
    };
    gpus.iter()
        .enumerate()
        .map(|(i, g)| {
            let mut lines = vec![Line::styled(fit(&g.name, w), fg(GREY))];
            if let Some(load) = g.usage_pct {
                lines.push(pct_row("GPU-LD", load, FULL_WARN_PCT, w));
            }
            if let (Some(used), Some(total)) = (g.mem_used_bytes, g.mem_total_bytes) {
                lines.push(pct_row(
                    "GPU-VR",
                    metrics::pct(used, total),
                    FULL_WARN_PCT,
                    w,
                ));
                lines.push(amount_row("GPU-VRU", used, state, w));
                lines.push(amount_row("GPU-VRT", total, state, w));
            }
            if let Some(t) = g.temp_c {
                lines.push(temp_row("GPU-TMP", t, state, w));
            }
            if let Some(watts) = g.power_w {
                lines.push(row("GPU-PWR", &format!("{watts:.1}"), "W", None, w));
            }
            let title = if gpus.len() > 1 {
                format!("GPU {}", i + 1)
            } else {
                "GPU".to_string()
            };
            Card::new(title, lines)
        })
        .collect()
}

fn disk_cards(state: &AppState, pal: &Palette, w: usize) -> Vec<Card> {
    let cfg = &state.config;
    let disks = state.snapshot.as_ref().map_or(&[][..], |s| &s.disks);
    let mut lines = Vec::new();
    let shown = disks
        .iter()
        .filter(|d| !network::is_hidden(&d.mount, &cfg.disks.hide));
    for (i, d) in shown.enumerate() {
        let used = common::used_of(d.used_bytes, d.total_bytes, state);
        lines.push(common::kv(&d.title(), used, w, pal, GREY));
        let pct = metrics::pct(d.used_bytes, d.total_bytes);
        let code = drive_code("DSK", &d.mount, i);
        lines.push(pct_row(&code, pct, cfg.thresholds.disk_warn_pct, w));
    }
    if lines.is_empty() {
        lines.push(Line::styled("NO DISKS FOUND", fg(GREY)));
    }
    let mut cards = vec![Card::new("DISKS", lines)];
    cards.extend(network_card(state, pal, w));
    cards
}

fn network_card(state: &AppState, pal: &Palette, w: usize) -> Option<Card> {
    let cfg = &state.config.disks;
    if !cfg.show_network {
        return None;
    }
    let Some(drives) = &state.network else {
        let checking = Line::styled("checking…", fg(GREY));
        return Some(Card::new("NETWORK", vec![checking]));
    };
    let rows = network::rows(drives, cfg.group_network, &cfg.hide);
    if rows.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for (i, r) in rows.iter().enumerate() {
        let mount = r.letters.first().map_or("", String::as_str);
        let code = drive_code("NET", mount, i);
        if r.online {
            let used = common::used_of(r.used_bytes, r.total_bytes, state);
            lines.push(common::kv(&r.title, used, w, pal, GREY));
            let pct = metrics::pct(r.used_bytes, r.total_bytes);
            let warn = state.config.thresholds.disk_warn_pct;
            lines.push(pct_row(&code, pct, warn, w));
        } else {
            lines.push(common::kv(&r.title, String::new(), w, pal, GREY));
            lines.push(row(&code, "OFFLINE", "", Some(Status::Warning), w));
        }
    }
    Some(Card::new("NETWORK", lines))
}

fn cards(group: Group, state: &AppState, pal: &Palette, w: usize) -> Vec<Card> {
    match group {
        Group::System => system_cards(state, w),
        Group::Gpu => gpu_cards(state, w),
        Group::Disks => disk_cards(state, pal, w),
        Group::Plugins => common::plugin_cards(state, pal, w)
            .into_iter()
            .map(|mut c| {
                c.title = c.title.to_uppercase();
                c
            })
            .collect(),
    }
}

/// ` TELEMETRIX MISSION CONTROL     MET 012:04:31:07  GMT 271:14:32:07 `
pub fn header(uptime_s: u64, now: SystemTime, w: usize) -> Line<'static> {
    let label = fg(AMBER).add_modifier(Modifier::BOLD);
    let time = fg(WHITE).add_modifier(Modifier::BOLD);
    let right = vec![
        Span::styled("MET ", label),
        Span::styled(format::elapsed_clock(uptime_s), time),
        Span::styled("  GMT ", label),
        Span::styled(format::utc_day_clock(now), time),
        Span::raw(" "),
    ];
    let right_w: usize = right.iter().map(|s| s.content.chars().count()).sum();
    let left = " TELEMETRIX MISSION CONTROL";
    let mut spans = Vec::new();
    if w > right_w + left.len() {
        spans.push(Span::styled(left, label));
        spans.push(Span::raw(" ".repeat(w - right_w - left.len())));
    } else {
        spans.push(Span::raw(" ".repeat(w.saturating_sub(right_w))));
    }
    spans.extend(right);
    Line::from(spans)
}

/// Apollo-era flight console: every metric is a row of code, value, unit
/// and status, white and amber on black. Static: redraws only on new data.
pub struct Nasa;

impl Theme for Nasa {
    fn name(&self) -> &'static str {
        "nasa"
    }

    fn frame_interval(&self, _cfg: &Config) -> Option<Duration> {
        None
    }

    fn draw(&mut self, frame: &mut Frame, state: &AppState) {
        common::fill_screen(frame, SCREEN);
        let pal = palette();
        let body = common::body_area(frame.area(), state);
        // The top margin row of the columns holds the mission clocks.
        let top = Rect { height: 1, ..body };
        let line = header(
            sysinfo::System::uptime(),
            SystemTime::now(),
            usize::from(body.width),
        );
        frame.render_widget(Paragraph::new(line).style(Style::new().bg(SCREEN)), top);
        for (col, groups) in common::columns(body, true) {
            let w = common::text_width(col, &pal);
            let mut cards: Vec<Card> = groups
                .iter()
                .flat_map(|g| cards(*g, state, &pal, w))
                .collect();
            if groups.first() != Some(&Group::Plugins)
                && let Some(first) = cards.first_mut()
            {
                first.lines.insert(0, heading(w));
            }
            common::stack(frame, col, cards, &pal, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{TOO_SMALL, demo};
    use ratatui::buffer::Buffer;

    fn text(buf: &Buffer) -> String {
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    fn fg_of(buf: &Buffer, needle: &str) -> Color {
        let cells = buf.content();
        let symbols: Vec<&str> = cells.iter().map(|c| c.symbol()).collect();
        let n = needle.chars().count();
        let pos = symbols
            .windows(n)
            .position(|w| w.concat() == needle)
            .unwrap_or_else(|| panic!("{needle} not drawn"));
        cells[pos].fg
    }

    #[test]
    fn rows_have_code_value_unit_and_status() {
        let buf = demo::render(&demo::state("nasa"), 100, 30);
        let t = text(&buf);
        for needle in [
            "MET ",
            "GMT ",
            "TELEMETRIX MISSION CONTROL",
            "CODE",
            "STATUS",
            " CPU ─",
            "CPU-LD",
            "23.0 %    NOMINAL",
            "MEM-USD",
            "DSK-C",
            "GPU-TMP",
            "112.4 W",
            " CRYPTO ─",
            "84,853 USDT",
            "+2.1%",
            " WEATHER ─",
        ] {
            assert!(t.contains(needle), "{needle}");
        }
        assert_eq!(fg_of(&buf, "CPU-LD"), AMBER);
        assert_eq!(fg_of(&buf, "CAUTION"), AMBER, "the 87 % full games disk");
        assert_eq!(buf[(0, 0)].bg, SCREEN);
        assert!(!t.contains('│'), "thin rules only, no side frames");
    }

    #[test]
    fn a_full_cpu_is_a_red_warning() {
        let mut s = demo::state("nasa");
        let mut snap = demo::snapshot();
        snap.cpu_usage = 100.0;
        s.snapshot = Some(snap);
        let buf = demo::render(&s, 100, 30);
        assert_eq!(fg_of(&buf, "WARNING"), RED);
        assert_eq!(fg_of(&buf, "100.0"), RED);
    }

    #[test]
    fn small_screens_still_show_rows_or_the_notice() {
        let t = text(&demo::render(&demo::state("nasa"), 40, 10));
        assert!(t.contains("CPU-LD") && t.contains(" telemetrix "), "{t}");
        assert!(!t.contains(TOO_SMALL));
        let t = text(&demo::render(&demo::state("nasa"), 30, 8));
        assert!(t.contains("terminal too small"), "{t}");
    }

    #[test]
    fn header_and_status_words() {
        let t = SystemTime::UNIX_EPOCH + Duration::from_secs(1_790_366_400);
        let line: String = header(93_784, t, 60)
            .spans
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(line.chars().count(), 60);
        assert!(
            line.ends_with("MET 001:02:03:04  GMT 268:20:00:00 "),
            "{line}"
        );
        assert_eq!(Status::of(50.0, 80.0), Status::Nominal);
        assert_eq!(Status::of(70.0, 80.0), Status::Caution);
        assert_eq!(Status::of(81.0, 80.0), Status::Warning);
        assert_eq!(drive_code("DSK", "C:\\", 0), "DSK-C");
        assert_eq!(drive_code("DSK", "/home", 1), "DSK-2");
    }
}
