//! Clickable look-and-feel mockup of telemetrix with fake data.
//! Run: `cargo run --release --example mockup`. Nothing is read or saved.

mod data;
mod rain;
mod settings;
mod ui;

use std::collections::VecDeque;
use std::io;
use std::time::{Duration, Instant, SystemTime};

use ratatui::DefaultTerminal;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use data::FakeData;
use rain::Rain;
use settings::{Field, MATRIX, Row, Settings};

const DATA_INTERVAL: Duration = Duration::from_secs(1);
const TOAST_FOR: Duration = Duration::from_secs(2);
const LOG_CAP: usize = 200;
const BROKEN_ERROR: &str = "attempt to index a nil value (line 12)";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Overlay {
    None,
    Settings,
    Log,
    Help,
}

#[derive(Clone, Copy)]
pub enum Level {
    Info,
    Warn,
    Error,
}

pub struct LogLine {
    pub time: String,
    pub level: Level,
    pub text: String,
}

pub struct App {
    pub settings: Settings,
    pub data: FakeData,
    pub rain: Rain,
    pub overlay: Overlay,
    pub cursor: usize,
    pub log: VecDeque<LogLine>,
    pub toast: Option<(String, Instant)>,
    pub banner: bool,
    pub paused: bool,
    quit: bool,
}

impl App {
    fn new() -> Self {
        let mut app = Self {
            settings: Settings::default(),
            data: FakeData::default(),
            rain: Rain::new(0x5EED_CAFE),
            overlay: Overlay::None,
            cursor: 0,
            log: VecDeque::new(),
            toast: None,
            banner: false,
            paused: false,
            quit: false,
        };
        app.log(Level::Info, "telemetrix 0.1.0 mockup started");
        app.log(Level::Info, "config: loaded telemetrix.toml");
        app.log(Level::Warn, "config: unknown key general.colour ignored");
        for (id, interval) in [
            ("clock", 1),
            ("uptime", 30),
            ("network_ping", 30),
            ("crypto", 120),
            ("weather", 600),
        ] {
            app.log(
                Level::Info,
                &format!("plugin {id} loaded (interval {interval} s)"),
            );
        }
        app.log(
            Level::Error,
            &format!("plugin broken_plugin: {BROKEN_ERROR}"),
        );
        app
    }

    fn log(&mut self, level: Level, text: &str) {
        if self.log.len() == LOG_CAP {
            self.log.pop_front();
        }
        self.log.push_back(LogLine {
            time: data::fmt_hms(SystemTime::now()),
            level,
            text: text.to_string(),
        });
    }

    fn animating(&self) -> bool {
        self.settings.theme == MATRIX && !self.paused
    }

    fn frame_interval(&self) -> Duration {
        Duration::from_secs_f32(1.0 / self.settings.fps() as f32)
    }

    fn toggle(&mut self, overlay: Overlay) {
        self.overlay = if self.overlay == overlay {
            Overlay::None
        } else {
            overlay
        };
    }

    fn on_key(&mut self, key: KeyEvent, now: Instant) {
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            if key.code == KeyCode::Char('c') {
                self.quit = true;
            }
            return;
        }
        if self.overlay == Overlay::Settings && self.on_settings_key(key.code) {
            return;
        }
        match key.code {
            KeyCode::Esc if self.overlay != Overlay::None => self.overlay = Overlay::None,
            KeyCode::Esc | KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('t') => self.settings.step(Field::Theme, 1),
            KeyCode::Char('T') => self.settings.step(Field::Theme, -1),
            KeyCode::Char(' ') => self.paused = !self.paused,
            KeyCode::Char('s') => self.toggle(Overlay::Settings),
            KeyCode::Char('l') => self.toggle(Overlay::Log),
            KeyCode::Char('?') => self.toggle(Overlay::Help),
            KeyCode::Char('b') => self.banner = !self.banner,
            KeyCode::Char('r') => {
                let msg = "plugins rescanned (5 loaded, 1 error)";
                self.log(Level::Info, msg);
                self.log(
                    Level::Error,
                    &format!("plugin broken_plugin: {BROKEN_ERROR}"),
                );
                self.toast = Some((msg.to_string(), now + TOAST_FOR));
            }
            KeyCode::Char('x') => {
                self.data.spike(now);
                self.log(Level::Warn, "cpu spike simulated: 95 % for 5 s");
            }
            _ => {}
        }
    }

    /// Returns true when the key was consumed by the settings overlay.
    fn on_settings_key(&mut self, code: KeyCode) -> bool {
        let rows = settings::rows();
        let last = settings::selectable(&rows).len() - 1;
        let dir = match code {
            KeyCode::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                return true;
            }
            KeyCode::Down => {
                self.cursor = (self.cursor + 1).min(last);
                return true;
            }
            KeyCode::Enter | KeyCode::Right => 1,
            KeyCode::Left => -1,
            _ => return false,
        };
        let row = settings::selectable(&rows)[self.cursor.min(last)];
        if let Row::Edit(field) = rows[row] {
            self.settings.step(field, dir);
        }
        true
    }
}

fn main() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let result = run(&mut terminal);
    ratatui::restore();
    result
}

fn run(terminal: &mut DefaultTerminal) -> io::Result<()> {
    let mut app = App::new();
    let mut next_data = Instant::now() + DATA_INTERVAL;
    let mut last_frame = Instant::now();
    loop {
        terminal.draw(|frame| ui::draw(frame, &mut app))?;

        let mut deadline = next_data;
        if app.animating() {
            deadline = deadline.min(last_frame + app.frame_interval());
        }
        if let Some((_, until)) = &app.toast {
            deadline = deadline.min(*until);
        }
        if event::poll(deadline.saturating_duration_since(Instant::now()))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.on_key(key, Instant::now());
        }
        if app.quit {
            return Ok(());
        }

        let now = Instant::now();
        if now >= next_data {
            if !app.paused {
                app.data.tick(now);
            }
            next_data = now + DATA_INTERVAL;
        }
        if !app.animating() {
            last_frame = now;
        } else if now >= last_frame + app.frame_interval() {
            // Real elapsed time, capped, keeps the rain speed independent of the fps setting.
            let dt = (now - last_frame).min(Duration::from_millis(200));
            app.rain.step(
                dt.as_secs_f32(),
                app.settings.density(),
                app.settings.speed(),
            );
            last_frame = now;
        }
        if app.toast.as_ref().is_some_and(|(_, until)| now >= *until) {
            app.toast = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn render(app: &mut App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| ui::draw(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    fn press(app: &mut App, code: KeyCode) {
        let now = Instant::now();
        app.on_key(KeyEvent::new(code, KeyModifiers::NONE), now);
    }

    #[test]
    fn every_theme_and_overlay_renders_at_all_sizes() {
        let mut app = App::new();
        for theme in 0..settings::THEMES.len() {
            app.settings.theme = theme;
            for overlay in [
                Overlay::None,
                Overlay::Settings,
                Overlay::Log,
                Overlay::Help,
            ] {
                app.overlay = overlay;
                app.banner = true;
                app.toast = Some(("toast".into(), Instant::now()));
                for (w, h) in [(120, 40), (80, 24), (64, 16), (40, 10)] {
                    render(&mut app, w, h);
                    app.rain.step(0.2, 1.0, 3.0);
                    let text = render(&mut app, w, h);
                    assert!(text.contains("MOCKUP"), "{theme} {overlay:?} {w}x{h}");
                }
            }
        }
    }

    #[test]
    fn tiny_terminal_shows_the_notice() {
        let mut app = App::new();
        assert!(render(&mut app, 39, 20).contains(ui::TOO_SMALL));
        assert!(render(&mut app, 50, 9).contains("too small"));
    }

    fn has_warn_bar(app: &mut App) -> bool {
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|f| ui::draw(f, app)).unwrap();
        let buf = terminal.backend().buffer();
        buf.content()
            .iter()
            .any(|c| c.symbol() == "█" && c.fg == ui::WARN)
    }

    #[test]
    fn spike_colors_the_cpu_bar() {
        let mut app = App::new();
        app.settings.theme = 0;
        assert!(
            has_warn_bar(&mut app),
            "E: at 92 % is above the 90 % default"
        );
        // Raise the disk threshold to 95 % so only the CPU bar can turn red.
        app.settings.step(Field::DiskWarn, 1);
        assert!(!has_warn_bar(&mut app));
        press(&mut app, KeyCode::Char('x'));
        assert!(has_warn_bar(&mut app));
    }

    #[test]
    fn keys_drive_overlays_settings_and_quit() {
        let mut app = App::new();
        press(&mut app, KeyCode::Char('s'));
        assert_eq!(app.overlay, Overlay::Settings);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.settings.theme_name(), "minimalist");
        press(&mut app, KeyCode::Char('t'));
        assert_eq!(app.settings.theme_name(), "matrix");
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.overlay, Overlay::None);
        assert!(!app.quit);
        let before = app.log.len();
        press(&mut app, KeyCode::Char('r'));
        assert!(app.log.len() > before && app.toast.is_some());
        press(&mut app, KeyCode::Esc);
        assert!(app.quit);
    }

    #[test]
    fn plugin_off_removes_its_card() {
        let mut app = App::new();
        app.settings.theme = 0;
        assert!(render(&mut app, 120, 40).contains("Crypto"));
        app.settings.step(Field::Plugin(1), 1);
        assert!(!render(&mut app, 120, 40).contains("Crypto"));
    }
}
