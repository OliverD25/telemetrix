use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::time::SystemTime;

use super::bundled;
use super::runner::{self, RunnerHandle, RunnerSettings};
use crate::config::Config;
use crate::event::{AppEvent, WorkerCmd};

/// `--plugins-dir`, else `general.plugins_dir` (relative to the settings
/// file's folder), else the plugin home next to the settings file.
pub fn plugins_dir(cfg: &Config, settings_path: &Path) -> PathBuf {
    bundled::plugins_dir(&cfg.general.plugins_dir, settings_path)
}

/// Every plugin id a theme's `hide` and `order` may name: the built-in
/// plugins and the plugin files in `dir`.
pub fn known_card_ids(dir: &Path) -> std::collections::BTreeSet<String> {
    let mut ids: std::collections::BTreeSet<String> = bundled::FILES
        .iter()
        .map(|(file, _)| file.trim_end_matches(".lua").to_string())
        .collect();
    ids.extend(discover(dir).iter().map(|(p, _)| super::runner::stem(p)));
    ids.insert("plugins".into());
    ids
}

/// Every `*.lua` file in `dir`, sorted, with its modification time.
pub fn discover(dir: &Path) -> Vec<(PathBuf, Option<SystemTime>)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(PathBuf, Option<SystemTime>)> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lua")))
        .map(|p| {
            let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
            (p, mtime)
        })
        .collect();
    files.sort();
    files
}

struct Running {
    handle: RunnerHandle,
    mtime: Option<SystemTime>,
}

pub struct Manager {
    tx: Sender<AppEvent>,
    running: BTreeMap<PathBuf, Running>,
    settings: RunnerSettings,
    enabled: bool,
    max: usize,
    dir: PathBuf,
    settings_path: PathBuf,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ScanReport {
    pub started: usize,
    pub stopped: usize,
    pub running: usize,
}

impl Manager {
    pub fn new(cfg: &Config, settings_path: &Path, tx: Sender<AppEvent>) -> Self {
        Self {
            tx,
            running: BTreeMap::new(),
            settings: RunnerSettings::from_config(cfg),
            enabled: cfg.plugins.enabled,
            max: cfg.plugins.max_plugins,
            dir: plugins_dir(cfg, settings_path),
            settings_path: settings_path.to_path_buf(),
        }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Takes new settings: running plugins get them at once, then a rescan
    /// starts or stops plugins whose `enabled` changed.
    pub fn reconfigure(&mut self, cfg: &Config) -> ScanReport {
        self.settings = RunnerSettings::from_config(cfg);
        self.enabled = cfg.plugins.enabled;
        self.max = cfg.plugins.max_plugins;
        let dir = plugins_dir(cfg, &self.settings_path);
        if dir != self.dir {
            self.stop_all();
            self.dir = dir;
        }
        for r in self.running.values() {
            let _ = r
                .handle
                .cmd
                .send(WorkerCmd::Reconfigure(self.settings.clone()));
        }
        self.rescan()
    }

    /// New files start, removed or disabled ones stop, changed ones restart.
    pub fn rescan(&mut self) -> ScanReport {
        let mut report = ScanReport::default();
        let files: BTreeMap<PathBuf, Option<SystemTime>> = if self.enabled {
            discover(&self.dir)
                .into_iter()
                .filter(|(p, _)| self.settings.enabled(&runner::stem(p)))
                .collect()
        } else {
            BTreeMap::new()
        };
        let gone: Vec<PathBuf> = self
            .running
            .iter()
            .filter(|(path, r)| files.get(*path) != Some(&r.mtime))
            .map(|(path, _)| path.clone())
            .collect();
        for path in gone {
            self.stop(&path);
            report.stopped += 1;
        }
        for (path, mtime) in files {
            if self.running.contains_key(&path) {
                continue;
            }
            if self.running.len() >= self.max {
                let _ = self.tx.send(AppEvent::Log(format!(
                    "warning: plugin limit of {} reached, {} not started",
                    self.max,
                    path.display()
                )));
                continue;
            }
            let handle = runner::spawn(path.clone(), self.settings.clone(), self.tx.clone());
            self.running.insert(path, Running { handle, mtime });
            report.started += 1;
        }
        report.running = self.running.len();
        report
    }

    /// Removes the card at once, even if the thread is still busy (it
    /// notices the stop flag at its next instruction check or sleep).
    fn stop(&mut self, path: &Path) {
        if let Some(r) = self.running.remove(path) {
            r.handle.stop();
            let _ = self.tx.send(AppEvent::PluginRemoved(r.handle.current_id()));
        }
    }

    /// Wakes the plugin with this card id to run now; false when none runs.
    pub fn run_now(&self, id: &str) -> bool {
        let running = self.running.values().find(|r| r.handle.current_id() == id);
        if let Some(r) = running {
            r.handle.run_now();
        }
        running.is_some()
    }

    /// Sends a search to the running plugin; false when it does not run.
    pub fn search(&self, id: &str, query: &str) -> bool {
        let running = self.running.values().find(|r| r.handle.current_id() == id);
        if let Some(r) = running {
            r.handle.search(query);
        }
        running.is_some()
    }

    pub fn stop_all(&mut self) {
        let paths: Vec<PathBuf> = self.running.keys().cloned().collect();
        for path in paths {
            self.stop(&path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("telemetrix-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    const PLUGIN: &str = "return { title = 'T', update = function() return { metrics = {} } end }";

    #[test]
    fn folder_rules_keep_the_v01_value_and_the_dev_flag_working() {
        let settings = temp_dir("home-rules").join("telemetrix.toml");
        let home = bundled::home(&settings);
        // The user's real file, written by the v0.1 template.
        let legacy = crate::config::parse_text(
            "[general]
plugins_dir = \"plugins\"
",
        )
        .unwrap();
        assert!(legacy.problems.is_empty());
        assert_eq!(plugins_dir(&legacy.config, &settings), home);
        assert_eq!(plugins_dir(&Config::default(), &settings), home);
        let mut cfg = Config::default();
        let flags = crate::cli::Flags {
            plugins_dir: Some("plugins".into()),
            ..Default::default()
        };
        crate::config::apply_flags(&mut cfg, &flags);
        assert_eq!(
            plugins_dir(&cfg, &settings),
            std::env::current_dir().unwrap().join("plugins"),
            "--plugins-dir is relative to the current folder"
        );
        std::fs::remove_dir_all(settings.parent().unwrap()).unwrap();
    }

    #[test]
    fn discovers_only_lua_files() {
        let dir = temp_dir("discover");
        std::fs::write(dir.join("b.lua"), PLUGIN).unwrap();
        std::fs::write(dir.join("a.LUA"), PLUGIN).unwrap();
        std::fs::write(dir.join("notes.txt"), "x").unwrap();
        let names: Vec<String> = discover(&dir)
            .iter()
            .map(|(p, _)| runner::stem(p))
            .collect();
        assert_eq!(names, ["a", "b"]);
        assert!(discover(&dir.join("missing")).is_empty());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn rescan_starts_stops_and_respects_enabled_and_cap() {
        let dir = temp_dir("rescan");
        for name in ["one", "two", "three"] {
            std::fs::write(dir.join(format!("{name}.lua")), PLUGIN).unwrap();
        }
        let mut cfg = Config::default();
        cfg.general.plugins_dir = dir.clone();
        cfg.plugins.max_plugins = 2;
        let (tx, rx) = mpsc::channel();
        let mut m = Manager::new(&cfg, &dir.join("telemetrix.toml"), tx);
        let r = m.rescan();
        assert_eq!((r.started, r.running), (2, 2), "the cap holds");

        cfg.plugins.max_plugins = 16;
        cfg.plugin_cfg.entry("two".into()).or_default().enabled = false;
        let r = m.reconfigure(&cfg);
        assert_eq!(r.running, 2, "two is disabled, three starts");
        assert!(!m.running.keys().any(|p| runner::stem(p) == "two"));

        std::fs::remove_file(dir.join("one.lua")).unwrap();
        let r = m.rescan();
        assert_eq!((r.stopped, r.running), (1, 1));
        let removed: Vec<String> = rx
            .try_iter()
            .filter_map(|e| match e {
                AppEvent::PluginRemoved(id) => Some(id),
                _ => None,
            })
            .collect();
        assert!(removed.contains(&"one".to_string()), "{removed:?}");

        cfg.plugins.enabled = false;
        assert_eq!(m.reconfigure(&cfg).running, 0);
        m.stop_all();
        std::thread::sleep(Duration::from_millis(50));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
