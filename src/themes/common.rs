//! Cards, bars and the column layout shared by every theme.

use std::collections::VecDeque;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use crate::app::AppState;
use crate::format;
use crate::metrics::network;
use crate::metrics::{self, SystemSnapshot};
use crate::plugins::{PluginCard, PluginStatus};

pub const MIN_W: u16 = 40;
pub const MIN_H: u16 = 10;
pub const WARN: Color = Color::Rgb(255, 95, 95);
pub const ACCENT: Color = Color::Rgb(215, 135, 0);
pub const MUTED: Color = Color::Rgb(115, 115, 115);
/// A trend value that went up (falling ones use `WARN`).
pub const RISE: Color = Color::Rgb(90, 200, 120);

pub struct Palette {
    pub bg: Option<Color>,
    pub border: Color,
    pub title: Color,
    pub label: Color,
    pub value: Color,
    pub bar: Color,
    pub bar_empty: Color,
    pub spark: Color,
    pub rounded: bool,
}

pub fn minimalist_palette() -> Palette {
    Palette {
        bg: None,
        border: Color::Rgb(78, 78, 78),
        title: Color::Rgb(200, 200, 200),
        label: Color::Rgb(128, 128, 128),
        value: Color::Rgb(215, 215, 215),
        bar: Color::Rgb(150, 150, 150),
        bar_empty: Color::Rgb(58, 58, 58),
        spark: Color::Rgb(120, 120, 120),
        rounded: true,
    }
}

pub fn scale((r, g, b): (u8, u8, u8), k: f32) -> Color {
    let f = |c: u8| (f32::from(c) * k).clamp(0.0, 255.0) as u8;
    Color::Rgb(f(r), f(g), f(b))
}

pub fn mix_white((r, g, b): (u8, u8, u8), k: f32) -> Color {
    let f = |c: u8| (f32::from(c) + (255.0 - f32::from(c)) * k) as u8;
    Color::Rgb(f(r), f(g), f(b))
}

/// Cards tinted with the rain colour on a dark solid background.
pub fn matrix_palette(c: (u8, u8, u8)) -> Palette {
    Palette {
        bg: Some(scale(c, 0.07)),
        border: scale(c, 0.55),
        title: mix_white(c, 0.2),
        label: scale(c, 0.65),
        value: mix_white(c, 0.55),
        bar: scale(c, 0.9),
        bar_empty: scale(c, 0.25),
        spark: scale(c, 0.8),
        rounded: false,
    }
}

pub fn fg(c: Color) -> Style {
    Style::new().fg(c)
}

pub fn too_small(area: Rect) -> bool {
    area.width < MIN_W || area.height < MIN_H
}

/// The screen minus the banner line (when shown) and the status bar.
pub fn body_area(area: Rect, state: &AppState) -> Rect {
    let [_, body, _] = Layout::vertical([
        Constraint::Length(u16::from(state.banner().is_some())),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    body
}

pub struct Card {
    pub title: String,
    pub title_color: Option<Color>,
    pub lines: Vec<Line<'static>>,
}

impl Card {
    fn new(title: impl Into<String>, lines: Vec<Line<'static>>) -> Self {
        Self {
            title: title.into(),
            title_color: None,
            lines,
        }
    }
}

#[derive(Clone, Copy)]
enum Group {
    System,
    Disks,
    Plugins,
}

/// Three columns from 96 cells wide, two from 64, else one.
pub fn draw_columns(frame: &mut Frame, body: Rect, state: &AppState, pal: &Palette, spaced: bool) {
    let plan: &[&[Group]] = if body.width >= 96 {
        &[&[Group::System], &[Group::Disks], &[Group::Plugins]]
    } else if body.width >= 64 {
        &[&[Group::System, Group::Disks], &[Group::Plugins]]
    } else {
        &[&[Group::System, Group::Disks, Group::Plugins]]
    };
    let (margin, gap) = if spaced { (1, 1) } else { (0, 0) };
    let columns = Layout::horizontal(vec![Constraint::Fill(1); plan.len()])
        .spacing(if spaced { 2 } else { 1 })
        .horizontal_margin(margin * 2)
        .vertical_margin(margin)
        .split(body);
    for (col, groups) in columns.iter().zip(plan) {
        let w = usize::from(col.width.saturating_sub(4));
        let cards = groups
            .iter()
            .flat_map(|g| match g {
                Group::System => system_cards(state, pal, w),
                Group::Disks => std::iter::once(disk_card(state, pal, w))
                    .chain(network_card(state, pal, w))
                    .collect(),
                Group::Plugins => plugin_cards(state, pal, w),
            })
            .collect();
        stack(frame, *col, cards, pal, gap);
    }
}

fn stack(frame: &mut Frame, col: Rect, cards: Vec<Card>, pal: &Palette, gap: u16) {
    let mut y = col.y;
    for card in cards {
        let room = col.bottom().saturating_sub(y);
        if room < 3 {
            break;
        }
        let want = u16::try_from(card.lines.len() + 2).unwrap_or(u16::MAX);
        let rect = Rect::new(col.x, y, col.width, want.min(room));
        render_card(frame, rect, card, pal);
        y += rect.height + gap;
    }
}

fn render_card(frame: &mut Frame, rect: Rect, card: Card, pal: &Palette) {
    let title_style = fg(card.title_color.unwrap_or(pal.title)).add_modifier(Modifier::BOLD);
    let mut style = Style::new();
    if let Some(bg) = pal.bg {
        style = style.bg(bg);
    }
    let block = Block::bordered()
        .border_type(if pal.rounded {
            BorderType::Rounded
        } else {
            BorderType::Plain
        })
        .border_style(fg(pal.border))
        .padding(Padding::horizontal(1))
        .title(Span::styled(format!(" {} ", card.title), title_style))
        .style(style);
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(card.lines).block(block), rect);
}

pub fn kv(
    label: &str,
    value: String,
    width: usize,
    pal: &Palette,
    value_color: Color,
) -> Line<'static> {
    let label = fit(label, width.saturating_sub(value.chars().count() + 1));
    let pad = width
        .saturating_sub(label.chars().count() + value.chars().count())
        .max(1);
    Line::from(vec![
        Span::styled(label, fg(pal.label)),
        Span::raw(" ".repeat(pad)),
        Span::styled(value, fg(value_color)),
    ])
}

/// Cuts `text` to `max` characters with "…", so values are never pushed out.
pub fn fit(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

/// A block-glyph gauge with the percentage at the right end.
pub fn bar(pct: f32, width: usize, pal: &Palette, warn: bool) -> Line<'static> {
    let bar_w = width.saturating_sub(6);
    let filled = ((pct / 100.0).clamp(0.0, 1.0) * bar_w as f32).round() as usize;
    let color = if warn { WARN } else { pal.bar };
    Line::from(vec![
        Span::styled("█".repeat(filled), fg(color)),
        Span::styled("░".repeat(bar_w - filled), fg(pal.bar_empty)),
        Span::styled(
            format!(" {pct:>4.0}%"),
            fg(if warn { WARN } else { pal.value }),
        ),
    ])
}

fn spark(history: &VecDeque<f32>, width: usize, color: Color) -> Line<'static> {
    let skip = history.len().saturating_sub(width);
    let values: Vec<f32> = history.iter().skip(skip).copied().collect();
    Line::styled(format::sparkline(&values, width), fg(color))
}

fn used_of(used: u64, total: u64, state: &AppState) -> String {
    let unit = state.config.units.bytes;
    format!(
        "{} / {}",
        format::bytes(used, unit),
        format::bytes(total, unit)
    )
}

fn system_cards(state: &AppState, pal: &Palette, w: usize) -> Vec<Card> {
    let Some(s) = &state.snapshot else {
        let wait = Line::styled("waiting for metrics", fg(pal.label));
        return vec![Card::new("CPU", vec![wait])];
    };
    let cfg = &state.config;
    let cpu_warn = s.cpu_usage > cfg.thresholds.cpu_warn_pct;
    let mut cpu = vec![bar(s.cpu_usage, w, pal, cpu_warn)];
    if let Some(t) = s.cpu_temp {
        let color = if t > cfg.thresholds.temp_warn_c {
            WARN
        } else {
            pal.value
        };
        cpu.push(kv(
            "temp",
            format::temperature(t, cfg.units.temperature),
            w,
            pal,
            color,
        ));
    }
    let mut ram = vec![
        kv(
            "used",
            used_of(s.ram_used_bytes, s.ram_total_bytes, state),
            w,
            pal,
            pal.value,
        ),
        bar(s.ram_pct(), w, pal, false),
    ];
    if cfg.theme_minimalist.show_sparklines {
        cpu.push(spark(
            &state.cpu_history,
            w,
            if cpu_warn { WARN } else { pal.spark },
        ));
        ram.push(spark(&state.ram_history, w, pal.spark));
    }
    let swap = vec![
        kv(
            "used",
            used_of(s.swap_used_bytes, s.swap_total_bytes, state),
            w,
            pal,
            pal.value,
        ),
        bar(
            metrics::pct(s.swap_used_bytes, s.swap_total_bytes),
            w,
            pal,
            false,
        ),
    ];
    vec![
        Card::new("CPU", cpu),
        Card::new("RAM", ram),
        Card::new("Swap", swap),
    ]
}

fn disk_card(state: &AppState, pal: &Palette, w: usize) -> Card {
    let disks = state
        .snapshot
        .as_ref()
        .map_or(&[][..], |s: &SystemSnapshot| &s.disks);
    let mut lines = Vec::new();
    let hide = &state.config.disks.hide;
    for d in disks.iter().filter(|d| !network::is_hidden(&d.mount, hide)) {
        let pct = metrics::pct(d.used_bytes, d.total_bytes);
        lines.push(kv(
            &d.title(),
            used_of(d.used_bytes, d.total_bytes, state),
            w,
            pal,
            pal.value,
        ));
        lines.push(bar(
            pct,
            w,
            pal,
            pct > state.config.thresholds.disk_warn_pct,
        ));
    }
    if lines.is_empty() {
        lines.push(Line::styled("no disks found", fg(pal.label)));
    }
    Card::new("Disks", lines)
}

/// The Network card, drawn like Disks; `None` when there is nothing to show.
fn network_card(state: &AppState, pal: &Palette, w: usize) -> Option<Card> {
    let cfg = &state.config.disks;
    if !cfg.show_network {
        return None;
    }
    let Some(drives) = &state.network else {
        let checking = Line::styled("checking…", fg(pal.label));
        return Some(Card::new("Network", vec![checking]));
    };
    let rows = network::rows(drives, cfg.group_network, &cfg.hide);
    if rows.is_empty() {
        return None;
    }
    let mut lines = Vec::new();
    for r in rows {
        if r.online {
            let pct = metrics::pct(r.used_bytes, r.total_bytes);
            lines.push(kv(
                &r.title,
                used_of(r.used_bytes, r.total_bytes, state),
                w,
                pal,
                pal.value,
            ));
            lines.push(bar(
                pct,
                w,
                pal,
                pct > state.config.thresholds.disk_warn_pct,
            ));
        } else {
            lines.push(kv(&r.title, "offline".into(), w, pal, WARN));
        }
    }
    Some(Card::new("Network", lines))
}

fn plugin_cards(state: &AppState, pal: &Palette, w: usize) -> Vec<Card> {
    let mut cards: Vec<Card> = state
        .plugins
        .values()
        .map(|p| plugin_card(p, pal, w))
        .collect();
    if cards.is_empty() {
        let lines = if state.plugins_running > 0 {
            vec![Line::styled("starting plugins…", fg(pal.label))]
        } else {
            let mut lines = vec![Line::styled("no plugins found in:", fg(pal.label))];
            let path = state.plugins_dir.display().to_string();
            for part in wrap_chars(&path, w) {
                lines.push(Line::styled(part, fg(pal.value)));
            }
            lines.push(Line::styled("press l for the log", fg(pal.label)));
            lines
        };
        cards.push(Card::new("Plugins", lines));
    }
    cards
}

/// Cuts text without spaces, like a path, into lines of at most `width` characters.
pub fn wrap_chars(text: &str, width: usize) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars
        .chunks(width.max(1))
        .map(|c| c.iter().collect())
        .collect()
}

fn plugin_card(card: &PluginCard, pal: &Palette, w: usize) -> Card {
    let d = &card.data;
    match &card.status {
        PluginStatus::Ok => Card::new(
            d.title.clone(),
            d.metrics.iter().map(|m| metric_line(m, pal, w)).collect(),
        ),
        PluginStatus::Error(msg) => {
            let mut lines: Vec<Line<'static>> = wrap(&format!("Error: {msg}"), w)
                .into_iter()
                .map(|l| Line::styled(l, fg(WARN)))
                .collect();
            for m in &d.metrics {
                lines.push(kv(&m.label, format!("{} (stale)", m.value), w, pal, MUTED));
            }
            Card {
                title: d.title.clone(),
                title_color: Some(WARN),
                lines,
            }
        }
    }
}

pub fn metric_line(m: &crate::plugins::MetricItem, pal: &Palette, w: usize) -> Line<'static> {
    use crate::plugins::MetricStyle;
    if m.bad {
        return kv(&m.label, m.value.clone(), w, pal, WARN);
    }
    if let Some(points) = &m.trend {
        return trend_line(&m.label, points, &m.value, w, pal);
    }
    match m.style {
        None => kv(&m.label, m.value.clone(), w, pal, pal.value),
        // Without a value, the label carries the colour (a footer like "stale since 14:32").
        Some(MetricStyle::Good | MetricStyle::Bad) => {
            let color = if m.style == Some(MetricStyle::Good) {
                RISE
            } else {
                WARN
            };
            let label = if m.value.is_empty() {
                fg(color)
            } else {
                fg(pal.label)
            };
            styled_kv(&m.label, &m.value, w, label, fg(color))
        }
        Some(MetricStyle::Header) => {
            let style = fg(pal.label).add_modifier(Modifier::BOLD);
            styled_kv(&m.label, &m.value, w, style, style)
        }
        Some(MetricStyle::Dim) => {
            // A secondary row: when both parts do not fit, the value goes.
            let fits = m.label.chars().count() + 1 + m.value.chars().count() <= w;
            let value = if fits { m.value.as_str() } else { "" };
            styled_kv(&m.label, value, w, fg(MUTED), fg(MUTED))
        }
    }
}

/// Like `kv`, with a style for each part.
fn styled_kv(
    label: &str,
    value: &str,
    w: usize,
    label_style: Style,
    value_style: Style,
) -> Line<'static> {
    let label = fit(label, w.saturating_sub(value.chars().count() + 1));
    let pad = w
        .saturating_sub(label.chars().count() + value.chars().count())
        .max(1);
    Line::from(vec![
        Span::styled(label, label_style),
        Span::raw(" ".repeat(pad)),
        Span::styled(value.to_string(), value_style),
    ])
}

/// `label ▁▂▄▆█ +0.8%`: the graph fills the width between label and value.
pub fn trend_line(
    label: &str,
    points: &[f32],
    value: &str,
    w: usize,
    pal: &Palette,
) -> Line<'static> {
    let (first, last) = (points.first().copied(), points.last().copied());
    let color = match (first, last) {
        (Some(a), Some(b)) if b > a => RISE,
        (Some(a), Some(b)) if b < a => WARN,
        _ => pal.value,
    };
    let label = fit(label, w.saturating_sub(value.chars().count() + 1));
    let room = w.saturating_sub(label.chars().count() + value.chars().count() + 2);
    let graph = trend_glyphs(points, room);
    let pad = w
        .saturating_sub(label.chars().count() + graph.chars().count() + value.chars().count() + 1)
        .max(1);
    Line::from(vec![
        Span::styled(label, fg(pal.label)),
        Span::raw(" "),
        Span::styled(graph, fg(pal.spark)),
        Span::raw(" ".repeat(pad)),
        Span::styled(value.to_string(), fg(color)),
    ])
}

/// Scales the points to their own min..max and resamples them to `width`.
pub fn trend_glyphs(points: &[f32], width: usize) -> String {
    if points.len() < 2 || width == 0 {
        return String::new();
    }
    let (lo, hi) = points
        .iter()
        .fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
    let span = hi - lo;
    let sampled: Vec<f32> = (0..width)
        .map(|i| {
            let idx = if width == 1 {
                points.len() - 1
            } else {
                (i * (points.len() - 1) + (width - 1) / 2) / (width - 1)
            };
            let p = points[idx];
            if span > 0.0 {
                (p - lo) / span * 100.0
            } else {
                50.0
            }
        })
        .collect();
    format::sparkline(&sampled, width)
}

pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in text.split(' ') {
        if !cur.is_empty() && cur.chars().count() + 1 + word.chars().count() > width {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(word);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trends_fill_the_width_and_colour_the_direction() {
        let pal = minimalist_palette();
        assert_eq!(trend_glyphs(&[1.0, 2.0], 4), "▁▁██");
        assert_eq!(
            trend_glyphs(&[3.0, 3.0, 3.0], 3),
            "▅▅▅",
            "a flat line sits in the middle"
        );
        assert_eq!(trend_glyphs(&[1.0], 5), "");
        let points: Vec<f32> = (0..400).map(|i| i as f32).collect();
        let line = trend_line("30d", &points, "+4.1%", 30, &pal);
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.chars().count(), 30, "{text}");
        assert!(
            text.starts_with("30d ▁") && text.ends_with(" +4.1%"),
            "{text}"
        );
        assert_eq!(line.spans.last().unwrap().style.fg, Some(RISE));
        let falling = trend_line("7d", &[5.0, 4.0, 1.0], "-2.0%", 30, &pal);
        assert_eq!(falling.spans.last().unwrap().style.fg, Some(WARN));
    }

    #[test]
    fn long_titles_are_cut_so_the_value_stays_visible() {
        assert_eq!(fit("System Disk (C:)", 20), "System Disk (C:)");
        assert_eq!(fit("SSD 1 Media (D:)", 12), "SSD 1 Progr…");
        assert_eq!(fit("abc", 0), "");
        let pal = minimalist_palette();
        let line = kv(
            "SSD 1 Media (D:)",
            "554.3 GiB / 1.8 TiB".into(),
            30,
            &pal,
            pal.value,
        );
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.chars().count(), 30);
        assert!(text.ends_with("554.3 GiB / 1.8 TiB"), "{text}");
        assert!(text.starts_with("SSD 1 Pro…"), "{text}");
    }
}
