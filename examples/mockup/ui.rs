use std::collections::VecDeque;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, Padding, Paragraph};

use crate::data::{self, FakeData, TEMP_WARN_C};
use crate::rain;
use crate::settings::{self, MATRIX, PLUGINS, Row, Settings};
use crate::{App, Level, Overlay};

pub const MIN_W: u16 = 40;
pub const MIN_H: u16 = 10;
pub const TOO_SMALL: &str = "terminal too small (need 40x10)";
pub const WARN: Color = Color::Rgb(255, 95, 95);
const ACCENT: Color = Color::Rgb(215, 135, 0);
const OVERLAY_BG: Color = Color::Rgb(22, 22, 22);
const OVERLAY_FG: Color = Color::Rgb(210, 210, 210);
const MUTED: Color = Color::Rgb(115, 115, 115);
const MATRIX_COLORS: [(u8, u8, u8); 4] =
    [(0, 255, 70), (255, 176, 0), (0, 215, 255), (220, 220, 220)];
const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
const BANNER: &str = " config syntax error line 14: expected '=' — using last good settings";
const HELP: [(&str, &str); 11] = [
    ("q  Esc  Ctrl+C", "quit (Esc closes an open overlay first)"),
    ("t / T", "next / previous theme"),
    ("space", "pause the animation and the fake data"),
    ("s", "settings overlay"),
    ("l", "log overlay"),
    ("r", "rescan plugins (fake)"),
    ("x", "CPU spike to 95 % for 5 s"),
    ("b", "toggle a fake config error banner"),
    ("?", "this help"),
    ("Up / Down", "settings: move between rows"),
    ("Enter Right / Left", "settings: next / previous value"),
];

struct Palette {
    bg: Option<Color>,
    border: Color,
    title: Color,
    label: Color,
    value: Color,
    bar: Color,
    bar_empty: Color,
    spark: Color,
    rounded: bool,
}

fn palette(s: &Settings) -> Palette {
    if s.theme == MATRIX {
        let c = MATRIX_COLORS[s.color];
        Palette {
            bg: Some(rain::scale(c, 0.07)),
            border: rain::scale(c, 0.55),
            title: rain::mix_white(c, 0.2),
            label: rain::scale(c, 0.65),
            value: rain::mix_white(c, 0.55),
            bar: rain::scale(c, 0.9),
            bar_empty: rain::scale(c, 0.25),
            spark: rain::scale(c, 0.8),
            rounded: false,
        }
    } else {
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
}

struct Card {
    title: String,
    title_color: Option<Color>,
    lines: Vec<Line<'static>>,
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

fn fg(c: Color) -> Style {
    Style::new().fg(c)
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    if area.width < MIN_W || area.height < MIN_H {
        let y = area.y + area.height / 2;
        let rect = Rect::new(area.x, y, area.width, 1).intersection(area);
        frame.render_widget(Paragraph::new(TOO_SMALL).centered(), rect);
        return;
    }
    let matrix = app.settings.theme == MATRIX;
    if matrix {
        app.rain
            .fit(area.width, area.height, app.settings.density());
        let buf = frame.buffer_mut();
        buf.set_style(area, Style::new().bg(Color::Rgb(0, 0, 0)));
        app.rain
            .render(buf, area, MATRIX_COLORS[app.settings.color]);
    }
    let [banner, body, status] = Layout::vertical([
        Constraint::Length(u16::from(app.banner)),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    if app.banner {
        let style = Style::new().bg(Color::Rgb(135, 20, 20)).fg(Color::White);
        frame.render_widget(
            Paragraph::new(BANNER).style(style.add_modifier(Modifier::BOLD)),
            banner,
        );
    }
    draw_columns(frame, body, app, matrix);
    draw_status(frame, status, app);
    if let Some((text, _)) = &app.toast {
        draw_toast(frame, body, text);
    }
    match app.overlay {
        Overlay::None => {}
        Overlay::Settings => draw_settings(frame, area, app),
        Overlay::Log => draw_log(frame, area, &app.log),
        Overlay::Help => draw_help(frame, area),
    }
}

fn draw_columns(frame: &mut Frame, body: Rect, app: &App, matrix: bool) {
    let plan: &[&[Group]] = if body.width >= 96 {
        &[&[Group::System], &[Group::Disks], &[Group::Plugins]]
    } else if body.width >= 64 {
        &[&[Group::System, Group::Disks], &[Group::Plugins]]
    } else {
        &[&[Group::System, Group::Disks, Group::Plugins]]
    };
    let (margin, gap) = if matrix { (1, 1) } else { (0, 0) };
    let columns = Layout::horizontal(vec![Constraint::Fill(1); plan.len()])
        .spacing(if matrix { 2 } else { 1 })
        .horizontal_margin(margin * 2)
        .vertical_margin(margin)
        .split(body);
    let pal = palette(&app.settings);
    for (col, groups) in columns.iter().zip(plan) {
        let w = usize::from(col.width.saturating_sub(4));
        let cards = groups
            .iter()
            .flat_map(|g| match g {
                Group::System => system_cards(&app.data, &app.settings, &pal, w),
                Group::Disks => vec![disk_card(&app.data, &app.settings, &pal, w)],
                Group::Plugins => plugin_cards(&app.data, &app.settings, &pal, w),
            })
            .collect();
        stack(frame, *col, cards, &pal, gap);
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

fn kv(
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

fn bar(pct: f32, width: usize, pal: &Palette, warn: bool) -> Line<'static> {
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
    let glyphs: String = history
        .iter()
        .skip(skip)
        .map(|v| SPARK[((v / 100.0).clamp(0.0, 1.0) * 7.0).round() as usize])
        .collect();
    Line::styled(format!("{glyphs:>width$}"), fg(color))
}

fn used_of(used: u64, total: u64, decimal: bool) -> String {
    format!(
        "{} / {}",
        data::fmt_bytes(used, decimal),
        data::fmt_bytes(total, decimal)
    )
}

fn system_cards(d: &FakeData, s: &Settings, pal: &Palette, w: usize) -> Vec<Card> {
    let cpu_warn = d.cpu > s.cpu_warn();
    let temp_color = if d.temp_c > TEMP_WARN_C {
        WARN
    } else {
        pal.value
    };
    let mut cpu = vec![
        bar(d.cpu, w, pal, cpu_warn),
        kv(
            "temp",
            data::fmt_temp(d.temp_c, s.fahrenheit),
            w,
            pal,
            temp_color,
        ),
    ];
    let mut ram = vec![
        kv(
            "used",
            used_of(d.ram_used, d.ram_total, s.decimal),
            w,
            pal,
            pal.value,
        ),
        bar(d.ram_pct(), w, pal, false),
    ];
    if s.sparklines {
        cpu.push(spark(
            &d.cpu_history,
            w,
            if cpu_warn { WARN } else { pal.spark },
        ));
        ram.push(spark(&d.ram_history, w, pal.spark));
    }
    let swap = vec![
        kv(
            "used",
            used_of(d.swap_used, d.swap_total, s.decimal),
            w,
            pal,
            pal.value,
        ),
        bar(data::pct(d.swap_used, d.swap_total), w, pal, false),
    ];
    vec![
        Card::new("CPU", cpu),
        Card::new("RAM", ram),
        Card::new("Swap", swap),
    ]
}

fn disk_card(d: &FakeData, s: &Settings, pal: &Palette, w: usize) -> Card {
    let mut lines = Vec::new();
    for disk in &d.disks {
        let pct = data::pct(disk.used, disk.total);
        lines.push(kv(
            disk.mount,
            used_of(disk.used, disk.total, s.decimal),
            w,
            pal,
            pal.value,
        ));
        lines.push(bar(pct, w, pal, pct > s.disk_warn()));
    }
    Card::new("Disks", lines)
}

fn plugin_cards(d: &FakeData, s: &Settings, pal: &Palette, w: usize) -> Vec<Card> {
    let mut cards: Vec<Card> = PLUGINS
        .iter()
        .zip(s.plugins)
        .filter(|(_, on)| *on)
        .map(|(id, _)| plugin_card(id, d, s, pal, w))
        .collect();
    if cards.is_empty() {
        let hint = Line::styled("all plugins are off (s to enable)", fg(pal.label));
        cards.push(Card::new("Plugins", vec![hint]));
    }
    cards
}

fn plugin_card(id: &str, d: &FakeData, s: &Settings, pal: &Palette, w: usize) -> Card {
    let v = pal.value;
    match id {
        "weather" => Card::new(
            "Weather · Kyiv",
            vec![
                kv("temp", data::fmt_temp(d.weather_c, s.fahrenheit), w, pal, v),
                kv("wind", format!("{:.1} m/s", d.wind_ms), w, pal, v),
                kv("direction", d.wind_dir().to_string(), w, pal, v),
            ],
        ),
        "crypto" => Card::new(
            "Crypto",
            vec![
                kv("BTC", data::fmt_money(d.btc), w, pal, v),
                kv("ETH", data::fmt_money(d.eth), w, pal, v),
                kv("SOL", data::fmt_money(d.sol), w, pal, v),
            ],
        ),
        "network_ping" => Card::new(
            "Internet Latency",
            vec![kv(
                "1.1.1.1:443",
                format!("{:.0} ms", d.latency_ms),
                w,
                pal,
                v,
            )],
        ),
        "clock" => Card::new(
            "Clock",
            vec![
                kv("time (UTC)", data::fmt_hms(d.now), w, pal, v),
                kv("date", data::fmt_date(d.now), w, pal, v),
            ],
        ),
        "uptime" => Card::new(
            "Uptime",
            vec![kv("up", data::fmt_uptime(d.uptime_s), w, pal, v)],
        ),
        _ => Card {
            title: id.to_string(),
            title_color: Some(WARN),
            lines: wrap("Error: attempt to index a nil value (line 12)", w)
                .into_iter()
                .map(|l| Line::styled(l, fg(WARN)))
                .collect(),
        },
    }
}

fn wrap(text: &str, width: usize) -> Vec<String> {
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

fn draw_status(frame: &mut Frame, area: Rect, app: &App) {
    let paused = if app.paused { " · PAUSED" } else { "" };
    let right = format!(
        " {} · {} fps{paused} ",
        app.settings.theme_name(),
        app.settings.fps()
    );
    let right_w = u16::try_from(right.chars().count()).unwrap_or(u16::MAX);
    let [left_area, right_area] =
        Layout::horizontal([Constraint::Fill(1), Constraint::Length(right_w)]).areas(area);
    let base = Style::new()
        .bg(Color::Rgb(38, 38, 38))
        .fg(Color::Rgb(190, 190, 190));
    let left = Line::from(vec![
        Span::styled(
            " MOCKUP ",
            Style::new()
                .bg(ACCENT)
                .fg(Color::Black)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" · fake data · t theme · s settings · l log · ? help · q quit"),
    ]);
    frame.render_widget(Paragraph::new(left).style(base), left_area);
    frame.render_widget(
        Paragraph::new(right).style(base.fg(Color::White).add_modifier(Modifier::BOLD)),
        right_area,
    );
}

fn draw_toast(frame: &mut Frame, body: Rect, text: &str) {
    let w = u16::try_from(text.chars().count() + 4)
        .unwrap_or(u16::MAX)
        .min(body.width);
    let y = body.bottom().saturating_sub(4).max(body.y);
    let rect = Rect::new(body.x + (body.width - w) / 2, y, w, 3).intersection(body);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(ACCENT))
        .style(Style::new().bg(OVERLAY_BG).fg(Color::White));
    frame.render_widget(Clear, rect);
    frame.render_widget(Paragraph::new(text).centered().block(block), rect);
}

fn popup(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// Clears `rect`, draws the overlay frame, returns the inner area.
fn overlay_frame(frame: &mut Frame, rect: Rect, title: &str) -> Rect {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(fg(MUTED))
        .title(Span::styled(
            format!(" {title} "),
            fg(Color::White).add_modifier(Modifier::BOLD),
        ))
        .style(Style::new().bg(OVERLAY_BG).fg(OVERLAY_FG));
    let inner = block.inner(rect);
    frame.render_widget(Clear, rect);
    frame.render_widget(block, rect);
    inner
}

fn draw_settings(frame: &mut Frame, area: Rect, app: &App) {
    let rows = settings::rows();
    let sel = settings::selectable(&rows);
    let current = sel[app.cursor.min(sel.len() - 1)];
    let height = u16::try_from(rows.len() + 5).unwrap_or(u16::MAX);
    let inner = overlay_frame(frame, popup(area, 62, height), "settings");
    let [list, footer] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(2)]).areas(inner);
    let visible = usize::from(list.height).max(1);
    let offset = (current + 1).saturating_sub(visible);
    let width = usize::from(list.width);
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(i, row)| row_line(row, i == current, &app.settings, width))
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
    let footer_lines = vec![
        Line::styled(
            "Up/Down move · Enter/Right next · Left previous · Esc close",
            fg(MUTED),
        ),
        Line::styled(
            "mockup: changes apply live, nothing is saved",
            fg(ACCENT).add_modifier(Modifier::ITALIC),
        ),
    ];
    frame.render_widget(Paragraph::new(footer_lines), footer);
}

fn row_line(row: &Row, selected: bool, s: &Settings, width: usize) -> Line<'static> {
    let highlight = Style::new().bg(Color::Rgb(62, 62, 62)).fg(Color::White);
    match row {
        Row::Section(name) => {
            Line::styled(format!("[{name}]"), fg(ACCENT).add_modifier(Modifier::BOLD))
        }
        Row::Edit(field) => {
            let value = s.value_text(*field);
            let value = if selected {
                format!("< {value} >")
            } else {
                format!("  {value}")
            };
            let text = format!("  {:<18}{value}", settings::key(*field));
            if selected {
                Line::styled(
                    format!("{text:<width$}"),
                    highlight.add_modifier(Modifier::BOLD),
                )
            } else {
                Line::raw(text)
            }
        }
        Row::Fixed { key, value } => {
            let text = format!("  {key:<18}  {value:<12}edit in file");
            let style = if selected {
                highlight.fg(MUTED)
            } else {
                fg(MUTED)
            };
            Line::styled(format!("{text:<width$}"), style)
        }
    }
}

fn draw_log(frame: &mut Frame, area: Rect, log: &VecDeque<crate::LogLine>) {
    let inner = overlay_frame(frame, popup(area, 92, 22), "log (newest at the bottom)");
    let skip = log.len().saturating_sub(usize::from(inner.height));
    let lines: Vec<Line> = log
        .iter()
        .skip(skip)
        .map(|l| {
            let (tag, color) = match l.level {
                Level::Info => ("INFO ", MUTED),
                Level::Warn => ("WARN ", ACCENT),
                Level::Error => ("ERROR", WARN),
            };
            Line::from(vec![
                Span::styled(format!("{} ", l.time), fg(MUTED)),
                Span::styled(tag, fg(color).add_modifier(Modifier::BOLD)),
                Span::raw(format!(" {}", l.text)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_help(frame: &mut Frame, area: Rect) {
    let height = u16::try_from(HELP.len() + 2).unwrap_or(u16::MAX);
    let inner = overlay_frame(frame, popup(area, 72, height), "keys");
    let lines: Vec<Line> = HELP
        .iter()
        .map(|(keys, what)| {
            Line::from(vec![
                Span::styled(
                    format!("  {keys:<20}"),
                    fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::raw(*what),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}
