use std::time::Duration;

use ratatui::Frame;
use ratatui::style::Color;
use ratatui::symbols::border;
use ratatui::widgets::Borders;

use super::Theme;
use super::common::{self, Palette};
use crate::app::AppState;
use crate::config::Config;

/// The Tokyo Night colour scheme: dark slate, quiet blues and purples.
pub const SCREEN: Color = Color::Rgb(0x1a, 0x1b, 0x26);

pub fn palette() -> Palette {
    Palette {
        bg: Some(Color::Rgb(0x1f, 0x23, 0x35)),
        border: Color::Rgb(0x3b, 0x42, 0x61),
        title: Color::Rgb(0xbb, 0x9a, 0xf7),
        label: Color::Rgb(0x73, 0x7a, 0xa2),
        value: Color::Rgb(0xc0, 0xca, 0xf5),
        bar: Color::Rgb(0x7a, 0xa2, 0xf7),
        bar_empty: Color::Rgb(0x29, 0x2e, 0x42),
        spark: Color::Rgb(0x7d, 0xcf, 0xff),
        border_set: border::ROUNDED,
        brackets: (" ", " "),
        warn: Color::Rgb(0xf7, 0x76, 0x8e),
        rise: Color::Rgb(0x9e, 0xce, 0x6a),
        muted: Color::Rgb(0x56, 0x5f, 0x89),
        gauge: common::BLOCK_GAUGE,
        borders: Borders::ALL,
    }
}

/// Static: redraws only when data changes.
pub struct TokyoNight;

impl Theme for TokyoNight {
    fn name(&self) -> &'static str {
        "tokyo-night"
    }

    fn frame_interval(&self, _cfg: &Config) -> Option<Duration> {
        None
    }

    fn draw(&mut self, frame: &mut Frame, state: &AppState) {
        common::fill_screen(frame, SCREEN);
        let body = common::body_area(frame.area(), state);
        common::draw_columns(frame, body, state, &palette(), true);
    }
}
