use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::themes::common::fg;

const KEYS: [(&str, &str); 7] = [
    ("q  Esc  Ctrl+C", "quit (Esc closes an open overlay first)"),
    ("t / T", "next / previous theme"),
    ("space", "pause the animation"),
    ("l", "log overlay"),
    ("r", "rescan plugins"),
    ("?", "this help"),
    ("--exit-on-any-key", "screensaver mode: any key quits"),
];

pub fn draw(frame: &mut Frame, area: Rect) {
    let height = u16::try_from(KEYS.len() + 2).unwrap_or(u16::MAX);
    let inner = super::overlay_frame(frame, super::popup(area, 72, height), "keys");
    let lines: Vec<Line> = KEYS
        .iter()
        .map(|(keys, what)| {
            Line::from(vec![
                Span::styled(
                    format!("  {keys:<20}"),
                    fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::raw(*what),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}
