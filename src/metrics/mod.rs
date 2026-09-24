#[derive(Clone, Debug, Default, PartialEq)]
pub struct SystemSnapshot {
    pub cpu_usage: f32,
    pub cpu_temp: Option<f32>,
    pub ram_used_bytes: u64,
    pub ram_total_bytes: u64,
    pub swap_used_bytes: u64,
    pub swap_total_bytes: u64,
    pub disks: Vec<DiskMetric>,
    /// Always empty in v0.1; GPU metrics are planned for v0.2.
    pub gpus: Vec<GpuMetric>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiskMetric {
    pub mount: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GpuMetric {
    pub name: String,
    pub usage_pct: Option<f32>,
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
