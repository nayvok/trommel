//! Small helpers: hashrate formatting/parsing, timestamps, durations.

/// Format a raw hash rate (in hashes/second) for display, e.g. `125.3 TH/s`.
pub fn format_hashrate(hs: f64) -> String {
    if !hs.is_finite() || hs <= 0.0 {
        return "—".to_string();
    }
    const UNITS: [(&str, f64); 6] = [
        ("H/s", 1.0),
        ("kH/s", 1e3),
        ("MH/s", 1e6),
        ("GH/s", 1e9),
        ("TH/s", 1e12),
        ("PH/s", 1e15),
    ];
    let mut idx = 0usize;
    for (i, (_, scale)) in UNITS.iter().enumerate() {
        if hs >= *scale {
            idx = i;
        }
    }
    let value = hs / UNITS[idx].1;
    let decimals = if value >= 100.0 { 1 } else { 2 };
    format!("{value:.decimals$} {}", UNITS[idx].0)
}

/// Format in a coin's preferred unit, falling back to automatic scaling when
/// that unit would be misleading (for example a MH/s coin reporting TH/s).
pub fn format_hashrate_in(hs: f64, unit: &str) -> String {
    if !hs.is_finite() || hs <= 0.0 {
        return "—".to_string();
    }
    let unit = unit.trim();
    if unit.is_empty() || unit.eq_ignore_ascii_case("auto") {
        return format_hashrate(hs);
    }
    let scale = match unit {
        "H/s" => 1.0,
        "kH/s" => 1e3,
        "MH/s" => 1e6,
        "GH/s" => 1e9,
        "TH/s" => 1e12,
        "PH/s" => 1e15,
        _ => return format_hashrate(hs),
    };
    let value = hs / scale;
    if !(0.01..1000.0).contains(&value) {
        return format_hashrate(hs);
    }
    let decimals = if value >= 100.0 { 1 } else { 2 };
    format!("{value:.decimals$} {unit}")
}

/// Compact form used by the panel indicator: `125.3T`, `980G`, `1.2P`.
pub fn format_hashrate_compact(hs: f64) -> String {
    if !hs.is_finite() || hs <= 0.0 {
        return "—".to_string();
    }
    let (value, suffix) = if hs >= 1e15 {
        (hs / 1e15, "P")
    } else if hs >= 1e12 {
        (hs / 1e12, "T")
    } else if hs >= 1e9 {
        (hs / 1e9, "G")
    } else if hs >= 1e6 {
        (hs / 1e6, "M")
    } else if hs >= 1e3 {
        (hs / 1e3, "k")
    } else {
        (hs, "")
    };
    if value >= 100.0 {
        format!("{value:.1}{suffix}")
    } else {
        format!("{value:.2}{suffix}")
    }
}

/// Разбирает число с учётом локали: `127.66`, `127,66` (ru/de) и `1,234`
/// (разделитель тысяч). Минуты и запятые в логах майнеров встречаются оба.
fn normalize_decimal(raw: &str) -> String {
    let commas = raw.matches(',').count();
    let dots = raw.matches('.').count();
    if commas == 0 {
        return raw.to_string();
    }
    if dots > 0 || commas > 1 {
        // `1,234,567` — это разделители тысяч.
        return raw.replace(',', "");
    }
    let (left, right) = raw.split_once(',').unwrap_or((raw, ""));
    let thousands = right.len() == 3
        && !left.is_empty()
        && left.len() <= 3
        && left != "0"
        && left.chars().all(|c| c.is_ascii_digit())
        && right.chars().all(|c| c.is_ascii_digit());
    if thousands {
        format!("{left}{right}")
    } else {
        format!("{left}.{right}")
    }
}

/// Parse `"125.1 TH/s"`, `"1117 H/s"`, `"1.2 GH/s"`, `"125.1T"` (log suffix).
///
/// Returns hashes per second.
pub fn parse_hashrate(text: &str) -> Option<f64> {
    let text = text.trim().trim_matches(|c: char| c == ',' || c == ';');
    // Split into numeric part and the unit suffix.
    let mut num_end = 0usize;
    let bytes = text.as_bytes();
    let mut seen_digit = false;
    for (i, ch) in text.char_indices() {
        let is_num = ch.is_ascii_digit() || ch == '.' || ch == ',';
        let is_sign = (ch == '-' || ch == '+') && !seen_digit && num_end == i;
        if is_num || is_sign {
            if ch.is_ascii_digit() {
                seen_digit = true;
            }
            num_end = i + ch.len_utf8();
        } else {
            break;
        }
    }
    if !seen_digit {
        return None;
    }
    let (num_part, unit_part) = text.split_at(num_end);
    let value: f64 = normalize_decimal(num_part).parse().ok()?;
    let unit = unit_part.trim().to_ascii_uppercase();
    let scale = if unit.is_empty() {
        1.0
    } else if unit.contains('P') {
        1e15
    } else if unit.contains('T') {
        1e12
    } else if unit.contains('G') {
        1e9
    } else if unit.contains('M') {
        1e6
    } else if unit.contains('K') {
        1e3
    } else if unit.starts_with('H') {
        1.0
    } else {
        // Unknown suffix: look at the first letter only.
        match bytes.get(num_end).copied() {
            Some(b'p') | Some(b'P') => 1e15,
            Some(b't') | Some(b'T') => 1e12,
            Some(b'g') | Some(b'G') => 1e9,
            Some(b'm') | Some(b'M') => 1e6,
            Some(b'k') | Some(b'K') => 1e3,
            _ => 1.0,
        }
    };
    Some(value * scale)
}

/// Unix timestamp (seconds) of "now".
pub fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// RFC 3339 / ISO 8601 in UTC (`2026-10-28T00:10:00Z`).
///
/// Implemented locally to avoid a date-time dependency.
pub fn iso8601_utc(unix: u64) -> String {
    let days = (unix / 86_400) as i64;
    let secs_of_day = unix % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z",
        secs_of_day / 3600,
        (secs_of_day % 3600) / 60,
        secs_of_day % 60
    )
}

/// Howard Hinnant's `civil_from_days` algorithm (days since 1970-01-01).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// `"1 days 02:03:04"` style duration for uptime display.
pub fn human_duration(secs: u64) -> String {
    let d = secs / 86_400;
    let h = (secs % 86_400) / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if d > 0 {
        format!("{d} д {h:02}:{m:02}:{s:02}")
    } else if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_hashrate() {
        assert_eq!(format_hashrate(125.1e12), "125.1 TH/s");
        assert_eq!(format_hashrate(1117.0), "1.12 kH/s");
        assert_eq!(format_hashrate(0.0), "—");
        assert_eq!(format_hashrate_compact(125.1e12), "125.1T");
        assert_eq!(format_hashrate_compact(950e9), "950.0G");
    }

    #[test]
    fn formats_in_a_coin_unit_with_a_safe_fallback() {
        assert_eq!(format_hashrate_in(125.1e12, "TH/s"), "125.1 TH/s");
        assert_eq!(format_hashrate_in(500e6, "MH/s"), "500.0 MH/s");
        // A MH/s coin with a TH/s-scale rig must not print 125,000,000 MH/s.
        assert_eq!(format_hashrate_in(125e12, "MH/s"), "125.0 TH/s");
        assert_eq!(format_hashrate_in(500e6, "auto"), "500.0 MH/s");
        assert_eq!(format_hashrate_in(0.0, "TH/s"), "—");
    }

    #[test]
    fn parses_hashrate_comma_decimal() {
        // Логи с локалью, где дробная часть отделяется запятой.
        assert_eq!(parse_hashrate("127,66"), Some(127.66));
        assert_eq!(parse_hashrate("127,6"), Some(127.6));
        // Разделитель тысяч остаётся разделителем.
        assert_eq!(parse_hashrate("1,234"), Some(1234.0));
        assert_eq!(parse_hashrate("1,234,567"), Some(1234567.0));
    }

    #[test]
    fn parses_hashrate() {
        assert_eq!(parse_hashrate("1117 H/s"), Some(1117.0));
        assert_eq!(parse_hashrate("125.1 TH/s"), Some(125.1e12));
        assert_eq!(parse_hashrate("1.2 GH/s"), Some(1.2e9));
        assert_eq!(parse_hashrate("125.1T"), Some(125.1e12));
        assert_eq!(parse_hashrate("42"), Some(42.0));
        assert_eq!(parse_hashrate("—"), None);
        assert_eq!(parse_hashrate(""), None);
    }

    #[test]
    fn iso_time() {
        assert_eq!(iso8601_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso8601_utc(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(iso8601_utc(1_793_000_000), "2026-10-26T07:33:20Z");
    }

    #[test]
    fn durations() {
        assert_eq!(human_duration(45), "00:45");
        assert_eq!(human_duration(3_725), "01:02:05");
        assert_eq!(human_duration(90_061), "1 д 01:01:01");
    }
}
