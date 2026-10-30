//! Miner command construction.
//!
//! Core arguments are driver-specific; everything else comes from the
//! catalogue's option descriptors and the preset's `miner_options` map. No
//! shell is involved: the supervisor spawns the binary directly.
//!
//! v4 moves the driver, API flags and per-algorithm fee into the catalogue so a
//! community miner with the generic shape is data-only.

use std::path::Path;

use crate::catalog::{self, MinerDriver};
use crate::config::{Config, Preset};
use crate::i18n::tr;
use crate::nvidia::GpuStats;

/// True when the catalogue documents a flag that lets this miner select the
/// cards it runs on.
pub fn supports_device_selection(miner_id: &str) -> bool {
    catalog::miner(miner_id)
        .map(|miner| miner.selects_devices())
        .unwrap_or(false)
}

fn unsupported_device_selection(miner_id: &str) -> anyhow::Error {
    anyhow::anyhow!(tr(
        &format!(
            "The selected miner ({miner_id}) cannot choose GPUs; enable every card or choose a miner that documents device selection"
        ),
        &format!(
            "Выбранный майнер ({miner_id}) не умеет выбирать видеокарты; включите все карты или выберите майнер с поддержкой выбора устройств"
        )
    ))
}

/// Indices (`nvidia-smi` order) to pass to the miner, or `None` when every
/// detected card takes part and the miner's default (all devices) is correct.
///
/// A partial participation is a hard capability check: a miner without a
/// documented device flag must not be started, because it would silently mine
/// on the excluded card as well.
pub fn device_plan(
    _cfg: &Config,
    preset: &Preset,
    gpus: &[GpuStats],
) -> anyhow::Result<Option<Vec<u32>>> {
    let participating: Vec<u32> = gpus
        .iter()
        .filter(|gpu| preset.gpu.tuning_for(&gpu.uuid).enabled)
        .map(|gpu| gpu.index)
        .collect();
    if participating.len() == gpus.len() {
        return Ok(None);
    }
    if participating.is_empty() {
        anyhow::bail!(tr(
            "No GPU is selected for mining",
            "Ни одна видеокарта не выбрана для майнинга"
        ));
    }
    if !supports_device_selection(&preset.miner) {
        return Err(unsupported_device_selection(&preset.miner));
    }
    Ok(Some(participating))
}

/// Render the device flag for a miner that documents one. The list is the
/// `nvidia-smi` index of every participating card, comma-separated.
fn render_device_args(
    miner: Option<&catalog::MinerDef>,
    miner_id: &str,
    devices: &[u32],
) -> anyhow::Result<Vec<String>> {
    if devices.is_empty() {
        anyhow::bail!(tr(
            "No GPU is selected for mining",
            "Ни одна видеокарта не выбрана для майнинга"
        ));
    }
    let Some(miner) = miner else {
        return Err(unsupported_device_selection(miner_id));
    };
    if !miner.selects_devices() {
        return Err(unsupported_device_selection(miner_id));
    }
    let list = devices
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");
    Ok(vec![miner.device_flag.clone(), list])
}

/// Build the full argument vector for a preset.
///
/// This is the "all detected cards take part" form. The supervisor uses
/// [`build_args_with_devices`] when a card is excluded from the session.
pub fn build_args(cfg: &Config, preset: &Preset, log_file: &Path) -> anyhow::Result<Vec<String>> {
    build_args_with_devices(cfg, preset, log_file, None)
}

/// Build the argument vector, optionally restricting the miner to `devices`
/// (`nvidia-smi` indices). `None` leaves the miner on all its default devices.
pub fn build_args_with_devices(
    cfg: &Config,
    preset: &Preset,
    log_file: &Path,
    devices: Option<&[u32]>,
) -> anyhow::Result<Vec<String>> {
    let log = log_file.to_string_lossy().into_owned();
    let miner = catalog::miner(&preset.miner);
    let driver = miner.map(|miner| miner.driver).unwrap_or_default();
    let temp_limit = cfg.behavior.temp_limit_c;
    let temp_resume = cfg.behavior.temp_resume_c;
    let api_port = preset.effective_api_port();

    let mut args = match driver {
        MinerDriver::Srbminer => {
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
            args.extend(["--log-file".into(), log]);
            args
        }
        MinerDriver::Onezero => {
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
        // WildRig and the generic custom driver share the argument shape.
        MinerDriver::Wildrig | MinerDriver::Generic => {
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

    append_api_args(&mut args, miner, preset, api_port);

    if let Some(devices) = devices {
        args.extend(render_device_args(miner, &preset.miner, devices)?);
    }

    if let Some(miner) = miner {
        append_options(&mut args, miner, preset);
        let fee = miner.fee_for(&preset.algo);
        if fee.configurable && !fee.flag.is_empty() {
            if let Some(override_pct) = preset.miner_fee_override_pct {
                args.push(fee.flag);
                args.push(override_pct.to_string());
            }
        }
    }

    let extra = if preset.extra_args.trim().is_empty() {
        None
    } else {
        Some(
            shell_words::split(&preset.extra_args)
                .map_err(|e| anyhow::anyhow!("invalid extra arguments: {e}"))?,
        )
    };
    // A second device list would contradict the switches (and the miner would
    // either ignore one or fail), so refuse instead of guessing.
    if devices.is_some() {
        if let (Some(miner), Some(extra)) = (miner, extra.as_ref()) {
            let flag = miner.device_flag.as_str();
            if !flag.is_empty()
                && extra
                    .iter()
                    .any(|arg| arg == flag || arg.starts_with(&format!("{flag}=")))
            {
                anyhow::bail!(tr(
                    &format!(
                        "GPU selection is controlled by the dashboard switches; remove {flag} from the extra arguments"
                    ),
                    &format!(
                        "Выбор карт задаётся переключателями на дашборде; уберите {flag} из дополнительных аргументов"
                    )
                ));
            }
        }
    }
    if let Some(extra) = extra {
        args.extend(extra);
    }

    Ok(args)
}

/// Append the documented local-API flags. This is catalogue data; adding a new
/// miner with the same shape needs no Rust branch.
fn append_api_args(
    args: &mut Vec<String>,
    miner: Option<&catalog::MinerDef>,
    preset: &Preset,
    api_port: u16,
) {
    if api_port == 0 {
        return;
    }
    let (port_flag, enable_flag, worker_flag, worker) = match miner {
        Some(miner) => (
            miner.api_port_flag.as_str(),
            miner.api_enable_flag.as_str(),
            miner.api_worker_flag.as_str(),
            if miner.api_worker_flag.is_empty() {
                ""
            } else {
                preset.worker.as_str()
            },
        ),
        // A custom/imported miner has no catalogue entry. Keep the historical
        // generic API shape when the user explicitly set a port.
        None => ("--api-port", "", "", ""),
    };
    if !enable_flag.is_empty() {
        args.push(enable_flag.to_string());
    }
    if !port_flag.is_empty() {
        args.push(port_flag.to_string());
        args.push(api_port.to_string());
    }
    if !worker_flag.is_empty() && !worker.is_empty() {
        args.push(worker_flag.to_string());
        args.push(worker.to_string());
    }
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
    use crate::config::{Config, GpuTuning, Preset};

    fn cfg_and_preset() -> (Config, Preset) {
        let mut cfg = Config::default();
        let preset = Preset::new_pearl();
        cfg.presets.insert(preset.id.clone(), preset.clone());
        cfg.active_preset = Some(preset.id.clone());
        (cfg, preset)
    }

    fn args(cfg: &Config, preset: &Preset) -> Vec<String> {
        build_args(cfg, preset, Path::new("/tmp/miner.log")).unwrap()
    }

    fn args_with_devices(
        cfg: &Config,
        preset: &Preset,
        devices: Option<&[u32]>,
    ) -> anyhow::Result<Vec<String>> {
        build_args_with_devices(cfg, preset, Path::new("/tmp/miner.log"), devices)
    }

    fn contains_pair(args: &[String], key: &str, value: &str) -> bool {
        args.windows(2).any(|w| w[0] == key && w[1] == value)
    }

    fn gpu(index: u32, uuid: &str) -> GpuStats {
        GpuStats {
            index,
            uuid: uuid.into(),
            ..Default::default()
        }
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
        assert!(contains_pair(&args, "--api-port", "21550"));
        assert!(contains_pair(&args, "--api-rig-name", "3080ti"));
    }

    #[test]
    fn onezero_uses_short_flags_and_default_api_port() {
        let (cfg, mut preset) = cfg_and_preset();
        preset.miner = "onezero".into();
        preset.miner_version = "1.2.0".into();
        preset.worker = "3080ti".into();
        let args = args(&cfg, &preset);
        assert_eq!(args[0], "-a");
        assert!(contains_pair(&args, "-w", "3080ti"));
        assert!(contains_pair(&args, "--api-port", "3001"));
    }

    #[test]
    fn explicit_api_port_overrides_the_catalogue_default() {
        let (cfg, mut preset) = cfg_and_preset();
        preset.api_port = 5555;
        let args = args(&cfg, &preset);
        assert!(contains_pair(&args, "--api-port", "5555"));
    }

    #[test]
    fn zero_api_port_disables_the_api() {
        let (cfg, mut preset) = cfg_and_preset();
        preset.api_port = 0;
        preset.miner = "private-build".into();
        let args = args(&cfg, &preset);
        assert!(!args.iter().any(|arg| arg == "--api-port"));
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

    #[test]
    fn device_flag_is_rendered_for_every_driver_branch() {
        let (cfg, mut preset) = cfg_and_preset();
        let args = args_with_devices(&cfg, &preset, Some(&[0])).unwrap();
        assert!(contains_pair(&args, "--gpu-list", "0"));

        preset.miner = "srbminer".into();
        let args = args_with_devices(&cfg, &preset, Some(&[1])).unwrap();
        assert!(contains_pair(&args, "--gpu-id", "1"));

        preset.miner = "onezero".into();
        let args = args_with_devices(&cfg, &preset, Some(&[0])).unwrap();
        assert!(contains_pair(&args, "--devices", "0"));
    }

    #[test]
    fn device_list_is_comma_separated_in_query_order() {
        let (cfg, preset) = cfg_and_preset();
        let args = args_with_devices(&cfg, &preset, Some(&[0, 2])).unwrap();
        assert!(contains_pair(&args, "--gpu-list", "0,2"));
    }

    #[test]
    fn device_args_come_before_catalogue_options() {
        let (cfg, mut preset) = cfg_and_preset();
        preset
            .miner_options
            .insert("fan_speed".into(), serde_json::json!(70));
        let args = args_with_devices(&cfg, &preset, Some(&[0])).unwrap();
        let device = args.iter().position(|arg| arg == "--gpu-list").unwrap();
        let fan = args
            .iter()
            .position(|arg| arg == "--gpu-fan-speed")
            .unwrap();
        assert!(device < fan);
    }

    #[test]
    fn all_cards_participating_needs_no_device_flag() {
        let (cfg, preset) = cfg_and_preset();
        let gpus = [gpu(0, "GPU-a"), gpu(1, "GPU-b")];
        assert_eq!(device_plan(&cfg, &preset, &gpus).unwrap(), None);
        let args = args(&cfg, &preset);
        assert!(!args.iter().any(|arg| arg == "--gpu-list"));
    }

    #[test]
    fn one_excluded_card_produces_an_explicit_list() {
        let (mut cfg, mut preset) = cfg_and_preset();
        preset.gpu.per_gpu.insert(
            "GPU-b".into(),
            GpuTuning {
                enabled: false,
                ..Default::default()
            },
        );
        cfg.presets.insert(preset.id.clone(), preset.clone());
        let gpus = [gpu(0, "GPU-a"), gpu(1, "GPU-b")];
        assert_eq!(
            device_plan(&cfg, &preset, &gpus).unwrap(),
            Some(vec![0]),
            "only the participating card is passed"
        );
    }

    #[test]
    fn every_card_excluded_is_an_error() {
        let (mut cfg, mut preset) = cfg_and_preset();
        preset.gpu.default.enabled = false;
        cfg.presets.insert(preset.id.clone(), preset.clone());
        let gpus = [gpu(0, "GPU-a"), gpu(1, "GPU-b")];
        let err = device_plan(&cfg, &preset, &gpus).unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn an_unsupported_miner_is_refused_instead_of_mining_on_every_card() {
        let (mut cfg, mut preset) = cfg_and_preset();
        preset.gpu.per_gpu.insert(
            "GPU-b".into(),
            GpuTuning {
                enabled: false,
                ..Default::default()
            },
        );
        preset.miner = "private-build".into();
        cfg.presets.insert(preset.id.clone(), preset.clone());
        let gpus = [gpu(0, "GPU-a"), gpu(1, "GPU-b")];
        let err = device_plan(&cfg, &preset, &gpus).unwrap_err();
        assert!(err.to_string().contains("private-build"), "{err}");
        let err = args_with_devices(&cfg, &preset, Some(&[0])).unwrap_err();
        assert!(err.to_string().contains("private-build"), "{err}");
    }

    #[test]
    fn an_empty_device_list_is_rejected() {
        let (cfg, preset) = cfg_and_preset();
        let err = args_with_devices(&cfg, &preset, Some(&[])).unwrap_err();
        assert!(!err.to_string().is_empty());
    }

    #[test]
    fn a_device_flag_in_extra_args_is_rejected() {
        let (cfg, mut preset) = cfg_and_preset();
        preset.extra_args = "--gpu-list 1".into();
        let err = args_with_devices(&cfg, &preset, Some(&[0])).unwrap_err();
        assert!(err.to_string().contains("--gpu-list"), "{err}");
    }
}
