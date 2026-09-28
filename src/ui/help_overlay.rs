use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::AppState;
use crate::themes::common::fg;

const KEYS: [(&str, &str); 13] = [
    ("q  Esc  Ctrl+C", "quit (Esc closes an open overlay first)"),
    ("t", "themes: Up/Down preview, Enter saves, Esc cancels"),
    (
        "t, then Right or o",
        "options of that theme: views, accent, cards",
    ),
    ("v", "CPU view of this theme: bar, chart, both (saved)"),
    ("space", "pause the animation"),
    ("s", "settings (changes are saved at once)"),
    ("Shift + Left/Right", "settings: ten steps at once"),
    ("+ / -", "more / fewer frames per second"),
    ("l", "log overlay"),
    ("r", "rescan plugins"),
    (
        "u",
        "restart into a new version (when the status bar says so)",
    ),
    ("?", "this help"),
    ("--exit-on-any-key", "screensaver mode: any key quits"),
];

pub fn draw(frame: &mut Frame, area: Rect, state: &AppState) {
    let plugin_rows: Vec<(String, String)> = state
        .plugin_keys
        .iter()
        .map(|(key, id)| {
            let title = state.plugin_titles.get(id).unwrap_or(id);
            (key.to_string(), format!("{title}: run now"))
        })
        .collect();
    let height = u16::try_from(KEYS.len() + plugin_rows.len() + 2).unwrap_or(u16::MAX);
    let inner = super::overlay_frame(frame, super::popup(area, 72, height), "keys");
    let rows = KEYS
        .iter()
        .map(|(k, w)| (k.to_string(), w.to_string()))
        .chain(plugin_rows);
    let lines: Vec<Line> = rows
        .map(|(keys, what)| {
            Line::from(vec![
                Span::styled(
                    format!("  {keys:<20}"),
                    fg(Color::White).add_modifier(Modifier::BOLD),
                ),
                Span::raw(what),
            ])
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), inner);
}
