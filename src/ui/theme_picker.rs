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

/// The first name shown when only `room` of `len` names fit: the
/// selected one always stays in view.
pub fn first_visible(selected: usize, len: usize, room: usize) -> usize {
    if room == 0 || len <= room {
        return 0;
    }
    selected.saturating_sub(room - 1).min(len - room)
}

pub fn draw(frame: &mut Frame, area: Rect, state: &AppState) {
    let top = area.y + u16::from(state.banner().is_some()) + 1;
    // Never over the status bar in the last row.
    let room = area.bottom().saturating_sub(top + 1);
    let wanted = u16::try_from(THEME_NAMES.len() + 4).unwrap_or(u16::MAX);
    let height = wanted.min(room);
    let w = WIDTH.min(area.width.saturating_sub(2));
    let rect = Rect::new(area.right().saturating_sub(w + 1), top, w, height).intersection(area);
    let inner = super::overlay_frame(frame, rect, "Themes");
    let width = usize::from(inner.width);
    let rows = usize::from(inner.height);
    let first = first_visible(state.theme_idx, THEME_NAMES.len(), rows);
    let mut lines: Vec<Line> = THEME_NAMES
        .iter()
        .enumerate()
        .skip(first)
        .take(rows)
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
    if rows > THEME_NAMES.len() + 1 {
        lines.push(Line::raw(""));
        lines.push(Line::styled(HINT, fg(MUTED)));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_selected_theme_stays_in_view() {
        assert_eq!(first_visible(0, 5, 9), 0, "all fit");
        assert_eq!(first_visible(4, 5, 3), 2);
        assert_eq!(first_visible(1, 5, 3), 0);
        assert_eq!(first_visible(3, 5, 3), 1);
        assert_eq!(first_visible(3, 5, 0), 0);
    }
}
