use std::time::Duration;

use ratatui::Frame;

use super::Theme;
use super::common;
use crate::app::AppState;
use crate::config::Config;

/// Greys everywhere, colour only above a threshold. Static: no frame timer.
pub struct Minimalist;

impl Theme for Minimalist {
    fn name(&self) -> &'static str {
        "minimalist"
    }

    fn frame_interval(&self, _cfg: &Config) -> Option<Duration> {
        None
    }

    fn draw(&mut self, frame: &mut Frame, state: &AppState) {
        let body = common::body_area(frame.area(), state);
        common::draw_columns(frame, body, state, &common::minimalist_palette(), false);
    }
}
