//! Cards, bars and the column layout shared by every theme.

use std::collections::VecDeque;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Padding, Paragraph};

use crate::app::AppState;
use crate::config::{ThemeOptions, View};
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
    /// The box-drawing characters of the card frames.
    pub border_set: border::Set<'static>,
    /// Put around a card title, like `[ ` and ` ]`.
    pub brackets: (&'static str, &'static str),
    /// Values above a threshold, errors and falling trends.
    pub warn: Color,
    /// Rising trends and `good` rows.
    pub rise: Color,
    /// `dim` rows and stale values.
    pub muted: Color,
    pub gauge: Gauge,
    /// Which sides of a card get a frame line.
    pub borders: Borders,
    /// History chart rows up to half, up to 80 % and above.
    pub levels: [Color; 3],
}

/// How a percentage gauge is drawn.
#[derive(Clone, Copy)]
pub struct Gauge {
    pub full: char,
    pub empty: char,
    /// Put around the gauge, like `[` and `]`.
    pub ends: (&'static str, &'static str),
    /// Fill the last cell in eighths (`▏▎▍▌▋▊▉`) for a finer reading.
    pub eighths: bool,
}

pub const BLOCK_GAUGE: Gauge = Gauge {
    full: '█',
    empty: '░',
    ends: ("", ""),
    eighths: false,
};

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
        border_set: border::ROUNDED,
        brackets: (" ", " "),
        warn: WARN,
        rise: RISE,
        muted: MUTED,
        gauge: BLOCK_GAUGE,
        borders: Borders::ALL,
        levels: [Color::Rgb(150, 150, 150), Color::Rgb(215, 215, 215), WARN],
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
        border_set: border::PLAIN,
        brackets: (" ", " "),
        warn: WARN,
        rise: RISE,
        muted: MUTED,
        gauge: BLOCK_GAUGE,
        borders: Borders::ALL,
        levels: [scale(c, 0.9), mix_white(c, 0.55), WARN],
    }
}

/// `red`, `#ff8800` and the other accent names as RGB; `None` for `default`
/// and anything unknown.
pub fn accent_rgb(name: &str) -> Option<(u8, u8, u8)> {
    let named = match name {
        "red" => (255, 85, 85),
        "orange" => (255, 150, 50),
        "amber" => (255, 176, 0),
        "yellow" => (240, 220, 60),
        "green" => (80, 220, 100),
        "cyan" => (0, 215, 255),
        "blue" => (90, 150, 255),
        "purple" => (170, 110, 255),
        "magenta" => (255, 70, 200),
        "pink" => (255, 130, 180),
        "white" => (230, 230, 230),
        _ => {
            let hex = |i: usize| {
                name.get(i..i + 2)
                    .and_then(|h| u8::from_str_radix(h, 16).ok())
            };
            return match (
                name.starts_with('#') && name.len() == 7,
                hex(1),
                hex(3),
                hex(5),
            ) {
                (true, Some(r), Some(g), Some(b)) => Some((r, g, b)),
                _ => None,
            };
        }
    };
    Some(named)
}

/// The options of the theme being drawn.
pub fn options(state: &AppState) -> &ThemeOptions {
    state.config.theme(state.theme_name())
}

/// The accent of the theme being drawn, when it is not `default`.
pub fn accent(state: &AppState) -> Option<(u8, u8, u8)> {
    accent_rgb(&options(state).accent)
}

/// A palette with the theme's accent as its title and gauge colour.
pub fn accented(mut pal: Palette, state: &AppState) -> Palette {
    if let Some((r, g, b)) = accent(state) {
        pal.title = Color::Rgb(r, g, b);
        pal.bar = Color::Rgb(r, g, b);
    }
    pal
}

/// Paints the whole screen in one background colour, under the cards.
pub fn fill_screen(frame: &mut Frame, bg: Color) {
    let area = frame.area();
    frame.buffer_mut().set_style(area, Style::new().bg(bg));
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
    /// What `hide` and `order` call it: `cpu`, `disks`, a plugin id.
    pub id: String,
    pub title: String,
    pub title_color: Option<Color>,
    pub lines: Vec<Line<'static>>,
}

impl Card {
    pub fn new(title: impl Into<String>, lines: Vec<Line<'static>>) -> Self {
        Self {
            id: String::new(),
            title: title.into(),
            title_color: None,
            lines,
        }
    }

    pub fn id(mut self, id: &str) -> Self {
        self.id = id.to_string();
        self
    }
}

pub const MIN_CHART_ROWS: usize = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Group {
    System,
    Gpu,
    Disks,
    Plugins,
}

/// Three columns from 96 cells wide, two from 64, else one. With fewer
/// columns the GPU card comes after Disks, so the cards that were there
/// before it keep their place when space runs out.
pub fn plan(width: u16) -> &'static [&'static [Group]] {
    if width >= 96 {
        &[
            &[Group::System, Group::Gpu],
            &[Group::Disks],
            &[Group::Plugins],
        ]
    } else if width >= 64 {
        &[
            &[Group::System, Group::Disks, Group::Gpu],
            &[Group::Plugins],
        ]
    } else {
        &[&[Group::System, Group::Disks, Group::Gpu, Group::Plugins]]
    }
}

/// Each column's area and the card groups that go into it.
pub fn columns(body: Rect, spaced: bool) -> Vec<(Rect, &'static [Group])> {
    let plan = plan(body.width);
    let margin = u16::from(spaced);
    let rects = Layout::horizontal(vec![Constraint::Fill(1); plan.len()])
        .spacing(if spaced { 2 } else { 1 })
        .horizontal_margin(margin * 2)
        .vertical_margin(margin)
        .split(body);
    rects.iter().copied().zip(plan.iter().copied()).collect()
}

/// The text width inside a card of a column this wide.
pub fn text_width(col: Rect, pal: &Palette) -> usize {
    let sides = [Borders::LEFT, Borders::RIGHT]
        .iter()
        .filter(|b| pal.borders.contains(**b))
        .count() as u16;
    usize::from(col.width.saturating_sub(2 + sides))
}

/// The standard cards of one group; `rows` is the height of history charts.
pub fn group_cards(
    group: Group,
    state: &AppState,
    pal: &Palette,
    w: usize,
    rows: usize,
) -> Vec<Card> {
    match group {
        Group::System => system_cards(state, pal, w, rows),
        Group::Gpu => gpu_cards(state, pal, w),
        Group::Disks => std::iter::once(disk_card(state, pal, w))
            .chain(network_card(state, pal, w))
            .collect(),
        Group::Plugins => plugin_cards(state, pal, w),
    }
}

pub fn draw_columns(frame: &mut Frame, body: Rect, state: &AppState, pal: &Palette, spaced: bool) {
    draw_cards(
        frame,
        body,
        state,
        pal,
        (spaced, u16::from(spaced)),
        |g, w, rows| group_cards(g, state, pal, w, rows),
        |_, _| {},
    );
}

/// The cards of every column, after the theme's `hide` and `order`.
/// Without `order` each column holds its groups (system cards left,
/// plugins right); with it the cards fill the columns one after another.
pub fn place(
    cols: &[(Rect, &'static [Group])],
    opts: &ThemeOptions,
    pal: &Palette,
    gap: u16,
    make: &mut dyn FnMut(Group, usize) -> Vec<Card>,
) -> Vec<Vec<Card>> {
    let mut visible = |g: Group, w: usize| -> Vec<Card> {
        make(g, w)
            .into_iter()
            .filter(|c| !opts.hidden(&c.id))
            .collect()
    };
    if opts.order.is_empty() {
        return cols
            .iter()
            .map(|(rect, groups)| {
                let w = text_width(*rect, pal);
                groups.iter().flat_map(|g| visible(*g, w)).collect()
            })
            .collect();
    }
    let mut ordered = |w: usize| -> Vec<Card> {
        let mut all: Vec<Card> = cols
            .iter()
            .flat_map(|(_, groups)| groups.iter().copied())
            .flat_map(|g| visible(g, w))
            .collect();
        // Stable: unlisted cards keep their default order after the listed ones.
        all.sort_by_key(|c| {
            opts.order
                .iter()
                .position(|o| *o == c.id)
                .unwrap_or(usize::MAX)
        });
        all
    };
    let widths: Vec<usize> = cols.iter().map(|(c, _)| text_width(*c, pal)).collect();
    let min_w = widths.iter().copied().min().unwrap_or(0);
    let first = ordered(min_w);
    // Which column each card goes to: a column is full when the next card
    // does not fit, or when it already holds its share of all the rows.
    let total: usize = first
        .iter()
        .map(|c| card_height(c, pal) + usize::from(gap))
        .sum();
    let share = total.div_ceil(cols.len().max(1));
    let mut column_of = Vec::with_capacity(first.len());
    let (mut c, mut used) = (0, 0usize);
    for card in &first {
        let h = card_height(card, pal);
        let need = if used == 0 { h } else { h + usize::from(gap) };
        let full = used + need > usize::from(cols[c].0.height) || used >= share;
        if used > 0 && full && c + 1 < cols.len() {
            c += 1;
            used = h;
        } else {
            used += need;
        }
        column_of.push(c);
    }
    // The same cards again at each column's own width, so text lines up
    // with the frame as it does without an order.
    let mut out: Vec<Vec<Card>> = cols.iter().map(|_| Vec::new()).collect();
    let mut by_width: Vec<(usize, Vec<Option<Card>>)> =
        vec![(min_w, first.into_iter().map(Some).collect())];
    for (i, col) in column_of.into_iter().enumerate() {
        let w = widths[col];
        let slot = match by_width.iter().position(|(bw, _)| *bw == w) {
            Some(p) => p,
            None => {
                by_width.push((w, ordered(w).into_iter().map(Some).collect()));
                by_width.len() - 1
            }
        };
        if let Some(card) = by_width[slot].1.get_mut(i).and_then(Option::take) {
            out[col].push(card);
        }
    }
    out
}

fn used_rows(cards: &[Card], pal: &Palette, gap: u16) -> usize {
    let h: usize = cards.iter().map(|c| card_height(c, pal)).sum();
    h + usize::from(gap) * cards.len().saturating_sub(1)
}

/// History chart rows: what the columns with charts have left, shared
/// between their charts, at most the theme's `chart_height`.
pub fn chart_rows(
    cols: &[(Rect, &'static [Group])],
    placed: &[Vec<Card>],
    opts: &ThemeOptions,
    pal: &Palette,
    gap: u16,
) -> usize {
    let is_chart = |c: &Card| {
        (c.id == "cpu" && opts.cpu_view.chart()) || (c.id == "ram" && opts.ram_view.chart())
    };
    let extra = cols
        .iter()
        .zip(placed)
        .filter_map(|((rect, _), cards)| {
            let charts = cards.iter().filter(|c| is_chart(c)).count();
            (charts > 0).then(|| {
                usize::from(rect.height).saturating_sub(used_rows(cards, pal, gap)) / charts
            })
        })
        .min()
        .unwrap_or(0);
    (MIN_CHART_ROWS + extra).clamp(MIN_CHART_ROWS, opts.chart_cap().max(MIN_CHART_ROWS))
}

/// Places the cards `make` gives for each group, with the theme's options,
/// and draws them. `spacing` is (margins around the columns, rows between
/// cards). `decorate` may change the cards of one column (and gets its
/// text width) before they are drawn.
pub fn draw_cards(
    frame: &mut Frame,
    body: Rect,
    state: &AppState,
    pal: &Palette,
    (spaced, gap): (bool, u16),
    mut make: impl FnMut(Group, usize, usize) -> Vec<Card>,
    decorate: impl Fn(&mut Vec<Card>, usize),
) {
    let opts = options(state);
    let cols = columns(body, spaced);
    let mut placed = place(&cols, opts, pal, gap, &mut |g, w| {
        make(g, w, MIN_CHART_ROWS)
    });
    let rows = chart_rows(&cols, &placed, opts, pal, gap);
    if rows != MIN_CHART_ROWS {
        placed = place(&cols, opts, pal, gap, &mut |g, w| make(g, w, rows));
    }
    for ((col, _), mut cards) in cols.into_iter().zip(placed) {
        decorate(&mut cards, text_width(col, pal));
        stack(frame, col, cards, pal, gap);
    }
}

/// A multi-row bar chart, top row first: the last samples (0..100) that
/// fit in `width`, right-aligned, each column as tall as its value in
/// eighths of a row, with a 0..100 scale on the left.
pub fn chart(
    history: &VecDeque<f32>,
    width: usize,
    height: usize,
    pal: &Palette,
) -> Vec<Line<'static>> {
    let bars = width.saturating_sub(AXIS_W);
    let skip = history.len().saturating_sub(bars);
    let values: Vec<f32> = history.iter().skip(skip).copied().collect();
    let pad = bars - values.len();
    (0..height)
        .map(|row| {
            let below = (height - 1 - row) as f32;
            let mut glyphs = " ".repeat(pad);
            for v in &values {
                let fill = ((v / 100.0).clamp(0.0, 1.0) * height as f32 - below).clamp(0.0, 1.0);
                let eighths = (fill * 8.0).round() as usize;
                glyphs.push(if eighths == 0 {
                    ' '
                } else {
                    CHART_BLOCKS[eighths - 1]
                });
            }
            let axis = match row {
                0 => "100┤",
                r if r == height - 1 => "  0┤",
                r if height >= 5 && r == height / 2 => " 50┤",
                _ => "   │",
            };
            let top_pct = (height - row) as f32 / height as f32 * 100.0;
            let level = if top_pct <= 50.0 {
                pal.levels[0]
            } else if top_pct <= 80.0 {
                pal.levels[1]
            } else {
                pal.levels[2]
            };
            Line::from(vec![
                Span::styled(axis, fg(pal.muted)),
                Span::styled(glyphs, fg(level)),
            ])
        })
        .collect()
}

/// Bottom-aligned blocks from one eighth to a full cell.
const CHART_BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// `100┤`: the scale left of a chart.
pub const AXIS_W: usize = 4;

/// `avg 26%  peak 35%` over the samples a chart of this width shows.
pub fn summary(history: &VecDeque<f32>, width: usize) -> String {
    let bars = width.saturating_sub(AXIS_W).max(1);
    let shown: Vec<f32> = history.iter().rev().take(bars).copied().collect();
    if shown.is_empty() {
        return String::new();
    }
    let avg = shown.iter().sum::<f32>() / shown.len() as f32;
    let peak = shown.iter().copied().fold(0.0, f32::max);
    format!("avg {avg:.0}%  peak {peak:.0}%")
}

/// The rows a card takes: its lines and its top and bottom frame lines.
pub fn card_height(card: &Card, pal: &Palette) -> usize {
    let frame = [Borders::TOP, Borders::BOTTOM]
        .iter()
        .filter(|b| pal.borders.contains(**b))
        .count();
    card.lines.len() + frame
}

/// Draws the cards from the top of `col` down; the ones that do not fit are left out.
pub fn stack(frame: &mut Frame, col: Rect, cards: Vec<Card>, pal: &Palette, gap: u16) {
    let mut y = col.y;
    for card in cards {
        let room = col.bottom().saturating_sub(y);
        if room < 3 {
            break;
        }
        let want = u16::try_from(card_height(&card, pal)).unwrap_or(u16::MAX);
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
    let (open, close) = pal.brackets;
    let block = Block::new()
        .borders(pal.borders)
        .border_set(pal.border_set)
        .border_style(fg(pal.border))
        .padding(Padding::horizontal(1))
        .title(Span::styled(
            format!("{open}{}{close}", card.title),
            title_style,
        ))
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

/// A gauge in the palette's glyphs with the percentage at the right end.
pub fn bar(pct: f32, width: usize, pal: &Palette, warn: bool) -> Line<'static> {
    let g = pal.gauge;
    let (open, close) = g.ends;
    let bar_w = width.saturating_sub(6 + open.chars().count() + close.chars().count());
    let fill = (pct / 100.0).clamp(0.0, 1.0) * bar_w as f32;
    let (filled, part) = if g.eighths {
        let eighths = (fill * 8.0).round() as usize;
        (eighths / 8, eighths % 8)
    } else {
        (fill.round() as usize, 0)
    };
    let color = if warn { pal.warn } else { pal.bar };
    let mut body: String = std::iter::repeat_n(g.full, filled).collect();
    if part > 0 {
        body.push(EIGHTHS[part - 1]);
    }
    let rest = bar_w.saturating_sub(filled + usize::from(part > 0));
    Line::from(vec![
        Span::styled(open, fg(pal.border)),
        Span::styled(body, fg(color)),
        Span::styled(
            std::iter::repeat_n(g.empty, rest).collect::<String>(),
            fg(pal.bar_empty),
        ),
        Span::styled(close, fg(pal.border)),
        Span::styled(
            format!(" {pct:>4.0}%"),
            fg(if warn { pal.warn } else { pal.value }),
        ),
    ])
}

/// Left blocks from one eighth to seven eighths of a cell.
const EIGHTHS: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];

fn spark(history: &VecDeque<f32>, width: usize, color: Color) -> Line<'static> {
    let skip = history.len().saturating_sub(width);
    let values: Vec<f32> = history.iter().skip(skip).copied().collect();
    Line::styled(format::sparkline(&values, width), fg(color))
}

pub fn used_of(used: u64, total: u64, state: &AppState) -> String {
    let unit = state.config.units.bytes;
    format!(
        "{} / {}",
        format::bytes(used, unit),
        format::bytes(total, unit)
    )
}

/// CPU, RAM and swap. The theme's `cpu_view` and `ram_view` choose a bar,
/// a history chart of `rows` rows, or both.
pub fn system_cards(state: &AppState, pal: &Palette, w: usize, rows: usize) -> Vec<Card> {
    let Some(s) = &state.snapshot else {
        let wait = Line::styled("waiting for metrics", fg(pal.label));
        return vec![Card::new("CPU", vec![wait]).id("cpu")];
    };
    let cfg = &state.config;
    let opts = options(state);
    let cpu_warn = s.cpu_usage > cfg.thresholds.cpu_warn_pct;
    let temp_color = match s.cpu_temp {
        Some(t) if t > cfg.thresholds.temp_warn_c => pal.warn,
        _ => pal.value,
    };
    let temp = s
        .cpu_temp
        .map(|t| format::temperature(t, cfg.units.temperature));
    let sparks = cfg.theme_minimalist.show_sparklines;
    let mut cpu = Vec::new();
    if opts.cpu_view.chart() {
        cpu.extend(chart(&state.cpu_history, w, rows, pal));
        let avg = summary(&state.cpu_history, w);
        cpu.push(kv(
            &avg,
            temp.clone().unwrap_or_default(),
            w,
            pal,
            temp_color,
        ));
    }
    if opts.cpu_view.bar() {
        cpu.push(bar(s.cpu_usage, w, pal, cpu_warn));
        if let (Some(t), false) = (&temp, opts.cpu_view.chart()) {
            cpu.push(kv("temp", t.clone(), w, pal, temp_color));
        }
        if sparks && opts.cpu_view == View::Bar {
            let color = if cpu_warn { pal.warn } else { pal.spark };
            cpu.push(spark(&state.cpu_history, w, color));
        }
    }
    let used = used_of(s.ram_used_bytes, s.ram_total_bytes, state);
    let mut ram = Vec::new();
    if opts.ram_view.chart() {
        ram.extend(chart(&state.ram_history, w, rows, pal));
    }
    ram.push(kv("used", used, w, pal, pal.value));
    if opts.ram_view.bar() {
        ram.push(bar(s.ram_pct(), w, pal, false));
        if sparks && opts.ram_view == View::Bar {
            ram.push(spark(&state.ram_history, w, pal.spark));
        }
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
    // Only a chart has no gauge with the percentage, so the title carries it.
    let mut cpu_card = if opts.cpu_view == View::Chart {
        Card::new(format!("CPU {:.0}%", s.cpu_usage), cpu)
    } else {
        Card::new("CPU", cpu)
    };
    if opts.cpu_view == View::Chart && cpu_warn {
        cpu_card.title_color = Some(pal.warn);
    }
    let ram_title = if opts.ram_view == View::Chart {
        format!("RAM {:.0}%", s.ram_pct())
    } else {
        "RAM".to_string()
    };
    vec![
        cpu_card.id("cpu"),
        Card::new(ram_title, ram).id("ram"),
        Card::new("Swap", swap).id("swap"),
    ]
}

/// One card per GPU; none without readings or with `gpu.enabled = false`.
pub fn gpu_cards(state: &AppState, pal: &Palette, w: usize) -> Vec<Card> {
    let cfg = &state.config;
    let gpus = match &state.snapshot {
        Some(s) if cfg.gpu.enabled => &s.gpus,
        _ => return Vec::new(),
    };
    gpus.iter()
        .enumerate()
        .map(|(i, g)| {
            let mut lines = vec![Line::styled(fit(&g.name, w), fg(pal.label))];
            if let Some(load) = g.usage_pct {
                lines.push(bar(load, w, pal, false));
            }
            if let (Some(used), Some(total)) = (g.mem_used_bytes, g.mem_total_bytes) {
                lines.push(kv("vram", used_of(used, total, state), w, pal, pal.value));
                lines.push(bar(metrics::pct(used, total), w, pal, false));
            }
            if let Some(t) = g.temp_c {
                let color = if t > cfg.thresholds.temp_warn_c {
                    pal.warn
                } else {
                    pal.value
                };
                lines.push(kv(
                    "temp",
                    format::temperature(t, cfg.units.temperature),
                    w,
                    pal,
                    color,
                ));
            }
            if let Some(watts) = g.power_w {
                lines.push(kv("power", format!("{watts:.1} W"), w, pal, pal.value));
            }
            let title = if gpus.len() > 1 {
                format!("GPU {}", i + 1)
            } else {
                "GPU".to_string()
            };
            Card::new(title, lines).id("gpu")
        })
        .collect()
}

pub fn disk_card(state: &AppState, pal: &Palette, w: usize) -> Card {
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
    Card::new("Disks", lines).id("disks")
}

/// The Network card, drawn like Disks; `None` when there is nothing to show.
pub fn network_card(state: &AppState, pal: &Palette, w: usize) -> Option<Card> {
    let cfg = &state.config.disks;
    if !cfg.show_network {
        return None;
    }
    let Some(drives) = &state.network else {
        let checking = Line::styled("checking…", fg(pal.label));
        return Some(Card::new("Network", vec![checking]).id("network"));
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
            lines.push(kv(&r.title, "offline".into(), w, pal, pal.warn));
        }
    }
    Some(Card::new("Network", lines).id("network"))
}

pub fn plugin_cards(state: &AppState, pal: &Palette, w: usize) -> Vec<Card> {
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
        cards.push(Card::new("Plugins", lines).id("plugins"));
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

pub fn plugin_card(card: &PluginCard, pal: &Palette, w: usize) -> Card {
    let d = &card.data;
    match &card.status {
        PluginStatus::Ok => Card::new(
            d.title.clone(),
            d.metrics.iter().map(|m| metric_line(m, pal, w)).collect(),
        )
        .id(&d.id),
        PluginStatus::Error(msg) => {
            let mut lines: Vec<Line<'static>> = wrap(&format!("Error: {msg}"), w)
                .into_iter()
                .map(|l| Line::styled(l, fg(pal.warn)))
                .collect();
            for m in &d.metrics {
                lines.push(kv(
                    &m.label,
                    format!("{} (stale)", m.value),
                    w,
                    pal,
                    pal.muted,
                ));
            }
            Card {
                id: d.id.clone(),
                title: d.title.clone(),
                title_color: Some(pal.warn),
                lines,
            }
        }
    }
}

pub fn metric_line(m: &crate::plugins::MetricItem, pal: &Palette, w: usize) -> Line<'static> {
    use crate::plugins::MetricStyle;
    if m.bad {
        return kv(&m.label, m.value.clone(), w, pal, pal.warn);
    }
    if let Some(points) = &m.trend {
        return trend_line(&m.label, points, &m.value, w, pal);
    }
    match m.style {
        None => kv(&m.label, m.value.clone(), w, pal, pal.value),
        // Without a value, the label carries the colour (a footer like "stale since 14:32").
        Some(MetricStyle::Good | MetricStyle::Bad) => {
            let color = if m.style == Some(MetricStyle::Good) {
                pal.rise
            } else {
                pal.warn
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
            styled_kv(&m.label, value, w, fg(pal.muted), fg(pal.muted))
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
        (Some(a), Some(b)) if b > a => pal.rise,
        (Some(a), Some(b)) if b < a => pal.warn,
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
        assert_eq!(fit("Archive Storage (D:)", 12), "Archive Sto…");
        assert_eq!(fit("abc", 0), "");
        let pal = minimalist_palette();
        let line = kv(
            "Archive Storage (D:)",
            "554.3 GiB / 1.8 TiB".into(),
            30,
            &pal,
            pal.value,
        );
        let text: String = line.spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text.chars().count(), 30);
        assert!(text.ends_with("554.3 GiB / 1.8 TiB"), "{text}");
        assert!(text.starts_with("Archive S…"), "{text}");
    }
}
