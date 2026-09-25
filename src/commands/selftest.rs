//! `telemetrix selftest --memory`: the memory regression guard (decision 33).
//!
//! It starts the real dashboard twice, one run after
//! the other: once with the default settings and all plugins and once with
//! `--no-plugins`. Windows runs it in a hidden console, Linux in a
//! pseudo-terminal from util-linux `script`. Each
//! instance reads its own working set after the given seconds, writes it to
//! a report file and quits. Measuring inside the real process, in a real
//! console, counts everything the dashboard really loads (console code,
//! threads, plugins, TLS); a fake terminal backend would leave that out.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::app::AppState;
use crate::cli::Flags;
use crate::config::{self, CORE_BUDGET_MB};
use crate::selfmem::{self, MB};

/// Extra time for start-up and for the report to appear after the run.
const GRACE: Duration = Duration::from_secs(30);

/// The report a dashboard started with `--selftest-report` writes before it quits.
pub fn report_json(state: &AppState) -> Value {
    let own = selfmem::read();
    let lua: serde_json::Map<String, Value> = state
        .plugins
        .values()
        .filter_map(|c| Some((c.data.id.clone(), json!(c.data.lua_bytes?))))
        .collect();
    json!({
        "working_set_bytes": own.map(|m| m.working_set),
        "peak_working_set_bytes": own.and_then(|m| m.peak_working_set),
        "private_bytes": own.and_then(|m| m.private),
        "plugins": state.plugins.len(),
        "lua_bytes": lua,
    })
}

struct Run {
    label: &'static str,
    budget_mb: u64,
    report: PathBuf,
}

pub fn run(seconds: u64, json: bool, flags: &Flags) -> ExitCode {
    let (_, cfg, _) = config::load_effective(flags);
    let Ok(exe) = std::env::current_exe() else {
        eprintln!("selftest: cannot find the telemetrix executable");
        return ExitCode::FAILURE;
    };
    let dir = std::env::temp_dir().join(format!("telemetrix-selftest-{}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("selftest: cannot create {}: {e}", dir.display());
        return ExitCode::FAILURE;
    }
    if !json {
        let host = if cfg!(windows) {
            "hidden consoles"
        } else {
            "pseudo-terminals"
        };
        println!("memory selftest: {seconds} s per run, one run after the other, {host}");
    }
    // One at a time: two instances side by side measured 1-2 MB higher for the core.
    let results: Vec<(Run, Option<Value>)> = [
        ("total", cfg.memory.budget_mb, false),
        ("core", CORE_BUDGET_MB, true),
    ]
    .into_iter()
    .map(|(label, budget_mb, core)| {
        let run = Run {
            label,
            budget_mb,
            report: dir.join(format!("{label}.json")),
        };
        let mut args = vec![
            "--config".to_string(),
            // Never created: the run uses the default settings.
            dir.join(format!("{label}.toml")).display().to_string(),
            "--selftest-report".to_string(),
            run.report.display().to_string(),
            "--selftest-seconds".to_string(),
            seconds.to_string(),
            // Plugin stores of the test runs never touch the user's data.
            "--data-dir".to_string(),
            dir.join(format!("{label}-data")).display().to_string(),
        ];
        if core {
            args.push("--no-plugins".into());
        }
        if let Err(e) = start_hidden(&exe, &args) {
            eprintln!("selftest: cannot start the {label} run: {e}");
            return (run, None);
        }
        let end = Instant::now() + Duration::from_secs(seconds) + GRACE;
        let report = wait_for(&run.report, end);
        (run, report)
    })
    .collect();
    let _ = std::fs::remove_dir_all(&dir);
    print_results(&results, seconds, json)
}

fn wait_for(path: &Path, end: Instant) -> Option<Value> {
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            return serde_json::from_str(&text).ok();
        }
        if Instant::now() >= end {
            return None;
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Judged on the peak, so a spike between readings still fails the test.
fn verdict(report: &Value, budget_mb: u64) -> bool {
    report["peak_working_set_bytes"]
        .as_u64()
        .or_else(|| report["working_set_bytes"].as_u64())
        .is_some_and(|b| b <= budget_mb * MB)
}

fn print_results(results: &[(Run, Option<Value>)], seconds: u64, json: bool) -> ExitCode {
    let all_ok = results
        .iter()
        .all(|(r, rep)| rep.as_ref().is_some_and(|rep| verdict(rep, r.budget_mb)));
    if json {
        let mut doc = json!({ "seconds": seconds, "ok": all_ok });
        for (r, rep) in results {
            doc[r.label] = match rep {
                Some(rep) => {
                    let mut v = rep.clone();
                    v["budget_mb"] = json!(r.budget_mb);
                    v["ok"] = json!(verdict(rep, r.budget_mb));
                    v
                }
                None => json!({ "budget_mb": r.budget_mb, "ok": false, "error": "no report" }),
            };
        }
        println!("{doc:#}");
    } else {
        let mb = |v: &Value| {
            v.as_u64()
                .map_or("-".to_string(), |b| format!("{:.1} MB", selfmem::mb(b)))
        };
        println!(
            "        {:>9} {:>9} {:>9} {:>7}  result",
            "final", "peak", "private", "budget"
        );
        for (r, rep) in results {
            match rep {
                Some(rep) => println!(
                    "{:<6}  {:>9} {:>9} {:>9} {:>4} MB  {}  ({} plugins)",
                    r.label,
                    mb(&rep["working_set_bytes"]),
                    mb(&rep["peak_working_set_bytes"]),
                    mb(&rep["private_bytes"]),
                    r.budget_mb,
                    if verdict(rep, r.budget_mb) {
                        "ok"
                    } else {
                        "OVER BUDGET"
                    },
                    rep["plugins"],
                ),
                None => println!("{:<6}  no report: the run did not finish", r.label),
            }
        }
    }
    if all_ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// A hidden legacy console: calling conhost.exe directly keeps Windows 11 from
/// handing the program to Windows Terminal, and nothing appears on screen.
#[cfg(windows)]
fn start_hidden(exe: &Path, args: &[String]) -> std::io::Result<()> {
    let line: Vec<String> = std::iter::once(exe.display().to_string())
        .chain(args.iter().cloned())
        .map(|a| format!("\"{a}\""))
        .collect();
    let script = format!(
        "Start-Process -WindowStyle Hidden -FilePath conhost.exe -ArgumentList '{}'",
        line.join(" ").replace('\'', "''")
    );
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!(
            "powershell exited with {status}"
        )))
    }
}

/// `script` gives the dashboard a pseudo-terminal without a window.
#[cfg(not(windows))]
fn start_hidden(exe: &Path, args: &[String]) -> std::io::Result<()> {
    let quote = |s: &str| format!("'{}'", s.replace('\'', r"'\''"));
    let line: Vec<String> = std::iter::once(exe.display().to_string())
        .chain(args.iter().cloned())
        .map(|a| quote(&a))
        .collect();
    Command::new("script")
        .args(["-qec", &line.join(" "), "/dev/null"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verdict_uses_the_peak() {
        let rep = json!({ "working_set_bytes": 9 * MB, "peak_working_set_bytes": 11 * MB });
        assert!(!verdict(&rep, 10));
        assert!(verdict(&rep, 11));
        let no_peak = json!({ "working_set_bytes": 9 * MB, "peak_working_set_bytes": null });
        assert!(verdict(&no_peak, 10));
        assert!(!verdict(&json!({}), 10));
    }
}
