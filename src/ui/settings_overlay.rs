//! The `s` overlay: one row per registry setting plus `enabled` and
//! `interval` rows for every plugin file. Every change is saved at once.

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::app::AppState;
use crate::config::{self, Config, Kind, MATRIX_COLORS, PluginConfig, SETTINGS, Setting, Value};
use crate::themes::common::{ACCENT, MUTED, WARN, fg};

const PLUGIN_INTERVALS: [u64; 10] = [5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600];
const KEY_WIDTH: usize = 20;

pub enum Row {
    Section(String),
    Setting(&'static Setting),
    PluginEnabled(String),
    PluginInterval(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    Set(String, Value),
    /// Removes the key from the file, so the default applies again.
    Unset(String),
}

impl Change {
    pub fn key(&self) -> &str {
        match self {
            Change::Set(key, _) | Change::Unset(key) => key,
        }
    }
}

pub fn rows(plugin_ids: &[String]) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut section = "";
    for s in SETTINGS.iter().filter(|s| s.path != "schema") {
        let (table, _) = s.path.rsplit_once('.').unwrap_or(("", s.path));
        if table != section {
            rows.push(Row::Section(table.to_string()));
            section = table;
        }
        rows.push(Row::Setting(s));
    }
    for id in plugin_ids {
        rows.push(Row::Section(format!("plugin.{id}")));
        rows.push(Row::PluginEnabled(id.clone()));
        rows.push(Row::PluginInterval(id.clone()));
    }
    rows
}

/// Indices of the rows the cursor can land on (everything but section headers).
pub fn selectable(rows: &[Row]) -> Vec<usize> {
    rows.iter()
        .enumerate()
        .filter(|(_, r)| !matches!(r, Row::Section(_)))
        .map(|(i, _)| i)
        .collect()
}

fn plugin_cfg<'a>(cfg: &'a Config, id: &str) -> Option<&'a PluginConfig> {
    cfg.plugin_cfg.get(id)
}

fn key_label(row: &Row) -> &str {
    match row {
        Row::Section(name) => name,
        Row::Setting(s) => s.path.rsplit_once('.').map_or(s.path, |(_, k)| k),
        Row::PluginEnabled(_) => "enabled",
        Row::PluginInterval(_) => "interval",
    }
}

pub fn value_text(row: &Row, cfg: &Config) -> String {
    match row {
        Row::Section(_) => String::new(),
        Row::Setting(s) => match cfg.get(s.path) {
            Some(Value::Bool(b)) => on_off(b),
            // Paths keep their quotes, as in the file, so an empty one is visible.
            Some(Value::Str(text)) if !matches!(s.kind, Kind::Path) => text.into_owned(),
            Some(v) => v.to_string(),
            None => String::new(),
        },
        Row::PluginEnabled(id) => on_off(plugin_cfg(cfg, id).is_none_or(|c| c.enabled)),
        Row::PluginInterval(id) => match plugin_cfg(cfg, id).and_then(|c| c.interval) {
            Some(s) => format!("{s} s"),
            None => "plugin default".into(),
        },
    }
}

fn on_off(b: bool) -> String {
    if b { "on" } else { "off" }.to_string()
}

pub fn editable(row: &Row) -> bool {
    match row {
        Row::Section(_) => false,
        Row::Setting(s) => {
            s.tui_editable && !matches!(s.kind, Kind::Path | Kind::Str | Kind::StrList)
        }
        Row::PluginEnabled(_) | Row::PluginInterval(_) => true,
    }
}

/// About a twentieth of the range, rounded to 1, 2 or 5 times a power of ten.
fn nice_step(range: i64) -> i64 {
    let raw = (range as f64 / 20.0).max(1.0);
    let magnitude = 10f64.powf(raw.log10().floor());
    let step = [1.0, 2.0, 5.0, 10.0]
        .iter()
        .map(|m| m * magnitude)
        .find(|s| *s >= raw)
        .unwrap_or(raw);
    step as i64
}

/// The next multiple of the step in `dir`, wrapping at both ends.
pub fn step_int(v: i64, min: i64, max: i64, dir: i32, big: bool) -> i64 {
    let step = nice_step(max - min) * if big { 10 } else { 1 };
    if dir > 0 {
        if v >= max {
            min
        } else {
            ((v.div_euclid(step) + 1) * step).clamp(min, max)
        }
    } else if v <= min {
        max
    } else {
        let below = if v.rem_euclid(step) == 0 {
            v - step
        } else {
            v.div_euclid(step) * step
        };
        below.clamp(min, max)
    }
}

pub fn step_float(v: f64, min: f64, max: f64, dir: i32, big: bool) -> f64 {
    let tenths = (v * 10.0).round() as i64;
    let (lo, hi) = ((min * 10.0).round() as i64, (max * 10.0).round() as i64);
    let step = if big { 10 } else { 1 };
    let next = if dir > 0 {
        if tenths >= hi {
            lo
        } else {
            (tenths + step).min(hi)
        }
    } else if tenths <= lo {
        hi
    } else {
        (tenths - step).max(lo)
    };
    next as f64 / 10.0
}

fn cycle(i: usize, len: usize, dir: i32) -> usize {
    (i as i64 + i64::from(dir)).rem_euclid(len as i64) as usize
}

/// The change one press makes to `row`, or `None` for read-only rows.
pub fn step(row: &Row, cfg: &Config, dir: i32, big: bool) -> Option<Change> {
    if !editable(row) {
        return None;
    }
    match row {
        Row::Section(_) => None,
        Row::Setting(s) => {
            let current = cfg.get(s.path)?;
            let value = match &s.kind {
                Kind::Bool => Value::Bool(!current.as_bool()),
                Kind::Int { min, max } => {
                    Value::Int(step_int(current.as_i64(), *min, *max, dir, big))
                }
                Kind::Float { min, max } => {
                    Value::Float(step_float(current.as_f64(), *min, *max, dir, big))
                }
                Kind::Enum(options) => {
                    let i = options
                        .iter()
                        .position(|o| *o == current.as_str())
                        .unwrap_or(0);
                    Value::Str(options[cycle(i, options.len(), dir)].into())
                }
                Kind::Color => {
                    let i = MATRIX_COLORS.iter().position(|c| *c == current.as_str());
                    let next = i.map_or(0, |i| cycle(i, MATRIX_COLORS.len(), dir));
                    Value::Str(MATRIX_COLORS[next].into())
                }
                Kind::Str | Kind::Path | Kind::StrList => return None,
            };
            Some(Change::Set(s.path.to_string(), value))
        }
        Row::PluginEnabled(id) => {
            let on = plugin_cfg(cfg, id).is_none_or(|c| c.enabled);
            Some(Change::Set(
                format!("plugin.{id}.enabled"),
                Value::Bool(!on),
            ))
        }
        Row::PluginInterval(id) => {
            // Position 0 is "file default"; 1.. are PLUGIN_INTERVALS.
            let key = format!("plugin.{id}.interval");
            let pos = match plugin_cfg(cfg, id).and_then(|c| c.interval) {
                None => 0,
                Some(s) => PLUGIN_INTERVALS
                    .iter()
                    .rposition(|v| *v <= s)
                    .map_or(1, |i| i + 1),
            };
            match cycle(pos, PLUGIN_INTERVALS.len() + 1, dir) {
                0 => Some(Change::Unset(key)),
                n => Some(Change::Set(key, Value::Int(PLUGIN_INTERVALS[n - 1] as i64))),
            }
        }
    }
}

/// Applies a change to the in-memory settings (the caller saves it).
pub fn apply(cfg: &mut Config, change: &Change) {
    let key = change.key();
    let Some(rest) = key.strip_prefix("plugin.") else {
        if let Change::Set(key, value) = change {
            cfg.assign(key, value);
        }
        return;
    };
    let Some((id, field)) = rest.rsplit_once('.') else {
        return;
    };
    let entry = cfg
        .plugin_cfg
        .entry(id.to_string())
        .or_insert_with(|| PluginConfig {
            enabled: true,
            ..PluginConfig::default()
        });
    match (field, change) {
        ("enabled", Change::Set(_, v)) => entry.enabled = v.as_bool(),
        ("interval", Change::Set(_, v)) => entry.interval = Some(v.as_i64().max(0) as u64),
        // Without the key the built-in default applies, as it will after the next reload.
        ("interval", Change::Unset(_)) => {
            entry.interval = Config::default()
                .plugin_cfg
                .get(id)
                .and_then(|c| c.interval);
        }
        _ => {}
    }
}

pub fn save(path: &std::path::Path, change: &Change) -> std::io::Result<()> {
    match change {
        Change::Set(key, value) => config::set(path, key, value),
        Change::Unset(key) => config::unset(path, key),
    }
}

pub fn draw(frame: &mut Frame, area: Rect, state: &AppState) {
    let rows = rows(&state.plugin_ids);
    let sel = selectable(&rows);
    let current = sel[state.settings_cursor.min(sel.len() - 1)];
    let height = u16::try_from(rows.len() + 5).unwrap_or(u16::MAX);
    let inner = super::overlay_frame(frame, super::popup(area, 62, height), "settings");
    let [list, footer] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(2)]).areas(inner);
    let visible = usize::from(list.height).max(1);
    let offset = (current + 1).saturating_sub(visible);
    let width = usize::from(list.width);
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(i, row)| row_line(row, i == current, &state.config, width))
        .collect();
    frame.render_widget(Paragraph::new(lines), list);
    let status = match &state.settings_footer {
        Some(Ok(path)) => Line::styled(format!("saved → {path}"), fg(ACCENT)),
        Some(Err(e)) => Line::styled(format!("not saved: {e}"), fg(WARN)),
        None => Line::styled(
            "changes apply live and are saved at once",
            fg(ACCENT).add_modifier(Modifier::ITALIC),
        ),
    };
    let footer_lines = vec![
        Line::styled(
            "Up/Down move · Enter/Right next · Left back · Esc close",
            fg(MUTED),
        ),
        status,
    ];
    frame.render_widget(Paragraph::new(footer_lines), footer);
}

fn row_line(row: &Row, selected: bool, cfg: &Config, width: usize) -> Line<'static> {
    let highlight = Style::new().bg(Color::Rgb(62, 62, 62)).fg(Color::White);
    if let Row::Section(name) = row {
        return Line::styled(format!("[{name}]"), fg(ACCENT).add_modifier(Modifier::BOLD));
    }
    let key = key_label(row);
    let value = value_text(row, cfg);
    if !editable(row) {
        let text = format!("  {key:<KEY_WIDTH$}  {value:<12}edit in file");
        let style = if selected {
            highlight.fg(MUTED)
        } else {
            fg(MUTED)
        };
        return Line::styled(format!("{text:<width$}"), style);
    }
    let value = if selected {
        format!("< {value} >")
    } else {
        format!("  {value}")
    };
    let text = format!("  {key:<KEY_WIDTH$}{value}");
    if selected {
        Line::styled(
            format!("{text:<width$}"),
            highlight.add_modifier(Modifier::BOLD),
        )
    } else {
        Line::raw(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find_row<'a>(rows: &'a [Row], path: &str) -> &'a Row {
        rows.iter()
            .find(|r| matches!(r, Row::Setting(s) if s.path == path))
            .unwrap()
    }

    #[test]
    fn rows_follow_the_registry_and_plugins() {
        let rows = rows(&["clock".into()]);
        assert!(matches!(&rows[0], Row::Section(s) if s == "general"));
        assert!(
            !rows
                .iter()
                .any(|r| matches!(r, Row::Setting(s) if s.path == "schema"))
        );
        assert!(matches!(rows.last(), Some(Row::PluginInterval(id)) if id == "clock"));
        assert!(!editable(find_row(&rows, "general.plugins_dir")));
        assert!(editable(find_row(&rows, "general.fps")));
    }

    #[test]
    fn values_read_like_the_mockup() {
        let cfg = Config::default();
        let rows = rows(&["clock".into()]);
        assert_eq!(value_text(find_row(&rows, "general.theme"), &cfg), "matrix");
        assert_eq!(
            value_text(find_row(&rows, "general.plugins_dir"), &cfg),
            "\"plugins\""
        );
        assert_eq!(
            value_text(find_row(&rows, "theme.minimalist.show_sparklines"), &cfg),
            "on"
        );
        assert_eq!(
            value_text(find_row(&rows, "theme.matrix.speed"), &cfg),
            "1.0"
        );
        let enabled = rows
            .iter()
            .find(|r| matches!(r, Row::PluginEnabled(_)))
            .unwrap();
        assert_eq!(value_text(enabled, &cfg), "on");
    }

    #[test]
    fn integer_steps_match_the_mockup_and_wrap() {
        assert_eq!(step_int(15, 1, 60, 1, false), 20);
        assert_eq!(step_int(15, 1, 60, -1, false), 10);
        assert_eq!(step_int(60, 1, 60, 1, false), 1);
        assert_eq!(step_int(1, 1, 60, -1, false), 60);
        assert_eq!(step_int(1, 1, 60, 1, false), 5);
        assert_eq!(step_int(80, 1, 100, 1, false), 85);
        assert_eq!(step_int(1000, 200, 60_000, 1, false), 5000);
        assert_eq!(step_int(15, 1, 60, 1, true), 50);
    }

    #[test]
    fn float_steps_are_tenths() {
        assert_eq!(step_float(0.5, 0.0, 1.0, 1, false), 0.6);
        assert_eq!(step_float(1.0, 0.0, 1.0, 1, false), 0.0);
        assert_eq!(step_float(0.1, 0.1, 5.0, -1, false), 5.0);
        assert_eq!(step_float(1.0, 0.1, 5.0, 1, true), 2.0);
    }

    #[test]
    fn each_kind_steps() {
        let cfg = Config::default();
        let rows = rows(&["weather".into()]);
        let set = |path: &str, dir| step(find_row(&rows, path), &cfg, dir, false);
        assert_eq!(
            set("general.theme", 1),
            Some(Change::Set(
                "general.theme".into(),
                Value::Str("minimalist".into())
            ))
        );
        assert_eq!(
            set("general.exit_on_any_key", 1),
            Some(Change::Set(
                "general.exit_on_any_key".into(),
                Value::Bool(true)
            ))
        );
        assert_eq!(
            set("theme.matrix.color", -1),
            Some(Change::Set(
                "theme.matrix.color".into(),
                Value::Str("white".into())
            ))
        );
        assert_eq!(
            set("theme.matrix.density", 1),
            Some(Change::Set(
                "theme.matrix.density".into(),
                Value::Float(0.6)
            ))
        );
        assert_eq!(set("general.plugins_dir", 1), None);
        let interval = rows
            .iter()
            .find(|r| matches!(r, Row::PluginInterval(_)))
            .unwrap();
        let change = step(interval, &cfg, 1, false).unwrap();
        // The defaults set weather to 600 s, so the next choice is 1800 s.
        assert_eq!(
            change,
            Change::Set("plugin.weather.interval".into(), Value::Int(1800))
        );
        let back = step(interval, &cfg, -1, false).unwrap();
        assert_eq!(
            back,
            Change::Set("plugin.weather.interval".into(), Value::Int(300))
        );
    }

    #[test]
    fn plugin_changes_apply_and_unset() {
        let mut cfg = Config::default();
        apply(
            &mut cfg,
            &Change::Set("plugin.new.enabled".into(), Value::Bool(false)),
        );
        assert!(!cfg.plugin_cfg["new"].enabled);
        apply(
            &mut cfg,
            &Change::Set("plugin.new.interval".into(), Value::Int(30)),
        );
        assert_eq!(cfg.plugin_cfg["new"].interval, Some(30));
        apply(&mut cfg, &Change::Unset("plugin.new.interval".into()));
        assert_eq!(cfg.plugin_cfg["new"].interval, None);
        apply(
            &mut cfg,
            &Change::Set("plugin.weather.interval".into(), Value::Int(30)),
        );
        apply(&mut cfg, &Change::Unset("plugin.weather.interval".into()));
        assert_eq!(cfg.plugin_cfg["weather"].interval, Some(600));
        apply(&mut cfg, &Change::Set("general.fps".into(), Value::Int(30)));
        assert_eq!(cfg.general.fps, 30);
    }
}
