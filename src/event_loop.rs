use std::io;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::Size;

use crate::app::{self, Action, AppState, InputKey, Overlay, TextInput};
use crate::cli::Flags;
use crate::config::{self, Config, ConfigStatus, LoadOutcome, THEME_NAMES, Value};
use crate::event::{AppEvent, WorkerCmd};
use crate::metrics::network::{self, NetworkCmd, NetworkSettings};
use crate::metrics::worker::{self, MetricsCmd, MetricsIntervals};
use crate::plugins::PluginStatus;
use crate::plugins::bundled;
use crate::plugins::manager::{self, Manager, ScanReport};
use crate::plugins::runner;
use crate::selfmem;
use crate::term::TerminalGuard;
use crate::themes::{self, Theme};
use crate::ui;
use crate::ui::settings_overlay::{self, Change};

pub const HOUSEKEEPING: Duration = Duration::from_secs(2);
/// `poll` does not wake when a worker sends on the channel, so the wait is
/// capped to pick up new data promptly even on a static theme.
pub const DRAIN: Duration = Duration::from_millis(250);
const PANIC_TEST_AFTER: Duration = Duration::from_secs(1);
/// How often the program reads its own memory (decision 31).
const SELF_MEMORY_EVERY: Duration = Duration::from_secs(5);

pub fn next_deadline(
    now: Instant,
    frame_due: Option<Instant>,
    housekeeping_due: Instant,
) -> Instant {
    let deadline = housekeeping_due.min(now + DRAIN);
    frame_due.map_or(deadline, |f| deadline.min(f))
}

struct Loop {
    state: AppState,
    flags: Flags,
    path: std::path::PathBuf,
    last_mtime: Option<SystemTime>,
    last_size: Size,
    quit: bool,
    metrics: mpsc::Sender<MetricsCmd>,
    themes: Vec<Box<dyn Theme>>,
    plugins: Manager,
    last_rescan: Instant,
    network: mpsc::Sender<NetworkCmd>,
}

pub fn run(cfg: Config, status: ConfigStatus, flags: &Flags) -> io::Result<()> {
    crate::term::install_signal_handling();
    let path = config::resolve_path(flags.config.as_deref());
    let (tx, rx) = mpsc::channel::<AppEvent>();
    let home = bundled::home(&path);
    let synced = bundled::sync(&home, &[], false);
    let plugins = Manager::new(&cfg, &path, tx.clone());
    let network = network::spawn(NetworkSettings::from_config(&cfg), tx.clone());
    let (metrics, _metrics_thread) = worker::spawn(MetricsIntervals::from_config(&cfg), tx);
    let mut state = AppState::new(cfg, status);
    state.log(&format!(
        "telemetrix {} started, settings: {}",
        env!("CARGO_PKG_VERSION"),
        path.display()
    ));
    if let ConfigStatus::Warnings(warnings) = state.config_status.clone() {
        for w in warnings {
            state.log(&format!("warning: config: {w}"));
        }
    }
    match synced {
        Ok(report) => {
            for (file, outcome) in report.iter().filter(|(_, o)| o.worth_logging()) {
                state.log(&outcome.describe(file));
            }
        }
        Err(e) => state.log(&format!(
            "warning: cannot install the built-in plugins into {}: {e}",
            home.display()
        )),
    }
    let mut guard = TerminalGuard::new();
    let mut lp = Loop {
        state,
        flags: flags.clone(),
        last_mtime: mtime(&path),
        path,
        last_size: guard.terminal.size()?,
        quit: false,
        metrics,
        themes: themes::all(),
        plugins,
        last_rescan: Instant::now(),
        network,
    };
    lp.rescan_plugins(false);
    let result = event_loop(&mut lp, &mut guard, &rx);
    let _ = lp.metrics.send(WorkerCmd::Stop);
    let _ = lp.network.send(WorkerCmd::Stop);
    lp.plugins.stop_all();
    result
}

fn event_loop(
    lp: &mut Loop,
    guard: &mut TerminalGuard,
    rx: &mpsc::Receiver<AppEvent>,
) -> io::Result<()> {
    let start = Instant::now();
    let mut next_house = start + HOUSEKEEPING;
    let mut next_memory = start;
    let mut next_frame = start;
    // Private bytes once a minute, for `selftest --memory --soak`.
    let mut private_series: Vec<u64> = Vec::new();
    loop {
        let now = Instant::now();
        let interval = lp.frame_interval();
        let frame_due = interval.is_some_and(|_| now >= next_frame);
        if lp.state.dirty || frame_due {
            let theme = lp.themes[lp.state.theme_idx].as_mut();
            let state = &lp.state;
            guard.terminal.draw(|f| {
                if frame_due {
                    theme.tick(f.area(), &state.config);
                }
                ui::draw(f, state, theme);
            })?;
            lp.state.dirty = false;
            if let Some(iv) = interval {
                next_frame = (next_frame + iv).max(now);
            }
        }
        let deadline = next_deadline(Instant::now(), interval.map(|_| next_frame), next_house);
        if event::poll(deadline.saturating_duration_since(Instant::now()))? {
            match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    let action = if lp.state.text_input.is_some() {
                        app::input_key_action(&key)
                    } else {
                        app::key_action(
                            &key,
                            lp.state.overlay,
                            lp.state.config.general.exit_on_any_key,
                        )
                    };
                    lp.act(action);
                }
                Ok(Event::Resize(..)) => lp.state.dirty = true,
                Ok(_) => {}
                Err(_) => return Ok(()),
            }
        }
        if lp.quit || crate::term::stop_requested() {
            return Ok(());
        }
        while let Ok(ev) = rx.try_recv() {
            lp.state.apply(ev);
        }
        let now = Instant::now();
        if lp
            .state
            .toast
            .as_ref()
            .is_some_and(|(_, until)| now >= *until)
        {
            lp.state.toast = None;
            lp.state.dirty = true;
        }
        if now >= next_house {
            lp.housekeeping(guard.terminal.size()?);
            next_house = now + HOUSEKEEPING;
        }
        if now >= next_memory {
            if let Some(m) = selfmem::read() {
                lp.state.record_self_memory(m);
            }
            next_memory = now + SELF_MEMORY_EVERY;
            let minute = start.elapsed().as_secs() / 60;
            if lp.flags.selftest_report.is_some() && private_series.len() as u64 <= minute {
                let private = lp.state.self_memory.and_then(|m| m.private).unwrap_or(0);
                private_series.push(private);
            }
            #[cfg(feature = "alloc-stats")]
            log_heap(&mut lp.state, start);
        }
        if let Some(report) = &lp.flags.selftest_report
            && start.elapsed() >= Duration::from_secs(lp.flags.selftest_seconds)
        {
            let doc = crate::commands::selftest::report_json(&lp.state, &private_series);
            return config::write_atomic(report, &format!("{doc:#}"));
        }
        if lp.flags.panic_test && start.elapsed() >= PANIC_TEST_AFTER {
            panic!("--panic-test: deliberate panic to check that the terminal is restored");
        }
    }
}

/// Once a minute with `--features alloc-stats`: live heap bytes next to the
/// private bytes and each plugin's Lua bytes, for memory soaks.
#[cfg(feature = "alloc-stats")]
fn log_heap(state: &mut AppState, start: Instant) {
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_MINUTE: AtomicU64 = AtomicU64::new(u64::MAX);
    let minute = start.elapsed().as_secs() / 60;
    if LAST_MINUTE.swap(minute, Ordering::Relaxed) == minute {
        return;
    }
    let (live, calls) = crate::alloc_stats::snapshot();
    let private = state.self_memory.and_then(|m| m.private).unwrap_or(0);
    let lua: usize = state
        .plugins
        .values()
        .filter_map(|c| c.data.lua_bytes)
        .sum();
    let log_len = state.log.len();
    let (heaps, heap_allocated, heap_committed) = crate::alloc_stats::heaps();
    state.log(&format!(
        "heap: minute {minute} live {live} allocs {calls} private {private} lua {lua} log_lines {log_len}          heaps {heaps} heap_allocated {heap_allocated} heap_committed {heap_committed}"
    ));
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Loop {
    /// `None` = static theme or paused: redraw only when something changed.
    fn frame_interval(&self) -> Option<Duration> {
        if self.state.paused {
            return None;
        }
        self.themes[self.state.theme_idx].frame_interval(&self.state.config)
    }

    fn act(&mut self, action: Action) {
        let s = &mut self.state;
        match action {
            Action::Quit => self.quit = true,
            Action::OpenThemes => {
                s.picker_original = s.theme_idx;
                s.overlay = Overlay::Themes;
                s.dirty = true;
            }
            Action::PickerMove(dir) => {
                // A preview only: the file is written on Enter.
                let n = THEME_NAMES.len() as i32;
                s.theme_idx = (s.theme_idx as i32 + dir).rem_euclid(n) as usize;
                s.dirty = true;
            }
            Action::PickerSave => {
                s.overlay = Overlay::None;
                let name = THEME_NAMES[s.theme_idx];
                self.change(Change::Set("general.theme".into(), Value::Str(name.into())));
                let saved = matches!(self.state.settings_footer, Some(Ok(_)));
                let note = if saved {
                    "saved"
                } else {
                    "not saved, see the log"
                };
                self.state.show_toast(&format!("theme: {name} {note}"));
            }
            Action::PickerCancel => {
                s.theme_idx = s.picker_original;
                s.overlay = Overlay::None;
                s.dirty = true;
            }
            Action::ToggleSettings => {
                if s.overlay != Overlay::Settings {
                    s.plugin_ids = manager::discover(self.plugins.dir())
                        .iter()
                        .map(|(p, _)| runner::stem(p))
                        .collect();
                    s.settings_footer = None;
                }
                s.toggle_overlay(Overlay::Settings);
            }
            Action::SettingsUp => {
                s.settings_cursor = s.settings_cursor.saturating_sub(1);
                s.dirty = true;
            }
            Action::SettingsDown => {
                let rows = settings_overlay::rows(&s.plugin_ids, &s.plugin_schemas);
                let last = settings_overlay::selectable(&rows).len().saturating_sub(1);
                s.settings_cursor = (s.settings_cursor + 1).min(last);
                s.dirty = true;
            }
            Action::SettingsStep { dir, big } => {
                let rows = settings_overlay::rows(&s.plugin_ids, &s.plugin_schemas);
                let sel = settings_overlay::selectable(&rows);
                let row = &rows[sel[s.settings_cursor.min(sel.len() - 1)]];
                if let Some((id, key)) = settings_overlay::text_row(row) {
                    if dir > 0 {
                        let text = settings_overlay::text_value(row, &s.config);
                        s.text_input = Some(TextInput::new(id, key, &text));
                        s.dirty = true;
                    }
                } else if let Some(change) = settings_overlay::step(row, &s.config, dir, big) {
                    self.change(change);
                }
            }
            Action::Input(InputKey::Cancel) => {
                s.text_input = None;
                s.dirty = true;
            }
            Action::Input(InputKey::Save) => {
                if let Some(input) = s.text_input.take() {
                    self.change(settings_overlay::text_change(&input));
                    // The plugin runs at once with trigger "key" (the settings change merges into it).
                    self.plugins.run_now(&input.id);
                }
            }
            Action::Input(key) => {
                if let Some(input) = &mut s.text_input {
                    input.edit(key);
                    s.dirty = true;
                }
            }
            Action::FpsStep(dir) => {
                let fps = i64::from(s.config.general.fps);
                let next = settings_overlay::step_int(fps, 1, 60, dir, false);
                // + and - stop at the ends instead of wrapping like the overlay does.
                let next = if dir > 0 {
                    next.max(fps)
                } else {
                    next.min(fps)
                };
                self.change(Change::Set("general.fps".into(), Value::Int(next)));
                let shown = self.state.config.general.fps;
                self.state.show_toast(&format!("{shown} fps"));
            }
            Action::ToggleLog => s.toggle_overlay(Overlay::Log),
            Action::ToggleHelp => s.toggle_overlay(Overlay::Help),
            Action::CloseOverlay => s.toggle_overlay(Overlay::None),
            Action::TogglePause => {
                s.paused = !s.paused;
                s.dirty = true;
            }
            Action::Reload => self.rescan_plugins(true),
            Action::Key(c) => {
                if let Some(id) = s.plugin_keys.get(&c).cloned() {
                    let msg = if self.plugins.run_now(&id) {
                        format!("{id}: running now")
                    } else {
                        format!("{id} is not running")
                    };
                    self.state.show_toast(&msg);
                }
            }
            Action::Nothing => {}
        }
    }

    /// Applies one change from a key press at once, then saves it to the file.
    fn change(&mut self, change: Change) {
        // The user chose this value, so a command-line flag must stop overriding it.
        config::release_flag(&mut self.flags, change.key());
        let mut new = self.state.config.clone();
        settings_overlay::apply(&mut new, &change);
        self.adopt(new, false);
        let saved = settings_overlay::save(&self.path, &change);
        // This write needs no reload: the settings in memory already match it.
        self.last_mtime = mtime(&self.path);
        let s = &mut self.state;
        s.settings_footer = Some(match saved {
            Ok(()) => Ok(self.path.display().to_string()),
            Err(e) => {
                s.log(&format!("error: cannot save {}: {e}", self.path.display()));
                Err(e.to_string())
            }
        });
        s.dirty = true;
    }

    /// What the empty Plugins card needs: the folder and whether anything runs.
    fn note_scan(&mut self, report: &ScanReport) {
        self.state.plugins_dir = self.plugins.dir().to_path_buf();
        self.state.plugins_running = report.running;
        self.state.dirty = true;
    }

    fn rescan_plugins(&mut self, announce: bool) {
        self.last_rescan = Instant::now();
        let report = self.plugins.rescan();
        self.note_scan(&report);
        let errors = self
            .state
            .plugins
            .values()
            .filter(|c| matches!(c.status, PluginStatus::Error(_)))
            .count();
        let msg = format!(
            "plugins rescanned ({} running, {errors} with errors, {} started, {} stopped) in {}",
            report.running,
            report.started,
            report.stopped,
            self.plugins.dir().display()
        );
        self.state.log(&msg);
        if announce {
            let short = format!(
                "plugins rescanned ({} running, {errors} with errors)",
                report.running
            );
            self.state.show_toast(&short);
        }
    }

    fn housekeeping(&mut self, size: Size) {
        let current = mtime(&self.path);
        if current != self.last_mtime {
            self.last_mtime = current;
            self.reload();
        }
        let every = self.state.config.plugins.rescan_interval_s;
        if every > 0 && self.last_rescan.elapsed() >= Duration::from_secs(every) {
            self.last_rescan = Instant::now();
            let report = self.plugins.rescan();
            self.note_scan(&report);
            if report.started + report.stopped > 0 {
                self.state.log(&format!(
                    "plugins: {} started, {} stopped in {}",
                    report.started,
                    report.stopped,
                    self.plugins.dir().display()
                ));
            }
        }
        if size != self.last_size {
            self.last_size = size;
            self.state.dirty = true;
        }
    }

    fn reload(&mut self) {
        let (config, status) = match config::load(&self.path) {
            LoadOutcome::Syntax { line, message } => {
                let where_ = line.map_or(String::new(), |l| format!(" line {l}"));
                self.state.log(&format!(
                    "error: config{where_}: {message}; keeping the last good settings"
                ));
                self.state.config_status = ConfigStatus::Syntax { line, message };
                self.state.dirty = true;
                return;
            }
            LoadOutcome::Loaded { config, warnings } => {
                for w in &warnings {
                    self.state.log(&format!("warning: config: {w}"));
                }
                let status = if warnings.is_empty() {
                    ConfigStatus::Ok
                } else {
                    ConfigStatus::Warnings(warnings)
                };
                (config, status)
            }
            LoadOutcome::Missing => (Config::default(), ConfigStatus::Ok),
        };
        let mut config = config;
        config::apply_flags(&mut config, &self.flags);
        if self.state.config_status != status {
            self.state.dirty = true;
        }
        self.state.config_status = status;
        self.adopt(config, true);
    }

    fn adopt(&mut self, new: Config, announce: bool) {
        let old = std::mem::replace(&mut self.state.config, new);
        let new = &self.state.config;
        if looks_different(&old, new) {
            let idx = app::theme_index(&new.general.theme);
            if self.state.overlay == Overlay::Themes {
                // Keep the preview; the file's theme becomes what Esc goes back to.
                self.state.picker_original = idx;
            } else {
                self.state.theme_idx = idx;
            }
            self.state.dirty = true;
        }
        let intervals = MetricsIntervals::from_config(new);
        if MetricsIntervals::from_config(&old) != intervals {
            let _ = self.metrics.send(WorkerCmd::Reconfigure(intervals));
        }
        let net = NetworkSettings::from_config(new);
        if NetworkSettings::from_config(&old) != net {
            let _ = self.network.send(WorkerCmd::Reconfigure(net));
        }
        if old.plugins != new.plugins
            || old.plugin_cfg != new.plugin_cfg
            || old.units != new.units
            || old.general.plugins_dir != new.general.plugins_dir
        {
            let report = self.plugins.reconfigure(new);
            self.note_scan(&report);
        }
        if old.general.log_file != self.state.config.general.log_file {
            self.state.open_log_file();
        }
        if announce {
            self.state.log("config reloaded");
        }
    }
}

fn looks_different(old: &Config, new: &Config) -> bool {
    old.general.theme != new.general.theme
        || old.general.fps != new.general.fps
        || old.units != new.units
        || old.thresholds != new.thresholds
        || old.theme_matrix != new.theme_matrix
        || old.theme_minimalist != new.theme_minimalist
        || old.disks != new.disks
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Loop {
        /// A loop without a terminal, worker threads or plugins.
        fn for_test(cfg: Config, flags: Flags, path: std::path::PathBuf) -> Self {
            let mut quiet = cfg.clone();
            quiet.plugins.enabled = false;
            let plugin_settings = path.clone();
            Self {
                state: AppState::new(cfg, ConfigStatus::Ok),
                flags,
                last_mtime: mtime(&path),
                path,
                last_size: Size::new(80, 24),
                quit: false,
                metrics: mpsc::channel().0,
                themes: themes::all(),
                plugins: Manager::new(&quiet, &plugin_settings, mpsc::channel().0),
                last_rescan: Instant::now(),
                network: mpsc::channel().0,
            }
        }
    }

    fn temp_file(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("telemetrix-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("telemetrix.toml")
    }

    fn saved_theme(path: &Path) -> String {
        let text = std::fs::read_to_string(path).unwrap();
        config::parse_text(&text).unwrap().config.general.theme
    }

    #[test]
    fn picker_esc_restores_the_theme_and_writes_nothing() {
        let path = temp_file("picker-esc");
        let mut lp = Loop::for_test(Config::default(), Flags::default(), path.clone());
        assert_eq!(lp.state.theme_name(), "matrix");
        lp.act(Action::OpenThemes);
        assert_eq!(lp.state.overlay, Overlay::Themes);
        lp.act(Action::PickerMove(1));
        assert_eq!(lp.state.theme_name(), "minimalist", "live preview");
        lp.act(Action::PickerCancel);
        assert_eq!(lp.state.theme_name(), "matrix");
        assert_eq!(lp.state.overlay, Overlay::None);
        assert!(!path.exists(), "a cancelled preview writes nothing");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn picker_enter_saves_and_beats_the_theme_flag_after_reload() {
        let path = temp_file("picker-enter");
        let flags = Flags {
            theme: Some("minimalist".into()),
            ..Flags::default()
        };
        let mut cfg = Config::default();
        config::apply_flags(&mut cfg, &flags);
        let mut lp = Loop::for_test(cfg, flags, path.clone());
        assert_eq!(lp.state.theme_name(), "minimalist");
        lp.act(Action::OpenThemes);
        lp.act(Action::PickerMove(1));
        lp.act(Action::PickerSave);
        assert_eq!(lp.state.theme_name(), "matrix");
        assert_eq!(lp.state.overlay, Overlay::None);
        assert_eq!(saved_theme(&path), "matrix");
        assert!(
            lp.state
                .toast
                .as_ref()
                .is_some_and(|(t, _)| t == "theme: matrix saved")
        );
        lp.reload();
        assert_eq!(
            lp.state.theme_name(),
            "matrix",
            "--theme must not win after the user chose"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn q_in_the_picker_does_not_quit() {
        let path = temp_file("picker-q");
        let mut lp = Loop::for_test(Config::default(), Flags::default(), path.clone());
        lp.act(Action::OpenThemes);
        let q = ratatui::crossterm::event::KeyEvent::new(
            ratatui::crossterm::event::KeyCode::Char('q'),
            ratatui::crossterm::event::KeyModifiers::NONE,
        );
        lp.act(app::key_action(&q, lp.state.overlay, false));
        assert!(!lp.quit);
        assert_eq!(lp.state.overlay, Overlay::Themes);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn reload_during_preview_moves_the_cancel_target() {
        let path = temp_file("picker-reload");
        let mut lp = Loop::for_test(Config::default(), Flags::default(), path.clone());
        lp.act(Action::OpenThemes);
        lp.act(Action::PickerMove(1));
        assert_eq!(lp.state.theme_name(), "minimalist");
        config::set(&path, "general.theme", &Value::Str("minimalist".into())).unwrap();
        lp.reload();
        assert_eq!(lp.state.overlay, Overlay::Themes, "the picker stays open");
        lp.act(Action::PickerMove(1));
        assert_eq!(lp.state.theme_name(), "matrix", "the preview keeps working");
        lp.act(Action::PickerCancel);
        assert_eq!(
            lp.state.theme_name(),
            "minimalist",
            "Esc goes back to the file's theme"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    fn press(lp: &mut Loop, code: ratatui::crossterm::event::KeyCode) {
        let key = ratatui::crossterm::event::KeyEvent::new(
            code,
            ratatui::crossterm::event::KeyModifiers::NONE,
        );
        let action = if lp.state.text_input.is_some() {
            app::input_key_action(&key)
        } else {
            app::key_action(&key, lp.state.overlay, false)
        };
        lp.act(action);
    }

    fn weather_schema() -> Vec<crate::plugins::schema::SchemaEntry> {
        use crate::plugins::schema::{SchemaEntry, SchemaKind};
        vec![
            SchemaEntry {
                key: "city".into(),
                label: "city".into(),
                kind: SchemaKind::Text,
                default: Value::Str("Kyiv".into()),
            },
            SchemaEntry {
                key: "days".into(),
                label: "days".into(),
                kind: SchemaKind::Int {
                    min: 1,
                    max: 3,
                    step: 1,
                },
                default: Value::Int(3),
            },
        ]
    }

    /// Opens the overlay with the cursor on the weather plugin's `key` row.
    fn open_on(lp: &mut Loop, key: &str) {
        lp.state
            .plugin_schemas
            .insert("weather".into(), weather_schema());
        lp.state.overlay = Overlay::Settings;
        lp.state.plugin_ids = vec!["weather".into()];
        let rows = settings_overlay::rows(&lp.state.plugin_ids, &lp.state.plugin_schemas);
        let sel = settings_overlay::selectable(&rows);
        lp.state.settings_cursor = sel
            .iter()
            .position(|i| {
                matches!(&rows[*i], settings_overlay::Row::PluginSetting { entry, .. } if entry.key == key)
            })
            .unwrap();
    }

    #[test]
    fn text_setting_is_typed_saved_and_keys_are_captured() {
        use ratatui::crossterm::event::KeyCode;
        let path = temp_file("text-input");
        let mut lp = Loop::for_test(Config::default(), Flags::default(), path.clone());
        open_on(&mut lp, "city");
        press(&mut lp, KeyCode::Enter);
        let input = lp.state.text_input.as_ref().expect("Enter opens the input");
        assert_eq!(input.text, "Kyiv", "it starts with the current value");
        for _ in 0..4 {
            press(&mut lp, KeyCode::Backspace);
        }
        for c in "Lviv qts".chars() {
            press(&mut lp, KeyCode::Char(c));
        }
        assert!(!lp.quit, "q does not quit while typing");
        assert_eq!(lp.state.overlay, Overlay::Settings, "t and s do nothing");
        for _ in 0..4 {
            press(&mut lp, KeyCode::Backspace);
        }
        press(&mut lp, KeyCode::Enter);
        assert!(lp.state.text_input.is_none());
        let text = std::fs::read_to_string(&path).unwrap();
        let saved = config::parse_text(&text).unwrap().config;
        assert_eq!(
            saved.plugin_cfg["weather"].settings["city"].as_str(),
            Some("Lviv")
        );
        assert_eq!(
            lp.state.config.plugin_cfg["weather"].settings["city"].as_str(),
            Some("Lviv"),
            "the running settings have it too"
        );

        press(&mut lp, KeyCode::Enter);
        press(&mut lp, KeyCode::Char('x'));
        press(&mut lp, KeyCode::Esc);
        assert!(lp.state.text_input.is_none());
        assert_eq!(
            lp.state.overlay,
            Overlay::Settings,
            "Esc only closes the input"
        );
        assert_eq!(
            lp.state.config.plugin_cfg["weather"].settings["city"].as_str(),
            Some("Lviv"),
            "Esc saves nothing"
        );
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn int_setting_steps_and_saves() {
        use ratatui::crossterm::event::KeyCode;
        let path = temp_file("schema-int");
        let mut lp = Loop::for_test(Config::default(), Flags::default(), path.clone());
        open_on(&mut lp, "days");
        press(&mut lp, KeyCode::Right);
        assert!(
            lp.state.text_input.is_none(),
            "only text rows open the input"
        );
        assert_eq!(
            lp.state.config.plugin_cfg["weather"].settings["days"].as_integer(),
            Some(1),
            "3 wraps to 1"
        );
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("days = 1"), "{text}");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn static_theme_wakes_for_housekeeping_and_drain_only() {
        let now = Instant::now();
        let cfg = Config::default();
        assert_eq!(themes::minimalist::Minimalist.frame_interval(&cfg), None);
        let house = now + HOUSEKEEPING;
        assert_eq!(next_deadline(now, None, house), now + DRAIN);
        let soon = now + Duration::from_millis(100);
        assert_eq!(next_deadline(now, None, soon), soon);
    }

    #[test]
    fn animated_theme_wakes_for_frames() {
        let now = Instant::now();
        let mut cfg = Config::default();
        cfg.general.fps = 20;
        let iv = themes::matrix::Matrix::with_seed(1)
            .frame_interval(&cfg)
            .expect("matrix is animated");
        assert_eq!(iv, Duration::from_millis(50));
        assert_eq!(
            next_deadline(now, Some(now + iv), now + HOUSEKEEPING),
            now + iv
        );
    }

    #[test]
    fn reload_diff_flags_visual_changes_only() {
        let old = Config::default();
        let mut new = old.clone();
        new.metrics.cpu_interval_ms = 5000;
        assert!(!looks_different(&old, &new));
        new.general.fps = 30;
        assert!(looks_different(&old, &new));
    }
}
