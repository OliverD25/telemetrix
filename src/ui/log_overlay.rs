use std::collections::VecDeque;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{Level, LogLine};
use crate::themes::common::{ACCENT, MUTED, WARN, fg};

/// The last lines that fit, newest at the bottom.
pub fn draw(frame: &mut Frame, area: Rect, log: &VecDeque<LogLine>) {
    let inner = super::overlay_frame(
        frame,
        super::popup(area, 92, 22),
        "log (newest at the bottom)",
    );
    let skip = log.len().saturating_sub(usize::from(inner.height));
    let lines: Vec<Line> = log
        .iter()
        .skip(skip)
        .map(|l| {
            let (tag, color) = match l.level {
                Level::Info => ("INFO ", MUTED),
                Level::Warn => ("WARN ", ACCENT),
                Level::Error => ("ERROR", WARN),
            };
            Line::from(vec![
                Span::styled(format!("{} ", l.time), fg(MUTED)),
                Span::styled(tag, fg(color).add_modifier(Modifier::BOLD)),
                Span::raw(format!(" {}", l.text)),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}
