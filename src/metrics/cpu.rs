//! Whole-machine CPU usage.
//!
//! On Windows it comes from `GetSystemTimes`: sysinfo reads CPU usage there
//! through PDH, whose setup loads every performance counter on the machine
//! and cost about 4 MB of private memory (measured in the perf pass).
//! Elsewhere sysinfo is cheap and is used as is.

use sysinfo::{CpuRefreshKind, RefreshKind, System};

/// Two readings of the idle and total CPU time, in any consistent unit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Times {
    pub idle: u64,
    pub total: u64,
}

/// Busy share between two readings, 0..=100.
pub fn usage_between(before: Times, after: Times) -> Option<f32> {
    let total = after.total.checked_sub(before.total)?;
    let idle = after.idle.checked_sub(before.idle)?;
    if total == 0 {
        return None;
    }
    let busy = total.saturating_sub(idle);
    Some((busy as f64 * 100.0 / total as f64).clamp(0.0, 100.0) as f32)
}

/// What sysinfo must refresh for CPU usage on this platform.
pub fn refresh_kind() -> RefreshKind {
    if cfg!(windows) {
        RefreshKind::nothing()
    } else {
        RefreshKind::nothing().with_cpu(CpuRefreshKind::nothing().with_cpu_usage())
    }
}

pub struct CpuMeter {
    last: Option<Times>,
    usage: f32,
}

impl CpuMeter {
    pub fn new() -> Self {
        Self {
            last: None,
            usage: 0.0,
        }
    }

    pub fn refresh(&mut self, sys: &mut System) {
        match read_times() {
            Some(now) => {
                if let Some(u) = self.last.and_then(|before| usage_between(before, now)) {
                    self.usage = u;
                }
                self.last = Some(now);
            }
            None => {
                sys.refresh_cpu_usage();
                self.usage = sys.global_cpu_usage();
            }
        }
    }

    pub fn usage(&self) -> f32 {
        self.usage
    }
}

#[cfg(windows)]
fn read_times() -> Option<Times> {
    #[repr(C)]
    #[derive(Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }
    impl FileTime {
        fn ticks(&self) -> u64 {
            (u64::from(self.high) << 32) | u64::from(self.low)
        }
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
    }
    let (mut idle, mut kernel, mut user) = (
        FileTime::default(),
        FileTime::default(),
        FileTime::default(),
    );
    // SAFETY: three valid, writable FILETIME-shaped structs; the call only writes them.
    let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) } != 0;
    // Kernel time already includes idle time.
    ok.then(|| Times {
        idle: idle.ticks(),
        total: kernel.ticks() + user.ticks(),
    })
}

#[cfg(not(windows))]
fn read_times() -> Option<Times> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_from_two_readings() {
        let a = Times {
            idle: 100,
            total: 200,
        };
        assert_eq!(
            usage_between(
                a,
                Times {
                    idle: 175,
                    total: 300
                }
            ),
            Some(25.0)
        );
        assert_eq!(
            usage_between(
                a,
                Times {
                    idle: 100,
                    total: 300
                }
            ),
            Some(100.0)
        );
        assert_eq!(usage_between(a, a), None, "no time passed");
        assert_eq!(
            usage_between(
                a,
                Times {
                    idle: 50,
                    total: 300
                }
            ),
            None,
            "counter went back"
        );
    }

    #[test]
    fn meter_reads_a_real_value() {
        let mut sys = System::new_with_specifics(refresh_kind());
        let mut meter = CpuMeter::new();
        meter.refresh(&mut sys);
        std::thread::sleep(std::time::Duration::from_millis(250));
        meter.refresh(&mut sys);
        assert!((0.0..=100.0).contains(&meter.usage()));
    }
}
