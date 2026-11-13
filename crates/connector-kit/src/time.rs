//! UTC time without a date library: unix seconds <-> RFC 3339.

use std::time::{SystemTime, UNIX_EPOCH};

pub fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

// Howard Hinnant's civil-date algorithms.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn civil_from_days(z: i64) -> (i64, i64, i64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// Parses `YYYY-MM-DDTHH:MM:SS[.frac](Z|±HH:MM)` to unix seconds.
pub fn parse_rfc3339(s: &str) -> Option<i64> {
    let s = s.trim();
    let b = s.as_bytes();
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || !matches!(b[10], b'T' | b't' | b' ')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let n = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d, h, mi, se) = (
        n(0..4)?,
        n(5..7)?,
        n(8..10)?,
        n(11..13)?,
        n(14..16)?,
        n(17..19)?,
    );
    if !(1..=12).contains(&mo) || !(1..=31).contains(&d) || h > 23 || mi > 59 || se > 60 {
        return None;
    }
    let mut rest = &s[19..];
    if let Some(r) = rest.strip_prefix('.') {
        let digits = r.chars().take_while(|c| c.is_ascii_digit()).count();
        rest = &r[digits..];
    }
    let offset = match rest {
        "Z" | "z" => 0,
        r if r.len() == 6 && (r.starts_with('+') || r.starts_with('-')) && &r[3..4] == ":" => {
            let sign = if r.starts_with('-') { -1 } else { 1 };
            sign * (r[1..3].parse::<i64>().ok()? * 3600 + r[4..6].parse::<i64>().ok()? * 60)
        }
        _ => return None,
    };
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3600 + mi * 60 + se - offset)
}

/// Formats unix seconds as `YYYY-MM-DDTHH:MM:SSZ`.
pub fn format_rfc3339(t: i64) -> String {
    let (days, secs) = (t.div_euclid(86_400), t.rem_euclid(86_400));
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

/// `YYYY-MM-DD` for unix seconds.
pub fn format_date(t: i64) -> String {
    format_rfc3339(t)[..10].to_owned()
}

/// Day of week, 0 = Monday … 6 = Sunday, and hour (UTC).
pub fn weekday_hour(t: i64) -> (u8, u8) {
    let days = t.div_euclid(86_400);
    // 1970-01-01 was a Thursday (index 3).
    (
        ((days + 3).rem_euclid(7)) as u8,
        (t.rem_euclid(86_400) / 3600) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips() {
        for t in [0, 951_782_400, 1_700_000_000, 1_782_000_123] {
            assert_eq!(parse_rfc3339(&format_rfc3339(t)), Some(t));
        }
        assert_eq!(parse_rfc3339("2009-10-25T06:57:33Z"), Some(1_256_453_853));
        assert_eq!(
            parse_rfc3339("2009-10-25T06:57:33.123Z"),
            Some(1_256_453_853)
        );
        assert_eq!(
            parse_rfc3339("2009-10-25T12:27:33+05:30"),
            Some(1_256_453_853)
        );
        assert_eq!(parse_rfc3339("2009-10-25"), None);
        assert_eq!(format_date(1_256_453_853), "2009-10-25");
        assert_eq!(weekday_hour(1_256_453_853), (6, 6)); // Sunday 06:xx
    }
}
