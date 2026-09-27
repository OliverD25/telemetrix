use std::collections::BTreeSet;
use std::process::{Command, Stdio};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sysinfo::{Components, DiskRefreshKind, Disks, MemoryRefreshKind, System};

use super::cpu::{self, CpuMeter};
use super::gpu::Nvml;
use super::network::{LINUX_NETWORK_FS, NetDrive, split_remote};
use super::{DiskMetric, GpuMetric, SystemSnapshot};
use crate::config::Config;
use crate::event::{AppEvent, WorkerCmd};

/// CPU usage needs two readings at least 200 ms apart for a real value.
const CPU_PRIME: Duration = Duration::from_millis(250);
/// The sensor probe child normally answers in well under a second.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
/// In order of preference: the whole CPU package (Intel), the control
/// temperature (AMD), then any Intel or AMD CPU sensor.
const PREFERRED_SENSORS: [&str; 4] = ["Package id 0", "Tctl", "coretemp", "k10temp"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetricsIntervals {
    pub cpu: Duration,
    pub memory: Duration,
    pub temps: Duration,
    pub disks: Duration,
    pub gpu: Duration,
    pub gpu_enabled: bool,
}

impl MetricsIntervals {
    pub fn from_config(cfg: &Config) -> Self {
        let ms = Duration::from_millis;
        let m = &cfg.metrics;
        Self {
            cpu: ms(m.cpu_interval_ms),
            memory: ms(m.memory_interval_ms),
            temps: ms(m.temps_interval_ms),
            disks: ms(m.disks_interval_ms),
            gpu: ms(cfg.gpu.interval_ms),
            gpu_enabled: cfg.gpu.enabled,
        }
    }

    fn all(&self) -> [Duration; 5] {
        [self.cpu, self.memory, self.temps, self.disks, self.gpu]
    }
}

fn disk_kind() -> DiskRefreshKind {
    DiskRefreshKind::nothing().with_storage()
}

pub struct Sampler {
    sys: System,
    cpu: CpuMeter,
    components: Components,
    /// False when no sensor reported a temperature at start (most Windows PCs).
    temps: bool,
    disks: Disks,
    /// Loaded only while `gpu.enabled`; `None` also when loading failed.
    nvml: Option<Nvml>,
    gpus: Vec<GpuMetric>,
}

impl Sampler {
    /// Creates the sampler and takes the first real CPU sample (about 250 ms).
    pub fn new() -> Self {
        let mut sys = System::new_with_specifics(
            cpu::refresh_kind().with_memory(MemoryRefreshKind::nothing().with_ram().with_swap()),
        );
        let mut cpu = CpuMeter::new();
        cpu.refresh(&mut sys);
        thread::sleep(CPU_PRIME);
        cpu.refresh(&mut sys);
        sys.refresh_memory();
        let components = if sensors_worth_loading() {
            Components::new_with_refreshed_list()
        } else {
            Components::new()
        };
        let temps = components.iter().any(|c| c.temperature().is_some());
        Self {
            sys,
            cpu,
            // An empty list is dropped, so later refreshes cost nothing.
            components: if temps { components } else { Components::new() },
            temps,
            disks: Disks::new_with_refreshed_list_specifics(disk_kind()),
            nvml: None,
            gpus: Vec::new(),
        }
    }

    /// Loads or unloads NVML; returns the line for the log. A failure only
    /// means no GPU card.
    pub fn set_gpu(&mut self, enabled: bool) -> String {
        self.gpus.clear();
        if !enabled {
            self.nvml = None;
            return "gpu: off (gpu.enabled = false)".into();
        }
        match Nvml::load() {
            Ok(nvml) => {
                let names = nvml.names().join(", ");
                self.gpus = nvml.read();
                self.nvml = Some(nvml);
                format!("gpu: NVML loaded: {names}")
            }
            Err(reason) => {
                self.nvml = None;
                format!("gpu: {reason}; no GPU card")
            }
        }
    }

    /// Refreshes the sources whose index (cpu, memory, temps, disks, gpu) is set.
    fn refresh(&mut self, due: [bool; 5]) {
        if due[0] {
            self.cpu.refresh(&mut self.sys);
        }
        if due[1] {
            self.sys.refresh_memory();
        }
        if due[2] && self.temps {
            self.components.refresh(true);
        }
        if due[3] {
            self.disks.refresh_specifics(true, disk_kind());
        }
        if due[4]
            && let Some(nvml) = &self.nvml
        {
            self.gpus = nvml.read();
        }
    }

    pub fn snapshot(&self) -> SystemSnapshot {
        let sensors = self.components.iter().map(|c| (c.label(), c.temperature()));
        let disks = self.disks.iter().map(|d| RawDisk {
            mount: d.mount_point().to_string_lossy().into_owned(),
            name: d.name().to_string_lossy().into_owned(),
            file_system: d.file_system().to_string_lossy().into_owned(),
            total: d.total_space(),
            available: d.available_space(),
        });
        let (disks, network) = split_disks(disks);
        SystemSnapshot {
            cpu_usage: self.cpu.usage(),
            cpu_temp: pick_cpu_temp(sensors),
            ram_used_bytes: self.sys.used_memory(),
            ram_total_bytes: self.sys.total_memory(),
            swap_used_bytes: self.sys.used_swap(),
            swap_total_bytes: self.sys.total_swap(),
            disks,
            gpus: self.gpus.clone(),
            network,
        }
    }
}

/// True when a sensor reports a temperature; the `probe-temps` command.
pub fn any_temperature() -> bool {
    Components::new_with_refreshed_list()
        .iter()
        .any(|c| c.temperature().is_some())
}

/// On Windows sysinfo reads sensors through WMI, which loads about 3.7 MB of
/// COM code that stays for the life of the process, while most PCs report no
/// sensor at all. A short child process asks first, so the dashboard loads WMI
/// only when it will get a temperature from it.
fn sensors_worth_loading() -> bool {
    if !cfg!(windows) {
        return true;
    }
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let child = Command::new(exe)
        .arg("probe-temps")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn();
    let Ok(mut child) = child else {
        return false;
    };
    let end = Instant::now() + PROBE_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < end => thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                return false;
            }
        }
    }
}

pub fn read_once(gpu: bool) -> SystemSnapshot {
    let mut sampler = Sampler::new();
    if gpu {
        sampler.set_gpu(true);
    }
    sampler.snapshot()
}

/// Known CPU package sensors first, else the hottest CPU-like sensor.
pub fn pick_cpu_temp<'a>(sensors: impl Iterator<Item = (&'a str, Option<f32>)>) -> Option<f32> {
    let readings: Vec<(&str, f32)> = sensors
        .filter_map(|(label, t)| t.filter(|t| t.is_finite()).map(|t| (label, t)))
        .collect();
    let preferred = PREFERRED_SENSORS
        .iter()
        .find_map(|p| readings.iter().find(|(label, _)| label.contains(p)));
    if let Some((_, t)) = preferred {
        return Some(*t);
    }
    readings
        .iter()
        .filter(|(label, _)| {
            let l = label.to_ascii_lowercase();
            l.contains("cpu") || l.contains("core")
        })
        .map(|(_, t)| *t)
        .reduce(f32::max)
}

pub struct RawDisk {
    pub mount: String,
    /// The volume label on Windows, the device on Linux.
    pub name: String,
    pub file_system: String,
    pub total: u64,
    pub available: u64,
}

/// Mount points that are never a user's disk: WSL internals and snap
/// packages (each snap is a read-only image mounted under /snap).
const IGNORED_MOUNT_PREFIXES: [&str; 6] = [
    "/usr/lib/wsl",
    "/usr/lib/modules",
    "/mnt/wslg",
    "/mnt/wsl",
    "/init",
    "/snap",
];
/// File systems of package images, never a user's disk.
const IGNORED_FS: [&str; 2] = ["squashfs", "fuse.snapfuse"];
/// How WSL mounts the Windows drives of its host.
const WSL_HOST_FS: [&str; 2] = ["9p", "drvfs"];

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DiskClass {
    Local { label: Option<String> },
    Network,
    Ignore,
}

fn under(mount: &str, prefix: &str) -> bool {
    mount == prefix
        || mount
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// `/proc/mounts` writes a space as `\040` and a backslash as `\134`.
pub fn unescape_octal(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let code = bytes.get(i + 1..i + 4).and_then(|d| {
            let d = std::str::from_utf8(d).ok()?;
            d.bytes()
                .all(|b| (b'0'..=b'7').contains(&b))
                .then(|| u8::from_str_radix(d, 8).ok())?
        });
        match (bytes[i], code) {
            (b'\\', Some(c)) => {
                out.push(c);
                i += 4;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Where a mount belongs. On Windows every disk sysinfo reports is local
/// and its name is the volume label.
pub fn classify(mount: &str, file_system: &str, source: &str) -> DiskClass {
    let fs = file_system.to_ascii_lowercase();
    if cfg!(windows) {
        return DiskClass::Local {
            label: (!source.is_empty()).then(|| source.to_string()),
        };
    }
    if IGNORED_FS.contains(&fs.as_str()) || IGNORED_MOUNT_PREFIXES.iter().any(|p| under(mount, p)) {
        return DiskClass::Ignore;
    }
    // WSL mounts the host's drive C: at /mnt/c with the 9p file system: that
    // is a local disk of the host, not a network share.
    let letter = mount
        .strip_prefix("/mnt/")
        .filter(|l| l.len() == 1 && l.bytes().all(|b| b.is_ascii_alphabetic()));
    let source_letter = source.get(..2).filter(|s| s.ends_with(':'));
    if WSL_HOST_FS.contains(&fs.as_str())
        && let (Some(l), Some(s)) = (letter, source_letter)
        && l.eq_ignore_ascii_case(&s[..1])
    {
        return DiskClass::Local {
            label: Some(s.to_ascii_uppercase()),
        };
    }
    if LINUX_NETWORK_FS.contains(&fs.as_str()) {
        DiskClass::Network
    } else {
        DiskClass::Local { label: None }
    }
}

/// Drops pseudo disks with no size, repeated mount points and system
/// mounts, and moves network file systems to the Network card.
pub fn split_disks(disks: impl Iterator<Item = RawDisk>) -> (Vec<DiskMetric>, Vec<NetDrive>) {
    let mut seen = BTreeSet::new();
    let (mut local, mut network) = (Vec::new(), Vec::new());
    for d in disks.filter(|d| d.total > 0 && seen.insert(d.mount.clone())) {
        let mount = unescape_octal(&d.mount);
        let source = unescape_octal(&d.name);
        match classify(&mount, &d.file_system, &source) {
            DiskClass::Ignore => {}
            DiskClass::Network => {
                let (server, share) = split_remote(&source);
                network.push(NetDrive {
                    mount,
                    server,
                    share,
                    label: None,
                    online: true,
                    total_bytes: d.total,
                    free_bytes: d.available,
                });
            }
            DiskClass::Local { label } => local.push(DiskMetric {
                label,
                mount,
                used_bytes: d.total.saturating_sub(d.available),
                total_bytes: d.total,
            }),
        }
    }
    // Windows reports volumes in its own order (H: C: D: ...); every card
    // and both snapshot formats read this list, so sort it once here.
    local.sort_by_cached_key(|d| super::network::mount_order(&d.mount));
    (local, network)
}

pub type MetricsCmd = WorkerCmd<MetricsIntervals>;

pub fn spawn(
    intervals: MetricsIntervals,
    tx: Sender<AppEvent>,
) -> (Sender<MetricsCmd>, JoinHandle<()>) {
    let (cmd_tx, cmd_rx) = mpsc::channel();
    let handle = thread::Builder::new()
        .name("metrics".into())
        .spawn(move || run(intervals, &tx, &cmd_rx))
        .expect("spawn the metrics thread");
    (cmd_tx, handle)
}

fn run(mut intervals: MetricsIntervals, tx: &Sender<AppEvent>, cmds: &mpsc::Receiver<MetricsCmd>) {
    let mut sampler = Sampler::new();
    let mut last = sampler.snapshot();
    if tx.send(AppEvent::Metrics(last.clone())).is_err() {
        return;
    }
    // After the first snapshot, so starting NVML never delays the CPU and RAM cards.
    if intervals.gpu_enabled && tx.send(AppEvent::Log(sampler.set_gpu(true))).is_err() {
        return;
    }
    let start = Instant::now();
    let mut next = intervals.all().map(|iv| start + iv);
    loop {
        let deadline = next.iter().copied().min().unwrap_or(start);
        match cmds.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(WorkerCmd::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(WorkerCmd::Reconfigure(new)) => {
                if new.gpu_enabled != intervals.gpu_enabled {
                    let _ = tx.send(AppEvent::Log(sampler.set_gpu(new.gpu_enabled)));
                }
                intervals = new;
                let now = Instant::now();
                next = intervals.all().map(|iv| now + iv);
                continue;
            }
            Ok(WorkerCmd::RunNow) | Err(RecvTimeoutError::Timeout) => {}
        }
        let now = Instant::now();
        let due = next.map(|n| now >= n);
        sampler.refresh(due);
        for (n, iv) in next.iter_mut().zip(intervals.all()) {
            if now >= *n {
                *n = now + iv;
            }
        }
        let snapshot = sampler.snapshot();
        if snapshot != last {
            if tx.send(AppEvent::Metrics(snapshot.clone())).is_err() {
                return;
            }
            last = snapshot;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octal_escapes_decode() {
        assert_eq!(unescape_octal(r"C:\134"), r"C:\");
        assert_eq!(unescape_octal(r"/mnt/my\040disk"), "/mnt/my disk");
        assert_eq!(unescape_octal(r"plain\9"), r"plain\9");
    }

    #[cfg(not(windows))]
    #[test]
    fn linux_mounts_are_classified() {
        let local = |label: Option<&str>| DiskClass::Local {
            label: label.map(Into::into),
        };
        // WSL internals and snap images are dropped.
        for (mount, fs, source) in [
            ("/usr/lib/wsl/drivers", "9p", "drivers"),
            ("/usr/lib/wsl/lib", "overlay", "none"),
            ("/usr/lib/modules/6.18-WSL2", "overlay", "none"),
            ("/mnt/wslg/distro", "ext4", "/dev/sdd"),
            ("/mnt/wslg", "tmpfs", "none"),
            ("/init", "rootfs", "rootfs"),
            ("/snap", "ext4", "/dev/sdd"),
            ("/snap/code/265", "fuse.snapfuse", "snapfuse"),
            ("/media/x", "squashfs", "/dev/loop3"),
        ] {
            assert_eq!(classify(mount, fs, source), DiskClass::Ignore, "{mount}");
        }
        // The host's Windows drives are local disks, labelled with their letter.
        assert_eq!(classify("/mnt/c", "9p", r"C:\"), local(Some("C:")));
        assert_eq!(classify("/mnt/e", "drvfs", r"E:\"), local(Some("E:")));
        // Real disks stay local; "/mnt/wslgames" is not under "/mnt/wslg".
        assert_eq!(classify("/", "ext4", "/dev/sdd"), local(None));
        assert_eq!(classify("/mnt/wslgames", "ext4", "/dev/sde1"), local(None));
        assert_eq!(classify("/home", "btrfs", "/dev/nvme0n1p2"), local(None));
        // Network file systems go to the Network card, 9p included when it is
        // not a WSL host drive.
        assert_eq!(
            classify("/mnt/nas", "nfs4", "nas:/export"),
            DiskClass::Network
        );
        assert_eq!(
            classify("/mnt/share", "cifs", "//nas/media"),
            DiskClass::Network
        );
        assert_eq!(classify("/srv/vm", "9p", "hostshare"), DiskClass::Network);
        assert_eq!(classify("/mnt/x", "9p", "hostshare"), DiskClass::Network);
    }

    #[test]
    fn temperature_prefers_package_sensors() {
        let sensors = [
            ("acpitz", Some(90.0)),
            ("coretemp Package id 0", Some(52.0)),
            ("Core 1", Some(60.0)),
        ];
        assert_eq!(pick_cpu_temp(sensors.into_iter()), Some(52.0));
        let k10 = [("nvme Composite", Some(40.0)), ("k10temp Tctl", Some(61.5))];
        assert_eq!(pick_cpu_temp(k10.into_iter()), Some(61.5));
    }

    #[test]
    fn linux_hwmon_lists_pick_the_package_sensor() {
        // Labels as sysinfo reports them on Linux: "<hwmon name> <label>".
        let intel = [
            ("acpitz temp1", Some(27.8)),
            ("coretemp Core 0", Some(51.0)),
            ("coretemp Core 1", Some(58.0)),
            ("coretemp Package id 0", Some(56.0)),
            ("nvme Composite WDC", Some(38.9)),
        ];
        assert_eq!(pick_cpu_temp(intel.into_iter()), Some(56.0));
        let amd = [
            ("amdgpu edge", Some(45.0)),
            ("k10temp Tccd1", Some(48.5)),
            ("k10temp Tctl", Some(61.2)),
        ];
        assert_eq!(
            pick_cpu_temp(amd.into_iter()),
            Some(61.2),
            "Tctl wins over a die sensor"
        );
        let cores_only = [
            ("coretemp Core 0", Some(51.0)),
            ("coretemp Core 1", Some(58.0)),
        ];
        assert_eq!(
            pick_cpu_temp(cores_only.into_iter()),
            Some(51.0),
            "first coretemp sensor"
        );
        let no_value = [
            ("coretemp Package id 0", None),
            ("k10temp Tctl", Some(f32::NAN)),
        ];
        assert_eq!(
            pick_cpu_temp(no_value.into_iter()),
            None,
            "missing and NaN readings are skipped"
        );
    }

    #[test]
    fn temperature_falls_back_to_hottest_cpu_sensor() {
        let sensors = [
            ("CPU 0", Some(48.0)),
            ("core 3", Some(55.0)),
            ("gpu", Some(80.0)),
            ("cpu x", None),
        ];
        assert_eq!(pick_cpu_temp(sensors.into_iter()), Some(55.0));
        assert_eq!(pick_cpu_temp([("acpitz", Some(40.0))].into_iter()), None);
        assert_eq!(pick_cpu_temp(std::iter::empty()), None);
    }

    #[test]
    fn disks_are_sorted_like_explorer() {
        let raw = |mount: &str| RawDisk {
            mount: mount.into(),
            name: String::new(),
            file_system: "ext4".into(),
            total: 100,
            available: 50,
        };
        let mounts: &[&str] = if cfg!(windows) {
            &[r"H:", r"C:", r"d:", r"G:", r"E:"]
        } else {
            &["/mnt/d", "/home", "/", "/mnt/c", "/data"]
        };
        let (disks, _) = split_disks(mounts.iter().map(|m| raw(m)));
        let order: Vec<&str> = disks.iter().map(|d| d.mount.as_str()).collect();
        if cfg!(windows) {
            assert_eq!(order, [r"C:", r"d:", r"E:", r"G:", r"H:"]);
        } else {
            assert_eq!(order, ["/", "/data", "/home", "/mnt/c", "/mnt/d"]);
        }
    }

    #[test]
    fn disks_drop_empty_and_duplicate_mounts() {
        let raw = |mount: &str, name: &str, fs: &str, total, available| RawDisk {
            mount: mount.into(),
            name: name.into(),
            file_system: fs.into(),
            total,
            available,
        };
        let (a, b) = if cfg!(windows) {
            (r"C:\", r"D:\")
        } else {
            ("/", "/data")
        };
        let list = vec![
            raw(
                a,
                if cfg!(windows) {
                    "System Disk"
                } else {
                    "/dev/sda1"
                },
                "ext4",
                100,
                30,
            ),
            raw("/proc", "proc", "proc", 0, 0),
            raw(a, "again", "ext4", 100, 30),
            raw(b, "", "ext4", 50, 60),
        ];
        let (disks, _) = split_disks(list.into_iter());
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].used_bytes, 70);
        assert_eq!(
            disks[1].used_bytes, 0,
            "available above total must not underflow"
        );
        if cfg!(windows) {
            assert_eq!(disks[0].title(), "System Disk (C:)");
            assert_eq!(disks[1].title(), r"D:\", "no label: the mount point");
        } else {
            let nfs = vec![raw("/mnt/nas", "nas:/export", "nfs4", 1000, 400)];
            let (disks, network) = split_disks(nfs.into_iter());
            assert!(disks.is_empty());
            assert_eq!(network[0].server.as_deref(), Some("nas"));
            assert_eq!(network[0].free_bytes, 400);
            let wsl = vec![raw("/mnt/c", r"C:\134", "9p", 1000, 400)];
            let (disks, network) = split_disks(wsl.into_iter());
            assert!(network.is_empty());
            assert_eq!(disks[0].title(), "C: (/mnt/c)");
        }
    }
}
