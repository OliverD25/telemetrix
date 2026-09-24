use std::collections::VecDeque;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const HISTORY_LEN: usize = 240;
pub const TEMP_WARN_C: f32 = 75.0;
const GIB: f64 = (1u64 << 30) as f64;
const SPIKE: Duration = Duration::from_secs(5);
const WIND_DIRS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];

pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Self(seed | 1)
    }

    pub fn from_clock() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E37_79B9_7F4A_7C15);
        Self::new(nanos)
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n.max(1) as u64) as usize
    }
}

pub struct Disk {
    pub mount: &'static str,
    pub used: u64,
    pub total: u64,
}

pub struct FakeData {
    rng: Rng,
    cpu_base: f32,
    spike_until: Option<Instant>,
    pub cpu: f32,
    pub temp_c: f32,
    pub ram_used: u64,
    pub ram_total: u64,
    pub swap_used: u64,
    pub swap_total: u64,
    pub disks: Vec<Disk>,
    pub cpu_history: VecDeque<f32>,
    pub ram_history: VecDeque<f32>,
    pub weather_c: f32,
    pub wind_ms: f32,
    wind_dir: usize,
    pub btc: f64,
    pub eth: f64,
    pub sol: f64,
    pub latency_ms: f32,
    pub uptime_s: u64,
    pub now: SystemTime,
}

fn gib(v: f64) -> u64 {
    (v * GIB) as u64
}

fn disk(mount: &'static str, total_gib: f64, used_pct: f64) -> Disk {
    Disk {
        mount,
        used: gib(total_gib * used_pct / 100.0),
        total: gib(total_gib),
    }
}

impl Default for FakeData {
    fn default() -> Self {
        let mut data = Self {
            rng: Rng::from_clock(),
            cpu_base: 18.0,
            spike_until: None,
            cpu: 18.0,
            temp_c: 55.0,
            ram_used: gib(11.2),
            ram_total: gib(31.9),
            swap_used: gib(0.4),
            swap_total: gib(8.0),
            disks: vec![
                disk("C:\\", 476.3, 71.0),
                disk("D:\\", 931.5, 43.0),
                disk("E:\\", 1863.0, 92.0),
            ],
            cpu_history: VecDeque::with_capacity(HISTORY_LEN),
            ram_history: VecDeque::with_capacity(HISTORY_LEN),
            weather_c: 14.2,
            wind_ms: 3.4,
            wind_dir: 1,
            btc: 64_250.0,
            eth: 3_120.0,
            sol: 148.3,
            latency_ms: 12.0,
            uptime_s: 3 * 86_400 + 4 * 3_600 + 12 * 60,
            now: SystemTime::now(),
        };
        // Pre-fill the history so the sparklines are not empty on the first frame.
        for _ in 0..60 {
            data.tick(Instant::now());
        }
        data.uptime_s -= 60;
        data
    }
}

impl FakeData {
    pub fn spike(&mut self, now: Instant) {
        self.spike_until = Some(now + SPIKE);
        self.cpu = 95.0;
    }

    pub fn tick(&mut self, now: Instant) {
        let spiking = self.spike_until.is_some_and(|until| now < until);
        self.cpu_base = walk(&mut self.rng, self.cpu_base, 4.0, 5.0, 40.0);
        self.cpu = if spiking {
            self.rng.range(93.0, 97.0)
        } else {
            self.cpu_base
        };
        let target = if spiking { 80.0 } else { 55.0 };
        self.temp_c += (target - self.temp_c) * 0.35 + self.rng.range(-0.8, 0.8);

        let ram = walk(
            &mut self.rng,
            self.ram_used as f32 / GIB as f32,
            0.05,
            11.0,
            11.5,
        );
        self.ram_used = gib(ram as f64);
        let swap = walk(
            &mut self.rng,
            self.swap_used as f32 / GIB as f32,
            0.01,
            0.38,
            0.42,
        );
        self.swap_used = gib(swap as f64);

        push_capped(&mut self.cpu_history, self.cpu);
        let ram_pct = self.ram_pct();
        push_capped(&mut self.ram_history, ram_pct);

        self.weather_c = walk(&mut self.rng, self.weather_c, 0.1, 12.0, 16.0);
        self.wind_ms = walk(&mut self.rng, self.wind_ms, 0.3, 1.0, 7.0);
        if self.rng.below(20) == 0 {
            self.wind_dir = (self.wind_dir + 1 + self.rng.below(2) * 6) % WIND_DIRS.len();
        }
        self.btc *= 1.0 + self.rng.range(-0.0015, 0.0015) as f64;
        self.eth *= 1.0 + self.rng.range(-0.002, 0.002) as f64;
        self.sol *= 1.0 + self.rng.range(-0.003, 0.003) as f64;
        self.latency_ms = walk(&mut self.rng, self.latency_ms, 1.5, 9.0, 16.0);
        self.uptime_s += 1;
        self.now = SystemTime::now();
    }

    pub fn ram_pct(&self) -> f32 {
        pct(self.ram_used, self.ram_total)
    }

    pub fn wind_dir(&self) -> &'static str {
        WIND_DIRS[self.wind_dir]
    }
}

fn walk(rng: &mut Rng, v: f32, step: f32, lo: f32, hi: f32) -> f32 {
    (v + rng.range(-step, step)).clamp(lo, hi)
}

fn push_capped(history: &mut VecDeque<f32>, v: f32) {
    if history.len() == HISTORY_LEN {
        history.pop_front();
    }
    history.push_back(v);
}

pub fn pct(used: u64, total: u64) -> f32 {
    if total == 0 {
        0.0
    } else {
        (used as f64 * 100.0 / total as f64) as f32
    }
}

pub fn fmt_bytes(n: u64, decimal: bool) -> String {
    let (base, units) = if decimal {
        (1000.0, ["B", "KB", "MB", "GB", "TB"])
    } else {
        (1024.0, ["B", "KiB", "MiB", "GiB", "TiB"])
    };
    let mut v = n as f64;
    let mut i = 0;
    while v >= base && i < units.len() - 1 {
        v /= base;
        i += 1;
    }
    format!("{v:.1} {}", units[i])
}

pub fn fmt_temp(c: f32, fahrenheit: bool) -> String {
    if fahrenheit {
        format!("{:.1} °F", c * 9.0 / 5.0 + 32.0)
    } else {
        format!("{c:.1} °C")
    }
}

pub fn fmt_money(v: f64) -> String {
    if v < 1000.0 {
        return format!("${v:.2}");
    }
    let digits = (v.round() as u64).to_string();
    let mut out = String::from("$");
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn fmt_uptime(secs: u64) -> String {
    format!(
        "{}d {:02}:{:02}",
        secs / 86_400,
        secs % 86_400 / 3_600,
        secs % 3_600 / 60
    )
}

pub fn fmt_hms(t: SystemTime) -> String {
    let secs = unix_secs(t);
    format!(
        "{:02}:{:02}:{:02}",
        secs % 86_400 / 3_600,
        secs % 3_600 / 60,
        secs % 60
    )
}

pub fn fmt_date(t: SystemTime) -> String {
    let (y, m, d) = civil_from_days((unix_secs(t) / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}")
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Days-to-civil conversion by Howard Hinnant, so no date crate is needed.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        assert_eq!(fmt_date(UNIX_EPOCH), "1970-01-01");
        let t = UNIX_EPOCH + Duration::from_secs(951_868_800 + 3_661);
        assert_eq!(fmt_date(t), "2000-03-01");
        assert_eq!(fmt_hms(t), "01:01:01");
    }

    #[test]
    fn units() {
        assert_eq!(fmt_bytes(gib(31.9), false), "31.9 GiB");
        assert_eq!(fmt_bytes(gib(31.9), true), "34.3 GB");
        assert_eq!(fmt_temp(55.0, true), "131.0 °F");
        assert_eq!(fmt_money(64_250.4), "$64,250");
        assert_eq!(fmt_money(148.3), "$148.30");
        assert_eq!(fmt_uptime(3 * 86_400 + 4 * 3_600 + 12 * 60), "3d 04:12");
    }

    #[test]
    fn cpu_stays_in_range_and_spikes() {
        let mut d = FakeData::default();
        let now = Instant::now();
        for _ in 0..500 {
            d.tick(now);
            assert!((5.0..=40.0).contains(&d.cpu));
        }
        d.spike(now);
        d.tick(now);
        assert!(d.cpu >= 93.0);
        d.tick(now + SPIKE);
        assert!(d.cpu <= 40.0);
    }
}
