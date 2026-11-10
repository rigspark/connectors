//! Text helpers for model-facing output.

/// Default cap on any single free-text field handed to the model.
pub const MAX_TEXT: usize = 2_000;

/// Trims `s` to at most `max` characters (not bytes), appending `…` when cut.
pub fn trim(s: &str, max: usize) -> String {
    let s = s.trim();
    if s.chars().count() <= max {
        return s.to_owned();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Parses an ISO 8601 duration such as `PT1H2M3S` or `P1DT4M` into seconds.
/// Returns `None` for anything it doesn't understand (years and months are rejected).
pub fn iso8601_duration_secs(s: &str) -> Option<u64> {
    let rest = s.strip_prefix('P')?;
    let (date, time) = match rest.split_once('T') {
        Some((d, t)) => (d, t),
        None => (rest, ""),
    };
    let mut total: u64 = 0;
    let mut parse = |part: &str, units: &[(char, u64)]| -> Option<()> {
        let mut num = String::new();
        for c in part.chars() {
            if c.is_ascii_digit() {
                num.push(c);
            } else {
                let mul = units.iter().find(|(u, _)| *u == c)?.1;
                let n: u64 = num.parse().ok()?;
                total = total.checked_add(n.checked_mul(mul)?)?;
                num.clear();
            }
        }
        num.is_empty().then_some(())
    };
    parse(date, &[('W', 604_800), ('D', 86_400)])?;
    parse(time, &[('H', 3_600), ('M', 60), ('S', 1)])?;
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_by_chars() {
        assert_eq!(trim("héllo", 10), "héllo");
        assert_eq!(trim("héllo world", 4), "hél…");
    }

    #[test]
    fn durations() {
        assert_eq!(iso8601_duration_secs("PT1H2M3S"), Some(3723));
        assert_eq!(iso8601_duration_secs("PT45S"), Some(45));
        assert_eq!(iso8601_duration_secs("P1DT1M"), Some(86_460));
        assert_eq!(iso8601_duration_secs("P0D"), Some(0));
        assert_eq!(iso8601_duration_secs("P1Y"), None);
        assert_eq!(iso8601_duration_secs("garbage"), None);
        assert_eq!(iso8601_duration_secs("PT5"), None);
    }
}
