use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{BytesUnit, TempUnit};

const SPARK: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

/// Binary units (GiB) or decimal units (GB), one decimal place.
pub fn bytes(n: u64, unit: BytesUnit) -> String {
    let (base, names) = match unit {
        BytesUnit::Decimal => (1000.0, ["B", "KB", "MB", "GB", "TB", "PB"]),
        BytesUnit::Binary => (1024.0, ["B", "KiB", "MiB", "GiB", "TiB", "PiB"]),
    };
    let mut v = n as f64;
    let mut i = 0;
    while v >= base && i < names.len() - 1 {
        v /= base;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", names[i])
    }
}

pub fn pct(v: f32) -> String {
    format!("{v:.1} %")
}

pub fn temperature(c: f32, unit: TempUnit) -> String {
    match unit {
        TempUnit::Celsius => format!("{c:.1} °C"),
        TempUnit::Fahrenheit => format!("{:.1} °F", c * 9.0 / 5.0 + 32.0),
    }
}

/// The last `width` percentages (0..100) as block glyphs, right-aligned.
pub fn sparkline(values: &[f32], width: usize) -> String {
    let skip = values.len().saturating_sub(width);
    let glyphs: String = values[skip..]
        .iter()
        .map(|v| SPARK[((v / 100.0).clamp(0.0, 1.0) * 7.0).round() as usize])
        .collect();
    format!("{glyphs:>width$}")
}

fn unix_secs(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
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
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

/// The inverse of `civil_from_days`.
fn days_from_civil(y: i64, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * i64::from((m + 9) % 12) + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `268:20:00:00`: the UTC day of the year and the time, as mission clocks show it.
pub fn utc_day_clock(t: SystemTime) -> String {
    let days = (unix_secs(t) / 86_400) as i64;
    let (y, _, _) = civil_from_days(days);
    let doy = days - days_from_civil(y, 1, 1) + 1;
    format!("{doy:03}:{}", utc_hms(t))
}

/// `012:04:31:07`: days, hours, minutes and seconds.
pub fn elapsed_clock(secs: u64) -> String {
    format!(
        "{:03}:{:02}:{:02}:{:02}",
        secs / 86_400,
        secs % 86_400 / 3_600,
        secs % 3_600 / 60,
        secs % 60
    )
}

/// `20:00:00`, UTC.
pub fn utc_hms(t: SystemTime) -> String {
    let s = unix_secs(t);
    format!(
        "{:02}:{:02}:{:02}",
        s % 86_400 / 3_600,
        s % 3_600 / 60,
        s % 60
    )
}

/// `2026-09-25T20:00:00Z`.
pub fn utc_timestamp(t: SystemTime) -> String {
    let s = unix_secs(t);
    let (y, m, d) = civil_from_days((s / 86_400) as i64);
    format!("{y:04}-{m:02}-{d:02}T{}Z", utc_hms(t))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn timestamps() {
        assert_eq!(utc_timestamp(UNIX_EPOCH), "1970-01-01T00:00:00Z");
        let t = UNIX_EPOCH + Duration::from_secs(951_868_800 + 3_661);
        assert_eq!(utc_timestamp(t), "2000-03-01T01:01:01Z");
        let t = UNIX_EPOCH + Duration::from_secs(1_790_366_400);
        assert_eq!(utc_timestamp(t), "2026-09-25T20:00:00Z");
        assert_eq!(utc_hms(t), "20:00:00");
        assert_eq!(utc_day_clock(t), "268:20:00:00");
        assert_eq!(utc_day_clock(UNIX_EPOCH), "001:00:00:00");
        let leap_end = UNIX_EPOCH + Duration::from_secs(1_095 * 86_400);
        assert_eq!(utc_timestamp(leap_end), "1972-12-31T00:00:00Z");
        assert_eq!(utc_day_clock(leap_end), "366:00:00:00");
        assert_eq!(elapsed_clock(93_784), "001:02:03:04");
    }

    #[test]
    fn byte_units() {
        let gib = 1u64 << 30;
        assert_eq!(bytes(512, BytesUnit::Binary), "512 B");
        assert_eq!(bytes(gib * 319 / 10, BytesUnit::Binary), "31.9 GiB");
        assert_eq!(bytes(gib * 319 / 10, BytesUnit::Decimal), "34.3 GB");
        assert_eq!(bytes(2 * gib * 1024, BytesUnit::Binary), "2.0 TiB");
        assert_eq!(bytes(1_500_000, BytesUnit::Decimal), "1.5 MB");
    }

    #[test]
    fn percent_and_temperature() {
        assert_eq!(pct(12.34), "12.3 %");
        assert_eq!(temperature(55.0, TempUnit::Celsius), "55.0 °C");
        assert_eq!(temperature(55.0, TempUnit::Fahrenheit), "131.0 °F");
        assert_eq!(temperature(-40.0, TempUnit::Fahrenheit), "-40.0 °F");
    }

    #[test]
    fn sparkline_glyphs_and_width() {
        assert_eq!(sparkline(&[0.0, 50.0, 100.0], 3), "▁▅█");
        assert_eq!(sparkline(&[0.0, 100.0], 4), "  ▁█");
        assert_eq!(sparkline(&[10.0, 0.0, 100.0, 250.0], 2), "██");
        assert_eq!(sparkline(&[], 0), "");
    }
}
