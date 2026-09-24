use std::cell::RefCell;
use std::net::{TcpStream, ToSocketAddrs};
use std::rc::Rc;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use mlua::{Lua, LuaSerdeExt, Table, Value};
use ureq::Agent;

use super::sandbox::{Deadline, remaining};

const BODY_LIMIT: u64 = 1 << 20;
const AGENT_IDLE: Duration = Duration::from_secs(60);
const PING_TIMEOUT: Duration = Duration::from_secs(2);
const USER_AGENT: &str = concat!("telemetrix/", env!("CARGO_PKG_VERSION"));

pub type LogFn = Rc<dyn Fn(&str)>;

/// The HTTP client is created on the first request and dropped after a minute
/// without one, so plugins that never use the network cost no memory for it.
pub struct Http {
    agent: Option<Agent>,
    last_used: Instant,
    pub timeout: Duration,
}

impl Http {
    pub fn new(timeout: Duration) -> Self {
        Self {
            agent: None,
            last_used: Instant::now(),
            timeout,
        }
    }

    fn agent(&mut self) -> &Agent {
        self.last_used = Instant::now();
        self.agent.get_or_insert_with(|| {
            Agent::config_builder()
                .timeout_global(Some(self.timeout))
                .http_status_as_error(false)
                .max_idle_connections(1)
                .input_buffer_size(16 * 1024)
                .output_buffer_size(4 * 1024)
                .user_agent(USER_AGENT)
                .build()
                .into()
        })
    }

    pub fn drop_if_idle(&mut self) {
        if self.agent.is_some() && self.last_used.elapsed() >= AGENT_IDLE {
            self.agent = None;
        }
    }
}

pub struct HostCtx {
    pub log: LogFn,
    pub http: Rc<RefCell<Http>>,
    pub deadline: Deadline,
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
    let mut http = http.borrow_mut();
    let wanted = timeout.map_or(http.timeout, Duration::from_secs_f64);
    let limit = cap(wanted, deadline);
    let mut response = http
        .agent()
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
            http: Rc::new(RefCell::new(Http::new(Duration::from_secs(1)))),
            deadline: Rc::new(Cell::new(None)),
        };
        install(&lua, ctx).unwrap();
        (lua, lines)
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
