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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Quit,
    ToggleSettings,
    SettingsUp,
    SettingsDown,
    /// Next (`dir` 1) or previous (`dir` -1) value; `big` = Shift, ten steps.
    SettingsStep {
        dir: i32,
        big: bool,
    },
    FpsStep(i32),
    OpenThemes,
    /// Preview the next (1) or previous (-1) theme in the picker.
    PickerMove(i32),
    PickerSave,
    PickerCancel,
    ToggleLog,
    ToggleHelp,
    TogglePause,
    Reload,
    CloseOverlay,
    Nothing,
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
    if overlay == Overlay::Themes {
        // Only Esc leaves the picker; q must not quit in the middle of a choice.
        return match key.code {
            KeyCode::Up | KeyCode::Char('T') => Action::PickerMove(-1),
            KeyCode::Down | KeyCode::Char('t') => Action::PickerMove(1),
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
            KeyCode::Enter | KeyCode::Right => return Action::SettingsStep { dir: 1, big },
            KeyCode::Left => return Action::SettingsStep { dir: -1, big },
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
        _ => Action::Nothing,
    }
}

const TOAST_FOR: Duration = Duration::from_secs(2);

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
    pub settings_cursor: usize,
    /// The result of the last save from the settings overlay: the path or the error.
    pub settings_footer: Option<Result<String, String>>,
    /// Plugin files found when the settings overlay was opened.
    pub plugin_ids: Vec<String>,
    /// The last reading of this program's own memory.
    pub self_memory: Option<SelfMemory>,
    pub over_budget: bool,
    plugins_over_budget: BTreeSet<String>,
    /// While the theme picker is open: the theme that Esc goes back to.
    pub picker_original: usize,
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
            settings_cursor: 0,
            settings_footer: None,
            plugin_ids: Vec::new(),
            self_memory: None,
            over_budget: false,
            plugins_over_budget: BTreeSet::new(),
            picker_original: theme_idx,
            network: None,
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
            AppEvent::PluginRemoved(id) => {
                self.plugins.remove(&id);
                self.plugins_over_budget.remove(&id);
            }
            AppEvent::Log(text) => self.log(&text),
            AppEvent::Network(drives) => self.network = Some(drives),
        }
        self.dirty = true;
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
        let shown = |m: &SelfMemory| (selfmem::mb(m.working_set) * 10.0).round() as i64;
        if self.self_memory.as_ref().map(shown) != Some(shown(&m)) {
            self.dirty = true;
        }
        self.self_memory = Some(m);
        let budget = self.config.memory.budget_mb;
        let over = m.working_set > budget * MB;
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
            Action::Nothing
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
            Action::SettingsStep { dir: 1, big: false }
        );
        let shift_left = KeyEvent::new(KeyCode::Left, KeyModifiers::SHIFT);
        assert_eq!(
            key_action(&shift_left, s, false),
            Action::SettingsStep { dir: -1, big: true }
        );
        assert_eq!(
            key_action(&key(KeyCode::Esc), s, false),
            Action::CloseOverlay
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
