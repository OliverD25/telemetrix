use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs::File;
use std::io::{LineWriter, Write};
use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{Config, ConfigStatus, THEME_NAMES};
use crate::event::AppEvent;
use crate::format;
use crate::metrics::SystemSnapshot;
use crate::metrics::network::NetDrive;
use crate::plugins::manifest::SearchOption;
use crate::plugins::schema::{SchemaEntry, TEXT_MAX};
use crate::plugins::{PluginCard, PluginData, PluginStatus};
use crate::selfmem::{self, MB, SelfMemory};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    None,
    Log,
    Help,
    Settings,
    /// The `t` theme picker.
    Themes,
    /// The options panel of the theme highlighted in the picker.
    ThemeOptions,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug)]
pub struct LogLine {
    pub time: String,
    pub level: Level,
    pub text: String,
}

impl LogLine {
    /// Workers send plain text; a leading `error:` or `warning:` sets the level.
    pub fn parse(text: &str, time: SystemTime) -> Self {
        let (level, rest) = if let Some(rest) = text.strip_prefix("error: ") {
            (Level::Error, rest)
        } else if let Some(rest) = text.strip_prefix("warning: ") {
            (Level::Warn, rest)
        } else {
            (Level::Info, text)
        };
        Self {
            time: format::utc_hms(time),
            level,
            text: rest.to_string(),
        }
    }
}

/// One key press in the settings overlay's text input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputKey {
    Char(char),
    Backspace,
    Delete,
    Left,
    Right,
    Home,
    End,
    /// Up and Down choose in the search box's list.
    Up,
    Down,
    Save,
    Cancel,
}

/// The inline text input on a `text` plugin setting row.
#[derive(Clone, Debug, PartialEq)]
pub struct TextInput {
    pub id: String,
    pub key: String,
    pub text: String,
    /// In characters, 0..=text length.
    pub cursor: usize,
}

impl TextInput {
    pub fn new(id: &str, key: &str, text: &str) -> Self {
        let text: String = text.chars().take(TEXT_MAX).collect();
        Self {
            id: id.to_string(),
            key: key.to_string(),
            cursor: text.chars().count(),
            text,
        }
    }

    fn byte_at(&self, cursor: usize) -> usize {
        self.text
            .char_indices()
            .nth(cursor)
            .map_or(self.text.len(), |(i, _)| i)
    }

    /// Applies an editing key; `Save` and `Cancel` are for the caller.
    pub fn edit(&mut self, key: InputKey) {
        let len = self.text.chars().count();
        match key {
            InputKey::Char(c) if len < TEXT_MAX => {
                let at = self.byte_at(self.cursor);
                self.text.insert(at, c);
                self.cursor += 1;
            }
            InputKey::Backspace if self.cursor > 0 => {
                self.cursor -= 1;
                let at = self.byte_at(self.cursor);
                self.text.remove(at);
            }
            InputKey::Delete if self.cursor < len => {
                let at = self.byte_at(self.cursor);
                self.text.remove(at);
            }
            InputKey::Left => self.cursor = self.cursor.saturating_sub(1),
            InputKey::Right => self.cursor = (self.cursor + 1).min(len),
            InputKey::Home => self.cursor = 0,
            InputKey::End => self.cursor = len,
            _ => {}
        }
    }
}

/// After the last key, the search box waits this long before it asks.
pub const SEARCH_PAUSE: Duration = Duration::from_millis(400);
/// A search needs at least this many characters.
pub const SEARCH_MIN_CHARS: usize = 2;
/// The box gives up on an answer after this long (the plugin may be busy
/// with a slow update before it gets to the search).
pub const SEARCH_GIVE_UP: Duration = Duration::from_secs(20);

/// The search box of a `kind = "search"` setting: a text line, the places
/// the plugin's `search(query)` found, and the one chosen.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchBox {
    /// The plugin id; `input.key` is the setting and `label` its label.
    pub input: TextInput,
    pub label: String,
    /// When the text last changed and has not been searched for yet.
    edited: Option<Instant>,
    /// The query sent to the plugin and when; `None` when nothing is pending.
    pub sent: Option<(String, Instant)>,
    /// The query whose results are shown.
    pub shown: Option<String>,
    pub results: Vec<SearchOption>,
    pub selected: usize,
    pub error: Option<String>,
}

impl SearchBox {
    pub fn new(id: &str, key: &str, label: &str) -> Self {
        Self {
            input: TextInput::new(id, key, ""),
            label: label.to_string(),
            edited: None,
            sent: None,
            shown: None,
            results: Vec::new(),
            selected: 0,
            error: None,
        }
    }

    pub fn query(&self) -> &str {
        self.input.text.trim()
    }

    /// Applies a key; Up and Down move in the list, other keys edit the text.
    pub fn key(&mut self, key: InputKey, now: Instant) {
        match key {
            InputKey::Up => self.selected = self.selected.saturating_sub(1),
            InputKey::Down => {
                self.selected = (self.selected + 1).min(self.results.len().saturating_sub(1));
            }
            _ => {
                let before = self.input.text.clone();
                self.input.edit(key);
                if self.input.text != before {
                    self.edited = Some(now);
                }
            }
        }
    }

    /// The query to send now: the pause after typing is over, the text is
    /// long enough and it is not the one already shown or asked for.
    pub fn due(&mut self, now: Instant) -> Option<String> {
        if let Some((_, at)) = &self.sent
            && now.duration_since(*at) >= SEARCH_GIVE_UP
        {
            self.sent = None;
            self.error = Some("no answer from the plugin".into());
        }
        let edited = self.edited?;
        if now.duration_since(edited) < SEARCH_PAUSE {
            return None;
        }
        self.edited = None;
        let query = self.query().to_string();
        if query.chars().count() < SEARCH_MIN_CHARS {
            self.sent = None;
            self.shown = None;
            self.results.clear();
            self.error = None;
            return None;
        }
        let pending = self.sent.as_ref().map(|(q, _)| q.as_str());
        if pending == Some(query.as_str())
            || (pending.is_none() && self.shown.as_deref() == Some(query.as_str()))
        {
            return None;
        }
        self.sent = Some((query.clone(), now));
        Some(query)
    }

    /// Takes the plugin's answer; an answer to an older query is dropped.
    pub fn answer(&mut self, query: &str, result: Result<Vec<SearchOption>, String>) {
        if self.sent.as_ref().map(|(q, _)| q.as_str()) != Some(query) {
            return;
        }
        self.sent = None;
        self.shown = Some(query.to_string());
        self.selected = 0;
        match result {
            Ok(results) => {
                self.results = results;
                self.error = None;
            }
            Err(e) => {
                self.results.clear();
                self.error = Some(e);
            }
        }
    }

    pub fn waiting(&self) -> bool {
        self.sent.is_some() || self.edited.is_some()
    }

    /// The chosen place, when the list shows one.
    pub fn chosen(&self) -> Option<&SearchOption> {
        self.results.get(self.selected)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Quit,
    ToggleSettings,
    SettingsUp,
    SettingsDown,
    /// Right (`dir` 1) or Left (`dir` -1): opens the pane or a plugin's
    /// page, or steps the value; `big` = Shift, ten steps. Left on a row
    /// with no value to step goes back.
    SettingsStep {
        dir: i32,
        big: bool,
    },
    /// Enter: opens the pane, runs an action row, or is the next value.
    SettingsEnter {
        big: bool,
    },
    /// Esc (`close`) or Backspace: back one level; Esc on the category
    /// list closes the box.
    SettingsBack {
        close: bool,
    },
    /// `/`: search every setting.
    SettingsFind,
    FpsStep(i32),
    OpenThemes,
    /// A key the app does not use; it may be a plugin's run key.
    Key(char),
    /// Preview the next (1) or previous (-1) theme in the picker.
    PickerMove(i32),
    PickerSave,
    PickerCancel,
    ToggleLog,
    ToggleHelp,
    TogglePause,
    Reload,
    /// `u`: restart into the new version when one is ready.
    UpdateRestart,
    /// `v`: the next CPU view of the current theme, saved at once.
    CycleCpuView,
    /// Right or `o` in the picker: the options of the highlighted theme.
    OpenThemeOptions,
    /// In the options panel: cursor up (-1) or down (1).
    PanelMove(i32),
    /// Move the card under the cursor up (-1) or down (1) in the order.
    PanelShift(i32),
    /// The previous (-1) or next (1) value of the row.
    PanelStep(i32),
    /// Enter: the next value, a card on/off, or Save on the Save row.
    PanelEnter,
    /// Space: a card on or off.
    PanelToggle,
    PanelSave,
    /// Esc: put back what the panel changed and return to the list.
    PanelCancel,
    CloseOverlay,
    /// A key while the text input is open.
    Input(InputKey),
    Nothing,
}

/// While the text input or the search box is open every key goes to it;
/// only Ctrl+C quits.
pub fn input_key_action(key: &KeyEvent) -> Action {
    let m = key.modifiers;
    // AltGr arrives as Ctrl+Alt on Windows and types characters like @.
    if m.contains(KeyModifiers::CONTROL) && !m.contains(KeyModifiers::ALT) {
        return if key.code == KeyCode::Char('c') {
            Action::Quit
        } else {
            Action::Nothing
        };
    }
    let k = match key.code {
        KeyCode::Char(c) if !c.is_control() => InputKey::Char(c),
        KeyCode::Backspace => InputKey::Backspace,
        KeyCode::Delete => InputKey::Delete,
        KeyCode::Left => InputKey::Left,
        KeyCode::Right => InputKey::Right,
        KeyCode::Home => InputKey::Home,
        KeyCode::End => InputKey::End,
        KeyCode::Up => InputKey::Up,
        KeyCode::Down => InputKey::Down,
        KeyCode::Enter => InputKey::Save,
        KeyCode::Esc => InputKey::Cancel,
        _ => return Action::Nothing,
    };
    Action::Input(k)
}

pub fn key_action(key: &KeyEvent, overlay: Overlay, exit_on_any_key: bool) -> Action {
    if exit_on_any_key {
        return Action::Quit;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return if key.code == KeyCode::Char('c') {
            Action::Quit
        } else {
            Action::Nothing
        };
    }
    if overlay == Overlay::ThemeOptions {
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        return match key.code {
            KeyCode::Up if shift => Action::PanelShift(-1),
            KeyCode::Down if shift => Action::PanelShift(1),
            KeyCode::Char('[') => Action::PanelShift(-1),
            KeyCode::Char(']') => Action::PanelShift(1),
            KeyCode::Up => Action::PanelMove(-1),
            KeyCode::Down => Action::PanelMove(1),
            KeyCode::Left => Action::PanelStep(-1),
            KeyCode::Right => Action::PanelStep(1),
            KeyCode::Enter => Action::PanelEnter,
            KeyCode::Char(' ') => Action::PanelToggle,
            KeyCode::Char('s') => Action::PanelSave,
            KeyCode::Esc => Action::PanelCancel,
            _ => Action::Nothing,
        };
    }
    if overlay == Overlay::Themes {
        // Only Esc leaves the picker; q must not quit in the middle of a choice.
        return match key.code {
            KeyCode::Up | KeyCode::Char('T') => Action::PickerMove(-1),
            KeyCode::Down | KeyCode::Char('t') => Action::PickerMove(1),
            KeyCode::Right | KeyCode::Char('o') => Action::OpenThemeOptions,
            KeyCode::Enter => Action::PickerSave,
            KeyCode::Esc => Action::PickerCancel,
            _ => Action::Nothing,
        };
    }
    if overlay == Overlay::Settings {
        let big = key.modifiers.contains(KeyModifiers::SHIFT);
        match key.code {
            KeyCode::Up => return Action::SettingsUp,
            KeyCode::Down => return Action::SettingsDown,
            KeyCode::Enter => return Action::SettingsEnter { big },
            KeyCode::Right => return Action::SettingsStep { dir: 1, big },
            KeyCode::Left => return Action::SettingsStep { dir: -1, big },
            KeyCode::Esc => return Action::SettingsBack { close: true },
            KeyCode::Backspace => return Action::SettingsBack { close: false },
            KeyCode::Char('/') => return Action::SettingsFind,
            _ => {}
        }
    }
    match key.code {
        KeyCode::Esc if overlay != Overlay::None => Action::CloseOverlay,
        KeyCode::Esc | KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('t') | KeyCode::Char('T') => Action::OpenThemes,
        KeyCode::Char('s') => Action::ToggleSettings,
        KeyCode::Char('+') | KeyCode::Char('=') => Action::FpsStep(1),
        KeyCode::Char('-') => Action::FpsStep(-1),
        KeyCode::Char('l') => Action::ToggleLog,
        KeyCode::Char('?') => Action::ToggleHelp,
        KeyCode::Char(' ') => Action::TogglePause,
        KeyCode::Char('r') => Action::Reload,
        KeyCode::Char('u') => Action::UpdateRestart,
        KeyCode::Char('v') if overlay == Overlay::None => Action::CycleCpuView,
        KeyCode::Char(c) => Action::Key(c),
        _ => Action::Nothing,
    }
}

const TOAST_FOR: Duration = Duration::from_secs(2);
/// Keys the dashboard itself uses; a plugin cannot take them as run keys.
pub const RESERVED_KEYS: [char; 14] = [
    'q', 't', 'T', 's', 'l', 'r', 'u', 'v', '?', ' ', '+', '=', '-', 'Q',
];

pub struct AppState {
    pub config: Config,
    pub config_status: ConfigStatus,
    pub snapshot: Option<SystemSnapshot>,
    pub cpu_history: VecDeque<f32>,
    pub ram_history: VecDeque<f32>,
    pub plugins: BTreeMap<String, PluginCard>,
    pub theme_idx: usize,
    pub overlay: Overlay,
    pub log: VecDeque<LogLine>,
    pub paused: bool,
    pub dirty: bool,
    /// A short message at the bottom and the moment it disappears.
    pub toast: Option<(String, Instant)>,
    /// The page and row of the `s` box, and its `/` search.
    pub settings: crate::ui::settings_overlay::SettingsNav,
    /// The result of the last save from the settings overlay: the path or the error.
    pub settings_footer: Option<Result<String, String>>,
    /// Plugin files found when the settings overlay was opened.
    pub plugin_ids: Vec<String>,
    /// The plugin folder of the last scan, and how many plugins run from it.
    pub plugins_dir: std::path::PathBuf,
    pub plugins_running: usize,
    /// The last reading of this program's own memory.
    pub self_memory: Option<SelfMemory>,
    pub over_budget: bool,
    /// Over the budget, but a speed test runs or ended moments ago: no
    /// warning and no amber until the grace period is over.
    pub memory_paused: bool,
    /// A newer version waits: the status bar offers `u` to restart into it.
    pub update: Option<crate::update::Pending>,
    /// The `update` group of the `s` box: the last check and a running job.
    pub update_group: crate::ui::update_group::UpdateGroup,
    /// The picker's options panel while it is open.
    pub theme_panel: Option<crate::ui::theme_options::ThemePanel>,
    /// `hide`/`order` entries already reported as naming no card.
    pub warned_cards: BTreeSet<(String, String)>,
    plugins_over_budget: BTreeSet<String>,
    /// While the theme picker is open: the theme that Esc goes back to.
    pub picker_original: usize,
    /// Run keys of plugins: key → plugin id.
    pub plugin_keys: BTreeMap<char, String>,
    /// Plugin titles from their files, by id, for the help overlay.
    pub plugin_titles: BTreeMap<String, String>,
    /// Each plugin's `settings_schema`, by plugin id. Kept when a plugin
    /// stops, so a disabled plugin's settings stay editable.
    pub plugin_schemas: BTreeMap<String, Vec<SchemaEntry>>,
    /// The open text input in the settings overlay.
    pub text_input: Option<TextInput>,
    /// The open search box in the settings overlay.
    pub search_box: Option<SearchBox>,
    /// Network drives; `None` until the first answer ("checking...").
    pub network: Option<Vec<NetDrive>>,
    log_sink: Option<LineWriter<File>>,
}

pub fn theme_index(name: &str) -> usize {
    crate::themes::index_of(name).unwrap_or(0)
}

impl AppState {
    pub fn new(config: Config, config_status: ConfigStatus) -> Self {
        let theme_idx = theme_index(&config.general.theme);
        let mut state = Self {
            config,
            config_status,
            snapshot: None,
            cpu_history: VecDeque::new(),
            ram_history: VecDeque::new(),
            plugins: BTreeMap::new(),
            theme_idx,
            overlay: Overlay::None,
            log: VecDeque::new(),
            paused: false,
            dirty: true,
            toast: None,
            settings: Default::default(),
            settings_footer: None,
            plugin_ids: Vec::new(),
            plugins_dir: std::path::PathBuf::new(),
            plugins_running: 0,
            self_memory: None,
            over_budget: false,
            memory_paused: false,
            update: None,
            update_group: Default::default(),
            theme_panel: None,
            warned_cards: BTreeSet::new(),
            plugins_over_budget: BTreeSet::new(),
            picker_original: theme_idx,
            network: None,
            plugin_keys: BTreeMap::new(),
            plugin_titles: BTreeMap::new(),
            plugin_schemas: BTreeMap::new(),
            text_input: None,
            search_box: None,
            log_sink: None,
        };
        state.open_log_file();
        state
    }

    /// Opens `general.log_file` for appending; an empty path turns it off.
    pub fn open_log_file(&mut self) {
        let path = self.config.general.log_file.clone();
        self.log_sink = None;
        if path.as_os_str().is_empty() {
            return;
        }
        match File::options().create(true).append(true).open(&path) {
            Ok(f) => self.log_sink = Some(LineWriter::new(f)),
            Err(e) => self.log(&format!(
                "warning: cannot open log file {}: {e}",
                path.display()
            )),
        }
    }

    pub fn log(&mut self, text: &str) {
        let now = SystemTime::now();
        if let Some(sink) = &mut self.log_sink
            && writeln!(sink, "{} {text}", format::utc_timestamp(now)).is_err()
        {
            self.log_sink = None;
        }
        let cap = self.config.general.log_lines.max(1);
        while self.log.len() >= cap {
            self.log.pop_front();
        }
        self.log.push_back(LogLine::parse(text, now));
        if self.overlay == Overlay::Log {
            self.dirty = true;
        }
    }

    pub fn show_toast(&mut self, text: &str) {
        self.toast = Some((text.to_string(), Instant::now() + TOAST_FOR));
        self.dirty = true;
    }

    /// A text line is open (plugin text, place search or `/` search): every
    /// key goes to it and only Ctrl+C quits.
    pub fn typing(&self) -> bool {
        self.text_input.is_some() || self.search_box.is_some() || self.settings.find.is_some()
    }

    pub fn theme_name(&self) -> &'static str {
        THEME_NAMES[self.theme_idx]
    }

    pub fn toggle_overlay(&mut self, overlay: Overlay) {
        self.overlay = if self.overlay == overlay {
            Overlay::None
        } else {
            overlay
        };
        self.dirty = true;
    }

    pub fn apply(&mut self, event: AppEvent) {
        match event {
            AppEvent::Metrics(snapshot) => {
                let cap = self.config.metrics.history_len.max(1);
                push_capped(&mut self.cpu_history, snapshot.cpu_usage, cap);
                push_capped(&mut self.ram_history, snapshot.ram_pct(), cap);
                if !cfg!(windows) {
                    self.network = Some(snapshot.network.clone());
                }
                self.snapshot = Some(snapshot);
            }
            AppEvent::Plugin(data) => {
                if let Some(bytes) = data.lua_bytes {
                    self.check_plugin_memory(&data.id, bytes);
                }
                self.store_plugin(data);
            }
            AppEvent::PluginMeta {
                id,
                title,
                run_key,
                schema,
            } => {
                self.plugin_titles.insert(id.clone(), title);
                self.plugin_keys.retain(|_, owner| *owner != id);
                if let Some(key) = run_key {
                    self.register_run_key(&id, key);
                }
                self.plugin_schemas.insert(id, schema);
            }
            AppEvent::PluginRemoved(id) => {
                self.plugin_keys.retain(|_, owner| *owner != id);
                self.plugin_titles.remove(&id);
                self.plugins.remove(&id);
                self.plugins_over_budget.remove(&id);
            }
            AppEvent::Log(text) => self.log(&text),
            AppEvent::Network(drives) => self.network = Some(drives),
            AppEvent::SearchResults { id, query, result } => {
                if let Some(b) = self.search_box.as_mut().filter(|b| b.input.id == id) {
                    b.answer(&query, result);
                }
            }
            AppEvent::Update(msg) => {
                let own = crate::update::Version::current();
                let out = self.update_group.apply(msg, &own, SystemTime::now());
                for line in &out.log {
                    self.log(line);
                }
                if let Some(v) = out.installed {
                    self.update = Some(crate::update::Pending::Installed(v));
                }
                if let Some(toast) = out.toast {
                    self.show_toast(&toast);
                }
            }
        }
        self.dirty = true;
    }

    /// Accepts a plugin's run key unless the app or another plugin uses it.
    fn register_run_key(&mut self, id: &str, key: char) {
        if RESERVED_KEYS.contains(&key) {
            self.log(&format!(
                "warning: plugin {id}: run key {key:?} is used by telemetrix itself, ignored"
            ));
        } else if let Some(owner) = self.plugin_keys.get(&key) {
            let owner = owner.clone();
            self.log(&format!(
                "warning: plugin {id}: run key {key:?} already belongs to {owner}, ignored"
            ));
        } else {
            self.plugin_keys.insert(key, id.to_string());
        }
    }

    fn store_plugin(&mut self, mut data: PluginData) {
        match data.error.take() {
            // A failed update keeps the last good metrics, shown as stale.
            Some(err) => match self.plugins.get_mut(&data.id) {
                Some(card) => {
                    card.status = PluginStatus::Error(err);
                    card.data.lua_bytes = data.lua_bytes;
                }
                None => {
                    let card = PluginCard {
                        data,
                        status: PluginStatus::Error(err),
                    };
                    self.plugins.insert(card.data.id.clone(), card);
                }
            },
            None => {
                let card = PluginCard {
                    data,
                    status: PluginStatus::Ok,
                };
                self.plugins.insert(card.data.id.clone(), card);
            }
        }
    }

    /// Logs once when a plugin's Lua memory goes over its budget, and once
    /// when it comes back. It only warns: nothing is stopped (decision 32).
    fn check_plugin_memory(&mut self, id: &str, bytes: usize) {
        let budget = self.config.memory.plugin_budget_mb * MB;
        let over = bytes as u64 > budget;
        let was_over = self.plugins_over_budget.contains(id);
        if over && !was_over {
            self.plugins_over_budget.insert(id.to_string());
            self.log(&format!(
                "warning: memory: plugin {id} uses {:.1} MB of Lua memory, over its {} MB budget",
                selfmem::mb(bytes as u64),
                self.config.memory.plugin_budget_mb
            ));
        } else if !over && was_over {
            self.plugins_over_budget.remove(id);
            self.log(&format!(
                "memory: plugin {id} is back under its {} MB budget",
                self.config.memory.plugin_budget_mb
            ));
        }
    }

    /// Stores a reading of the program's own memory; logs once per budget crossing.
    pub fn record_self_memory(&mut self, m: SelfMemory) {
        let a = &crate::plugins::speed::ACTIVITY;
        self.record_memory_with(m, a.running(), a.since_last_end());
    }

    /// Amber and the warning, unless a speed test pauses them.
    pub fn memory_alarm(&self) -> bool {
        self.over_budget && !self.memory_paused
    }

    pub fn record_memory_with(
        &mut self,
        m: SelfMemory,
        test_running: bool,
        since_test: Option<Duration>,
    ) {
        let shown = |m: &SelfMemory| (selfmem::mb(m.working_set) * 10.0).round() as i64;
        if self.self_memory.as_ref().map(shown) != Some(shown(&m)) {
            self.dirty = true;
        }
        self.self_memory = Some(m);
        let budget = self.config.memory.budget_mb;
        let state = budget_state(m.working_set > budget * MB, test_running, since_test);
        let paused = state == Budget::Paused;
        if paused != self.memory_paused {
            self.memory_paused = paused;
            self.dirty = true;
        }
        if paused {
            return;
        }
        let over = state == Budget::Over;
        if over != self.over_budget {
            self.over_budget = over;
            self.dirty = true;
            let now = selfmem::mb(m.working_set);
            if over {
                self.log(&format!(
                    "warning: memory: {now:.1} MB is over the {budget} MB budget"
                ));
            } else {
                self.log(&format!(
                    "memory: {now:.1} MB is back under the {budget} MB budget"
                ));
            }
        }
    }

    /// The top banner text, or `None` when the settings are fine.
    pub fn banner(&self) -> Option<String> {
        match &self.config_status {
            ConfigStatus::Ok => None,
            ConfigStatus::Warnings(w) if w.is_empty() => None,
            ConfigStatus::Warnings(w) => Some(format!("{} config warnings (press l)", w.len())),
            ConfigStatus::Syntax { line, message } => Some(match line {
                Some(line) => {
                    format!("config syntax error line {line}: {message} - using last good settings")
                }
                None => format!("config error: {message} - using last good settings"),
            }),
        }
    }
}

/// How long after a speed test the budget warning still waits: the working
/// set shrinks slowly after the test's connections close.
pub const SPEED_TEST_GRACE: Duration = Duration::from_secs(10);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Budget {
    Under,
    Over,
    /// Over, during a speed test or its grace period.
    Paused,
}

pub fn budget_state(over: bool, test_running: bool, since_test: Option<Duration>) -> Budget {
    if !over {
        Budget::Under
    } else if test_running || since_test.is_some_and(|d| d < SPEED_TEST_GRACE) {
        Budget::Paused
    } else {
        Budget::Over
    }
}

fn push_capped(history: &mut VecDeque<f32>, v: f32, cap: usize) {
    while history.len() >= cap {
        history.pop_front();
    }
    history.push_back(v);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn key_mapping() {
        let none = Overlay::None;
        assert_eq!(
            key_action(&key(KeyCode::Char('q')), none, false),
            Action::Quit
        );
        assert_eq!(key_action(&key(KeyCode::Esc), none, false), Action::Quit);
        assert_eq!(
            key_action(&key(KeyCode::Esc), Overlay::Log, false),
            Action::CloseOverlay
        );
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_action(&ctrl_c, none, false), Action::Quit);
        assert_eq!(
            key_action(&key(KeyCode::Char('c')), none, false),
            Action::Key('c')
        );
        assert_eq!(
            key_action(&key(KeyCode::Char('t')), none, false),
            Action::OpenThemes
        );
        assert_eq!(
            key_action(&key(KeyCode::Char('T')), none, false),
            Action::OpenThemes
        );
        assert_eq!(
            key_action(&key(KeyCode::Char('l')), none, false),
            Action::ToggleLog
        );
        assert_eq!(
            key_action(&key(KeyCode::Char(' ')), none, false),
            Action::TogglePause
        );
        assert_eq!(
            key_action(&key(KeyCode::Char('r')), none, false),
            Action::Reload
        );
    }

    #[test]
    fn theme_picker_keys() {
        let p = Overlay::Themes;
        assert_eq!(
            key_action(&key(KeyCode::Char('t')), p, false),
            Action::PickerMove(1)
        );
        assert_eq!(
            key_action(&key(KeyCode::Char('T')), p, false),
            Action::PickerMove(-1)
        );
        assert_eq!(
            key_action(&key(KeyCode::Down), p, false),
            Action::PickerMove(1)
        );
        assert_eq!(
            key_action(&key(KeyCode::Up), p, false),
            Action::PickerMove(-1)
        );
        assert_eq!(
            key_action(&key(KeyCode::Enter), p, false),
            Action::PickerSave
        );
        assert_eq!(
            key_action(&key(KeyCode::Esc), p, false),
            Action::PickerCancel
        );
        assert_eq!(
            key_action(&key(KeyCode::Char('q')), p, false),
            Action::Nothing,
            "q must not quit"
        );
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(key_action(&ctrl_c, p, false), Action::Quit);
    }

    #[test]
    fn settings_overlay_keys() {
        let s = Overlay::Settings;
        assert_eq!(key_action(&key(KeyCode::Up), s, false), Action::SettingsUp);
        assert_eq!(
            key_action(&key(KeyCode::Enter), s, false),
            Action::SettingsEnter { big: false }
        );
        assert_eq!(
            key_action(&key(KeyCode::Right), s, false),
            Action::SettingsStep { dir: 1, big: false }
        );
        let shift_left = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
        assert_eq!(
            key_action(&shift_left, s, false),
            Action::SettingsStep { dir: -1, big: true }
        );
        assert_eq!(
            key_action(&key(KeyCode::Esc), s, false),
            Action::SettingsBack { close: true }
        );
        assert_eq!(
            key_action(&key(KeyCode::Backspace), s, false),
            Action::SettingsBack { close: false }
        );
        assert_eq!(
            key_action(&key(KeyCode::Char('/')), s, false),
            Action::SettingsFind
        );
        let none = Overlay::None;
        assert_eq!(key_action(&key(KeyCode::Up), none, false), Action::Nothing);
        assert_eq!(
            key_action(&key(KeyCode::Char('s')), none, false),
            Action::ToggleSettings
        );
        assert_eq!(
            key_action(&key(KeyCode::Char('+')), none, false),
            Action::FpsStep(1)
        );
    }

    #[test]
    fn u_restarts_and_no_plugin_can_take_it() {
        use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
        let u = KeyEvent::new(KeyCode::Char('u'), KeyModifiers::NONE);
        assert_eq!(key_action(&u, Overlay::None, false), Action::UpdateRestart);
        assert!(RESERVED_KEYS.contains(&'u'));
    }

    #[test]
    fn any_key_quits_in_screensaver_mode() {
        for code in [
            KeyCode::Char('t'),
            KeyCode::Char('x'),
            KeyCode::Up,
            KeyCode::Enter,
        ] {
            assert_eq!(key_action(&key(code), Overlay::None, true), Action::Quit);
        }
    }

    #[test]
    fn history_is_capped_and_log_levels_parse() {
        let mut cfg = Config::default();
        cfg.metrics.history_len = 3;
        let mut state = AppState::new(cfg, ConfigStatus::Ok);
        for i in 0..5 {
            state.apply(AppEvent::Metrics(SystemSnapshot {
                cpu_usage: i as f32,
                ..SystemSnapshot::default()
            }));
        }
        assert_eq!(state.cpu_history, [2.0, 3.0, 4.0]);
        state.apply(AppEvent::Log("error: plugin x failed".into()));
        let last = state.log.back().unwrap();
        assert_eq!(
            (last.level, last.text.as_str()),
            (Level::Error, "plugin x failed")
        );
    }

    #[test]
    fn plugin_error_keeps_the_last_good_metrics() {
        use crate::plugins::{MetricItem, PluginData};
        let mut state = AppState::new(Config::default(), ConfigStatus::Ok);
        let good = PluginData {
            id: "w".into(),
            title: "W".into(),
            metrics: vec![MetricItem::text("t", "1")],
            error: None,
            lua_bytes: None,
        };
        state.apply(AppEvent::Plugin(good));
        let mut bad = crate::plugins::runner::error_data("w", "W", "http timeout".into());
        bad.error = Some("http timeout".into());
        state.apply(AppEvent::Plugin(bad));
        let card = &state.plugins["w"];
        assert_eq!(card.status, PluginStatus::Error("http timeout".into()));
        assert_eq!(card.data.metrics.len(), 1);
        state.apply(AppEvent::PluginRemoved("w".into()));
        assert!(state.plugins.is_empty());
    }

    #[test]
    fn memory_budgets_log_each_crossing_once() {
        let mut state = AppState::new(Config::default(), ConfigStatus::Ok);
        let budget = state.config.memory.budget_mb * MB;
        let reading = |ws| SelfMemory {
            working_set: ws,
            peak_working_set: None,
            private: None,
        };
        let lines = |s: &AppState| s.log.len();
        let start = lines(&state);
        state.record_self_memory(reading(budget - MB));
        assert!(!state.over_budget);
        state.record_self_memory(reading(budget + MB));
        state.record_self_memory(reading(budget + 2 * MB));
        assert!(state.over_budget);
        assert_eq!(lines(&state), start + 1, "one line when crossing up");
        assert_eq!(state.log.back().unwrap().level, Level::Warn);
        state.record_self_memory(reading(budget - MB));
        assert_eq!(lines(&state), start + 2, "one line when coming back");

        let mut data = crate::plugins::runner::error_data("p", "P", "x".into());
        data.error = None;
        data.lua_bytes = Some((2 * MB) as usize);
        state.apply(AppEvent::Plugin(data.clone()));
        state.apply(AppEvent::Plugin(data.clone()));
        assert_eq!(
            lines(&state),
            start + 3,
            "one plugin warning, not one per update"
        );
        data.lua_bytes = Some(1000);
        state.apply(AppEvent::Plugin(data));
        assert_eq!(lines(&state), start + 4);
    }

    #[test]
    fn budget_state_pauses_during_a_speed_test_and_10_s_after() {
        let s = Duration::from_secs;
        assert_eq!(budget_state(false, true, None), Budget::Under);
        assert_eq!(budget_state(true, false, None), Budget::Over);
        assert_eq!(budget_state(true, true, None), Budget::Paused);
        assert_eq!(budget_state(true, false, Some(s(9))), Budget::Paused);
        assert_eq!(budget_state(true, false, Some(s(10))), Budget::Over);
        assert_eq!(budget_state(true, false, Some(s(600))), Budget::Over);
    }

    #[test]
    fn a_speed_test_pauses_the_memory_warning() {
        let mut state = AppState::new(Config::default(), ConfigStatus::Ok);
        let over = SelfMemory {
            working_set: (state.config.memory.budget_mb + 1) * MB,
            peak_working_set: Some((state.config.memory.budget_mb + 2) * MB),
            private: Some(5 * MB),
        };
        let lines = state.log.len();
        state.record_memory_with(over, true, None);
        assert!(state.memory_paused && !state.memory_alarm());
        assert_eq!(state.log.len(), lines, "no warning while the test runs");
        assert_eq!(state.self_memory, Some(over), "the real values are kept");
        state.record_memory_with(over, false, Some(Duration::from_secs(3)));
        assert!(state.memory_paused);
        assert_eq!(state.log.len(), lines, "nor in the grace period");
        state.record_memory_with(over, false, Some(Duration::from_secs(11)));
        assert!(!state.memory_paused && state.memory_alarm());
        assert_eq!(
            state.log.len(),
            lines + 1,
            "still over afterwards: one warning"
        );
        assert_eq!(state.log.back().unwrap().level, Level::Warn);
    }

    #[test]
    fn run_keys_skip_app_keys_and_duplicates() {
        let mut state = AppState::new(Config::default(), ConfigStatus::Ok);
        let meta = |id: &str, key| AppEvent::PluginMeta {
            id: id.into(),
            title: id.into(),
            run_key: Some(key),
            schema: Vec::new(),
        };
        state.apply(meta("speedtest", 'g'));
        state.apply(meta("other", 'g'));
        state.apply(meta("bad", 's'));
        assert_eq!(state.plugin_keys.len(), 1);
        assert_eq!(state.plugin_keys[&'g'], "speedtest");
        let warnings = state.log.iter().filter(|l| l.level == Level::Warn).count();
        assert_eq!(warnings, 2);
        state.apply(AppEvent::PluginRemoved("speedtest".into()));
        assert!(state.plugin_keys.is_empty());
        assert_eq!(
            key_action(&key(KeyCode::Char('g')), Overlay::None, false),
            Action::Key('g')
        );
    }

    #[test]
    fn text_input_takes_every_key_but_ctrl_c() {
        let k = |code| input_key_action(&key(code));
        assert_eq!(k(KeyCode::Char('q')), Action::Input(InputKey::Char('q')));
        assert_eq!(k(KeyCode::Char('t')), Action::Input(InputKey::Char('t')));
        assert_eq!(k(KeyCode::Char('s')), Action::Input(InputKey::Char('s')));
        assert_eq!(k(KeyCode::Char(' ')), Action::Input(InputKey::Char(' ')));
        assert_eq!(k(KeyCode::Esc), Action::Input(InputKey::Cancel));
        assert_eq!(k(KeyCode::Enter), Action::Input(InputKey::Save));
        assert_eq!(
            k(KeyCode::Up),
            Action::Input(InputKey::Up),
            "the text input ignores it; the search box moves"
        );
        assert_eq!(k(KeyCode::Tab), Action::Nothing);
        let shift_k = KeyEvent::new(KeyCode::Char('K'), KeyModifiers::SHIFT);
        assert_eq!(
            input_key_action(&shift_k),
            Action::Input(InputKey::Char('K'))
        );
        let altgr = KeyEvent::new(
            KeyCode::Char('@'),
            KeyModifiers::CONTROL | KeyModifiers::ALT,
        );
        assert_eq!(input_key_action(&altgr), Action::Input(InputKey::Char('@')));
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(input_key_action(&ctrl_c), Action::Quit);
        let ctrl_x = KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL);
        assert_eq!(input_key_action(&ctrl_x), Action::Nothing);
    }

    #[test]
    fn text_input_edits_at_the_cursor_and_stops_at_64() {
        let mut t = TextInput::new("weather", "city", "Kyv");
        assert_eq!(t.cursor, 3);
        t.edit(InputKey::Left);
        t.edit(InputKey::Char('i'));
        assert_eq!((t.text.as_str(), t.cursor), ("Kyiv", 3));
        t.edit(InputKey::Home);
        t.edit(InputKey::Delete);
        t.edit(InputKey::Char('К'));
        assert_eq!(t.text, "Кyiv", "non-ASCII letters insert as one character");
        t.edit(InputKey::End);
        t.edit(InputKey::Backspace);
        assert_eq!(t.text, "Кyi");
        t.edit(InputKey::Home);
        t.edit(InputKey::Backspace);
        assert_eq!(t.text, "Кyi", "nothing before the cursor");
        let mut long = TextInput::new("w", "city", &"x".repeat(70));
        assert_eq!(long.text.chars().count(), TEXT_MAX);
        long.edit(InputKey::Char('y'));
        assert_eq!(long.text.chars().count(), TEXT_MAX);
    }

    fn option(label: &str) -> SearchOption {
        SearchOption {
            label: label.into(),
            values: vec![(
                "city".into(),
                crate::config::Value::Str(label.to_string().into()),
            )],
        }
    }

    #[test]
    fn search_box_waits_for_a_pause_and_two_letters() {
        let t0 = Instant::now();
        let ms = |n| t0 + Duration::from_millis(n);
        let mut b = SearchBox::new("weather", "city", "city");
        b.key(InputKey::Char('L'), ms(0));
        assert_eq!(b.due(ms(500)), None, "one letter is not enough");
        b.key(InputKey::Char('v'), ms(600));
        assert_eq!(b.due(ms(900)), None, "still typing");
        assert_eq!(b.due(ms(1000)).as_deref(), Some("Lv"));
        assert!(b.waiting());
        b.key(InputKey::Char('i'), ms(1100));
        assert_eq!(b.due(ms(1600)).as_deref(), Some("Lvi"));
        // The answer to "Lv" arrives late: dropped.
        b.answer("Lv", Ok(vec![option("Lviv"), option("Lvivka")]));
        assert!(b.results.is_empty());
        b.answer("Lvi", Ok(vec![option("Lviv"), option("Lvivka")]));
        assert!(!b.waiting());
        b.key(InputKey::Down, ms(1700));
        b.key(InputKey::Down, ms(1700));
        assert_eq!(b.chosen().map(|o| o.label.as_str()), Some("Lvivka"));
        b.key(InputKey::Up, ms(1800));
        assert_eq!(b.chosen().map(|o| o.label.as_str()), Some("Lviv"));
        // Typing and deleting back to the shown query asks nothing new.
        b.key(InputKey::Char('x'), ms(1900));
        b.key(InputKey::Backspace, ms(1950));
        assert_eq!(b.due(ms(2400)), None);
        b.key(InputKey::Char('x'), ms(2500));
        assert_eq!(b.due(ms(2900)).as_deref(), Some("Lvix"));
        b.answer("Lvix", Err("Open-Meteo: HTTP 500".into()));
        assert_eq!(b.error.as_deref(), Some("Open-Meteo: HTTP 500"));
        assert!(b.chosen().is_none());
        b.key(InputKey::Char('y'), ms(3000));
        assert_eq!(b.due(ms(3400)).as_deref(), Some("Lvixy"));
        assert_eq!(b.due(ms(3400) + SEARCH_GIVE_UP), None);
        assert_eq!(b.error.as_deref(), Some("no answer from the plugin"));
        assert!(!b.waiting());
    }

    #[test]
    fn search_keys_reach_the_box_and_ctrl_c_still_quits() {
        assert_eq!(
            input_key_action(&key(KeyCode::Down)),
            Action::Input(InputKey::Down)
        );
        assert_eq!(
            input_key_action(&key(KeyCode::Char('q'))),
            Action::Input(InputKey::Char('q'))
        );
        let ctrl_c = KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert_eq!(input_key_action(&ctrl_c), Action::Quit);
        let mut state = AppState::new(Config::default(), ConfigStatus::Ok);
        state.search_box = Some(SearchBox::new("weather", "city", "city"));
        let b = state.search_box.as_mut().unwrap();
        b.key(InputKey::Char('K'), Instant::now());
        b.key(InputKey::Char('y'), Instant::now());
        let query = b.due(Instant::now() + SEARCH_PAUSE).unwrap();
        state.apply(AppEvent::SearchResults {
            id: "other".into(),
            query: query.clone(),
            result: Ok(vec![option("x")]),
        });
        assert!(state.search_box.as_ref().unwrap().results.is_empty());
        state.apply(AppEvent::SearchResults {
            id: "weather".into(),
            query,
            result: Ok(vec![option("Kyiv")]),
        });
        assert_eq!(state.search_box.as_ref().unwrap().results.len(), 1);
    }

    #[test]
    fn banner_text() {
        let mut state = AppState::new(Config::default(), ConfigStatus::Ok);
        assert_eq!(state.banner(), None);
        state.config_status = ConfigStatus::Warnings(vec!["a".into(), "b".into()]);
        assert_eq!(state.banner().unwrap(), "2 config warnings (press l)");
        state.config_status = ConfigStatus::Syntax {
            line: Some(14),
            message: "expected `=`".into(),
        };
        assert!(
            state
                .banner()
                .unwrap()
                .starts_with("config syntax error line 14: expected `=`")
        );
    }
}
