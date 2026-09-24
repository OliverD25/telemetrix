use std::io;
use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant, SystemTime};

use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Size};
use ratatui::style::{Color, Style};
use ratatui::widgets::Paragraph;

use crate::app::{self, Action, AppState, Overlay};
use crate::cli::Flags;
use crate::config::{self, Config, ConfigStatus, LoadOutcome, THEME_NAMES, Value};
use crate::event::{AppEvent, WorkerCmd};
use crate::metrics::worker::{self, MetricsCmd, MetricsIntervals};
use crate::term::TerminalGuard;

pub const HOUSEKEEPING: Duration = Duration::from_secs(2);
/// `poll` does not wake when a worker sends on the channel, so the wait is
/// capped to pick up new data promptly even on a static theme.
pub const DRAIN: Duration = Duration::from_millis(250);
const PANIC_TEST_AFTER: Duration = Duration::from_secs(1);

pub fn next_deadline(
    now: Instant,
    frame_due: Option<Instant>,
    housekeeping_due: Instant,
) -> Instant {
    let deadline = housekeeping_due.min(now + DRAIN);
    frame_due.map_or(deadline, |f| deadline.min(f))
}

/// `None` = static theme: redraw only when something changed.
pub fn frame_interval(theme: &str, cfg: &Config, paused: bool) -> Option<Duration> {
    (theme == "matrix" && !paused)
        .then(|| Duration::from_secs_f64(1.0 / f64::from(cfg.general.fps.max(1))))
}

struct Loop {
    state: AppState,
    flags: Flags,
    path: std::path::PathBuf,
    last_mtime: Option<SystemTime>,
    last_size: Size,
    quit: bool,
    metrics: mpsc::Sender<MetricsCmd>,
}

pub fn run(cfg: Config, status: ConfigStatus, flags: &Flags) -> io::Result<()> {
    let path = config::resolve_path(flags.config.as_deref());
    let (tx, rx) = mpsc::channel::<AppEvent>();
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
    let mut guard = TerminalGuard::new();
    let mut lp = Loop {
        state,
        flags: flags.clone(),
        last_mtime: mtime(&path),
        path,
        last_size: guard.terminal.size()?,
        quit: false,
        metrics,
    };
    let result = event_loop(&mut lp, &mut guard, &rx);
    let _ = lp.metrics.send(WorkerCmd::Stop);
    result
}

fn event_loop(
    lp: &mut Loop,
    guard: &mut TerminalGuard,
    rx: &mpsc::Receiver<AppEvent>,
) -> io::Result<()> {
    let start = Instant::now();
    let mut next_house = start + HOUSEKEEPING;
    let mut next_frame = start;
    loop {
        let now = Instant::now();
        let interval = frame_interval(lp.state.theme_name(), &lp.state.config, lp.state.paused);
        let frame_due = interval.is_some_and(|_| now >= next_frame);
        if lp.state.dirty || frame_due {
            guard.terminal.draw(|f| draw(f, &lp.state))?;
            lp.state.dirty = false;
            if let Some(iv) = interval {
                next_frame = (next_frame + iv).max(now);
            }
        }
        let deadline = next_deadline(Instant::now(), interval.map(|_| next_frame), next_house);
        if event::poll(deadline.saturating_duration_since(Instant::now()))? {
            match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => {
                    let action = app::key_action(
                        &key,
                        lp.state.overlay,
                        lp.state.config.general.exit_on_any_key,
                    );
                    lp.act(action);
                }
                Ok(Event::Resize(..)) => lp.state.dirty = true,
                Ok(_) => {}
                Err(_) => return Ok(()),
            }
        }
        if lp.quit {
            return Ok(());
        }
        while let Ok(ev) = rx.try_recv() {
            lp.state.apply(ev);
        }
        let now = Instant::now();
        if now >= next_house {
            lp.housekeeping(guard.terminal.size()?);
            next_house = now + HOUSEKEEPING;
        }
        if lp.flags.panic_test && start.elapsed() >= PANIC_TEST_AFTER {
            panic!("--panic-test: deliberate panic to check that the terminal is restored");
        }
    }
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

impl Loop {
    fn act(&mut self, action: Action) {
        let s = &mut self.state;
        match action {
            Action::Quit => self.quit = true,
            Action::NextTheme => self.cycle_theme(1),
            Action::PrevTheme => self.cycle_theme(THEME_NAMES.len() - 1),
            Action::ToggleLog => s.toggle_overlay(Overlay::Log),
            Action::ToggleHelp => s.toggle_overlay(Overlay::Help),
            Action::CloseOverlay => s.toggle_overlay(Overlay::None),
            Action::TogglePause => {
                s.paused = !s.paused;
                s.dirty = true;
            }
            Action::Reload => s.log("reload requested"),
            Action::Nothing => {}
        }
    }

    fn cycle_theme(&mut self, step: usize) {
        let s = &mut self.state;
        s.theme_idx = (s.theme_idx + step) % THEME_NAMES.len();
        let name = THEME_NAMES[s.theme_idx];
        s.config.general.theme = name.to_string();
        s.dirty = true;
        // The user picked a theme, so a --theme flag must stop overriding it on reload.
        self.flags.theme = None;
        if let Err(e) = config::set(&self.path, "general.theme", &Value::Str(name.into())) {
            s.log(&format!("error: cannot save {}: {e}", self.path.display()));
        }
    }

    fn housekeeping(&mut self, size: Size) {
        let current = mtime(&self.path);
        if current != self.last_mtime {
            self.last_mtime = current;
            self.reload();
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
        self.adopt(config);
    }

    fn adopt(&mut self, new: Config) {
        let old = std::mem::replace(&mut self.state.config, new);
        let new = &self.state.config;
        if looks_different(&old, new) {
            self.state.theme_idx = app::theme_index(&new.general.theme);
            self.state.dirty = true;
        }
        let intervals = MetricsIntervals::from_config(new);
        if MetricsIntervals::from_config(&old) != intervals {
            let _ = self.metrics.send(WorkerCmd::Reconfigure(intervals));
        }
        if old.general.log_file != new.general.log_file {
            self.state.open_log_file();
        }
        self.state.log("config reloaded");
    }
}

fn looks_different(old: &Config, new: &Config) -> bool {
    old.general.theme != new.general.theme
        || old.general.fps != new.general.fps
        || old.units != new.units
        || old.thresholds != new.thresholds
        || old.theme_matrix != new.theme_matrix
        || old.theme_minimalist != new.theme_minimalist
}

fn draw(frame: &mut Frame, state: &AppState) {
    let banner = state.banner();
    let [top, body] = Layout::vertical([
        Constraint::Length(u16::from(banner.is_some())),
        Constraint::Fill(1),
    ])
    .areas(frame.area());
    if let Some(text) = banner {
        let style = Style::new().bg(Color::Rgb(135, 20, 20)).fg(Color::White);
        frame.render_widget(Paragraph::new(format!(" {text}")).style(style), top);
    }
    let text = if state.snapshot.is_some() {
        ""
    } else {
        "telemetrix — waiting for metrics"
    };
    let [middle] = Layout::vertical([Constraint::Length(1)])
        .flex(ratatui::layout::Flex::Center)
        .areas(body);
    frame.render_widget(Paragraph::new(text).centered(), middle);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_theme_wakes_for_housekeeping_and_drain_only() {
        let now = Instant::now();
        let cfg = Config::default();
        assert_eq!(frame_interval("minimalist", &cfg, false), None);
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
        let iv = frame_interval("matrix", &cfg, false).expect("matrix is animated");
        assert_eq!(iv, Duration::from_millis(50));
        assert_eq!(
            frame_interval("matrix", &cfg, true),
            None,
            "paused means static"
        );
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
