#[cfg(test)]
use std::collections::VecDeque;
use std::time::Duration;

use ratatui::Frame;
#[cfg(test)]
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::symbols::border;
#[cfg(test)]
use ratatui::text::Line;
use ratatui::widgets::Borders;

use super::Theme;
use super::common::{self, Gauge, Palette};
use crate::app::AppState;
use crate::config::Config;

pub const SCREEN: Color = Color::Rgb(16, 18, 24);
pub const LOW: Color = Color::Rgb(110, 210, 120);
pub const MID: Color = Color::Rgb(235, 200, 80);
pub const HIGH: Color = Color::Rgb(255, 95, 95);
#[cfg(test)]
const AXIS_W: usize = common::AXIS_W;

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
        // Green up to half, yellow up to 80 %, red above.
        levels: [LOW, MID, HIGH],
    }
}

/// The history chart in this theme's colours.
#[cfg(test)]
pub fn chart(history: &VecDeque<f32>, width: usize, height: usize) -> Vec<Line<'static>> {
    common::chart(history, width, height, &palette())
}

/// The chart rows the CPU and RAM cards get in `body`.
#[cfg(test)]
pub fn chart_rows(body: Rect, state: &AppState, pal: &Palette) -> usize {
    let opts = common::options(state);
    let cols = common::columns(body, true);
    let mut make = |g, w| common::group_cards(g, state, pal, w, common::MIN_CHART_ROWS);
    let placed = common::place(&cols, opts, pal, 1, &mut make);
    common::chart_rows(&cols, &placed, opts, pal, 1)
}

/// Block-glyph charts of the CPU and RAM history (its default `cpu_view`
/// and `ram_view`) and fine horizontal bars for the rest. Static: redraws
/// only when data changes.
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
        let pal = common::accented(palette(), state);
        let body = common::body_area(frame.area(), state);
        common::draw_columns(frame, body, state, &pal, true);
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
