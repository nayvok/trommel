//! Daemon-owned, local-only history: one JSONL sample per minute per rig.
//!
//! Daily UTC files (`history-YYYYMMDD.jsonl`) keep append cheap, make retention
//! a directory prune, and let a damaged line be skipped without a database.
//! The file never contains a wallet, worker, pool URL or anything network-bound.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::write_atomic;
use crate::status::Status;
use crate::util;

pub const VERSION: u32 = 1;
pub const DEFAULT_INTERVAL_S: u64 = 60;
/// Keep a week plus one day of margin for the 7-day query.
pub const RETENTION_DAYS: u64 = 8;
pub const MAX_RANGE_S: u64 = 8 * 86_400;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HistoryGpu {
    #[serde(default)]
    pub uuid: String,
    #[serde(default)]
    pub index: u32,
    #[serde(default)]
    pub bus_id: Option<u32>,
    pub hashrate: Option<f64>,
    pub power_w: Option<f64>,
    pub temp_c: Option<u32>,
    pub fan_pct: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct HistorySample {
    #[serde(default = "version")]
    pub v: u32,
    pub ts: u64,
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub external: bool,
    #[serde(default)]
    pub preset: String,
    #[serde(default)]
    pub miner: String,
    pub total_hashrate: Option<f64>,
    pub power_w: Option<f64>,
    pub mining_power_w: Option<f64>,
    pub accepted: Option<u64>,
    pub rejected: Option<u64>,
    pub stale: Option<u64>,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub gpus: Vec<HistoryGpu>,
}

fn version() -> u32 {
    VERSION
}

fn date_key(ts: u64) -> u32 {
    let iso = util::iso8601_utc(ts);
    format!("{}{}{}", &iso[0..4], &iso[5..7], &iso[8..10])
        .parse()
        .unwrap_or(0)
}

fn history_path(dir: &Path, ts: u64) -> PathBuf {
    dir.join(format!("history-{:08}.jsonl", date_key(ts)))
}

/// Build the local sample from the display status. Only mapped per-card
/// records are stored; an unmapped miner record has no stable card identity.
pub fn sample_from_status(status: &Status) -> HistorySample {
    let gpus = status
        .gpus
        .iter()
        .map(|gpu| {
            let record = status.miner.gpus.iter().find(|record| {
                record.uuid.as_deref() == Some(gpu.uuid.as_str())
                    || (record.uuid.is_none() && record.index == Some(gpu.index))
            });
            HistoryGpu {
                uuid: gpu.uuid.clone(),
                index: gpu.index,
                bus_id: record.and_then(|record| record.bus_id),
                hashrate: record.and_then(|record| {
                    record
                        .hashrate_10s
                        .or(record.hashrate_60s)
                        .or(record.hashrate_15m)
                }),
                power_w: gpu.power_w,
                temp_c: gpu.temp_c,
                fan_pct: gpu.fan_pct,
            }
        })
        .collect();
    HistorySample {
        v: VERSION,
        ts: status.ts,
        running: status.state.running,
        external: status.state.external,
        preset: status.preset.clone().unwrap_or_default(),
        miner: status.miner_info.id.clone(),
        total_hashrate: status.total.hashrate.or_else(|| status.miner.primary()),
        power_w: status.total.power_w,
        mining_power_w: status.total.mining_power_w,
        accepted: status.miner.accepted,
        rejected: status.miner.rejected,
        stale: status.miner.stale,
        source: status.miner.source.clone(),
        gpus,
    }
}

/// Append-only writer owned by the daemon status loop.
pub struct Writer {
    dir: PathBuf,
    interval_s: u64,
    last_ts: Option<u64>,
    last_bucket: Option<u64>,
    last_prune_day: Option<u32>,
}

impl Writer {
    pub fn new(dir: PathBuf, interval_s: u64) -> Self {
        let interval_s = interval_s.clamp(1, 3600);
        let last_ts = read_last_ts(&dir);
        Self {
            dir,
            interval_s,
            last_ts,
            last_bucket: last_ts.map(|ts| ts / interval_s),
            last_prune_day: None,
        }
    }

    pub fn interval_s(&self) -> u64 {
        self.interval_s
    }

    /// Store at most one sample per interval. A repeated status timestamp, a
    /// clock that stepped backwards or a second sample in the same minute is
    /// skipped; the file keeps the first value of the bucket.
    pub fn record(&mut self, status: &Status) -> anyhow::Result<bool> {
        if status.ts == 0 {
            return Ok(false);
        }
        if self.last_ts.is_some_and(|last| status.ts <= last) {
            return Ok(false);
        }
        let bucket = status.ts / self.interval_s;
        if self.last_bucket == Some(bucket) {
            return Ok(false);
        }
        std::fs::create_dir_all(&self.dir)?;
        let sample = sample_from_status(status);
        let path = history_path(&self.dir, status.ts);
        append_line(&path, &serde_json::to_vec(&sample)?)?;
        self.last_ts = Some(status.ts);
        self.last_bucket = Some(bucket);
        let today = date_key(util::now_unix());
        if self.last_prune_day != Some(today) {
            self.prune(util::now_unix());
            self.last_prune_day = Some(today);
        }
        Ok(true)
    }

    /// Remove daily files older than [`RETENTION_DAYS`]. Missing/unreadable
    /// directories are ignored; this runs once per day.
    pub fn prune(&self, now: u64) {
        let cutoff = date_key(now.saturating_sub(RETENTION_DAYS * 86_400));
        let Ok(entries) = std::fs::read_dir(&self.dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(key) = name
                .strip_prefix("history-")
                .and_then(|rest| rest.strip_suffix(".jsonl"))
                .and_then(|key| key.parse::<u32>().ok())
            else {
                continue;
            };
            if key < cutoff {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

fn append_line(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    // A crash during the previous append can leave a partial line. Starting a
    // new line keeps the next valid sample parseable instead of merging them.
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    if len > 0 {
        let mut last = [0u8; 1];
        let mut read = std::fs::File::open(path)?;
        if read.seek_to_end(&mut last)? == 1 && last[0] != b'\n' {
            file.write_all(b"\n")?;
        }
    }
    file.write_all(bytes)?;
    file.write_all(b"\n")?;
    file.flush()?;
    Ok(())
}

trait SeekToEnd {
    fn seek_to_end(&mut self, last: &mut [u8; 1]) -> std::io::Result<usize>;
}

impl SeekToEnd for std::fs::File {
    fn seek_to_end(&mut self, last: &mut [u8; 1]) -> std::io::Result<usize> {
        use std::io::{Seek, SeekFrom};
        let len = self.metadata()?.len();
        if len == 0 {
            return Ok(0);
        }
        self.seek(SeekFrom::Start(len - 1))?;
        self.read(last)
    }
}

fn read_last_ts(dir: &Path) -> Option<u64> {
    let now = util::now_unix();
    for ts in [now, now.saturating_sub(86_400)] {
        let path = history_path(dir, ts);
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let last = text
            .lines()
            .rev()
            .filter_map(|line| serde_json::from_str::<HistorySample>(line).ok())
            .map(|sample| sample.ts)
            .max();
        if last.is_some() {
            return last;
        }
    }
    None
}

/// Read the requested range, skip corrupt lines and deduplicate by timestamp
/// (the last valid line wins). Results are sorted by `ts`.
pub fn load_range(dir: &Path, since: u64, until: u64) -> Vec<HistorySample> {
    if until < since {
        return Vec::new();
    }
    let mut by_ts: BTreeMap<u64, HistorySample> = BTreeMap::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if !name.starts_with("history-") || !name.ends_with(".jsonl") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(entry.path()) else {
            continue;
        };
        for line in text.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let Ok(sample) = serde_json::from_str::<HistorySample>(line) else {
                continue;
            };
            if sample.ts < since || sample.ts > until {
                continue;
            }
            by_ts.insert(sample.ts, sample);
        }
    }
    by_ts.into_values().collect()
}

fn csv_escape(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') || value.contains('\r') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

fn csv_value<T: ToString>(value: Option<T>) -> String {
    value.map(|value| value.to_string()).unwrap_or_default()
}

/// Long CSV: one row per GPU sample (one row without GPU fields for a stopped
/// sample). Long format keeps a variable number of cards and a stable header.
pub fn to_csv(samples: &[HistorySample]) -> String {
    let mut out = String::from(
        "ts,ts_iso,preset,total_hashrate_hps,power_w,mining_power_w,accepted,rejected,stale,source,gpu_uuid,gpu_index,gpu_hashrate_hps,gpu_power_w,gpu_temp_c,gpu_fan_pct\n",
    );
    for sample in samples {
        let base = [
            sample.ts.to_string(),
            util::iso8601_utc(sample.ts),
            csv_escape(&sample.preset),
            csv_value(sample.total_hashrate),
            csv_value(sample.power_w),
            csv_value(sample.mining_power_w),
            csv_value(sample.accepted),
            csv_value(sample.rejected),
            csv_value(sample.stale),
            csv_escape(&sample.source),
        ];
        let base = base.join(",");
        if sample.gpus.is_empty() {
            out.push_str(&base);
            out.push_str(",,,,,,\n");
            continue;
        }
        for gpu in &sample.gpus {
            out.push_str(&base);
            out.push(',');
            out.push_str(&csv_escape(&gpu.uuid));
            out.push(',');
            out.push_str(&gpu.index.to_string());
            out.push(',');
            out.push_str(&csv_value(gpu.hashrate));
            out.push(',');
            out.push_str(&csv_value(gpu.power_w));
            out.push(',');
            out.push_str(&csv_value(gpu.temp_c));
            out.push(',');
            out.push_str(&csv_value(gpu.fan_pct));
            out.push('\n');
        }
    }
    out
}

/// Format a daily file atomically; used by tests and future compaction.
pub fn write_file_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    write_atomic(path, bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "trommel-history-{tag}-{}-{}",
            std::process::id(),
            util::now_unix()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[allow(clippy::field_reassign_with_default)]
    fn status(ts: u64, total: f64) -> Status {
        let mut status = Status::default();
        status.ts = ts;
        status.state.running = true;
        status.preset = Some("pearl".into());
        status.miner_info.id = "wildrig".into();
        status.miner.source = "api".into();
        status.total.hashrate = Some(total);
        status.miner.hashrate_10s = Some(total);
        status.gpus = vec![crate::nvidia::GpuStats {
            index: 0,
            uuid: "GPU-0".into(),
            power_w: Some(300.0),
            temp_c: Some(60),
            fan_pct: Some(50),
            ..Default::default()
        }];
        status.miner.gpus = vec![crate::minerapi::GpuMinerStats {
            index: Some(0),
            uuid: Some("GPU-0".into()),
            bus_id: Some(1),
            hashrate_10s: Some(total),
            ..Default::default()
        }];
        status
    }

    #[test]
    fn one_sample_per_bucket_and_no_clock_rewind() {
        let dir = temp_dir("bucket");
        let base = (util::now_unix() / 86_400) * 86_400;
        let mut writer = Writer::new(dir.clone(), 60);
        assert!(writer.record(&status(base, 10.0)).unwrap());
        assert!(!writer.record(&status(base + 1, 11.0)).unwrap());
        assert!(writer.record(&status(base + 60, 12.0)).unwrap());
        assert!(!writer.record(&status(base + 50, 13.0)).unwrap());
        let samples = load_range(&dir, 0, u64::MAX);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[1].total_hashrate, Some(12.0));
        assert_eq!(samples[0].gpus[0].hashrate, Some(10.0));
    }

    #[test]
    fn daily_files_roll_over_and_retention_prunes_old_days() {
        let dir = temp_dir("retention");
        let mut writer = Writer::new(dir.clone(), 60);
        let day = 86_400;
        let base = (util::now_unix() / day) * day;
        writer.record(&status(base, 1.0)).unwrap();
        writer.record(&status(base + 60, 2.0)).unwrap();
        let files = std::fs::read_dir(&dir).unwrap().count();
        assert_eq!(files, 1);
        assert!(load_range(&dir, base, base + 60).len() == 2);
        writer.prune(base + RETENTION_DAYS * day + day);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0);
    }

    #[test]
    fn corrupt_and_duplicate_lines_are_tolerated() {
        let dir = temp_dir("corrupt");
        let path = history_path(&dir, 1_000);
        std::fs::write(
            &path,
            "{\"ts\":1000,\"total_hashrate\":1.0}\nnot json\n{\"ts\":1000,\"total_hashrate\":2.0}\n{\"ts\":1060,\"total_hashrate\":3.0}\n",
        )
        .unwrap();
        let samples = load_range(&dir, 0, 2_000);
        assert_eq!(samples.len(), 2);
        assert_eq!(samples[0].total_hashrate, Some(2.0));
        assert_eq!(samples[1].v, VERSION);
    }

    #[test]
    fn empty_history_and_a_missing_directory_are_empty() {
        let dir = temp_dir("empty");
        assert!(load_range(&dir, 0, 10).is_empty());
        assert!(load_range(Path::new("/nonexistent/trommel-history"), 0, 10).is_empty());
    }

    #[test]
    fn csv_is_long_format_and_escaped() {
        let mut sample = sample_from_status(&status(1_000, 10.0));
        sample.preset = "pearl, \"main\"".into();
        let csv = to_csv(&[sample]);
        assert!(csv.starts_with("ts,ts_iso,preset,total_hashrate_hps"));
        assert!(csv.contains("\"pearl, \"\"main\"\"\""), "{csv}");
        assert!(csv.contains("GPU-0"), "{csv}");
        assert!(csv.contains("1970-01-01T00:16:40Z"), "{csv}");
    }

    #[test]
    fn history_contains_no_wallet_or_pool_url() {
        let sample = sample_from_status(&status(1_000, 10.0));
        let json = serde_json::to_string(&sample).unwrap();
        assert!(!json.contains("wallet"));
        assert!(!json.contains("prl1"));
        assert!(!json.contains("pool."));
    }
}
