//! The status contract shared by the daemon, the CLI, the GUI and the GNOME
//! Shell extension.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::config::Config;
use crate::minerapi::{self, MinerStats};
use crate::nvidia::{self, GpuStats};
use crate::pool::{self, PoolStats};
use crate::state::State;
use crate::util;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct StateView {
    pub running: bool,
    pub uptime_s: u64,
    pub restarts: u32,
    pub last_error: Option<String>,
    /// The miner was started outside of our control (`start-mining.sh`).
    pub external: bool,
    pub profile: String,
}

fn default_temp_limit() -> u32 {
    90
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Status {
    pub state: StateView,
    pub gpu: Option<GpuStats>,
    /// Why the GPU section is empty (driver gone, sandbox, no NVIDIA card).
    pub gpu_error: Option<String>,
    pub miner: MinerStats,
    pub pool: Option<PoolStats>,
    /// Non-fatal configuration problems.
    pub warnings: Vec<String>,
    /// Dev fee (miner) + pool fee, percent.
    pub total_fee_pct: f64,
    pub expected_hashrate: f64,
    /// Порог температуры из конфига: по нему шкала и предупреждения в UI.
    #[serde(default = "default_temp_limit")]
    pub temp_limit: u32,
    pub ts: u64,
    pub ts_iso: String,
    pub version: String,
}

impl Status {
    /// Build a status snapshot. Blocking: calls `nvidia-smi` and the HTTP APIs.
    pub fn collect(cfg: &Config, state: &State) -> Self {
        let (gpu, gpu_error) = match nvidia::query() {
            Ok(mut stats) => {
                if let Some((min, max)) = nvidia::power_range() {
                    stats.power_min_w = Some(min);
                    stats.power_max_w = Some(max);
                }
                (Some(stats), None)
            }
            Err(err) => (None, Some(err.to_string())),
        };

        let miner = if state.running {
            minerapi::query_api(cfg.api_port).unwrap_or_else(|_| {
                let tail = read_tail(&crate::paths::log_path(), 96 * 1024);
                let mut stats = minerapi::parse_log(&tail);
                if stats.is_empty() {
                    stats.source = "нет данных".to_string();
                }
                stats
            })
        } else {
            MinerStats::default()
        };

        let pool = if state.running || state.external {
            pool::query_cached(cfg)
        } else {
            None
        };

        let ts = util::now_unix();
        Self {
            state: StateView {
                running: state.running || state.external,
                uptime_s: state.uptime_s(),
                restarts: state.restarts,
                last_error: state.last_error.clone(),
                external: state.external,
                profile: state
                    .profile
                    .clone()
                    .unwrap_or_else(|| cfg.miner_profile.clone()),
            },
            gpu,
            gpu_error,
            miner,
            pool,
            warnings: cfg.warnings(),
            total_fee_pct: cfg.total_fee_pct(),
            expected_hashrate: cfg.expected_hashrate,
            temp_limit: cfg.gpu.temp_limit,
            ts,
            ts_iso: util::iso8601_utc(ts),
            version: crate::VERSION.to_string(),
        }
    }

    /// Cheap snapshot without touching the network (used on stop).
    pub fn stopped(cfg: &Config) -> Self {
        let ts = util::now_unix();
        Self {
            state: StateView {
                running: false,
                profile: cfg.miner_profile.clone(),
                ..Default::default()
            },
            total_fee_pct: cfg.total_fee_pct(),
            expected_hashrate: cfg.expected_hashrate,
            temp_limit: cfg.gpu.temp_limit,
            ts,
            ts_iso: util::iso8601_utc(ts),
            version: crate::VERSION.to_string(),
            ..Default::default()
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(text: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(text)?)
    }

    /// Share acceptance ratio in percent (accepted / all shares).
    pub fn acceptance_pct(&self) -> Option<f64> {
        let accepted = self.miner.accepted? as f64;
        let rejected = self.miner.rejected.unwrap_or(0) as f64;
        let total = accepted + rejected;
        if total <= 0.0 {
            return None;
        }
        Some(accepted / total * 100.0)
    }
}

/// Read at most `max_bytes` from the end of a file (log fallback).
pub fn read_tail(path: &Path, max_bytes: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return String::new(),
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes as u64);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buffer = Vec::new();
    if file.read_to_end(&mut buffer).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Status {
        let cfg = Config::default();
        let state = State {
            running: true,
            miner_pid: Some(1234),
            started_at: Some(1_700_000_000),
            restarts: 1,
            last_error: None,
            external: false,
            profile: Some("wildrig".into()),
        };
        let mut status = Status::stopped(&cfg);
        status.state = StateView {
            running: true,
            uptime_s: state.uptime_s(),
            restarts: state.restarts,
            last_error: None,
            external: false,
            profile: "wildrig".into(),
        };
        status.miner = MinerStats {
            hashrate_10s: Some(125.1e12),
            hashrate_60s: Some(125.0e12),
            hashrate_15m: Some(124.2e12),
            accepted: Some(512),
            rejected: Some(2),
            stale: Some(0),
            source: "api".into(),
        };
        status
    }

    #[test]
    fn json_contract_round_trip() {
        let status = sample();
        let json = status.to_json();
        let back = Status::from_json(&json).unwrap();
        assert_eq!(status, back);
        // The GNU/Linux extension and GUI rely on these key names.
        for key in [
            "\"state\"",
            "\"gpu\"",
            "\"miner\"",
            "\"pool\"",
            "\"hashrate_10s\"",
            "\"uptime_s\"",
            "\"ts_iso\"",
            "\"total_fee_pct\"",
            "\"temp_limit\"",
        ] {
            assert!(json.contains(key), "missing key {key} in {json}");
        }
    }

    #[test]
    fn acceptance_ratio() {
        assert_eq!(sample().acceptance_pct(), Some(512.0 / 514.0 * 100.0));
        let mut status = sample();
        status.miner.accepted = None;
        assert_eq!(status.acceptance_pct(), None);
    }

    #[test]
    fn stopped_status_has_no_metrics() {
        let status = Status::stopped(&Config::default());
        assert!(!status.state.running);
        assert!(status.gpu.is_none());
        assert!(status.miner.is_empty());
        assert!(status.pool.is_none());
    }

    #[test]
    fn tail_of_missing_file_is_empty() {
        assert!(read_tail(Path::new("/nonexistent/file.log"), 1024).is_empty());
    }
}
