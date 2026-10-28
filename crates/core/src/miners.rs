//! Miner command construction.
//!
//! Core arguments are driver-specific; everything else comes from the
//! catalogue's option descriptors and the preset's `miner_options` map. No
//! shell is involved: the supervisor spawns the binary directly.

use std::path::Path;

use crate::catalog;
use crate::config::{Config, GpuProfile, Preset};

/// Resolve the GPU profile used by the preset for temperature/fan defaults.
fn profile(cfg: &Config, preset: &Preset) -> GpuProfile {
    cfg.gpu_profiles
        .get(&preset.gpu_profile)
        .cloned()
        .unwrap_or_default()
}

/// Build the full argument vector for a preset.
pub fn build_args(cfg: &Config, preset: &Preset, log_file: &Path) -> anyhow::Result<Vec<String>> {
    let log = log_file.to_string_lossy().into_owned();
    let gpu = profile(cfg, preset);
    let temp_limit = gpu.default.temp_limit_c;
    let temp_resume = gpu.default.temp_resume_c;
    let mut args = match preset.miner.as_str() {
        "srbminer" => {
            let mut args: Vec<String> = vec![
                "--algorithm".into(),
                preset.algo.clone(),
                "--pool".into(),
                preset.pool_url.clone(),
                "--wallet".into(),
                endpoint_wallet(&preset.wallet, &preset.worker),
                "--password".into(),
                "x".into(),
            ];
            if preset.api_port > 0 {
                args.extend([
                    "--api-enable".into(),
                    "--api-port".into(),
                    preset.api_port.to_string(),
                ]);
                if !preset.worker.is_empty() {
                    args.extend(["--api-rig-name".into(), preset.worker.clone()]);
                }
            }
            args.extend(["--log-file".into(), log]);
            args
        }
        "onezero" => {
            let mut args: Vec<String> = vec![
                "-a".into(),
                preset.algo.clone(),
                "-o".into(),
                preset.pool_url.clone(),
                "-u".into(),
                preset.wallet.clone(),
            ];
            if !preset.worker.is_empty() {
                args.extend(["-w".into(), preset.worker.clone()]);
            }
            args
        }
        // WildRig and the generic custom driver.
        _ => {
            let mut args: Vec<String> = vec![
                "--algo".into(),
                preset.algo.clone(),
                "--url".into(),
                preset.pool_url.clone(),
                "--user".into(),
                preset.wallet.clone(),
            ];
            if !preset.worker.is_empty() {
                args.extend(["--worker".into(), preset.worker.clone()]);
            }
            if preset.api_port > 0 {
                args.extend(["--api-port".into(), preset.api_port.to_string()]);
                if !preset.worker.is_empty() {
                    args.extend(["--api-worker-id".into(), preset.worker.clone()]);
                }
            }
            args.extend([
                "--log-file".into(),
                log,
                "--no-color".into(),
                "--print-time".into(),
                "10".into(),
                "--gpu-temp-limit".into(),
                temp_limit.to_string(),
                "--gpu-temp-resume".into(),
                temp_resume.to_string(),
            ]);
            args
        }
    };

    if let Some(miner) = catalog::miner(&preset.miner) {
        append_options(&mut args, miner, preset);
        if miner.fee_configurable && !miner.fee_flag.is_empty() {
            if let Some(fee) = preset.miner_fee_override_pct {
                args.extend([miner.fee_flag.clone(), format!("{fee}")]);
            }
        }
    }

    if !preset.extra_args.trim().is_empty() {
        let parsed = shell_words::split(&preset.extra_args)
            .map_err(|e| anyhow::anyhow!("invalid extra arguments: {e}"))?;
        args.extend(parsed);
    }

    Ok(args)
}

fn append_options(args: &mut Vec<String>, miner: &catalog::MinerDef, preset: &Preset) {
    for option in &miner.options {
        if option.flag.is_empty() {
            continue;
        }
        let Some(value) = preset.miner_options.get(&option.key) else {
            continue;
        };
        match option.kind {
            catalog::OptionKind::Bool => {
                if value.as_bool().unwrap_or(false) {
                    args.push(option.flag.clone());
                }
            }
            catalog::OptionKind::Int => {
                let Some(number) = value.as_i64() else {
                    continue;
                };
                if option.key == "fan_speed" && number == 0 {
                    continue;
                }
                if let Some(min) = option.min {
                    if (number as f64) < min {
                        continue;
                    }
                }
                if let Some(max) = option.max {
                    if (number as f64) > max {
                        continue;
                    }
                }
                args.push(option.flag.clone());
                args.push(number.to_string());
            }
            catalog::OptionKind::Enum | catalog::OptionKind::String => {
                let Some(text) = value.as_str() else { continue };
                if text.trim().is_empty() {
                    continue;
                }
                args.push(option.flag.clone());
                args.push(text.to_string());
            }
        }
    }
}

/// WildRig/OneZero take `wallet` and `worker` separately, SRBMiner expects
/// `wallet.worker` in a single `--wallet` value.
fn endpoint_wallet(wallet: &str, worker: &str) -> String {
    if worker.is_empty() {
        wallet.to_string()
    } else {
        format!("{wallet}.{worker}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, Preset};

    fn cfg_and_preset() -> (Config, Preset) {
        let cfg = Config::default();
        let preset = cfg.preset().unwrap().clone();
        (cfg, preset)
    }

    fn args(cfg: &Config, preset: &Preset) -> Vec<String> {
        build_args(cfg, preset, Path::new("/tmp/miner.log")).unwrap()
    }

    fn contains_pair(args: &[String], key: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == key && w[1] == value)
    }

    #[test]
    fn wildrig_matches_the_existing_setup() {
        let (cfg, preset) = cfg_and_preset();
        let args = args(&cfg, &preset);
        assert_eq!(args[0], "--algo");
        assert_eq!(args[1], "pearlhash");
        assert!(contains_pair(&args, "--url", "pool.pearlhash.xyz:9000"));
        assert!(contains_pair(&args, "--user", ""));
        assert!(contains_pair(&args, "--api-port", "4028"));
        assert!(contains_pair(&args, "--log-file", "/tmp/miner.log"));
        assert!(contains_pair(&args, "--gpu-temp-limit", "85"));
        assert!(args.contains(&"--no-color".to_string()));
    }

    #[test]
    fn options_are_rendered_from_catalogue() {
        let (cfg, mut preset) = cfg_and_preset();
        preset
            .miner_options
            .insert("fan_speed".into(), serde_json::json!(70));
        preset
            .miner_options
            .insert("intensity".into(), serde_json::json!(24));
        let args = args(&cfg, &preset);
        assert!(contains_pair(&args, "--gpu-fan-speed", "70"));
        assert!(contains_pair(&args, "--gpu-intensity", "24"));
    }

    #[test]
    fn zero_fan_is_auto_and_skipped() {
        let (cfg, mut preset) = cfg_and_preset();
        preset
            .miner_options
            .insert("fan_speed".into(), serde_json::json!(0));
        let args = args(&cfg, &preset);
        assert!(!args.contains(&"--gpu-fan-speed".to_string()));
    }

    #[test]
    fn srbminer_uses_wallet_worker_and_api() {
        let (cfg, mut preset) = cfg_and_preset();
        preset.miner = "srbminer".into();
        preset.miner_version = "3.8.0".into();
        preset.worker = "3080ti".into();
        let args = args(&cfg, &preset);
        assert!(contains_pair(&args, "--algorithm", "pearlhash"));
        assert!(contains_pair(&args, "--wallet", ".3080ti"));
        assert!(args.contains(&"--api-enable".to_string()));
    }

    #[test]
    fn onezero_uses_short_flags() {
        let (cfg, mut preset) = cfg_and_preset();
        preset.miner = "onezero".into();
        preset.miner_version = "1.2.0".into();
        preset.worker = "3080ti".into();
        let args = args(&cfg, &preset);
        assert_eq!(args[0], "-a");
        assert!(contains_pair(&args, "-w", "3080ti"));
    }

    #[test]
    fn extra_args_are_quoted_and_appended() {
        let (cfg, mut preset) = cfg_and_preset();
        preset.extra_args = "--gpu-intensity 24 --benchmark \"two words\"".into();
        let args = args(&cfg, &preset);
        let tail: Vec<&str> = args[args.len() - 4..].iter().map(String::as_str).collect();
        assert_eq!(tail, ["--gpu-intensity", "24", "--benchmark", "two words"]);
    }

    #[test]
    fn fixed_fee_is_not_passed_to_the_miner() {
        let (cfg, mut preset) = cfg_and_preset();
        preset.miner_fee_override_pct = Some(0.0);
        let args = args(&cfg, &preset);
        assert!(!args.iter().any(|a| a.contains("fee")));
    }
}
