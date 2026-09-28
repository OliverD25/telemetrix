use std::collections::VecDeque;
use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::Borders;

use super::Theme;
use super::common::{self, Card, Gauge, Group, Palette, fg};
use crate::app::AppState;
use crate::config::Config;
use crate::format;
use crate::metrics;

pub const SCREEN: Color = Color::Rgb(16, 18, 24);
pub const LOW: Color = Color::Rgb(110, 210, 120);
pub const MID: Color = Color::Rgb(235, 200, 80);
pub const HIGH: Color = Color::Rgb(255, 95, 95);
/// Bottom-aligned blocks from one eighth to a full cell.
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
/// `100┤`: the scale left of a chart.
const AXIS_W: usize = 4;
const MIN_CHART_ROWS: usize = 2;
const MAX_CHART_ROWS: usize = 10;

pub fn palette() -> Palette {
    Palette {
        bg: Some(Color::Rgb(22, 25, 33)),
        border: Color::Rgb(70, 78, 96),
        title: Color::Rgb(120, 200, 255),
        label: Color::Rgb(140, 150, 170),
        value: Color::Rgb(230, 232, 238),
        bar: Color::Rgb(90, 200, 255),
        bar_empty: Color::Rgb(62, 68, 84),
        spark: Color::Rgb(120, 220, 160),
        border_set: border::PLAIN,
        brackets: ("┤ ", " ├"),
        warn: HIGH,
        rise: LOW,
        muted: Color::Rgb(100, 106, 122),
        gauge: Gauge {
            full: '█',
            empty: '·',
            ends: ("", ""),
            eighths: true,
        },
        borders: Borders::ALL,
    }
}

/// Green up to half, yellow up to 80 %, red above: the colour of a chart row
/// whose top edge stands for `top_pct`.
fn level_color(top_pct: f32) -> Color {
    if top_pct <= 50.0 {
        LOW
    } else if top_pct <= 80.0 {
        MID
    } else {
        HIGH
    }
}

/// A multi-row bar chart, top row first: the last samples (0..100) that fit
/// in `width`, right-aligned, each column as tall as its value in eighths of
/// a row, with a 0..100 scale on the left.
pub fn chart(history: &VecDeque<f32>, width: usize, height: usize) -> Vec<Line<'static>> {
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
                    BLOCKS[eighths - 1]
                });
            }
            let axis = match row {
                0 => "100┤",
                r if r == height - 1 => "  0┤",
                r if height >= 5 && r == height / 2 => " 50┤",
                _ => "   │",
            };
            let top_pct = (height - row) as f32 / height as f32 * 100.0;
            Line::from(vec![
                Span::styled(axis, fg(palette().muted)),
                Span::styled(glyphs, fg(level_color(top_pct))),
            ])
        })
        .collect()
}

/// `avg 26%  peak 35%` over the samples a chart of this width shows.
fn summary(history: &VecDeque<f32>, width: usize) -> String {
    let bars = width.saturating_sub(AXIS_W).max(1);
    let shown: Vec<f32> = history.iter().rev().take(bars).copied().collect();
    if shown.is_empty() {
        return String::new();
    }
    let avg = shown.iter().sum::<f32>() / shown.len() as f32;
    let peak = shown.iter().copied().fold(0.0, f32::max);
    format!("avg {avg:.0}%  peak {peak:.0}%")
}

fn system_cards(state: &AppState, pal: &Palette, w: usize, rows: usize) -> Vec<Card> {
    let Some(s) = &state.snapshot else {
        return common::system_cards(state, pal, w);
    };
    let cfg = &state.config;
    let hot = s.cpu_usage > cfg.thresholds.cpu_warn_pct;
    let mut cpu = chart(&state.cpu_history, w, rows);
    let temp = s.cpu_temp.map_or(String::new(), |t| {
        format::temperature(t, cfg.units.temperature)
    });
    let temp_color = match s.cpu_temp {
        Some(t) if t > cfg.thresholds.temp_warn_c => pal.warn,
        _ => pal.value,
    };
    let avg = summary(&state.cpu_history, w);
    cpu.push(common::kv(&avg, temp, w, pal, temp_color));
    let mut ram = chart(&state.ram_history, w, rows);
    let used = common::used_of(s.ram_used_bytes, s.ram_total_bytes, state);
    ram.push(common::kv("used", used, w, pal, pal.value));
    let swap_used = common::used_of(s.swap_used_bytes, s.swap_total_bytes, state);
    let swap_pct = metrics::pct(s.swap_used_bytes, s.swap_total_bytes);
    let swap = vec![
        common::kv("used", swap_used, w, pal, pal.value),
        common::bar(swap_pct, w, pal, false),
    ];
    let mut cpu_card = Card::new(format!("CPU {:.0}%", s.cpu_usage), cpu);
    if hot {
        cpu_card.title_color = Some(pal.warn);
    }
    vec![
        cpu_card,
        Card::new(format!("RAM {:.0}%", s.ram_pct()), ram),
        Card::new("Swap", swap),
    ]
}

/// Chart rows that let the column with the system cards fit the screen:
/// what is left after the other cards, split between the CPU and RAM charts.
pub fn chart_rows(body: Rect, state: &AppState, pal: &Palette) -> usize {
    let columns = common::columns(body, true);
    let Some((col, groups)) = columns.iter().find(|(_, g)| g.contains(&Group::System)) else {
        return MIN_CHART_ROWS;
    };
    let w = common::text_width(*col, pal);
    // CPU and RAM: a summary line and two frame lines each; Swap: four rows.
    let mut fixed = 3 + 3 + 4;
    let mut cards = 3;
    for g in groups.iter().filter(|g| **g != Group::System) {
        for card in common::group_cards(*g, state, pal, w) {
            fixed += common::card_height(&card, pal);
            cards += 1;
        }
    }
    let gaps = cards - 1;
    let free = usize::from(col.height).saturating_sub(fixed + gaps);
    (free / 2).clamp(MIN_CHART_ROWS, MAX_CHART_ROWS)
}

/// Block-glyph charts of the CPU and RAM history and fine horizontal bars
/// for the rest. Static: redraws only when data changes.
pub struct AsciiDashboard;

impl Theme for AsciiDashboard {
    fn name(&self) -> &'static str {
        "ascii-dashboard"
    }

    fn frame_interval(&self, _cfg: &Config) -> Option<Duration> {
        None
    }

    fn draw(&mut self, frame: &mut Frame, state: &AppState) {
        common::fill_screen(frame, SCREEN);
        let pal = palette();
        let body = common::body_area(frame.area(), state);
        let rows = chart_rows(body, state, &pal);
        common::draw_columns_with(frame, body, &pal, true, |g, w| match g {
            Group::System => system_cards(state, &pal, w, rows),
            _ => common::group_cards(g, state, &pal, w),
        });
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

    fn glyphs(line: &Line) -> String {
        line.spans[1].content.to_string()
    }

    #[test]
    fn charts_grow_in_eighths_and_colour_by_height() {
        let history: VecDeque<f32> = [0.0, 25.0, 50.0, 100.0].into_iter().collect();
        let rows = chart(&history, AXIS_W + 5, 2);
        assert_eq!(glyphs(&rows[0]), "    █");
        assert_eq!(glyphs(&rows[1]), "  ▄██");
        assert_eq!(rows[0].spans[0].content, "100┤");
        assert_eq!(rows[1].spans[0].content, "  0┤");
        assert_eq!(rows[0].spans[1].style.fg, Some(HIGH));
        assert_eq!(rows[1].spans[1].style.fg, Some(LOW));
        let tall = chart(&history, AXIS_W + 4, 5);
        assert_eq!(tall[2].spans[0].content, " 50┤");
        assert_eq!(tall[1].spans[1].style.fg, Some(MID));
        let long: VecDeque<f32> = (0..300).map(|i| (i % 100) as f32).collect();
        assert_eq!(glyphs(&chart(&long, 30, 3)[2]).chars().count(), 26);
    }

    #[test]
    fn big_history_charts_and_fine_bars() {
        let buf = demo::render(&demo::state("ascii-dashboard"), 100, 30);
        let t = text(&buf);
        for needle in [
            "┌┤ CPU 23% ├─",
            "┌┤ RAM 29% ├─",
            "100┤",
            "  0┤",
            "avg ",
            "peak 35%",
            "52.0 °C",
            "┌┤ Disks ├─",
            "·   44%",
            "NVIDIA GeForce",
            "84,853 USDT",
            "+2.1%",
            "7d ",
        ] {
            assert!(t.contains(needle), "{needle}");
        }
        assert!(
            ['▏', '▎', '▍', '▌', '▋', '▊', '▉']
                .iter()
                .any(|c| t.contains(*c)),
            "a bar ends in a part of a cell"
        );
        assert_eq!(buf[(0, 0)].bg, SCREEN);
    }

    #[test]
    fn charts_take_the_room_the_column_has() {
        let s = demo::state("ascii-dashboard");
        let pal = palette();
        let small = chart_rows(Rect::new(0, 0, 100, 29), &s, &pal);
        let large = chart_rows(Rect::new(0, 0, 120, 39), &s, &pal);
        assert_eq!(small, 3);
        assert!(large > small, "{large}");
        for (w, h) in [(100, 30), (120, 40), (80, 40)] {
            let t = text(&demo::render(&s, w, h));
            assert!(
                t.contains("power") && t.contains("112.4 W"),
                "{w}x{h}: the whole GPU card"
            );
        }
    }

    #[test]
    fn warnings_and_small_screens() {
        let mut s = demo::state("ascii-dashboard");
        let mut snap = demo::snapshot();
        snap.cpu_usage = 100.0;
        s.snapshot = Some(snap);
        let buf = demo::render(&s, 100, 30);
        let t = text(&buf);
        let pos = t.find("CPU 100%").map(|b| t[..b].chars().count()).unwrap();
        assert_eq!(buf.content()[pos].fg, HIGH, "the CPU title turns red");
        let t = text(&demo::render(&demo::state("ascii-dashboard"), 40, 10));
        assert!(t.contains(" CPU ") && t.contains(" telemetrix "), "{t}");
        assert!(!t.contains(TOO_SMALL));
        let t = text(&demo::render(&demo::state("ascii-dashboard"), 30, 8));
        assert!(t.contains("terminal too small"), "{t}");
        assert_eq!(AsciiDashboard.frame_interval(&Config::default()), None);
    }
}
