use std::io;
use std::path::Path;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant, SystemTime};

use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::Size;

use crate::app::{self, Action, AppState, InputKey, Overlay, SearchBox, TextInput};
use crate::cli::Flags;
use crate::config::View;
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
use crate::ui::settings_overlay::{self, Change, RowAction};
use crate::ui::theme_options::{self, ThemePanel};
use crate::ui::update_group::{Job, Msg};
use crate::update::{self, CheckError, Pending, Updater, Version};

/// How often an open dashboard looks for a newer program (decision 53).
const UPDATE_LOOK: Duration = Duration::from_secs(3600);

/// How the dashboard ended.
#[derive(Debug, PartialEq, Eq)]
pub enum Exit {
    Quit,
    /// `u`: start the new program in this console.
    Restart,
}

/// What the dashboard needs to notice a newer program on disk.
struct UpdateWatch {
    exe: std::path::PathBuf,
    /// Size and time of the program file when this dashboard started.
    started: Option<(u64, SystemTime)>,
    next_look: Instant,
    restart: bool,
    /// Off in tests, which must never run the real `schtasks`.
    refresh_watcher: bool,
}

impl UpdateWatch {
    fn new(exe: &Path) -> Self {
        Self {
            exe: exe.to_path_buf(),
            started: update::stamp(exe),
            next_look: Instant::now(),
            restart: false,
            refresh_watcher: true,
        }
    }
}

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
    updates: UpdateWatch,
    /// Where the background check and install of the `s` box report to.
    events: mpsc::Sender<AppEvent>,
    updater: Arc<dyn Updater>,
}

/// `exe` is this program's file as it was at start; an update replaces it.
pub fn run(cfg: Config, status: ConfigStatus, flags: &Flags, exe: &Path) -> io::Result<Exit> {
    crate::term::install_signal_handling();
    let path = config::resolve_path(flags.config.as_deref());
    let (tx, rx) = mpsc::channel::<AppEvent>();
    let home = bundled::home(&path);
    let synced = bundled::sync(&home, &[], false);
    let plugins = Manager::new(&cfg, &path, tx.clone());
    let network = network::spawn(NetworkSettings::from_config(&cfg), tx.clone());
    let events = tx.clone();
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
        updates: UpdateWatch::new(exe),
        events,
        updater: Arc::new(crate::commands::update_cmd::GitHub),
    };
    lp.rescan_plugins(false);
    let result = event_loop(&mut lp, &mut guard, &rx);
    let _ = lp.metrics.send(WorkerCmd::Stop);
    let _ = lp.network.send(WorkerCmd::Stop);
    lp.plugins.stop_all();
    result?;
    Ok(if lp.updates.restart {
        Exit::Restart
    } else {
        Exit::Quit
    })
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
                    let action = if lp.state.text_input.is_some() || lp.state.search_box.is_some() {
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
        lp.search_tick(Instant::now());
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
        lp.look_for_update(now);
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

/// The install of the `s` box: progress when the shown step changes, then
/// the result. The watcher's copy follows a good install.
fn install_in_background(up: &dyn Updater, release: &update::Release, tx: &mpsc::Sender<AppEvent>) {
    let mut shown = None;
    let result = up.install(release, &mut |step| {
        let job = Job::of(step);
        if shown != Some(job) {
            shown = Some(job);
            let _ = tx.send(AppEvent::Update(Msg::Progress(step)));
        }
    });
    let msg = match result {
        Ok(v) => {
            if let Err(e) = up.after_install() {
                let _ = tx.send(AppEvent::Log(format!("warning: update: {e}")));
            }
            Msg::Installed(v)
        }
        Err(e) => Msg::InstallFailed(e),
    };
    let _ = tx.send(AppEvent::Update(msg));
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
                let rows = settings_overlay::rows_for(s);
                let last = settings_overlay::selectable(&rows).len().saturating_sub(1);
                s.settings_cursor = (s.settings_cursor + 1).min(last);
                s.dirty = true;
            }
            Action::SettingsStep { dir, big } => {
                let rows = settings_overlay::rows_for(s);
                let sel = settings_overlay::selectable(&rows);
                let row = &rows[sel[s.settings_cursor.min(sel.len() - 1)]];
                if let Some((id, entry)) = settings_overlay::search_row(row) {
                    if dir > 0 {
                        s.search_box = Some(SearchBox::new(id, &entry.key, &entry.label));
                        s.dirty = true;
                    }
                } else if let Some((id, key)) = settings_overlay::text_row(row) {
                    if dir > 0 {
                        let text = settings_overlay::text_value(row, &s.config);
                        s.text_input = Some(TextInput::new(id, key, &text));
                        s.dirty = true;
                    }
                } else if let Some(change) = settings_overlay::step(row, &s.config, dir, big) {
                    self.change(change);
                }
            }
            Action::SettingsEnter { big } => {
                let rows = settings_overlay::rows_for(s);
                let sel = settings_overlay::selectable(&rows);
                let row = &rows[sel[s.settings_cursor.min(sel.len() - 1)]];
                match settings_overlay::action_row(row) {
                    Some(action) => self.run_row_action(action),
                    None => self.act(Action::SettingsStep { dir: 1, big }),
                }
            }
            Action::Input(InputKey::Cancel) => {
                s.text_input = None;
                s.search_box = None;
                s.dirty = true;
            }
            Action::Input(InputKey::Save) if s.search_box.is_some() => {
                let chosen = s.search_box.as_ref().and_then(|b| {
                    let id = b.input.id.clone();
                    b.chosen().map(|c| (id, c.values.clone()))
                });
                if let Some((id, values)) = chosen {
                    s.search_box = None;
                    let sets = values
                        .into_iter()
                        .map(|(k, v)| (format!("plugin.{id}.{k}"), v))
                        .collect();
                    self.change_many(sets);
                    self.plugins.run_now(&id);
                }
            }
            Action::Input(key) if s.search_box.is_some() => {
                if let Some(b) = &mut s.search_box {
                    b.key(key, Instant::now());
                    s.dirty = true;
                }
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
            Action::UpdateRestart => self.update_restart(),
            Action::CycleCpuView => self.cycle_cpu_view(),
            Action::OpenThemeOptions => {
                s.theme_panel = Some(ThemePanel {
                    theme: THEME_NAMES[s.theme_idx].to_string(),
                    cursor: 0,
                    before: s.config.clone(),
                    changed: Vec::new(),
                });
                s.overlay = Overlay::ThemeOptions;
                s.dirty = true;
            }
            Action::PanelMove(dir) => {
                if let Some(p) = &s.theme_panel {
                    let last = theme_options::rows(s, p).len().saturating_sub(1);
                    let cursor = p.cursor.saturating_add_signed(dir as isize).min(last);
                    if let Some(p) = s.theme_panel.as_mut() {
                        p.cursor = cursor;
                    }
                    s.dirty = true;
                }
            }
            Action::PanelStep(dir) => self.panel_step(dir, false),
            Action::PanelEnter => self.panel_step(1, true),
            Action::PanelToggle => self.panel_toggle(),
            Action::PanelShift(dir) => self.panel_shift(dir),
            Action::PanelSave => self.panel_save(),
            Action::PanelCancel => {
                if let Some(p) = s.theme_panel.take() {
                    let before = p.before;
                    self.adopt(before, false);
                }
                self.state.overlay = Overlay::Themes;
                self.state.dirty = true;
            }
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
        self.saved(saved);
    }

    /// Applies several settings at once and saves them in one write.
    fn change_many(&mut self, sets: Vec<(String, Value)>) {
        let mut new = self.state.config.clone();
        for (key, value) in &sets {
            config::release_flag(&mut self.flags, key);
            settings_overlay::apply(&mut new, &Change::Set(key.clone(), value.clone()));
        }
        self.adopt(new, false);
        let saved = config::set_many(&self.path, &sets);
        self.saved(saved);
    }

    /// Notes the result of a save in the overlay footer.
    fn saved(&mut self, saved: io::Result<()>) {
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

    /// Sends the search box's query to its plugin once typing has paused,
    /// and keeps the spinner turning while an answer is pending.
    fn search_tick(&mut self, now: Instant) {
        let Some(b) = &mut self.state.search_box else {
            return;
        };
        if let Some(query) = b.due(now)
            && !self.plugins.search(&b.input.id, &query)
        {
            b.answer(
                &query,
                Err("the plugin is not running; turn it on first".into()),
            );
        }
        if b.waiting() {
            self.state.dirty = true;
        }
    }

    /// What the empty Plugins card needs: the folder and whether anything runs.
    fn note_scan(&mut self, report: &ScanReport) {
        self.state.plugins_dir = self.plugins.dir().to_path_buf();
        self.state.plugins_running = report.running;
        self.state.dirty = true;
        self.check_card_ids();
    }

    /// One log warning for each `hide`/`order` entry that names no card.
    fn check_card_ids(&mut self) {
        let mut known = manager::known_card_ids(self.plugins.dir());
        known.extend(self.state.plugins.keys().cloned());
        for (key, id) in config::unknown_cards(&self.state.config, &known) {
            if self.state.warned_cards.insert((key.clone(), id.clone())) {
                self.state
                    .log(&format!("warning: {key}: no card {id:?}, ignored"));
            }
        }
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

    /// `v`: bar, chart, both, bar... for the current theme, saved at once.
    fn cycle_cpu_view(&mut self) {
        let theme = self.state.theme_name().to_string();
        let next = match self.state.config.theme(&theme).cpu_view {
            View::Bar => View::Chart,
            View::Chart => View::Both,
            View::Both => View::Bar,
        };
        let key = format!("theme.{theme}.cpu_view");
        self.change(Change::Set(key, Value::Str(next.name().into())));
        let note = if crate::themes::uses_option(&theme, "cpu_view") {
            ""
        } else {
            ", not used by this theme"
        };
        self.state
            .show_toast(&format!("cpu view: {} ({theme}{note})", next.name()));
    }

    /// Applies one panel change to the settings in memory only: a preview
    /// until the panel saves.
    fn panel_preview(&mut self, key: String, value: Value) {
        let mut new = self.state.config.clone();
        settings_overlay::apply(&mut new, &Change::Set(key.clone(), value));
        self.adopt(new, false);
        if let Some(p) = self.state.theme_panel.as_mut()
            && !p.changed.contains(&key)
        {
            p.changed.push(key);
        }
        self.state.dirty = true;
    }

    fn panel_step(&mut self, dir: i32, enter: bool) {
        let Some(p) = &self.state.theme_panel else {
            return;
        };
        let rows = theme_options::rows(&self.state, p);
        let Some(row) = rows.get(p.cursor) else {
            return;
        };
        if enter && matches!(row, theme_options::PanelRow::Save) {
            self.panel_save();
            return;
        }
        if let Some((key, value)) = theme_options::step(row, &self.state.config, &p.theme, dir) {
            self.panel_preview(key, value);
        }
    }

    fn panel_toggle(&mut self) {
        let Some(p) = &self.state.theme_panel else {
            return;
        };
        let rows = theme_options::rows(&self.state, p);
        if let Some(row @ theme_options::PanelRow::Card(_)) = rows.get(p.cursor)
            && let Some((key, value)) = theme_options::step(row, &self.state.config, &p.theme, 1)
        {
            self.panel_preview(key, value);
        }
    }

    fn panel_shift(&mut self, dir: i32) {
        let Some(p) = &self.state.theme_panel else {
            return;
        };
        let rows = theme_options::rows(&self.state, p);
        let first_card = rows
            .iter()
            .position(|r| matches!(r, theme_options::PanelRow::Card(_)))
            .unwrap_or(0);
        let cards: Vec<String> = rows
            .iter()
            .filter_map(|r| match r {
                theme_options::PanelRow::Card(id) => Some(id.clone()),
                _ => None,
            })
            .collect();
        let Some(index) = p
            .cursor
            .checked_sub(first_card)
            .filter(|i| *i < cards.len())
        else {
            return;
        };
        let Some(order) = theme_options::moved(&cards, index, dir) else {
            return;
        };
        let key = format!("theme.{}.order", p.theme);
        let cursor = p.cursor.saturating_add_signed(dir as isize);
        self.panel_preview(key, Value::List(order));
        if let Some(p) = self.state.theme_panel.as_mut() {
            p.cursor = cursor;
        }
    }

    /// Saves every key the panel changed, for its theme only, in one write.
    fn panel_save(&mut self) {
        let Some(p) = self.state.theme_panel.take() else {
            return;
        };
        let sets: Vec<(String, Value)> = p
            .changed
            .iter()
            .filter_map(|k| Some((k.clone(), self.state.config.get(k)?)))
            .collect();
        let theme = p.theme;
        self.state.overlay = Overlay::Themes;
        if sets.is_empty() {
            self.state
                .show_toast(&format!("{theme} options: nothing changed"));
            return;
        }
        self.change_many(sets);
        let saved = matches!(self.state.settings_footer, Some(Ok(_)));
        let note = if saved {
            "saved"
        } else {
            "not saved, see the log"
        };
        self.state.show_toast(&format!("{theme} options {note}"));
    }

    /// Looks for a newer program on disk or a ready download, once an hour.
    fn look_for_update(&mut self, now: Instant) {
        let u = &mut self.updates;
        if now < u.next_look {
            return;
        }
        u.next_look = now + UPDATE_LOOK;
        let found = update::pending(&u.exe, u.started, &Version::current());
        if found != self.state.update {
            if let Some(p) = &found {
                let (Pending::Ready(v) | Pending::Installed(v)) = p;
                self.state.log(&format!(
                    "update: telemetrix {v} is ready; press u to restart"
                ));
            }
            self.state.update = found;
            self.state.dirty = true;
        }
    }

    /// Enter on an action row of the `s` box. A check or an install runs in
    /// its own thread and reports with `AppEvent::Update`.
    fn run_row_action(&mut self, action: RowAction) {
        let (tx, up) = (self.events.clone(), Arc::clone(&self.updater));
        let group = &mut self.state.update_group;
        let spawned = match action {
            RowAction::CheckUpdate => {
                if !group.start_check() {
                    self.state.show_toast("already running");
                    return;
                }
                std::thread::Builder::new()
                    .name("update-check".into())
                    .spawn(move || {
                        let _ = tx.send(AppEvent::Update(Msg::Checked(up.latest())));
                    })
            }
            RowAction::InstallUpdate => {
                let release = match group.start_install(&Version::current()) {
                    Ok(r) => r,
                    Err(why) => {
                        self.state.show_toast(why);
                        return;
                    }
                };
                self.state.log(&format!(
                    "update: installing {} from the s box",
                    release.tag
                ));
                std::thread::Builder::new()
                    .name("update-install".into())
                    .spawn(move || install_in_background(&*up, &release, &tx))
            }
        };
        if let Err(e) = spawned {
            let why = format!("cannot start a thread: {e}");
            let msg = match action {
                RowAction::CheckUpdate => Msg::Checked(Err(CheckError::Failed(why))),
                RowAction::InstallUpdate => Msg::InstallFailed(why),
            };
            self.state.apply(AppEvent::Update(msg));
        }
        self.state.dirty = true;
    }

    /// `u`: installs a ready download if needed, then ends the loop so the
    /// new program starts in this console.
    fn update_restart(&mut self) {
        let exe = self.updates.exe.clone();
        match self.state.update.clone() {
            None => {}
            Some(Pending::Installed(_)) => {
                self.updates.restart = true;
                self.quit = true;
            }
            Some(Pending::Ready(_)) => match update::install_ready(&exe, &Version::current()) {
                Ok(v) => {
                    self.state
                        .log(&format!("update: installed telemetrix {v}; restarting"));
                    if self.updates.refresh_watcher
                        && crate::commands::screensaver::task_installed(None)
                        && let Err(e) = crate::commands::screensaver::refresh_watcher(&exe, false)
                    {
                        self.state.log(&format!("warning: update: {e}"));
                    }
                    self.updates.restart = true;
                    self.quit = true;
                }
                Err(e) => {
                    self.state.log(&format!("warning: update: {e}"));
                    self.state.show_toast("update failed, see the log (l)");
                    self.state.update = None;
                }
            },
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
            if matches!(self.state.overlay, Overlay::Themes | Overlay::ThemeOptions) {
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
            || old.commands != new.commands
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
        || old.theme_opts != new.theme_opts
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
                updates: UpdateWatch {
                    refresh_watcher: false,
                    ..UpdateWatch::new(Path::new("telemetrix-test-no-such-exe"))
                },
                events: mpsc::channel().0,
                updater: Arc::new(FakeUpdater::new("v0.0.1", Ok(()))),
            }
        }
    }

    /// An updater without the network: a fixed latest release, and an
    /// install that reports every step and then succeeds or fails.
    struct FakeUpdater {
        latest: update::Release,
        install: Result<(), String>,
    }

    impl FakeUpdater {
        fn new(tag: &str, install: Result<(), String>) -> Self {
            Self {
                latest: update::Release {
                    tag: tag.into(),
                    version: Version::parse(tag).unwrap(),
                    assets: Vec::new(),
                },
                install,
            }
        }
    }

    impl Updater for FakeUpdater {
        fn latest(&self) -> Result<update::Release, CheckError> {
            Ok(self.latest.clone())
        }

        fn install(
            &self,
            release: &update::Release,
            on: &mut dyn FnMut(update::Step),
        ) -> Result<Version, String> {
            for done in [0, 10, 42, 42, 100] {
                on(update::Step::Downloading {
                    done,
                    total: Some(100),
                });
            }
            on(update::Step::Verifying);
            self.install.clone()?;
            on(update::Step::Installing);
            Ok(release.version.clone())
        }

        fn after_install(&self) -> Result<(), String> {
            Err("no watcher here".into())
        }
    }

    /// The loop with `updater`, the `s` box open on `label`, and the channel
    /// its background threads report to.
    fn update_loop(updater: FakeUpdater, name: &str) -> (Loop, mpsc::Receiver<AppEvent>) {
        let mut lp = Loop::for_test(Config::default(), Flags::default(), temp_file(name));
        let (tx, rx) = mpsc::channel();
        lp.events = tx;
        lp.updater = Arc::new(updater);
        lp.act(Action::ToggleSettings);
        (lp, rx)
    }

    fn cursor_to(lp: &mut Loop, label: &str) {
        let rows = settings_overlay::rows_for(&lp.state);
        let sel = settings_overlay::selectable(&rows);
        lp.state.settings_cursor = sel
            .iter()
            .position(|&i| matches!(&rows[i], settings_overlay::Row::Action { label: l, .. } if *l == label))
            .unwrap_or_else(|| panic!("no {label} row"));
    }

    /// Applies what the background thread sent, until `last` came.
    fn drain(lp: &mut Loop, rx: &mpsc::Receiver<AppEvent>, last: fn(&AppEvent) -> bool) -> usize {
        let mut n = 0;
        loop {
            let ev = rx.recv_timeout(Duration::from_secs(10)).expect("an answer");
            let done = last(&ev);
            lp.state.apply(ev);
            n += 1;
            if done {
                return n;
            }
        }
    }

    fn toast(lp: &Loop) -> &str {
        lp.state.toast.as_ref().map_or("", |(t, _)| t)
    }

    /// The real "check now" against GitHub, read-only; nothing is installed.
    /// `cargo test real_check_now -- --ignored --nocapture`
    #[test]
    #[ignore = "asks api.github.com"]
    fn real_check_now_asks_github() {
        use ratatui::crossterm::event::KeyCode;
        let (mut lp, rx) = update_loop(FakeUpdater::new("v0.0.1", Ok(())), "update-real");
        lp.updater = Arc::new(crate::commands::update_cmd::GitHub);
        cursor_to(&mut lp, "check now");
        press(&mut lp, KeyCode::Enter);
        drain(&mut lp, &rx, |e| {
            matches!(e, AppEvent::Update(Msg::Checked(_)))
        });
        for row in settings_overlay::update_rows(&lp.state) {
            println!(
                "{:<12} {}",
                match &row {
                    settings_overlay::Row::Info { label, .. }
                    | settings_overlay::Row::Action { label, .. } => *label,
                    _ => "",
                },
                settings_overlay::value_text(&row, &lp.state.config)
            );
        }
        println!("toast: {}", toast(&lp));
        for line in &lp.state.log {
            println!("log: {}", line.text);
        }
        assert_eq!(lp.state.update, None, "a check installs nothing");
    }

    #[test]
    fn enter_runs_an_action_row_and_left_right_do_nothing() {
        use ratatui::crossterm::event::KeyCode;
        let (mut lp, rx) = update_loop(FakeUpdater::new("v0.0.1", Ok(())), "update-row");
        cursor_to(&mut lp, "check now");
        let before = lp.state.config.clone();
        press(&mut lp, KeyCode::Left);
        press(&mut lp, KeyCode::Right);
        assert_eq!(
            lp.state.update_group.job,
            Job::Idle,
            "Left/Right start nothing"
        );
        assert!(rx.try_recv().is_err());
        assert_eq!(lp.state.config, before);
        press(&mut lp, KeyCode::Enter);
        assert_eq!(lp.state.update_group.job, Job::Checking);
        press(&mut lp, KeyCode::Enter);
        assert_eq!(toast(&lp), "already running");
        drain(&mut lp, &rx, |e| {
            matches!(e, AppEvent::Update(Msg::Checked(_)))
        });
        assert_eq!(lp.state.update_group.job, Job::Idle);
        assert_eq!(toast(&lp), "this build is newer than v0.0.1");
        let rows = settings_overlay::rows_for(&lp.state);
        assert!(
            !rows.iter().any(|r| matches!(
                r,
                settings_overlay::Row::Action {
                    label: "install now",
                    ..
                }
            )),
            "nothing newer to install"
        );
    }

    #[test]
    fn check_then_install_from_the_s_box_ends_with_u_restart() {
        use ratatui::crossterm::event::KeyCode;
        let (mut lp, rx) = update_loop(FakeUpdater::new("v99.0.0", Ok(())), "update-install");
        cursor_to(&mut lp, "check now");
        press(&mut lp, KeyCode::Enter);
        drain(&mut lp, &rx, |e| {
            matches!(e, AppEvent::Update(Msg::Checked(_)))
        });
        assert_eq!(toast(&lp), "v99.0.0 available");
        cursor_to(&mut lp, "install now");
        press(&mut lp, KeyCode::Enter);
        cursor_to(&mut lp, "check now");
        press(&mut lp, KeyCode::Enter);
        assert_eq!(toast(&lp), "already running", "no check during an install");
        let n = drain(&mut lp, &rx, |e| {
            matches!(
                e,
                AppEvent::Update(Msg::Installed(_) | Msg::InstallFailed(_))
            )
        });
        // Downloading 0, 10, 42 and 100 %, verifying, installing, the watcher
        // warning and the result: a repeated 42 % is not sent again.
        assert_eq!(n, 8);
        let v = Version::parse("99.0.0").unwrap();
        assert_eq!(lp.state.update, Some(Pending::Installed(v)));
        assert_eq!(toast(&lp), "update ready · u restart");
        assert!(
            lp.state
                .log
                .iter()
                .any(|l| l.text == "update: no watcher here"),
            "the watcher refresh failure is logged"
        );
        press(&mut lp, KeyCode::Esc);
        press(&mut lp, KeyCode::Char('u'));
        assert!(lp.quit && lp.updates.restart, "u restarts into it");
    }

    #[test]
    fn a_failed_install_changes_nothing_and_says_why() {
        use ratatui::crossterm::event::KeyCode;
        let bad = Err("checksum mismatch for x; it was deleted and nothing was changed".into());
        let (mut lp, rx) = update_loop(FakeUpdater::new("v99.0.0", bad), "update-fail");
        cursor_to(&mut lp, "check now");
        press(&mut lp, KeyCode::Enter);
        drain(&mut lp, &rx, |e| {
            matches!(e, AppEvent::Update(Msg::Checked(_)))
        });
        cursor_to(&mut lp, "install now");
        press(&mut lp, KeyCode::Enter);
        drain(&mut lp, &rx, |e| {
            matches!(
                e,
                AppEvent::Update(Msg::Installed(_) | Msg::InstallFailed(_))
            )
        });
        assert_eq!(toast(&lp), "install failed: checksum mismatch");
        assert_eq!(lp.state.update, None, "no restart offered");
        assert_eq!(lp.state.update_group.job, Job::Idle);
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
        assert_eq!(lp.state.theme_name(), "tokyo-night", "live preview");
        lp.act(Action::PickerCancel);
        assert_eq!(lp.state.theme_name(), "matrix");
        assert_eq!(lp.state.overlay, Overlay::None);
        assert!(!path.exists(), "a cancelled preview writes nothing");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn the_options_panel_previews_saves_one_theme_and_esc_restores() {
        use ratatui::crossterm::event::KeyCode;
        let path = temp_file("panel");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            "[theme.matrix]
density = 0.3
",
        )
        .unwrap();
        let mut lp = Loop::for_test(Config::default(), Flags::default(), path.clone());
        press(&mut lp, KeyCode::Char('t'));
        while lp.state.theme_name() != "synthwave" {
            press(&mut lp, KeyCode::Down);
        }
        press(&mut lp, KeyCode::Char('o'));
        assert_eq!(lp.state.overlay, Overlay::ThemeOptions);
        // Esc puts every previewed change back and writes nothing.
        press(&mut lp, KeyCode::Right);
        assert_eq!(lp.state.config.theme("synthwave").cpu_view, View::Chart);
        press(&mut lp, KeyCode::Esc);
        assert_eq!(lp.state.overlay, Overlay::Themes);
        assert_eq!(lp.state.config.theme("synthwave").cpu_view, View::Bar);
        assert_eq!(lp.state.theme_name(), "synthwave", "still previewing");
        let untouched = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            untouched,
            "[theme.matrix]
density = 0.3
"
        );

        press(&mut lp, KeyCode::Right);
        press(&mut lp, KeyCode::Right);
        assert_eq!(lp.state.config.theme("synthwave").cpu_view, View::Chart);
        // Rows: 4 options, then the cards cpu, ram, swap...
        for _ in 0..6 {
            press(&mut lp, KeyCode::Down);
        }
        press(&mut lp, KeyCode::Char(' '));
        assert!(lp.state.config.theme("synthwave").hidden("swap"));
        press(&mut lp, KeyCode::Char('['));
        assert_eq!(
            lp.state.config.theme("synthwave").order[..3],
            ["cpu", "swap", "ram"]
        );
        assert_eq!(
            lp.state.theme_panel.as_ref().unwrap().cursor,
            5,
            "follows the card"
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            untouched,
            "only a preview"
        );
        press(&mut lp, KeyCode::Char('s'));
        assert_eq!(lp.state.overlay, Overlay::Themes);
        let text = std::fs::read_to_string(&path).unwrap();
        let parsed = config::parse_text(&text).unwrap();
        let o = parsed.config.theme("synthwave");
        assert_eq!(o.cpu_view, View::Chart);
        assert_eq!(o.hide, ["swap"]);
        assert_eq!(o.order[..3], ["cpu", "swap", "ram"]);
        assert_eq!(parsed.config.theme_matrix.density, 0.3, "{text}");
        assert!(
            !text.contains("theme.minimalist") && !text.contains("general"),
            "{text}"
        );
        assert!(
            lp.state
                .toast
                .as_ref()
                .is_some_and(|(t, _)| t == "synthwave options saved")
        );
        // Leaving the picker without Enter keeps the saved theme choice.
        press(&mut lp, KeyCode::Esc);
        assert_eq!(lp.state.theme_name(), "matrix");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn v_cycles_the_cpu_view_of_the_current_theme_and_saves() {
        use ratatui::crossterm::event::KeyCode;
        let path = temp_file("cycle-view");
        let mut lp = Loop::for_test(Config::default(), Flags::default(), path.clone());
        for want in ["chart", "both", "bar"] {
            press(&mut lp, KeyCode::Char('v'));
            let text = std::fs::read_to_string(&path).unwrap();
            let parsed = config::parse_text(&text).unwrap();
            assert_eq!(parsed.config.theme("matrix").cpu_view.name(), want);
            assert_eq!(parsed.config.theme("minimalist").cpu_view, View::Bar);
            let toast = format!("cpu view: {want} (matrix)");
            assert!(lp.state.toast.as_ref().is_some_and(|(t, _)| *t == toast));
        }
        lp.state.overlay = Overlay::Help;
        press(&mut lp, KeyCode::Char('v'));
        assert_eq!(
            lp.state.config.theme("matrix").cpu_view,
            View::Bar,
            "ignored"
        );
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
        assert_eq!(lp.state.theme_name(), "tokyo-night");
        config::set(&path, "general.theme", &Value::Str("minimalist".into())).unwrap();
        lp.reload();
        assert_eq!(lp.state.overlay, Overlay::Themes, "the picker stays open");
        lp.act(Action::PickerMove(1));
        assert_eq!(
            lp.state.theme_name(),
            "crt-amber",
            "the preview keeps working"
        );
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
        let action = if lp.state.text_input.is_some() || lp.state.search_box.is_some() {
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
                key: "spot".into(),
                label: "spot".into(),
                kind: SchemaKind::Search,
                default: Value::Str("".into()),
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
        let rows = settings_overlay::rows_for(&lp.state);
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
    fn a_picked_search_result_saves_all_its_values_at_once() {
        use crate::plugins::manifest::SearchOption;
        use ratatui::crossterm::event::KeyCode;
        let path = temp_file("search-box");
        let mut lp = Loop::for_test(Config::default(), Flags::default(), path.clone());
        open_on(&mut lp, "spot");
        press(&mut lp, KeyCode::Enter);
        assert!(lp.state.search_box.is_some(), "Enter opens the search box");
        for c in "Lq".chars() {
            press(&mut lp, KeyCode::Char(c));
        }
        assert!(!lp.quit, "q does not quit while searching");
        let later = Instant::now() + app::SEARCH_PAUSE;
        lp.search_tick(later);
        let b = lp.state.search_box.as_ref().unwrap();
        assert_eq!(
            b.error.as_deref(),
            Some("the plugin is not running; turn it on first"),
            "plugins are off in this test"
        );
        press(&mut lp, KeyCode::Backspace);
        press(&mut lp, KeyCode::Char('v'));
        let query = lp
            .state
            .search_box
            .as_mut()
            .unwrap()
            .due(later + app::SEARCH_PAUSE);
        let option = |label: &str, lat: f64| SearchOption {
            label: label.into(),
            values: vec![
                ("lat".into(), Value::Float(lat)),
                ("spot".into(), Value::Str(label.to_string().into())),
            ],
        };
        lp.state.apply(AppEvent::SearchResults {
            id: "weather".into(),
            query: query.unwrap(),
            result: Ok(vec![option("Place A", 1.5), option("Place B", 2.5)]),
        });
        press(&mut lp, KeyCode::Down);
        press(&mut lp, KeyCode::Enter);
        assert!(lp.state.search_box.is_none(), "Enter closes the box");
        let text = std::fs::read_to_string(&path).unwrap();
        let saved = config::parse_text(&text).unwrap().config;
        let w = &saved.plugin_cfg["weather"].settings;
        assert_eq!(w["spot"].as_str(), Some("Place B"));
        assert_eq!(w["lat"].as_float(), Some(2.5));
        assert_eq!(
            lp.state.config.plugin_cfg["weather"].settings["spot"].as_str(),
            Some("Place B"),
            "the running settings have it too"
        );
        press(&mut lp, KeyCode::Enter);
        press(&mut lp, KeyCode::Char('x'));
        press(&mut lp, KeyCode::Esc);
        assert!(lp.state.search_box.is_none());
        assert_eq!(
            lp.state.overlay,
            Overlay::Settings,
            "Esc only closes the box"
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
    fn u_installs_a_ready_update_and_asks_for_a_restart() {
        let path = temp_file("update-ready");
        let dir = path.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("telemetrix.exe");
        std::fs::write(&exe, "running").unwrap();
        let mut lp = Loop::for_test(Config::default(), Flags::default(), path);
        lp.updates = UpdateWatch {
            refresh_watcher: false,
            ..UpdateWatch::new(&exe)
        };
        press(&mut lp, ratatui::crossterm::event::KeyCode::Char('u'));
        assert!(!lp.quit, "u does nothing without an update");

        let download = dir.join("telemetrix.exe.download");
        std::fs::write(&download, "new program").unwrap();
        let d = update::Downloaded {
            sha256: update::sha256_file(&download).unwrap(),
            path: download,
            version: Version::parse("99.0.0").unwrap(),
            bytes: 11,
        };
        update::save_ready(&exe, &d).unwrap();
        let now = Instant::now();
        lp.look_for_update(now);
        assert_eq!(
            lp.state.update,
            Some(Pending::Ready(Version::parse("99.0.0").unwrap()))
        );
        lp.look_for_update(now + Duration::from_secs(60));
        assert!(lp.updates.next_look >= now + UPDATE_LOOK, "once an hour");
        press(&mut lp, ratatui::crossterm::event::KeyCode::Char('u'));
        assert!(lp.quit && lp.updates.restart);
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new program");
        std::fs::remove_dir_all(dir).unwrap();
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
