//! The status contract shared by the daemon, CLI, GUI and GNOME extension.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::catalog;
use crate::config::Config;
use crate::i18n::tr;
use crate::minerapi::{self, MinerStats};
use crate::nvidia::{self, GpuStats};
use crate::pool::{self, PoolStats};
use crate::state::State;
use crate::util;

pub const CONTRACT_VERSION: u32 = 3;

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
    /// Tuning from the current snapshot was applied (or an apply was started).
    pub applied: bool,
    /// The tuning is still on the cards while the miner is stopped. The GUI
    /// shows a banner with a Restore action in this state.
    pub needs_restore: bool,
    pub snapshot_available: bool,
    pub stale: bool,
    pub boot_id: String,
    pub restore_on_stop: bool,
    pub last_error: Option<String>,
    pub snapshot_created_at: Option<u64>,
    pub snapshot_boot_id: Option<String>,
    /// Summary for the first card; the full per-GPU targets are in `oc_report`.
    pub restore_power_w: Option<u32>,
    pub driver_default_w: Option<u32>,
    pub was_tuned: bool,
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

/// Compare requested clock locks with what the driver actually reports.
///
/// A hard `-lgc`/`-lmc` lock should match exactly; the tolerances only absorb
/// rounding. Core is checked under load because an idle GPU legitimately
/// downclocks below the lock.
fn lock_warnings(cfg: &Config, gpus: &[GpuStats], running: bool, uptime_s: u64) -> Vec<String> {
    if !running {
        return Vec::new();
    }
    let Some(profile) = cfg.gpu_profile() else {
        return Vec::new();
    };
    let mut warnings = Vec::new();
    for gpu in gpus {
        let tuning = profile.tuning_for(&gpu.uuid);
        if !tuning.enabled {
            continue;
        }
        if let (Some(want), Some(have)) = (tuning.memory_clock_mhz, gpu.mem_mhz) {
            if want.abs_diff(have) > 50 {
                warnings.push(tr(
                    &format!(
                        "GPU #{}: memory lock {want} MHz is not active (now {have} MHz)",
                        gpu.index
                    ),
                    &format!(
                        "Карта #{}: лок памяти {want} MHz не применился (сейчас {have} MHz)",
                        gpu.index
                    ),
                ));
            }
        }
        if uptime_s >= 10 && gpu.util_pct.unwrap_or(0) >= 80 {
            if let (Some(want), Some(have)) = (tuning.core_clock_mhz, gpu.core_mhz) {
                if want.abs_diff(have) > 100 {
                    warnings.push(tr(
                        &format!(
                            "GPU #{}: core lock {want} MHz is not active (now {have} MHz)",
                            gpu.index
                        ),
                        &format!(
                            "Карта #{}: лок ядра {want} MHz не применился (сейчас {have} MHz)",
                            gpu.index
                        ),
                    ));
                }
            }
        }
    }
    warnings
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
        let boot_id = crate::oc::current_boot_id();
        // A snapshot from another boot is diagnostic only: it must never make
        // the UI offer a restore or claim that settings are still applied.
        let fresh = snapshot
            .as_ref()
            .filter(|snapshot| snapshot.boot_id == boot_id);
        let applied = fresh.map(|s| s.applied).unwrap_or(false);
        let first = fresh.and_then(|s| s.gpus.first());
        let oc = OcView {
            applied,
            needs_restore: applied && !state.running,
            snapshot_available: fresh.is_some(),
            stale: snapshot.is_some() && fresh.is_none(),
            boot_id,
            restore_on_stop: cfg.behavior.restore_stock_on_stop,
            last_error: fresh.and_then(|s| s.last_error.clone()),
            snapshot_created_at: fresh.map(|s| s.created_at),
            snapshot_boot_id: fresh.map(|s| s.boot_id.clone()),
            restore_power_w: first
                .and_then(|gpu| gpu.restore_power_w(crate::oc::RestoreMode::Snapshot)),
            driver_default_w: first
                .and_then(|gpu| gpu.power_default_w)
                .map(|value| value.round() as u32),
            was_tuned: first.map(|gpu| gpu.was_tuned).unwrap_or(false),
        };

        let ts = util::now_unix();
        let mut warnings = cfg.warnings();
        warnings.extend(lock_warnings(
            cfg,
            &gpus,
            state.running && !state.external,
            state.uptime_s(),
        ));
        if oc.needs_restore {
            warnings.push(tr(
                "GPU power/clocks are still applied; use Restore stock in Settings",
                "Питание/частоты GPU всё ещё применены — верните сток в «Настройках»",
            ));
        }
        if let Some(err) = &oc.last_error {
            warnings.push(format!(
                "{} {err}",
                tr("GPU restore failed:", "Возврат стока не удался:")
            ));
        }
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
            warnings,
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

    fn tuned_config(uuid: &str) -> Config {
        let mut cfg = Config::default();
        let profile = cfg.gpu_profiles.get_mut("balanced").unwrap();
        profile.per_gpu.insert(
            uuid.to_string(),
            crate::config::GpuTuning {
                enabled: true,
                power_limit_w: Some(300),
                core_clock_mhz: Some(1620),
                memory_clock_mhz: Some(810),
                custom: true,
                ..Default::default()
            },
        );
        cfg
    }

    fn tuned_gpu(uuid: &str) -> GpuStats {
        GpuStats {
            index: 0,
            uuid: uuid.into(),
            core_mhz: Some(1620),
            mem_mhz: Some(810),
            util_pct: Some(100),
            ..Default::default()
        }
    }

    #[test]
    fn matching_locks_do_not_warn() {
        let uuid = "GPU-test";
        let cfg = tuned_config(uuid);
        let warnings = lock_warnings(&cfg, &[tuned_gpu(uuid)], true, 60);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn memory_mismatch_warns_even_at_idle() {
        let uuid = "GPU-test";
        let cfg = tuned_config(uuid);
        let mut gpu = tuned_gpu(uuid);
        gpu.mem_mhz = Some(9501);
        gpu.util_pct = Some(0);
        let warnings = lock_warnings(&cfg, &[gpu], true, 60);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("810"), "{warnings:?}");
        assert!(warnings[0].contains("9501"), "{warnings:?}");
    }

    #[test]
    fn core_mismatch_is_ignored_while_idle_or_starting() {
        let uuid = "GPU-test";
        let cfg = tuned_config(uuid);
        let mut gpu = tuned_gpu(uuid);
        gpu.core_mhz = Some(1770);
        gpu.util_pct = Some(10);
        assert!(lock_warnings(&cfg, &[gpu.clone()], true, 60).is_empty());
        gpu.util_pct = Some(100);
        assert!(lock_warnings(&cfg, &[gpu.clone()], true, 5).is_empty());
        let warnings = lock_warnings(&cfg, &[gpu], true, 60);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("1620"), "{warnings:?}");
        assert!(warnings[0].contains("1770"), "{warnings:?}");
    }

    #[test]
    fn disabled_gpu_does_not_warn() {
        let uuid = "GPU-test";
        let mut cfg = tuned_config(uuid);
        cfg.gpu_profiles
            .get_mut("balanced")
            .unwrap()
            .per_gpu
            .get_mut(uuid)
            .unwrap()
            .enabled = false;
        let mut gpu = tuned_gpu(uuid);
        gpu.mem_mhz = Some(9501);
        assert!(lock_warnings(&cfg, &[gpu], true, 60).is_empty());
    }
}
