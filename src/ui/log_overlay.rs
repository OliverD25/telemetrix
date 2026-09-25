use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::Modifier;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{AppState, Level};
use crate::selfmem;
use crate::themes::common::{ACCENT, MUTED, WARN, fg};

/// A memory header, then the last log lines that fit, newest at the bottom.
pub fn draw(frame: &mut Frame, area: Rect, state: &AppState) {
    let inner = super::overlay_frame(
        frame,
        super::popup(area, 92, 22),
        "log (newest at the bottom)",
    );
    let header = memory_header(state);
    let header_h = u16::try_from(header.len()).unwrap_or(0).min(inner.height);
    let [top, rest] =
        Layout::vertical([Constraint::Length(header_h), Constraint::Fill(1)]).areas(inner);
    frame.render_widget(Paragraph::new(header), top);
    let log = &state.log;
    let inner = rest;
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

/// `self 9.8 MB of 15 MB · peak 10.2 MB · private 2.4 MB` and each plugin's Lua memory.
fn memory_header(state: &AppState) -> Vec<Line<'static>> {
    let mb = |b: u64| format!("{:.1} MB", selfmem::mb(b));
    let mut own = match state.self_memory {
        Some(m) => {
            let mut text = format!(
                "memory: self {} of {} MB budget",
                mb(m.working_set),
                state.config.memory.budget_mb
            );
            if let Some(peak) = m.peak_working_set {
                text.push_str(&format!(" · peak {}", mb(peak)));
            }
            if let Some(private) = m.private {
                text.push_str(&format!(" · private {}", mb(private)));
            }
            text
        }
        None => "memory: not measured yet".to_string(),
    };
    if state.memory_paused {
        own.push_str(" (speed test)");
    } else if state.over_budget {
        own.push_str(" (over budget)");
    }
    let own_color = if state.memory_alarm() { ACCENT } else { MUTED };
    let plugins: Vec<String> = state
        .plugins
        .values()
        .filter_map(|c| {
            let kb = c.data.lua_bytes? / 1024;
            Some(format!("{} {kb} KB", c.data.id))
        })
        .collect();
    let lua = if plugins.is_empty() {
        "Lua: no plugin data yet".to_string()
    } else {
        format!("Lua: {}", plugins.join(" · "))
    };
    vec![
        Line::styled(own, fg(own_color)),
        Line::styled(lua, fg(MUTED)),
        Line::raw(""),
    ]
}
