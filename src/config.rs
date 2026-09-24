//! Settings registry, lenient loading, atomic saving.
//!
//! Every setting is declared once in [`SETTINGS`]. The registry produces the
//! default file, validation, the settings overlay rows and the save path.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use toml_edit::{DocumentMut, Item, Table};

use crate::cli::Flags;

pub const THEME_NAMES: &[&str] = &["minimalist", "matrix"];
pub const MATRIX_COLORS: &[&str] = &["green", "amber", "cyan", "white"];
const FILE_NAME: &str = "telemetrix.toml";
const PLUGIN_MIN_INTERVAL: i64 = 5;
/// The default setup (Matrix + 5 plugins) measured 11.1-11.3 MB working set
/// in step 9 (`selftest --memory`); 13 MB leaves about 15 % headroom.
const DEFAULT_BUDGET_MB: i64 = 13;
/// The core (no plugins) must stay under this (decision 30, fixed).
pub const CORE_BUDGET_MB: u64 = 10;

// Str and StrList have no registry key yet; plugin settings may use them later.
#[allow(dead_code)]
pub enum Kind {
    Enum(&'static [&'static str]),
    Int {
        min: i64,
        max: i64,
    },
    Float {
        min: f64,
        max: f64,
    },
    Bool,
    Str,
    Path,
    StrList,
    /// A name from [`MATRIX_COLORS`] or `#rrggbb`.
    Color,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Str(Cow<'static, str>),
    Int(i64),
    Float(f64),
    Bool(bool),
    List(Vec<String>),
}

impl Value {
    pub fn as_str(&self) -> &str {
        match self {
            Value::Str(s) => s,
            _ => "",
        }
    }

    pub fn as_i64(&self) -> i64 {
        match self {
            Value::Int(i) => *i,
            Value::Float(f) => *f as i64,
            _ => 0,
        }
    }

    pub fn as_f64(&self) -> f64 {
        match self {
            Value::Int(i) => *i as f64,
            Value::Float(f) => *f,
            _ => 0.0,
        }
    }

    pub fn as_bool(&self) -> bool {
        matches!(self, Value::Bool(true))
    }

    fn to_toml(&self) -> toml_edit::Value {
        match self {
            Value::Str(s) => toml_edit::Value::from(s.as_ref()),
            Value::Int(i) => toml_edit::Value::from(*i),
            Value::Float(f) => toml_edit::Value::from(*f),
            Value::Bool(b) => toml_edit::Value::from(*b),
            Value::List(items) => {
                toml_edit::Value::Array(items.iter().map(String::as_str).collect())
            }
        }
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&bare(&self.to_toml()))
    }
}

pub struct Setting {
    pub path: &'static str,
    pub kind: Kind,
    pub default: Value,
    pub tui_editable: bool,
    pub help: &'static str,
}

const fn setting(
    path: &'static str,
    kind: Kind,
    default: Value,
    tui_editable: bool,
    help: &'static str,
) -> Setting {
    Setting {
        path,
        kind,
        default,
        tui_editable,
        help,
    }
}

const fn int(min: i64, max: i64) -> Kind {
    Kind::Int { min, max }
}

const fn text(s: &'static str) -> Value {
    Value::Str(Cow::Borrowed(s))
}

pub static SETTINGS: &[Setting] = &[
    setting(
        "schema",
        int(1, 1),
        Value::Int(1),
        false,
        "format version of this file, do not change",
    ),
    setting(
        "general.theme",
        Kind::Enum(THEME_NAMES),
        text("matrix"),
        true,
        "minimalist | matrix",
    ),
    setting(
        "general.fps",
        int(1, 60),
        Value::Int(15),
        true,
        "frames per second for animated themes, 1..60",
    ),
    setting(
        "general.exit_on_any_key",
        Kind::Bool,
        Value::Bool(false),
        true,
        "true = screensaver mode: any key quits",
    ),
    setting(
        "general.plugins_dir",
        Kind::Path,
        text("plugins"),
        false,
        "relative to the executable, or an absolute path",
    ),
    setting(
        "general.log_file",
        Kind::Path,
        text(""),
        false,
        "\"\" = off, otherwise a file path",
    ),
    setting(
        "general.log_lines",
        int(50, 2000),
        Value::Int(200),
        true,
        "lines kept for the l overlay, 50..2000",
    ),
    setting(
        "units.temperature",
        Kind::Enum(&["celsius", "fahrenheit"]),
        text("celsius"),
        true,
        "celsius | fahrenheit",
    ),
    setting(
        "units.bytes",
        Kind::Enum(&["binary", "decimal"]),
        text("binary"),
        true,
        "binary = GiB, decimal = GB",
    ),
    setting(
        "metrics.cpu_interval_ms",
        int(200, 60_000),
        Value::Int(1000),
        true,
        "200..60000",
    ),
    setting(
        "metrics.memory_interval_ms",
        int(200, 60_000),
        Value::Int(1000),
        true,
        "200..60000",
    ),
    setting(
        "metrics.temps_interval_ms",
        int(1000, 60_000),
        Value::Int(5000),
        true,
        "1000..60000",
    ),
    setting(
        "metrics.disks_interval_ms",
        int(1000, 600_000),
        Value::Int(30_000),
        true,
        "1000..600000",
    ),
    setting(
        "metrics.history_len",
        int(10, 3600),
        Value::Int(240),
        true,
        "samples kept for sparklines, 10..3600",
    ),
    setting(
        "thresholds.cpu_warn_pct",
        int(1, 100),
        Value::Int(80),
        true,
        "highlight CPU above this, 1..100",
    ),
    setting(
        "thresholds.temp_warn_c",
        int(1, 150),
        Value::Int(75),
        true,
        "highlight temperature above this",
    ),
    setting(
        "thresholds.disk_warn_pct",
        int(1, 100),
        Value::Int(90),
        true,
        "highlight disks fuller than this",
    ),
    setting(
        "plugins.enabled",
        Kind::Bool,
        Value::Bool(true),
        true,
        "false = run no plugins at all",
    ),
    setting(
        "plugins.default_interval_s",
        int(5, 86_400),
        Value::Int(60),
        true,
        "used when a plugin sets no interval, 5..86400",
    ),
    setting(
        "plugins.http_timeout_s",
        int(1, 60),
        Value::Int(10),
        true,
        "per request, 1..60",
    ),
    setting(
        "plugins.call_timeout_s",
        int(1, 60),
        Value::Int(5),
        true,
        "Lua time limit per update(), 1..60",
    ),
    setting(
        "plugins.memory_limit_mb",
        int(1, 64),
        Value::Int(8),
        true,
        "per plugin, 1..64",
    ),
    setting(
        "plugins.rescan_interval_s",
        int(0, 86_400),
        Value::Int(60),
        true,
        "0 = rescan only on the r key",
    ),
    setting(
        "plugins.max_plugins",
        int(1, 64),
        Value::Int(16),
        true,
        "at most this many plugins run, 1..64",
    ),
    setting(
        "memory.budget_mb",
        int(5, 1024),
        Value::Int(DEFAULT_BUDGET_MB),
        true,
        "whole program, above it the status bar turns amber",
    ),
    setting(
        "memory.plugin_budget_mb",
        int(1, 64),
        Value::Int(1),
        true,
        "Lua memory per plugin, above it one log warning",
    ),
    setting(
        "theme.matrix.density",
        Kind::Float { min: 0.0, max: 1.0 },
        Value::Float(0.5),
        true,
        "0.0..1.0",
    ),
    setting(
        "theme.matrix.speed",
        Kind::Float { min: 0.1, max: 5.0 },
        Value::Float(1.0),
        true,
        "0.1..5.0",
    ),
    setting(
        "theme.matrix.color",
        Kind::Color,
        text("green"),
        true,
        "green | amber | cyan | white | #rrggbb",
    ),
    setting(
        "theme.minimalist.show_sparklines",
        Kind::Bool,
        Value::Bool(true),
        true,
        "history graphs under CPU and RAM",
    ),
];

const HEADER: &str = "\
# telemetrix settings. Every key is optional; a missing key uses the default shown here.
# Unknown keys are ignored with a warning. A wrong value falls back to its default.
# The running app picks up changes within 2 seconds after you save this file.
";

const PLUGIN_DEFAULTS: &str = r#"
# Per-plugin sections. `enabled` and `interval` are reserved; every other key
# reaches the plugin as telemetrix.settings.<key>.
[plugin.weather]
enabled = true
interval = 600
lat = 50.45
lon = 30.52
label = "Kyiv"

[plugin.crypto]
interval = 120
coins = ["bitcoin", "ethereum", "solana"]

[plugin.network_ping]
interval = 30
host = "1.1.1.1"
port = 443
"#;

pub fn find(path: &str) -> Option<&'static Setting> {
    SETTINGS.iter().find(|s| s.path == path)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TempUnit {
    Celsius,
    Fahrenheit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BytesUnit {
    Binary,
    Decimal,
}

#[derive(Clone, Debug, PartialEq)]
pub struct General {
    pub theme: String,
    pub fps: u32,
    pub exit_on_any_key: bool,
    pub plugins_dir: PathBuf,
    pub log_file: PathBuf,
    pub log_lines: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Units {
    pub temperature: TempUnit,
    pub bytes: BytesUnit,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    pub cpu_interval_ms: u64,
    pub memory_interval_ms: u64,
    pub temps_interval_ms: u64,
    pub disks_interval_ms: u64,
    pub history_len: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Thresholds {
    pub cpu_warn_pct: f32,
    pub temp_warn_c: f32,
    pub disk_warn_pct: f32,
}

/// Budgets only warn; nothing is stopped because of memory (decision 32).
#[derive(Clone, Debug, PartialEq)]
pub struct Memory {
    pub budget_mb: u64,
    pub plugin_budget_mb: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plugins {
    pub enabled: bool,
    pub default_interval_s: u64,
    pub http_timeout_s: u64,
    pub call_timeout_s: u64,
    pub memory_limit_mb: u64,
    pub rescan_interval_s: u64,
    pub max_plugins: usize,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThemeMatrix {
    pub density: f64,
    pub speed: f64,
    pub color: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThemeMinimalist {
    pub show_sparklines: bool,
}

#[derive(Clone, Debug, Default)]
pub struct PluginConfig {
    pub enabled: bool,
    pub interval: Option<u64>,
    pub settings: Table,
}

impl PartialEq for PluginConfig {
    fn eq(&self, other: &Self) -> bool {
        self.enabled == other.enabled
            && self.interval == other.interval
            && canonical(&self.settings) == canonical(&other.settings)
    }
}

fn canonical(t: &Table) -> Vec<(String, String)> {
    t.iter()
        .map(|(k, v)| (k.to_string(), v.as_value().map(bare).unwrap_or_default()))
        .collect()
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub general: General,
    pub units: Units,
    pub metrics: Metrics,
    pub thresholds: Thresholds,
    pub plugins: Plugins,
    pub memory: Memory,
    pub theme_matrix: ThemeMatrix,
    pub theme_minimalist: ThemeMinimalist,
    pub plugin_cfg: BTreeMap<String, PluginConfig>,
}

impl Default for Config {
    fn default() -> Self {
        let mut cfg = Config {
            general: General {
                theme: String::new(),
                fps: 0,
                exit_on_any_key: false,
                plugins_dir: PathBuf::new(),
                log_file: PathBuf::new(),
                log_lines: 0,
            },
            units: Units {
                temperature: TempUnit::Celsius,
                bytes: BytesUnit::Binary,
            },
            metrics: Metrics {
                cpu_interval_ms: 0,
                memory_interval_ms: 0,
                temps_interval_ms: 0,
                disks_interval_ms: 0,
                history_len: 0,
            },
            thresholds: Thresholds {
                cpu_warn_pct: 0.0,
                temp_warn_c: 0.0,
                disk_warn_pct: 0.0,
            },
            memory: Memory {
                budget_mb: 0,
                plugin_budget_mb: 0,
            },
            plugins: Plugins {
                enabled: false,
                default_interval_s: 0,
                http_timeout_s: 0,
                call_timeout_s: 0,
                memory_limit_mb: 0,
                rescan_interval_s: 0,
                max_plugins: 0,
            },
            theme_matrix: ThemeMatrix {
                density: 0.0,
                speed: 0.0,
                color: String::new(),
            },
            theme_minimalist: ThemeMinimalist {
                show_sparklines: false,
            },
            plugin_cfg: BTreeMap::new(),
        };
        for s in SETTINGS {
            cfg.assign(s.path, &s.default);
        }
        let doc =
            toml_edit::Document::parse(PLUGIN_DEFAULTS).expect("built-in plugin defaults parse");
        let mut problems = Vec::new();
        read_plugin_tables(&doc, PLUGIN_DEFAULTS, &mut cfg.plugin_cfg, &mut problems);
        cfg
    }
}

fn clamp_u64(v: &Value) -> u64 {
    v.as_i64().max(0) as u64
}

impl Config {
    /// Current value of one registry key.
    pub fn get(&self, path: &str) -> Option<Value> {
        let g = &self.general;
        let m = &self.metrics;
        let p = &self.plugins;
        let v = match path {
            "schema" => Value::Int(1),
            "general.theme" => Value::Str(g.theme.clone().into()),
            "general.fps" => Value::Int(g.fps.into()),
            "general.exit_on_any_key" => Value::Bool(g.exit_on_any_key),
            "general.plugins_dir" => {
                Value::Str(g.plugins_dir.to_string_lossy().into_owned().into())
            }
            "general.log_file" => Value::Str(g.log_file.to_string_lossy().into_owned().into()),
            "general.log_lines" => Value::Int(g.log_lines as i64),
            "units.temperature" => text(match self.units.temperature {
                TempUnit::Celsius => "celsius",
                TempUnit::Fahrenheit => "fahrenheit",
            }),
            "units.bytes" => text(match self.units.bytes {
                BytesUnit::Binary => "binary",
                BytesUnit::Decimal => "decimal",
            }),
            "metrics.cpu_interval_ms" => Value::Int(m.cpu_interval_ms as i64),
            "metrics.memory_interval_ms" => Value::Int(m.memory_interval_ms as i64),
            "metrics.temps_interval_ms" => Value::Int(m.temps_interval_ms as i64),
            "metrics.disks_interval_ms" => Value::Int(m.disks_interval_ms as i64),
            "metrics.history_len" => Value::Int(m.history_len as i64),
            "thresholds.cpu_warn_pct" => Value::Int(self.thresholds.cpu_warn_pct as i64),
            "thresholds.temp_warn_c" => Value::Int(self.thresholds.temp_warn_c as i64),
            "thresholds.disk_warn_pct" => Value::Int(self.thresholds.disk_warn_pct as i64),
            "plugins.enabled" => Value::Bool(p.enabled),
            "plugins.default_interval_s" => Value::Int(p.default_interval_s as i64),
            "plugins.http_timeout_s" => Value::Int(p.http_timeout_s as i64),
            "plugins.call_timeout_s" => Value::Int(p.call_timeout_s as i64),
            "plugins.memory_limit_mb" => Value::Int(p.memory_limit_mb as i64),
            "plugins.rescan_interval_s" => Value::Int(p.rescan_interval_s as i64),
            "plugins.max_plugins" => Value::Int(p.max_plugins as i64),
            "memory.budget_mb" => Value::Int(self.memory.budget_mb as i64),
            "memory.plugin_budget_mb" => Value::Int(self.memory.plugin_budget_mb as i64),
            "theme.matrix.density" => Value::Float(self.theme_matrix.density),
            "theme.matrix.speed" => Value::Float(self.theme_matrix.speed),
            "theme.matrix.color" => Value::Str(self.theme_matrix.color.clone().into()),
            "theme.minimalist.show_sparklines" => {
                Value::Bool(self.theme_minimalist.show_sparklines)
            }
            _ => return None,
        };
        Some(v)
    }

    /// Stores an already validated value for one registry key.
    pub fn assign(&mut self, path: &str, v: &Value) {
        let g = &mut self.general;
        let m = &mut self.metrics;
        let p = &mut self.plugins;
        match path {
            "general.theme" => g.theme = v.as_str().to_string(),
            "general.fps" => g.fps = v.as_i64() as u32,
            "general.exit_on_any_key" => g.exit_on_any_key = v.as_bool(),
            "general.plugins_dir" => g.plugins_dir = PathBuf::from(v.as_str()),
            "general.log_file" => g.log_file = PathBuf::from(v.as_str()),
            "general.log_lines" => g.log_lines = clamp_u64(v) as usize,
            "units.temperature" => {
                self.units.temperature = if v.as_str() == "fahrenheit" {
                    TempUnit::Fahrenheit
                } else {
                    TempUnit::Celsius
                }
            }
            "units.bytes" => {
                self.units.bytes = if v.as_str() == "decimal" {
                    BytesUnit::Decimal
                } else {
                    BytesUnit::Binary
                }
            }
            "metrics.cpu_interval_ms" => m.cpu_interval_ms = clamp_u64(v),
            "metrics.memory_interval_ms" => m.memory_interval_ms = clamp_u64(v),
            "metrics.temps_interval_ms" => m.temps_interval_ms = clamp_u64(v),
            "metrics.disks_interval_ms" => m.disks_interval_ms = clamp_u64(v),
            "metrics.history_len" => m.history_len = clamp_u64(v) as usize,
            "thresholds.cpu_warn_pct" => self.thresholds.cpu_warn_pct = v.as_f64() as f32,
            "thresholds.temp_warn_c" => self.thresholds.temp_warn_c = v.as_f64() as f32,
            "thresholds.disk_warn_pct" => self.thresholds.disk_warn_pct = v.as_f64() as f32,
            "plugins.enabled" => p.enabled = v.as_bool(),
            "plugins.default_interval_s" => p.default_interval_s = clamp_u64(v),
            "plugins.http_timeout_s" => p.http_timeout_s = clamp_u64(v),
            "plugins.call_timeout_s" => p.call_timeout_s = clamp_u64(v),
            "plugins.memory_limit_mb" => p.memory_limit_mb = clamp_u64(v),
            "plugins.rescan_interval_s" => p.rescan_interval_s = clamp_u64(v),
            "plugins.max_plugins" => p.max_plugins = clamp_u64(v) as usize,
            "memory.budget_mb" => self.memory.budget_mb = clamp_u64(v),
            "memory.plugin_budget_mb" => self.memory.plugin_budget_mb = clamp_u64(v),
            "theme.matrix.density" => self.theme_matrix.density = v.as_f64(),
            "theme.matrix.speed" => self.theme_matrix.speed = v.as_f64(),
            "theme.matrix.color" => self.theme_matrix.color = v.as_str().to_string(),
            "theme.minimalist.show_sparklines" => {
                self.theme_minimalist.show_sparklines = v.as_bool()
            }
            _ => {}
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum ConfigStatus {
    Ok,
    Warnings(Vec<String>),
    Syntax {
        line: Option<usize>,
        message: String,
    },
}

// Built at most once per reload, so the size of the Loaded variant does not matter.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum LoadOutcome {
    Loaded {
        config: Config,
        warnings: Vec<String>,
    },
    Syntax {
        line: Option<usize>,
        message: String,
    },
    Missing,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Problem {
    pub line: Option<usize>,
    pub message: String,
}

impl fmt::Display for Problem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.line {
            Some(line) => write!(f, "line {line}: {}", self.message),
            None => f.write_str(&self.message),
        }
    }
}

pub struct Parsed {
    pub config: Config,
    pub problems: Vec<Problem>,
    /// Registry keys whose value came from the file.
    pub from_file: BTreeSet<&'static str>,
}

pub fn load(path: &Path) -> LoadOutcome {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return LoadOutcome::Missing,
        Err(e) => {
            return LoadOutcome::Syntax {
                line: None,
                message: format!("cannot read {}: {e}", path.display()),
            };
        }
    };
    match parse_text(&text) {
        Ok(parsed) => LoadOutcome::Loaded {
            config: parsed.config,
            warnings: parsed.problems.iter().map(ToString::to_string).collect(),
        },
        Err(p) => LoadOutcome::Syntax {
            line: p.line,
            message: p.message,
        },
    }
}

fn line_of(text: &str, span: Option<std::ops::Range<usize>>) -> Option<usize> {
    let start = span?.start.min(text.len());
    Some(
        text.as_bytes()[..start]
            .iter()
            .filter(|b| **b == b'\n')
            .count()
            + 1,
    )
}

/// Lenient parse: a syntax error is the only `Err`; every other problem is
/// reported and replaced by the default of that one key.
pub fn parse_text(text: &str) -> Result<Parsed, Problem> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let doc = toml_edit::Document::parse(text).map_err(|e| Problem {
        line: line_of(text, e.span()),
        message: e.message().trim().to_string(),
    })?;
    let mut config = Config::default();
    let mut problems = Vec::new();
    let mut from_file = BTreeSet::new();
    for s in SETTINGS {
        let Some(item) = lookup(doc.as_table(), s.path) else {
            continue;
        };
        match validate(s, item) {
            Ok(v) => {
                config.assign(s.path, &v);
                from_file.insert(s.path);
            }
            Err(reason) => problems.push(Problem {
                line: line_of(text, item.span()),
                message: format!("{} = {} {reason}, using {}", s.path, shown(item), s.default),
            }),
        }
    }
    find_unknown(doc.as_table(), "", text, &mut problems);
    read_plugin_tables(&doc, text, &mut config.plugin_cfg, &mut problems);
    problems.sort_by_key(|p| p.line);
    Ok(Parsed {
        config,
        problems,
        from_file,
    })
}

fn lookup<'a>(root: &'a Table, path: &str) -> Option<&'a Item> {
    let mut parts = path.split('.');
    let mut item = root.get(parts.next()?)?;
    for part in parts {
        item = item.as_table_like()?.get(part)?;
    }
    Some(item)
}

fn bare(v: &toml_edit::Value) -> String {
    let mut v = v.clone();
    v.decor_mut().clear();
    v.to_string()
}

fn shown(item: &Item) -> String {
    item.as_value()
        .map(bare)
        .unwrap_or_else(|| "a table".into())
}

fn validate(s: &Setting, item: &Item) -> Result<Value, String> {
    let v = item.as_value().ok_or("is not a single value")?;
    match &s.kind {
        Kind::Int { min, max } => {
            let n = match v {
                toml_edit::Value::Integer(n) => *n.value(),
                toml_edit::Value::Float(_) => return Err("is not a whole number".into()),
                _ => return Err("is not a number".into()),
            };
            if n < *min || n > *max {
                return Err(format!("is out of range {min}..{max}"));
            }
            Ok(Value::Int(n))
        }
        Kind::Float { min, max } => {
            let f = match v {
                toml_edit::Value::Integer(n) => *n.value() as f64,
                toml_edit::Value::Float(f) => *f.value(),
                _ => return Err("is not a number".into()),
            };
            if !(f >= *min && f <= *max) {
                return Err(format!("is out of range {min:?}..{max:?}"));
            }
            Ok(Value::Float(f))
        }
        Kind::Bool => v
            .as_bool()
            .map(Value::Bool)
            .ok_or_else(|| "is not true or false".into()),
        Kind::Enum(options) => {
            let text = v.as_str().ok_or("is not a text value")?;
            if options.contains(&text) {
                Ok(Value::Str(text.to_string().into()))
            } else {
                Err(format!("is not one of {}", options.join(", ")))
            }
        }
        Kind::Str | Kind::Path => v
            .as_str()
            .map(|t| Value::Str(t.to_string().into()))
            .ok_or_else(|| "is not a text value".into()),
        Kind::StrList => v
            .as_array()
            .and_then(|a| a.iter().map(|x| x.as_str().map(str::to_string)).collect())
            .map(Value::List)
            .ok_or_else(|| "is not a list of text values".into()),
        Kind::Color => {
            let text = v.as_str().ok_or("is not a text value")?;
            if is_color(text) {
                Ok(Value::Str(text.to_string().into()))
            } else {
                Err(format!(
                    "is not one of {} or #rrggbb",
                    MATRIX_COLORS.join(", ")
                ))
            }
        }
    }
}

pub fn is_color(s: &str) -> bool {
    MATRIX_COLORS.contains(&s)
        || (s.len() == 7 && s.starts_with('#') && s[1..].chars().all(|c| c.is_ascii_hexdigit()))
}

fn find_unknown(table: &Table, prefix: &str, text: &str, problems: &mut Vec<Problem>) {
    for (key, item) in table.iter() {
        let path = if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{prefix}.{key}")
        };
        if path == "plugin" || find(&path).is_some() {
            continue;
        }
        let is_section = SETTINGS
            .iter()
            .any(|s| s.path.starts_with(&format!("{path}.")));
        match item.as_table() {
            Some(sub) if is_section => find_unknown(sub, &path, text, problems),
            _ => {
                let span = table
                    .key(key)
                    .and_then(|k| k.span())
                    .or_else(|| item.span());
                problems.push(Problem {
                    line: line_of(text, span),
                    message: format!("unknown key {path} ignored"),
                });
            }
        }
    }
}

fn read_plugin_tables(
    doc: &toml_edit::Document<&str>,
    text: &str,
    out: &mut BTreeMap<String, PluginConfig>,
    problems: &mut Vec<Problem>,
) {
    let Some(plugins) = doc.get("plugin").and_then(Item::as_table_like) else {
        return;
    };
    for (id, item) in plugins.iter() {
        let Some(table) = item.as_table_like() else {
            problems.push(Problem {
                line: line_of(text, item.span()),
                message: format!("plugin.{id} is not a table, ignored"),
            });
            continue;
        };
        let cfg = out.entry(id.to_string()).or_insert_with(|| PluginConfig {
            enabled: true,
            ..PluginConfig::default()
        });
        for (key, value) in table.iter() {
            let line = line_of(text, value.span());
            let bad = |what: &str| Problem {
                line,
                message: format!("plugin.{id}.{key} = {} {what}, ignored", shown(value)),
            };
            match key {
                "enabled" => match value.as_bool() {
                    Some(b) => cfg.enabled = b,
                    None => problems.push(bad("is not true or false")),
                },
                "interval" => match value.as_integer() {
                    Some(n) if n >= PLUGIN_MIN_INTERVAL => cfg.interval = Some(n as u64),
                    _ => problems.push(bad("is not a whole number of seconds, 5 or more")),
                },
                _ => {
                    cfg.settings.insert(key, value.clone());
                }
            }
        }
    }
}

/// Settings file path, effective settings (file, then flags) and status.
pub fn load_effective(flags: &Flags) -> (PathBuf, Config, ConfigStatus) {
    let path = resolve_path(flags.config.as_deref());
    let (mut cfg, status) = match load(&path) {
        LoadOutcome::Loaded { config, warnings } if warnings.is_empty() => {
            (config, ConfigStatus::Ok)
        }
        LoadOutcome::Loaded { config, warnings } => (config, ConfigStatus::Warnings(warnings)),
        LoadOutcome::Syntax { line, message } => {
            (Config::default(), ConfigStatus::Syntax { line, message })
        }
        LoadOutcome::Missing => (Config::default(), ConfigStatus::Ok),
    };
    apply_flags(&mut cfg, flags);
    (path, cfg, status)
}

/// `--config` → `TELEMETRIX_CONFIG` → next to the executable → OS config dir.
pub fn resolve_path(flag: Option<&Path>) -> PathBuf {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(Path::to_path_buf));
    resolve_from(flag, |k| std::env::var_os(k), exe_dir, |p| p.is_file())
}

fn resolve_from(
    flag: Option<&Path>,
    env: impl Fn(&str) -> Option<OsString>,
    exe_dir: Option<PathBuf>,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    if let Some(p) = flag {
        return p.to_path_buf();
    }
    if let Some(p) = env("TELEMETRIX_CONFIG").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    let beside_exe = exe_dir.map(|d| d.join(FILE_NAME));
    if let Some(p) = beside_exe.as_deref().filter(|p| exists(p)) {
        return p.to_path_buf();
    }
    let os_dir = if cfg!(windows) {
        env("APPDATA").map(PathBuf::from)
    } else {
        env("XDG_CONFIG_HOME")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".config")))
    };
    match os_dir {
        Some(dir) => dir.join("telemetrix").join(FILE_NAME),
        None => beside_exe.unwrap_or_else(|| PathBuf::from(FILE_NAME)),
    }
}

fn split_path(path: &str) -> (&str, &str) {
    path.rsplit_once('.').unwrap_or(("", path))
}

pub fn render_default_file() -> String {
    let mut out = String::from(HEADER);
    let mut section = "";
    for s in SETTINGS {
        let (table, key) = split_path(s.path);
        if table != section {
            out.push_str(&format!("\n[{table}]\n"));
            section = table;
        }
        let line = format!("{key} = {}", s.default);
        if s.help.is_empty() {
            out.push_str(&format!("{line}\n"));
        } else {
            out.push_str(&format!("{line:<27} # {}\n", s.help));
        }
    }
    out.push_str(PLUGIN_DEFAULTS);
    out
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// Sets one dotted key in the file, keeping every other line as it was.
pub fn set(path: &Path, key: &str, value: &Value) -> io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => render_default_file(),
        Err(e) => return Err(e),
    };
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
        invalid(format!(
            "cannot save, the file has a syntax error: {}",
            e.message().trim()
        ))
    })?;
    let parts: Vec<&str> = key.split('.').collect();
    let (last, tables) = parts.split_last().ok_or_else(|| invalid("empty key"))?;
    let mut table = doc.as_table_mut();
    for part in tables {
        table = table
            .entry(part)
            .or_insert_with(toml_edit::table)
            .as_table_mut()
            .ok_or_else(|| invalid(format!("{part} in {key} is not a table")))?;
    }
    let mut new_value = value.to_toml();
    if let Some(old) = table.get(last).and_then(Item::as_value) {
        *new_value.decor_mut() = keep_comment_column(old, &new_value);
    }
    table.insert(last, Item::Value(new_value));
    write_atomic(path, &doc.to_string())
}

/// Removes one dotted key from the file, so its default applies again.
pub fn unset(path: &Path, key: &str) -> io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut doc: DocumentMut = text.parse().map_err(|e: toml_edit::TomlError| {
        invalid(format!(
            "cannot save, the file has a syntax error: {}",
            e.message().trim()
        ))
    })?;
    let parts: Vec<&str> = key.split('.').collect();
    let Some((last, tables)) = parts.split_last() else {
        return Ok(());
    };
    let mut table = doc.as_table_mut();
    for part in tables {
        match table.get_mut(part).and_then(Item::as_table_mut) {
            Some(t) => table = t,
            None => return Ok(()),
        }
    }
    if table.remove(last).is_none() {
        return Ok(());
    }
    write_atomic(path, &doc.to_string())
}

/// Copies the old value's decor, adjusting the spaces before a trailing
/// comment so the `#` column stays where it was when there is room.
fn keep_comment_column(old: &toml_edit::Value, new: &toml_edit::Value) -> toml_edit::Decor {
    let mut decor = old.decor().clone();
    let suffix = decor.suffix().and_then(|s| s.as_str()).map(str::to_string);
    if let Some(suffix) = suffix {
        let trimmed = suffix.trim_start_matches(' ');
        if trimmed.starts_with('#') {
            let spaces = suffix.len() - trimmed.len();
            let old_len = bare(old).chars().count() as isize;
            let new_len = bare(new).chars().count() as isize;
            let wanted = (spaces as isize + old_len - new_len).max(1) as usize;
            decor.set_suffix(format!("{}{trimmed}", " ".repeat(wanted)));
        }
    }
    decor
}

pub fn write_atomic(path: &Path, contents: &str) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path)
}

pub fn apply_flags(cfg: &mut Config, flags: &Flags) {
    if let Some(theme) = &flags.theme {
        cfg.general.theme = theme.clone();
    }
    if let Some(fps) = flags.fps {
        cfg.general.fps = fps;
    }
    if let Some(dir) = &flags.plugins_dir {
        cfg.general.plugins_dir = dir.clone();
    }
    if flags.no_plugins {
        cfg.plugins.enabled = false;
    }
    if flags.exit_on_any_key {
        cfg.general.exit_on_any_key = true;
    }
    if let Some(log) = &flags.log {
        cfg.general.log_file = log.clone();
    }
}

/// The user changed `key` in the dashboard, so its flag stops overriding it.
pub fn release_flag(flags: &mut Flags, key: &str) {
    match key {
        "general.theme" => flags.theme = None,
        "general.fps" => flags.fps = None,
        "plugins.enabled" => flags.no_plugins = false,
        "general.exit_on_any_key" => flags.exit_on_any_key = false,
        _ => {}
    }
}

/// Registry keys that a command-line flag overrides.
pub fn flag_keys(flags: &Flags) -> BTreeSet<&'static str> {
    let mut keys = BTreeSet::new();
    let mut add = |on: bool, key| {
        if on {
            keys.insert(key);
        }
    };
    add(flags.theme.is_some(), "general.theme");
    add(flags.fps.is_some(), "general.fps");
    add(flags.plugins_dir.is_some(), "general.plugins_dir");
    add(flags.no_plugins, "plugins.enabled");
    add(flags.exit_on_any_key, "general.exit_on_any_key");
    add(flags.log.is_some(), "general.log_file");
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    const BAD: &str = include_str!("../tests/fixtures/bad.toml");

    #[test]
    fn default_file_parses_back_to_defaults() {
        let parsed = parse_text(&render_default_file()).expect("default file parses");
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        assert_eq!(parsed.config, Config::default());
    }

    #[test]
    fn default_file_matches_the_plan_layout() {
        let text = render_default_file();
        assert!(text.starts_with("# telemetrix settings."));
        assert!(text.contains("\ntheme = \"matrix\"            # minimalist | matrix\n"));
        assert!(text.contains("\nfps = 15                    # frames per second"));
        assert!(text.contains("\n[theme.matrix]\ndensity = 0.5"));
        assert!(text.contains("\n[plugin.crypto]\ninterval = 120\n"));
        assert!(text.contains("\n[memory]\nbudget_mb = 13 "));
        assert!(text.contains("\nplugin_budget_mb = 1 "));
    }

    #[test]
    fn bad_fixture_falls_back_per_key() {
        let parsed = parse_text(BAD).expect("fixture has no syntax error");
        let d = Config::default();
        let c = &parsed.config;
        assert_eq!(parsed.problems.len(), 4, "{:#?}", parsed.problems);
        assert!(parsed.problems.iter().all(|p| p.line.is_some()));
        let messages: Vec<&str> = parsed.problems.iter().map(|p| p.message.as_str()).collect();
        assert!(messages.contains(&"metrics.history_len = \"lots\" is not a number, using 240"));
        assert!(messages.contains(&"general.fps = 500 is out of range 1..60, using 15"));
        assert!(messages.contains(
            &"units.temperature = \"kelvin\" is not one of celsius, fahrenheit, using \"celsius\""
        ));
        assert!(
            parsed
                .problems
                .iter()
                .any(|p| p.message.contains("unknown key general.colour"))
        );
        assert_eq!(c.general.fps, d.general.fps);
        assert_eq!(c.metrics.history_len, d.metrics.history_len);
        assert_eq!(c.units.temperature, d.units.temperature);
        assert_eq!(c.general.theme, "minimalist");
        assert!(c.general.exit_on_any_key);
        assert_eq!(c.metrics.cpu_interval_ms, 2000);
        assert_eq!(c.theme_matrix.color, "#00ff88");
        assert_eq!(
            c.plugin_cfg["weather"].settings["label"].as_str(),
            Some("Lviv")
        );
        assert!(!c.plugin_cfg["crypto"].enabled);
    }

    #[test]
    fn syntax_error_reports_the_line() {
        let err = parse_text("schema = 1\n[general]\nfps = = 3\n")
            .err()
            .expect("syntax error");
        assert_eq!(err.line, Some(3));
        let dir = std::env::temp_dir().join(format!("telemetrix-syntax-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("t.toml");
        std::fs::write(&file, "[general\n").unwrap();
        assert!(matches!(
            load(&file),
            LoadOutcome::Syntax { line: Some(1), .. }
        ));
        assert!(matches!(
            load(&dir.join("missing.toml")),
            LoadOutcome::Missing
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn set_keeps_untouched_lines() {
        let dir = std::env::temp_dir().join(format!("telemetrix-set-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("telemetrix.toml");
        let before = render_default_file();
        std::fs::write(&file, &before).unwrap();
        set(&file, "general.fps", &Value::Int(30)).unwrap();
        set(&file, "theme.matrix.color", &text("amber")).unwrap();
        let after = std::fs::read_to_string(&file).unwrap();
        let (b, a): (Vec<&str>, Vec<&str>) = (before.lines().collect(), after.lines().collect());
        assert_eq!(b.len(), a.len());
        let changed: Vec<(&str, &str)> = b
            .iter()
            .zip(&a)
            .filter(|(x, y)| x != y)
            .map(|(x, y)| (*x, *y))
            .collect();
        assert_eq!(
            changed,
            vec![
                (
                    "fps = 15                    # frames per second for animated themes, 1..60",
                    "fps = 30                    # frames per second for animated themes, 1..60"
                ),
                (
                    "color = \"green\"             # green | amber | cyan | white | #rrggbb",
                    "color = \"amber\"             # green | amber | cyan | white | #rrggbb"
                ),
            ]
        );
        set(&file, "plugin.weather.interval", &Value::Int(30)).unwrap();
        set(&file, "plugin.clock.interval", &Value::Int(30)).unwrap();
        unset(&file, "plugin.weather.interval").unwrap();
        unset(&file, "plugin.clock.interval").unwrap();
        unset(&file, "plugin.nothing.here").unwrap();
        let parsed = parse_text(&std::fs::read_to_string(&file).unwrap()).unwrap();
        assert_eq!(
            parsed.config.plugin_cfg["weather"].interval,
            Some(600),
            "built-in default"
        );
        assert_eq!(parsed.config.plugin_cfg["clock"].interval, None);
        assert!(parsed.problems.is_empty());
        set(
            &dir.join("new").join("t.toml"),
            "general.theme",
            &text("minimalist"),
        )
        .unwrap();
        let created =
            parse_text(&std::fs::read_to_string(dir.join("new").join("t.toml")).unwrap()).unwrap();
        assert_eq!(created.config.general.theme, "minimalist");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn path_resolution_order() {
        let env = |k: &str| match k {
            "APPDATA" => Some(OsString::from("A")),
            "HOME" => Some(OsString::from("H")),
            _ => None,
        };
        let exe = Some(PathBuf::from("X"));
        let flag = Path::new("F.toml");
        assert_eq!(resolve_from(Some(flag), env, exe.clone(), |_| true), flag);
        let with_env = |k: &str| (k == "TELEMETRIX_CONFIG").then(|| OsString::from("E.toml"));
        assert_eq!(
            resolve_from(None, with_env, exe.clone(), |_| true),
            Path::new("E.toml")
        );
        assert_eq!(
            resolve_from(None, env, exe.clone(), |_| true),
            Path::new("X").join(FILE_NAME)
        );
        let os = resolve_from(None, env, exe, |_| false);
        let expected = if cfg!(windows) {
            Path::new("A")
        } else {
            Path::new("H/.config")
        };
        assert_eq!(os, expected.join("telemetrix").join(FILE_NAME));
    }

    #[test]
    fn flags_override_without_touching_the_file() {
        let mut cfg = Config::default();
        let flags = Flags {
            theme: Some("minimalist".into()),
            fps: Some(30),
            no_plugins: true,
            ..Flags::default()
        };
        apply_flags(&mut cfg, &flags);
        assert_eq!(cfg.general.theme, "minimalist");
        assert_eq!(cfg.general.fps, 30);
        assert!(!cfg.plugins.enabled);
        assert!(flag_keys(&flags).contains("plugins.enabled"));
    }

    #[test]
    fn get_and_assign_cover_every_setting() {
        let cfg = Config::default();
        for s in SETTINGS {
            assert_eq!(cfg.get(s.path).as_ref(), Some(&s.default), "{}", s.path);
        }
    }
}
