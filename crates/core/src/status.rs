//! The status contract shared by the daemon, CLI, GUI and GNOME extension.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::catalog;
use crate::config::Config;
use crate::minerapi::{self, MinerStats};
use crate::nvidia::{self, GpuStats};
use crate::pool::{self, PoolStats};
use crate::state::State;
use crate::util;

pub const CONTRACT_VERSION: u32 = 2;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct StateView {
    pub running: bool,
    pub uptime_s: u64,
    pub restarts: u32,
    pub last_error: Option<String>,
    pub external: bool,
    pub preset_id: String,
    pub preset_name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MinerInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub short_description: String,
    pub license: String,
    pub source_url: String,
    pub verified: bool,
    pub installed: bool,
    pub custom: bool,
    pub miner_fee_pct: f64,
    pub pool_fee_pct: f64,
    pub total_fee_pct: f64,
    pub fee_configurable: bool,
    pub fee_source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TotalView {
    pub hashrate: Option<f64>,
    pub power_w: Option<f64>,
    pub efficiency_j_per_th: Option<f64>,
    pub temp_max_c: Option<u32>,
    pub gpus_enabled: u32,
    pub gpus_total: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OcView {
    pub applied: bool,
    pub snapshot_available: bool,
    pub stale: bool,
    pub boot_id: String,
}

fn default_temp_limit() -> u32 {
    90
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Status {
    pub contract_version: u32,
    pub state: StateView,
    pub preset: Option<String>,
    pub miner_info: MinerInfo,
    pub gpus: Vec<GpuStats>,
    /// First GPU, kept for older clients.
    pub gpu: Option<GpuStats>,
    pub gpu_error: Option<String>,
    pub total: TotalView,
    pub miner: MinerStats,
    pub pool: Option<PoolStats>,
    pub oc: OcView,
    pub warnings: Vec<String>,
    pub total_fee_pct: f64,
    pub expected_hashrate: f64,
    #[serde(default = "default_temp_limit")]
    pub temp_limit: u32,
    pub ts: u64,
    pub ts_iso: String,
    pub version: String,
}

impl Status {
    /// Build a status snapshot. Blocking: calls `nvidia-smi` and HTTP APIs.
    pub fn collect(cfg: &Config, state: &State) -> Self {
        let preset = cfg.preset();
        let gpus = nvidia::query_all();
        let (gpus, gpu_error) = match gpus {
            Ok(list) => (list, None),
            Err(err) => (Vec::new(), Some(err.to_string())),
        };
        let gpu = gpus.first().cloned();

        let miner = if state.running {
            minerapi::query_api(preset.map(|p| p.api_port).unwrap_or(0)).unwrap_or_else(|_| {
                let tail = read_tail(&crate::paths::log_path(), 96 * 1024);
                let mut stats = minerapi::parse_log(&tail);
                if stats.is_empty() {
                    stats.source = "no data".to_string();
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

        let profile = cfg.gpu_profile();
        let temp_limit = profile
            .map(|p| p.default.temp_limit_c)
            .unwrap_or_else(default_temp_limit);
        let gpus_total = gpus.len() as u32;
        let gpus_enabled = gpus
            .iter()
            .filter(|gpu| {
                profile
                    .map(|p| p.tuning_for(&gpu.uuid).enabled)
                    .unwrap_or(true)
            })
            .count() as u32;
        let power = gpus
            .iter()
            .filter_map(|gpu| gpu.power_w)
            .reduce(|a, b| a + b);
        let temp_max = gpus.iter().filter_map(|gpu| gpu.temp_c).max();
        let hashrate = miner.primary();
        let efficiency = match (power, hashrate) {
            (Some(power), Some(hashrate)) if power > 0.0 && hashrate > 0.0 => {
                Some(power / (hashrate / 1e12))
            }
            _ => None,
        };

        let (miner_info, total_fee_pct, expected_hashrate) = match preset {
            Some(preset) => {
                let def = catalog::miner(&preset.miner);
                let install = cfg.install_for(preset);
                let info = MinerInfo {
                    id: preset.miner.clone(),
                    name: def
                        .map(|m| m.name.clone())
                        .unwrap_or_else(|| preset.miner.clone()),
                    version: preset.miner_version.clone(),
                    short_description: def
                        .map(|m| m.short_description().to_string())
                        .unwrap_or_default(),
                    license: def.map(|m| m.license.clone()).unwrap_or_default(),
                    source_url: def.map(|m| m.source_url.clone()).unwrap_or_default(),
                    verified: install.map(|i| i.verified).unwrap_or(false),
                    installed: install.is_some(),
                    custom: install.map(|i| i.custom).unwrap_or(false),
                    miner_fee_pct: preset.effective_miner_fee_pct(),
                    pool_fee_pct: preset.pool_fee_pct,
                    total_fee_pct: preset.total_fee_pct(),
                    fee_configurable: def.map(|m| m.fee_configurable).unwrap_or(false),
                    fee_source: if preset.miner_fee_override_pct.is_some() {
                        "override".into()
                    } else {
                        "catalog".into()
                    },
                };
                (
                    info,
                    preset.total_fee_pct(),
                    preset.expected_hashrate.unwrap_or(0.0),
                )
            }
            None => (MinerInfo::default(), 0.0, 0.0),
        };

        let snapshot = crate::oc::load_snapshot();
        let boot_id = std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .map(|text| text.trim().to_string())
            .unwrap_or_default();
        let oc = OcView {
            applied: snapshot.is_some() && state.running,
            snapshot_available: snapshot.is_some(),
            stale: snapshot
                .as_ref()
                .map(|s| s.boot_id != boot_id)
                .unwrap_or(false),
            boot_id,
        };

        let ts = util::now_unix();
        Self {
            contract_version: CONTRACT_VERSION,
            state: StateView {
                running: state.running || state.external,
                uptime_s: state.uptime_s(),
                restarts: state.restarts,
                last_error: state.last_error.clone(),
                external: state.external,
                preset_id: state
                    .preset_id
                    .clone()
                    .or_else(|| cfg.active_preset.clone())
                    .unwrap_or_default(),
                preset_name: preset.map(|p| p.name.clone()).unwrap_or_default(),
            },
            preset: cfg.active_preset.clone(),
            miner_info,
            gpus,
            gpu,
            gpu_error,
            total: TotalView {
                hashrate,
                power_w: power,
                efficiency_j_per_th: efficiency,
                temp_max_c: temp_max,
                gpus_enabled,
                gpus_total,
            },
            miner,
            pool,
            oc,
            warnings: cfg.warnings(),
            total_fee_pct,
            expected_hashrate,
            temp_limit,
            ts,
            ts_iso: util::iso8601_utc(ts),
            version: crate::VERSION.to_string(),
        }
    }

    /// Cheap snapshot without touching the network (used on stop).
    pub fn stopped(cfg: &Config) -> Self {
        let ts = util::now_unix();
        let preset = cfg.preset();
        Self {
            contract_version: CONTRACT_VERSION,
            state: StateView {
                running: false,
                preset_id: cfg.active_preset.clone().unwrap_or_default(),
                preset_name: preset.map(|p| p.name.clone()).unwrap_or_default(),
                ..Default::default()
            },
            preset: cfg.active_preset.clone(),
            total_fee_pct: cfg.total_fee_pct(),
            expected_hashrate: preset.and_then(|p| p.expected_hashrate).unwrap_or(0.0),
            temp_limit: cfg
                .gpu_profile()
                .map(|p| p.default.temp_limit_c)
                .unwrap_or_else(default_temp_limit),
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

    #[test]
    fn json_contract_round_trip() {
        let cfg = Config::default();
        let status = Status::stopped(&cfg);
        let json = status.to_json();
        let back = Status::from_json(&json).unwrap();
        assert_eq!(status, back);
        for key in [
            "\"state\"",
            "\"gpu\"",
            "\"gpus\"",
            "\"miner\"",
            "\"miner_info\"",
            "\"pool\"",
            "\"contract_version\"",
            "\"ts_iso\"",
            "\"total_fee_pct\"",
        ] {
            assert!(json.contains(key), "missing key {key} in {json}");
        }
    }

    #[test]
    fn stopped_status_has_no_metrics() {
        let status = Status::stopped(&Config::default());
        assert!(!status.state.running);
        assert!(status.gpu.is_none());
        assert!(status.gpus.is_empty());
        assert!(status.miner.is_empty());
        assert!(status.pool.is_none());
    }

    #[test]
    fn tail_of_missing_file_is_empty() {
        assert!(read_tail(Path::new("/nonexistent/file.log"), 1024).is_empty());
    }
}
