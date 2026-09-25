use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::cli::{Flags, PluginCmd};
use crate::config::{self, Config, Value as ConfigValue};
use crate::plugins::PluginData;
use crate::plugins::bundled;
use crate::plugins::host_api::Trigger;
use crate::plugins::manager::{discover, plugins_dir};
use crate::plugins::runner::{self, Plugin, RunnerSettings};
use crate::plugins::schema::{SchemaEntry, SchemaKind};

/// `snapshot --plugins` waits at most this long for all plugins together.
const ALL_PLUGINS_CAP: Duration = Duration::from_secs(10);

pub fn to_json(d: &PluginData) -> Value {
    match &d.error {
        Some(error) => json!({ "id": d.id, "title": d.title, "ok": false, "error": error }),
        None => {
            let metrics: Vec<Value> = d
                .metrics
                .iter()
                .map(|m| {
                    json!({
                        "label": m.label,
                        "value": m.value,
                        "trend": m.trend,
                        "style": m.style.map(|s| s.name()),
                    })
                })
                .collect();
            json!({ "id": d.id, "title": d.title, "ok": true, "metrics": metrics })
        }
    }
}

pub fn run(cmd: PluginCmd, flags: &Flags) -> ExitCode {
    let (path, cfg, _) = config::load_effective(flags);
    match cmd {
        PluginCmd::Check { file, json, run } => check(&file, json, run, &cfg),
        PluginCmd::List => list(&cfg, &path),
        PluginCmd::Install { force, names } => {
            let home = bundled::home(&path);
            println!("built-in plugins in {}", home.display());
            match install(&home, force, &names) {
                Ok(lines) => {
                    for line in lines {
                        println!("  {line}");
                    }
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{e}");
                    ExitCode::FAILURE
                }
            }
        }
    }
}

/// `plugin install`: the start-up rules, limited to `names` when given;
/// `force` also replaces edited files and restores deleted ones.
fn install(home: &Path, force: bool, names: &[String]) -> Result<Vec<String>, String> {
    let mut files = Vec::new();
    for name in names {
        match bundled::file_name(name) {
            Some(file) => files.push(file),
            None => {
                let known: Vec<&str> = bundled::FILES
                    .iter()
                    .filter_map(|(f, _)| f.strip_suffix(".lua"))
                    .collect();
                return Err(format!(
                    "{name} is not a built-in plugin; the built-in ones are {}",
                    known.join(", ")
                ));
            }
        }
    }
    let report = bundled::sync(home, &files, force)
        .map_err(|e| format!("cannot write to {}: {e}", home.display()))?;
    Ok(report.iter().map(|(f, o)| o.describe(f)).collect())
}

fn schema_json(schema: &[SchemaEntry]) -> Value {
    let entries: Vec<Value> = schema
        .iter()
        .map(|e| {
            let mut v = json!({
                "key": e.key,
                "label": e.label,
                "kind": e.kind_name(),
                "default": match &e.default {
                    ConfigValue::Str(s) => json!(s),
                    ConfigValue::Int(n) => json!(n),
                    ConfigValue::Bool(b) => json!(b),
                    other => json!(other.to_string()),
                },
            });
            match &e.kind {
                SchemaKind::Enum(options) => v["options"] = json!(options),
                SchemaKind::Int { min, max, step } => {
                    v["min"] = json!(min);
                    v["max"] = json!(max);
                    v["step"] = json!(step);
                }
                SchemaKind::Text | SchemaKind::Bool => {}
            }
            v
        })
        .collect();
    Value::Array(entries)
}

/// One line per schema key: `city  text  default "Kyiv"  City`.
fn schema_lines(schema: &[SchemaEntry]) -> Vec<String> {
    let width = schema.iter().map(|e| e.key.len()).max().unwrap_or(0);
    schema
        .iter()
        .map(|e| {
            let range = match &e.kind {
                SchemaKind::Enum(options) => format!(" ({})", options.join(" | ")),
                SchemaKind::Int { min, max, step } if *step > 1 => {
                    format!(" ({min}..{max} step {step})")
                }
                SchemaKind::Int { min, max, .. } => format!(" ({min}..{max})"),
                SchemaKind::Text | SchemaKind::Bool => String::new(),
            };
            format!(
                "  {:<width$}  {}{range}, default {}  \"{}\"",
                e.key,
                e.kind_name(),
                e.default,
                e.label
            )
        })
        .collect()
}

fn check(file: &Path, json: bool, run: bool, cfg: &Config) -> ExitCode {
    let settings = RunnerSettings::from_config(cfg);
    let progress = Rc::new(|d: crate::plugins::PluginData| {
        let rows: Vec<String> = d
            .metrics
            .iter()
            .map(|m| format!("{} {}", m.label, m.value))
            .collect();
        eprintln!("progress: {}", rows.join(", "));
    });
    let trigger = if run { Trigger::Manual } else { Trigger::Start };
    let log = Rc::new(|msg: &str| eprintln!("log: {msg}"));
    let stop = Arc::new(AtomicBool::new(false));
    let (data, schema) = match Plugin::load(file, &settings, stop, log, progress) {
        Ok(p) => {
            let data = p
                .update(&settings, trigger)
                .unwrap_or_else(|e| runner::error_data(p.id(), p.title(), e));
            (data, p.schema().to_vec())
        }
        Err(e) => {
            let stem = runner::stem(file);
            (runner::error_data(&stem, &stem, e), Vec::new())
        }
    };
    if json {
        let mut doc = to_json(&data);
        doc["settings_schema"] = schema_json(&schema);
        println!("{doc:#}");
    } else if let Some(error) = &data.error {
        println!("{} ({}): error: {error}", data.title, data.id);
    } else {
        println!("{} ({})", data.title, data.id);
        let width = data
            .metrics
            .iter()
            .map(|m| m.label.chars().count())
            .max()
            .unwrap_or(0);
        for m in &data.metrics {
            println!("  {:<width$}  {}", m.label, m.value);
        }
    }
    if !json && !schema.is_empty() {
        println!("settings_schema:");
        for line in schema_lines(&schema) {
            println!("{line}");
        }
    }
    if data.error.is_some() {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn list(cfg: &Config, settings_path: &Path) -> ExitCode {
    let dir = plugins_dir(cfg, settings_path);
    let settings = RunnerSettings::from_config(cfg);
    println!("plugin home: {}", bundled::home(settings_path).display());
    println!("plugins in {}", dir.display());
    let files = discover(&dir);
    if files.is_empty() {
        println!("  (none)");
        if dir == bundled::home(settings_path) {
            println!(
                "  The dashboard installs the built-in plugins here when it starts; telemetrix plugin install does it now."
            );
        }
    }
    for (path, _) in files {
        let stop = Arc::new(AtomicBool::new(false));
        match Plugin::load(
            &path,
            &settings,
            stop,
            Rc::new(|_: &str| {}),
            runner::no_emit(),
        ) {
            Ok(p) => {
                let enabled = if cfg.plugins.enabled && settings.enabled(p.id()) {
                    "enabled"
                } else {
                    "disabled"
                };
                let interval = p.interval(&settings).as_secs();
                println!(
                    "  {:<16} {:>6} s  {enabled:<8}  {:<15}  {}",
                    p.id(),
                    interval,
                    bundled::origin(&path).label(),
                    path.display()
                );
            }
            Err(e) => println!(
                "  {:<16} error: {e}  {}",
                runner::stem(&path),
                path.display()
            ),
        }
    }
    ExitCode::SUCCESS
}

/// Runs every enabled plugin once, in parallel, for `snapshot --plugins`.
pub fn run_all_once(cfg: &Config, settings_path: &Path) -> Vec<Value> {
    if !cfg.plugins.enabled {
        return Vec::new();
    }
    let settings = RunnerSettings::from_config(cfg);
    let files: Vec<PathBuf> = discover(&plugins_dir(cfg, settings_path))
        .into_iter()
        .map(|(p, _)| p)
        .filter(|p| settings.enabled(&runner::stem(p)))
        .take(cfg.plugins.max_plugins)
        .collect();
    let (tx, rx) = mpsc::channel();
    for (i, path) in files.iter().enumerate() {
        let (tx, path, settings) = (tx.clone(), path.clone(), settings.clone());
        thread::spawn(move || {
            // A fresh process that runs each plugin once is a start, not a key press.
            let data = runner::run_once(
                &path,
                &settings,
                Trigger::Start,
                Rc::new(|_: &str| {}),
                runner::no_emit(),
            );
            let _ = tx.send((i, data));
        });
    }
    drop(tx);
    let end = Instant::now() + ALL_PLUGINS_CAP;
    let mut results: Vec<Option<PluginData>> = vec![None; files.len()];
    while let Ok((i, data)) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
        results[i] = Some(data);
    }
    results
        .into_iter()
        .zip(&files)
        .map(|(data, path)| {
            let stem = runner::stem(path);
            let data = data.unwrap_or_else(|| {
                runner::error_data(
                    &stem,
                    &stem,
                    format!("no answer within {} s", ALL_PLUGINS_CAP.as_secs()),
                )
            });
            to_json(&data)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Value as V;

    #[test]
    fn install_force_replaces_and_names_must_be_built_in() {
        let home = std::env::temp_dir().join(format!("telemetrix-install-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        let lines = install(&home, false, &[]).unwrap();
        assert_eq!(lines.len(), 7);
        assert!(
            lines.iter().all(|l| l.ends_with(": installed")),
            "{lines:?}"
        );
        std::fs::write(home.join("weather.lua"), "-- mine").unwrap();
        std::fs::remove_file(home.join("clock.lua")).unwrap();
        let lines = install(&home, false, &["weather".into()]).unwrap();
        assert!(lines[0].starts_with("weather.lua was changed by you"));
        assert_eq!(
            std::fs::read_to_string(home.join("weather.lua")).unwrap(),
            "-- mine"
        );
        let lines = install(&home, true, &["weather".into()]).unwrap();
        assert_eq!(lines, ["weather.lua: replaced with the built-in version"]);
        assert!(!home.join("clock.lua").exists(), "not named: stays deleted");
        let lines = install(&home, true, &[]).unwrap();
        assert!(lines.contains(&"clock.lua: replaced with the built-in version".to_string()));
        assert!(home.join("clock.lua").is_file());
        assert!(install(&home, true, &["nope".into()]).is_err());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn schema_prints_one_line_per_key() {
        let schema = vec![
            SchemaEntry {
                key: "city".into(),
                label: "City".into(),
                kind: SchemaKind::Text,
                default: V::Str("Kyiv".into()),
            },
            SchemaEntry {
                key: "download_mb".into(),
                label: "Download MB".into(),
                kind: SchemaKind::Int {
                    min: 5,
                    max: 100,
                    step: 5,
                },
                default: V::Int(25),
            },
        ];
        let lines = schema_lines(&schema);
        assert_eq!(lines[0], "  city         text, default \"Kyiv\"  \"City\"");
        assert_eq!(
            lines[1],
            "  download_mb  int (5..100 step 5), default 25  \"Download MB\""
        );
        let j = schema_json(&schema);
        assert_eq!(j[1]["step"], 5);
        assert_eq!(j[0]["kind"], "text");
        assert_eq!(
            (j[0]["default"].as_str(), j[1]["default"].as_i64()),
            (Some("Kyiv"), Some(25))
        );
    }
}
