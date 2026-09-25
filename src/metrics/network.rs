//! Network drives (decisions 27-29).
//!
//! Windows: mapped drive letters are queried on their own thread, never on
//! the metrics thread, because a dead NAS makes `GetDiskFreeSpaceExW` block
//! for 20 s or more. Each drive is asked in a short-lived helper thread and
//! waited for with a timeout; a drive whose helper is still stuck is shown
//! offline and not asked again until that helper ends, so threads cannot
//! pile up.
//!
//! Linux: network mounts come from sysinfo's disk list (see `worker.rs`).
//! Protection against hung NFS or SMB mounts there is best effort in v0.1.

use std::collections::BTreeMap;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;

use crate::config::Config;
use crate::event::{AppEvent, WorkerCmd};

/// File systems that make a Linux mount a network drive.
pub const LINUX_NETWORK_FS: [&str; 7] =
    ["nfs", "nfs4", "cifs", "smb3", "smbfs", "fuse.sshfs", "9p"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetDrive {
    /// `M:` on Windows, the mount point on Linux.
    pub mount: String,
    pub server: Option<String>,
    pub share: Option<String>,
    pub label: Option<String>,
    pub online: bool,
    pub total_bytes: u64,
    pub free_bytes: u64,
}

impl NetDrive {
    pub fn used_bytes(&self) -> u64 {
        self.total_bytes.saturating_sub(self.free_bytes)
    }

    fn name(&self) -> Option<&str> {
        self.share
            .as_deref()
            .or(self.label.as_deref())
            .filter(|s| !s.is_empty())
    }
}

/// One line of the Network card: a single drive or a group of drives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetRow {
    pub title: String,
    pub server: Option<String>,
    pub letters: Vec<String>,
    pub online: bool,
    pub used_bytes: u64,
    pub total_bytes: u64,
}

/// `\\server\share` → (`server`, `share`); `server:/export` → (`server`, `/export`).
pub fn split_remote(remote: &str) -> (Option<String>, Option<String>) {
    let text = |s: &str| (!s.is_empty()).then(|| s.to_string());
    if let Some(rest) = remote
        .strip_prefix(r"\\")
        .or_else(|| remote.strip_prefix("//"))
    {
        let mut parts = rest.splitn(2, ['\\', '/']);
        let server = parts.next().and_then(text);
        let share = parts
            .next()
            .and_then(|s| text(s.trim_end_matches(['\\', '/'])));
        return (server, share);
    }
    match remote.split_once(':') {
        Some((server, path)) if !server.is_empty() => (text(server), text(path)),
        _ => (None, None),
    }
}

/// Server and share from a Windows device path of a mapped drive:
/// `\Device\LanmanRedirector\;M:0000000000012345\server\share` or
/// `\Device\Mup\;LanmanRedirector\;M:000…\server\share`.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn split_device_path(path: &str) -> (Option<String>, Option<String>) {
    let parts: Vec<&str> = path.split('\\').collect();
    let Some(at) = parts
        .iter()
        .rposition(|p| p.starts_with(';') && p.get(2..3) == Some(":"))
    else {
        return (None, None);
    };
    let text = |i: usize| {
        parts
            .get(i)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };
    (text(at + 1), text(at + 2))
}

/// `C:\` and `c:` both become `C:`; Linux paths lose a trailing slash.
pub fn normalize_mount(mount: &str) -> String {
    let trimmed = mount.trim_end_matches(['\\', '/']);
    let trimmed = if trimmed.is_empty() { mount } else { trimmed };
    if trimmed.len() == 2 && trimmed.ends_with(':') {
        trimmed.to_ascii_uppercase()
    } else {
        trimmed.to_string()
    }
}

/// True when `disks.hide` lists this drive letter or mount point.
pub fn is_hidden(mount: &str, hide: &[String]) -> bool {
    let mount = normalize_mount(mount);
    hide.iter()
        .any(|h| normalize_mount(h).eq_ignore_ascii_case(&mount))
}

/// Groups drives that are one volume on one server (same server, same
/// total and free bytes); everything else, and every offline drive, is its
/// own row.
pub fn rows(drives: &[NetDrive], group: bool, hide: &[String]) -> Vec<NetRow> {
    let mut visible: Vec<&NetDrive> = drives
        .iter()
        .filter(|d| !is_hidden(&d.mount, hide))
        .collect();
    visible.sort_by(|a, b| a.mount.cmp(&b.mount));
    let key = |d: &NetDrive| {
        d.server
            .as_ref()
            .filter(|_| group && d.online)
            .map(|s| (s.to_ascii_lowercase(), d.total_bytes, d.free_bytes))
    };
    let mut groups: BTreeMap<(String, u64, u64), Vec<&NetDrive>> = BTreeMap::new();
    for d in &visible {
        if let Some(k) = key(d) {
            groups.entry(k).or_default().push(d);
        }
    }
    let mut out = Vec::new();
    let mut done = std::collections::BTreeSet::new();
    for d in &visible {
        if done.contains(&d.mount) {
            continue;
        }
        let members = key(d).and_then(|k| groups.get(&k)).filter(|g| g.len() > 1);
        match members {
            Some(members) => {
                let letters: Vec<String> =
                    members.iter().map(|m| normalize_mount(&m.mount)).collect();
                let server = d.server.clone().unwrap_or_default();
                out.push(NetRow {
                    title: format!("{server}  {}", letters.join(" ")),
                    server: d.server.clone(),
                    letters,
                    online: true,
                    used_bytes: d.used_bytes(),
                    total_bytes: d.total_bytes,
                });
                done.extend(members.iter().map(|m| m.mount.clone()));
            }
            None => {
                let letter = normalize_mount(&d.mount);
                let title = match d.name() {
                    Some(name) => format!("{name} ({letter})"),
                    None => letter.clone(),
                };
                out.push(NetRow {
                    title,
                    server: d.server.clone(),
                    letters: vec![letter],
                    online: d.online,
                    used_bytes: d.used_bytes(),
                    total_bytes: d.total_bytes,
                });
                done.insert(d.mount.clone());
            }
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NetworkSettings {
    pub show: bool,
    pub interval: Duration,
    pub timeout: Duration,
}

impl NetworkSettings {
    pub fn from_config(cfg: &Config) -> Self {
        Self {
            show: cfg.disks.show_network,
            interval: Duration::from_secs(cfg.disks.network_interval_s),
            timeout: Duration::from_secs(cfg.disks.network_timeout_s),
        }
    }
}

pub type NetworkCmd = WorkerCmd<NetworkSettings>;

/// Starts the network-drive thread (Windows only; elsewhere it does nothing).
pub fn spawn(settings: NetworkSettings, tx: Sender<AppEvent>) -> Sender<NetworkCmd> {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    if cfg!(windows) {
        let spawned = std::thread::Builder::new()
            .name("network drives".into())
            .stack_size(128 * 1024)
            .spawn(move || run(settings, &tx, &cmd_rx));
        if spawned.is_err() {
            eprintln!("telemetrix: cannot start the network-drive thread");
        }
    }
    cmd_tx
}

fn run(mut settings: NetworkSettings, tx: &Sender<AppEvent>, cmds: &mpsc::Receiver<NetworkCmd>) {
    let mut busy = Busy::new();
    loop {
        if settings.show {
            let drives = query_all(settings.timeout, &mut busy);
            if tx.send(AppEvent::Network(drives)).is_err() {
                return;
            }
        }
        let wait = if settings.show {
            settings.interval
        } else {
            Duration::MAX
        };
        match cmds.recv_timeout(wait) {
            Ok(WorkerCmd::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(WorkerCmd::Reconfigure(new)) => settings = new,
            Ok(WorkerCmd::RunNow) | Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// Drive letters whose helper thread from an earlier round has not ended.
type Busy = BTreeMap<String, std::sync::Arc<std::sync::atomic::AtomicBool>>;

#[cfg(not(windows))]
pub fn query_all(_timeout: Duration, _busy: &mut Busy) -> Vec<NetDrive> {
    Vec::new()
}

/// One round over all mapped drives, each in its own helper thread.
#[cfg(windows)]
pub fn query_all(timeout: Duration, busy: &mut Busy) -> Vec<NetDrive> {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    let (tx, rx) = mpsc::channel::<NetDrive>();
    let mut results: BTreeMap<String, NetDrive> = BTreeMap::new();
    let mut pending: BTreeMap<String, NetDrive> = BTreeMap::new();
    for offline in win::remote_drives() {
        let letter = offline.mount.clone();
        if busy.get(&letter).is_some_and(|b| b.load(Ordering::Relaxed)) {
            results.insert(letter, offline);
            continue;
        }
        let alive = Arc::new(AtomicBool::new(true));
        busy.insert(letter.clone(), alive.clone());
        let tx = tx.clone();
        let base = offline.clone();
        let spawned = std::thread::Builder::new()
            .name(format!("drive {letter}"))
            .stack_size(64 * 1024)
            .spawn(move || {
                let drive = win::query(base);
                alive.store(false, Ordering::Relaxed);
                let _ = tx.send(drive);
            });
        match spawned {
            Ok(_) => {
                pending.insert(letter, offline);
            }
            Err(_) => {
                busy.remove(&letter);
                results.insert(letter, offline);
            }
        }
    }
    drop(tx);
    let end = Instant::now() + timeout;
    while !pending.is_empty() {
        match rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
            Ok(drive) => {
                pending.remove(&drive.mount);
                results.insert(drive.mount.clone(), drive);
            }
            Err(_) => break,
        }
    }
    // Whatever did not answer in time is shown offline.
    results.extend(pending);
    busy.retain(|_, alive| alive.load(Ordering::Relaxed));
    results.into_values().collect()
}

/// A single round with its own bookkeeping, for `snapshot`.
pub fn query_once(timeout: Duration) -> Vec<NetDrive> {
    query_all(timeout, &mut Busy::new())
}

#[cfg(windows)]
mod win {
    use windows_sys::Win32::Storage::FileSystem::{
        GetDiskFreeSpaceExW, GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW,
        QueryDosDeviceW,
    };

    use super::{NetDrive, split_device_path};

    /// `GetDriveTypeW` result for a network drive (from WindowsProgramming,
    /// which is a large feature for one constant).
    const DRIVE_REMOTE: u32 = 4;

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    fn from_wide(buf: &[u16]) -> String {
        let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
        String::from_utf16_lossy(&buf[..end])
    }

    /// Every lettered network drive, marked offline until queried. Drives
    /// without a letter (Explorer "network locations") are not included.
    pub fn remote_drives() -> Vec<NetDrive> {
        // SAFETY: no arguments; returns a bit mask.
        let mask = unsafe { GetLogicalDrives() };
        (0..26u8)
            .filter(|i| mask & (1 << i) != 0)
            .map(|i| format!("{}:", char::from(b'A' + i)))
            .filter(|letter| {
                let root = wide(&format!("{letter}\\"));
                // SAFETY: `root` is a zero-terminated wide string.
                unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
            })
            .map(|letter| {
                let (server, share) =
                    device_path(&letter).map_or((None, None), |p| split_device_path(&p));
                NetDrive {
                    mount: letter,
                    server,
                    share,
                    label: None,
                    online: false,
                    total_bytes: 0,
                    free_bytes: 0,
                }
            })
            .collect()
    }

    /// The device path behind a drive letter, such as
    /// `\Device\LanmanRedirector\;M:0000…\server\share`. kernel32 answers
    /// from the local device table. `WNetGetConnectionW` gives the same names
    /// but loads every network provider DLL into the process, which cost
    /// about 2 MB of working set (measured in step 9).
    fn device_path(letter: &str) -> Option<String> {
        let name = wide(letter);
        let mut buf = vec![0u16; 1024];
        // SAFETY: `name` is zero-terminated; `buf` holds `buf.len()` u16 values.
        let n = unsafe { QueryDosDeviceW(name.as_ptr(), buf.as_mut_ptr(), buf.len() as u32) };
        (n > 0).then(|| from_wide(&buf))
    }

    /// May block for a long time on a dead server: call it on a helper thread.
    pub fn query(mut drive: NetDrive) -> NetDrive {
        let root = wide(&format!("{}\\", drive.mount));
        let (mut avail, mut total, mut free) = (0u64, 0u64, 0u64);
        // SAFETY: `root` is zero-terminated; the three outputs are writable u64s.
        let ok =
            unsafe { GetDiskFreeSpaceExW(root.as_ptr(), &mut avail, &mut total, &mut free) } != 0;
        if !ok || total == 0 {
            return drive;
        }
        drive.online = true;
        drive.total_bytes = total;
        drive.free_bytes = free;
        let mut label = vec![0u16; 261];
        // SAFETY: `root` is zero-terminated; `label` holds 261 u16 values and
        // every other output is allowed to be null.
        let ok = unsafe {
            GetVolumeInformationW(
                root.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        } != 0;
        if ok {
            drive.label = Some(from_wide(&label)).filter(|l| !l.is_empty());
        }
        drive
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TB: u64 = 1_000_000_000_000;

    fn drive(
        letter: &str,
        server: Option<&str>,
        share: &str,
        online: bool,
        total: u64,
        free: u64,
    ) -> NetDrive {
        NetDrive {
            mount: letter.into(),
            server: server.map(Into::into),
            share: Some(share.into()),
            label: None,
            online,
            total_bytes: total,
            free_bytes: free,
        }
    }

    fn nas() -> Vec<NetDrive> {
        let (t, f) = (5_230 * TB / 1000, 2_720 * TB / 1000);
        vec![
            drive("M:", Some("nas"), "music", true, t, f),
            drive("P:", Some("nas"), "photos", true, t, f),
            drive("R:", Some("nas"), "projects", true, t, f),
            drive("W:", Some("nas"), "archive", true, t, f),
            drive("X:", Some("nas"), "vault", true, t, f),
            drive(
                "Y:",
                Some("nas"),
                "Archive",
                true,
                24_400 * TB / 1000,
                8_600 * TB / 1000,
            ),
        ]
    }

    #[test]
    fn same_volume_on_one_server_becomes_one_row() {
        let rows = rows(&nas(), true, &[]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].title, "nas  M: P: R: W: X:");
        assert_eq!(rows[0].letters, ["M:", "P:", "R:", "W:", "X:"]);
        assert_eq!(rows[1].title, "Archive (Y:)");
        assert!(rows.iter().all(|r| r.online));
    }

    #[test]
    fn grouping_off_hide_and_offline() {
        assert_eq!(rows(&nas(), false, &[]).len(), 6);
        let hidden = rows(&nas(), true, &["x:".into(), "Y:\\".into()]);
        assert_eq!(hidden.len(), 1);
        assert_eq!(hidden[0].title, "nas  M: P: R: W:");
        let mut drives = nas();
        drives[1].online = false;
        let rows = rows(&drives, true, &[]);
        let offline: Vec<&NetRow> = rows.iter().filter(|r| !r.online).collect();
        assert_eq!(offline.len(), 1);
        assert_eq!(offline[0].title, "photos (P:)");
        assert_eq!(
            rows[0].letters,
            ["M:", "R:", "W:", "X:"],
            "the offline drive leaves the group"
        );
    }

    #[test]
    fn a_lone_drive_is_never_a_group() {
        let one = vec![drive("Z:", Some("srv"), "data", true, 10, 5)];
        let r = rows(&one, true, &[]);
        assert_eq!(r[0].title, "data (Z:)");
        assert_eq!(r[0].used_bytes, 5);
        let mut nameless = one.clone();
        nameless[0].share = None;
        assert_eq!(rows(&nameless, true, &[])[0].title, "Z:");
    }

    #[test]
    fn remote_names_split() {
        let s = |x: &str| Some(x.to_string());
        assert_eq!(
            split_remote(r"\\nas\music"),
            (s("nas"), s("music"))
        );
        assert_eq!(split_remote("//nas/media/"), (s("nas"), s("media")));
        assert_eq!(
            split_remote("nas:/export/home"),
            (s("nas"), s("/export/home"))
        );
        assert_eq!(split_remote("/dev/sda1"), (None, None));
    }

    #[test]
    fn device_paths_split() {
        let s = |x: &str| Some(x.to_string());
        assert_eq!(
            split_device_path(r"\Device\LanmanRedirector\;M:0000000000012345\nas\music"),
            (s("nas"), s("music"))
        );
        assert_eq!(
            split_device_path(
                r"\Device\Mup\;LanmanRedirector\;Y:000000000001a2b3\nas\Archive"
            ),
            (s("nas"), s("Archive"))
        );
        assert_eq!(split_device_path(r"\Device\HarddiskVolume3"), (None, None));
    }

    #[test]
    fn mounts_normalize() {
        assert_eq!(normalize_mount(r"c:\"), "C:");
        assert_eq!(normalize_mount("/mnt/nas/"), "/mnt/nas");
        assert_eq!(normalize_mount("/"), "/");
        assert!(is_hidden(r"E:\", &["e:".into()]));
        assert!(!is_hidden("/mnt/a", &["/mnt/ab".into()]));
    }
}
