use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use super::PluginData;
use super::host_api::{self, HostCtx, Http, LogFn};
use super::manifest::{CardUpdate, PluginManifest};
use super::sandbox::Sandbox;
use crate::config::{Config, PluginConfig};
use crate::event::{AppEvent, WorkerCmd};

/// Everything a plugin thread needs from the settings file.
#[derive(Clone, Debug)]
pub struct RunnerSettings {
    pub plugin_cfg: BTreeMap<String, PluginConfig>,
    pub default_interval: Duration,
    pub http_timeout: Duration,
    pub call_timeout: Duration,
    pub memory_limit: usize,
}

impl RunnerSettings {
    pub fn from_config(cfg: &Config) -> Self {
        let p = &cfg.plugins;
        Self {
            plugin_cfg: cfg.plugin_cfg.clone(),
            default_interval: Duration::from_secs(p.default_interval_s),
            http_timeout: Duration::from_secs(p.http_timeout_s),
            call_timeout: Duration::from_secs(p.call_timeout_s),
            memory_limit: (p.memory_limit_mb as usize) << 20,
        }
    }

    pub fn for_id(&self, id: &str) -> Option<&PluginConfig> {
        self.plugin_cfg.get(id)
    }

    pub fn enabled(&self, id: &str) -> bool {
        self.for_id(id).is_none_or(|c| c.enabled)
    }
}

pub fn stem(path: &Path) -> String {
    path.file_stem()
        .map_or_else(|| "plugin".into(), |s| s.to_string_lossy().into_owned())
}

pub fn error_data(id: &str, title: &str, error: String) -> PluginData {
    PluginData {
        id: id.to_string(),
        title: title.to_string(),
        metrics: Vec::new(),
        error: Some(error),
    }
}

/// Lua errors carry a traceback after the first line; the card shows only that line.
fn short(e: &mlua::Error) -> String {
    let text = match e {
        mlua::Error::CallbackError { cause, .. } => cause.to_string(),
        other => other.to_string(),
    };
    let first = text.lines().next().unwrap_or_default();
    first
        .strip_prefix("runtime error: ")
        .unwrap_or(first)
        .to_string()
}

/// One loaded plugin: its sandbox, its manifest and its HTTP client.
pub struct Plugin {
    sandbox: Sandbox,
    manifest: PluginManifest,
    http: Rc<RefCell<Http>>,
}

impl Plugin {
    pub fn load(
        path: &Path,
        s: &RunnerSettings,
        stop: Arc<AtomicBool>,
        log: LogFn,
    ) -> Result<Self, String> {
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let sandbox = Sandbox::new(s.memory_limit, stop).map_err(|e| short(&e))?;
        let http = Rc::new(RefCell::new(Http::new(s.http_timeout)));
        let ctx = HostCtx {
            log,
            http: http.clone(),
            deadline: sandbox.deadline.clone(),
        };
        host_api::install(&sandbox.lua, ctx).map_err(|e| short(&e))?;
        let stem = stem(path);
        if let Some(cfg) = s.for_id(&stem) {
            host_api::set_settings(&sandbox.lua, &cfg.settings).map_err(|e| short(&e))?;
        }
        sandbox.arm(s.call_timeout);
        let table = sandbox.load_file(path, &source).map_err(|e| short(&e));
        sandbox.disarm();
        let manifest = PluginManifest::from_table(&table?, &stem)?;
        Ok(Self {
            sandbox,
            manifest,
            http,
        })
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn title(&self) -> &str {
        &self.manifest.title
    }

    /// `[plugin.<id>].interval`, else the file's own, else the default.
    pub fn interval(&self, s: &RunnerSettings) -> Duration {
        s.for_id(self.id())
            .and_then(|c| c.interval)
            .or(self.manifest.interval)
            .map_or(s.default_interval, Duration::from_secs)
    }

    pub fn apply_limits(&self, s: &RunnerSettings) {
        let _ = self.sandbox.lua.set_memory_limit(s.memory_limit);
        self.http.borrow_mut().timeout = s.http_timeout;
    }

    pub fn update(&self, s: &RunnerSettings) -> Result<PluginData, String> {
        let empty = toml_edit::Table::new();
        let settings = s.for_id(self.id()).map_or(&empty, |c| &c.settings);
        host_api::set_settings(&self.sandbox.lua, settings).map_err(|e| short(&e))?;
        self.sandbox.arm(s.call_timeout);
        let result = self.manifest.update.call::<mlua::Value>(());
        self.sandbox.disarm();
        let card = CardUpdate::from_value(result.map_err(|e| short(&e))?)?;
        Ok(PluginData {
            id: self.id().to_string(),
            title: card.title.unwrap_or_else(|| self.title().to_string()),
            metrics: card.metrics,
            error: None,
        })
    }

    pub fn drop_idle_http(&self) {
        self.http.borrow_mut().drop_if_idle();
    }
}

/// Loads a plugin and runs `update()` once on the calling thread.
pub fn run_once(path: &Path, s: &RunnerSettings, log: LogFn) -> PluginData {
    let stem = stem(path);
    match Plugin::load(path, s, Arc::new(AtomicBool::new(false)), log) {
        Ok(p) => p
            .update(s)
            .unwrap_or_else(|e| error_data(p.id(), p.title(), e)),
        Err(e) => error_data(&stem, &stem, e),
    }
}

pub struct RunnerHandle {
    pub cmd: Sender<WorkerCmd<RunnerSettings>>,
    pub stop: Arc<AtomicBool>,
    /// The card id: the file stem until the file is loaded, then the plugin's own id.
    pub id: Arc<Mutex<String>>,
}

impl RunnerHandle {
    pub fn current_id(&self) -> String {
        self.id.lock().map(|id| id.clone()).unwrap_or_default()
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.cmd.send(WorkerCmd::Stop);
    }
}

pub fn spawn(path: PathBuf, settings: RunnerSettings, tx: Sender<AppEvent>) -> RunnerHandle {
    let (cmd, cmd_rx) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let id = Arc::new(Mutex::new(stem(&path)));
    let handle = RunnerHandle {
        cmd,
        stop: stop.clone(),
        id: id.clone(),
    };
    let name = format!("plugin {}", stem(&path));
    let spawned = thread::Builder::new()
        .name(name)
        .spawn(move || run(&path, settings, &tx, &cmd_rx, stop, &id));
    if let Err(e) = spawned {
        handle.stop.store(true, Ordering::Relaxed);
        eprintln!("telemetrix: cannot start a plugin thread: {e}");
    }
    handle
}

fn run(
    path: &Path,
    mut settings: RunnerSettings,
    tx: &Sender<AppEvent>,
    cmds: &mpsc::Receiver<WorkerCmd<RunnerSettings>>,
    stop: Arc<AtomicBool>,
    id_slot: &Mutex<String>,
) {
    let stem = stem(path);
    let log_tx = tx.clone();
    let log_name = stem.clone();
    let log: LogFn = Rc::new(move |msg: &str| {
        let _ = log_tx.send(AppEvent::Log(format!("plugin {log_name}: {msg}")));
    });
    let plugin = match Plugin::load(path, &settings, stop.clone(), log) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(AppEvent::Log(format!("error: plugin {stem}: {e}")));
            let _ = tx.send(AppEvent::Plugin(error_data(&stem, &stem, e)));
            while let Ok(WorkerCmd::Reconfigure(_)) = cmds.recv() {}
            return;
        }
    };
    if let Ok(mut slot) = id_slot.lock() {
        *slot = plugin.id().to_string();
    }
    let _ = tx.send(AppEvent::Log(format!(
        "plugin {} loaded from {}",
        plugin.id(),
        path.display()
    )));
    let mut last_error: Option<String> = None;
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if settings.enabled(plugin.id()) {
            let data = match plugin.update(&settings) {
                Ok(data) => {
                    last_error = None;
                    data
                }
                Err(e) => {
                    if last_error.as_deref() != Some(e.as_str()) {
                        let _ =
                            tx.send(AppEvent::Log(format!("error: plugin {}: {e}", plugin.id())));
                    }
                    last_error = Some(e.clone());
                    error_data(plugin.id(), plugin.title(), e)
                }
            };
            if stop.load(Ordering::Relaxed) || tx.send(AppEvent::Plugin(data)).is_err() {
                return;
            }
        }
        plugin.drop_idle_http();
        match cmds.recv_timeout(plugin.interval(&settings)) {
            Ok(WorkerCmd::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(WorkerCmd::Reconfigure(new)) => {
                settings = new;
                plugin.apply_limits(&settings);
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    fn settings() -> RunnerSettings {
        let mut s = RunnerSettings::from_config(&Config::default());
        s.call_timeout = Duration::from_millis(300);
        s
    }

    fn quiet() -> LogFn {
        Rc::new(|_: &str| {})
    }

    #[test]
    fn static_plugin_returns_its_metrics() {
        let mut s = settings();
        let doc: toml_edit::DocumentMut = "greeting = 'hi'\n".parse().unwrap();
        s.plugin_cfg.insert(
            "static".into(),
            PluginConfig {
                enabled: true,
                interval: Some(7),
                settings: doc.as_table().clone(),
            },
        );
        let data = run_once(&fixture("static.lua"), &s, quiet());
        assert_eq!(data.error, None);
        assert_eq!(data.title, "Static");
        assert_eq!(data.metrics[0].label, "answer");
        assert_eq!(data.metrics[0].value, "42");
        assert_eq!(data.metrics[1].value, "hi", "settings reach the plugin");
        let p = Plugin::load(
            &fixture("static.lua"),
            &s,
            Arc::new(AtomicBool::new(false)),
            quiet(),
        )
        .unwrap();
        assert_eq!(
            p.interval(&s),
            Duration::from_secs(7),
            "the config interval wins"
        );
    }

    #[test]
    fn broken_plugin_reports_the_line() {
        let data = run_once(&fixture("broken.lua"), &settings(), quiet());
        let err = data.error.expect("broken.lua must fail");
        assert_eq!(data.id, "broken");
        assert!(err.contains("broken.lua:"), "{err}");
    }

    #[test]
    fn slow_plugin_hits_the_deadline() {
        let start = Instant::now();
        let data = run_once(&fixture("slow.lua"), &settings(), quiet());
        assert!(
            data.error
                .as_deref()
                .is_some_and(|e| e.contains("time limit exceeded")),
            "{data:?}"
        );
        assert!(start.elapsed() < Duration::from_secs(3));
    }

    #[test]
    fn default_plugins_load_and_the_offline_ones_run() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins");
        let s = RunnerSettings::from_config(&Config::default());
        for name in ["clock", "uptime", "network_ping", "crypto", "weather"] {
            let path = dir.join(format!("{name}.lua"));
            let p = Plugin::load(&path, &s, Arc::new(AtomicBool::new(false)), quiet())
                .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(p.id(), name);
        }
        for name in ["clock", "uptime"] {
            let data = run_once(&dir.join(format!("{name}.lua")), &s, quiet());
            assert_eq!(data.error, None, "{name}");
            assert_eq!(data.metrics.len(), 2, "{name}");
        }
    }

    #[test]
    fn thread_sends_cards_and_stops() {
        let (tx, rx) = mpsc::channel();
        let handle = spawn(fixture("static.lua"), settings(), tx);
        let mut got_card = false;
        while let Ok(ev) = rx.recv_timeout(Duration::from_secs(5)) {
            if let AppEvent::Plugin(d) = ev {
                assert_eq!(d.id, "static");
                got_card = true;
                break;
            }
        }
        assert!(got_card);
        assert_eq!(handle.current_id(), "static");
        handle.stop();
    }
}
