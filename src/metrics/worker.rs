use std::collections::BTreeSet;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use sysinfo::{
    Components, CpuRefreshKind, DiskRefreshKind, Disks, MemoryRefreshKind, RefreshKind, System,
};

use super::{DiskMetric, SystemSnapshot};
use crate::config::Config;
use crate::event::{AppEvent, WorkerCmd};

/// sysinfo needs two CPU refreshes at least 200 ms apart for a real value.
const CPU_PRIME: Duration = Duration::from_millis(250);
const PREFERRED_SENSORS: [&str; 4] = ["coretemp", "k10temp", "Tctl", "Package id 0"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MetricsIntervals {
    pub cpu: Duration,
    pub memory: Duration,
    pub temps: Duration,
    pub disks: Duration,
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
        }
    }

    fn all(&self) -> [Duration; 4] {
        [self.cpu, self.memory, self.temps, self.disks]
    }
}

fn disk_kind() -> DiskRefreshKind {
    DiskRefreshKind::nothing().with_storage()
}

pub struct Sampler {
    sys: System,
    components: Components,
    disks: Disks,
}

impl Sampler {
    /// Creates the sampler and takes the first real CPU sample (about 250 ms).
    pub fn new() -> Self {
        let mut sys = System::new_with_specifics(
            RefreshKind::nothing()
                .with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
                .with_memory(MemoryRefreshKind::nothing().with_ram().with_swap()),
        );
        sys.refresh_cpu_usage();
        thread::sleep(CPU_PRIME);
        sys.refresh_cpu_usage();
        sys.refresh_memory();
        Self {
            sys,
            components: Components::new_with_refreshed_list(),
            disks: Disks::new_with_refreshed_list_specifics(disk_kind()),
        }
    }

    /// Refreshes the sources whose index (cpu, memory, temps, disks) is set.
    fn refresh(&mut self, due: [bool; 4]) {
        if due[0] {
            self.sys.refresh_cpu_usage();
        }
        if due[1] {
            self.sys.refresh_memory();
        }
        if due[2] {
            self.components.refresh(true);
        }
        if due[3] {
            self.disks.refresh_specifics(true, disk_kind());
        }
    }

    pub fn snapshot(&self) -> SystemSnapshot {
        let sensors = self.components.iter().map(|c| (c.label(), c.temperature()));
        let disks = self.disks.iter().map(|d| {
            (
                d.mount_point().to_string_lossy().into_owned(),
                d.total_space(),
                d.available_space(),
            )
        });
        SystemSnapshot {
            cpu_usage: self.sys.global_cpu_usage(),
            cpu_temp: pick_cpu_temp(sensors),
            ram_used_bytes: self.sys.used_memory(),
            ram_total_bytes: self.sys.total_memory(),
            swap_used_bytes: self.sys.used_swap(),
            swap_total_bytes: self.sys.total_swap(),
            disks: real_disks(disks),
            gpus: Vec::new(),
        }
    }
}

pub fn read_once() -> SystemSnapshot {
    Sampler::new().snapshot()
}

/// Known CPU package sensors first, else the hottest CPU-like sensor.
pub fn pick_cpu_temp<'a>(sensors: impl Iterator<Item = (&'a str, Option<f32>)>) -> Option<f32> {
    let readings: Vec<(&str, f32)> = sensors
        .filter_map(|(label, t)| t.filter(|t| t.is_finite()).map(|t| (label, t)))
        .collect();
    let preferred = readings
        .iter()
        .find(|(label, _)| PREFERRED_SENSORS.iter().any(|p| label.contains(p)));
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

/// Drops pseudo disks with no size and repeated mount points.
pub fn real_disks(disks: impl Iterator<Item = (String, u64, u64)>) -> Vec<DiskMetric> {
    let mut seen = BTreeSet::new();
    disks
        .filter(|(mount, total, _)| *total > 0 && seen.insert(mount.clone()))
        .map(|(mount, total, available)| DiskMetric {
            mount,
            used_bytes: total.saturating_sub(available),
            total_bytes: total,
        })
        .collect()
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
    let start = Instant::now();
    let mut next = intervals.all().map(|iv| start + iv);
    loop {
        let deadline = next.iter().copied().min().unwrap_or(start);
        match cmds.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(WorkerCmd::Stop) | Err(RecvTimeoutError::Disconnected) => return,
            Ok(WorkerCmd::Reconfigure(new)) => {
                intervals = new;
                let now = Instant::now();
                next = intervals.all().map(|iv| now + iv);
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
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
    fn disks_drop_empty_and_duplicate_mounts() {
        let raw = vec![
            ("C:\\".to_string(), 100, 30),
            ("/proc".to_string(), 0, 0),
            ("C:\\".to_string(), 100, 30),
            ("D:\\".to_string(), 50, 60),
        ];
        let disks = real_disks(raw.into_iter());
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].used_bytes, 70);
        assert_eq!(
            disks[1].used_bytes, 0,
            "available above total must not underflow"
        );
    }
}
