//! A/B benchmark of miner profiles on the user's own card.
//!
//! The only honest way to pick a miner: run every candidate against the same
//! pool for the same time and compare `effective = raw × (1 − dev_fee)`.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::minerapi;
use crate::miners;
use crate::paths;
use crate::pool;
use crate::util;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct BenchEntry {
    pub profile: String,
    pub name: String,
    /// Average hash rate reported by the miner itself (H/s).
    pub raw_hps: Option<f64>,
    /// Hash rate reported by the pool (H/s), if available.
    pub pool_hps: Option<f64>,
    pub dev_fee_pct: f64,
    /// `raw_hps × (1 − dev_fee)`.
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

    /// Winner by effective hash rate.
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

/// Run the benchmark sequentially for every profile. Mining must be stopped by
/// the caller; the OC profile is assumed to be already applied.
pub fn run(cfg: &Config, profiles: &[String], seconds: u64) -> BenchReport {
    let seconds = seconds.clamp(30, 3600);
    let mut report = BenchReport {
        started_at: util::now_unix(),
        duration_s: seconds,
        entries: Vec::new(),
    };

    for profile in profiles {
        let info = miners::info(profile);
        let miner_cfg = cfg.miner(profile);
        let name = info.map(|i| i.name).unwrap_or(profile.as_str()).to_string();
        let mut entry = BenchEntry {
            profile: profile.clone(),
            name,
            dev_fee_pct: miner_cfg.dev_fee_pct,
            ..Default::default()
        };

        let binary = PathBuf::from(miner_cfg.path.trim());
        if miner_cfg.path.trim().is_empty() || !binary.exists() {
            entry.error = Some(format!("бинарник не найден: {}", binary.display()));
            report.entries.push(entry);
            continue;
        }

        let mut bench_cfg = cfg.clone();
        bench_cfg.miner_profile = profile.clone();
        let log_path = paths::bench_dir().join(format!("bench-{profile}.log"));
        let _ = std::fs::create_dir_all(paths::bench_dir());
        let args = miners::build_args(&bench_cfg, profile, &log_path);

        let child = spawn(&binary, &args, &log_path);
        let mut child = match child {
            Ok(child) => child,
            Err(err) => {
                entry.error = Some(err.to_string());
                report.entries.push(entry);
                continue;
            }
        };

        // Warm-up: DAG/JIT compilation takes a while on pearlhash.
        let warmup = (seconds / 3).clamp(10, 45);
        std::thread::sleep(Duration::from_secs(warmup));
        let started = Instant::now();
        let mut samples: Vec<f64> = Vec::new();
        let mut accepted: Option<u64> = None;

        while started.elapsed() < Duration::from_secs(seconds.saturating_sub(warmup)) {
            if let Ok(stats) = minerapi::query_api(cfg.api_port) {
                if let Some(value) = stats.hashrate_60s.or(stats.hashrate_10s) {
                    samples.push(value);
                }
                if stats.accepted.is_some() {
                    accepted = stats.accepted;
                }
            }
            std::thread::sleep(Duration::from_secs(2));
            if child.try_wait().ok().flatten().is_some() {
                entry.error = Some("майнер завершился во время теста".into());
                break;
            }
        }

        let _ = child.kill();
        let _ = child.wait();

        if entry.error.is_none() {
            if samples.is_empty() {
                entry.error = Some("не удалось получить хешрейт (API и лог пусты)".into());
            } else {
                let raw = samples.iter().sum::<f64>() / samples.len() as f64;
                entry.raw_hps = Some(raw);
                entry.samples = samples.len() as u32;
                entry.accepted = accepted;
                entry.effective_hps = Some(raw * (1.0 - entry.dev_fee_pct / 100.0));
                entry.pool_hps = pool::query(cfg).ok().and_then(|p| p.hashrate);
            }
        }

        report.entries.push(entry);
        // Let the GPU cool down and the port free up between candidates.
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
        .map_err(|e| anyhow::anyhow!("не удалось запустить {}: {e}", binary.display()))
}

/// Persist the report under `~/.local/state/pearl-miner/bench/`.
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

    fn entry(profile: &str, raw: f64, fee: f64) -> BenchEntry {
        BenchEntry {
            profile: profile.into(),
            name: profile.into(),
            raw_hps: Some(raw),
            dev_fee_pct: fee,
            effective_hps: Some(raw * (1.0 - fee / 100.0)),
            ..Default::default()
        }
    }

    #[test]
    fn picks_the_best_effective_hashrate() {
        let report = BenchReport {
            started_at: 1,
            duration_s: 60,
            entries: vec![
                entry("wildrig", 125.0e12, 0.0),
                entry("srbminer", 124.0e12, 0.85),
                BenchEntry {
                    profile: "onezero".into(),
                    error: Some("нет бинарника".into()),
                    ..Default::default()
                },
            ],
        };
        assert_eq!(report.best().unwrap().profile, "wildrig");
    }

    #[test]
    fn fee_can_flip_the_winner() {
        // 0.85 % fee pays off only when the raw lead is bigger than the fee.
        let report = BenchReport {
            started_at: 1,
            duration_s: 60,
            entries: vec![
                entry("wildrig", 125.0e12, 0.0),
                entry("srbminer", 127.0e12, 0.85),
            ],
        };
        assert_eq!(report.best().unwrap().profile, "srbminer");
        let narrow = BenchReport {
            started_at: 1,
            duration_s: 60,
            entries: vec![
                entry("wildrig", 125.0e12, 0.0),
                entry("srbminer", 125.5e12, 0.85),
            ],
        };
        assert_eq!(narrow.best().unwrap().profile, "wildrig");
    }

    #[test]
    fn report_survives_json() {
        let report = BenchReport {
            started_at: 42,
            duration_s: 120,
            entries: vec![entry("wildrig", 125.0e12, 0.0)],
        };
        let back = BenchReport::from_json(&report.to_json()).unwrap();
        assert_eq!(report, back);
    }

    #[test]
    fn missing_binary_is_reported() {
        let mut cfg = Config::default();
        cfg.miners.get_mut("onezero").unwrap().path = "/nonexistent/onezero".into();
        let report = run(&cfg, &["onezero".to_string()], 30);
        assert!(report.entries[0].error.is_some());
    }
}
