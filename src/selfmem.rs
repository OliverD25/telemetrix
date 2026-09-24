//! The program's own memory use (decision 31): a cheap reading, taken every
//! few seconds, shown in the status bar and checked against the budget.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelfMemory {
    pub working_set: u64,
    pub peak_working_set: Option<u64>,
    /// Committed private bytes; `None` where the OS has no cheap equivalent.
    pub private: Option<u64>,
}

pub const MB: u64 = 1024 * 1024;

pub fn mb(bytes: u64) -> f64 {
    bytes as f64 / MB as f64
}

#[cfg(windows)]
pub fn read() -> Option<SelfMemory> {
    use windows_sys::Win32::System::ProcessStatus::{
        K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    let mut c = PROCESS_MEMORY_COUNTERS_EX {
        cb: size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        ..Default::default()
    };
    // SAFETY: `c` is a writable PROCESS_MEMORY_COUNTERS_EX whose `cb` holds its
    // size; the EX struct starts with the fields of the plain one.
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&raw mut c).cast::<PROCESS_MEMORY_COUNTERS>(),
            c.cb,
        )
    } != 0;
    ok.then_some(SelfMemory {
        working_set: c.WorkingSetSize as u64,
        peak_working_set: Some(c.PeakWorkingSetSize as u64),
        private: Some(c.PrivateUsage as u64),
    })
}

#[cfg(target_os = "linux")]
pub fn read() -> Option<SelfMemory> {
    parse_proc_status(&std::fs::read_to_string("/proc/self/status").ok()?)
}

#[cfg(not(any(windows, target_os = "linux")))]
pub fn read() -> Option<SelfMemory> {
    None
}

/// `VmRSS` (resident now) and `VmHWM` (resident peak) from `/proc/self/status`.
/// They equal statm's resident pages times the page size, without needing it.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn parse_proc_status(text: &str) -> Option<SelfMemory> {
    let kb = |key: &str| {
        text.lines()
            .find_map(|l| l.strip_prefix(key))
            .and_then(|rest| rest.split_whitespace().next())
            .and_then(|n| n.parse::<u64>().ok())
            .map(|n| n * 1024)
    };
    Some(SelfMemory {
        working_set: kb("VmRSS:")?,
        peak_working_set: kb("VmHWM:"),
        private: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_this_process() {
        if cfg!(any(windows, target_os = "linux")) {
            let m = read().expect("own memory is readable");
            assert!(m.working_set > MB, "{m:?}");
            assert!(m.peak_working_set.unwrap_or(u64::MAX) >= m.working_set);
        }
    }

    #[test]
    fn parses_linux_status() {
        let text = "Name:\ttelemetrix\nVmHWM:\t   10240 kB\nVmRSS:\t    8192 kB\n";
        let m = parse_proc_status(text).unwrap();
        assert_eq!(m.working_set, 8 * MB);
        assert_eq!(m.peak_working_set, Some(10 * MB));
        assert_eq!(m.private, None);
        assert_eq!(parse_proc_status("Name: x\n"), None);
        assert!((mb(9 * MB + MB / 2) - 9.5).abs() < 1e-9);
    }
}
