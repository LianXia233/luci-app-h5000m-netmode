//! Monotonic clock in centiseconds, and the wall-clock unix timestamp.
//!
//! The legacy backend measures switch phases and the total switch budget on
//! `/proc/uptime` (centisecond resolution) so that an NTP step or a manual
//! `date -s` can never shorten or stretch a budget. `/proc/uptime` is a
//! kernel-maintained monotonic counter; it is also the fastest clock available
//! on router SoCs (no vDSO dependency).

use std::fs;

/// Path of the uptime file; a test seam, like the shell's `H5000M_UPTIME_FILE`.
pub fn uptime_path() -> &'static str {
    "/proc/uptime"
}

/// Monotonic time in centiseconds. Falls back to the wall clock (times 100)
/// when /proc/uptime is unavailable, exactly like `now_cs()` in the shell.
pub fn now_cs() -> u64 {
    if let Ok(text) = fs::read_to_string(uptime_path()) {
        if let Some(up) = text.split_whitespace().next() {
            if let Some(v) = parse_uptime(up) {
                return v;
            }
        }
    }
    // Wall-clock fallback.
    unix_ts() * 100
}

/// Parse `NNNN.NN` (the first field of /proc/uptime) into centiseconds without
/// any floating point and without octal pitfalls: `08` etc. are handled as
/// decimal strings, so a leading zero can never abort the parse.
fn parse_uptime(up: &str) -> Option<u64> {
    let (whole, frac) = match up.split_once('.') {
        Some((w, f)) => (w, f),
        None => (up, ""),
    };
    let whole: u64 = whole.parse().ok()?;
    let mut frac_digits: u64 = 0;
    if !frac.is_empty() {
        let mut digits: String = frac.chars().take(2).collect();
        while digits.len() < 2 {
            digits.push('0');
        }
        frac_digits = digits.parse().unwrap_or(0);
    }
    Some(whole * 100 + frac_digits)
}

/// Wall-clock unix seconds (`date +%s` equivalent), 0 on failure.
pub fn unix_ts() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// `fmt_cs`: centiseconds -> "0.42s" style human string.
pub fn fmt_cs(cs: u64) -> String {
    format!("{}.{:02}s", cs / 100, cs % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_uptime_plain() {
        assert_eq!(parse_uptime("123.45"), Some(12345));
        assert_eq!(parse_uptime("0.08"), Some(8)); // leading-zero fraction
        assert_eq!(parse_uptime("10.5"), Some(1050));
        assert_eq!(parse_uptime("99"), Some(9900));
    }

    #[test]
    fn fmt_cs_rounds() {
        assert_eq!(fmt_cs(42), "0.42s");
        assert_eq!(fmt_cs(0), "0.00s");
        assert_eq!(fmt_cs(15000), "150.00s");
    }
}
