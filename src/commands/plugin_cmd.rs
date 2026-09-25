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
use crate::config::{self, Config};
use crate::plugins::PluginData;
use crate::plugins::manager::{discover, plugins_dir};
use crate::plugins::runner::{self, Plugin, RunnerSettings};

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
        PluginCmd::Check { file, json } => check(&file, json, &cfg),
        PluginCmd::List => list(&cfg),
    }
}

fn check(file: &Path, json: bool, cfg: &Config) -> ExitCode {
    let settings = RunnerSettings::from_config(cfg);
    let progress = Rc::new(|d: crate::plugins::PluginData| {
        let rows: Vec<String> = d
            .metrics
            .iter()
            .map(|m| format!("{} {}", m.label, m.value))
            .collect();
        eprintln!("progress: {}", rows.join(", "));
    });
    let data = runner::run_once(
        file,
        &settings,
        Rc::new(|msg: &str| eprintln!("log: {msg}")),
        progress,
    );
    if json {
        println!("{:#}", to_json(&data));
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
            let data = runner::run_once(&path, &settings, Rc::new(|_: &str| {}), runner::no_emit());
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
