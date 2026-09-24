use std::time::{SystemTime, UNIX_EPOCH};

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
    }
}
