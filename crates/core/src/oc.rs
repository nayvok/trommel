//! Overclock/power control through the polkit helper `pearl-oc`.
//!
//! Everything that needs root (`nvidia-smi -pl/-lgc/-lmc/-rgc/-rmc`) goes
//! through a tiny root-owned script installed at `/usr/local/sbin/pearl-oc`,
//! invoked via `pkexec`. A polkit rule allows the active local user to run it
//! without a password prompt; without the rule pkexec shows the standard
//! authentication dialog.

use std::path::PathBuf;
use std::process::Command;

use crate::config::GpuCfg;

/// Absolute path of the installed helper.
pub const HELPER_PATH: &str = "/usr/local/sbin/pearl-oc";

pub fn helper_path() -> PathBuf {
    PathBuf::from(HELPER_PATH)
}

pub fn helper_installed() -> bool {
    helper_path().exists()
}

/// Arguments for `apply`, mirroring `start-mining.sh`.
fn apply_args(gpu: &GpuCfg) -> Vec<String> {
    let mut args = vec!["apply".to_string()];
    if let Some(pl) = gpu.power_limit {
        args.push("--pl".into());
        args.push(pl.to_string());
    }
    if let Some(core) = gpu.core_clock {
        args.push("--lgc".into());
        args.push(core.to_string());
    }
    if let Some(mem) = gpu.memory_clock {
        args.push("--lmc".into());
        args.push(mem.to_string());
    }
    args
}

/// Arguments for `reset`, mirroring `stop-mining.sh`.
fn reset_args(gpu: &GpuCfg) -> Vec<String> {
    vec![
        "reset".to_string(),
        "--pl".to_string(),
        gpu.stock_power_limit.to_string(),
    ]
}

/// Verification rules shared by the helper script and its tests.
pub fn validate_apply_args(args: &[String]) -> Result<(), String> {
    if args.first().map(String::as_str) != Some("apply") {
        return Err("первая подкоманда должна быть apply".into());
    }
    let mut i = 1;
    let mut seen = 0;
    while i < args.len() {
        let flag = args[i].as_str();
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("{flag}: нет значения"))?;
        let number: u32 = value
            .parse()
            .map_err(|_| format!("{flag}: «{value}» не число"))?;
        match flag {
            "--pl" if (50..=1000).contains(&number) => {}
            "--lgc" | "--lmc" if (100..=4000).contains(&number) => {}
            "--pl" | "--lgc" | "--lmc" => return Err(format!("{flag}: {number} вне диапазона")),
            other => return Err(format!("неизвестный флаг {other}")),
        }
        seen += 1;
        i += 2;
    }
    if seen == 0 {
        return Err("нужен хотя бы один параметр".into());
    }
    Ok(())
}

pub fn validate_reset_args(args: &[String]) -> Result<(), String> {
    let expected = ["reset", "--pl"];
    if args.len() != 3 || args[0] != expected[0] || args[1] != expected[1] {
        return Err("использование: reset --pl <Ватт>".into());
    }
    let pl: u32 = args[2].parse().map_err(|_| "--pl: не число".to_string())?;
    if !(50..=1000).contains(&pl) {
        return Err("--pl вне диапазона".into());
    }
    Ok(())
}

/// Apply the mining profile (300 W, locked clocks).
pub fn apply(gpu: &GpuCfg) -> anyhow::Result<()> {
    let args = apply_args(gpu);
    validate_apply_args(&args).map_err(|e| anyhow::anyhow!(e))?;
    if gpu.power_limit.is_none() && gpu.core_clock.is_none() && gpu.memory_clock.is_none() {
        return Ok(());
    }
    run_helper(&args)
}

/// Restore stock behaviour (full power limit, unlocked clocks).
pub fn reset(gpu: &GpuCfg) -> anyhow::Result<()> {
    let args = reset_args(gpu);
    validate_reset_args(&args).map_err(|e| anyhow::anyhow!(e))?;
    run_helper(&args)
}

fn run_helper(args: &[String]) -> anyhow::Result<()> {
    if !helper_installed() {
        anyhow::bail!(
            "polkit-хелпер не установлен ({HELPER_PATH}); запусти install.sh — разгон пропущен"
        );
    }
    let output = Command::new("pkexec")
        .arg(HELPER_PATH)
        .args(args)
        .output()
        .map_err(|e| anyhow::anyhow!("pkexec не запускается: {e}"))?;

    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let message = if !stderr.is_empty() { stderr } else { stdout };
    let code = output.status.code().unwrap_or(-1);
    if code == 126 || code == 127 {
        anyhow::bail!("применение настроек отменено (pkexec, код {code})");
    }
    anyhow::bail!("pearl-oc: {}", first_line(&message));
}

fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("ошибка")
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn builds_apply_args_from_config() {
        let args = apply_args(&GpuCfg::default());
        assert_eq!(
            args,
            s(&["apply", "--pl", "300", "--lgc", "1620", "--lmc", "810"])
        );
        assert!(validate_apply_args(&args).is_ok());
    }

    #[test]
    fn skips_missing_params() {
        let gpu = GpuCfg {
            power_limit: None,
            core_clock: None,
            memory_clock: None,
            ..GpuCfg::default()
        };
        assert_eq!(apply_args(&gpu), s(&["apply"]));
        assert!(validate_apply_args(&apply_args(&gpu)).is_err());
    }

    #[test]
    fn builds_reset_args() {
        let args = reset_args(&GpuCfg::default());
        assert_eq!(args, s(&["reset", "--pl", "350"]));
        assert!(validate_reset_args(&args).is_ok());
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(validate_apply_args(&s(&["apply", "--pl", "5"])).is_err());
        assert!(validate_apply_args(&s(&["apply", "--pl", "abc"])).is_err());
        assert!(validate_apply_args(&s(&["apply", "--evil", "1"])).is_err());
        assert!(validate_apply_args(&s(&["apply", "--pl"])).is_err());
        assert!(validate_apply_args(&s(&["reset", "--pl", "350"])).is_err());
        assert!(validate_reset_args(&s(&["reset", "--pl", "350", "x"])).is_err());
    }
}
