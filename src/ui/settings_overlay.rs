//! The `s` box: five categories on the left, the chosen category's rows on
//! the right, one sub-page per plugin, and `/` to search every row. Every
//! change is saved at once.

use std::collections::BTreeMap;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use crate::app::{AppState, SEARCH_MIN_CHARS, SearchBox, TextInput};
use crate::config::{
    self, ACCENT_NAMES, Config, Kind, MATRIX_COLORS, PluginConfig, SETTINGS, Setting, Value,
};
use crate::plugins::PluginStatus;
use crate::plugins::schema::{self, SchemaEntry, SchemaKind, TEXT_MAX};
use crate::themes::common::{ACCENT, MUTED, WARN, fg};

const PLUGIN_INTERVALS: [u64; 10] = [5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600];
const KEY_WIDTH: usize = 20;
const BOX_WIDTH: u16 = 78;
/// The category column: " > Appearance" and a space.
const LIST_WIDTH: u16 = 14;
/// Narrower than this, the categories become one line of letters on top.
const COLUMNS_MIN_WIDTH: u16 = 56;
const HISTORY_KEY: &str = "metrics.history_len";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Appearance,
    Cards,
    Plugins,
    Updates,
    System,
}

pub const CATEGORIES: [Category; 5] = [
    Category::Appearance,
    Category::Cards,
    Category::Plugins,
    Category::Updates,
    Category::System,
];

impl Category {
    pub fn name(self) -> &'static str {
        match self {
            Category::Appearance => "Appearance",
            Category::Cards => "Cards",
            Category::Plugins => "Plugins",
            Category::Updates => "Updates",
            Category::System => "System",
        }
    }
}

/// Which side of the box the arrow keys work on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Focus {
    #[default]
    Categories,
    Pane,
}

/// Where the user is in the `s` box. It starts at Appearance every time
/// the box opens, and a reload of the file leaves it alone.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsNav {
    /// An index into [`CATEGORIES`].
    pub category: usize,
    pub focus: Focus,
    /// The plugin whose sub-page is open on the Plugins page.
    pub plugin: Option<String>,
    /// The chosen row among the page's [`selectable`] rows.
    pub cursor: usize,
    /// The `/` search while it is open.
    pub find: Option<Finder>,
}

impl SettingsNav {
    pub fn category(&self) -> Category {
        CATEGORIES[self.category.min(CATEGORIES.len() - 1)]
    }
}

/// The `/` search: the typed text and the chosen result.
#[derive(Clone, Debug, PartialEq)]
pub struct Finder {
    pub input: TextInput,
    pub selected: usize,
}

impl Finder {
    pub fn new() -> Self {
        Self {
            input: TextInput::new("", "", ""),
            selected: 0,
        }
    }
}

impl Default for Finder {
    fn default() -> Self {
        Self::new()
    }
}

pub enum Row {
    Section(String),
    Setting(&'static Setting),
    PluginEnabled(String),
    PluginInterval(String),
    /// A key from the plugin's `settings_schema`.
    PluginSetting {
        id: String,
        entry: SchemaEntry,
    },
    /// A plugin on the Plugins page; Right or Enter opens its sub-page.
    PluginLink {
        id: String,
        status: &'static str,
    },
    /// A card of a theme: on, or listed in the theme's `hide`.
    ThemeCard {
        theme: String,
        id: String,
    },
    /// A value to read, not to change.
    Info {
        label: &'static str,
        value: String,
    },
    /// Enter runs `action`; Left and Right do nothing. `value` says how it goes.
    Action {
        label: &'static str,
        value: String,
        action: RowAction,
    },
}

/// What an action row does; the main loop runs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowAction {
    CheckUpdate,
    InstallUpdate,
}

/// Every card id a theme can show: the system cards, then `plugins`.
pub fn card_ids(plugins: impl IntoIterator<Item = String>) -> Vec<String> {
    let mut ids: Vec<String> = config::SYSTEM_CARDS.iter().map(|s| s.to_string()).collect();
    for id in plugins {
        if !ids.contains(&id) {
            ids.push(id);
        }
    }
    ids
}

/// The options of `theme` that a settings box shows: views, chart height
/// and accent, then one on/off row per card.
pub fn theme_rows(theme: &str, cards: &[String]) -> Vec<Row> {
    let mut rows = vec![Row::Section(format!("options of {theme}"))];
    let keys = ["cpu_view", "ram_view", "chart_height"]
        .iter()
        .map(|k| format!("theme.{theme}.{k}"))
        .chain(std::iter::once(config::accent_key(theme)));
    for key in keys {
        if let Some(s) = config::find(&key) {
            rows.push(Row::Setting(s));
        }
    }
    rows.push(Row::Section(format!("cards of {theme}")));
    for id in cards {
        rows.push(Row::ThemeCard {
            theme: theme.to_string(),
            id: id.clone(),
        });
    }
    rows
}

fn table_of(path: &str) -> &str {
    path.rsplit_once('.').map_or("", |(table, _)| table)
}

/// The registry settings of `tables`, in that order, each table under its
/// own header.
fn sections(tables: &[&str], keep: impl Fn(&Setting) -> bool) -> Vec<Row> {
    let mut rows = Vec::new();
    for table in tables {
        let mut settings = SETTINGS
            .iter()
            .filter(|s| table_of(s.path) == *table && !s.theme_option && keep(s))
            .peekable();
        if settings.peek().is_some() {
            rows.push(Row::Section(table.to_string()));
        }
        rows.extend(settings.map(Row::Setting));
    }
    rows
}

fn settings_rows(paths: &[&str]) -> impl Iterator<Item = Row> {
    paths
        .iter()
        .filter_map(|p| config::find(p))
        .map(Row::Setting)
}

/// Theme, frame rate, the current theme's options and cards, units and
/// the keys only one theme has.
fn appearance(state: &AppState) -> Vec<Row> {
    let theme = state.theme_name();
    let cards = card_ids(state.plugin_ids.iter().cloned());
    let accent = config::accent_key(theme);
    let mut rows: Vec<Row> = settings_rows(&["general.theme", "general.fps"]).collect();
    rows.extend(theme_rows(theme, &cards));
    rows.extend(sections(
        &["units", "theme.matrix", "theme.minimalist"],
        |s| s.path != accent,
    ));
    rows
}

fn cards() -> Vec<Row> {
    sections(&["thresholds", "disks", "gpu", "metrics"], |s| {
        table_of(s.path) != "metrics" || s.path == HISTORY_KEY
    })
}

fn system() -> Vec<Row> {
    sections(&["general", "memory", "plugins", "metrics"], |s| {
        !matches!(
            s.path,
            "general.theme" | "general.fps" | "plugins.enabled" | HISTORY_KEY
        )
    })
}

/// "on", "off" (this plugin or all plugins switched off) or "error".
fn plugin_status(state: &AppState, id: &str) -> &'static str {
    let on =
        state.config.plugins.enabled && state.config.plugin_cfg.get(id).is_none_or(|c| c.enabled);
    let failed = state
        .plugins
        .get(id)
        .is_some_and(|c| matches!(c.status, PluginStatus::Error(_)));
    if !on {
        "off"
    } else if failed {
        "error"
    } else {
        "on"
    }
}

/// The switch for all plugins, then one row per plugin file.
fn plugin_list(state: &AppState) -> Vec<Row> {
    let mut rows: Vec<Row> = settings_rows(&["plugins.enabled"]).collect();
    rows.push(Row::Section("plugin files".into()));
    if state.plugin_ids.is_empty() {
        rows.push(Row::Info {
            label: "none found",
            value: String::new(),
        });
    }
    for id in &state.plugin_ids {
        rows.push(Row::PluginLink {
            id: id.clone(),
            status: plugin_status(state, id),
        });
    }
    rows
}

/// One plugin's sub-page: `enabled`, `interval` and its `settings_schema`.
pub fn plugin_rows(id: &str, schemas: &BTreeMap<String, Vec<SchemaEntry>>) -> Vec<Row> {
    let mut rows = vec![
        Row::Section(format!("plugin.{id}")),
        Row::PluginEnabled(id.to_string()),
        Row::PluginInterval(id.to_string()),
    ];
    for entry in schemas.get(id).into_iter().flatten() {
        rows.push(Row::PluginSetting {
            id: id.to_string(),
            entry: entry.clone(),
        });
    }
    rows
}

/// The rows of one page: a category, or a plugin's sub-page when `plugin`
/// is set on the Plugins page.
pub fn page(state: &AppState, category: Category, plugin: Option<&str>) -> Vec<Row> {
    match category {
        Category::Appearance => appearance(state),
        Category::Cards => cards(),
        Category::Plugins => match plugin {
            Some(id) => plugin_rows(id, &state.plugin_schemas),
            None => plugin_list(state),
        },
        Category::Updates => {
            let mut rows = update_rows(state);
            rows.extend(settings_rows(&["update.auto", "update.check_interval_h"]));
            rows
        }
        Category::System => system(),
    }
}

/// The page the box shows now.
pub fn current_page(state: &AppState) -> Vec<Row> {
    let nav = &state.settings;
    page(state, nav.category(), nav.plugin.as_deref())
}

/// The index in `rows` of the row under the cursor.
pub fn current_index(rows: &[Row], cursor: usize) -> Option<usize> {
    let sel = selectable(rows);
    sel.get(cursor.min(sel.len().saturating_sub(1))).copied()
}

/// Every page with the category index and plugin that open it: the five
/// categories, and a sub-page for each plugin file.
pub fn pages(state: &AppState) -> Vec<(usize, Option<String>, Vec<Row>)> {
    let mut out = Vec::new();
    for (i, c) in CATEGORIES.iter().enumerate() {
        out.push((i, None, page(state, *c, None)));
        if *c == Category::Plugins {
            for id in &state.plugin_ids {
                out.push((i, Some(id.clone()), page(state, *c, Some(id))));
            }
        }
    }
    out
}

/// The file key of a row, which the `/` search matches besides the label.
/// A card row gives its theme's `hide` key.
pub fn row_key(row: &Row) -> String {
    match row {
        Row::Section(_) => String::new(),
        Row::Setting(s) => s.path.to_string(),
        Row::PluginEnabled(id) => format!("plugin.{id}.enabled"),
        Row::PluginInterval(id) => format!("plugin.{id}.interval"),
        Row::PluginSetting { id, entry } => format!("plugin.{id}.{}", entry.key),
        Row::PluginLink { id, .. } => format!("plugin.{id}"),
        Row::ThemeCard { theme, .. } => format!("theme.{theme}.hide"),
        Row::Info { label, .. } | Row::Action { label, .. } => label.to_string(),
    }
}

/// One result of the `/` search and the place Enter jumps to.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    /// Like `Cards › gpu › enabled` or `Plugins › weather › city`.
    pub path: String,
    pub category: usize,
    pub plugin: Option<String>,
    pub cursor: usize,
}

/// Every row whose label or key contains `query`, ignoring case; an empty
/// query lists them all.
pub fn hits(state: &AppState, query: &str) -> Vec<Hit> {
    let query = query.trim().to_lowercase();
    let mut out = Vec::new();
    for (category, plugin, rows) in pages(state) {
        let name = CATEGORIES[category].name();
        let mut section: Option<&str> = None;
        let mut cursor = 0;
        for row in &rows {
            if let Row::Section(s) = row {
                section = Some(s);
                continue;
            }
            let label = key_label(row);
            let found = query.is_empty()
                || label.to_lowercase().contains(&query)
                || row_key(row).to_lowercase().contains(&query);
            if found {
                // The Plugins headers only repeat what the plugin id says.
                let mut parts = vec![name];
                if let Some(id) = &plugin {
                    parts.push(id);
                } else if let Some(s) =
                    section.filter(|_| CATEGORIES[category] != Category::Plugins)
                {
                    parts.push(s);
                }
                parts.push(label);
                out.push(Hit {
                    path: parts.join(" › "),
                    category,
                    plugin: plugin.clone(),
                    cursor,
                });
            }
            cursor += 1;
        }
    }
    out
}

/// The update group above its `auto` and `check_interval_h` rows.
pub fn update_rows(state: &AppState) -> Vec<Row> {
    let own = crate::update::Version::current();
    let g = &state.update_group;
    let mut rows = vec![
        Row::Info {
            label: "version",
            value: own.to_string(),
        },
        Row::Info {
            label: "latest",
            value: g.latest_text(),
        },
        Row::Action {
            label: "check now",
            value: g.check_text(),
            action: RowAction::CheckUpdate,
        },
    ];
    if let Some(value) = g.install_text(&own) {
        rows.push(Row::Action {
            label: "install now",
            value,
            action: RowAction::InstallUpdate,
        });
    }
    rows
}

/// The action of an action row.
pub fn action_row(row: &Row) -> Option<RowAction> {
    match row {
        Row::Action { action, .. } => Some(*action),
        _ => None,
    }
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

/// The text a `text` row holds, for the input to start with.
pub fn text_value(row: &Row, cfg: &Config) -> String {
    match row {
        Row::PluginSetting { id, entry } => schema_value(cfg, id, entry).as_str().to_string(),
        _ => String::new(),
    }
}

/// The plugin id and schema key of a `text` row, which Enter edits inline.
pub fn text_row(row: &Row) -> Option<(&str, &str)> {
    match row {
        Row::PluginSetting { id, entry } if entry.kind == SchemaKind::Text => {
            Some((id, &entry.key))
        }
        _ => None,
    }
}

/// The plugin id and schema entry of a `search` row, which Enter opens
/// the search box for.
pub fn search_row(row: &Row) -> Option<(&str, &SchemaEntry)> {
    match row {
        Row::PluginSetting { id, entry } if entry.kind == SchemaKind::Search => Some((id, entry)),
        _ => None,
    }
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

pub fn key_label(row: &Row) -> &str {
    match row {
        Row::Section(name) => name,
        Row::Setting(s) if s.path == "plugins.enabled" => "all plugins",
        Row::Setting(s) => s.path.rsplit_once('.').map_or(s.path, |(_, k)| k),
        Row::PluginLink { id, .. } => id,
        Row::PluginEnabled(_) => "enabled",
        Row::PluginInterval(_) => "interval",
        Row::PluginSetting { entry, .. } => &entry.label,
        Row::ThemeCard { id, .. } => id,
        Row::Info { label, .. } | Row::Action { label, .. } => label,
    }
}

fn schema_value(cfg: &Config, id: &str, entry: &SchemaEntry) -> Value {
    entry.current(plugin_cfg(cfg, id).map(|c| &c.settings))
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
        Row::PluginSetting { id, entry } => match schema_value(cfg, id, entry) {
            Value::Bool(b) => on_off(b),
            Value::Str(text) if text.is_empty() => "(not set)".into(),
            Value::Str(text) => text.into_owned(),
            v => v.to_string(),
        },
        Row::ThemeCard { theme, id } => on_off(!cfg.theme(theme).hidden(id)),
        Row::PluginLink { status, .. } => status.to_string(),
        Row::Info { value, .. } | Row::Action { value, .. } => value.clone(),
    }
}

/// A theme option this theme draws nothing for, like the CPU view of kernel-log.
pub fn unused(row: &Row) -> bool {
    match row {
        Row::Setting(s) if s.theme_option => {
            let rest = s.path.strip_prefix("theme.").unwrap_or(s.path);
            let (theme, key) = rest.rsplit_once('.').unwrap_or(("", rest));
            !crate::themes::uses_option(theme, key)
        }
        _ => false,
    }
}

/// The `hide` list after switching one card on or off.
pub fn toggle_card(cfg: &Config, theme: &str, id: &str) -> Change {
    let mut hide = cfg.theme(theme).hide.clone();
    if let Some(i) = hide.iter().position(|h| h == id) {
        hide.remove(i);
    } else {
        hide.push(id.to_string());
    }
    Change::Set(format!("theme.{theme}.hide"), Value::List(hide))
}

fn on_off(b: bool) -> String {
    if b { "on" } else { "off" }.to_string()
}

pub fn editable(row: &Row) -> bool {
    match row {
        Row::Section(_) | Row::Info { .. } | Row::Action { .. } | Row::PluginLink { .. } => false,
        Row::Setting(s) => {
            s.tui_editable
                && !matches!(
                    s.kind,
                    Kind::Path | Kind::Str | Kind::StrList | Kind::CardList
                )
        }
        Row::PluginEnabled(_)
        | Row::PluginInterval(_)
        | Row::PluginSetting { .. }
        | Row::ThemeCard { .. } => true,
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
        Row::Section(_) | Row::Info { .. } | Row::Action { .. } | Row::PluginLink { .. } => None,
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
                Kind::Accent => {
                    let i = ACCENT_NAMES.iter().position(|c| *c == current.as_str());
                    let next = i.map_or(0, |i| cycle(i, ACCENT_NAMES.len(), dir));
                    Value::Str(ACCENT_NAMES[next].into())
                }
                Kind::Str | Kind::Path | Kind::StrList | Kind::CardList => return None,
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
        Row::PluginSetting { id, entry } => {
            let current = schema_value(cfg, id, entry);
            let value = match &entry.kind {
                SchemaKind::Text | SchemaKind::Search => return None,
                SchemaKind::Bool => Value::Bool(!current.as_bool()),
                SchemaKind::Enum(options) => {
                    let i = options
                        .iter()
                        .position(|o| o == current.as_str())
                        .unwrap_or(0);
                    Value::Str(options[cycle(i, options.len(), dir)].clone().into())
                }
                SchemaKind::Int { min, max, step } => Value::Int(schema::step_int(
                    current.as_i64(),
                    *min,
                    *max,
                    *step,
                    dir,
                    big,
                )),
            };
            Some(Change::Set(format!("plugin.{id}.{}", entry.key), value))
        }
        Row::ThemeCard { theme, id } => Some(toggle_card(cfg, theme, id)),
    }
}

/// The change Enter in the text input makes: an empty text removes the key,
/// so the default applies again.
pub fn text_change(input: &TextInput) -> Change {
    let key = format!("plugin.{}.{}", input.id, input.key);
    if input.text.is_empty() {
        Change::Unset(key)
    } else {
        Change::Set(key, Value::Str(input.text.clone().into()))
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
        (_, Change::Set(_, v)) => {
            entry
                .settings
                .insert(field, toml_edit::Item::Value(v.to_toml()));
        }
        (_, Change::Unset(_)) => {
            let default = Config::default()
                .plugin_cfg
                .get(id)
                .and_then(|c| c.settings.get(field).cloned());
            match default {
                Some(item) => {
                    entry.settings.insert(field, item);
                }
                None => {
                    entry.settings.remove(field);
                }
            }
        }
    }
}

pub fn save(path: &std::path::Path, change: &Change) -> std::io::Result<()> {
    match change {
        Change::Set(key, value) => config::set(path, key, value),
        Change::Unset(key) => config::unset(path, key),
    }
}

fn highlight() -> Style {
    Style::new().bg(Color::Rgb(62, 62, 62)).fg(Color::White)
}

/// The box is as tall as the longest page, so moving between pages does
/// not resize it; a short terminal makes the pane scroll instead.
fn box_height(state: &AppState) -> u16 {
    let tallest = pages(state)
        .iter()
        .map(|(_, _, rows)| rows.len())
        .max()
        .unwrap_or(0)
        .max(CATEGORIES.len());
    u16::try_from(tallest + 4).unwrap_or(u16::MAX)
}

/// `long` when it fits in `width`, else `short`.
fn fit(long: &str, short: &str, width: usize) -> String {
    if long.chars().count() <= width {
        long
    } else {
        short
    }
    .to_string()
}

fn keys_hint(state: &AppState, width: usize) -> String {
    let nav = &state.settings;
    if nav.find.is_some() {
        return fit(
            "↑↓ choose · Enter go there · Esc cancel",
            "↑↓ · Enter go · Esc cancel",
            width,
        );
    }
    if state.text_input.is_some() {
        return fit(
            &format!("Enter save · Esc cancel · max {TEXT_MAX} characters"),
            "Enter save · Esc cancel",
            width,
        );
    }
    if nav.focus == Focus::Categories {
        return fit(
            "↑↓ choose · → open · / search · Esc close",
            "↑↓ · → open · / · Esc close",
            width,
        );
    }
    let rows = current_page(state);
    let on_link =
        current_index(&rows, nav.cursor).is_some_and(|i| matches!(rows[i], Row::PluginLink { .. }));
    if on_link {
        fit(
            "↑↓ move · → open · / search · Esc back",
            "↑↓ · → open · / · Esc back",
            width,
        )
    } else {
        fit(
            "↑↓ move · ←→ change · / search · Esc back",
            "↑↓ ←→ · / search · Esc back",
            width,
        )
    }
}

pub fn draw(frame: &mut Frame, area: Rect, state: &AppState) {
    let nav = &state.settings;
    let rect = super::popup(area, BOX_WIDTH, box_height(state));
    let inner = super::overlay_frame(frame, rect, "Settings");
    let [body, footer] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(2)]).areas(inner);
    if let Some(finder) = &nav.find {
        draw_find(frame, body, state, finder);
    } else {
        let columns =
            body.width >= COLUMNS_MIN_WIDTH && usize::from(body.height) >= CATEGORIES.len();
        let pane = if columns {
            let [list, bar, pane] = Layout::horizontal([
                Constraint::Length(LIST_WIDTH),
                Constraint::Length(2),
                Constraint::Fill(1),
            ])
            .areas(body);
            draw_categories(frame, list, nav);
            let bar_lines: Vec<Line> = (0..bar.height)
                .map(|_| Line::styled("│", fg(MUTED)))
                .collect();
            frame.render_widget(Paragraph::new(bar_lines), bar);
            pane
        } else {
            let [tabs, pane] =
                Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(body);
            draw_tabs(frame, tabs, nav);
            pane
        };
        draw_pane(frame, pane, state);
    }
    let status = match &state.settings_footer {
        Some(Ok(path)) => Line::styled(format!("saved → {path}"), fg(ACCENT)),
        Some(Err(e)) => Line::styled(format!("not saved: {e}"), fg(WARN)),
        None => Line::styled(
            "changes apply live and are saved at once",
            fg(ACCENT).add_modifier(Modifier::ITALIC),
        ),
    };
    let keys = keys_hint(state, usize::from(footer.width));
    let footer_lines = vec![Line::styled(keys, fg(MUTED)), status];
    frame.render_widget(Paragraph::new(footer_lines), footer);
    if let Some(b) = &state.search_box {
        draw_search(frame, area, b);
    }
}

/// The chosen category: highlighted while the arrows work on the list,
/// in the accent colour while they work on the pane.
fn category_style(chosen: bool, focus: Focus) -> Style {
    match (chosen, focus) {
        (true, Focus::Categories) => highlight().add_modifier(Modifier::BOLD),
        (true, Focus::Pane) => fg(ACCENT).add_modifier(Modifier::BOLD),
        (false, _) => Style::new(),
    }
}

fn draw_categories(frame: &mut Frame, area: Rect, nav: &SettingsNav) {
    let width = usize::from(area.width);
    let lines: Vec<Line> = CATEGORIES
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let chosen = i == nav.category;
            let text = format!(" {} {}", if chosen { '>' } else { ' ' }, c.name());
            Line::styled(format!("{text:<width$}"), category_style(chosen, nav.focus))
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// On a narrow or short box: the categories as letters on one line, then
/// the name of the open page.
fn draw_tabs(frame: &mut Frame, area: Rect, nav: &SettingsNav) {
    let mut spans = vec![Span::raw(" ")];
    for (i, c) in CATEGORIES.iter().enumerate() {
        let chosen = i == nav.category;
        let style = if chosen {
            category_style(true, nav.focus)
        } else {
            fg(MUTED)
        };
        spans.push(Span::styled(format!(" {} ", &c.name()[..1]), style));
    }
    let mut name = nav.category().name().to_string();
    if let Some(id) = &nav.plugin {
        name = format!("{name} › {id}");
    }
    spans.push(Span::styled(
        format!(" {name}"),
        fg(ACCENT).add_modifier(Modifier::BOLD),
    ));
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_pane(frame: &mut Frame, area: Rect, state: &AppState) {
    let nav = &state.settings;
    let rows = current_page(state);
    let focused = nav.focus == Focus::Pane;
    let current = current_index(&rows, nav.cursor).unwrap_or(0);
    let visible = usize::from(area.height).max(1);
    let offset = if focused {
        (current + 1).saturating_sub(visible)
    } else {
        0
    };
    let width = usize::from(area.width);
    let key_w = KEY_WIDTH.min(width.saturating_sub(4) / 2);
    let lines: Vec<Line> = rows
        .iter()
        .enumerate()
        .skip(offset)
        .take(visible)
        .map(|(i, row)| {
            let selected = focused && i == current;
            let input = state.text_input.as_ref().filter(|_| selected);
            row_line(row, selected, &state.config, width, key_w, input)
        })
        .collect();
    frame.render_widget(Paragraph::new(lines), area);
}

/// The `/` search: the typed text, then every matching row as
/// `Category › group › setting`, the chosen one highlighted.
fn draw_find(frame: &mut Frame, area: Rect, state: &AppState, finder: &Finder) {
    let found = hits(state, &finder.input.text);
    let width = usize::from(area.width);
    let head = " / ";
    let room = width.saturating_sub(head.chars().count() + 1);
    let mut first = vec![Span::styled(head, fg(ACCENT).add_modifier(Modifier::BOLD))];
    first.extend(input_spans(&finder.input, room, Style::new()));
    let mut lines = vec![Line::from(first)];
    if found.is_empty() {
        lines.push(Line::styled(" nothing found", fg(WARN)));
    }
    let visible = usize::from(area.height).saturating_sub(1).max(1);
    let selected = finder.selected.min(found.len().saturating_sub(1));
    let offset = (selected + 1).saturating_sub(visible);
    for (i, hit) in found.iter().enumerate().skip(offset).take(visible) {
        let mark = if i == selected { '>' } else { ' ' };
        let text: String = format!(" {mark} {}", hit.path)
            .chars()
            .take(width)
            .collect();
        lines.push(if i == selected {
            Line::styled(
                format!("{text:<width$}"),
                highlight().add_modifier(Modifier::BOLD),
            )
        } else {
            Line::raw(text)
        });
    }
    frame.render_widget(Paragraph::new(lines), area);
}

const SPINNER: [char; 4] = ['|', '/', '-', '\\'];

/// The spinner character for this moment: a quarter turn every 250 ms.
fn spinner() -> char {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    SPINNER[(ms / 250 % 4) as usize]
}

/// The search box over the settings: the typed text, a status line and up
/// to eight places, the chosen one highlighted.
pub fn draw_search(frame: &mut Frame, area: Rect, b: &SearchBox) {
    let rows = crate::plugins::manifest::SEARCH_MAX;
    let height = u16::try_from(rows + 6).unwrap_or(u16::MAX);
    let title = format!("search {}", b.label);
    let inner = super::overlay_frame(frame, super::popup(area, 60, height), &title);
    let width = usize::from(inner.width);
    let highlight = Style::new().bg(Color::Rgb(62, 62, 62)).fg(Color::White);
    let head = format!(" {}: ", b.label);
    let room = width.saturating_sub(head.chars().count() + 1);
    let mut first = vec![Span::styled(head, fg(ACCENT))];
    first.extend(input_spans(&b.input, room, Style::new()));
    let mut lines = vec![Line::from(first)];
    let status = if b.waiting() && b.query().chars().count() >= SEARCH_MIN_CHARS {
        Line::styled(format!(" {} searching...", spinner()), fg(MUTED))
    } else if let Some(e) = &b.error {
        Line::styled(format!(" {e}"), fg(WARN))
    } else if b.query().chars().count() < SEARCH_MIN_CHARS {
        Line::styled(
            format!(" type {SEARCH_MIN_CHARS} or more letters"),
            fg(MUTED),
        )
    } else if b.results.is_empty() && b.shown.is_some() {
        Line::styled(" nothing found", fg(WARN))
    } else {
        Line::styled(format!(" {} found", b.results.len()), fg(MUTED))
    };
    lines.push(status);
    for (i, option) in b.results.iter().enumerate() {
        let text = format!(
            " {} {}",
            if i == b.selected { '>' } else { ' ' },
            option.label
        );
        let text: String = text.chars().take(width).collect();
        lines.push(if i == b.selected {
            Line::styled(
                format!("{text:<width$}"),
                highlight.add_modifier(Modifier::BOLD),
            )
        } else {
            Line::raw(text)
        });
    }
    let [list, footer] =
        Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(inner);
    frame.render_widget(Paragraph::new(lines), list);
    frame.render_widget(
        Paragraph::new(Line::styled(
            " Up/Down choose · Enter save · Esc cancel",
            fg(MUTED),
        )),
        footer,
    );
}

/// The text with a block cursor, scrolled so the cursor stays in `room`.
fn input_spans(input: &TextInput, room: usize, style: Style) -> Vec<Span<'static>> {
    let chars: Vec<char> = input.text.chars().collect();
    let room = room.max(1);
    let start = (input.cursor + 1).saturating_sub(room);
    let end = chars.len().min(start + room);
    let text = |r: std::ops::Range<usize>| chars.get(r).map_or(String::new(), String::from_iter);
    let under = chars.get(input.cursor).copied().unwrap_or(' ');
    vec![
        Span::styled(text(start..input.cursor), style),
        Span::styled(under.to_string(), style.add_modifier(Modifier::REVERSED)),
        Span::styled(text(input.cursor + 1..end), style),
    ]
}

fn row_line(
    row: &Row,
    selected: bool,
    cfg: &Config,
    width: usize,
    key_w: usize,
    input: Option<&TextInput>,
) -> Line<'static> {
    let highlight = highlight();
    if let Row::Section(name) = row {
        return Line::styled(format!("[{name}]"), fg(ACCENT).add_modifier(Modifier::BOLD));
    }
    let key = key_label(row);
    if let Some(input) = input {
        let style = highlight.add_modifier(Modifier::BOLD);
        let head = format!("  {key:<key_w$}  ");
        let room = width.saturating_sub(head.chars().count() + 1);
        let mut spans = vec![Span::styled(head, style)];
        spans.extend(input_spans(input, room, style));
        let used: usize = spans.iter().map(|s| s.content.chars().count()).sum();
        spans.push(Span::styled(" ".repeat(width.saturating_sub(used)), style));
        return Line::from(spans);
    }
    if let Row::PluginLink { status, .. } = row {
        if selected {
            let text = format!("  {key:<key_w$}  {status:<6}(→ open)");
            return Line::styled(
                format!("{text:<width$}"),
                highlight.add_modifier(Modifier::BOLD),
            );
        }
        let status_style = match *status {
            "error" => fg(WARN),
            "off" => fg(MUTED),
            _ => Style::new(),
        };
        return Line::from(vec![
            Span::raw(format!("  {key:<key_w$}  ")),
            Span::styled(format!("{status:<6}"), status_style),
            Span::styled("›", fg(MUTED)),
        ]);
    }
    if let Row::Info { value, .. } = row {
        let text = format!("  {key:<key_w$}  {value}");
        return if selected {
            Line::styled(format!("{text:<width$}"), highlight)
        } else {
            Line::raw(text)
        };
    }
    if let Row::Action { value, .. } = row {
        let text = if selected {
            let value = if value.is_empty() {
                String::new()
            } else {
                format!("{value}  ")
            };
            format!("  {key:<key_w$}  {value}(Enter to run)")
        } else {
            format!("  {key:<key_w$}  {value}")
        };
        return if selected {
            Line::styled(
                format!("{text:<width$}"),
                highlight.add_modifier(Modifier::BOLD),
            )
        } else {
            Line::raw(text)
        };
    }
    if selected && (text_row(row).is_some() || search_row(row).is_some()) {
        let value = value_text(row, cfg);
        let hint = if search_row(row).is_some() {
            "search"
        } else {
            "edit"
        };
        let text = format!("  {key:<key_w$}  {value}  (Enter to {hint})");
        return Line::styled(
            format!("{text:<width$}"),
            highlight.add_modifier(Modifier::BOLD),
        );
    }
    let value = value_text(row, cfg);
    if unused(row) {
        let text = format!("  {key:<key_w$}  {value:<8}(not used by this theme)");
        let style = if selected {
            highlight.fg(MUTED)
        } else {
            fg(MUTED)
        };
        return Line::styled(format!("{text:<width$}"), style);
    }
    if !editable(row) {
        let text = format!("  {key:<key_w$}  {value:<12}edit in file");
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
    let text = format!("  {key:<key_w$}{value}");
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
    use crate::config::ConfigStatus;

    fn state_with(theme: &str, plugins: &[&str]) -> AppState {
        let mut cfg = Config::default();
        cfg.general.theme = theme.into();
        let mut state = AppState::new(cfg, ConfigStatus::Ok);
        state.plugin_ids = plugins.iter().map(|p| p.to_string()).collect();
        state
    }

    /// The rows of every page, one after another.
    fn all_rows(state: &AppState) -> Vec<Row> {
        pages(state)
            .into_iter()
            .flat_map(|(_, _, rows)| rows)
            .collect()
    }

    fn find_row<'a>(rows: &'a [Row], path: &str) -> &'a Row {
        rows.iter()
            .find(|r| matches!(r, Row::Setting(s) if s.path == path))
            .unwrap()
    }

    fn labels(rows: &[Row]) -> Vec<&str> {
        rows.iter().map(key_label).collect()
    }

    #[test]
    fn five_categories_hold_their_settings() {
        let names: Vec<&str> = CATEGORIES.iter().map(|c| c.name()).collect();
        assert_eq!(
            names,
            ["Appearance", "Cards", "Plugins", "Updates", "System"]
        );
        let s = state_with("synthwave", &["clock"]);
        let names = |c| labels(&page(&s, c, None)).join(" ");
        assert_eq!(
            names(Category::Appearance),
            "theme fps options of synthwave cpu_view ram_view chart_height accent \
             cards of synthwave cpu ram swap gpu disks network clock \
             units temperature bytes theme.matrix density speed color \
             theme.minimalist show_sparklines"
        );
        assert_eq!(
            names(Category::Cards),
            "thresholds cpu_warn_pct temp_warn_c disk_warn_pct \
             disks show_network network_interval_s network_timeout_s group_network hide \
             gpu enabled interval_ms source metrics history_len"
        );
        assert_eq!(names(Category::Plugins), "all plugins plugin files clock");
        assert_eq!(
            names(Category::Updates),
            "version latest check now auto check_interval_h"
        );
        assert_eq!(
            names(Category::System),
            "general exit_on_any_key plugins_dir log_file log_lines \
             memory budget_mb plugin_budget_mb \
             plugins default_interval_s http_timeout_s call_timeout_s memory_limit_mb \
             rescan_interval_s max_plugins \
             metrics cpu_interval_ms memory_interval_ms temps_interval_ms disks_interval_ms"
        );
        assert_eq!(
            labels(&page(&s, Category::Plugins, Some("clock"))),
            ["plugin.clock", "enabled", "interval"]
        );
        let rows = all_rows(&s);
        assert!(!editable(find_row(&rows, "general.plugins_dir")));
        assert!(!editable(find_row(&rows, "disks.hide")));
        assert!(editable(find_row(&rows, "general.fps")));
        let empty = state_with("matrix", &[]);
        assert_eq!(
            labels(&page(&empty, Category::Plugins, None)),
            ["all plugins", "plugin files", "none found"]
        );
    }

    #[test]
    fn plugin_rows_show_on_off_and_error() {
        let mut s = state_with("matrix", &["clock", "weather", "crypto"]);
        apply(
            &mut s.config,
            &Change::Set("plugin.crypto.enabled".into(), Value::Bool(false)),
        );
        let mut bad = crate::plugins::runner::error_data("weather", "Weather", "x".into());
        bad.error = Some("HTTP 500".into());
        s.apply(crate::event::AppEvent::Plugin(bad));
        let status = |s: &AppState| -> Vec<String> {
            page(s, Category::Plugins, None)
                .iter()
                .filter(|r| matches!(r, Row::PluginLink { .. }))
                .map(|r| format!("{} {}", key_label(r), value_text(r, &s.config)))
                .collect()
        };
        assert_eq!(status(&s), ["clock on", "weather error", "crypto off"]);
        s.config.plugins.enabled = false;
        assert_eq!(
            status(&s),
            ["clock off", "weather off", "crypto off"],
            "all plugins off"
        );
        let link = &page(&s, Category::Plugins, None)[2];
        assert!(!editable(link) && step(link, &s.config, 1, false).is_none());
    }

    #[test]
    fn values_read_like_the_file() {
        let cfg = Config::default();
        let rows = all_rows(&state_with("minimalist", &["clock"]));
        assert_eq!(value_text(find_row(&rows, "general.theme"), &cfg), "matrix");
        assert_eq!(
            value_text(find_row(&rows, "general.plugins_dir"), &cfg),
            "\"\""
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
    fn the_current_theme_options_follow_theme_and_fps() {
        let mut state = crate::ui::demo::state("synthwave");
        state.plugin_ids = vec!["clock".into(), "weather".into()];
        let rows = page(&state, Category::Appearance, None);
        let first: Vec<&str> = rows.iter().take(15).map(key_label).collect();
        assert_eq!(
            first,
            [
                "theme",
                "fps",
                "options of synthwave",
                "cpu_view",
                "ram_view",
                "chart_height",
                "accent",
                "cards of synthwave",
                "cpu",
                "ram",
                "swap",
                "gpu",
                "disks",
                "network",
                "clock"
            ]
        );
        assert!(
            !rows
                .iter()
                .any(|r| matches!(r, Row::Section(s) if s == "theme.synthwave")),
            "no second listing of the generated keys"
        );
        let cfg = &state.config;
        assert_eq!(value_text(&rows[3], cfg), "bar");
        let swap = &rows[10];
        assert_eq!(value_text(swap, cfg), "on");
        assert_eq!(
            step(swap, cfg, 1, false),
            Some(Change::Set(
                "theme.synthwave.hide".into(),
                Value::List(vec!["swap".into()])
            ))
        );
        assert_eq!(
            step(&rows[3], cfg, 1, false),
            Some(Change::Set(
                "theme.synthwave.cpu_view".into(),
                Value::Str("chart".into())
            ))
        );
        assert_eq!(
            step(&rows[6], cfg, 1, false),
            Some(Change::Set(
                "theme.synthwave.accent".into(),
                Value::Str("red".into())
            ))
        );
        assert!(!unused(&rows[3]));
        let klog = page(
            &crate::ui::demo::state("kernel-log"),
            Category::Appearance,
            None,
        );
        assert!(unused(&klog[3]) && unused(&klog[5]) && !unused(&klog[6]));
        let matrix = page(
            &crate::ui::demo::state("matrix"),
            Category::Appearance,
            None,
        );
        assert!(matches!(&matrix[6], Row::Setting(s) if s.path == "theme.matrix.color"));
        let colors = matrix
            .iter()
            .filter(|r| matches!(r, Row::Setting(s) if s.path == "theme.matrix.color"))
            .count();
        assert_eq!(colors, 1, "matrix's accent is not listed twice");
    }

    /// Every setting the box can change is on exactly one page: the
    /// registry, each theme's options (with that theme chosen) and every
    /// built-in plugin's `enabled`, `interval` and `settings_schema`.
    #[test]
    fn every_setting_is_on_exactly_one_page() {
        use crate::plugins::runner::{Plugin, RunnerSettings, no_emit};
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        let dir = std::env::temp_dir().join(format!("telemetrix-reach-{}", std::process::id()));
        let mut run = RunnerSettings::from_config(&Config::default());
        run.data_dir = dir.clone();
        let mut ids = Vec::new();
        let mut schemas = BTreeMap::new();
        for (file, _) in crate::plugins::bundled::FILES {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("plugins")
                .join(file);
            let plugin = Plugin::load(
                &path,
                &run,
                Arc::new(AtomicBool::new(false)),
                std::rc::Rc::new(|_: &str| {}),
                no_emit(),
            )
            .unwrap_or_else(|e| panic!("{file}: {e}"));
            ids.push(plugin.id().to_string());
            schemas.insert(plugin.id().to_string(), plugin.schema().to_vec());
        }
        let _ = std::fs::remove_dir_all(&dir);
        let weather = &schemas["weather"];
        assert!(weather.iter().any(|e| e.kind == SchemaKind::Search));
        for theme in config::THEME_NAMES {
            let mut state = state_with(theme, &[]);
            state.plugin_ids = ids.clone();
            state.plugin_schemas = schemas.clone();
            let mut seen: BTreeMap<String, usize> = BTreeMap::new();
            for (_, _, rows) in pages(&state) {
                for row in rows.iter().filter(|r| !matches!(r, Row::Section(_))) {
                    let key = match row {
                        Row::ThemeCard { theme, id } => format!("theme.{theme}.hide:{id}"),
                        _ => row_key(row),
                    };
                    *seen.entry(key).or_default() += 1;
                }
            }
            let mut wanted: Vec<String> = Vec::new();
            for s in SETTINGS.iter().filter(|s| s.path != "schema") {
                let Some(rest) = s.path.strip_prefix(&format!("theme.{theme}.")) else {
                    if !s.theme_option {
                        wanted.push(s.path.to_string());
                    }
                    continue;
                };
                match rest {
                    // One row per card instead; the order is set in the `t` box.
                    "hide" => wanted.extend(
                        card_ids(ids.iter().cloned())
                            .iter()
                            .map(|id| format!("{}:{id}", s.path)),
                    ),
                    "order" => {}
                    _ => wanted.push(s.path.to_string()),
                }
            }
            for id in &ids {
                wanted.push(format!("plugin.{id}.enabled"));
                wanted.push(format!("plugin.{id}.interval"));
                for e in &schemas[id] {
                    wanted.push(format!("plugin.{id}.{}", e.key));
                }
            }
            for key in &wanted {
                assert_eq!(seen.get(key), Some(&1), "{theme}: {key}");
            }
            for key in seen.keys() {
                let other_theme = key.starts_with("theme.")
                    && !key.starts_with(&format!("theme.{theme}."))
                    && config::find(key).is_some_and(|s| s.theme_option);
                assert!(!other_theme, "{theme}: {key} belongs to another theme");
            }
            let editable_rows = SETTINGS
                .iter()
                .filter(|s| editable(&Row::Setting(s)))
                .filter(|s| !s.theme_option || s.path.starts_with(&format!("theme.{theme}.")));
            for s in editable_rows {
                assert_eq!(seen.get(s.path), Some(&1), "{theme}: {}", s.path);
            }
        }
    }

    #[test]
    fn search_filters_every_page_and_names_the_place() {
        let mut s = state_with("matrix", &["clock", "weather"]);
        s.plugin_schemas.insert(
            "weather".into(),
            vec![SchemaEntry {
                key: "city".into(),
                label: "city".into(),
                kind: SchemaKind::Search,
                default: Value::Str("".into()),
            }],
        );
        let paths = |q: &str| -> Vec<String> { hits(&s, q).into_iter().map(|h| h.path).collect() };
        assert_eq!(paths("city"), ["Plugins › weather › city"]);
        assert_eq!(paths("CiTy"), paths("city"), "case does not matter");
        assert_eq!(
            paths("interval_ms"),
            [
                "Cards › gpu › interval_ms",
                "System › metrics › cpu_interval_ms",
                "System › metrics › memory_interval_ms",
                "System › metrics › temps_interval_ms",
                "System › metrics › disks_interval_ms",
            ]
        );
        assert_eq!(
            paths("gpu.enabled"),
            ["Cards › gpu › enabled"],
            "the key matches too"
        );
        assert!(paths("weather").contains(&"Plugins › weather".to_string()));
        assert!(paths("weather").contains(&"Appearance › cards of matrix › weather".to_string()));
        assert!(paths("check").contains(&"Updates › check now".to_string()));
        assert!(paths("zzz").is_empty());
        let all = hits(&s, "");
        let rows: usize = pages(&s)
            .iter()
            .map(|(_, _, rows)| selectable(rows).len())
            .sum();
        assert_eq!(all.len(), rows, "an empty search lists every row");
        let city = &hits(&s, "city")[0];
        assert_eq!(
            (city.category, city.plugin.as_deref(), city.cursor),
            (2, Some("weather"), 2),
            "Plugins, weather's page, after enabled and interval"
        );
    }

    #[test]
    fn settings_rows_step_each_kind() {
        let cfg = Config::default();
        let rows = all_rows(&state_with("minimalist", &["weather"]));
        let set = |path: &str, dir| step(find_row(&rows, path), &cfg, dir, false);
        assert_eq!(
            set("general.theme", 1),
            Some(Change::Set(
                "general.theme".into(),
                Value::Str("tokyo-night".into())
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
    fn integer_steps_are_round_and_wrap() {
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
    fn schema_rows_follow_interval_and_step() {
        let lua = mlua::Lua::new();
        let t: mlua::Table = lua
            .load(
                "return { city = { kind = 'text', label = 'City', default = 'Kyiv' },
                          bank = { kind = 'enum', options = { 'mono', 'privat' } },
                          month = { kind = 'bool', default = true },
                          mb = { kind = 'int', min = 5, max = 100, step = 5, default = 25 } }",
            )
            .eval()
            .unwrap();
        let schemas = BTreeMap::from([("fx".to_string(), schema::parse(&t).unwrap())]);
        let rows: Vec<Row> = ["fx", "clock"]
            .iter()
            .flat_map(|id| plugin_rows(id, &schemas))
            .collect();
        let labels: Vec<&str> = rows
            .iter()
            .skip_while(|r| !matches!(r, Row::Section(s) if s == "plugin.fx"))
            .map(key_label)
            .collect();
        assert_eq!(
            labels,
            [
                "plugin.fx",
                "enabled",
                "interval",
                "bank",
                "City",
                "mb",
                "month",
                "plugin.clock",
                "enabled",
                "interval"
            ]
        );
        let mut cfg = Config::default();
        let find = |key: &str| {
            rows.iter()
                .find(|r| matches!(r, Row::PluginSetting { entry, .. } if entry.key == key))
                .unwrap()
        };
        assert_eq!(value_text(find("city"), &cfg), "Kyiv");
        let no_city = Row::PluginSetting {
            id: "fx".into(),
            entry: SchemaEntry {
                key: "city".into(),
                label: "city".into(),
                kind: SchemaKind::Text,
                default: Value::Str("".into()),
            },
        };
        assert_eq!(value_text(&no_city, &cfg), "(not set)");
        assert_eq!(text_value(&no_city, &cfg), "", "the input starts empty");
        assert_eq!(value_text(find("month"), &cfg), "on");
        assert_eq!(value_text(find("mb"), &cfg), "25");
        assert_eq!(text_row(find("city")), Some(("fx", "city")));
        assert_eq!(text_row(find("mb")), None);
        assert_eq!(
            step(find("city"), &cfg, 1, false),
            None,
            "text rows do not step"
        );
        let change = step(find("bank"), &cfg, 1, false).unwrap();
        assert_eq!(
            change,
            Change::Set("plugin.fx.bank".into(), Value::Str("privat".into()))
        );
        apply(&mut cfg, &change);
        assert_eq!(value_text(find("bank"), &cfg), "privat");
        let change = step(find("mb"), &cfg, -1, false).unwrap();
        assert_eq!(change, Change::Set("plugin.fx.mb".into(), Value::Int(20)));
        let change = step(find("month"), &cfg, 1, false).unwrap();
        apply(&mut cfg, &change);
        assert_eq!(value_text(find("month"), &cfg), "off");
        apply(&mut cfg, &Change::Unset("plugin.fx.bank".into()));
        assert_eq!(
            value_text(find("bank"), &cfg),
            "mono",
            "unset: the default again"
        );
    }

    #[test]
    fn empty_text_unsets_and_plugin_defaults_come_back() {
        let input = TextInput::new("network_ping", "host", "");
        let change = text_change(&input);
        assert_eq!(change, Change::Unset("plugin.network_ping.host".into()));
        let mut cfg = Config::default();
        let host = |cfg: &Config| {
            cfg.plugin_cfg["network_ping"].settings["host"]
                .as_str()
                .map(str::to_string)
        };
        apply(
            &mut cfg,
            &Change::Set(
                "plugin.network_ping.host".into(),
                Value::Str("8.8.8.8".into()),
            ),
        );
        assert_eq!(host(&cfg).as_deref(), Some("8.8.8.8"));
        apply(&mut cfg, &change);
        assert_eq!(
            host(&cfg).as_deref(),
            Some("1.1.1.1"),
            "the built-in plugin default, as after a reload"
        );
    }

    #[test]
    fn text_input_draws_a_cursor_that_stays_visible() {
        let mut input = TextInput::new("w", "city", "Kyiv");
        let spans = input_spans(&input, 10, Style::new());
        let text: Vec<&str> = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, ["Kyiv", " ", ""], "the cursor sits after the text");
        input.cursor = 1;
        let text: Vec<String> = input_spans(&input, 10, Style::new())
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(text, ["K", "y", "iv"]);
        let long = TextInput::new("w", "city", &"abcdefghij".repeat(3));
        let text: String = input_spans(&long, 8, Style::new())
            .iter()
            .map(|s| s.content.to_string())
            .collect();
        assert_eq!(text, "defghij ", "scrolled to keep the cursor in view");
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
