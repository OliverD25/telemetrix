//! The options panel of the `t` picker: the highlighted theme's views,
//! chart height, accent and cards, previewed live and saved together.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::{Paragraph, Wrap};

use super::settings_overlay::{self, Row};
use crate::app::AppState;
use crate::config::{self, Config, Setting, Value};
use crate::themes::common::{MUTED, fg};

pub const HINT: &str = "↑↓ move · ←→ Enter change · Space card on/off · \
                        [ ] or Shift+↑↓ move card · s save · Esc back";
const WIDTH: u16 = 44;
const LABEL_W: usize = 14;

/// The panel while it is open: which theme, where the cursor is, and the
/// settings from before, which Esc puts back.
#[derive(Clone, Debug)]
pub struct ThemePanel {
    pub theme: String,
    pub cursor: usize,
    pub before: Config,
    /// Keys changed since the panel opened, saved together.
    pub changed: Vec<String>,
}

pub enum PanelRow {
    Option(&'static Setting),
    Card(String),
    Save,
}

/// Every card of `theme`, in the order it shows them: `order` first, then
/// the system cards, then the plugins.
pub fn card_order(cfg: &Config, theme: &str, plugins: &[String]) -> Vec<String> {
    let all = settings_overlay::card_ids(plugins.iter().cloned());
    let order = &cfg.theme(theme).order;
    let mut out: Vec<String> = order
        .iter()
        .filter(|id| all.contains(id))
        .cloned()
        .collect();
    out.extend(all.into_iter().filter(|id| !order.contains(id)));
    out
}

/// The plugins with a card now: the running ones.
pub fn plugin_ids(state: &AppState) -> Vec<String> {
    state.plugins.keys().cloned().collect()
}

pub fn rows(state: &AppState, panel: &ThemePanel) -> Vec<PanelRow> {
    let theme = &panel.theme;
    let mut rows: Vec<PanelRow> = ["cpu_view", "ram_view", "chart_height"]
        .iter()
        .map(|k| format!("theme.{theme}.{k}"))
        .chain(std::iter::once(config::accent_key(theme)))
        .filter_map(|k| config::find(&k))
        .map(PanelRow::Option)
        .collect();
    for id in card_order(&state.config, theme, &plugin_ids(state)) {
        rows.push(PanelRow::Card(id));
    }
    rows.push(PanelRow::Save);
    rows
}

/// The change one step makes on a row; `None` on the Save row.
pub fn step(row: &PanelRow, cfg: &Config, theme: &str, dir: i32) -> Option<(String, Value)> {
    match row {
        PanelRow::Option(s) => match settings_overlay::step(&Row::Setting(s), cfg, dir, false)? {
            settings_overlay::Change::Set(k, v) => Some((k, v)),
            settings_overlay::Change::Unset(_) => None,
        },
        PanelRow::Card(id) => match settings_overlay::toggle_card(cfg, theme, id) {
            settings_overlay::Change::Set(k, v) => Some((k, v)),
            settings_overlay::Change::Unset(_) => None,
        },
        PanelRow::Save => None,
    }
}

/// The new `order` after moving the card at `index` of `cards` by `dir`.
pub fn moved(cards: &[String], index: usize, dir: i32) -> Option<Vec<String>> {
    let to = index.checked_add_signed(dir as isize)?;
    if to >= cards.len() {
        return None;
    }
    let mut out = cards.to_vec();
    out.swap(index, to);
    Some(out)
}

fn label(key: &str) -> &'static str {
    match key {
        "cpu_view" => "CPU view",
        "ram_view" => "RAM view",
        "chart_height" => "chart height",
        _ => "accent",
    }
}

pub fn draw(frame: &mut Frame, area: Rect, state: &AppState) {
    let Some(panel) = &state.theme_panel else {
        return;
    };
    let top = area.y + u16::from(state.banner().is_some()) + 1;
    let room = area.bottom().saturating_sub(top + 1);
    let rows = rows(state, panel);
    let wanted = u16::try_from(rows.len() + 6).unwrap_or(u16::MAX);
    let w = WIDTH.min(area.width.saturating_sub(2));
    // Left of the picker when both fit, otherwise over it.
    let picker_w = super::theme_picker::WIDTH + 1;
    let right = if area.width >= w + picker_w + 2 {
        area.right().saturating_sub(picker_w)
    } else {
        area.right()
    };
    let rect = Rect::new(right.saturating_sub(w + 1), top, w, wanted.min(room)).intersection(area);
    let title = format!("{} options", panel.theme);
    let inner = super::overlay_frame(frame, rect, &title);
    let [list, hint] = Layout::vertical([Constraint::Fill(1), Constraint::Length(3)]).areas(inner);
    let width = usize::from(list.width);
    let visible = usize::from(list.height).max(1);
    let first = panel.cursor.saturating_sub(visible - 1);
    let cfg = &state.config;
    let theme_opts = cfg.theme(&panel.theme);
    let highlight = Style::new()
        .bg(Color::Rgb(62, 62, 62))
        .fg(Color::White)
        .add_modifier(Modifier::BOLD);
    let mut lines = Vec::new();
    for (i, row) in rows.iter().enumerate().skip(first).take(visible) {
        let selected = i == panel.cursor;
        let (text, dim) = match row {
            PanelRow::Option(s) => {
                let key = s.path.rsplit_once('.').map_or(s.path, |(_, k)| k);
                let value = cfg.get(s.path).map(|v| v.as_str().to_string());
                let value = value.unwrap_or_default();
                let value = if selected {
                    format!("< {value} >")
                } else {
                    format!("  {value}")
                };
                let unused = settings_overlay::unused(&Row::Setting(s));
                let note = if unused {
                    "  (not used by this theme)"
                } else {
                    ""
                };
                (format!(" {:<LABEL_W$}{value}{note}", label(key)), unused)
            }
            PanelRow::Card(id) => {
                let mark = if theme_opts.hidden(id) { ' ' } else { 'x' };
                (format!(" [{mark}] {id}"), false)
            }
            PanelRow::Save => (format!(" Save options for {}", panel.theme), false),
        };
        let text: String = text.chars().take(width).collect();
        let line = if selected {
            let style = if dim { highlight.fg(MUTED) } else { highlight };
            Line::styled(format!("{text:<width$}"), style)
        } else if dim {
            Line::styled(text, fg(MUTED))
        } else {
            Line::raw(text)
        };
        lines.push(line);
    }
    frame.render_widget(Paragraph::new(lines), list);
    let note = if panel.changed.is_empty() {
        String::new()
    } else {
        format!(" · {} not saved", panel.changed.len())
    };
    let hint_text = format!("{HINT}{note}");
    frame.render_widget(
        Paragraph::new(Line::styled(hint_text, fg(MUTED))).wrap(Wrap { trim: true }),
        hint,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cards_move_one_place_and_stop_at_the_ends() {
        let cards: Vec<String> = ["cpu", "ram", "swap"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(moved(&cards, 1, -1).unwrap(), ["ram", "cpu", "swap"]);
        assert_eq!(moved(&cards, 1, 1).unwrap(), ["cpu", "swap", "ram"]);
        assert!(moved(&cards, 0, -1).is_none());
        assert!(moved(&cards, 2, 1).is_none());
        let mut cfg = Config::default();
        let plugins = vec!["clock".to_string()];
        assert_eq!(
            card_order(&cfg, "matrix", &plugins),
            ["cpu", "ram", "swap", "gpu", "disks", "network", "clock"]
        );
        cfg.assign(
            "theme.matrix.order",
            &Value::List(vec!["clock".into(), "gone".into(), "swap".into()]),
        );
        assert_eq!(
            card_order(&cfg, "matrix", &plugins),
            ["clock", "swap", "cpu", "ram", "gpu", "disks", "network"],
            "unknown ids are left out"
        );
    }
}
