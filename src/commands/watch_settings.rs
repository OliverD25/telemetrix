//! The two settings the screensaver watcher follows while it runs
//! (decision 57): `screensaver.enabled` and `screensaver.idle_minutes`.
//! The watcher looks at the file's modified time at every poll and parses
//! it again only when that changed.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::config::{self, Config};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WatchSettings {
    /// Off: the watcher never opens the dashboard, but still installs updates.
    pub enabled: bool,
    pub idle_minutes: u32,
}

impl WatchSettings {
    /// `arg_minutes` is the task's `--idle-minutes`. It counts only when the
    /// file has no valid `screensaver.idle_minutes`, so a task installed
    /// before the key existed keeps its limit.
    pub fn from_config(cfg: &Config, minutes_in_file: bool, arg_minutes: u32) -> Self {
        Self {
            enabled: cfg.screensaver.enabled,
            idle_minutes: if minutes_in_file {
                cfg.screensaver.idle_minutes
            } else {
                arg_minutes
            },
        }
    }

    /// From the file's text; `None` for no file. A file with a syntax
    /// error gives `Err`, so the watcher keeps what it had.
    pub fn from_text(text: Option<&str>, arg_minutes: u32) -> Result<Self, ()> {
        match text {
            None => Ok(Self::from_config(&Config::default(), false, arg_minutes)),
            Some(text) => {
                let parsed = config::parse_text(text).map_err(|_| ())?;
                let in_file = parsed.from_file.contains("screensaver.idle_minutes");
                Ok(Self::from_config(&parsed.config, in_file, arg_minutes))
            }
        }
    }
}

/// The log lines for a change from `old` to `new`; none when nothing the
/// user would notice changed.
pub fn changes(old: WatchSettings, new: WatchSettings) -> Vec<String> {
    match (old.enabled, new.enabled) {
        (true, false) => vec!["screensaver off: will not open the dashboard".into()],
        (false, true) => vec![format!(
            "screensaver on: the dashboard opens after {} min without input",
            new.idle_minutes
        )],
        (true, true) if old.idle_minutes != new.idle_minutes => vec![format!(
            "idle limit now {} min (screensaver.idle_minutes)",
            new.idle_minutes
        )],
        _ => Vec::new(),
    }
}

/// The lines `screensaver status` prints for the two settings in `path`.
/// `task_minutes` is the installed task's `--idle-minutes`.
pub fn status_lines(path: &Path, task_minutes: Option<u32>) -> Vec<String> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return vec![format!("settings:   cannot read {}: {e}", path.display())],
    };
    let parsed = match text.as_deref().map(config::parse_text) {
        None => None,
        Some(Ok(parsed)) => Some(parsed),
        Some(Err(p)) => {
            return vec![format!(
                "settings:   {} has a syntax error ({}); the watcher keeps its last values",
                path.display(),
                p.message
            )];
        }
    };
    let cfg = parsed
        .as_ref()
        .map_or_else(Config::default, |p| p.config.clone());
    let in_file = parsed
        .as_ref()
        .is_some_and(|p| p.from_file.contains("screensaver.idle_minutes"));
    let arg = task_minutes.unwrap_or(cfg.screensaver.idle_minutes);
    let s = WatchSettings::from_config(&cfg, in_file, arg);
    let switch = if s.enabled {
        "on (screensaver.enabled)"
    } else {
        "off (screensaver.enabled): the dashboard does not open; updates still install"
    };
    let source = match (in_file, task_minutes) {
        (true, _) => "screensaver.idle_minutes",
        (false, Some(_)) => "the task's --idle-minutes; the file has no screensaver.idle_minutes",
        (false, None) => "the default",
    };
    vec![
        format!("settings:   {}", path.display()),
        format!("switch:     {switch}"),
        format!("idle after: {} min ({source})", s.idle_minutes),
    ]
}

/// The settings file as the watcher last read it.
pub struct SettingsFile {
    path: PathBuf,
    arg_minutes: u32,
    /// `None` until the first read; then the file's time (`None` for no file).
    seen: Option<Option<SystemTime>>,
    current: WatchSettings,
}

impl SettingsFile {
    pub fn new(path: PathBuf, arg_minutes: u32) -> Self {
        let mut file = Self {
            path,
            arg_minutes,
            seen: None,
            current: WatchSettings {
                enabled: true,
                idle_minutes: arg_minutes,
            },
        };
        file.refresh();
        file
    }

    pub fn current(&self) -> WatchSettings {
        self.current
    }

    /// Reads the file again when its modified time changed. Returns the
    /// log lines for what changed.
    pub fn refresh(&mut self) -> Vec<String> {
        let mtime = std::fs::metadata(&self.path)
            .ok()
            .and_then(|m| m.modified().ok());
        if self.seen == Some(mtime) {
            return Vec::new();
        }
        self.seen = Some(mtime);
        let text = std::fs::read_to_string(&self.path).ok();
        let Ok(new) = WatchSettings::from_text(text.as_deref(), self.arg_minutes) else {
            return Vec::new();
        };
        let lines = changes(self.current, new);
        self.current = new;
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws(enabled: bool, idle_minutes: u32) -> WatchSettings {
        WatchSettings {
            enabled,
            idle_minutes,
        }
    }

    #[test]
    fn the_file_key_wins_over_the_task_argument() {
        let read = |text: Option<&str>| WatchSettings::from_text(text, 25).unwrap();
        assert_eq!(read(None), ws(true, 25), "no file: the task's minutes");
        assert_eq!(read(Some("[general]\nfps = 10\n")), ws(true, 25));
        assert_eq!(read(Some("[screensaver]\nidle_minutes = 5\n")), ws(true, 5));
        assert_eq!(
            read(Some("[screensaver]\nenabled = false\n")),
            ws(false, 25)
        );
        assert_eq!(
            read(Some("[screensaver]\nidle_minutes = 0\n")),
            ws(true, 25),
            "a wrong value counts as no key"
        );
        assert!(WatchSettings::from_text(Some("[screensaver\n"), 25).is_err());
    }

    #[test]
    fn status_names_where_the_minutes_come_from() {
        let dir = std::env::temp_dir().join(format!("telemetrix-ssstat-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("telemetrix.toml");
        let _ = std::fs::remove_file(&path);
        let lines = |task| status_lines(&path, task)[1..].to_vec();
        assert_eq!(
            lines(None),
            [
                "switch:     on (screensaver.enabled)",
                "idle after: 10 min (the default)"
            ]
        );
        assert_eq!(
            lines(Some(30))[1],
            "idle after: 30 min (the task's --idle-minutes; the file has no screensaver.idle_minutes)"
        );
        std::fs::write(&path, "[screensaver]\nenabled = false\nidle_minutes = 4\n").unwrap();
        assert_eq!(
            lines(Some(30)),
            [
                "switch:     off (screensaver.enabled): the dashboard does not open; updates still install",
                "idle after: 4 min (screensaver.idle_minutes)"
            ]
        );
        std::fs::write(&path, "[screensaver\n").unwrap();
        assert!(status_lines(&path, None)[0].contains("syntax error"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_change_of_the_switch_or_the_limit_is_logged_once() {
        assert_eq!(
            changes(ws(true, 10), ws(false, 10)),
            ["screensaver off: will not open the dashboard"]
        );
        assert_eq!(
            changes(ws(false, 10), ws(true, 7)),
            ["screensaver on: the dashboard opens after 7 min without input"]
        );
        assert_eq!(
            changes(ws(true, 10), ws(true, 15)),
            ["idle limit now 15 min (screensaver.idle_minutes)"]
        );
        assert!(changes(ws(true, 10), ws(true, 10)).is_empty());
        assert!(changes(ws(false, 10), ws(false, 15)).is_empty());
    }

    #[test]
    fn the_file_is_read_again_only_when_it_changes() {
        let dir = std::env::temp_dir().join(format!("telemetrix-watchset-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("telemetrix.toml");
        let _ = std::fs::remove_file(&path);
        let mut file = SettingsFile::new(path.clone(), 10);
        assert_eq!(file.current(), ws(true, 10));
        assert!(file.refresh().is_empty());
        std::fs::write(&path, "[screensaver]\nenabled = false\n").unwrap();
        assert_eq!(
            file.refresh(),
            ["screensaver off: will not open the dashboard"]
        );
        assert_eq!(file.current(), ws(false, 10));
        assert!(file.refresh().is_empty(), "same time: not read again");
        std::fs::write(&path, "[screensaver\n").unwrap();
        set_later(&path, 60);
        file.refresh();
        assert_eq!(file.current(), ws(false, 10), "a syntax error keeps it");
        std::fs::write(&path, "[screensaver]\nidle_minutes = 3\n").unwrap();
        set_later(&path, 120);
        file.refresh();
        assert_eq!(file.current(), ws(true, 3));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Moves the file's time forward, so a write in the same clock tick
    /// still counts as a change.
    fn set_later(path: &std::path::Path, seconds: u64) {
        let f = std::fs::File::options().write(true).open(path).unwrap();
        let later = std::time::SystemTime::now() + std::time::Duration::from_secs(seconds);
        f.set_modified(later).unwrap();
    }
}
