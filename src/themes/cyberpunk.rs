use std::time::Duration;

use ratatui::Frame;
use ratatui::style::Color;
use ratatui::symbols::border;
use ratatui::widgets::Borders;

use super::Theme;
use super::common::{self, Palette};
use crate::app::AppState;
use crate::config::Config;

pub const SCREEN: Color = Color::Rgb(9, 3, 18);
pub const MAGENTA: Color = Color::Rgb(255, 0, 200);
const CYAN: Color = Color::Rgb(0, 240, 255);
const YELLOW: Color = Color::Rgb(255, 230, 0);

/// Double lines across, single lines down: a HUD frame.
const HUD: border::Set<'static> = border::Set {
    top_left: "╒",
    top_right: "╕",
    bottom_left: "╘",
    bottom_right: "╛",
    vertical_left: "│",
    vertical_right: "│",
    horizontal_top: "═",
    horizontal_bottom: "═",
};

pub fn palette() -> Palette {
    Palette {
        bg: Some(Color::Rgb(16, 5, 32)),
        border: CYAN,
        title: MAGENTA,
        label: Color::Rgb(0, 170, 190),
        value: YELLOW,
        bar: MAGENTA,
        bar_empty: Color::Rgb(70, 0, 60),
        spark: CYAN,
        border_set: HUD,
        brackets: ("[ ", " ]"),
        warn: Color::Rgb(255, 50, 80),
        rise: Color::Rgb(0, 255, 150),
        muted: Color::Rgb(125, 95, 165),
        gauge: common::BLOCK_GAUGE,
        borders: Borders::ALL,
        levels: [CYAN, YELLOW, Color::Rgb(255, 50, 80)],
    }
}

/// Magenta, cyan and yellow on near-black, card titles in brackets.
/// Static: redraws only when data changes.
pub struct Cyberpunk;

impl Theme for Cyberpunk {
    fn name(&self) -> &'static str {
        "cyberpunk"
    }

    fn frame_interval(&self, _cfg: &Config) -> Option<Duration> {
        None
    }

    fn draw(&mut self, frame: &mut Frame, state: &AppState) {
        common::fill_screen(frame, SCREEN);
        let body = common::body_area(frame.area(), state);
        common::draw_columns(
            frame,
            body,
            state,
            &common::accented(palette(), state),
            true,
        );
    }
}
