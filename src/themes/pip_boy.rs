use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Borders, Paragraph};

use super::Theme;
use super::common::{self, Gauge, Palette, fg, mix_white, scale};
use crate::app::AppState;
use crate::config::Config;

/// The one phosphor hue; every colour of the theme is a brightness of it.
pub const GREEN: (u8, u8, u8) = (26, 255, 128);
pub const SCREEN: Color = Color::Rgb(1, 12, 6);
const TABS: [&str; 5] = ["STAT", "INV", "DATA", "MAP", "RADIO"];

/// Plain ASCII frames, like a text terminal from before box-drawing fonts.
const FRAME: border::Set<'static> = border::Set {
    top_left: "+",
    top_right: "+",
    bottom_left: "+",
    bottom_right: "+",
    vertical_left: "|",
    vertical_right: "|",
    horizontal_top: "-",
    horizontal_bottom: "-",
};

pub fn palette() -> Palette {
    Palette {
        bg: Some(scale(GREEN, 0.09)),
        border: scale(GREEN, 0.6),
        title: mix_white(GREEN, 0.35),
        label: scale(GREEN, 0.62),
        value: scale(GREEN, 0.95),
        bar: scale(GREEN, 0.9),
        bar_empty: scale(GREEN, 0.3),
        spark: scale(GREEN, 0.8),
        border_set: FRAME,
        brackets: ("[ ", " ]"),
        // One hue only, so a warning is the brightest shade, close to white.
        warn: mix_white(GREEN, 0.75),
        rise: Color::Rgb(GREEN.0, GREEN.1, GREEN.2),
        muted: scale(GREEN, 0.42),
        gauge: Gauge {
            full: '▮',
            empty: '▯',
            ends: ("[", "]"),
            eighths: false,
        },
        borders: Borders::ALL,
    }
}

/// `[STAT] [INV] [DATA] [MAP] [RADIO]` with STAT lit, as on the wrist unit.
fn tabs() -> Line<'static> {
    let mut spans = vec![Span::raw(" ")];
    for (i, tab) in TABS.iter().enumerate() {
        let style = if i == 0 {
            Style::new()
                .bg(mix_white(GREEN, 0.2))
                .fg(SCREEN)
                .add_modifier(Modifier::BOLD)
        } else {
            fg(scale(GREEN, 0.55))
        };
        spans.push(Span::styled(format!("[{tab}]"), style));
        spans.push(Span::raw(" "));
    }
    Line::from(spans)
}

/// A monochrome green wrist-computer screen: ASCII frames, bracketed tabs
/// and `[▮▮▮▯▯]` gauges. Static: redraws only when data changes.
pub struct PipBoy;

impl Theme for PipBoy {
    fn name(&self) -> &'static str {
        "pip-boy"
    }

    fn frame_interval(&self, _cfg: &Config) -> Option<Duration> {
        None
    }

    fn draw(&mut self, frame: &mut Frame, state: &AppState) {
        common::fill_screen(frame, SCREEN);
        let body = common::body_area(frame.area(), state);
        // The tabs sit in the empty margin row above the cards.
        let top = Rect { height: 1, ..body };
        frame.render_widget(Paragraph::new(tabs()), top);
        common::draw_columns(frame, body, state, &palette(), true);
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

    /// Scaling and mixing with white keep the channel order of `GREEN`.
    fn is_green(c: Color) -> bool {
        match c {
            Color::Rgb(r, g, b) => g >= b && b >= r,
            _ => true,
        }
    }

    #[test]
    fn tabs_ascii_frames_and_bracket_gauges() {
        let buf = demo::render(&demo::state("pip-boy"), 100, 30);
        let t = text(&buf);
        for needle in [
            "[STAT] [INV] [DATA] [MAP] [RADIO]",
            "+[ CPU ]---",
            "+[ Crypto ]-",
            "| [▮▮▮▮▯",
            "▯]   23%",
            "84,853 USDT",
            "+2.1%",
            "NVIDIA GeForce",
        ] {
            assert!(t.contains(needle), "{needle}");
        }
        assert!(
            !t.contains('│') && !t.contains('─'),
            "no box-drawing frames"
        );
        assert_eq!(buf[(1, 0)].bg, mix_white(GREEN, 0.2), "STAT is lit");
        assert_eq!(buf[(0, 5)].bg, SCREEN);
    }

    #[test]
    fn every_colour_is_one_green_hue() {
        let mut s = demo::state("pip-boy");
        let mut snap = demo::snapshot();
        snap.cpu_usage = 100.0;
        s.snapshot = Some(snap);
        let buf = demo::render(&s, 100, 30);
        let area = buf.area;
        // The last row is the shared status bar, grey in every theme.
        for y in 0..area.height - 1 {
            for x in 0..area.width {
                let c = &buf[(x, y)];
                assert!(is_green(c.fg) && is_green(c.bg), "{x},{y}: {c:?}");
            }
        }
        let warn = palette().warn;
        assert!(
            buf.content()
                .iter()
                .any(|c| c.symbol() == "▮" && c.fg == warn),
            "a full CPU gauge in the brightest shade"
        );
    }

    #[test]
    fn small_screens() {
        let t = text(&demo::render(&demo::state("pip-boy"), 40, 10));
        assert!(t.contains("[ CPU ]") && t.contains(" telemetrix "), "{t}");
        assert!(!t.contains(TOO_SMALL));
        let t = text(&demo::render(&demo::state("pip-boy"), 30, 8));
        assert!(t.contains("terminal too small"), "{t}");
        assert_eq!(PipBoy.frame_interval(&Config::default()), None);
    }
}
