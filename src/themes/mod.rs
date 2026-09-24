pub mod common;
pub mod matrix;
pub mod minimalist;

use std::time::Duration;

use ratatui::Frame;
use ratatui::layout::Rect;

use crate::app::AppState;
use crate::config::Config;

pub trait Theme {
    fn name(&self) -> &'static str;
    /// `None` = static: redraw only when data changes. `Some(d)` = animated.
    fn frame_interval(&self, cfg: &Config) -> Option<Duration>;
    fn tick(&mut self, _area: Rect, _cfg: &Config) {}
    fn draw(&mut self, frame: &mut Frame, state: &AppState);
}

/// Same order as `config::THEME_NAMES`, so a theme index means the same in both.
pub fn all() -> Vec<Box<dyn Theme>> {
    vec![
        Box::new(minimalist::Minimalist),
        Box::new(matrix::Matrix::new()),
    ]
}

pub fn index_of(name: &str) -> Option<usize> {
    all().iter().position(|t| t.name() == name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::THEME_NAMES;

    #[test]
    fn registry_matches_the_config_enum() {
        let names: Vec<&str> = all().iter().map(|t| t.name()).collect();
        assert_eq!(names, THEME_NAMES);
        assert_eq!(index_of("matrix"), Some(1));
        assert_eq!(index_of("neon"), None);
    }
}
