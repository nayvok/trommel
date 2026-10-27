//! Miner statistics: the local HTTP API (preferred) and a log fallback.
//!
//! Different miners expose different JSON, so the parser is tolerant: it walks
//! the whole document, remembers the key path of every number and picks values
//! by key name (`hashrate`, `10s`, `accepted`, ...).

use serde::{Deserialize, Serialize};

use crate::util;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MinerStats {
    pub hashrate_10s: Option<f64>,
    pub hashrate_60s: Option<f64>,
    pub hashrate_15m: Option<f64>,
    pub accepted: Option<u64>,
    pub rejected: Option<u64>,
    pub stale: Option<u64>,
    /// `api` when the values come from the HTTP API, `log` for the fallback.
    pub source: String,
}

impl MinerStats {
    /// Best available hash rate for compact display.
    pub fn primary(&self) -> Option<f64> {
        self.hashrate_10s
            .or(self.hashrate_60s)
            .or(self.hashrate_15m)
    }

    pub fn is_empty(&self) -> bool {
        self.primary().is_none() && self.accepted.is_none()
    }
}

/// Fetch and parse the miner API at `http://127.0.0.1:<port>/`.
pub fn query_api(port: u16) -> anyhow::Result<MinerStats> {
    if port == 0 {
        anyhow::bail!("порт API не задан");
    }
    let url = format!("http://127.0.0.1:{port}/");
    let response = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_millis(1200))
        .build()
        .get(&url)
        .call()
        .map_err(|e| anyhow::anyhow!("API майнера недоступен: {e}"))?;
    let body = response
        .into_string()
        .map_err(|e| anyhow::anyhow!("не удалось прочитать ответ API: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| anyhow::anyhow!("API вернул не JSON: {e}"))?;
    let mut stats = parse_json(&json);
    stats.source = "api".to_string();
    Ok(stats)
}

/// Parse a miner statistics document.
pub fn parse_json(json: &serde_json::Value) -> MinerStats {
    let mut pairs: Vec<(String, f64)> = Vec::new();
    walk(json, "", &mut pairs);

    let mut stats = MinerStats::default();

    let by = |needles: &[&str], excludes: &[&str]| -> Option<f64> {
        pairs
            .iter()
            .filter(|(path, _)| {
                let lower = path.to_ascii_lowercase();
                needles.iter().all(|n| lower.contains(n))
                    && !excludes.iter().any(|e| lower.contains(e))
            })
            // Prefer the shortest key path: the top-level summary wins over
            // per-GPU details.
            .min_by_key(|(path, _)| path.len())
            .map(|(_, value)| *value)
    };

    stats.hashrate_10s = by(&["hashrate", "10s"], &["per_gpu", "gpu"])
        .or_else(|| by(&["hash", "10s"], &["gpu", "share"]));
    stats.hashrate_60s = by(&["hashrate", "60s"], &["per_gpu", "gpu"])
        .or_else(|| by(&["hash", "60s"], &["gpu", "share"]));
    stats.hashrate_15m = by(&["hashrate", "15m"], &["per_gpu", "gpu"])
        .or_else(|| by(&["hash", "15m"], &["gpu", "share"]));

    if stats.primary().is_none() {
        let overall = by(&["hashrate", "total"], &[])
            .or_else(|| by(&["hashrate"], &["gpu", "share", "max", "min"]))
            .or_else(|| by(&["hash"], &["gpu", "share", "max", "min"]));
        if let Some(value) = overall {
            stats.hashrate_60s = Some(value);
        }
    }

    stats.accepted = by(&["accepted"], &["rejected", "invalid", "stale"])
        .or_else(|| by(&["valid"], &["invalid"]))
        .or_else(|| by(&["shares_good"], &[]))
        .map(|v| v as u64);
    stats.rejected = by(&["rejected"], &["accepted"])
        .or_else(|| by(&["invalid"], &["valid"]))
        .map(|v| v as u64);
    stats.stale = by(&["stale"], &[]).map(|v| v as u64);
    stats
}

/// Flatten every number of a JSON document into `(key path, value)` pairs.
pub fn flatten_numbers(value: &serde_json::Value) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    walk(value, "", &mut out);
    out
}

fn walk(value: &serde_json::Value, path: &str, out: &mut Vec<(String, f64)>) {
    match value {
        serde_json::Value::Object(map) => {
            for (key, child) in map {
                let child_path = if path.is_empty() {
                    key.clone()
                } else {
                    format!("{path}.{key}")
                };
                walk(child, &child_path, out);
            }
        }
        serde_json::Value::Array(items) => {
            for (index, child) in items.iter().enumerate() {
                walk(child, &format!("{path}[{index}]"), out);
            }
        }
        serde_json::Value::Number(number) => {
            if let Some(as_f64) = number.as_f64() {
                out.push((path.to_string(), as_f64));
            }
        }
        serde_json::Value::String(text) => {
            // Some miners report hash rates as strings ("125.1 TH/s").
            if let Some(parsed) = util::parse_hashrate(text) {
                out.push((path.to_string(), parsed));
            }
        }
        _ => {}
    }
}

/// Fallback: scan the tail of the miner log for a hash rate report.
pub fn parse_log(text: &str) -> MinerStats {
    let mut stats = MinerStats {
        source: "log".to_string(),
        ..Default::default()
    };
    // The newest report wins, so scan from the end.
    for line in text.lines().rev() {
        let line = strip_ansi(line);
        let lower = line.to_ascii_lowercase();
        if stats.primary().is_none() && lower.contains("hashrate") {
            let parsed = parse_hashrate_line(&line);
            if parsed.primary().is_some() {
                stats.hashrate_10s = parsed.hashrate_10s;
                stats.hashrate_60s = parsed.hashrate_60s;
                stats.hashrate_15m = parsed.hashrate_15m;
            }
        }
        if stats.accepted.is_none() && lower.contains("accepted") {
            if let Some(value) = number_before(&line, "accepted") {
                stats.accepted = Some(value as u64);
            }
        }
        if stats.rejected.is_none() && lower.contains("rejected") {
            if let Some(value) = number_before(&line, "rejected") {
                stats.rejected = Some(value as u64);
            }
        }
        if stats.stale.is_none() && lower.contains("stale") {
            if let Some(value) = number_before(&line, "stale") {
                stats.stale = Some(value as u64);
            }
        }
        if stats.primary().is_some() && stats.accepted.is_some() {
            break;
        }
    }
    stats
}

/// Parse a line like `hashrate: 10s: 1117 60s: 1117 15m: 1025 H/s`.
fn parse_hashrate_line(line: &str) -> MinerStats {
    let tokens: Vec<&str> = line.split_whitespace().collect();
    let unit_scale = tokens
        .iter()
        .rev()
        .find(|token| token.to_ascii_lowercase().contains("/s"))
        .and_then(|token| util::parse_hashrate(&format!("1 {token}")))
        .unwrap_or(1.0);

    let mut stats = MinerStats {
        source: "log".to_string(),
        ..Default::default()
    };

    let value_after = |needle: &str| -> Option<f64> {
        let position = tokens
            .iter()
            .position(|token| token.eq_ignore_ascii_case(needle))?;
        let next = tokens.get(position + 1)?;
        util::parse_hashrate(next).map(|value| {
            if next.to_ascii_lowercase().contains("/s")
                || next.chars().any(|c| c.is_ascii_alphabetic())
            {
                value // the token carries its own unit
            } else {
                value * unit_scale
            }
        })
    };

    stats.hashrate_10s = value_after("10s:");
    stats.hashrate_60s = value_after("60s:");
    stats.hashrate_15m = value_after("15m:");

    if stats.primary().is_none() {
        // Generic fallback: the first number that carries a unit.
        for token in &tokens {
            if token.to_ascii_lowercase().contains("/s") {
                if let Some(value) = util::parse_hashrate(token) {
                    stats.hashrate_60s = Some(value);
                    break;
                }
            }
        }
    }
    stats
}

/// Largest number directly before `needle` in the same line.
fn number_before(line: &str, needle: &str) -> Option<f64> {
    let tokens: Vec<&str> = line
        .split(|c: char| c.is_whitespace() || c == ',' || c == ':')
        .filter(|token| !token.is_empty())
        .collect();
    let position = tokens
        .iter()
        .position(|token| token.eq_ignore_ascii_case(needle))?;
    let parse = |token: &str| -> Option<f64> {
        let cleaned = token.trim_matches(|c: char| !c.is_ascii_digit() && c != '.');
        cleaned.parse::<f64>().ok()
    };
    for token in tokens[..position].iter().rev() {
        if let Some(value) = parse(token) {
            return Some(value);
        }
    }
    // `accepted: 7` — the number may also follow the word.
    for token in tokens[position + 1..].iter() {
        if let Some(value) = parse(token) {
            return Some(value);
        }
    }
    None
}

fn strip_ansi(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' {
            // Skip CSI sequences until a letter terminator.
            if chars.peek() == Some(&'[') {
                chars.next();
                for next in chars.by_ref() {
                    if next.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wildrig_style_json() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{
                "worker_id": "3080ti",
                "uptime": 3600,
                "hashrate": { "10s": 125.1e12, "60s": 125.0e12, "15m": 124.2e12 },
                "shares": { "accepted": 512, "rejected": 2, "stale": 1 },
                "gpus": [ { "gpu_id": 0, "hashrate": { "10s": 125.1e12 } } ]
            }"#,
        )
        .unwrap();
        let stats = parse_json(&json);
        assert_eq!(stats.hashrate_10s, Some(125.1e12));
        assert_eq!(stats.hashrate_60s, Some(125.0e12));
        assert_eq!(stats.hashrate_15m, Some(124.2e12));
        assert_eq!(stats.accepted, Some(512));
        assert_eq!(stats.rejected, Some(2));
        assert_eq!(stats.stale, Some(1));
    }

    #[test]
    fn parses_flat_json() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{ "total_hashrate": 950e9, "accepted_shares": 12, "rejected_shares": 1 }"#,
        )
        .unwrap();
        let stats = parse_json(&json);
        assert_eq!(stats.hashrate_60s, Some(950e9));
        assert_eq!(stats.accepted, Some(12));
        assert_eq!(stats.rejected, Some(1));
    }

    #[test]
    fn parses_string_hashrate() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{ "hashrate": "125.4 TH/s" }"#).unwrap();
        let stats = parse_json(&json);
        assert_eq!(stats.primary(), Some(125.4e12));
    }

    #[test]
    fn unknown_json_is_empty() {
        let json: serde_json::Value = serde_json::from_str(r#"{ "hello": "world" }"#).unwrap();
        assert!(parse_json(&json).is_empty());
    }

    #[test]
    fn parses_wildrig_log_line() {
        let log = "\
[20:00:44] GPU #0[T:67C F:54%]: 10s: 125.0 60s: 125.0 15m: 124.0 TH/s
[20:00:54] hashrate: 10s: 125.1 60s: 125.0 15m: 124.2 TH/s max: 126.0 TH/s
[20:00:54] Shares: 512 accepted, 2 rejected
";
        let stats = parse_log(log);
        assert_eq!(stats.hashrate_10s, Some(125.1e12));
        assert_eq!(stats.hashrate_60s, Some(125.0e12));
        assert_eq!(stats.hashrate_15m, Some(124.2e12));
        assert_eq!(stats.accepted, Some(512));
        assert_eq!(stats.rejected, Some(2));
        assert_eq!(stats.source, "log");
    }

    #[test]
    fn parses_hashrate_in_hps() {
        let log = "[20:00:54] hashrate: 10s: 1117 60s: 1117 15m: 1025 H/s";
        let stats = parse_log(log);
        assert_eq!(stats.hashrate_10s, Some(1117.0));
        assert_eq!(stats.hashrate_15m, Some(1025.0));
    }

    #[test]
    fn parses_colored_log() {
        let log = "\u{1b}[32m[20:00:54]\u{1b}[0m hashrate: 10s: 1.5 60s: 1.4 15m: 1.3 MH/s";
        let stats = parse_log(log);
        assert_eq!(stats.hashrate_10s, Some(1.5e6));
    }

    #[test]
    fn empty_log_gives_empty_stats() {
        assert!(parse_log("").is_empty());
        assert!(parse_log("miner starting...\n").is_empty());
    }

    #[test]
    fn number_before_needle() {
        assert_eq!(
            number_before("Shares: 512 accepted, 2 rejected", "accepted"),
            Some(512.0)
        );
        assert_eq!(number_before("accepted: 7", "accepted"), Some(7.0));
        assert_eq!(number_before("nothing here", "accepted"), None);
    }
}
