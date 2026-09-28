//! Updates from the GitHub Releases of `OliverD25/telemetrix` (decision 53).
//!
//! Only that repository is asked, only over https, and a download counts
//! only when its SHA-256 matches the release's own `SHA256SUMS` asset. The
//! new program replaces the installed one by renames in the same folder:
//! on Windows the running file moves aside to `<exe>.old` (a running exe
//! can be renamed, not overwritten) and is deleted at the next start; on
//! Linux one rename swaps the files at once.

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ring::digest;
use serde::{Deserialize, Serialize};
use ureq::Agent;

const API_URL: &str = "https://api.github.com/repos/OliverD25/telemetrix/releases/latest";
const DOWNLOAD_BASE: &str = "https://github.com/OliverD25/telemetrix/releases/download";
const USER_AGENT: &str = concat!("telemetrix/", env!("CARGO_PKG_VERSION"), " (updater)");
pub const SUMS_NAME: &str = "SHA256SUMS";
/// Far above the real size (about 4 MB), so a wrong answer cannot fill the disk.
const MAX_EXE_BYTES: u64 = 64 << 20;
const MAX_SUMS_BYTES: u64 = 64 << 10;
const MAX_API_BYTES: u64 = 1 << 20;
const API_TIMEOUT: Duration = Duration::from_secs(30);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);

/// A version like `0.3.0` or `0.3.0-dev`; a leading `v` and `+build` are ignored.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.split('+').next()?;
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) if !pre.is_empty() => (core, Some(pre.to_string())),
            Some(_) => return None,
            None => (text, None),
        };
        let mut parts = core.split('.').map(|p| {
            (!p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
                .then(|| p.parse::<u64>().ok())
                .flatten()
        });
        let (major, minor, patch) = (parts.next()??, parts.next()??, parts.next()??);
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            major,
            minor,
            patch,
            pre,
        })
    }

    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).expect("the package version is semver")
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(pre) = &self.pre {
            write!(f, "-{pre}")?;
        }
        Ok(())
    }
}

impl Ord for Version {
    /// Semver precedence: a pre-release comes before its release, and
    /// pre-release parts compare as numbers when both are numbers.
    fn cmp(&self, other: &Self) -> Ordering {
        let core =
            (self.major, self.minor, self.patch).cmp(&(other.major, other.minor, other.patch));
        if core != Ordering::Equal {
            return core;
        }
        match (&self.pre, &other.pre) {
            (None, None) => Ordering::Equal,
            (None, Some(_)) => Ordering::Greater,
            (Some(_), None) => Ordering::Less,
            (Some(a), Some(b)) => {
                let mut a = a.split('.');
                let mut b = b.split('.');
                loop {
                    match (a.next(), b.next()) {
                        (None, None) => return Ordering::Equal,
                        (None, Some(_)) => return Ordering::Less,
                        (Some(_), None) => return Ordering::Greater,
                        (Some(x), Some(y)) => {
                            let o = match (x.parse::<u64>(), y.parse::<u64>()) {
                                (Ok(x), Ok(y)) => x.cmp(&y),
                                (Ok(_), Err(_)) => Ordering::Less,
                                (Err(_), Ok(_)) => Ordering::Greater,
                                (Err(_), Err(_)) => x.cmp(y),
                            };
                            if o != Ordering::Equal {
                                return o;
                            }
                        }
                    }
                }
            }
        }
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

/// The release build name of this platform; `None` where no build is published.
pub fn platform() -> Option<&'static str> {
    platform_of(std::env::consts::OS, std::env::consts::ARCH)
}

pub fn platform_of(os: &str, arch: &str) -> Option<&'static str> {
    match (os, arch) {
        ("windows", "x86_64") => Some("windows-x86_64"),
        ("linux", "x86_64") => Some("linux-x86_64"),
        _ => None,
    }
}

/// The bare program of a release: `telemetrix-v0.3.0-windows-x86_64.exe`.
pub fn asset_name(tag: &str, platform: &str) -> String {
    let ext = if platform.starts_with("windows") {
        ".exe"
    } else {
        ""
    };
    format!("telemetrix-{tag}-{platform}{ext}")
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub tag: String,
    pub version: Version,
    pub assets: Vec<String>,
}

#[derive(Deserialize)]
struct ApiRelease {
    tag_name: String,
    #[serde(default)]
    assets: Vec<ApiAsset>,
}

#[derive(Deserialize)]
struct ApiAsset {
    name: String,
}

/// Tags become part of a download URL, so only plain characters pass.
fn safe_tag(tag: &str) -> bool {
    !tag.is_empty()
        && tag.len() <= 64
        && tag
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
}

pub fn parse_release(json: &str) -> Result<Release, String> {
    let api: ApiRelease =
        serde_json::from_str(json).map_err(|e| format!("GitHub sent an unreadable answer: {e}"))?;
    if !safe_tag(&api.tag_name) {
        return Err(format!(
            "the release tag {:?} is not a plain tag",
            api.tag_name
        ));
    }
    let version = Version::parse(&api.tag_name)
        .ok_or_else(|| format!("the release tag {} is not a version", api.tag_name))?;
    Ok(Release {
        tag: api.tag_name,
        version,
        assets: api.assets.into_iter().map(|a| a.name).collect(),
    })
}

/// This platform's program in the release, and a check that `SHA256SUMS` is there.
pub fn pick_asset(release: &Release, platform: &str) -> Result<String, String> {
    let name = asset_name(&release.tag, platform);
    if !release.assets.contains(&name) {
        return Err(format!("release {} has no {name}", release.tag));
    }
    if !release.assets.iter().any(|a| a == SUMS_NAME) {
        return Err(format!(
            "release {} has no {SUMS_NAME}, so a download cannot be checked",
            release.tag
        ));
    }
    Ok(name)
}

/// `sha256sum` lines: `<64 hex>  <name>` (or `*<name>` for binary mode).
pub fn parse_sums(text: &str) -> BTreeMap<String, [u8; 32]> {
    text.lines()
        .filter_map(|line| {
            let (hash, name) = line.trim_end().split_once(char::is_whitespace)?;
            let name = name.trim_start();
            let name = name.strip_prefix('*').unwrap_or(name);
            Some((name.to_string(), from_hex(hash)?))
        })
        .collect()
}

fn from_hex(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(out)
}

pub fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn sha256_file(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = std::fs::File::open(path)?;
    let mut ctx = digest::Context::new(&digest::SHA256);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(ctx.finish().as_ref());
    Ok(out)
}

/// Where releases come from. Only tests build one with other addresses.
pub struct Source {
    pub api_url: String,
    pub download_base: String,
    pub https_only: bool,
}

impl Source {
    pub fn github() -> Self {
        Self {
            api_url: API_URL.into(),
            download_base: DOWNLOAD_BASE.into(),
            https_only: true,
        }
    }

    fn agent(&self) -> Agent {
        Agent::config_builder()
            .https_only(self.https_only)
            .http_status_as_error(false)
            .user_agent(USER_AGENT)
            .timeout_connect(Some(Duration::from_secs(15)))
            .build()
            .into()
    }

    fn url(&self, tag: &str, name: &str) -> String {
        format!("{}/{tag}/{name}", self.download_base)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum CheckError {
    /// GitHub answered 403 or 429: stop and ask again much later.
    RateLimited(String),
    Failed(String),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RateLimited(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

/// The latest release, from the GitHub API.
pub fn latest(source: &Source) -> Result<Release, CheckError> {
    let mut response = source
        .agent()
        .get(&source.api_url)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .config()
        .timeout_global(Some(API_TIMEOUT))
        .build()
        .call()
        .map_err(|e| CheckError::Failed(format!("cannot reach GitHub: {e}")))?;
    let status = response.status().as_u16();
    if status == 403 || status == 429 {
        let reset = response
            .headers()
            .get("x-ratelimit-reset")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok())
            .map(|s| {
                let at = SystemTime::UNIX_EPOCH + Duration::from_secs(s);
                format!(" until {} UTC", crate::format::utc_hms(at))
            })
            .unwrap_or_default();
        return Err(CheckError::RateLimited(format!(
            "GitHub refused the request (HTTP {status}, rate limit{reset})"
        )));
    }
    if status == 404 {
        return Err(CheckError::Failed("no release is published yet".into()));
    }
    if status != 200 {
        return Err(CheckError::Failed(format!("GitHub answered HTTP {status}")));
    }
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_API_BYTES)
        .read_to_string()
        .map_err(|e| CheckError::Failed(format!("cannot read GitHub's answer: {e}")))?;
    parse_release(&body).map_err(CheckError::Failed)
}

fn get(
    source: &Source,
    url: &str,
    timeout: Duration,
) -> Result<ureq::http::Response<ureq::Body>, String> {
    let response = source
        .agent()
        .get(url)
        .config()
        .timeout_global(Some(timeout))
        .build()
        .call()
        .map_err(|e| format!("cannot download {url}: {e}"))?;
    let status = response.status().as_u16();
    if status != 200 {
        return Err(format!("{url} answered HTTP {status}"));
    }
    Ok(response)
}

/// A verified download next to the program, not installed yet.
#[derive(Debug)]
pub struct Downloaded {
    pub path: PathBuf,
    pub version: Version,
    pub sha256: [u8; 32],
    pub bytes: u64,
}

/// `telemetrix.exe` → `telemetrix.exe.<suffix>`, in the same folder.
fn beside(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.file_name().map(OsString::from).unwrap_or_default();
    name.push(format!(".{suffix}"));
    exe.with_file_name(name)
}

pub fn old_path(exe: &Path) -> PathBuf {
    beside(exe, "old")
}

pub fn ready_path(exe: &Path) -> PathBuf {
    beside(exe, "ready")
}

fn ready_info_path(exe: &Path) -> PathBuf {
    beside(exe, "ready.json")
}

/// Downloads `name` of `release` to `<exe>.download`, hashing it on the way.
/// A checksum that does not match `SHA256SUMS` deletes the file and fails,
/// so nothing else changes.
pub fn download(
    source: &Source,
    release: &Release,
    name: &str,
    exe: &Path,
) -> Result<Downloaded, String> {
    let mut sums_response = get(source, &source.url(&release.tag, SUMS_NAME), API_TIMEOUT)?;
    let sums_text = sums_response
        .body_mut()
        .with_config()
        .limit(MAX_SUMS_BYTES)
        .read_to_string()
        .map_err(|e| format!("cannot read {SUMS_NAME}: {e}"))?;
    let expected = *parse_sums(&sums_text)
        .get(name)
        .ok_or_else(|| format!("{SUMS_NAME} of {} has no line for {name}", release.tag))?;
    let path = beside(exe, "download");
    let result = save_hashed(source, &source.url(&release.tag, name), &path);
    let (sha256, bytes) = match result {
        Ok(got) => got,
        Err(e) => {
            let _ = std::fs::remove_file(&path);
            return Err(e);
        }
    };
    if sha256 != expected {
        let _ = std::fs::remove_file(&path);
        return Err(format!(
            "checksum mismatch for {name}: {SUMS_NAME} says {}, the download is {}; \
             it was deleted and nothing was changed",
            to_hex(&expected),
            to_hex(&sha256)
        ));
    }
    Ok(Downloaded {
        path,
        version: release.version.clone(),
        sha256,
        bytes,
    })
}

fn save_hashed(source: &Source, url: &str, path: &Path) -> Result<([u8; 32], u64), String> {
    let mut response = get(source, url, DOWNLOAD_TIMEOUT)?;
    let mut reader = response
        .body_mut()
        .with_config()
        .limit(MAX_EXE_BYTES)
        .reader();
    let mut file =
        std::fs::File::create(path).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let mut ctx = digest::Context::new(&digest::SHA256);
    let mut buf = vec![0u8; 64 * 1024];
    let mut bytes = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("the download broke off: {e}"))?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
        file.write_all(&buf[..n])
            .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
        bytes += n as u64;
    }
    file.sync_all()
        .map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    let mut out = [0u8; 32];
    out.copy_from_slice(ctx.finish().as_ref());
    Ok((out, bytes))
}

/// Puts `new` in the place of `exe`. With `keep_old` (Windows) the current
/// file first moves aside to `<exe>.old`, because a running program can be
/// renamed but not replaced; if the second rename fails the old file goes
/// back. Without it one rename replaces the file at once (Linux).
pub fn replace_with(exe: &Path, new: &Path, keep_old: bool) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(new, std::fs::Permissions::from_mode(0o755))?;
    }
    if !keep_old || !exe.exists() {
        return std::fs::rename(new, exe);
    }
    let mut old = old_path(exe);
    if old.exists() && std::fs::remove_file(&old).is_err() {
        // An older version still runs from it; this name is cleaned up later.
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        old = beside(exe, &format!("old.{stamp}"));
    }
    std::fs::rename(exe, &old)?;
    if let Err(e) = std::fs::rename(new, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(e);
    }
    Ok(())
}

pub fn replace_exe(exe: &Path, new: &Path) -> io::Result<()> {
    replace_with(exe, new, cfg!(windows))
}

/// Deletes `<exe>.old` files left by an update; one that a running older
/// version still uses stays until a later start.
pub fn cleanup_old(exe: &Path) {
    let (Some(dir), Some(name)) = (exe.parent(), exe.file_name()) else {
        return;
    };
    let prefix = format!("{}.old", name.to_string_lossy());
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file = entry.file_name();
        let file = file.to_string_lossy();
        if file == prefix || file.starts_with(&format!("{prefix}.")) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// A verified download that waits for the dashboard to close.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadyInfo {
    pub version: String,
    pub sha256: String,
}

/// Keeps a verified download as `<exe>.ready` with its version and checksum.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn save_ready(exe: &Path, d: &Downloaded) -> io::Result<()> {
    std::fs::rename(&d.path, ready_path(exe))?;
    let info = ReadyInfo {
        version: d.version.to_string(),
        sha256: to_hex(&d.sha256),
    };
    let text = serde_json::to_string(&info).map_err(io::Error::other)?;
    std::fs::write(ready_info_path(exe), text)
}

pub fn ready(exe: &Path) -> Option<ReadyInfo> {
    let text = std::fs::read_to_string(ready_info_path(exe)).ok()?;
    let info: ReadyInfo = serde_json::from_str(&text).ok()?;
    ready_path(exe).exists().then_some(info)
}

pub fn discard_ready(exe: &Path) {
    let _ = std::fs::remove_file(ready_path(exe));
    let _ = std::fs::remove_file(ready_info_path(exe));
}

/// Installs `<exe>.ready` after checking its checksum again. A ready file
/// that is not newer than `current`, or does not match, is deleted.
pub fn install_ready(exe: &Path, current: &Version) -> Result<Version, String> {
    let info = ready(exe).ok_or("no update is ready")?;
    let version = Version::parse(&info.version).filter(|v| v > current);
    let Some(version) = version else {
        discard_ready(exe);
        return Err(format!(
            "the ready version {} is not newer; it was removed",
            info.version
        ));
    };
    let path = ready_path(exe);
    let sha = sha256_file(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if to_hex(&sha) != info.sha256 {
        discard_ready(exe);
        return Err("the ready file changed after it was checked; it was removed".into());
    }
    replace_exe(exe, &path).map_err(|e| format!("cannot replace {}: {e}", exe.display()))?;
    let _ = std::fs::remove_file(ready_info_path(exe));
    Ok(version)
}

/// What the dashboard shows in its status bar.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pending {
    /// Downloaded and verified; `u` installs it and restarts.
    Ready(Version),
    /// The program on disk is newer than the one running; `u` restarts.
    Installed(Version),
}

/// Size and time of a file, to notice that it was replaced.
pub fn stamp(path: &Path) -> Option<(u64, SystemTime)> {
    let m = std::fs::metadata(path).ok()?;
    Some((m.len(), m.modified().ok()?))
}

/// The version of the program at `exe`, from its `--version`.
pub fn version_of(exe: &Path) -> Option<Version> {
    let mut cmd = std::process::Command::new(exe);
    cmd.arg("--version")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    let out = cmd.output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    Version::parse(text.trim().strip_prefix("telemetrix ")?)
}

/// A newer program on disk or a ready download. `started` is the program
/// file's stamp when this dashboard started; `--version` runs only after
/// the file changed.
pub fn pending(exe: &Path, started: Option<(u64, SystemTime)>, own: &Version) -> Option<Pending> {
    if stamp(exe) != started
        && let Some(v) = version_of(exe).filter(|v| v > own)
    {
        return Some(Pending::Installed(v));
    }
    let info = ready(exe)?;
    Version::parse(&info.version)
        .filter(|v| v > own)
        .map(Pending::Ready)
}

/// Starts `exe` with this process's arguments in the same console, after
/// the dashboard gave the terminal back.
///
/// Linux replaces this process (`exec`): same process id, nothing waits.
/// Windows has no `exec`. There the shell (PowerShell in Windows Terminal)
/// waits for this process, so this process must not end first: the shell
/// would print its prompt and read keys while the new dashboard draws. So
/// it starts the new program with the same console, gives back its own
/// memory pages and waits, then ends with the new program's exit code.
pub fn restart(exe: &Path) -> std::process::ExitCode {
    use std::process::{Command, ExitCode};
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let e = Command::new(exe).args(&args).exec();
        eprintln!("telemetrix: cannot start {}: {e}", exe.display());
        ExitCode::FAILURE
    }
    #[cfg(windows)]
    {
        let mut child = match Command::new(exe).args(&args).spawn() {
            Ok(c) => c,
            Err(e) => {
                eprintln!("telemetrix: cannot start {}: {e}", exe.display());
                return ExitCode::FAILURE;
            }
        };
        // SAFETY: the pseudo handle of this process.
        unsafe {
            use windows_sys::Win32::System::ProcessStatus::K32EmptyWorkingSet;
            use windows_sys::Win32::System::Threading::GetCurrentProcess;
            K32EmptyWorkingSet(GetCurrentProcess());
        }
        match child.wait() {
            Ok(status) => ExitCode::from(status.code().map_or(1, |c| c.clamp(0, 255) as u8)),
            Err(_) => ExitCode::FAILURE,
        }
    }
}

/// What the screensaver watcher does at one look.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchAction {
    Wait,
    /// A verified download waits and no dashboard runs: install it now.
    InstallReady,
    /// Ask GitHub; `install` = no dashboard runs, so install at once.
    Check {
        install: bool,
    },
}

#[cfg_attr(not(windows), allow(dead_code))]
pub struct WatchInput {
    pub now: u64,
    pub last_check: Option<u64>,
    pub auto: bool,
    pub interval_h: u64,
    pub dashboard_running: bool,
    pub ready: bool,
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn watch_action(i: &WatchInput) -> WatchAction {
    if !i.auto {
        return WatchAction::Wait;
    }
    if i.ready {
        return if i.dashboard_running {
            WatchAction::Wait
        } else {
            WatchAction::InstallReady
        };
    }
    let due = i
        .last_check
        .is_none_or(|t| i.now >= t.saturating_add(i.interval_h * 3600) || i.now < t);
    if due {
        WatchAction::Check {
            install: !i.dashboard_running,
        }
    } else {
        WatchAction::Wait
    }
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// The watcher's record of its last check: unix seconds in a text file.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn read_last_check(path: &Path) -> Option<u64> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

#[cfg_attr(not(windows), allow(dead_code))]
pub fn write_last_check(path: &Path, when: u64) {
    let _ = std::fs::write(path, when.to_string());
}

/// What one watcher look did.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, PartialEq, Eq)]
pub enum WatchOutcome {
    Nothing,
    /// The installed program was replaced: refresh the watcher and end.
    Installed(Version),
}

/// One look of the screensaver watcher at updates: install a ready download
/// when no dashboard runs, or ask GitHub when the interval has passed.
/// `dashboard_running` is asked again right before a replace.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn watch_step(
    source: &Source,
    exe: &Path,
    input: &WatchInput,
    last_check_file: &Path,
    dashboard_running: &dyn Fn() -> bool,
    say: &mut dyn FnMut(&str),
) -> WatchOutcome {
    let own = Version::current();
    match watch_action(input) {
        WatchAction::Wait => WatchOutcome::Nothing,
        WatchAction::InstallReady => match install_ready(exe, &own) {
            Ok(v) => {
                say(&format!(
                    "update: installed the ready telemetrix {v} into {}",
                    exe.display()
                ));
                WatchOutcome::Installed(v)
            }
            Err(e) => {
                say(&format!("update: {e}"));
                WatchOutcome::Nothing
            }
        },
        WatchAction::Check { .. } => {
            let Some(platform) = platform() else {
                write_last_check(last_check_file, input.now);
                return WatchOutcome::Nothing;
            };
            say("update: asking GitHub for the latest release");
            let release = match latest(source) {
                Ok(r) => r,
                Err(CheckError::RateLimited(m)) => {
                    say(&format!("update: {m}; next try after the check interval"));
                    write_last_check(last_check_file, input.now);
                    return WatchOutcome::Nothing;
                }
                Err(CheckError::Failed(m)) => {
                    say(&format!("update: {m}; trying again in an hour"));
                    return WatchOutcome::Nothing;
                }
            };
            write_last_check(last_check_file, input.now);
            if release.version <= own {
                say(&format!(
                    "update: {} is the latest release; nothing to do",
                    release.tag
                ));
                return WatchOutcome::Nothing;
            }
            let name = match pick_asset(&release, platform) {
                Ok(n) => n,
                Err(e) => {
                    say(&format!("update: {e}"));
                    return WatchOutcome::Nothing;
                }
            };
            say(&format!("update: downloading {name}"));
            let d = match download(source, &release, &name, exe) {
                Ok(d) => d,
                Err(e) => {
                    say(&format!("update: {e}"));
                    return WatchOutcome::Nothing;
                }
            };
            say(&format!(
                "update: {} bytes, SHA-256 {} matches {SUMS_NAME}",
                d.bytes,
                to_hex(&d.sha256)
            ));
            if dashboard_running() {
                return match save_ready(exe, &d) {
                    Ok(()) => {
                        say(
                            "update: a dashboard is open; the update is ready and installs when it closes",
                        );
                        WatchOutcome::Nothing
                    }
                    Err(e) => {
                        let _ = std::fs::remove_file(&d.path);
                        say(&format!("update: cannot keep the download: {e}"));
                        WatchOutcome::Nothing
                    }
                };
            }
            match replace_exe(exe, &d.path) {
                Ok(()) => {
                    say(&format!(
                        "update: installed telemetrix {} into {}",
                        d.version,
                        exe.display()
                    ));
                    WatchOutcome::Installed(d.version)
                }
                Err(e) => {
                    let _ = std::fs::remove_file(&d.path);
                    say(&format!(
                        "update: cannot replace {}: {e}; nothing was changed",
                        exe.display()
                    ));
                    WatchOutcome::Nothing
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> Version {
        Version::parse(s).unwrap()
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("tx-update-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn versions_compare_as_semver() {
        assert_eq!(v("v0.2.0"), v("0.2.0"));
        assert!(v("0.3.0") > v("0.2.0"));
        assert!(v("0.10.0") > v("0.9.9"));
        assert!(v("1.0.0") > v("0.99.99"));
        assert!(v("0.3.0-dev") < v("0.3.0"));
        assert!(v("0.3.0-dev") > v("0.2.0"));
        assert!(v("0.3.0-rc.2") < v("0.3.0-rc.10"));
        assert!(v("0.3.0-alpha") < v("0.3.0-beta"));
        assert!(v("0.3.0-1") < v("0.3.0-alpha"));
        assert_eq!(v("0.3.0+build.7"), v("0.3.0"));
        assert_eq!(v("0.3.0-dev").to_string(), "0.3.0-dev");
        for bad in ["", "0.3", "0.3.0.1", "x.1.2", "0.3.0-", "0.-1.0", "0.3.a"] {
            assert_eq!(Version::parse(bad), None, "{bad:?}");
        }
        assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
    }

    #[test]
    fn assets_are_picked_per_platform() {
        assert_eq!(platform_of("windows", "x86_64"), Some("windows-x86_64"));
        assert_eq!(platform_of("linux", "x86_64"), Some("linux-x86_64"));
        assert_eq!(platform_of("linux", "aarch64"), None);
        assert_eq!(platform_of("macos", "x86_64"), None);
        let json = r#"{"tag_name":"v0.3.0","draft":false,"assets":[
            {"name":"telemetrix-v0.3.0-windows-x86_64.zip","browser_download_url":"x"},
            {"name":"telemetrix-v0.3.0-windows-x86_64.exe"},
            {"name":"telemetrix-v0.3.0-linux-x86_64.tar.gz"},
            {"name":"telemetrix-v0.3.0-linux-x86_64"},
            {"name":"SHA256SUMS"}]}"#;
        let r = parse_release(json).unwrap();
        assert_eq!(r.version, v("0.3.0"));
        assert_eq!(
            pick_asset(&r, "windows-x86_64").unwrap(),
            "telemetrix-v0.3.0-windows-x86_64.exe"
        );
        assert_eq!(
            pick_asset(&r, "linux-x86_64").unwrap(),
            "telemetrix-v0.3.0-linux-x86_64"
        );
        let old = parse_release(
            r#"{"tag_name":"v0.2.0","assets":[{"name":"telemetrix-v0.2.0-windows-x86_64.zip"}]}"#,
        )
        .unwrap();
        let e = pick_asset(&old, "windows-x86_64").unwrap_err();
        assert!(
            e.contains("has no telemetrix-v0.2.0-windows-x86_64.exe"),
            "{e}"
        );
        let no_sums = parse_release(
            r#"{"tag_name":"v0.3.0","assets":[{"name":"telemetrix-v0.3.0-linux-x86_64"}]}"#,
        )
        .unwrap();
        assert!(
            pick_asset(&no_sums, "linux-x86_64")
                .unwrap_err()
                .contains("SHA256SUMS")
        );
        assert!(
            parse_release(r#"{"tag_name":"v0.3.0/../x"}"#).is_err(),
            "no path parts"
        );
        assert!(parse_release(r#"{"tag_name":"latest"}"#).is_err());
        assert!(parse_release("not json").is_err());
    }

    #[test]
    fn sums_parse_like_sha256sum_writes_them() {
        let a = "a".repeat(64);
        let b = "0123456789abcdef".repeat(4);
        let text = format!(
            "{a}  telemetrix-v0.3.0-linux-x86_64\n{b} *telemetrix-v0.3.0-windows-x86_64.exe\r\n\
             short  broken\n\n{a}\n"
        );
        let sums = parse_sums(&text);
        assert_eq!(sums.len(), 2, "{sums:?}");
        assert_eq!(sums["telemetrix-v0.3.0-linux-x86_64"], [0xaa; 32]);
        assert_eq!(to_hex(&sums["telemetrix-v0.3.0-windows-x86_64.exe"]), b);
        // The empty input of sha256sum.
        let dir = temp_dir("hash");
        let f = dir.join("empty");
        std::fs::write(&f, "").unwrap();
        assert_eq!(
            to_hex(&sha256_file(&f).unwrap()),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn replace_keeps_the_old_file_aside_and_cleans_it_later() {
        let dir = temp_dir("replace");
        let exe = dir.join("telemetrix.exe");
        let new = dir.join("telemetrix.exe.download");
        std::fs::write(&exe, "old").unwrap();
        std::fs::write(&new, "new").unwrap();
        replace_with(&exe, &new, true).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(old_path(&exe)).unwrap(), "old");
        assert!(!new.exists());
        // A second update while the first .old is still there.
        std::fs::write(&new, "newer").unwrap();
        replace_with(&exe, &new, true).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "newer");
        std::fs::write(dir.join("telemetrix.exe.old.123"), "stale").unwrap();
        std::fs::write(dir.join("telemetrix.exe.older-notes"), "keep").unwrap();
        cleanup_old(&exe);
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(left, ["telemetrix.exe", "telemetrix.exe.older-notes"]);
        // Without keep_old (Linux) one rename replaces the file.
        std::fs::write(&new, "linux").unwrap();
        replace_with(&exe, &new, false).unwrap();
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "linux");
        assert!(!old_path(&exe).exists());
        // A failed move of the new file puts the old one back.
        let missing = dir.join("nothing-here");
        assert!(replace_with(&exe, &missing, true).is_err());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "linux");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ready_files_are_checked_again_before_install() {
        let dir = temp_dir("ready");
        let exe = dir.join("telemetrix.exe");
        std::fs::write(&exe, "running").unwrap();
        let make = |content: &str| {
            let path = dir.join("telemetrix.exe.download");
            std::fs::write(&path, content).unwrap();
            Downloaded {
                sha256: sha256_file(&path).unwrap(),
                path,
                version: v("9.0.0"),
                bytes: content.len() as u64,
            }
        };
        let own = v("0.3.0");
        assert!(install_ready(&exe, &own).is_err(), "nothing ready");
        save_ready(&exe, &make("v9")).unwrap();
        assert_eq!(ready(&exe).unwrap().version, "9.0.0");
        assert_eq!(
            pending(&exe, stamp(&exe), &own),
            Some(Pending::Ready(v("9.0.0")))
        );
        assert_eq!(pending(&exe, stamp(&exe), &v("9.0.0")), None, "not newer");
        // Changed after the check: removed, the program stays.
        save_ready(&exe, &make("v9")).unwrap();
        std::fs::write(ready_path(&exe), "tampered").unwrap();
        let e = install_ready(&exe, &own).unwrap_err();
        assert!(e.contains("changed"), "{e}");
        assert!(ready(&exe).is_none() && !ready_path(&exe).exists());
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "running");
        // Not newer than the running one: removed.
        save_ready(&exe, &make("v9")).unwrap();
        assert!(install_ready(&exe, &v("9.0.0")).is_err());
        assert!(!ready_path(&exe).exists());
        // The good case.
        save_ready(&exe, &make("v9")).unwrap();
        assert_eq!(install_ready(&exe, &own).unwrap(), v("9.0.0"));
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "v9");
        assert!(ready(&exe).is_none());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_watcher_checks_once_per_interval_and_waits_for_the_dashboard() {
        let day = 24 * 3600;
        let base = WatchInput {
            now: 10 * day,
            last_check: None,
            auto: true,
            interval_h: 24,
            dashboard_running: false,
            ready: false,
        };
        let with = |f: &dyn Fn(&mut WatchInput)| {
            let mut i = WatchInput { ..base };
            f(&mut i);
            watch_action(&i)
        };
        assert_eq!(watch_action(&base), WatchAction::Check { install: true });
        assert_eq!(
            with(&|i| i.dashboard_running = true),
            WatchAction::Check { install: false },
            "a dashboard is open: download only"
        );
        assert_eq!(
            with(&|i| i.last_check = Some(10 * day - 3600)),
            WatchAction::Wait
        );
        assert_eq!(
            with(&|i| i.last_check = Some(9 * day)),
            WatchAction::Check { install: true },
            "a day later"
        );
        assert_eq!(
            with(&|i| {
                i.last_check = Some(10 * day - 3 * 3600);
                i.interval_h = 2;
            }),
            WatchAction::Check { install: true }
        );
        assert_eq!(
            with(&|i| i.last_check = Some(11 * day)),
            WatchAction::Check { install: true },
            "a clock set back does not block checks for days"
        );
        assert_eq!(with(&|i| i.auto = false), WatchAction::Wait);
        assert_eq!(with(&|i| i.ready = true), WatchAction::InstallReady);
        assert_eq!(
            with(&|i| {
                i.ready = true;
                i.dashboard_running = true;
            }),
            WatchAction::Wait
        );
        let dir = temp_dir("last");
        let file = dir.join("update-check.txt");
        assert_eq!(read_last_check(&file), None);
        write_last_check(&file, 1234);
        assert_eq!(read_last_check(&file), Some(1234));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// A tiny HTTP server with fixed answers, for the whole download path
    /// without the network.
    fn serve(routes: Vec<(String, Vec<u8>)>) -> String {
        use std::io::{BufRead, BufReader};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut reader = BufReader::new(stream.try_clone().unwrap());
                let mut first = String::new();
                if reader.read_line(&mut first).is_err() {
                    continue;
                }
                let mut line = String::new();
                while reader.read_line(&mut line).is_ok_and(|n| n > 2) {
                    line.clear();
                }
                let path = first.split(' ').nth(1).unwrap_or("").to_string();
                let found = routes.iter().find(|(p, _)| *p == path);
                let (status, body) = match found {
                    Some((_, b)) => ("200 OK", b.clone()),
                    None => ("404 Not Found", Vec::new()),
                };
                let mut out = stream;
                let head = format!(
                    "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = out.write_all(head.as_bytes());
                let _ = out.write_all(&body);
            }
        });
        format!("http://{addr}")
    }

    fn fixture(program: &[u8], listed: &[u8]) -> (Source, String) {
        let platform = platform().unwrap_or("linux-x86_64");
        let name = asset_name("v9.0.0", platform);
        let mut ctx = digest::Context::new(&digest::SHA256);
        ctx.update(listed);
        let sums = format!("{}  {name}\n", to_hex(ctx.finish().as_ref()));
        let api = format!(
            r#"{{"tag_name":"v9.0.0","assets":[{{"name":"{name}"}},{{"name":"SHA256SUMS"}}]}}"#
        );
        let base = serve(vec![
            ("/api".into(), api.into_bytes()),
            ("/dl/v9.0.0/SHA256SUMS".into(), sums.into_bytes()),
            (format!("/dl/v9.0.0/{name}"), program.to_vec()),
        ]);
        let source = Source {
            api_url: format!("{base}/api"),
            download_base: format!("{base}/dl"),
            https_only: false,
        };
        (source, name)
    }

    #[test]
    fn download_verify_replace_through_a_local_server() {
        let dir = temp_dir("http");
        let exe = dir.join("telemetrix.exe");
        std::fs::write(&exe, "old program").unwrap();
        let program = b"new program".repeat(10_000);
        let (source, name) = fixture(&program, &program);
        let release = latest(&source).unwrap();
        assert_eq!(release.version, v("9.0.0"));
        assert_eq!(
            pick_asset(&release, platform().unwrap_or("linux-x86_64")).unwrap(),
            name
        );
        let d = download(&source, &release, &name, &exe).unwrap();
        assert_eq!(d.bytes, program.len() as u64);
        replace_exe(&exe, &d.path).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), program);

        // A mismatch deletes the download and leaves the program alone.
        let (bad, name) = fixture(b"evil program", b"good program");
        let release = latest(&bad).unwrap();
        let e = download(&bad, &release, &name, &exe).unwrap_err();
        assert!(e.contains("checksum mismatch"), "{e}");
        assert_eq!(std::fs::read(&exe).unwrap(), program);
        assert!(!dir.join("telemetrix.exe.download").exists());

        // https only for the real source: a plain http address is refused.
        let strict = Source {
            https_only: true,
            ..bad
        };
        assert!(matches!(latest(&strict), Err(CheckError::Failed(_))));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn the_watcher_downloads_only_while_a_dashboard_is_open() {
        let dir = temp_dir("watch");
        let exe = dir.join("telemetrix.exe");
        std::fs::write(&exe, "old").unwrap();
        let (source, _) = fixture(b"new", b"new");
        let check = dir.join("update-check.txt");
        let input = WatchInput {
            now: 1_000_000,
            last_check: None,
            auto: true,
            interval_h: 24,
            dashboard_running: true,
            ready: false,
        };
        let mut log = Vec::new();
        let mut say = |t: &str| log.push(t.to_string());
        let open = || true;
        let out = watch_step(&source, &exe, &input, &check, &open, &mut say);
        assert_eq!(out, WatchOutcome::Nothing);
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "old");
        assert_eq!(ready(&exe).unwrap().version, "9.0.0");
        assert_eq!(read_last_check(&check), Some(1_000_000));
        // The dashboard closed: the ready file is installed.
        let closed = || false;
        let later = WatchInput {
            ready: true,
            dashboard_running: false,
            ..input
        };
        let out = watch_step(&source, &exe, &later, &check, &closed, &mut say);
        assert_eq!(out, WatchOutcome::Installed(v("9.0.0")));
        assert_eq!(std::fs::read_to_string(&exe).unwrap(), "new");
        assert!(
            log.iter().any(|l| l.contains("installs when it closes")),
            "{log:?}"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}
