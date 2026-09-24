use std::collections::BTreeSet;
use std::path::Path;
use std::process::ExitCode;

use crate::cli::{ConfigCmd, Flags};
use crate::config::{self, Config, Problem, SETTINGS};

pub fn run(cmd: ConfigCmd, flags: &Flags) -> ExitCode {
    let path = config::resolve_path(flags.config.as_deref());
    match cmd {
        ConfigCmd::Init { force } => init(&path, force),
        ConfigCmd::Path => {
            let state = if path.is_file() {
                "exists"
            } else {
                "does not exist, defaults are used"
            };
            println!("{} ({state})", path.display());
            ExitCode::SUCCESS
        }
        ConfigCmd::Check { json } => check(&path, json),
        ConfigCmd::Show => show(&path, flags),
    }
}

fn init(path: &Path, force: bool) -> ExitCode {
    if path.exists() && !force {
        eprintln!(
            "{} already exists. Use --force to replace it.",
            path.display()
        );
        return ExitCode::FAILURE;
    }
    match config::write_atomic(path, &config::render_default_file()) {
        Ok(()) => {
            println!("wrote {}", path.display());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("cannot write {}: {e}", path.display());
            ExitCode::FAILURE
        }
    }
}

/// `None` = no file. `Err` = unreadable or a syntax error.
fn read(path: &Path) -> Option<Result<config::Parsed, Problem>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Some(config::parse_text(&text)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => Some(Err(Problem {
            line: None,
            message: format!("cannot read the file: {e}"),
        })),
    }
}

fn check(path: &Path, json: bool) -> ExitCode {
    let parsed = read(path);
    let problems: Vec<Problem> = match &parsed {
        None => Vec::new(),
        Some(Ok(p)) => p.problems.clone(),
        Some(Err(p)) => vec![p.clone()],
    };
    if json {
        let list: Vec<_> = problems
            .iter()
            .map(|p| serde_json::json!({ "line": p.line, "message": p.message }))
            .collect();
        let doc = serde_json::json!({
            "path": path.display().to_string(),
            "exists": parsed.is_some(),
            "ok": problems.is_empty(),
            "problems": list,
        });
        println!("{doc:#}");
    } else if parsed.is_none() {
        println!("{}: no file, defaults are used", path.display());
    } else if problems.is_empty() {
        println!("{}: ok", path.display());
    } else {
        println!("{}: {} problem(s)", path.display(), problems.len());
        for p in &problems {
            println!("  {p}");
        }
    }
    if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

fn show(path: &Path, flags: &Flags) -> ExitCode {
    let (mut cfg, from_file, status) = match read(path) {
        None => (
            Config::default(),
            BTreeSet::new(),
            "no file, defaults".to_string(),
        ),
        Some(Ok(p)) => {
            let status = match p.problems.len() {
                0 => "ok".to_string(),
                n => format!("{n} problem(s), see config check"),
            };
            (p.config, p.from_file, status)
        }
        Some(Err(p)) => (
            Config::default(),
            BTreeSet::new(),
            format!("{p}, defaults used"),
        ),
    };
    config::apply_flags(&mut cfg, flags);
    let flagged = config::flag_keys(flags);
    println!("# {} ({status})", path.display());
    for s in SETTINGS {
        let source = if flagged.contains(s.path) {
            "flag"
        } else if from_file.contains(s.path) {
            "file"
        } else {
            "default"
        };
        let value = cfg.get(s.path).map(|v| v.to_string()).unwrap_or_default();
        println!("{:<34} = {value:<24} ({source})", s.path);
    }
    for (id, p) in &cfg.plugin_cfg {
        let interval = p
            .interval
            .map_or("plugin default".to_string(), |i| i.to_string());
        println!(
            "plugin.{id}: enabled = {}, interval = {interval}",
            p.enabled
        );
        for (key, item) in p.settings.iter() {
            let value = item.as_value().map(|v| v.to_string()).unwrap_or_default();
            println!("  {key} = {}", value.trim());
        }
    }
    ExitCode::SUCCESS
}
