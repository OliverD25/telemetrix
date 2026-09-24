use std::collections::{BTreeMap, VecDeque};
use std::fs::File;
use std::io::{LineWriter, Write};
use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::config::{Config, ConfigStatus, THEME_NAMES};
use crate::event::AppEvent;
use crate::format;
use crate::metrics::SystemSnapshot;
use crate::plugins::{PluginCard, PluginStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    None,
    Log,
    Help,
    /// Filled in by the settings overlay step.
    #[allow(dead_code)]
    Settings,
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
    NextTheme,
    PrevTheme,
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
    match key.code {
        KeyCode::Esc if overlay != Overlay::None => Action::CloseOverlay,
        KeyCode::Esc | KeyCode::Char('q') => Action::Quit,
        KeyCode::Char('t') => Action::NextTheme,
        KeyCode::Char('T') => Action::PrevTheme,
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
    log_sink: Option<LineWriter<File>>,
}

pub fn theme_index(name: &str) -> usize {
    THEME_NAMES.iter().position(|t| *t == name).unwrap_or(0)
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
                self.snapshot = Some(snapshot);
            }
            AppEvent::Plugin(mut data) => match data.error.take() {
                // A failed update keeps the last good metrics, shown as stale.
                Some(err) => match self.plugins.get_mut(&data.id) {
                    Some(card) => card.status = PluginStatus::Error(err),
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
            },
            AppEvent::PluginRemoved(id) => {
                self.plugins.remove(&id);
            }
            AppEvent::Log(text) => self.log(&text),
        }
        self.dirty = true;
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
            Action::NextTheme
        );
        assert_eq!(
            key_action(&key(KeyCode::Char('T')), none, false),
            Action::PrevTheme
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
            metrics: vec![MetricItem {
                label: "t".into(),
                value: "1".into(),
            }],
            error: None,
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
