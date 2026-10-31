//! Miner statistics: the local HTTP API (preferred) and a log fallback.
//!
//! Different miners expose different JSON, so the parser is tolerant: it walks
//! the whole document, remembers the key path of every number and picks values
//! by key name (`hashrate`, `10s`, `accepted`, ...).

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::catalog::MinerDriver;
use crate::nvidia::{self, GpuStats};
use crate::util;

/// One miner-side per-card record. `index`/`uuid` are filled only when the
/// record's bus id matches exactly one `nvidia-smi` card: a guessed mapping is
/// worse than `—` on the dashboard.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GpuMinerStats {
    #[serde(default)]
    pub index: Option<u32>,
    #[serde(default)]
    pub uuid: Option<String>,
    /// Miner-side identifier (`gpu0`, `thread0`, `0`), for diagnostics only.
    #[serde(default)]
    pub device: String,
    #[serde(default)]
    pub bus_id: Option<u32>,
    #[serde(default)]
    pub hashrate_10s: Option<f64>,
    #[serde(default)]
    pub hashrate_60s: Option<f64>,
    #[serde(default)]
    pub hashrate_15m: Option<f64>,
    #[serde(default)]
    pub accepted: Option<u64>,
    #[serde(default)]
    pub rejected: Option<u64>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MinerStats {
    pub hashrate_10s: Option<f64>,
    pub hashrate_60s: Option<f64>,
    pub hashrate_15m: Option<f64>,
    pub accepted: Option<u64>,
    pub rejected: Option<u64>,
    pub stale: Option<u64>,
    /// Per-card statistics when the selected miner's API documents them. An
    /// empty list means "the miner does not report per-card data", never a
    /// reason to divide the total by the card count.
    #[serde(default)]
    pub gpus: Vec<GpuMinerStats>,
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

/// Fetch and parse the miner API with the selected driver's documented shape.
pub fn query_api_for(port: u16, driver: MinerDriver) -> anyhow::Result<MinerStats> {
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
    let mut stats = parse_json_for(&json, driver);
    stats.source = "api".to_string();
    Ok(stats)
}

/// Parse a miner statistics document, using the driver's documented per-GPU
/// keys when available, and falling back to the tolerant generic parser.
pub fn parse_json_for(json: &serde_json::Value, driver: MinerDriver) -> MinerStats {
    let mut stats = parse_json(json);
    match driver {
        MinerDriver::Wildrig => apply_wildrig(json, &mut stats),
        MinerDriver::Generic => {}
        MinerDriver::Srbminer => apply_srbminer(json, &mut stats),
        MinerDriver::Onezero => apply_onezero(json, &mut stats),
    }
    stats
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

fn as_number(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => util::parse_hashrate(text),
        _ => None,
    }
}

fn array_numbers(value: &serde_json::Value) -> Vec<f64> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(as_number).collect())
        .unwrap_or_default()
}

fn nested_numbers(value: &serde_json::Value) -> Vec<Vec<f64>> {
    value
        .as_array()
        .map(|items| items.iter().map(array_numbers).collect())
        .unwrap_or_default()
}

fn sum_column(rows: &[Vec<f64>], column: usize) -> Option<f64> {
    let mut sum = 0.0;
    let mut seen = false;
    for row in rows {
        if let Some(value) = row.get(column) {
            sum += value;
            seen = true;
        }
    }
    seen.then_some(sum)
}

fn set_windows(stats: &mut MinerStats, windows: &[f64; 3]) {
    stats.hashrate_10s = windows[0].is_finite().then_some(windows[0]);
    stats.hashrate_60s = windows[1].is_finite().then_some(windows[1]);
    stats.hashrate_15m = windows[2].is_finite().then_some(windows[2]);
}

/// WildRig: `hashrate.total[10s,60s,15m]`, `hashrate.threads[gpu][window]`,
/// `hwmon.busID[gpu]`, `results.shares_accepted/rejected[gpu]`.
///
/// Threads are only attributed when their count matches the bus-id array; a
/// `--gpu-threads` build with more threads than cards must not be guessed.
fn apply_wildrig(json: &serde_json::Value, stats: &mut MinerStats) {
    let total = json
        .pointer("/hashrate/total")
        .map(array_numbers)
        .unwrap_or_default();
    let threads = json
        .pointer("/hashrate/threads")
        .map(nested_numbers)
        .unwrap_or_default();
    let bus_ids = json
        .pointer("/hwmon/busID")
        .map(array_numbers)
        .unwrap_or_default();

    let mut windows = [f64::NAN; 3];
    for (index, value) in total.iter().take(3).enumerate() {
        windows[index] = *value;
    }
    if total.is_empty() {
        for (index, window) in windows.iter_mut().enumerate() {
            if let Some(value) = sum_column(&threads, index) {
                *window = value;
            }
        }
    }
    set_windows(stats, &windows);

    let accepted = json
        .pointer("/results/shares_accepted")
        .map(array_numbers)
        .unwrap_or_default();
    let rejected = json
        .pointer("/results/shares_rejected")
        .map(array_numbers)
        .unwrap_or_default();
    if threads.len() == bus_ids.len() && !threads.is_empty() {
        stats.gpus = threads
            .iter()
            .enumerate()
            .map(|(index, windows)| GpuMinerStats {
                index: None,
                uuid: None,
                device: format!("thread{index}"),
                bus_id: bus_ids.get(index).map(|value| *value as u32),
                hashrate_10s: windows.first().copied(),
                hashrate_60s: windows.get(1).copied(),
                hashrate_15m: windows.get(2).copied(),
                accepted: accepted.get(index).map(|value| *value as u64),
                rejected: rejected.get(index).map(|value| *value as u64),
            })
            .collect();
    }

    if let Some(good) = json
        .pointer("/results/shares_good")
        .and_then(as_number)
        .map(|value| value as u64)
    {
        stats.accepted = Some(good);
    }
    if let (Some(total), Some(good)) = (
        json.pointer("/results/shares_total")
            .and_then(as_number)
            .map(|value| value as u64),
        stats.accepted,
    ) {
        if total >= good {
            stats.rejected = Some(total - good);
        }
    }
}

/// SRBMiner-MULTI 3.x: `algorithms[].hashrate.gpu.gpuN` plus
/// `gpu_devices[].{id,bus_id}`. The GPU map has one current value per card;
/// `10s/60s/15m` stay at the algorithm level.
fn apply_srbminer(json: &serde_json::Value, stats: &mut MinerStats) {
    let Some(algos) = json.get("algorithms").and_then(|value| value.as_array()) else {
        return;
    };
    let algo = algos
        .iter()
        .find(|algo| {
            algo.pointer("/hashrate/gpu")
                .and_then(|value| value.as_object())
                .map(|map| !map.is_empty())
                .unwrap_or(false)
        })
        .or_else(|| algos.first());
    let Some(algo) = algo else { return };
    let devices = json
        .get("gpu_devices")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let mut gpus = Vec::new();
    if let Some(map) = algo
        .pointer("/hashrate/gpu")
        .and_then(|value| value.as_object())
    {
        for (key, value) in map {
            let id = key
                .chars()
                .filter(|ch| ch.is_ascii_digit())
                .collect::<String>()
                .parse::<u32>()
                .ok();
            let device = devices
                .iter()
                .find(|device| {
                    id.is_some()
                        && device.get("id").and_then(|value| value.as_u64()) == id.map(u64::from)
                })
                .or_else(|| id.and_then(|id| devices.get(id as usize)));
            let bus_id = device
                .and_then(|device| device.get("bus_id"))
                .and_then(|value| value.as_u64())
                .map(|value| value as u32);
            let hashrate = as_number(value).or_else(|| {
                value
                    .get("10s")
                    .and_then(as_number)
                    .or_else(|| value.get("now").and_then(as_number))
            });
            gpus.push(GpuMinerStats {
                index: None,
                uuid: None,
                device: key.clone(),
                bus_id,
                hashrate_10s: hashrate,
                hashrate_60s: None,
                hashrate_15m: None,
                accepted: None,
                rejected: None,
            });
        }
    }
    gpus.sort_by(|left, right| left.device.cmp(&right.device));
    if !gpus.is_empty() {
        stats.gpus = gpus;
    }

    let window = |key: &str| {
        algo.pointer(&format!("/hashrate/{key}"))
            .and_then(as_number)
    };
    stats.hashrate_10s = window("10s").or_else(|| window("now")).or_else(|| {
        sum_column(
            &stats
                .gpus
                .iter()
                .map(|gpu| vec![gpu.hashrate_10s.unwrap_or(f64::NAN)])
                .collect::<Vec<_>>(),
            0,
        )
        .filter(|value| value.is_finite())
    });
    stats.hashrate_60s = window("60s");
    stats.hashrate_15m = window("15m");
    if let Some(accepted) = algo.pointer("/shares/accepted").and_then(as_number) {
        stats.accepted = Some(accepted as u64);
    }
    if let Some(rejected) = algo.pointer("/shares/rejected").and_then(as_number) {
        stats.rejected = Some(rejected as u64);
    }
}

/// OneZeroMiner: `devices[].{id,bus_id}` with parallel
/// `algos[0].hashrates[]` and `devices_accepted_shares[]` arrays. The API
/// exposes one averaging window per device (`--hashrate-avg`, 10 s by default).
fn apply_onezero(json: &serde_json::Value, stats: &mut MinerStats) {
    let Some(algos) = json.get("algos").and_then(|value| value.as_array()) else {
        return;
    };
    let algo = algos
        .iter()
        .find(|algo| {
            algo.get("hashrates")
                .and_then(|value| value.as_array())
                .map(|rates| !rates.is_empty())
                .unwrap_or(false)
        })
        .or_else(|| algos.first());
    let Some(algo) = algo else { return };
    let devices = json
        .get("devices")
        .and_then(|value| value.as_array())
        .cloned()
        .unwrap_or_default();
    let rates = algo.get("hashrates").map(array_numbers).unwrap_or_default();
    let accepted = algo
        .get("devices_accepted_shares")
        .map(array_numbers)
        .unwrap_or_default();
    let rejected = algo
        .get("devices_rejected_shares")
        .map(array_numbers)
        .unwrap_or_default();
    let mut gpus = Vec::new();
    for (index, hashrate) in rates.iter().enumerate() {
        let device = devices.get(index);
        let id = device
            .and_then(|device| device.get("id"))
            .and_then(|value| value.as_u64());
        gpus.push(GpuMinerStats {
            index: None,
            uuid: None,
            device: id
                .map(|id| format!("device{id}"))
                .unwrap_or_else(|| format!("device{index}")),
            bus_id: device
                .and_then(|device| device.get("bus_id"))
                .and_then(|value| value.as_u64())
                .map(|value| value as u32),
            hashrate_10s: Some(*hashrate),
            hashrate_60s: None,
            hashrate_15m: None,
            accepted: accepted.get(index).map(|value| *value as u64),
            rejected: rejected.get(index).map(|value| *value as u64),
        });
    }
    if !gpus.is_empty() {
        stats.gpus = gpus;
    }
    let total = algo.get("total_hashrate").and_then(as_number).or_else(|| {
        sum_column(
            &rates.iter().map(|value| vec![*value]).collect::<Vec<_>>(),
            0,
        )
    });
    stats.hashrate_10s = total;
    stats.hashrate_60s = None;
    stats.hashrate_15m = None;
    if let Some(value) = algo.get("total_accepted_shares").and_then(as_number) {
        stats.accepted = Some(value as u64);
    }
    if let Some(value) = algo.get("total_rejected_shares").and_then(as_number) {
        stats.rejected = Some(value as u64);
    }
}

/// What the per-card mapping could and could not prove. The status document
/// turns the non-empty lists into one calm warning instead of blaming a card.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GpuMappingReport {
    pub mapped: usize,
    pub unmapped_devices: Vec<String>,
    pub ambiguous_buses: Vec<u32>,
    pub missing_cards: Vec<u32>,
}

/// Attach miner per-card records to `nvidia-smi` cards by unique PCI bus id.
/// No bus id, a collision or an unknown bus leaves the record unmapped; the
/// total is never split between cards.
pub fn map_to_gpus(
    stats: &mut MinerStats,
    gpus: &[GpuStats],
    participating_uuids: &[String],
) -> GpuMappingReport {
    if stats.gpus.is_empty() || gpus.is_empty() {
        return GpuMappingReport::default();
    }
    let mut by_bus: HashMap<u32, Vec<(u32, String)>> = HashMap::new();
    for gpu in gpus {
        let Some(bus) = gpu.pci_bus_id.as_deref().and_then(nvidia::pci_bus_number) else {
            continue;
        };
        by_bus
            .entry(bus)
            .or_default()
            .push((gpu.index, gpu.uuid.clone()));
    }
    let mut record_buses: HashMap<u32, usize> = HashMap::new();
    for record in &stats.gpus {
        if let Some(bus) = record.bus_id {
            *record_buses.entry(bus).or_default() += 1;
        }
    }

    let mut report = GpuMappingReport::default();
    for record in &mut stats.gpus {
        let Some(bus) = record.bus_id else {
            report.unmapped_devices.push(record.device.clone());
            continue;
        };
        let candidates = by_bus.get(&bus).map(Vec::as_slice).unwrap_or_default();
        let duplicate_record = record_buses.get(&bus).copied().unwrap_or(0) > 1;
        if candidates.len() == 1 && !duplicate_record {
            record.index = Some(candidates[0].0);
            record.uuid = Some(candidates[0].1.clone());
            report.mapped += 1;
        } else if candidates.len() > 1 || duplicate_record {
            if !report.ambiguous_buses.contains(&bus) {
                report.ambiguous_buses.push(bus);
            }
        } else {
            report.unmapped_devices.push(record.device.clone());
        }
    }
    stats.gpus.sort_by_key(|record| {
        (
            record.index.unwrap_or(u32::MAX),
            record.bus_id,
            record.device.clone(),
        )
    });
    for uuid in participating_uuids {
        if !stats
            .gpus
            .iter()
            .any(|record| record.uuid.as_deref() == Some(uuid.as_str()))
        {
            if let Some(gpu) = gpus.iter().find(|gpu| &gpu.uuid == uuid) {
                report.missing_cards.push(gpu.index);
            }
        }
    }
    report
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

    fn gpu(index: u32, bus: &str) -> GpuStats {
        GpuStats {
            index,
            uuid: format!("GPU-{index}"),
            pci_bus_id: Some(bus.into()),
            ..Default::default()
        }
    }

    #[test]
    fn wildrig_real_api_keeps_three_windows_and_per_gpu_threads() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{
                "hashrate": {
                    "total": [125.1e12, 125.0e12, 124.2e12],
                    "threads": [[100.0e12, 99.0e12, 98.0e12], [25.1e12, 26.0e12, 25.0e12]]
                },
                "hwmon": {"busID": [1, 2]},
                "results": {
                    "shares_good": 512, "shares_total": 514,
                    "shares_accepted": [410, 102], "shares_rejected": [1, 1]
                }
            }"#,
        )
        .unwrap();
        let stats = parse_json_for(&json, MinerDriver::Wildrig);
        assert_eq!(stats.hashrate_10s, Some(125.1e12));
        assert_eq!(stats.hashrate_60s, Some(125.0e12));
        assert_eq!(stats.hashrate_15m, Some(124.2e12));
        assert_eq!(stats.accepted, Some(512));
        assert_eq!(stats.rejected, Some(2));
        assert_eq!(stats.gpus.len(), 2);
        assert_eq!(stats.gpus[0].bus_id, Some(1));
        assert_eq!(stats.gpus[0].hashrate_10s, Some(100.0e12));
        assert_eq!(stats.gpus[1].accepted, Some(102));
    }

    #[test]
    fn wildrig_with_more_threads_than_cards_does_not_guess() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{
                "hashrate": {"total": [10.0e12], "threads": [[5.0e12], [5.0e12]]},
                "hwmon": {"busID": [1]}
            }"#,
        )
        .unwrap();
        let stats = parse_json_for(&json, MinerDriver::Wildrig);
        assert!(stats.gpus.is_empty());
        assert_eq!(stats.hashrate_10s, Some(10.0e12));
    }

    #[test]
    fn srbminer_uses_the_gpu_hashrate_map_and_device_bus_ids() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{
                "algorithms": [{
                    "name": "pearlhash",
                    "hashrate": {
                        "10s": 125.0e12, "60s": 124.0e12, "15m": 123.0e12,
                        "gpu": {"gpu0": 100.0e12, "gpu1": 25.0e12}
                    },
                    "shares": {"accepted": 512, "rejected": 2}
                }],
                "gpu_devices": [
                    {"id": 0, "bus_id": 1, "device": "gpu0"},
                    {"id": 1, "bus_id": 2, "device": "gpu1"}
                ]
            }"#,
        )
        .unwrap();
        let stats = parse_json_for(&json, MinerDriver::Srbminer);
        assert_eq!(stats.hashrate_10s, Some(125.0e12));
        assert_eq!(stats.accepted, Some(512));
        assert_eq!(stats.gpus.len(), 2);
        assert_eq!(stats.gpus[0].device, "gpu0");
        assert_eq!(stats.gpus[0].bus_id, Some(1));
        assert_eq!(stats.gpus[0].hashrate_10s, Some(100.0e12));
        assert_eq!(stats.gpus[1].hashrate_10s, Some(25.0e12));
    }

    #[test]
    fn onezero_uses_parallel_device_and_share_arrays() {
        let json: serde_json::Value = serde_json::from_str(
            r#"{
                "devices": [
                    {"id": 0, "bus_id": 1, "name": "3080 Ti"},
                    {"id": 1, "bus_id": 2, "name": "3060"}
                ],
                "algos": [{
                    "name": "Xelis",
                    "hashrates": [4351.0, 1200.0],
                    "total_hashrate": 5551.0,
                    "total_accepted_shares": 12,
                    "total_rejected_shares": 1,
                    "devices_accepted_shares": [10, 2],
                    "devices_rejected_shares": [1, 0]
                }]
            }"#,
        )
        .unwrap();
        let stats = parse_json_for(&json, MinerDriver::Onezero);
        assert_eq!(stats.hashrate_10s, Some(5551.0));
        assert_eq!(stats.hashrate_60s, None);
        assert_eq!(stats.gpus.len(), 2);
        assert_eq!(stats.gpus[0].device, "device0");
        assert_eq!(stats.gpus[0].hashrate_10s, Some(4351.0));
        assert_eq!(stats.gpus[0].accepted, Some(10));
        assert_eq!(stats.gpus[1].rejected, Some(0));
    }

    #[test]
    fn missing_per_gpu_data_is_not_fabricated() {
        let json: serde_json::Value =
            serde_json::from_str(r#"{"hashrate": {"10s": 1.0}}"#).unwrap();
        let stats = parse_json_for(&json, MinerDriver::Srbminer);
        assert!(stats.gpus.is_empty());
    }

    #[test]
    fn maps_per_gpu_records_by_unique_pci_bus() {
        let mut stats = MinerStats {
            gpus: vec![
                GpuMinerStats {
                    device: "gpu0".into(),
                    bus_id: Some(1),
                    ..Default::default()
                },
                GpuMinerStats {
                    device: "gpu1".into(),
                    bus_id: Some(2),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let gpus = [gpu(0, "00000000:01:00.0"), gpu(1, "00000000:02:00.0")];
        let report = map_to_gpus(&mut stats, &gpus, &["GPU-0".into(), "GPU-1".into()]);
        assert_eq!(report.mapped, 2);
        assert_eq!(stats.gpus[0].index, Some(0));
        assert_eq!(stats.gpus[1].uuid.as_deref(), Some("GPU-1"));
        assert!(report.missing_cards.is_empty());
    }

    #[test]
    fn ambiguous_bus_and_missing_records_are_reported_not_guessed() {
        let mut stats = MinerStats {
            gpus: vec![
                GpuMinerStats {
                    device: "gpu0".into(),
                    bus_id: Some(1),
                    ..Default::default()
                },
                GpuMinerStats {
                    device: "gpu1".into(),
                    bus_id: Some(2),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        // Two cards share bus 1: neither record may be attributed.
        let gpus = [
            gpu(0, "00000000:01:00.0"),
            gpu(1, "00000000:01:00.1"),
            gpu(2, "00000000:03:00.0"),
        ];
        let report = map_to_gpus(
            &mut stats,
            &gpus,
            &["GPU-0".into(), "GPU-1".into(), "GPU-2".into()],
        );
        assert_eq!(report.mapped, 0);
        assert!(report.ambiguous_buses.contains(&1));
        assert_eq!(report.missing_cards, vec![0, 1, 2]);
        assert!(stats.gpus.iter().all(|record| record.index.is_none()));
    }

    #[test]
    fn duplicate_miner_bus_is_not_attributed() {
        let mut stats = MinerStats {
            gpus: vec![
                GpuMinerStats {
                    device: "a".into(),
                    bus_id: Some(1),
                    ..Default::default()
                },
                GpuMinerStats {
                    device: "b".into(),
                    bus_id: Some(1),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let gpus = [gpu(0, "00000000:01:00.0")];
        let report = map_to_gpus(&mut stats, &gpus, &[]);
        assert_eq!(report.mapped, 0);
        assert!(report.ambiguous_buses.contains(&1));
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
