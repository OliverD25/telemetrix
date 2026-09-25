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
                .map(|m| json!({ "label": m.label, "value": m.value, "trend": m.trend }))
                .collect();
            json!({ "id": d.id, "title": d.title, "ok": true, "metrics": metrics })
        }
    }
}

pub fn run(cmd: PluginCmd, flags: &Flags) -> ExitCode {
    let (_, cfg, _) = config::load_effective(flags);
    match cmd {
        PluginCmd::Check { file, json, run } => check(&file, json, run, &cfg),
        PluginCmd::List => list(&cfg),
    }
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

fn list(cfg: &Config) -> ExitCode {
    let dir = plugins_dir(cfg);
    let settings = RunnerSettings::from_config(cfg);
    println!("plugins in {}", dir.display());
    let files = discover(&dir);
    if files.is_empty() {
        println!("  (none)");
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
                    "  {:<16} {:>6} s  {enabled:<8}  {}",
                    p.id(),
                    interval,
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
pub fn run_all_once(cfg: &Config) -> Vec<Value> {
    if !cfg.plugins.enabled {
        return Vec::new();
    }
    let settings = RunnerSettings::from_config(cfg);
    let files: Vec<PathBuf> = discover(&plugins_dir(cfg))
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
