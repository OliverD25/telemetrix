pub mod cpu;
pub mod gpu;
#[cfg(windows)]
pub mod gpu_d3dkmt;
#[cfg(not(windows))]
pub mod gpu_sysfs;
pub mod network;
pub mod worker;

use network::NetDrive;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemSnapshot {
    pub cpu_usage: f32,
    pub cpu_temp: Option<f32>,
    pub ram_used_bytes: u64,
    pub ram_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
    pub disks: Vec<DiskMetric>,
    /// GPUs from `gpu.source`; empty without readings or with `gpu.enabled = false`.
    pub gpus: Vec<GpuMetric>,
    /// Network mounts on Linux; Windows has its own network-drive thread.
    pub network: Vec<NetDrive>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiskMetric {
    pub mount: String,
    /// The volume label on Windows ("System Disk"); `None` on Linux.
    pub label: Option<String>,
    pub used_bytes: u64,
    pub total_bytes: u64,
}

/// One GPU reading; a value the GPU does not report is `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GpuMetric {
    pub name: String,
    pub usage_pct: Option<f32>,
    pub mem_used_bytes: Option<u64>,
    pub mem_total_bytes: Option<u64>,
    pub temp_c: Option<f32>,
    pub power_w: Option<f32>,
}

impl DiskMetric {
    /// `System Disk (C:)`, or the mount point when there is no label.
    pub fn title(&self) -> String {
        match &self.label {
            Some(label) => format!("{label} ({})", network::normalize_mount(&self.mount)),
            None => self.mount.clone(),
        }
    }
}

impl SystemSnapshot {
    pub fn ram_pct(&self) -> f32 {
        pct(self.ram_used_bytes, self.ram_total_bytes)
    }
}

pub fn pct(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used as f64 * 100.0 / total as f64) as f32
    }
}
