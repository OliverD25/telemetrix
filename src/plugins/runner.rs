use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use super::PluginData;
use super::host_api::{self, EmitFn, HostCtx, Http, LogFn, PluginMeta, Trigger};
use super::manifest::{CardUpdate, PluginManifest};
use super::sandbox::Sandbox;
use super::schema::{self, SchemaEntry};
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
    /// Where plugin stores live (`--data-dir` or the OS data folder).
    pub data_dir: PathBuf,
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
            data_dir: super::store::data_dir(),
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
        lua_bytes: None,
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

/// A settings change waits this long for more changes, and for the `RunNow`
/// that follows a saved text setting, so they end in one `update()`.
const SETTLE: Duration = Duration::from_millis(300);

/// One loaded plugin: its sandbox, its manifest and its HTTP client.
pub struct Plugin {
    sandbox: Sandbox,
    manifest: PluginManifest,
    http: Rc<RefCell<Http>>,
    trigger: Rc<Cell<Trigger>>,
    log: LogFn,
    /// Settings warnings already logged, so each is logged once.
    warned: RefCell<BTreeSet<String>>,
}

/// An emit sink for callers that show no progress.
pub fn no_emit() -> EmitFn {
    Rc::new(|_| {})
}

impl Plugin {
    pub fn load(
        path: &Path,
        s: &RunnerSettings,
        stop: Arc<AtomicBool>,
        log: LogFn,
        emit: EmitFn,
    ) -> Result<Self, String> {
        let source = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let sandbox = Sandbox::new(s.memory_limit, stop).map_err(|e| short(&e))?;
        let http = Rc::new(RefCell::new(Http::new(s.http_timeout)));
        let trigger = Rc::new(Cell::new(Trigger::Start));
        let stem = stem(path);
        let meta = Rc::new(RefCell::new(PluginMeta {
            id: stem.clone(),
            title: stem.clone(),
        }));
        let ctx = HostCtx {
            log: log.clone(),
            emit,
            meta: meta.clone(),
            data_dir: s.data_dir.clone(),
            http: http.clone(),
            deadline: sandbox.deadline.clone(),
            trigger: trigger.clone(),
        };
        host_api::install(&sandbox.lua, ctx).map_err(|e| short(&e))?;
        if let Some(cfg) = s.for_id(&stem) {
            host_api::set_settings(&sandbox.lua, &cfg.settings).map_err(|e| short(&e))?;
        }
        sandbox.arm(s.call_timeout);
        let table = sandbox.load_file(path, &source).map_err(|e| short(&e));
        sandbox.disarm();
        let manifest = PluginManifest::from_table(&table?, &stem)?;
        *meta.borrow_mut() = PluginMeta {
            id: manifest.id.clone(),
            title: manifest.title.clone(),
        };
        Ok(Self {
            sandbox,
            manifest,
            http,
            trigger,
            log,
            warned: RefCell::new(BTreeSet::new()),
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

    pub fn run_key(&self) -> Option<char> {
        self.manifest.run_key
    }

    pub fn schema(&self) -> &[SchemaEntry] {
        &self.manifest.settings_schema
    }

    /// The plugin's own `call_timeout`, else `plugins.call_timeout_s`.
    pub fn call_timeout(&self, s: &RunnerSettings) -> Duration {
        self.manifest
            .call_timeout
            .map_or(s.call_timeout, Duration::from_secs)
    }

    pub fn apply_limits(&self, s: &RunnerSettings) {
        let _ = self.sandbox.lua.set_memory_limit(s.memory_limit);
        self.http.borrow_mut().timeout = s.http_timeout;
    }

    /// The plugin's settings with its schema applied; logs each wrong value once.
    fn checked_settings(&self, s: &RunnerSettings) -> toml_edit::Table {
        let empty = toml_edit::Table::new();
        let file = s.for_id(self.id()).map_or(&empty, |c| &c.settings);
        let mut now = BTreeSet::new();
        let table = schema::apply(self.schema(), file, |w| {
            now.insert(w);
        });
        for w in now.difference(&self.warned.borrow()) {
            (self.log)(&format!("warning: {w}"));
        }
        *self.warned.borrow_mut() = now;
        table
    }

    pub fn update(&self, s: &RunnerSettings, trigger: Trigger) -> Result<PluginData, String> {
        let settings = self.checked_settings(s);
        host_api::set_settings(&self.sandbox.lua, &settings).map_err(|e| short(&e))?;
        self.trigger.set(trigger);
        self.sandbox.arm(self.call_timeout(s));
        let result = self.manifest.update.call::<mlua::Value>(());
        self.sandbox.disarm();
        let card = CardUpdate::from_value(result.map_err(|e| short(&e))?)?;
        Ok(PluginData {
            id: self.id().to_string(),
            title: card.title.unwrap_or_else(|| self.title().to_string()),
            metrics: card.metrics,
            error: None,
            lua_bytes: Some(self.lua_bytes()),
        })
    }

    /// Tests replace host functions (like `telemetrix.http_get`) through this.
    #[cfg(test)]
    pub fn lua(&self) -> &mlua::Lua {
        &self.sandbox.lua
    }

    #[cfg(test)]
    pub fn update_fn(&self) -> &mlua::Function {
        &self.manifest.update
    }

    pub fn lua_bytes(&self) -> usize {
        self.sandbox.lua.used_memory()
    }

    pub fn drop_idle_http(&self) {
        self.http.borrow_mut().drop_if_idle();
    }
}

/// Loads a plugin and runs `update()` once on the calling thread.
pub fn run_once(
    path: &Path,
    s: &RunnerSettings,
    trigger: Trigger,
    log: LogFn,
    emit: EmitFn,
) -> PluginData {
    let stem = stem(path);
    match Plugin::load(path, s, Arc::new(AtomicBool::new(false)), log, emit) {
        Ok(p) => p
            .update(s, trigger)
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

    /// Wakes the thread from its interval sleep to run `update()` now.
    pub fn run_now(&self) {
        let _ = self.cmd.send(WorkerCmd::RunNow);
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
        let line = match msg.strip_prefix("warning: ") {
            Some(rest) => format!("warning: plugin {log_name}: {rest}"),
            None => format!("plugin {log_name}: {msg}"),
        };
        let _ = log_tx.send(AppEvent::Log(line));
    });
    let emit_tx = tx.clone();
    let emit: EmitFn = Rc::new(move |data| {
        let _ = emit_tx.send(AppEvent::Plugin(data));
    });
    let plugin = match Plugin::load(path, &settings, stop.clone(), log, emit) {
        Ok(p) => p,
        Err(e) => {
            let _ = tx.send(AppEvent::Log(format!("error: plugin {stem}: {e}")));
            let _ = tx.send(AppEvent::Plugin(error_data(&stem, &stem, e)));
            while let Ok(WorkerCmd::Reconfigure(_) | WorkerCmd::RunNow) = cmds.recv() {}
            return;
        }
    };
    if let Ok(mut slot) = id_slot.lock() {
        *slot = plugin.id().to_string();
    }
    let _ = tx.send(AppEvent::PluginMeta {
        id: plugin.id().to_string(),
        run_key: plugin.run_key(),
        schema: plugin.schema().to_vec(),
    });
    let _ = tx.send(AppEvent::Log(format!(
        "plugin {} loaded from {}",
        plugin.id(),
        path.display()
    )));
    let mut last_error: Option<String> = None;
    let mut trigger = Trigger::Start;
    loop {
        if stop.load(Ordering::Relaxed) {
            return;
        }
        if settings.enabled(plugin.id()) {
            let data = match plugin.update(&settings, trigger) {
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
                    let mut data = error_data(plugin.id(), plugin.title(), e);
                    data.lua_bytes = Some(plugin.lua_bytes());
                    data
                }
            };
            if stop.load(Ordering::Relaxed) || tx.send(AppEvent::Plugin(data)).is_err() {
                return;
            }
        }
        plugin.drop_idle_http();
        trigger = match wait(&plugin, &mut settings, cmds) {
            Some(t) => t,
            None => return,
        };
    }
}

/// Sleeps until the next `update()` is due and says why it is due; `None`
/// means stop. A change to other plugins' settings only updates the limits
/// and keeps the timer; a change to this plugin's own settings runs it soon.
fn wait(
    plugin: &Plugin,
    settings: &mut RunnerSettings,
    cmds: &mpsc::Receiver<WorkerCmd<RunnerSettings>>,
) -> Option<Trigger> {
    let ran = Instant::now();
    let mut due = ran + plugin.interval(settings);
    let mut changed = false;
    loop {
        match cmds.recv_timeout(due.saturating_duration_since(Instant::now())) {
            Ok(WorkerCmd::Stop) | Err(RecvTimeoutError::Disconnected) => return None,
            Ok(WorkerCmd::Reconfigure(new)) => {
                changed |= new.for_id(plugin.id()) != settings.for_id(plugin.id());
                *settings = new;
                plugin.apply_limits(settings);
                due = if changed {
                    Instant::now() + SETTLE
                } else {
                    ran + plugin.interval(settings)
                };
            }
            Ok(WorkerCmd::RunNow) => return Some(Trigger::Key),
            Err(RecvTimeoutError::Timeout) if changed => return Some(Trigger::Settings),
            Err(RecvTimeoutError::Timeout) => return Some(Trigger::Interval),
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

    fn temp_data(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("telemetrix-runner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn store_survives_between_runs() {
        let mut s = settings();
        s.data_dir = temp_data("store");
        let first = run_once(
            &fixture("store.lua"),
            &s,
            Trigger::Start,
            quiet(),
            no_emit(),
        );
        assert_eq!(first.error, None);
        assert_eq!(first.metrics[0].value, "1");
        let second = run_once(
            &fixture("store.lua"),
            &s,
            Trigger::Start,
            quiet(),
            no_emit(),
        );
        assert_eq!(
            second.metrics[0].value, "2",
            "the count came back from the store"
        );
        assert!(s.data_dir.join("plugins").join("store.json").is_file());
        std::fs::remove_dir_all(&s.data_dir).unwrap();
    }

    fn with_settings(s: &mut RunnerSettings, id: &str, toml: &str) {
        let doc: toml_edit::DocumentMut = toml.parse().unwrap();
        s.plugin_cfg.insert(
            id.into(),
            PluginConfig {
                enabled: true,
                interval: None,
                settings: doc.as_table().clone(),
            },
        );
    }

    #[test]
    fn schema_checks_settings_and_warns_once() {
        let mut s = settings();
        with_settings(&mut s, "schema", "bank = 'sber'\n");
        let lines = Rc::new(RefCell::new(Vec::new()));
        let sink = lines.clone();
        let log: LogFn = Rc::new(move |m: &str| sink.borrow_mut().push(m.to_string()));
        let p = Plugin::load(
            &fixture("schema.lua"),
            &s,
            Arc::new(AtomicBool::new(false)),
            log,
            no_emit(),
        )
        .unwrap();
        let keys: Vec<&str> = p.schema().iter().map(|e| e.key.as_str()).collect();
        assert_eq!(keys, ["bank", "city"]);
        for _ in 0..3 {
            let data = p.update(&s, Trigger::Interval).unwrap();
            assert_eq!(data.metrics[1].value, "Kyiv", "missing: the default");
            assert_eq!(data.metrics[2].value, "mono", "wrong: the default");
        }
        assert_eq!(
            *lines.borrow(),
            ["warning: bank = 'sber' is not one of mono, privat, using \"mono\""],
            "one warning, not one per update"
        );
    }

    #[test]
    fn trigger_says_why_update_runs() {
        let mut s = settings();
        with_settings(&mut s, "schema", "city = 'Kyiv'\n");
        let (tx, rx) = mpsc::channel();
        let handle = spawn(fixture("schema.lua"), s.clone(), tx);
        let next = |wait| loop {
            match rx.recv_timeout(wait) {
                Ok(AppEvent::Plugin(d)) => return Some((d.metrics[0].value.clone(), d)),
                Ok(_) => {}
                Err(_) => return None,
            }
        };
        let (t, _) = next(Duration::from_secs(5)).unwrap();
        assert_eq!(t, "start");
        handle.run_now();
        assert_eq!(next(Duration::from_secs(2)).unwrap().0, "key");

        with_settings(&mut s, "other", "x = 1\n");
        let _ = handle.cmd.send(WorkerCmd::Reconfigure(s.clone()));
        assert!(
            next(Duration::from_millis(700)).is_none(),
            "another plugin's change does not run this one"
        );

        with_settings(&mut s, "schema", "city = 'Lviv'\n");
        let _ = handle.cmd.send(WorkerCmd::Reconfigure(s.clone()));
        let (t, d) = next(Duration::from_secs(2)).unwrap();
        assert_eq!(
            (t.as_str(), d.metrics[1].value.as_str()),
            ("settings", "Lviv")
        );

        // A saved text setting: the change and the RunNow end in one "key" run.
        with_settings(&mut s, "schema", "city = 'Odesa'\n");
        let _ = handle.cmd.send(WorkerCmd::Reconfigure(s.clone()));
        handle.run_now();
        let (t, d) = next(Duration::from_secs(2)).unwrap();
        assert_eq!((t.as_str(), d.metrics[1].value.as_str()), ("key", "Odesa"));
        assert!(next(Duration::from_millis(700)).is_none(), "only one run");

        s.plugin_cfg.get_mut("schema").unwrap().interval = Some(1);
        let _ = handle.cmd.send(WorkerCmd::Reconfigure(s.clone()));
        assert_eq!(next(Duration::from_secs(2)).unwrap().0, "settings");
        assert_eq!(next(Duration::from_secs(3)).unwrap().0, "interval");
        handle.stop();
    }

    #[test]
    fn run_now_wakes_the_thread_between_intervals() {
        let (tx, rx) = mpsc::channel();
        let handle = spawn(fixture("static.lua"), settings(), tx);
        let card = |rx: &mpsc::Receiver<AppEvent>, wait| loop {
            match rx.recv_timeout(wait) {
                Ok(AppEvent::Plugin(d)) => return Some(d),
                Ok(_) => {}
                Err(_) => return None,
            }
        };
        assert!(card(&rx, Duration::from_secs(5)).is_some(), "first run");
        let start = Instant::now();
        handle.run_now();
        assert!(
            card(&rx, Duration::from_secs(2)).is_some(),
            "woken, although the interval is 60 s"
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        handle.stop();
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
        let data = run_once(
            &fixture("static.lua"),
            &s,
            Trigger::Start,
            quiet(),
            no_emit(),
        );
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
            no_emit(),
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
        let data = run_once(
            &fixture("broken.lua"),
            &settings(),
            Trigger::Start,
            quiet(),
            no_emit(),
        );
        let err = data.error.expect("broken.lua must fail");
        assert_eq!(data.id, "broken");
        assert!(err.contains("broken.lua:"), "{err}");
    }

    #[test]
    fn slow_plugin_hits_the_deadline() {
        let start = Instant::now();
        let data = run_once(
            &fixture("slow.lua"),
            &settings(),
            Trigger::Start,
            quiet(),
            no_emit(),
        );
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
        for name in [
            "clock",
            "uptime",
            "network_ping",
            "crypto",
            "weather",
            "currency",
            "speedtest",
        ] {
            let path = dir.join(format!("{name}.lua"));
            let p = Plugin::load(
                &path,
                &s,
                Arc::new(AtomicBool::new(false)),
                quiet(),
                no_emit(),
            )
            .unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(p.id(), name);
        }
        for name in ["clock", "uptime"] {
            let data = run_once(
                &dir.join(format!("{name}.lua")),
                &s,
                Trigger::Start,
                quiet(),
                no_emit(),
            );
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
