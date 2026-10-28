//! Overclock/power control and the pre-mining snapshot.
//!
//! All privileged operations go through the root-owned helper
//! `/usr/libexec/trommel/trommel-oc` via `pkexec`. The helper accepts only a
//! GPU UUID and numeric power/clock arguments; no paths or shell fragments.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::GpuTuning;
use crate::nvidia::{self, GpuStats};

pub const HELPER_PATH: &str = "/usr/libexec/trommel/trommel-oc";

pub fn helper_path() -> PathBuf {
    // Test/development hook: lets e2e run a fake helper without root.
    std::env::var_os("TROMMEL_OC_HELPER")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(HELPER_PATH))
}

pub fn helper_installed() -> bool {
    helper_path().exists()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GpuSnapshot {
    pub index: u32,
    pub uuid: String,
    pub name: String,
    pub power_limit_w: Option<f64>,
    pub power_default_w: Option<f64>,
    pub power_min_w: Option<f64>,
    pub power_max_w: Option<f64>,
    pub core_mhz: Option<u32>,
    pub mem_mhz: Option<u32>,
}

impl GpuSnapshot {
    pub fn restore_power_w(&self) -> Option<u32> {
        self.power_limit_w
            .or(self.power_default_w)
            .map(|value| value.round() as u32)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OcSnapshot {
    pub boot_id: String,
    pub created_at: u64,
    pub gpus: Vec<GpuSnapshot>,
}

impl OcSnapshot {
    pub fn for_uuid(&self, uuid: &str) -> Option<&GpuSnapshot> {
        self.gpus.iter().find(|gpu| gpu.uuid == uuid)
    }
}

fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

/// Capture the current stock state of every GPU before applying a profile.
pub fn snapshot() -> anyhow::Result<OcSnapshot> {
    let gpus = nvidia::query_all()?;
    Ok(OcSnapshot {
        boot_id: boot_id(),
        created_at: crate::util::now_unix(),
        gpus: gpus
            .into_iter()
            .map(|gpu| GpuSnapshot {
                index: gpu.index,
                uuid: gpu.uuid,
                name: gpu.name,
                power_limit_w: gpu.power_limit_w,
                power_default_w: gpu.power_default_w,
                power_min_w: gpu.power_min_w,
                power_max_w: gpu.power_max_w,
                core_mhz: gpu.core_mhz,
                mem_mhz: gpu.mem_mhz,
            })
            .collect(),
    })
}

/// Best-effort snapshot when no file exists: restore the driver's default
/// power limit instead of a hardcoded value.
pub fn fallback_snapshot() -> Option<OcSnapshot> {
    let gpus = nvidia::query_all().ok()?;
    Some(OcSnapshot {
        boot_id: boot_id(),
        created_at: crate::util::now_unix(),
        gpus: gpus
            .into_iter()
            .map(|gpu| GpuSnapshot {
                index: gpu.index,
                uuid: gpu.uuid,
                name: gpu.name,
                power_limit_w: gpu.power_default_w.or(gpu.power_limit_w),
                power_default_w: gpu.power_default_w,
                power_min_w: gpu.power_min_w,
                power_max_w: gpu.power_max_w,
                core_mhz: gpu.core_mhz,
                mem_mhz: gpu.mem_mhz,
            })
            .collect(),
    })
}

pub fn snapshot_path() -> PathBuf {
    crate::paths::state_dir().join("oc-snapshot.json")
}

pub fn save_snapshot(snapshot: &OcSnapshot) -> anyhow::Result<()> {
    let path = snapshot_path();
    let text = serde_json::to_string_pretty(snapshot)?;
    crate::config::write_atomic(&path, text.as_bytes())
}

pub fn load_snapshot() -> Option<OcSnapshot> {
    let text = std::fs::read_to_string(snapshot_path()).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn clear_snapshot() {
    let _ = std::fs::remove_file(snapshot_path());
}

/// Drop a snapshot captured before the last reboot.
pub fn cleanup_stale_snapshot() -> bool {
    let Some(snapshot) = load_snapshot() else {
        return false;
    };
    if snapshot.boot_id != boot_id() {
        clear_snapshot();
        return true;
    }
    false
}

pub fn validate_uuid(value: &str) -> Result<(), String> {
    let value = value.trim();
    if !value.starts_with("GPU-") {
        return Err("GPU UUID must start with GPU-".into());
    }
    if value.len() < 8 || value.len() > 64 {
        return Err("GPU UUID has an unexpected length".into());
    }
    if !value
        .chars()
        .all(|c| c.is_ascii_hexdigit() || c == '-' || c == 'G' || c == 'P' || c == 'U')
    {
        return Err("GPU UUID contains invalid characters".into());
    }
    Ok(())
}

fn apply_args(gpu: &GpuStats, tuning: &GpuTuning) -> Vec<String> {
    let mut args = vec!["apply".to_string(), "--gpu".into(), gpu.uuid.clone()];
    if let Some(pl) = tuning.power_limit_w {
        args.extend(["--pl".into(), pl.to_string()]);
    }
    if let Some(core) = tuning.core_clock_mhz {
        args.extend(["--lgc".into(), core.to_string()]);
    }
    if let Some(mem) = tuning.memory_clock_mhz {
        args.extend(["--lmc".into(), mem.to_string()]);
    }
    args
}

fn reset_args(gpu: &GpuSnapshot) -> Vec<String> {
    let mut args = vec!["reset".to_string(), "--gpu".into(), gpu.uuid.clone()];
    if let Some(pl) = gpu.restore_power_w() {
        args.extend(["--pl".into(), pl.to_string()]);
    }
    args
}

pub fn validate_apply_args(args: &[String]) -> Result<(), String> {
    if args.first().map(String::as_str) != Some("apply") {
        return Err("first subcommand must be apply".into());
    }
    let mut i = 1;
    let mut seen = 0;
    let mut gpu_seen = false;
    while i < args.len() {
        let flag = args[i].as_str();
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("{flag}: missing value"))?;
        if flag == "--gpu" {
            validate_uuid(value)?;
            gpu_seen = true;
        } else {
            let number: u32 = value
                .parse()
                .map_err(|_| format!("{flag}: «{value}» is not a number"))?;
            match flag {
                "--pl" if (50..=1000).contains(&number) => {}
                "--lgc" | "--lmc" if (100..=4000).contains(&number) => {}
                "--pl" | "--lgc" | "--lmc" => return Err(format!("{flag}: {number} out of range")),
                other => return Err(format!("unknown flag {other}")),
            }
        }
        seen += 1;
        i += 2;
    }
    if !gpu_seen {
        return Err("--gpu <uuid> is required".into());
    }
    if seen < 2 {
        return Err("at least one tuning parameter is required".into());
    }
    Ok(())
}

pub fn validate_reset_args(args: &[String]) -> Result<(), String> {
    if args.first().map(String::as_str) != Some("reset") {
        return Err("first subcommand must be reset".into());
    }
    let mut i = 1;
    let mut gpu_seen = false;
    let mut pl_seen = false;
    while i < args.len() {
        let flag = args[i].as_str();
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("{flag}: missing value"))?;
        match flag {
            "--gpu" => {
                validate_uuid(value)?;
                gpu_seen = true;
            }
            "--pl" => {
                let pl: u32 = value
                    .parse()
                    .map_err(|_| "--pl: not a number".to_string())?;
                if !(50..=1000).contains(&pl) {
                    return Err("--pl out of range".into());
                }
                pl_seen = true;
            }
            other => return Err(format!("unknown flag {other}")),
        }
        i += 2;
    }
    if !gpu_seen {
        return Err("--gpu <uuid> is required".into());
    }
    let _ = pl_seen;
    Ok(())
}

/// Apply a tuning profile to one GPU.
pub fn apply_tuning(gpu: &GpuStats, tuning: &GpuTuning) -> anyhow::Result<()> {
    let args = apply_args(gpu, tuning);
    if args.len() <= 3 {
        return Ok(());
    }
    validate_apply_args(&args).map_err(|e| anyhow::anyhow!(e))?;
    run_helper(&args)
}

/// Restore one GPU to the snapshot (or driver default power limit).
pub fn restore_gpu(snapshot: &GpuSnapshot) -> anyhow::Result<()> {
    let args = reset_args(snapshot);
    validate_reset_args(&args).map_err(|e| anyhow::anyhow!(e))?;
    run_helper(&args)
}

pub fn restore_snapshot(snapshot: &OcSnapshot) -> anyhow::Result<()> {
    let mut first_error = None;
    for gpu in &snapshot.gpus {
        if let Err(err) = restore_gpu(gpu) {
            if first_error.is_none() {
                first_error = Some(err);
            }
        }
    }
    match first_error {
        Some(err) => Err(err),
        None => {
            clear_snapshot();
            Ok(())
        }
    }
}

/// How long a pkexec call may wait for polkit/authentication before the
/// daemon gives up. The per-user rule makes this instant; the timeout only
/// protects the supervisor from hanging forever when the rule is missing.
const HELPER_TIMEOUT: Duration = Duration::from_secs(30);

fn run_helper(args: &[String]) -> anyhow::Result<()> {
    if !helper_installed() {
        anyhow::bail!(
            "polkit helper is not installed ({}); run the installer",
            HELPER_PATH
        );
    }
    let mut command = if let Some(helper) = std::env::var_os("TROMMEL_OC_HELPER") {
        Command::new(helper)
    } else {
        let mut command = Command::new("pkexec");
        command.arg(HELPER_PATH);
        command
    };
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| anyhow::anyhow!("cannot start OC helper: {e}"))?;
    let deadline = Instant::now() + HELPER_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!(
                    "GPU helper did not answer within {} s; check the polkit rule (scripts/install-polkit-rule.sh)",
                    HELPER_TIMEOUT.as_secs()
                );
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(err) => anyhow::bail!("cannot poll OC helper: {err}"),
        }
    };
    let mut stdout = String::new();
    let mut stderr = String::new();
    if let Some(mut pipe) = child.stdout.take() {
        let _ = pipe.read_to_string(&mut stdout);
    }
    if let Some(mut pipe) = child.stderr.take() {
        let _ = pipe.read_to_string(&mut stderr);
    }
    let stdout = stdout.trim().to_string();
    let stderr = stderr.trim().to_string();
    if status.success() {
        return Ok(());
    }
    let message = if !stderr.is_empty() { stderr } else { stdout };
    let code = status.code().unwrap_or(-1);
    if code == 126 || code == 127 {
        anyhow::bail!("applying GPU settings was cancelled (pkexec, code {code})");
    }
    anyhow::bail!("trommel-oc: {}", first_line(&message))
}

fn first_line(text: &str) -> String {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("error")
        .trim()
        .to_string()
}

/// Read a snapshot file if it belongs to the current boot.
pub fn current_boot_snapshot() -> Option<OcSnapshot> {
    let snapshot = load_snapshot()?;
    if snapshot.boot_id == boot_id() {
        Some(snapshot)
    } else {
        None
    }
}

/// Test helper: write a snapshot to an explicit path.
pub fn save_snapshot_to(path: &Path, snapshot: &OcSnapshot) -> anyhow::Result<()> {
    let text = serde_json::to_string_pretty(snapshot)?;
    crate::config::write_atomic(path, text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(items: &[&str]) -> Vec<String> {
        items.iter().map(|v| v.to_string()).collect()
    }

    fn gpu() -> GpuStats {
        GpuStats {
            index: 0,
            uuid: "GPU-12345678-1234-1234-1234-123456789abc".into(),
            name: "RTX".into(),
            ..Default::default()
        }
    }

    #[test]
    fn builds_apply_args() {
        let tuning = GpuTuning {
            power_limit_w: Some(300),
            core_clock_mhz: Some(1620),
            memory_clock_mhz: Some(810),
            ..Default::default()
        };
        let args = apply_args(&gpu(), &tuning);
        assert_eq!(args[0], "apply");
        assert_eq!(args[1], "--gpu");
        assert!(args.contains(&"--pl".to_string()));
        assert!(validate_apply_args(&args).is_ok());
    }

    #[test]
    fn no_locks_means_no_lgc_lmc() {
        let tuning = GpuTuning {
            power_limit_w: None,
            core_clock_mhz: None,
            memory_clock_mhz: None,
            ..Default::default()
        };
        let args = apply_args(&gpu(), &tuning);
        assert!(!args.contains(&"--pl".to_string()));
        assert!(!args.contains(&"--lgc".to_string()));
        assert!(!args.contains(&"--lmc".to_string()));
    }

    #[test]
    fn memory_lock_is_optional() {
        let tuning = GpuTuning {
            memory_clock_mhz: Some(810),
            ..Default::default()
        };
        let args = apply_args(&gpu(), &tuning);
        assert!(args.windows(2).any(|w| w[0] == "--lmc" && w[1] == "810"));
    }

    #[test]
    fn builds_reset_args() {
        let snapshot = GpuSnapshot {
            index: 0,
            uuid: "GPU-12345678-1234-1234-1234-123456789abc".into(),
            name: "RTX".into(),
            power_limit_w: Some(350.0),
            power_default_w: Some(350.0),
            power_min_w: Some(100.0),
            power_max_w: Some(400.0),
            core_mhz: None,
            mem_mhz: None,
        };
        let args = reset_args(&snapshot);
        assert_eq!(args, s(&["reset", "--gpu", &snapshot.uuid, "--pl", "350"]));
        assert!(validate_reset_args(&args).is_ok());
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(validate_apply_args(&s(&["apply", "--pl", "5"])).is_err());
        assert!(validate_apply_args(&s(&["apply", "--gpu", "nope", "--pl", "300"])).is_err());
        assert!(validate_apply_args(&s(&["apply", "--gpu", "GPU-abc", "--pl", "5"])).is_err());
        assert!(validate_reset_args(&s(&["reset", "--pl", "350"])).is_err());
    }

    #[test]
    fn uuid_validation() {
        assert!(validate_uuid("GPU-12345678-1234-1234-1234-123456789abc").is_ok());
        assert!(validate_uuid("GPU-abc").is_err());
        assert!(validate_uuid("1234").is_err());
    }
}
