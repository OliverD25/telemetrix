use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::io::Read;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use mlua::{Function, Lua, LuaSerdeExt, Table, Value};
use ureq::Agent;

use super::PluginData;
use super::manifest::CardUpdate;
use super::sandbox::{Deadline, remaining};
use super::speed::{self, Multi};
use super::store;

const BODY_LIMIT: u64 = 1 << 20;
const AGENT_IDLE: Duration = Duration::from_secs(60);
const PING_TIMEOUT: Duration = Duration::from_secs(2);
/// Speed tests report progress at most this often.
const PROGRESS_EVERY: Duration = Duration::from_millis(250);
/// Extra time for connecting and the response, on top of `max_seconds`.
const SPEED_GRACE: Duration = Duration::from_secs(5);
const CHUNK: usize = 16 * 1024;
const USER_AGENT: &str = concat!("telemetrix/", env!("CARGO_PKG_VERSION"));

pub type LogFn = Rc<dyn Fn(&str)>;
/// Where `telemetrix.emit(card)` sends an in-between card.
pub type EmitFn = Rc<dyn Fn(PluginData)>;

/// More emits than this per second are dropped.
pub const EMITS_PER_SECOND: usize = 10;

/// Why `update()` runs, for `telemetrix.trigger()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// The first call after the plugin (re)started; also `snapshot --plugins`
    /// and `plugin check` without `--run`.
    Start,
    Interval,
    /// The plugin's run key, or a text setting saved in the overlay.
    Key,
    /// `plugin check --run`.
    Manual,
    /// The plugin's own `[plugin.<id>]` settings changed.
    Settings,
}

impl Trigger {
    pub fn as_str(self) -> &'static str {
        match self {
            Trigger::Start => "start",
            Trigger::Interval => "interval",
            Trigger::Key => "key",
            Trigger::Manual => "manual",
            Trigger::Settings => "settings",
        }
    }
}

/// The plugin's id and title: the file name until the file has loaded.
#[derive(Clone, Debug, Default)]
pub struct PluginMeta {
    pub id: String,
    pub title: String,
}

/// One HTTP client for all plugin threads, so the TLS root store and the
/// connection pool exist once. It is created on the first request and dropped
/// after a minute without one, so offline plugins cost no memory for it.
static SHARED_AGENT: Mutex<Option<(Agent, Instant)>> = Mutex::new(None);

fn shared_agent() -> Agent {
    let mut slot = SHARED_AGENT.lock().unwrap_or_else(|e| e.into_inner());
    let (agent, last_used) = slot.get_or_insert_with(|| {
        let agent = Agent::config_builder()
            .http_status_as_error(false)
            .max_idle_connections(2)
            .input_buffer_size(16 * 1024)
            .output_buffer_size(4 * 1024)
            .user_agent(USER_AGENT)
            .build()
            .into();
        (agent, Instant::now())
    });
    *last_used = Instant::now();
    agent.clone()
}

/// A plugin's HTTP settings; the client itself is shared.
pub struct Http {
    pub timeout: Duration,
}

impl Http {
    pub fn new(timeout: Duration) -> Self {
        Self { timeout }
    }

    pub fn drop_if_idle(&mut self) {
        let mut slot = SHARED_AGENT.lock().unwrap_or_else(|e| e.into_inner());
        if slot
            .as_ref()
            .is_some_and(|(_, used)| used.elapsed() >= AGENT_IDLE)
        {
            *slot = None;
        }
    }
}

pub struct HostCtx {
    pub log: LogFn,
    pub emit: EmitFn,
    pub meta: Rc<RefCell<PluginMeta>>,
    pub data_dir: PathBuf,
    pub http: Rc<RefCell<Http>>,
    pub deadline: Deadline,
    pub trigger: Rc<Cell<Trigger>>,
}

fn now_ms() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

/// The shorter of the wanted timeout and what is left of the call deadline.
fn cap(wanted: Duration, deadline: &Deadline) -> Duration {
    remaining(deadline).map_or(wanted, |left| wanted.min(left))
}

/// Lua values for JSON: `null` becomes `nil`, not a userdata sentinel.
pub fn json_to_lua(lua: &Lua, json: &serde_json::Value) -> mlua::Result<Value> {
    let options = mlua::serde::ser::Options::new()
        .serialize_none_to_null(false)
        .serialize_unit_to_null(false)
        .set_array_metatable(false);
    lua.to_value_with(json, options)
}

fn http_get(
    http: &RefCell<Http>,
    deadline: &Deadline,
    url: &str,
    timeout: Option<f64>,
) -> Result<(String, u16), String> {
    let wanted = timeout.map_or(http.borrow().timeout, Duration::from_secs_f64);
    let limit = cap(wanted, deadline);
    let mut response = shared_agent()
        .get(url)
        .config()
        .timeout_global(Some(limit))
        .build()
        .call()
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    let body = response
        .body_mut()
        .with_config()
        .limit(BODY_LIMIT)
        .read_to_string()
        .map_err(|e| e.to_string())?;
    Ok((body, status))
}

/// What one speed-test direction moved, and how fast.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transfer {
    pub bytes: u64,
    pub seconds: f64,
}

impl Transfer {
    pub fn mbps(&self) -> f64 {
        if self.seconds > 0.0 {
            self.bytes as f64 * 8.0 / self.seconds / 1_000_000.0
        } else {
            0.0
        }
    }
}

/// Calls the Lua progress callback with `(bytes, seconds)`, at most every
/// `PROGRESS_EVERY`. An error in the callback stops the transfer.
struct Progress {
    callback: Option<Function>,
    last: Option<Instant>,
    start: Instant,
}

impl Progress {
    fn new(callback: Option<Function>, start: Instant) -> Self {
        Self {
            callback,
            last: None,
            start,
        }
    }

    fn tick(&mut self, bytes: u64) -> mlua::Result<()> {
        let Some(f) = &self.callback else {
            return Ok(());
        };
        if self.last.is_some_and(|t| t.elapsed() < PROGRESS_EVERY) {
            return Ok(());
        }
        self.last = Some(Instant::now());
        f.call::<()>((bytes, self.start.elapsed().as_secs_f64()))
    }
}

fn check_speed_args(url: &str, max_seconds: f64) -> Result<(), String> {
    if !url.starts_with("https://") {
        return Err("speed tests accept only https:// addresses".into());
    }
    if !(max_seconds > 0.0 && max_seconds.is_finite()) {
        return Err("max_seconds must be above 0".into());
    }
    Ok(())
}

/// Reads up to `max_bytes` for up to `max_seconds`, 16 KB at a time, and
/// throws the data away: nothing is kept in memory.
fn speed_download(
    deadline: &Deadline,
    url: &str,
    max_bytes: u64,
    max_seconds: f64,
    callback: Option<Function>,
) -> Result<Transfer, String> {
    let _running = speed::ACTIVITY.start();
    check_speed_args(url, max_seconds)?;
    let limit = cap(Duration::from_secs_f64(max_seconds), deadline);
    let start = Instant::now();
    let mut response = shared_agent()
        .get(url)
        .config()
        .timeout_global(Some(cap(limit + SPEED_GRACE, deadline)))
        .build()
        .call()
        .map_err(|e| e.to_string())?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("the server answered HTTP {status}"));
    }
    let mut reader = response.body_mut().with_config().limit(u64::MAX).reader();
    let mut buf = vec![0u8; CHUNK];
    let mut bytes = 0u64;
    let mut progress = Progress::new(callback, start);
    while bytes < max_bytes && start.elapsed() < limit {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => bytes += n as u64,
            Err(_) if bytes > 0 => break,
            Err(e) => return Err(e.to_string()),
        }
        progress.tick(bytes).map_err(|e| e.to_string())?;
    }
    Ok(Transfer {
        bytes,
        seconds: start.elapsed().as_secs_f64(),
    })
}

/// Zeros for an upload body: stops at the byte or time limit, counts what
/// it handed out, and reports progress on the way.
pub struct Zeros {
    remaining: u64,
    sent: Rc<Cell<u64>>,
    start: Instant,
    limit: Duration,
    progress: Progress,
}

impl Zeros {
    pub fn new(bytes: u64, limit: Duration, sent: Rc<Cell<u64>>) -> Self {
        let start = Instant::now();
        Self {
            remaining: bytes,
            sent,
            start,
            limit,
            progress: Progress::new(None, start),
        }
    }
}

impl Read for Zeros {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.remaining == 0 || self.start.elapsed() >= self.limit {
            return Ok(0);
        }
        let n = buf.len().min(CHUNK).min(self.remaining as usize);
        buf[..n].fill(0);
        self.remaining -= n as u64;
        self.sent.set(self.sent.get() + n as u64);
        self.progress
            .tick(self.sent.get())
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        Ok(n)
    }
}

/// Sends `bytes` zeros (or fewer, when `max_seconds` runs out first).
fn speed_upload(
    deadline: &Deadline,
    url: &str,
    bytes: u64,
    max_seconds: f64,
    callback: Option<Function>,
) -> Result<Transfer, String> {
    let _running = speed::ACTIVITY.start();
    check_speed_args(url, max_seconds)?;
    let limit = cap(Duration::from_secs_f64(max_seconds), deadline);
    let sent = Rc::new(Cell::new(0u64));
    let mut body = Zeros::new(bytes, limit, sent.clone());
    body.progress = Progress::new(callback, body.start);
    let start = body.start;
    let result = shared_agent()
        .post(url)
        .config()
        .timeout_global(Some(cap(limit + SPEED_GRACE, deadline)))
        .build()
        .send(ureq::SendBody::from_owned_reader(body));
    let transfer = Transfer {
        bytes: sent.get(),
        seconds: start.elapsed().as_secs_f64(),
    };
    match result {
        Ok(r) if r.status().as_u16() >= 400 => {
            Err(format!("the server answered HTTP {}", r.status().as_u16()))
        }
        Ok(_) => Ok(transfer),
        // Stopping at the time limit can end the request early; what was sent still counts.
        Err(_) if transfer.bytes > 0 && transfer.seconds >= limit.as_secs_f64() => Ok(transfer),
        Err(e) => Err(e.to_string()),
    }
}

fn tcp_ping(host: &str, port: u16, timeout: Duration) -> Result<f64, String> {
    let addr = (host, port)
        .to_socket_addrs()
        .map_err(|e| e.to_string())?
        .next()
        .ok_or("no address found")?;
    let start = Instant::now();
    TcpStream::connect_timeout(&addr, timeout).map_err(|e| e.to_string())?;
    Ok(start.elapsed().as_secs_f64() * 1000.0)
}

/// Installs the global `telemetrix` table and routes `print` to the log.
pub fn install(lua: &Lua, ctx: HostCtx) -> mlua::Result<()> {
    let t = lua.create_table()?;

    let (http, deadline) = (ctx.http.clone(), ctx.deadline.clone());
    t.set(
        "http_get",
        lua.create_function(move |lua, (url, timeout): (String, Option<f64>)| {
            Ok(match http_get(&http, &deadline, &url, timeout) {
                Ok((body, status)) => (
                    Value::String(lua.create_string(body)?),
                    Value::Integer(status.into()),
                ),
                Err(e) => (Value::Nil, Value::String(lua.create_string(e)?)),
            })
        })?,
    )?;

    t.set(
        "json_decode",
        lua.create_function(|lua, text: String| {
            Ok(match serde_json::from_str::<serde_json::Value>(&text) {
                Ok(json) => (json_to_lua(lua, &json)?, Value::Nil),
                Err(e) => (Value::Nil, Value::String(lua.create_string(e.to_string())?)),
            })
        })?,
    )?;

    let log = ctx.log.clone();
    t.set(
        "log",
        lua.create_function(move |_, msg: String| {
            log(&msg);
            Ok(())
        })?,
    )?;
    let log = ctx.log.clone();
    lua.globals().set(
        "print",
        lua.create_function(move |_, args: mlua::Variadic<Value>| {
            let parts: Vec<String> = args
                .iter()
                .map(|v| v.to_string().unwrap_or_default())
                .collect();
            log(&parts.join("\t"));
            Ok(())
        })?,
    )?;

    t.set("now_ms", lua.create_function(|_, ()| Ok(now_ms()))?)?;
    let trigger = ctx.trigger.clone();
    t.set(
        "trigger",
        lua.create_function(move |_, ()| Ok(trigger.get().as_str()))?,
    )?;

    for upload in [false, true] {
        let deadline = ctx.deadline.clone();
        let name = if upload {
            "speed_upload"
        } else {
            "speed_download"
        };
        t.set(
            name,
            lua.create_function(
                move |lua,
                      (url, amount, max_seconds, progress): (
                    String,
                    f64,
                    f64,
                    Option<Function>,
                )| {
                    let amount = amount.max(0.0) as u64;
                    let result = if upload {
                        speed_upload(&deadline, &url, amount, max_seconds, progress)
                    } else {
                        speed_download(&deadline, &url, amount, max_seconds, progress)
                    };
                    Ok(match result {
                        Ok(t) => {
                            let out = lua.create_table()?;
                            out.set("bytes", t.bytes)?;
                            out.set("seconds", t.seconds)?;
                            out.set("mbps", t.mbps())?;
                            (Value::Table(out), Value::Nil)
                        }
                        Err(e) => (Value::Nil, Value::String(lua.create_string(e)?)),
                    })
                },
            )?,
        )?;
    }

    let deadline = ctx.deadline.clone();
    t.set(
        "speed_multi",
        lua.create_function(move |lua, opts: Table| {
            let direction: Option<String> = opts.get("direction")?;
            let upload = match direction.as_deref() {
                None | Some("down") => false,
                Some("up") => true,
                Some(other) => {
                    let msg = format!("direction must be \"down\" or \"up\", not {other:?}");
                    return Ok((Value::Nil, Value::String(lua.create_string(msg)?)));
                }
            };
            let spec = Multi {
                upload,
                url: opts.get::<Option<String>>("url")?.unwrap_or_default(),
                streams: opts.get::<Option<usize>>("streams")?.unwrap_or(4),
                seconds: opts.get::<Option<f64>>("seconds")?.unwrap_or(3.0),
                warmup: opts.get::<Option<f64>>("warmup")?.unwrap_or(0.5),
                piece_bytes: opts
                    .get::<Option<u64>>("piece_bytes")?
                    .unwrap_or(25_000_000),
            };
            let progress: Option<Function> = opts.get("progress")?;
            let result = speed::run(&spec, remaining(&deadline), |mbps, secs| match &progress {
                Some(f) => f.call::<()>((mbps, secs)).map_err(|e| e.to_string()),
                None => Ok(()),
            });
            Ok(match result {
                Ok(t) => {
                    let out = lua.create_table()?;
                    out.set("bytes", t.bytes)?;
                    out.set("seconds", t.seconds)?;
                    out.set("mbps", t.mbps())?;
                    (Value::Table(out), Value::Nil)
                }
                Err(e) => (Value::Nil, Value::String(lua.create_string(e)?)),
            })
        })?,
    )?;

    let (sink, meta) = (ctx.emit.clone(), ctx.meta.clone());
    let recent: RefCell<VecDeque<Instant>> = RefCell::new(VecDeque::new());
    t.set(
        "emit",
        lua.create_function(move |_, card: Value| {
            let now = Instant::now();
            let mut recent = recent.borrow_mut();
            while recent
                .front()
                .is_some_and(|t| now.duration_since(*t) >= Duration::from_secs(1))
            {
                recent.pop_front();
            }
            // A malformed card is an error even when it would be dropped.
            let card = CardUpdate::from_value(card).map_err(mlua::Error::runtime)?;
            if recent.len() >= EMITS_PER_SECOND {
                return Ok(false);
            }
            recent.push_back(now);
            let meta = meta.borrow();
            sink(PluginData {
                id: meta.id.clone(),
                title: card.title.unwrap_or_else(|| meta.title.clone()),
                metrics: card.metrics,
                error: None,
                lua_bytes: None,
            });
            Ok(true)
        })?,
    )?;

    let (dir, meta) = (ctx.data_dir.clone(), ctx.meta.clone());
    t.set(
        "store_get",
        lua.create_function(move |lua, ()| {
            Ok(match store::load(&dir, &meta.borrow().id) {
                Ok(Some(json)) => (json_to_lua(lua, &json)?, Value::Nil),
                Ok(None) => (Value::Nil, Value::Nil),
                Err(e) => (Value::Nil, Value::String(lua.create_string(e)?)),
            })
        })?,
    )?;
    let (dir, meta) = (ctx.data_dir.clone(), ctx.meta.clone());
    t.set(
        "store_set",
        lua.create_function(move |lua, value: Value| {
            let json: serde_json::Value = match lua.from_value(value) {
                Ok(json) => json,
                Err(e) => {
                    let msg =
                        format!("only tables of text, numbers and booleans can be stored: {e}");
                    return Ok((Value::Nil, Value::String(lua.create_string(msg)?)));
                }
            };
            Ok(match store::save(&dir, &meta.borrow().id, &json) {
                Ok(()) => (Value::Boolean(true), Value::Nil),
                Err(e) => (Value::Nil, Value::String(lua.create_string(e)?)),
            })
        })?,
    )?;

    let deadline = ctx.deadline.clone();
    t.set(
        "tcp_ping_ms",
        lua.create_function(
            move |lua, (host, port, timeout_ms): (String, u16, Option<f64>)| {
                let wanted =
                    timeout_ms.map_or(PING_TIMEOUT, |ms| Duration::from_secs_f64(ms / 1000.0));
                Ok(match tcp_ping(&host, port, cap(wanted, &deadline)) {
                    Ok(ms) => (Value::Number(ms), Value::Nil),
                    Err(e) => (Value::Nil, Value::String(lua.create_string(e)?)),
                })
            },
        )?,
    )?;

    t.set(
        "uptime_s",
        lua.create_function(|_, ()| Ok(sysinfo::System::uptime()))?,
    )?;
    t.set(
        "hostname",
        lua.create_function(|_, ()| Ok(sysinfo::System::host_name().unwrap_or_default()))?,
    )?;
    t.set("settings", lua.create_table()?)?;
    lua.globals().set("telemetrix", t)
}

fn toml_to_json(v: &toml_edit::Value) -> serde_json::Value {
    use serde_json::Value as J;
    match v {
        toml_edit::Value::String(s) => J::from(s.value().as_str()),
        toml_edit::Value::Integer(n) => J::from(*n.value()),
        toml_edit::Value::Float(f) => J::from(*f.value()),
        toml_edit::Value::Boolean(b) => J::from(*b.value()),
        toml_edit::Value::Datetime(d) => J::from(d.value().to_string()),
        toml_edit::Value::Array(a) => J::Array(a.iter().map(toml_to_json).collect()),
        toml_edit::Value::InlineTable(t) => J::Object(
            t.iter()
                .map(|(k, v)| (k.to_string(), toml_to_json(v)))
                .collect(),
        ),
    }
}

/// Sets `telemetrix.units = { temperature = "celsius"|"fahrenheit",
/// bytes = "binary"|"decimal" }` from `[units]`.
pub fn set_units(lua: &Lua, units: &crate::config::Units) -> mlua::Result<()> {
    use crate::config::{BytesUnit, TempUnit};
    let u = lua.create_table()?;
    let temperature = match units.temperature {
        TempUnit::Celsius => "celsius",
        TempUnit::Fahrenheit => "fahrenheit",
    };
    let bytes = match units.bytes {
        BytesUnit::Binary => "binary",
        BytesUnit::Decimal => "decimal",
    };
    u.set("temperature", temperature)?;
    u.set("bytes", bytes)?;
    let t: Table = lua.globals().get("telemetrix")?;
    t.set("units", u)
}

/// Replaces `telemetrix.settings` with the plugin's `[plugin.<id>]` keys.
pub fn set_settings(lua: &Lua, settings: &toml_edit::Table) -> mlua::Result<()> {
    let map: serde_json::Map<String, serde_json::Value> = settings
        .iter()
        .filter_map(|(k, item)| item.as_value().map(|v| (k.to_string(), toml_to_json(v))))
        .collect();
    let value = json_to_lua(lua, &serde_json::Value::Object(map))?;
    let t: Table = lua.globals().get("telemetrix")?;
    t.set("settings", value)
}

/// Network soak of the HTTP client: `cargo test --features alloc-stats
/// http_client_soak -- --ignored --nocapture`. It sends a few hundred cheap
/// requests to Binance's ping endpoint (weight 1 each).
#[cfg(all(test, feature = "alloc-stats"))]
mod http_soak {
    use super::*;
    use crate::alloc_stats::snapshot;

    const URL: &str = "https://api.binance.com/api/v3/ping";

    fn get(http: &RefCell<Http>) {
        let deadline: Deadline = Rc::new(Cell::new(None));
        let (_, status) = http_get(http, &deadline, URL, Some(10.0)).unwrap();
        assert_eq!(status, 200);
    }

    fn private() -> i64 {
        crate::selfmem::read().and_then(|m| m.private).unwrap_or(0) as i64
    }

    /// Runs `f` 300 times after 30 warm-up calls; prints heap and private growth.
    fn probe(what: &str, mut f: impl FnMut()) {
        for _ in 0..30 {
            f();
        }
        let (heap, priv0) = (snapshot().0, private());
        for _ in 0..300 {
            f();
        }
        println!(
            "{what}: heap {:+} bytes, private {:+} KB over 300 calls",
            snapshot().0 - heap,
            (private() - priv0) / 1024
        );
    }

    #[test]
    #[ignore = "uses the network"]
    fn native_memory_bisect() {
        let http = RefCell::new(Http::new(Duration::from_secs(10)));
        probe("thread spawn+join", || {
            std::thread::spawn(|| {}).join().unwrap()
        });
        probe("dns in a new thread", || {
            std::thread::spawn(|| {
                let _ = std::net::ToSocketAddrs::to_socket_addrs("api.binance.com:443");
            })
            .join()
            .unwrap()
        });
        probe("dns on this thread", || {
            let _ = std::net::ToSocketAddrs::to_socket_addrs("api.binance.com:443");
        });
        probe("tcp connect to an IP", || {
            let _ =
                TcpStream::connect_timeout(&"1.1.1.1:443".parse().unwrap(), Duration::from_secs(2));
        });
        probe("https, pooled agent", || get(&http));
        probe("https, new agent each time", || {
            get(&http);
            *SHARED_AGENT.lock().unwrap() = None;
        });
    }

    #[test]
    #[ignore = "uses the network"]
    fn http_client_soak() {
        let http = RefCell::new(Http::new(Duration::from_secs(10)));
        for _ in 0..20 {
            get(&http);
        }
        let before = snapshot().0;
        for _ in 0..200 {
            get(&http);
        }
        let kept = snapshot().0 - before;
        let before = snapshot().0;
        for _ in 0..100 {
            get(&http);
            // What drop_if_idle does after a minute without requests.
            *SHARED_AGENT.lock().unwrap() = None;
        }
        let rebuilt = snapshot().0 - before;
        println!("200 requests on one agent: heap {kept:+} bytes");
        println!("100 requests, agent rebuilt each time: heap {rebuilt:+} bytes");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn lua_with_host() -> (Lua, Rc<RefCell<Vec<String>>>) {
        let lua = Lua::new();
        let lines = Rc::new(RefCell::new(Vec::new()));
        let sink = lines.clone();
        let ctx = HostCtx {
            log: Rc::new(move |m: &str| sink.borrow_mut().push(m.to_string())),
            emit: Rc::new(|_| {}),
            meta: Rc::new(RefCell::new(PluginMeta {
                id: "test".into(),
                title: "Test".into(),
            })),
            data_dir: std::env::temp_dir().join(format!("telemetrix-host-{}", std::process::id())),
            http: Rc::new(RefCell::new(Http::new(Duration::from_secs(1)))),
            deadline: Rc::new(Cell::new(None)),
            trigger: Rc::new(Cell::new(Trigger::Interval)),
        };
        install(&lua, ctx).unwrap();
        (lua, lines)
    }

    #[test]
    fn zeros_stop_at_the_byte_and_time_limits() {
        let sent = Rc::new(Cell::new(0));
        let mut z = Zeros::new(40_000, Duration::from_secs(60), sent.clone());
        let mut buf = vec![1u8; 64 * 1024];
        let mut total = 0;
        loop {
            let n = z.read(&mut buf).unwrap();
            if n == 0 {
                break;
            }
            assert!(n <= CHUNK && buf[..n].iter().all(|b| *b == 0));
            total += n;
        }
        assert_eq!((total, sent.get()), (40_000, 40_000));
        let mut late = Zeros::new(1_000_000, Duration::ZERO, Rc::new(Cell::new(0)));
        assert_eq!(late.read(&mut buf).unwrap(), 0, "no time left: no bytes");
        let t = Transfer {
            bytes: 25_000_000,
            seconds: 2.0,
        };
        assert!((t.mbps() - 100.0).abs() < 1e-9);
    }

    #[test]
    fn speed_multi_checks_its_arguments_without_the_network() {
        let (lua, _) = lua_with_host();
        let ok: bool = lua
            .load(
                "local a, e1 = telemetrix.speed_multi({ url = 'http://x/' })                  local b, e2 = telemetrix.speed_multi({ url = 'https://x/', direction = 'sideways' })                  local c, e3 = telemetrix.speed_multi({ url = 'https://x/', streams = 0 })                  return a == nil and b == nil and c == nil and e1:find('https') ~= nil                  and e2:find('direction') ~= nil and e3:find('streams') ~= nil",
            )
            .eval()
            .unwrap();
        assert!(ok);
    }

    #[test]
    fn speed_tests_refuse_plain_http() {
        let (lua, _) = lua_with_host();
        let ok: bool = lua
            .load(
                "local r, e1 = telemetrix.speed_download('http://example.com', 10, 1) \
                 local u, e2 = telemetrix.speed_upload('ftp://x', 10, 1) \
                 local z, e3 = telemetrix.speed_download('https://example.com', 10, 0) \
                 return r == nil and u == nil and z == nil and e1:find('https') ~= nil \
                 and e2:find('https') ~= nil and e3:find('max_seconds') ~= nil",
            )
            .eval()
            .unwrap();
        assert!(ok);
    }

    #[test]
    fn emit_is_limited_to_ten_per_second() {
        let lua = Lua::new();
        let got = Rc::new(RefCell::new(Vec::new()));
        let seen = got.clone();
        let ctx = HostCtx {
            log: Rc::new(|_: &str| {}),
            emit: Rc::new(move |d: PluginData| seen.borrow_mut().push(d)),
            meta: Rc::new(RefCell::new(PluginMeta {
                id: "speed".into(),
                title: "Speed".into(),
            })),
            data_dir: std::env::temp_dir(),
            http: Rc::new(RefCell::new(Http::new(Duration::from_secs(1)))),
            deadline: Rc::new(std::cell::Cell::new(None)),
            trigger: Rc::new(Cell::new(Trigger::Start)),
        };
        install(&lua, ctx).unwrap();
        let sent: i64 = lua
            .load(
                "local n = 0 for i = 1, 15 do \
                 if telemetrix.emit({ metrics = { { label = 'down', value = i } } }) then n = n + 1 end end \
                 return n",
            )
            .eval()
            .unwrap();
        assert_eq!(sent, 10);
        let got = got.borrow();
        assert_eq!(got.len(), 10);
        assert_eq!(
            (got[0].id.as_str(), got[0].title.as_str()),
            ("speed", "Speed")
        );
        assert_eq!(got[9].metrics[0].value, "10");
        assert!(
            lua.load("telemetrix.emit({ nope = 1 })").exec().is_err(),
            "a bad card is an error"
        );
    }

    #[test]
    fn json_null_becomes_nil_and_errors_return_nil() {
        let (lua, _) = lua_with_host();
        let ok: bool = lua
            .load(
                "local t = telemetrix.json_decode('{\"a\": null, \"b\": [1, 2], \"c\": \"x\"}') \
                 local bad, err = telemetrix.json_decode('{') \
                 return t.a == nil and #t.b == 2 and t.c == 'x' and bad == nil and type(err) == 'string'",
            )
            .eval()
            .unwrap();
        assert!(ok);
    }

    #[test]
    fn log_print_and_small_helpers() {
        let (lua, lines) = lua_with_host();
        lua.load("telemetrix.log('hello') print('a', 1)")
            .exec()
            .unwrap();
        assert_eq!(*lines.borrow(), ["hello", "a\t1"]);
        let ok: bool = lua
            .load("return telemetrix.now_ms() >= 0 and telemetrix.uptime_s() > 0 and type(telemetrix.hostname()) == 'string'")
            .eval()
            .unwrap();
        assert!(ok);
    }

    #[test]
    fn units_reach_lua() {
        let (lua, _) = lua_with_host();
        let mut units = crate::config::Config::default().units;
        set_units(&lua, &units).unwrap();
        let text: String = lua
            .load("return telemetrix.units.temperature .. ' ' .. telemetrix.units.bytes")
            .eval()
            .unwrap();
        assert_eq!(text, "celsius binary");
        units.temperature = crate::config::TempUnit::Fahrenheit;
        units.bytes = crate::config::BytesUnit::Decimal;
        set_units(&lua, &units).unwrap();
        let text: String = lua
            .load("return telemetrix.units.temperature .. ' ' .. telemetrix.units.bytes")
            .eval()
            .unwrap();
        assert_eq!(text, "fahrenheit decimal");
    }

    #[test]
    fn settings_reach_lua() {
        let (lua, _) = lua_with_host();
        let doc: toml_edit::DocumentMut = "label = 'Kyiv'\nlat = 50.45\ncoins = ['a', 'b']\n"
            .parse()
            .unwrap();
        set_settings(&lua, doc.as_table()).unwrap();
        let ok: bool = lua
            .load("local s = telemetrix.settings return s.label == 'Kyiv' and s.lat == 50.45 and s.coins[2] == 'b'")
            .eval()
            .unwrap();
        assert!(ok);
    }

    #[test]
    fn ping_to_a_closed_local_port_fails_fast() {
        let (lua, _) = lua_with_host();
        let ok: bool = lua
            .load("local ms, err = telemetrix.tcp_ping_ms('127.0.0.1', 9, 300) return ms == nil and type(err) == 'string'")
            .eval()
            .unwrap();
        assert!(ok);
    }
}
