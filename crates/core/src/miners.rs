//! Miner profiles: argument builders for the supported miners.
//!
//! Only WildRig is configured by default (0 % dev fee on `pearlhash`); the
//! alternatives exist so that the built-in A/B benchmark can compare them and
//! so the active miner can be switched without touching code.

use std::path::Path;

use crate::config::Config;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MinerInfo {
    pub id: &'static str,
    pub name: &'static str,
    /// Dev fee on `pearlhash`, percent (from the miners' own docs).
    pub default_fee_pct: f64,
    /// Short note shown in the "Майнер" page.
    pub note: &'static str,
}

const INFOS: [MinerInfo; 3] = [
    MinerInfo {
        id: "wildrig",
        name: "WildRig Multi",
        default_fee_pct: 0.0,
        note: "Установлен у тебя (0.51.8). Комиссия на pearlhash — 0%.",
    },
    MinerInfo {
        id: "srbminer",
        name: "SRBMiner-MULTI",
        default_fee_pct: 0.85,
        note: "С 3.7.3 включены NVIDIA-ядра pearlhash для Ampere, в 3.8.0 ускорены для RTX 30/40.",
    },
    MinerInfo {
        id: "onezero",
        name: "OneZeroMiner",
        default_fee_pct: 1.0,
        note: "Комиссия 1% на pearlhash.",
    },
];

pub fn infos() -> &'static [MinerInfo] {
    &INFOS
}

pub fn info(id: &str) -> Option<MinerInfo> {
    INFOS.iter().copied().find(|i| i.id == id)
}

/// Build the command line for the given profile.
///
/// `extra_args` is appended verbatim (split on whitespace) so the user can add
/// anything the profile does not model yet.
pub fn build_args(cfg: &Config, profile: &str, log_file: &Path) -> Vec<String> {
    let log = log_file.to_string_lossy().into_owned();
    let miner_cfg = cfg.miner(profile);
    let mut args = match profile {
        "srbminer" => {
            let mut args: Vec<String> = vec![
                "--algorithm".into(),
                cfg.algo.clone(),
                "--pool".into(),
                cfg.pool.clone(),
                "--wallet".into(),
                endpoint_wallet(cfg),
                "--password".into(),
                "x".into(),
            ];
            if cfg.api_port > 0 {
                args.extend([
                    "--api-enable".into(),
                    "--api-port".into(),
                    cfg.api_port.to_string(),
                ]);
                if !cfg.worker.is_empty() {
                    args.extend(["--api-rig-name".into(), cfg.worker.clone()]);
                }
            }
            args.extend(["--log-file".into(), log.clone()]);
            args
        }
        "onezero" => {
            let mut args: Vec<String> = vec![
                "-a".into(),
                cfg.algo.clone(),
                "-o".into(),
                cfg.pool.clone(),
                "-u".into(),
                cfg.wallet.clone(),
            ];
            if !cfg.worker.is_empty() {
                args.extend(["-w".into(), cfg.worker.clone()]);
            }
            args
        }
        // WildRig is the default and mirrors the user's `start-mining.sh`.
        _ => {
            let mut args: Vec<String> = vec![
                "--algo".into(),
                cfg.algo.clone(),
                "--url".into(),
                cfg.pool.clone(),
                "--user".into(),
                cfg.wallet.clone(),
            ];
            if !cfg.worker.is_empty() {
                args.extend(["--worker".into(), cfg.worker.clone()]);
            }
            if cfg.api_port > 0 {
                args.extend(["--api-port".into(), cfg.api_port.to_string()]);
                if !cfg.worker.is_empty() {
                    args.extend(["--api-worker-id".into(), cfg.worker.clone()]);
                }
            }
            args.extend([
                "--log-file".into(),
                log.clone(),
                "--no-color".into(),
                "--print-time".into(),
                "10".into(),
                "--gpu-temp-limit".into(),
                cfg.gpu.temp_limit.to_string(),
                "--gpu-temp-resume".into(),
                cfg.gpu.temp_resume.to_string(),
            ]);
            if let Some(fan) = cfg.gpu.fan_speed {
                args.extend(["--gpu-fan-speed".into(), fan.to_string()]);
            }
            args
        }
    };

    if !miner_cfg.extra_args.trim().is_empty() {
        args.extend(
            miner_cfg
                .extra_args
                .split_whitespace()
                .map(|token| token.to_string()),
        );
    }
    args
}

/// WildRig/OneZero take `wallet` and `worker` separately, SRBMiner expects
/// `wallet.worker` in a single `--wallet` value.
fn endpoint_wallet(cfg: &Config) -> String {
    if cfg.worker.is_empty() {
        cfg.wallet.clone()
    } else {
        format!("{}.{}", cfg.wallet, cfg.worker)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(cfg: &Config, profile: &str) -> Vec<String> {
        build_args(cfg, profile, Path::new("/tmp/miner.log"))
    }

    fn contains_pair(args: &[String], key: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == key && w[1] == value)
    }

    #[test]
    fn wildrig_matches_the_existing_script() {
        let cfg = Config::default();
        let args = args(&cfg, "wildrig");
        assert_eq!(args[0], "--algo");
        assert_eq!(args[1], "pearlhash");
        assert!(contains_pair(&args, "--url", "pool.pearlhash.xyz:9000"));
        assert!(contains_pair(&args, "--user", &cfg.wallet));
        assert!(contains_pair(&args, "--worker", "3080ti"));
        assert!(contains_pair(&args, "--api-port", "4028"));
        assert!(contains_pair(&args, "--log-file", "/tmp/miner.log"));
        assert!(contains_pair(&args, "--gpu-temp-limit", "90"));
        assert!(contains_pair(&args, "--gpu-temp-resume", "65"));
        assert!(args.contains(&"--no-color".to_string()));
        // No fan flag unless configured: the card manages its own fans.
        assert!(!args.contains(&"--gpu-fan-speed".to_string()));
    }

    #[test]
    fn wildrig_passes_fan_speed_when_set() {
        let mut cfg = Config::default();
        cfg.gpu.fan_speed = Some(70);
        let args = args(&cfg, "wildrig");
        assert!(contains_pair(&args, "--gpu-fan-speed", "70"));
    }

    #[test]
    fn srbminer_uses_wallet_worker_and_api() {
        let cfg = Config::default();
        let args = args(&cfg, "srbminer");
        assert!(contains_pair(&args, "--algorithm", "pearlhash"));
        assert!(contains_pair(&args, "--pool", "pool.pearlhash.xyz:9000"));
        assert!(contains_pair(
            &args,
            "--wallet",
            &format!("{}.3080ti", cfg.wallet)
        ));
        assert!(args.contains(&"--api-enable".to_string()));
        assert!(contains_pair(&args, "--api-port", "4028"));
    }

    #[test]
    fn onezero_uses_short_flags() {
        let cfg = Config::default();
        let args = args(&cfg, "onezero");
        assert_eq!(args[0], "-a");
        assert!(contains_pair(&args, "-o", "pool.pearlhash.xyz:9000"));
        assert!(contains_pair(&args, "-u", &cfg.wallet));
        assert!(contains_pair(&args, "-w", "3080ti"));
    }

    #[test]
    fn extra_args_are_appended() {
        let mut cfg = Config::default();
        cfg.miners.get_mut("wildrig").unwrap().extra_args =
            "--gpu-intensity 24  --benchmark".into();
        let args = args(&cfg, "wildrig");
        let tail: Vec<&str> = args[args.len() - 3..].iter().map(String::as_str).collect();
        assert_eq!(tail, ["--gpu-intensity", "24", "--benchmark"]);
    }

    #[test]
    fn api_port_zero_disables_api_flags() {
        let mut cfg = Config::default();
        cfg.api_port = 0;
        let args = args(&cfg, "wildrig");
        assert!(!args.contains(&"--api-port".to_string()));
    }

    #[test]
    fn known_profiles_are_registered() {
        for id in ["wildrig", "srbminer", "onezero"] {
            assert!(info(id).is_some(), "profile {id} is missing");
        }
        assert_eq!(info("wildrig").unwrap().default_fee_pct, 0.0);
    }
}
