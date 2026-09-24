//! The `t` box: pick a theme with a live preview, in a corner so the
//! dashboard stays visible behind it.

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::app::AppState;
use crate::config::THEME_NAMES;
use crate::themes::common::{MUTED, fg};

pub const HINT: &str = "↑↓ preview · Enter save · Esc cancel";
const WIDTH: u16 = 40;

pub fn draw(frame: &mut Frame, area: Rect, state: &AppState) {
    let height = u16::try_from(THEME_NAMES.len() + 4).unwrap_or(u16::MAX);
    let top = area.y + u16::from(state.banner().is_some()) + 1;
    let w = WIDTH.min(area.width.saturating_sub(2));
    let rect = Rect::new(area.right().saturating_sub(w + 1), top, w, height).intersection(area);
    let inner = super::overlay_frame(frame, rect, "Themes");
    let width = usize::from(inner.width);
    let mut lines: Vec<Line> = THEME_NAMES
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let saved = if i == state.picker_original {
                "  (saved)"
            } else {
                ""
            };
            if i == state.theme_idx {
                let text = format!("› {name}{saved}");
                let style = Style::new()
                    .bg(Color::Rgb(62, 62, 62))
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD);
                Line::styled(format!("{text:<width$}"), style)
            } else {
                Line::raw(format!("  {name}{saved}"))
            }
        })
        .collect();
    lines.push(Line::raw(""));
    lines.push(Line::styled(HINT, fg(MUTED)));
    frame.render_widget(Paragraph::new(lines), inner);
}
