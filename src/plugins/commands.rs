//! Running the programs listed in `[commands]` for `telemetrix.run`.
//!
//! The user's settings file decides what can run: a plugin passes only a
//! name, never a program or an argument, and nothing goes through a shell.
//! Each run has a time limit (the process is killed after it) and keeps at
//! most `OUTPUT_CAP` bytes of each output.

use std::collections::BTreeMap;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
pub const MAX_TIMEOUT: Duration = Duration::from_secs(120);
pub const OUTPUT_CAP: usize = 64 * 1024;
/// After the program ends, its outputs get this long to arrive; a child
/// it left behind may keep them open.
const DRAIN: Duration = Duration::from_secs(2);
const POLL: Duration = Duration::from_millis(20);

pub type Commands = BTreeMap<String, Vec<String>>;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    /// `None` when the program ended without an exit code (a signal).
    pub code: Option<i32>,
}

/// The time limit a plugin asked for, within 1 s..`MAX_TIMEOUT`.
pub fn timeout(seconds: Option<f64>) -> Duration {
    match seconds {
        Some(s) if s.is_finite() => {
            Duration::from_secs_f64(s.clamp(1.0, MAX_TIMEOUT.as_secs_f64()))
        }
        _ => DEFAULT_TIMEOUT,
    }
}

/// Reads a pipe to its end, keeping the first `OUTPUT_CAP` bytes. The
/// rest is read and dropped, so a talkative program never blocks.
fn drain(mut pipe: impl Read + Send + 'static) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut kept = Vec::new();
        let mut buf = [0u8; 8192];
        while let Ok(n) = pipe.read(&mut buf) {
            if n == 0 {
                break;
            }
            let room = OUTPUT_CAP.saturating_sub(kept.len());
            kept.extend_from_slice(&buf[..n.min(room)]);
        }
        let _ = tx.send(kept);
    });
    rx
}

fn text(rx: &mpsc::Receiver<Vec<u8>>) -> String {
    String::from_utf8_lossy(&rx.recv_timeout(DRAIN).unwrap_or_default()).into_owned()
}

/// Runs `commands[name]`; `Err` for an unknown name, a program that does
/// not start, or the time limit.
pub fn run(commands: &Commands, name: &str, limit: Duration) -> Result<Output, String> {
    let argv = commands
        .get(name)
        .ok_or_else(|| format!("no command {name:?} in [commands] of telemetrix.toml"))?;
    let (program, args) = argv
        .split_first()
        .ok_or_else(|| format!("command {name:?} is empty"))?;
    let mut cmd = Command::new(program);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console window flashes up for a program started from the dashboard.
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("cannot start {program}: {e}"))?;
    let stdout = drain(child.stdout.take().expect("stdout is piped"));
    let stderr = drain(child.stderr.take().expect("stderr is piped"));
    let end = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < end => thread::sleep(POLL),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "{name} did not end within {} s and was stopped",
                    limit.as_secs_f64().round()
                ));
            }
            Err(e) => return Err(format!("{name}: {e}")),
        }
    };
    Ok(Output {
        stdout: text(&stdout),
        stderr: text(&stderr),
        code: status.code(),
    })
}

/// Runs several commands at the same time; the results come in the order
/// of `names`.
pub fn run_all(
    commands: &Commands,
    names: &[String],
    limit: Duration,
) -> Vec<Result<Output, String>> {
    thread::scope(|scope| {
        let handles: Vec<_> = names
            .iter()
            .map(|name| scope.spawn(move || run(commands, name, limit)))
            .collect();
        handles
            .into_iter()
            .map(|h| {
                h.join()
                    .unwrap_or_else(|_| Err("the command thread failed".into()))
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A shell is fine in a test: the command list is written here.
    fn sh(script: &str) -> Vec<String> {
        if cfg!(windows) {
            vec!["cmd".into(), "/C".into(), script.into()]
        } else {
            vec!["sh".into(), "-c".into(), script.into()]
        }
    }

    fn commands() -> Commands {
        let mut c = Commands::new();
        c.insert("hello".into(), sh("echo hello&& echo oops 1>&2&& exit 3"));
        c.insert(
            "slow".into(),
            if cfg!(windows) {
                vec!["ping".into(), "-n".into(), "30".into(), "127.0.0.1".into()]
            } else {
                vec!["sleep".into(), "30".into()]
            },
        );
        c.insert(
            "loud".into(),
            if cfg!(windows) {
                sh("for /L %i in (1,1,3000) do @echo 0123456789012345678901234567890123456789")
            } else {
                sh("yes 0123456789012345678901234567890123456789 | head -n 3000")
            },
        );
        c.insert("missing".into(), vec!["telemetrix-no-such-program".into()]);
        c
    }

    #[test]
    fn runs_only_listed_commands_and_reports_everything() {
        let c = commands();
        let out = run(&c, "hello", DEFAULT_TIMEOUT).unwrap();
        assert_eq!(out.stdout.trim(), "hello");
        assert_eq!(out.stderr.trim(), "oops");
        assert_eq!(out.code, Some(3));
        let err = run(&c, "rm", DEFAULT_TIMEOUT).unwrap_err();
        assert!(err.contains("no command \"rm\""), "{err}");
        assert!(
            run(&c, "missing", DEFAULT_TIMEOUT)
                .unwrap_err()
                .starts_with("cannot start")
        );
    }

    #[test]
    fn the_time_limit_stops_the_program() {
        let start = Instant::now();
        let err = run(&commands(), "slow", Duration::from_secs(1)).unwrap_err();
        assert!(err.contains("did not end within 1 s"), "{err}");
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
    }

    #[test]
    fn output_is_capped() {
        let out = run(&commands(), "loud", DEFAULT_TIMEOUT).unwrap();
        assert_eq!(out.stdout.len(), OUTPUT_CAP);
        assert_eq!(out.code, Some(0));
    }

    #[test]
    fn several_commands_run_at_once() {
        let c = commands();
        let names = ["slow".to_string(), "hello".to_string(), "nope".to_string()];
        let start = Instant::now();
        let results = run_all(&c, &names, Duration::from_secs(1));
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(results[0].is_err(), "slow hits the limit");
        assert_eq!(results[1].as_ref().unwrap().code, Some(3));
        assert!(results[2].is_err());
    }

    #[test]
    fn timeouts_stay_in_range() {
        assert_eq!(timeout(None), DEFAULT_TIMEOUT);
        assert_eq!(timeout(Some(0.1)), Duration::from_secs(1));
        assert_eq!(timeout(Some(500.0)), MAX_TIMEOUT);
        assert_eq!(timeout(Some(f64::NAN)), DEFAULT_TIMEOUT);
        assert_eq!(timeout(Some(10.0)), Duration::from_secs(10));
    }
}
