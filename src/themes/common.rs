//! Cards, bars and the column layout shared by every theme (ported from the
//! approved `examples/mockup`).

use std::collections::VecDeque;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use crate::app::AppState;
use crate::format;
use crate::metrics::{self, SystemSnapshot};
use crate::plugins::{PluginCard, PluginStatus};

pub const MIN_W: u16 = 40;
pub const MIN_H: u16 = 10;
pub const WARN: Color = Color::Rgb(255, 95, 95);
pub const ACCENT: Color = Color::Rgb(215, 135, 0);
pub const MUTED: Color = Color::Rgb(115, 115, 115);

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
                Group::Disks => vec![disk_card(state, pal, w)],
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
    let pad = width
        .saturating_sub(label.chars().count() + value.chars().count())
        .max(1);
    Line::from(vec![
        Span::styled(label.to_string(), fg(pal.label)),
        Span::raw(" ".repeat(pad)),
        Span::styled(value, fg(value_color)),
    ])
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
    for d in disks {
        let pct = metrics::pct(d.used_bytes, d.total_bytes);
        lines.push(kv(
            &d.mount,
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

fn plugin_cards(state: &AppState, pal: &Palette, w: usize) -> Vec<Card> {
    let mut cards: Vec<Card> = state
        .plugins
        .values()
        .map(|p| plugin_card(p, pal, w))
        .collect();
    if cards.is_empty() {
        let hint = Line::styled("no plugins yet", fg(pal.label));
        cards.push(Card::new("Plugins", vec![hint]));
    }
    cards
}

fn plugin_card(card: &PluginCard, pal: &Palette, w: usize) -> Card {
    let d = &card.data;
    match &card.status {
        PluginStatus::Ok => Card::new(
            d.title.clone(),
            d.metrics
                .iter()
                .map(|m| kv(&m.label, m.value.clone(), w, pal, pal.value))
                .collect(),
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
