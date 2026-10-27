//! A/B benchmark of installed miners for the active preset.
//!
//! The only honest way to pick a miner: run every candidate against the same
//! pool for the same time and compare `effective = raw × (1 − total_fee)`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::catalog;
use crate::config::{Config, Preset};
use crate::minerapi;
use crate::miners;
use crate::paths;
use crate::pool;
use crate::util;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BenchEntry {
    pub profile: String,
    pub name: String,
    pub version: String,
    pub raw_hps: Option<f64>,
    pub pool_hps: Option<f64>,
    pub miner_fee_pct: f64,
    pub pool_fee_pct: f64,
    pub total_fee_pct: f64,
    pub effective_hps: Option<f64>,
    pub accepted: Option<u64>,
    pub samples: u32,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BenchReport {
    pub started_at: u64,
    pub duration_s: u64,
    pub entries: Vec<BenchEntry>,
}

impl BenchReport {
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".into())
    }

    pub fn from_json(text: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(text)?)
    }

    pub fn best(&self) -> Option<&BenchEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.effective_hps.is_some() && entry.error.is_none())
            .max_by(|a, b| {
                a.effective_hps
                    .partial_cmp(&b.effective_hps)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    }
}

/// Run the benchmark sequentially for every miner id. Mining must be stopped by
/// the caller; the OC profile is assumed to be already applied.
pub fn run(cfg: &Config, profiles: &[String], seconds: u64) -> BenchReport {
    let seconds = seconds.clamp(30, 3600);
    let mut report = BenchReport {
        started_at: util::now_unix(),
        duration_s: seconds,
        entries: Vec::new(),
    };

    let Some(base_preset) = cfg.preset().cloned() else {
        return report;
    };

    for miner_id in profiles {
        let def = catalog::miner(miner_id);
        let installed = cfg
            .miner_installs
            .values()
            .filter(|i| i.miner == *miner_id && std::path::Path::new(&i.path).exists())
            .max_by_key(|i| i.installed_at);
        let version = installed
            .map(|i| i.version.clone())
            .or_else(|| {
                def.and_then(|m| m.latest_version())
                    .map(|v| v.version.clone())
            })
            .unwrap_or_default();
        let mut entry = BenchEntry {
            profile: miner_id.clone(),
            name: def
                .map(|m| m.name.clone())
                .unwrap_or_else(|| miner_id.clone()),
            version: version.clone(),
            miner_fee_pct: def.map(|m| m.fee_pct).unwrap_or(0.0),
            pool_fee_pct: base_preset.pool_fee_pct,
            ..Default::default()
        };
        entry.total_fee_pct = entry.miner_fee_pct + entry.pool_fee_pct;

        let Some(install) = cfg.install(miner_id, &version) else {
            entry.error = Some(format!("miner {miner_id} {version} is not installed"));
            report.entries.push(entry);
            continue;
        };
        let binary = PathBuf::from(&install.path);
        if !binary.exists() {
            entry.error = Some(format!("binary not found: {}", binary.display()));
            report.entries.push(entry);
            continue;
        }

        let mut bench_cfg = cfg.clone();
        {
            let preset = bench_cfg.ensure_preset();
            preset.miner = miner_id.clone();
            preset.miner_version = version.clone();
            preset.miner_fee_pct = entry.miner_fee_pct;
            preset.miner_fee_override_pct = None;
        }
        let log_path = paths::bench_dir().join(format!("bench-{miner_id}.log"));
        let _ = std::fs::create_dir_all(paths::bench_dir());
        let bench_preset = bench_cfg
            .preset()
            .cloned()
            .unwrap_or_else(Preset::new_pearl);
        let args = match miners::build_args(&bench_cfg, &bench_preset, &log_path) {
            Ok(args) => args,
            Err(err) => {
                entry.error = Some(err.to_string());
                report.entries.push(entry);
                continue;
            }
        };
        let mut child = match spawn(&binary, &args, &log_path) {
            Ok(child) => child,
            Err(err) => {
                entry.error = Some(err.to_string());
                report.entries.push(entry);
                continue;
            }
        };

        let warmup = (seconds / 3).clamp(10, 45);
        std::thread::sleep(Duration::from_secs(warmup));
        let started = Instant::now();
        let mut samples: Vec<f64> = Vec::new();
        let mut accepted: Option<u64> = None;

        let api_port = bench_cfg.preset().map(|p| p.api_port).unwrap_or(0);
        while started.elapsed() < Duration::from_secs(seconds.saturating_sub(warmup)) {
            if let Ok(stats) = minerapi::query_api(api_port) {
                if let Some(value) = stats.hashrate_60s.or(stats.hashrate_10s) {
                    samples.push(value);
                }
                if stats.accepted.is_some() {
                    accepted = stats.accepted;
                }
            }
            std::thread::sleep(Duration::from_secs(2));
            if child.try_wait().ok().flatten().is_some() {
                entry.error = Some("miner exited during the benchmark".into());
                break;
            }
        }

        let _ = child.kill();
        let _ = child.wait();

        if entry.error.is_none() {
            if samples.is_empty() {
                entry.error = Some("no hash rate from API or log".into());
            } else {
                let raw = samples.iter().sum::<f64>() / samples.len() as f64;
                entry.raw_hps = Some(raw);
                entry.samples = samples.len() as u32;
                entry.accepted = accepted;
                entry.effective_hps = Some(raw * (1.0 - entry.total_fee_pct / 100.0));
                entry.pool_hps = pool::query(cfg).ok().and_then(|p| p.hashrate);
            }
        }

        report.entries.push(entry);
        std::thread::sleep(Duration::from_secs(5));
    }

    report
}

fn spawn(
    binary: &std::path::Path,
    args: &[String],
    log_path: &std::path::Path,
) -> anyhow::Result<Child> {
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log_path)?;
    let log_err = log.try_clone()?;
    let mut command = Command::new(binary);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    if let Some(dir) = binary.parent() {
        command.current_dir(dir);
    }
    command
        .spawn()
        .map_err(|e| anyhow::anyhow!("cannot start {}: {e}", binary.display()))
}

pub fn save(report: &BenchReport) -> anyhow::Result<PathBuf> {
    let dir = paths::bench_dir();
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("bench-{}.json", report.started_at));
    crate::config::write_atomic(&path, report.to_json().as_bytes())?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(profile: &str, raw: f64, miner_fee: f64, pool_fee: f64) -> BenchEntry {
        BenchEntry {
            profile: profile.into(),
            name: profile.into(),
            raw_hps: Some(raw),
            miner_fee_pct: miner_fee,
            pool_fee_pct: pool_fee,
            total_fee_pct: miner_fee + pool_fee,
            effective_hps: Some(raw * (1.0 - (miner_fee + pool_fee) / 100.0)),
            ..Default::default()
        }
    }

    #[test]
    fn picks_the_best_effective_hashrate() {
        let report = BenchReport {
            started_at: 1,
            duration_s: 60,
            entries: vec![
                entry("wildrig", 125.0e12, 0.0, 1.0),
                entry("srbminer", 124.0e12, 0.85, 1.0),
            ],
        };
        assert_eq!(report.best().unwrap().profile, "wildrig");
    }

    #[test]
    fn fee_can_flip_the_winner() {
        let report = BenchReport {
            started_at: 1,
            duration_s: 60,
            entries: vec![
                entry("wildrig", 125.0e12, 0.0, 1.0),
                entry("srbminer", 127.0e12, 0.85, 1.0),
            ],
        };
        assert_eq!(report.best().unwrap().profile, "srbminer");
    }

    #[test]
    fn report_survives_json() {
        let report = BenchReport {
            started_at: 42,
            duration_s: 120,
            entries: vec![entry("wildrig", 125.0e12, 0.0, 1.0)],
        };
        let back = BenchReport::from_json(&report.to_json()).unwrap();
        assert_eq!(report, back);
    }

    #[test]
    fn missing_install_is_reported() {
        let cfg = Config::default();
        let report = run(&cfg, &["onezero".to_string()], 30);
        assert!(report.entries[0].error.is_some());
    }
}
