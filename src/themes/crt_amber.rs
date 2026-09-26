use std::time::Duration;

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::symbols::border;

use super::Theme;
use super::common::{self, Palette, scale};
use crate::app::AppState;
use crate::config::Config;

const AMBER: (u8, u8, u8) = (255, 176, 0);
pub const SCREEN: Color = Color::Rgb(14, 8, 0);
/// How much darker every second row is: the scanlines.
const SCANLINE: f32 = 0.55;

pub fn palette() -> Palette {
    Palette {
        bg: Some(Color::Rgb(24, 14, 0)),
        border: scale(AMBER, 0.75),
        title: Color::Rgb(255, 214, 110),
        label: scale(AMBER, 0.62),
        value: Color::Rgb(255, 196, 70),
        bar: scale(AMBER, 0.95),
        bar_empty: scale(AMBER, 0.22),
        spark: scale(AMBER, 0.8),
        border_set: border::THICK,
        brackets: (" ", " "),
        // A monochrome screen still needs "too high" and "falling" to stand out.
        warn: Color::Rgb(255, 84, 32),
        rise: Color::Rgb(255, 232, 160),
        muted: scale(AMBER, 0.42),
    }
}

/// Darkens the background of every second row, like the gaps between the
/// lines of an old monitor. Only colours set as RGB change.
pub fn scanlines(buf: &mut Buffer, area: Rect) {
    for y in (area.y..area.bottom()).skip(1).step_by(2) {
        for x in area.x..area.right() {
            let cell = &mut buf[(x, y)];
            if let Color::Rgb(r, g, b) = cell.bg {
                cell.set_bg(scale((r, g, b), SCANLINE));
            }
        }
    }
}

/// An amber monochrome monitor with heavy frames and scanlines. Static:
/// the scanlines do not move, so it redraws only when data changes.
pub struct CrtAmber;

impl Theme for CrtAmber {
    fn name(&self) -> &'static str {
        "crt-amber"
    }

    fn frame_interval(&self, _cfg: &Config) -> Option<Duration> {
        None
    }

    fn draw(&mut self, frame: &mut Frame, state: &AppState) {
        common::fill_screen(frame, SCREEN);
        let area = frame.area();
        let body = common::body_area(area, state);
        common::draw_columns(frame, body, state, &palette(), true);
        scanlines(frame.buffer_mut(), area);
    }
}
