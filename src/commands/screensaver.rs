//! `telemetrix screensaver install|uninstall|status|watch`.
//!
//! Windows has no reliable "run after N idle minutes" trigger: Task
//! Scheduler's idle condition also waits for low CPU and disk use and is
//! checked only every few minutes. So a small watcher process runs from
//! logon, reads the time since the last key or mouse input
//! (`GetLastInputInfo`) every few seconds, and opens the dashboard full
//! screen when that time passes the limit. `install` only registers the
//! watcher as a Task Scheduler logon task; the logon trigger is reliable,
//! runs in the user's own session and needs no administrator rights.
//!
//! The task runs a copy of the program (`telemetrix-watch.exe` in the data
//! folder), not the installed `telemetrix.exe`. A running exe cannot be
//! replaced on Windows, so a watcher started from the installed file would
//! make `cargo install` fail with "Access is denied". The copy still opens
//! the dashboard from the installed file, whose path `install` records.
//!
//! On Linux the idle daemons (swayidle, xautolock) already do this job, so
//! `install` only prints the line to add.

// Most of this module is the Windows watcher; Linux builds use only a part.
#![cfg_attr(not(windows), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Duration;

use crate::cli::{Flags, ScreensaverCmd};

pub const TASK_NAME: &str = "telemetrix screensaver";
/// The file name of the copy the task runs.
pub const WATCH_EXE: &str = "telemetrix-watch.exe";
pub const DEFAULT_IDLE_MINUTES: u32 = 10;
/// How often the watcher looks at the idle time.
pub const POLL: Duration = Duration::from_secs(5);
/// Held by the watcher, so a second one ends at once.
const WATCH_MUTEX: &str = r"Local\telemetrix-screensaver-watch";
/// `uninstall` sets it; the watcher sleeps on it and ends when it is set.
const STOP_EVENT: &str = r"Local\telemetrix-screensaver-stop";
/// Held by a dashboard started with `--screensaver`, so the watcher does
/// not start a second one while it runs.
pub const DASHBOARD_MUTEX: &str = r"Local\telemetrix-screensaver-dashboard";

/// What the watcher sees at one poll.
#[derive(Clone, Copy, Debug)]
pub struct Observation {
    pub idle: Duration,
    pub dashboard_running: bool,
    /// A program asks Windows to keep the display on, like a video player.
    pub display_required: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decision {
    Wait,
    Launch,
}

/// The watcher's decision: one launch per idle period. Input re-arms it, so
/// a dashboard that closed without input (or failed to start) is not
/// started again and again.
pub struct IdleWatch {
    limit: Duration,
    armed: bool,
}

impl IdleWatch {
    pub fn new(limit: Duration) -> Self {
        Self { limit, armed: true }
    }

    pub fn step(&mut self, o: Observation) -> Decision {
        if o.idle < self.limit {
            self.armed = true;
            return Decision::Wait;
        }
        if !self.armed || o.dashboard_running || o.display_required {
            return Decision::Wait;
        }
        self.armed = false;
        Decision::Launch
    }
}

/// Milliseconds since the last input from two `GetTickCount` values; the
/// counter wraps after 49.7 days.
pub fn idle_from_ticks(now: u32, last_input: u32) -> Duration {
    Duration::from_millis(u64::from(now.wrapping_sub(last_input)))
}

/// Quotes one argument for a Windows command line when it needs it.
fn quote(arg: &str) -> String {
    if !arg.is_empty() && !arg.contains([' ', '\t', '"']) {
        return arg.to_string();
    }
    format!("\"{}\"", arg.replace('"', "\\\""))
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The watcher's arguments after the program path. `dashboard` is the
/// installed program the watcher opens when it runs from a copy.
pub fn watch_args(
    idle_minutes: u32,
    config: Option<&Path>,
    dashboard: Option<&Path>,
) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(c) = config {
        args.extend(["--config".to_string(), c.display().to_string()]);
    }
    args.extend([
        "screensaver".to_string(),
        "watch".to_string(),
        "--idle-minutes".to_string(),
        idle_minutes.to_string(),
    ]);
    if let Some(d) = dashboard {
        args.extend(["--dashboard-exe".to_string(), d.display().to_string()]);
    }
    args
}

/// Where `install` puts the copy the task runs:
/// `%LOCALAPPDATA%\telemetrix\screensaver\telemetrix-watch.exe`.
pub fn watch_copy_path(local_app_data: Option<PathBuf>) -> PathBuf {
    local_app_data
        .unwrap_or_else(std::env::temp_dir)
        .join("telemetrix")
        .join("screensaver")
        .join(WATCH_EXE)
}

/// Splits a command line made by [`quote`] back into its arguments.
fn split_args(line: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut started = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' if in_quotes && chars.peek() == Some(&'"') => {
                cur.push('"');
                chars.next();
            }
            '"' => {
                in_quotes = !in_quotes;
                started = true;
            }
            ' ' | '\t' if !in_quotes => {
                if started {
                    args.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            _ => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        args.push(cur);
    }
    args
}

/// What the installed task runs, read back from its `conhost.exe` arguments.
#[derive(Debug, PartialEq, Eq)]
pub struct TaskInfo {
    /// The program the task starts: the copy, or the installed exe for a
    /// task from before the copy existed.
    pub program: PathBuf,
    /// `--dashboard-exe`: the installed program the watcher opens.
    pub dashboard: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub idle_minutes: Option<u32>,
}

pub fn parse_task_args(line: &str) -> Option<TaskInfo> {
    let args = split_args(line);
    let mut it = args.iter();
    if it.next()? != "--headless" {
        return None;
    }
    let program = PathBuf::from(it.next()?);
    let mut info = TaskInfo {
        program,
        dashboard: None,
        config: None,
        idle_minutes: None,
    };
    while let Some(a) = it.next() {
        match a.as_str() {
            "--config" => info.config = it.next().map(PathBuf::from),
            "--dashboard-exe" => info.dashboard = it.next().map(PathBuf::from),
            "--idle-minutes" => info.idle_minutes = it.next().and_then(|n| n.parse().ok()),
            _ => {}
        }
    }
    Some(info)
}

#[derive(Debug, PartialEq, Eq)]
pub enum CopyState {
    Missing,
    UpToDate,
    /// The installed program changed after the copy was made.
    Older,
}

/// Compares the copy with the installed program. `fs::copy` keeps the
/// modified time, so a copy made from the current file has the same time.
pub fn copy_state(copy: &Path, installed: &Path) -> CopyState {
    let meta = |p: &Path| {
        std::fs::metadata(p)
            .ok()
            .map(|m| (m.modified().ok(), m.len()))
    };
    match (meta(copy), meta(installed)) {
        (None, _) => CopyState::Missing,
        (Some(_), None) => CopyState::UpToDate,
        (Some((ct, cl)), Some((it, il))) => {
            if cl != il || matches!((ct, it), (Some(c), Some(i)) if c < i) {
                CopyState::Older
            } else {
                CopyState::UpToDate
            }
        }
    }
}

/// The dashboard the watcher starts: Windows Terminal full screen with the
/// `telemetrix` profile when it exists (Windows Terminal falls back to the
/// default profile otherwise).
pub fn launch_args(exe: &Path, config: Option<&Path>) -> Vec<String> {
    let mut args = vec![
        "-F".to_string(),
        "-p".to_string(),
        "telemetrix".to_string(),
        exe.display().to_string(),
    ];
    args.extend(dashboard_args(config));
    args
}

fn dashboard_args(config: Option<&Path>) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(c) = config {
        args.extend(["--config".to_string(), c.display().to_string()]);
    }
    args.push("--screensaver".to_string());
    args
}

/// The Task Scheduler task: start the watcher at this user's logon, in a
/// console without a window (`conhost.exe --headless`), with no time limit.
pub fn task_xml(exe: &Path, watch: &[String], user: &str) -> String {
    let arguments: Vec<String> = std::iter::once("--headless".to_string())
        .chain(std::iter::once(quote(&exe.display().to_string())))
        .chain(watch.iter().map(|a| quote(a)))
        .collect();
    let user = xml_escape(user);
    let arguments = xml_escape(&arguments.join(" "));
    format!(
        r#"<?xml version="1.0" encoding="UTF-16"?>
<Task version="1.2" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo>
    <Description>Opens the telemetrix dashboard full screen after some minutes without input. Remove it with: telemetrix screensaver uninstall</Description>
  </RegistrationInfo>
  <Triggers>
    <LogonTrigger>
      <Enabled>true</Enabled>
      <UserId>{user}</UserId>
    </LogonTrigger>
  </Triggers>
  <Principals>
    <Principal id="Author">
      <UserId>{user}</UserId>
      <LogonType>InteractiveToken</LogonType>
      <RunLevel>LeastPrivilege</RunLevel>
    </Principal>
  </Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>
    <StopIfGoingOnBatteries>false</StopIfGoingOnBatteries>
    <AllowHardTerminate>true</AllowHardTerminate>
    <StartWhenAvailable>false</StartWhenAvailable>
    <RunOnlyIfNetworkAvailable>false</RunOnlyIfNetworkAvailable>
    <IdleSettings>
      <StopOnIdleEnd>false</StopOnIdleEnd>
      <RestartOnIdle>false</RestartOnIdle>
    </IdleSettings>
    <AllowStartOnDemand>true</AllowStartOnDemand>
    <Enabled>true</Enabled>
    <Hidden>false</Hidden>
    <RunOnlyIfIdle>false</RunOnlyIfIdle>
    <WakeToRun>false</WakeToRun>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <Priority>7</Priority>
  </Settings>
  <Actions Context="Author">
    <Exec>
      <Command>conhost.exe</Command>
      <Arguments>{arguments}</Arguments>
    </Exec>
  </Actions>
</Task>
"#
    )
}

/// The recipe for Linux idle daemons; nothing is written.
pub fn linux_recipe(exe: &Path, idle_minutes: u32) -> String {
    let exe = exe.display();
    let seconds = idle_minutes * 60;
    format!(
        "telemetrix does not install anything on Linux: your idle daemon starts it.\n\
         \n\
         Sway (Wayland), in ~/.config/sway/config:\n\
         \x20 exec swayidle -w timeout {seconds} 'foot {exe} --exit-on-any-key'\n\
         \n\
         X11, in your session start file (for example ~/.xprofile):\n\
         \x20 xautolock -time {idle_minutes} -locker \"xterm -fullscreen -e {exe} --exit-on-any-key\" &\n\
         \n\
         Use your own terminal instead of foot or xterm if you like.\n"
    )
}

fn exe_path() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from("telemetrix"))
}

fn config_arg(flags: &Flags) -> Option<PathBuf> {
    flags
        .config
        .as_ref()
        .map(|c| std::path::absolute(c).unwrap_or_else(|_| c.clone()))
}

pub fn run(cmd: ScreensaverCmd, flags: &Flags) -> ExitCode {
    let exe = exe_path();
    let config = config_arg(flags);
    match cmd {
        ScreensaverCmd::Install {
            idle_minutes,
            dry_run,
        } => {
            if cfg!(windows) {
                sys::install(&exe, config.as_deref(), idle_minutes, dry_run)
            } else {
                print!("{}", linux_recipe(&exe, idle_minutes));
                ExitCode::SUCCESS
            }
        }
        ScreensaverCmd::Uninstall { dry_run } => {
            if cfg!(windows) {
                sys::uninstall(dry_run)
            } else {
                println!(
                    "nothing to remove: on Linux, delete the swayidle or xautolock line you added"
                );
                ExitCode::SUCCESS
            }
        }
        ScreensaverCmd::Update { dry_run } => {
            if cfg!(windows) {
                sys::update(&exe, dry_run)
            } else {
                println!("nothing to update on Linux: your idle daemon starts telemetrix itself");
                ExitCode::SUCCESS
            }
        }
        ScreensaverCmd::Status => sys::status(),
        ScreensaverCmd::Watch {
            idle_minutes,
            dry_run,
            dashboard_exe,
        } => {
            let dashboard = dashboard_exe.unwrap_or_else(|| exe.clone());
            sys::watch(&dashboard, config.as_deref(), idle_minutes, dry_run, flags)
        }
    }
}

/// A human duration: "45 s", "12 min", "3 h 5 min".
fn human(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        _ => format!("{} h {} min", s / 3600, s % 3600 / 60),
    }
}

#[cfg(windows)]
mod sys {
    use std::io::Write;
    use std::os::windows::process::CommandExt;
    use std::path::{Path, PathBuf};
    use std::process::{Command, ExitCode, Stdio};
    use std::time::Duration;

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, WAIT_OBJECT_0,
    };
    use windows_sys::Win32::System::Console::{FreeConsole, GetConsoleWindow};
    use windows_sys::Win32::System::Power::{
        CallNtPowerInformation, ES_DISPLAY_REQUIRED, SystemExecutionState,
    };
    use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
    use windows_sys::Win32::System::SystemInformation::GetTickCount;
    use windows_sys::Win32::System::Threading::{
        CREATE_NEW_CONSOLE, CreateEventW, CreateMutexW, EVENT_MODIFY_STATE, GetCurrentProcess,
        OpenEventW, OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE, SetEvent, WaitForSingleObject,
    };
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{GetLastInputInfo, LASTINPUTINFO};

    use super::{
        DASHBOARD_MUTEX, Decision, IdleWatch, Observation, POLL, STOP_EVENT, TASK_NAME,
        WATCH_MUTEX, human, idle_from_ticks, launch_args, task_xml, watch_args,
    };
    use crate::cli::Flags;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// A kernel object handle, closed on drop.
    pub struct Handle(HANDLE);

    impl Drop for Handle {
        fn drop(&mut self) {
            // SAFETY: a handle this process opened and closes once.
            unsafe { CloseHandle(self.0) };
        }
    }

    pub fn idle_time() -> Option<Duration> {
        let mut info = LASTINPUTINFO {
            cbSize: size_of::<LASTINPUTINFO>() as u32,
            dwTime: 0,
        };
        // SAFETY: `info` is a LASTINPUTINFO with its size set.
        let ok = unsafe { GetLastInputInfo(&mut info) } != 0;
        // SAFETY: no arguments.
        ok.then(|| idle_from_ticks(unsafe { GetTickCount() }, info.dwTime))
    }

    pub fn display_required() -> Option<bool> {
        let mut state: u32 = 0;
        // SAFETY: the output buffer is one u32, as SystemExecutionState needs.
        let status = unsafe {
            CallNtPowerInformation(
                SystemExecutionState,
                std::ptr::null(),
                0,
                (&raw mut state).cast(),
                size_of::<u32>() as u32,
            )
        };
        (status == 0).then_some(state & ES_DISPLAY_REQUIRED != 0)
    }

    /// Creates the named mutex; `None` when another process already has it.
    pub fn claim(name: &str) -> Option<Handle> {
        let name = wide(name);
        // SAFETY: a nul-terminated name and no security attributes.
        let h = unsafe { CreateMutexW(std::ptr::null(), 0, name.as_ptr()) };
        if h.is_null() {
            return None;
        }
        let handle = Handle(h);
        // SAFETY: right after the call that set it.
        (unsafe { GetLastError() } != ERROR_ALREADY_EXISTS).then_some(handle)
    }

    fn mutex_exists(name: &str) -> bool {
        let name = wide(name);
        // SAFETY: a nul-terminated name.
        let h = unsafe { OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, 0, name.as_ptr()) };
        let found = !h.is_null();
        if found {
            drop(Handle(h));
        }
        found
    }

    fn stop_event(name: &str) -> Option<Handle> {
        let name = wide(name);
        // SAFETY: a nul-terminated name; auto-reset, not set at first.
        let h = unsafe { CreateEventW(std::ptr::null(), 0, 0, name.as_ptr()) };
        (!h.is_null()).then(|| Handle(h))
    }

    /// Sets the stop event of a running watcher; false when none runs.
    fn signal_stop(name: &str) -> bool {
        let name = wide(name);
        // SAFETY: a nul-terminated name.
        let h = unsafe { OpenEventW(EVENT_MODIFY_STATE, 0, name.as_ptr()) };
        if h.is_null() {
            return false;
        }
        let h = Handle(h);
        // SAFETY: an open event handle with EVENT_MODIFY_STATE.
        unsafe { SetEvent(h.0) != 0 }
    }

    fn user() -> String {
        let name = std::env::var("USERNAME").unwrap_or_default();
        match std::env::var("USERDOMAIN") {
            Ok(domain) if !domain.is_empty() => format!("{domain}\\{name}"),
            _ => name,
        }
    }

    fn schtasks(args: &[&str]) -> std::io::Result<std::process::Output> {
        Command::new("schtasks.exe")
            .args(args)
            .stdin(Stdio::null())
            .output()
    }

    fn show(args: &[&str]) -> String {
        std::iter::once("schtasks")
            .chain(args.iter().copied())
            .map(super::quote)
            .collect::<Vec<_>>()
            .join(" ")
    }

    /// The copy the task runs.
    fn copy_path() -> PathBuf {
        super::watch_copy_path(std::env::var_os("LOCALAPPDATA").map(PathBuf::from))
    }

    fn same_file(a: &Path, b: &Path) -> bool {
        match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
            (Ok(a), Ok(b)) => a == b,
            _ => a == b,
        }
    }

    /// Asks a running watcher to end and waits until it has; true when one ran.
    fn stop_watcher() -> bool {
        if !signal_stop(STOP_EVENT) {
            return false;
        }
        for _ in 0..100 {
            if !mutex_exists(WATCH_MUTEX) {
                break;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        true
    }

    /// Copies the program for the task. The old watcher may hold its file
    /// for a moment after it released the mutex, so a locked file is
    /// retried for a few seconds.
    fn copy_exe(from: &Path, to: &Path) -> Result<(), String> {
        if let Some(dir) = to.parent() {
            std::fs::create_dir_all(dir)
                .map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        }
        let mut last = None;
        for _ in 0..20 {
            match std::fs::copy(from, to) {
                Ok(_) => return Ok(()),
                Err(e) => last = Some(e),
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(format!(
            "cannot copy {} to {}: {}",
            from.display(),
            to.display(),
            last.map(|e| e.to_string()).unwrap_or_default()
        ))
    }

    fn run_task() -> bool {
        schtasks(&["/Run", "/TN", TASK_NAME]).is_ok_and(|o| o.status.success())
    }

    pub fn install(
        exe: &Path,
        config: Option<&Path>,
        idle_minutes: u32,
        dry_run: bool,
    ) -> ExitCode {
        let copy = copy_path();
        if same_file(exe, &copy) {
            eprintln!(
                "screensaver: this is the watcher's copy; run install from the installed telemetrix.exe"
            );
            return ExitCode::FAILURE;
        }
        let xml = task_xml(&copy, &watch_args(idle_minutes, config, Some(exe)), &user());
        let file = std::env::temp_dir().join("telemetrix-screensaver-task.xml");
        let file_text = file.display().to_string();
        let create = ["/Create", "/TN", TASK_NAME, "/XML", &file_text, "/F"];
        let start = ["/Run", "/TN", TASK_NAME];
        if dry_run {
            println!("--dry-run: nothing is changed. install would:");
            println!("1. ask a running watcher to end (event {STOP_EVENT}) and wait for it");
            println!(
                "2. copy {} to {}, so the watcher never locks the installed program",
                exe.display(),
                copy.display()
            );
            println!("3. write this task definition to {file_text}:\n");
            println!("{xml}");
            println!(
                "4. register it (replacing a task with the same name): {}",
                show(&create)
            );
            println!("5. delete {file_text}");
            println!(
                "6. start the watcher now instead of at the next logon: {}",
                show(&start)
            );
            return ExitCode::SUCCESS;
        }
        stop_watcher();
        if let Err(e) = copy_exe(exe, &copy) {
            eprintln!("screensaver: {e}");
            return ExitCode::FAILURE;
        }
        // schtasks reads the XML as UTF-16, the encoding the header names.
        let mut bytes = vec![0xFF, 0xFE];
        bytes.extend(xml.encode_utf16().flat_map(u16::to_le_bytes));
        if let Err(e) = std::fs::File::create(&file).and_then(|mut f| f.write_all(&bytes)) {
            eprintln!("screensaver: cannot write {file_text}: {e}");
            return ExitCode::FAILURE;
        }
        let created = schtasks(&create);
        let _ = std::fs::remove_file(&file);
        match created {
            Ok(out) if out.status.success() => {}
            Ok(out) => {
                eprintln!(
                    "screensaver: schtasks could not create the task: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                );
                return ExitCode::FAILURE;
            }
            Err(e) => {
                eprintln!("screensaver: cannot run schtasks: {e}");
                return ExitCode::FAILURE;
            }
        }
        let started = run_task();
        println!(
            "installed: after {idle_minutes} min without input the dashboard opens full screen.\n\
             The watcher starts at every logon{}. It runs from {}.\n\
             After `cargo install`, run: telemetrix screensaver update\n\
             Remove it with: telemetrix screensaver uninstall",
            if started {
                " and runs now"
            } else {
                " (it could not be started now)"
            },
            copy.display()
        );
        ExitCode::SUCCESS
    }

    /// `update`: copy the installed program to the watcher again and restart
    /// it. A task from before the copy existed is installed again with its
    /// own settings.
    pub fn update(exe: &Path, dry_run: bool) -> ExitCode {
        let Some(info) = installed_arguments().and_then(|a| super::parse_task_args(&a)) else {
            eprintln!(
                "screensaver: no task \"{TASK_NAME}\" is installed; use: telemetrix screensaver install"
            );
            return ExitCode::FAILURE;
        };
        let Some(dashboard) = info.dashboard else {
            println!(
                "the task runs {} itself (an older install), so it locks that file.\n\
                 It is installed again to run a copy, with the same settings:",
                info.program.display()
            );
            let idle = info.idle_minutes.unwrap_or(super::DEFAULT_IDLE_MINUTES);
            return install(exe, info.config.as_deref(), idle, dry_run);
        };
        if !same_file(exe, &dashboard) {
            println!(
                "note: the task opens {}, but this program is {}. \
                 To switch, run install from the program you want.",
                dashboard.display(),
                exe.display()
            );
        }
        let copy = info.program;
        if dry_run {
            println!("--dry-run: nothing is changed. update would:");
            println!("1. ask a running watcher to end (event {STOP_EVENT}) and wait for it");
            println!("2. copy {} to {}", dashboard.display(), copy.display());
            println!(
                "3. start the watcher again: {}",
                show(&["/Run", "/TN", TASK_NAME])
            );
            return ExitCode::SUCCESS;
        }
        let was_running = stop_watcher();
        if let Err(e) = copy_exe(&dashboard, &copy) {
            eprintln!("screensaver: {e}");
            if was_running {
                run_task();
            }
            return ExitCode::FAILURE;
        }
        let started = run_task();
        println!(
            "updated: {} is a fresh copy of {}; the watcher {}",
            copy.display(),
            dashboard.display(),
            if started {
                "runs again"
            } else {
                "could not be started now (it starts at the next logon)"
            }
        );
        ExitCode::SUCCESS
    }

    pub fn uninstall(dry_run: bool) -> ExitCode {
        let delete = ["/Delete", "/TN", TASK_NAME, "/F"];
        let copy = installed_arguments()
            .and_then(|a| super::parse_task_args(&a))
            .filter(|i| i.dashboard.is_some())
            .map(|i| i.program)
            .unwrap_or_else(copy_path);
        if dry_run {
            println!("--dry-run: nothing is changed. uninstall would:");
            println!("1. ask a running watcher to end (it waits on the event {STOP_EVENT})");
            println!("2. remove the task: {}", show(&delete));
            println!("3. delete the watcher's copy {}", copy.display());
            return ExitCode::SUCCESS;
        }
        let stopped = stop_watcher();
        let removed = match schtasks(&delete) {
            Ok(out) if out.status.success() => true,
            Ok(_) => false,
            Err(e) => {
                eprintln!("screensaver: cannot run schtasks: {e}");
                return ExitCode::FAILURE;
            }
        };
        let copy_gone = remove_copy(&copy);
        println!(
            "{}; {}; {}",
            if removed {
                "the task is removed"
            } else {
                "there was no task to remove"
            },
            if stopped {
                "the watcher was asked to end"
            } else {
                "no watcher was running"
            },
            match copy_gone {
                Ok(true) => "the watcher's copy is deleted".to_string(),
                Ok(false) => "there was no watcher copy".to_string(),
                Err(e) => format!("the watcher's copy could not be deleted: {e}"),
            }
        );
        ExitCode::SUCCESS
    }

    /// Deletes the copy (retrying while the old watcher still holds it) and
    /// its folder when that is empty; Ok(false) when there was none.
    fn remove_copy(copy: &Path) -> Result<bool, String> {
        if !copy.exists() {
            return Ok(false);
        }
        let mut last = None;
        for _ in 0..20 {
            match std::fs::remove_file(copy) {
                Ok(()) => {
                    if let Some(dir) = copy.parent() {
                        let _ = std::fs::remove_dir(dir);
                    }
                    return Ok(true);
                }
                Err(e) => last = Some(e),
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(last.map(|e| e.to_string()).unwrap_or_default())
    }

    /// The watcher arguments of the installed task, from `schtasks /Query /XML`.
    fn installed_arguments() -> Option<String> {
        let out = schtasks(&["/Query", "/TN", TASK_NAME, "/XML"]).ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let start = text.find("<Arguments>")? + "<Arguments>".len();
        let end = text[start..].find("</Arguments>")? + start;
        Some(
            text[start..end]
                .replace("&quot;", "\"")
                .replace("&lt;", "<")
                .replace("&gt;", ">")
                .replace("&amp;", "&"),
        )
    }

    pub fn status() -> ExitCode {
        let args = installed_arguments();
        let task = match &args {
            Some(args) => format!("installed (\"{TASK_NAME}\"): conhost.exe {args}"),
            None => "not installed".to_string(),
        };
        let yes_no = |b: bool| if b { "running" } else { "not running" };
        println!("task:       {task}");
        if let Some(info) = args.as_deref().and_then(super::parse_task_args) {
            println!("runs:       {}", info.program.display());
            match &info.dashboard {
                Some(dashboard) => {
                    println!("opens:      {}", dashboard.display());
                    let copy = match super::copy_state(&info.program, dashboard) {
                        super::CopyState::UpToDate => "up to date".to_string(),
                        super::CopyState::Older => format!(
                            "older than {}: run telemetrix screensaver update",
                            dashboard.display()
                        ),
                        super::CopyState::Missing => {
                            "missing: run telemetrix screensaver update".to_string()
                        }
                    };
                    println!("copy:       {copy}");
                }
                None => println!(
                    "copy:       none (an older install that locks the program; \
                     run telemetrix screensaver update)"
                ),
            }
        }
        println!("watcher:    {}", yes_no(mutex_exists(WATCH_MUTEX)));
        println!("dashboard:  {}", yes_no(mutex_exists(DASHBOARD_MUTEX)));
        match idle_time() {
            Some(d) => println!("idle now:   {}", human(d)),
            None => println!("idle now:   unknown"),
        }
        let display = match display_required() {
            Some(true) => "a program keeps the display on (the watcher waits)",
            Some(false) => "no program keeps the display on",
            None => "unknown",
        };
        println!("display:    {display}");
        ExitCode::SUCCESS
    }

    fn launch(exe: &Path, config: Option<&Path>) -> Result<&'static str, String> {
        let quiet = |c: &mut Command| {
            c.stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
        };
        let mut wt = Command::new("wt.exe");
        wt.args(launch_args(exe, config));
        quiet(&mut wt);
        match wt.spawn() {
            Ok(_) => Ok("Windows Terminal"),
            Err(wt_err) => {
                // No Windows Terminal: a new console window of its own.
                let mut direct = Command::new(exe);
                direct
                    .args(super::dashboard_args(config))
                    .creation_flags(CREATE_NEW_CONSOLE);
                quiet(&mut direct);
                direct
                    .spawn()
                    .map(|_| "a new console")
                    .map_err(|e| format!("wt.exe: {wt_err}; {}: {e}", exe.display()))
            }
        }
    }

    pub fn watch(
        exe: &Path,
        config: Option<&Path>,
        idle_minutes: u32,
        dry_run: bool,
        flags: &Flags,
    ) -> ExitCode {
        let mut log_file = flags.log.as_ref().and_then(|p| {
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(p)
                .ok()
        });
        let mut say = |text: &str| {
            let line = format!(
                "{} {text}",
                crate::format::utc_timestamp(std::time::SystemTime::now())
            );
            println!("{line}");
            if let Some(f) = log_file.as_mut() {
                let _ = writeln!(f, "{line}");
            }
        };
        // The logon task starts the watcher in `conhost.exe --headless`: a
        // console with no window. Leaving it ends that conhost, which would
        // otherwise hold about 10 MB for nothing. A watcher started by hand
        // in a terminal has a console window and keeps it.
        // SAFETY: plain console calls without arguments.
        if unsafe { GetConsoleWindow() }.is_null() {
            unsafe { FreeConsole() };
        }
        let Some(_single) = claim(WATCH_MUTEX) else {
            say("another screensaver watcher is already running; this one ends");
            return ExitCode::SUCCESS;
        };
        let Some(stop) = stop_event(STOP_EVENT) else {
            say("cannot create the stop event; the watcher ends");
            return ExitCode::FAILURE;
        };
        let limit = Duration::from_secs(u64::from(idle_minutes) * 60);
        let mut idle = IdleWatch::new(limit);
        say(&format!(
            "watching: the dashboard opens after {idle_minutes} min without input{}",
            if dry_run {
                " (--dry-run: it is never opened)"
            } else {
                ""
            }
        ));
        let mut polls: u32 = 0;
        loop {
            let obs = Observation {
                idle: idle_time().unwrap_or_default(),
                dashboard_running: mutex_exists(DASHBOARD_MUTEX),
                display_required: display_required().unwrap_or(false),
            };
            if idle.step(obs) == Decision::Launch {
                let line = std::iter::once("wt.exe".to_string())
                    .chain(launch_args(exe, config))
                    .map(|a| super::quote(&a))
                    .collect::<Vec<_>>()
                    .join(" ");
                if dry_run {
                    say(&format!("idle {}: would open {line}", human(obs.idle)));
                } else {
                    match launch(exe, config) {
                        Ok(how) => say(&format!("idle {}: opened in {how}", human(obs.idle))),
                        Err(e) => say(&format!("cannot open the dashboard: {e}")),
                    }
                }
            }
            polls += 1;
            if polls == 2 {
                // Start-up pages (argument parsing, the first log line) are
                // never used again; a poll touches only a few pages.
                // SAFETY: the pseudo handle of this process.
                unsafe { K32EmptyWorkingSet(GetCurrentProcess()) };
            }
            if stopped(&stop, POLL) {
                say("asked to end (screensaver uninstall)");
                return ExitCode::SUCCESS;
            }
        }
    }

    /// Waits up to `wait` for the stop event; true when it was set.
    fn stopped(event: &Handle, wait: Duration) -> bool {
        // SAFETY: an open event handle.
        unsafe { WaitForSingleObject(event.0, wait.as_millis() as u32) == WAIT_OBJECT_0 }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn named_objects_mark_stop_and_claim() {
            let tag = std::process::id();
            let stop_name = format!(r"Local	elemetrix-test-stop-{tag}");
            assert!(!signal_stop(&stop_name), "no watcher yet");
            let stop = stop_event(&stop_name).unwrap();
            assert!(!stopped(&stop, Duration::from_millis(10)));
            assert!(signal_stop(&stop_name));
            assert!(stopped(&stop, Duration::from_millis(10)));
            assert!(!stopped(&stop, Duration::ZERO), "auto-reset: seen once");

            let mutex_name = format!(r"Local	elemetrix-test-mutex-{tag}");
            assert!(!mutex_exists(&mutex_name));
            let first = claim(&mutex_name).expect("the first claim wins");
            assert!(mutex_exists(&mutex_name));
            assert!(claim(&mutex_name).is_none(), "a second claim fails");
            drop(first);
            assert!(!mutex_exists(&mutex_name), "released with the handle");
            assert!(idle_time().is_some() && display_required().is_some());
        }
    }
}

#[cfg(not(windows))]
mod sys {
    use std::path::Path;
    use std::process::ExitCode;

    use crate::cli::Flags;

    pub fn install(_: &Path, _: Option<&Path>, _: u32, _: bool) -> ExitCode {
        ExitCode::SUCCESS
    }

    pub fn uninstall(_: bool) -> ExitCode {
        ExitCode::SUCCESS
    }

    pub fn update(_: &Path, _: bool) -> ExitCode {
        ExitCode::SUCCESS
    }

    pub fn status() -> ExitCode {
        println!(
            "telemetrix installs no screensaver on Linux: use `telemetrix screensaver install` for the swayidle or xautolock line"
        );
        ExitCode::SUCCESS
    }

    pub fn watch(_: &Path, _: Option<&Path>, _: u32, _: bool, _: &Flags) -> ExitCode {
        eprintln!(
            "screensaver watch works on Windows only; on Linux use swayidle or xautolock (see screensaver install)"
        );
        ExitCode::FAILURE
    }
}

/// The dashboard's `--screensaver` mutex; keep it while the dashboard runs.
#[cfg(windows)]
pub fn mark_dashboard() -> Option<sys::Handle> {
    sys::claim(DASHBOARD_MUTEX)
}

#[cfg(not(windows))]
pub fn mark_dashboard() -> Option<()> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obs(idle_s: u64) -> Observation {
        Observation {
            idle: Duration::from_secs(idle_s),
            dashboard_running: false,
            display_required: false,
        }
    }

    #[test]
    fn one_launch_per_idle_period() {
        let mut w = IdleWatch::new(Duration::from_secs(600));
        assert_eq!(w.step(obs(0)), Decision::Wait);
        assert_eq!(w.step(obs(599)), Decision::Wait);
        assert_eq!(w.step(obs(600)), Decision::Launch);
        // Still idle (the dashboard closed without input, or failed): no repeat.
        assert_eq!(w.step(obs(605)), Decision::Wait);
        assert_eq!(w.step(obs(4000)), Decision::Wait);
        // Input re-arms it.
        assert_eq!(w.step(obs(3)), Decision::Wait);
        assert_eq!(w.step(obs(601)), Decision::Launch);
    }

    #[test]
    fn a_running_dashboard_or_a_video_holds_it_back() {
        let mut w = IdleWatch::new(Duration::from_secs(60));
        let running = Observation {
            dashboard_running: true,
            ..obs(120)
        };
        assert_eq!(w.step(running), Decision::Wait);
        let video = Observation {
            display_required: true,
            ..obs(130)
        };
        assert_eq!(w.step(video), Decision::Wait);
        // Both gone and still idle: now it opens.
        assert_eq!(w.step(obs(140)), Decision::Launch);
    }

    #[test]
    fn idle_time_survives_the_tick_counter_wrapping() {
        assert_eq!(idle_from_ticks(5_000, 2_000), Duration::from_secs(3));
        assert_eq!(
            idle_from_ticks(1_000, u32::MAX - 999),
            Duration::from_millis(2_000)
        );
    }

    #[test]
    fn task_xml_starts_the_watcher_at_logon_without_a_window() {
        let exe = Path::new(r"C:\Program Files\tx & co\telemetrix.exe");
        let args = watch_args(15, Some(Path::new(r"D:\cfg\t.toml")), None);
        let xml = task_xml(exe, &args, r"PC\someone");
        assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-16\"?>"));
        assert!(xml.contains(
            "<LogonTrigger>\n      <Enabled>true</Enabled>\n      <UserId>PC\\someone</UserId>"
        ));
        assert!(xml.contains("<LogonType>InteractiveToken</LogonType>"));
        assert!(xml.contains("<RunLevel>LeastPrivilege</RunLevel>"));
        assert!(xml.contains("<ExecutionTimeLimit>PT0S</ExecutionTimeLimit>"));
        assert!(xml.contains("<DisallowStartIfOnBatteries>false</DisallowStartIfOnBatteries>"));
        assert!(xml.contains("<Command>conhost.exe</Command>"));
        assert!(xml.contains(
            "<Arguments>--headless &quot;C:\\Program Files\\tx &amp; co\\telemetrix.exe&quot; \
             --config D:\\cfg\\t.toml screensaver watch --idle-minutes 15</Arguments>"
        ));
        assert!(!xml.contains("RunOnlyIfIdle>true"));
    }

    #[test]
    fn the_task_runs_the_copy_and_opens_the_installed_program() {
        let copy = watch_copy_path(Some(PathBuf::from(r"C:\Data Local")));
        assert_eq!(
            copy,
            Path::new(r"C:\Data Local")
                .join("telemetrix")
                .join("screensaver")
                .join("telemetrix-watch.exe")
        );
        let installed = Path::new(r"C:\Program Files\tx & co\telemetrix.exe");
        let args = watch_args(7, Some(Path::new(r"D:\my cfg\t.toml")), Some(installed));
        assert_eq!(
            &args[args.len() - 2..],
            [
                "--dashboard-exe",
                r"C:\Program Files\tx & co\telemetrix.exe"
            ]
        );
        let xml = task_xml(&copy, &args, r"PC\someone");
        let start = xml.find("<Arguments>").unwrap() + "<Arguments>".len();
        let end = xml.find("</Arguments>").unwrap();
        let line = xml[start..end]
            .replace("&quot;", "\"")
            .replace("&amp;", "&");
        assert_eq!(
            parse_task_args(&line).unwrap(),
            TaskInfo {
                program: copy,
                dashboard: Some(installed.to_path_buf()),
                config: Some(PathBuf::from(r"D:\my cfg\t.toml")),
                idle_minutes: Some(7),
            }
        );
    }

    #[test]
    fn an_older_task_without_a_copy_is_recognised() {
        let info = parse_task_args(
            r#"--headless C:\t\telemetrix.exe screensaver watch --idle-minutes 10"#,
        )
        .unwrap();
        assert_eq!(info.program, Path::new(r"C:\t\telemetrix.exe"));
        assert_eq!(info.dashboard, None);
        assert_eq!(info.config, None);
        assert_eq!(info.idle_minutes, Some(10));
        assert!(parse_task_args("/c something else").is_none());
        assert_eq!(
            split_args(r#"a "b c" "d \"e\"" """#),
            ["a", "b c", "d \"e\"", ""]
        );
    }

    #[test]
    fn copy_state_compares_time_and_size() {
        let dir = std::env::temp_dir().join(format!("telemetrix-copy-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let installed = dir.join("telemetrix.exe");
        let copy = dir.join("telemetrix-watch.exe");
        std::fs::write(&installed, b"version one").unwrap();
        assert_eq!(copy_state(&copy, &installed), CopyState::Missing);
        std::fs::copy(&installed, &copy).unwrap();
        assert_eq!(copy_state(&copy, &installed), CopyState::UpToDate);
        // A new build of the same size, written later.
        std::fs::write(&installed, b"version two").unwrap();
        let later = std::time::SystemTime::now() + Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&installed)
            .unwrap()
            .set_modified(later)
            .unwrap();
        assert_eq!(copy_state(&copy, &installed), CopyState::Older);
        std::fs::write(&installed, b"a longer version three").unwrap();
        std::fs::copy(&installed, &copy).unwrap();
        assert_eq!(copy_state(&copy, &installed), CopyState::UpToDate);
        std::fs::write(&installed, b"short").unwrap();
        assert_eq!(copy_state(&copy, &installed), CopyState::Older);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_dashboard_opens_full_screen_in_screensaver_mode() {
        let args = launch_args(Path::new(r"C:\t\telemetrix.exe"), None);
        assert_eq!(
            args,
            [
                "-F",
                "-p",
                "telemetrix",
                r"C:\t\telemetrix.exe",
                "--screensaver"
            ]
        );
        let with_config = launch_args(Path::new("t.exe"), Some(Path::new("c.toml")));
        assert_eq!(&with_config[4..], ["--config", "c.toml", "--screensaver"]);
    }

    #[test]
    fn linux_prints_a_recipe() {
        let text = linux_recipe(Path::new("/usr/local/bin/telemetrix"), 5);
        assert!(text.contains(
            "exec swayidle -w timeout 300 'foot /usr/local/bin/telemetrix --exit-on-any-key'"
        ));
        assert!(text.contains("xautolock -time 5 -locker"));
    }

    #[test]
    fn durations_read_well() {
        assert_eq!(human(Duration::from_secs(45)), "45 s");
        assert_eq!(human(Duration::from_secs(720)), "12 min");
        assert_eq!(human(Duration::from_secs(3 * 3600 + 300)), "3 h 5 min");
        assert_eq!(quote("a b"), "\"a b\"");
        assert_eq!(quote("plain"), "plain");
    }
}
