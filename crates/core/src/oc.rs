//! Overclock/power control and the pre-mining snapshot.
//!
//! All privileged operations go through the root-owned helper
//! `/usr/libexec/trommel/trommel-oc` via `pkexec`. The helper accepts only a
//! GPU UUID and numeric power/clock arguments; no paths or shell fragments.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

#[cfg(unix)]
use std::os::unix::io::AsRawFd;

#[cfg(unix)]
extern "C" {
    fn flock(fd: std::os::raw::c_int, operation: std::os::raw::c_int) -> std::os::raw::c_int;
}

#[cfg(unix)]
const LOCK_EX: std::os::raw::c_int = 2;
#[cfg(unix)]
const LOCK_NB: std::os::raw::c_int = 4;
#[cfg(unix)]
const LOCK_UN: std::os::raw::c_int = 8;

/// How long an apply/restore waits for another process to release the OC lock.
const LOCK_TIMEOUT: Duration = Duration::from_secs(15);

/// Acquire the cross-process GPU lock with the standard timeout.
pub fn acquire_lock() -> anyhow::Result<OcLock> {
    OcLock::acquire(LOCK_TIMEOUT)
}

use crate::config::GpuTuning;
use crate::nvidia::{self, GpuStats};

pub const HELPER_PATH: &str = "/usr/libexec/trommel/trommel-oc";

/// Default wait for pkexec authentication plus the helper itself.
///
/// The old value (30 s) killed pkexec while the polkit password dialog was
/// still open; the journal showed the agent helper failing exactly 30 s after
/// the request. Authentication is interactive, so be generous. A missing
/// polkit rule does not make this wait longer: it simply prompts once.
const DEFAULT_HELPER_TIMEOUT: Duration = Duration::from_secs(180);

pub fn helper_path() -> PathBuf {
    // Test/development hook: lets e2e run a fake helper without root.
    std::env::var_os("TROMMEL_OC_HELPER")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(HELPER_PATH))
}

pub fn helper_installed() -> bool {
    helper_path().exists()
}

fn helper_timeout() -> Duration {
    std::env::var("TROMMEL_OC_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms.max(100)))
        .unwrap_or(DEFAULT_HELPER_TIMEOUT)
}

fn lock_path() -> PathBuf {
    crate::paths::state_dir().join("oc.lock")
}

static OC_LOCK_HELD: AtomicBool = AtomicBool::new(false);

/// Cross-process serialization for every GPU mutation.
///
/// `trommeld`, `trommel oc restore` and `trommel emergency-stop` may all try to
/// touch the cards; the helper must never apply a stale profile after another
/// process restored the snapshot. The lock is an advisory `flock` on
/// `~/.local/state/trommel/oc.lock`, held for the whole apply/restore block.
///
/// A second acquisition in the same process returns an error instead of
/// deadlocking; callers that already hold the lock use the `*_locked` variants.
pub struct OcLock {
    file: File,
}

impl OcLock {
    pub fn acquire(timeout: Duration) -> anyhow::Result<Self> {
        if OC_LOCK_HELD.swap(true, Ordering::AcqRel) {
            anyhow::bail!("nested GPU operation: this process already holds the OC lock");
        }
        let result = Self::acquire_file(timeout);
        if result.is_err() {
            OC_LOCK_HELD.store(false, Ordering::Release);
        }
        result
    }

    fn acquire_file(timeout: Duration) -> anyhow::Result<Self> {
        let path = lock_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&path)
            .map_err(|e| anyhow::anyhow!("cannot open the OC lock {}: {e}", path.display()))?;
        let deadline = Instant::now() + timeout;
        loop {
            #[cfg(unix)]
            {
                let rc = unsafe { flock(file.as_raw_fd(), LOCK_EX | LOCK_NB) };
                if rc == 0 {
                    return Ok(Self { file });
                }
            }
            #[cfg(not(unix))]
            {
                return Ok(Self { file });
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "another GPU operation is still running (waited {} s for the OC lock)",
                    timeout.as_secs()
                );
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for OcLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        unsafe {
            flock(self.file.as_raw_fd(), LOCK_UN);
        }
        OC_LOCK_HELD.store(false, Ordering::Release);
    }
}

/// PID/start-time lease of an in-flight helper.
///
/// The daemon passes its own PID and `/proc/<pid>/stat` start time to the
/// helper (`--lease-pid/--lease-start`), so a helper that was queued before a
/// crash aborts instead of applying clocks to a rig that is being restored.
/// The same file records the spawned `pkexec` PID, which `emergency-stop` waits
/// for and, if necessary, terminates before restoring the snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HelperLease {
    pub daemon_pid: u32,
    pub daemon_start: u64,
    pub helper_pid: u32,
    pub helper_start: u64,
    pub created_at: u64,
}

pub fn lease_path() -> PathBuf {
    crate::paths::state_dir().join("oc-lease.json")
}

/// `--lease-pid/--lease-start` for the current process, when `/proc` allows it.
pub fn helper_lease_args() -> Vec<String> {
    let pid = std::process::id();
    match crate::state::proc_start_time(pid) {
        Some(start) => vec![
            "--lease-pid".into(),
            pid.to_string(),
            "--lease-start".into(),
            start.to_string(),
        ],
        None => Vec::new(),
    }
}

fn write_lease(lease: &HelperLease) {
    let path = lease_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(text) = serde_json::to_string_pretty(lease) {
        let _ = crate::config::write_atomic(&path, text.as_bytes());
    }
}

fn clear_lease() {
    let _ = std::fs::remove_file(lease_path());
}

pub fn load_lease() -> Option<HelperLease> {
    let text = std::fs::read_to_string(lease_path()).ok()?;
    serde_json::from_str(&text).ok()
}

fn lease_process_alive(lease: &HelperLease) -> bool {
    lease.helper_pid != 0
        && crate::state::proc_start_time(lease.helper_pid) == Some(lease.helper_start)
}

/// Wait for an in-flight helper to finish. Returns `true` when it is gone.
pub fn wait_for_lease(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        match load_lease() {
            None => return true,
            Some(lease) if !lease_process_alive(&lease) => {
                clear_lease();
                return true;
            }
            Some(_) if Instant::now() >= deadline => return false,
            Some(_) => std::thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn pid_has_lease(pid: u32, daemon_pid: u32, daemon_start: u64) -> bool {
    let Ok(bytes) = std::fs::read(format!("/proc/{pid}/cmdline")) else {
        return false;
    };
    let tokens: Vec<String> = String::from_utf8_lossy(&bytes)
        .split('\0')
        .map(str::to_string)
        .collect();
    let mut lease_pid = None;
    let mut lease_start = None;
    let mut index = 0;
    while index + 1 < tokens.len() {
        match tokens[index].as_str() {
            "--lease-pid" => lease_pid = tokens[index + 1].parse::<u32>().ok(),
            "--lease-start" => lease_start = tokens[index + 1].parse::<u64>().ok(),
            _ => {}
        }
        index += 1;
    }
    lease_pid == Some(daemon_pid) && lease_start == Some(daemon_start)
}

fn terminate_helper_pid(pid: u32) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        if crate::state::proc_start_time(pid).is_none() {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .status();
}

/// Terminate a helper that outlived its daemon. Only processes carrying the
/// exact lease marker (`--lease-pid`/`--lease-start`) are touched, so an
/// unrelated `nvidia-smi` or a helper from another daemon is never killed.
pub fn kill_lease_processes() -> usize {
    let Some(lease) = load_lease() else {
        return 0;
    };
    let mut victims: Vec<u32> = Vec::new();
    if lease_process_alive(&lease) {
        victims.push(lease.helper_pid);
    }
    if let Ok(entries) = std::fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
                continue;
            };
            if pid == std::process::id() || victims.contains(&pid) {
                continue;
            }
            if pid_has_lease(pid, lease.daemon_pid, lease.daemon_start) {
                victims.push(pid);
            }
        }
    }
    let count = victims.len();
    for pid in victims {
        terminate_helper_pid(pid);
    }
    clear_lease();
    count
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
    /// The card already had a non-default power limit when the snapshot was
    /// taken (manual/legacy tuning). Restore will return to that value unless
    /// the user explicitly asks for driver defaults.
    #[serde(default)]
    pub was_tuned: bool,
}

impl GpuSnapshot {
    fn from_stats(gpu: &GpuStats, prefer_default: bool) -> Self {
        let limit = gpu.power_limit_w;
        let default = gpu.power_default_w;
        let was_tuned = match (limit, default) {
            (Some(limit), Some(default)) => (limit - default).abs() > 1.0,
            _ => false,
        };
        Self {
            index: gpu.index,
            uuid: gpu.uuid.clone(),
            name: gpu.name.clone(),
            power_limit_w: if prefer_default {
                default.or(limit)
            } else {
                limit
            },
            power_default_w: default,
            power_min_w: gpu.power_min_w,
            power_max_w: gpu.power_max_w,
            core_mhz: gpu.core_mhz,
            mem_mhz: gpu.mem_mhz,
            was_tuned,
        }
    }

    /// Power limit to write back. `Snapshot` keeps whatever the card had
    /// before mining; `Defaults` always targets the driver default.
    pub fn restore_power_w(&self, mode: RestoreMode) -> Option<u32> {
        let value = match mode {
            RestoreMode::Snapshot => self.power_limit_w.or(self.power_default_w),
            RestoreMode::Defaults => self.power_default_w.or(self.power_limit_w),
        };
        value.map(|value| value.round() as u32)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RestoreMode {
    /// Restore the values captured before mining.
    #[default]
    Snapshot,
    /// Reset power to the driver default and unlock clocks.
    Defaults,
}

impl RestoreMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "snapshot" => Some(Self::Snapshot),
            "defaults" | "default" => Some(Self::Defaults),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Snapshot => "snapshot",
            Self::Defaults => "defaults",
        }
    }
}

/// The tuning values that were actually requested for one card. Kept in the
/// snapshot so a later restore/runtime check knows what to verify.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AppliedTuning {
    pub power_limit_w: Option<u32>,
    pub core_clock_mhz: Option<u32>,
    pub memory_clock_mhz: Option<u32>,
}

impl From<&GpuTuning> for AppliedTuning {
    fn from(tuning: &GpuTuning) -> Self {
        Self {
            power_limit_w: tuning.power_limit_w,
            core_clock_mhz: tuning.core_clock_mhz,
            memory_clock_mhz: tuning.memory_clock_mhz,
        }
    }
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OcSnapshot {
    pub boot_id: String,
    pub created_at: u64,
    /// Tuning from this snapshot was applied. A snapshot is written with
    /// `applied = true` before the helper runs, so a crash mid-apply still
    /// leaves a restorable record. Old snapshots default to `true`.
    #[serde(default = "default_true")]
    pub applied: bool,
    /// Cards the helper was asked to touch. Empty in old snapshots, which
    /// means "all cards in the snapshot".
    #[serde(default)]
    pub applied_gpus: Vec<String>,
    #[serde(default)]
    pub applied_tuning: BTreeMap<String, AppliedTuning>,
    #[serde(default)]
    pub last_error: Option<String>,
    pub gpus: Vec<GpuSnapshot>,
}

impl OcSnapshot {
    pub fn for_uuid(&self, uuid: &str) -> Option<&GpuSnapshot> {
        self.gpus.iter().find(|gpu| gpu.uuid == uuid)
    }

    /// Cards this snapshot needs to restore. Old snapshots without the list
    /// restore everything they know about.
    pub fn restore_gpus(&self) -> Vec<&GpuSnapshot> {
        if self.applied_gpus.is_empty() {
            self.gpus.iter().collect()
        } else {
            self.gpus
                .iter()
                .filter(|gpu| self.applied_gpus.iter().any(|uuid| uuid == &gpu.uuid))
                .collect()
        }
    }
}

fn boot_id() -> String {
    std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .map(|text| text.trim().to_string())
        .unwrap_or_default()
}

/// Current kernel boot id. Snapshots from another boot are stale.
pub fn current_boot_id() -> String {
    boot_id()
}

/// Capture the current stock state of every GPU before applying a profile.
pub fn snapshot() -> anyhow::Result<OcSnapshot> {
    let gpus = nvidia::query_all()?;
    Ok(OcSnapshot {
        boot_id: boot_id(),
        created_at: crate::util::now_unix(),
        applied: false,
        applied_gpus: Vec::new(),
        applied_tuning: BTreeMap::new(),
        last_error: None,
        gpus: gpus
            .iter()
            .map(|gpu| GpuSnapshot::from_stats(gpu, false))
            .collect(),
    })
}

/// Snapshot whose targets are the driver defaults. Used when there is no
/// usable pre-mining snapshot or the user explicitly asks for defaults.
pub fn driver_default_snapshot() -> Option<OcSnapshot> {
    let gpus = nvidia::query_all().ok()?;
    Some(OcSnapshot {
        boot_id: boot_id(),
        created_at: crate::util::now_unix(),
        applied: false,
        applied_gpus: Vec::new(),
        applied_tuning: BTreeMap::new(),
        last_error: None,
        gpus: gpus
            .iter()
            .map(|gpu| GpuSnapshot::from_stats(gpu, true))
            .collect(),
    })
}

/// Backwards-compatible name for the driver-default snapshot.
pub fn fallback_snapshot() -> Option<OcSnapshot> {
    driver_default_snapshot()
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

/// Persist a restore/apply error next to the snapshot. The snapshot is kept
/// on failure, so the UI and the next attempt can see what went wrong.
pub fn record_error(error: &str) {
    let Some(mut snapshot) = load_snapshot() else {
        return;
    };
    snapshot.last_error = Some(error.to_string());
    let _ = save_snapshot(&snapshot);
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
    args.extend(helper_lease_args());
    args
}

fn reset_args(gpu: &GpuSnapshot, mode: RestoreMode) -> Vec<String> {
    let mut args = vec!["reset".to_string(), "--gpu".into(), gpu.uuid.clone()];
    if let Some(pl) = gpu.restore_power_w(mode) {
        args.extend(["--pl".into(), pl.to_string()]);
    }
    args.extend(helper_lease_args());
    args
}

fn numeric(value: &str, flag: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("{flag}: «{value}» is not a number"))
}

fn validate_lease_arg(
    flag: &str,
    value: &str,
    lease_pid: &mut bool,
    lease_start: &mut bool,
) -> Result<(), String> {
    let number = numeric(value, flag)?;
    match flag {
        "--lease-pid" => {
            if !(1..=4_194_304).contains(&number) {
                return Err(format!("--lease-pid: {number} out of range"));
            }
            *lease_pid = true;
        }
        "--lease-start" => {
            if !(1..=999_999_999_999_999_999).contains(&number) {
                return Err(format!("--lease-start: {number} out of range"));
            }
            *lease_start = true;
        }
        _ => return Err(format!("unknown flag {flag}")),
    }
    Ok(())
}

fn validate_lease_pair(lease_pid: bool, lease_start: bool) -> Result<(), String> {
    if lease_pid != lease_start {
        return Err("--lease-pid and --lease-start must be passed together".into());
    }
    Ok(())
}

pub fn validate_apply_args(args: &[String]) -> Result<(), String> {
    if args.first().map(String::as_str) != Some("apply") {
        return Err("first subcommand must be apply".into());
    }
    let mut i = 1;
    let mut tuning_seen = 0;
    let mut gpu_seen = false;
    let mut lease_pid = false;
    let mut lease_start = false;
    while i < args.len() {
        let flag = args[i].as_str();
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("{flag}: missing value"))?;
        if flag == "--gpu" {
            validate_uuid(value)?;
            gpu_seen = true;
        } else if flag == "--lease-pid" || flag == "--lease-start" {
            validate_lease_arg(flag, value, &mut lease_pid, &mut lease_start)?;
        } else {
            let number = numeric(value, flag)?;
            if number > u32::MAX as u64 {
                return Err(format!("{flag}: {number} out of range"));
            }
            let number = number as u32;
            match flag {
                "--pl" if (50..=1000).contains(&number) => {}
                "--lgc" if (100..=4000).contains(&number) => {}
                "--lmc" if (100..=12000).contains(&number) => {}
                "--pl" | "--lgc" | "--lmc" => return Err(format!("{flag}: {number} out of range")),
                other => return Err(format!("unknown flag {other}")),
            }
            tuning_seen += 1;
        }
        i += 2;
    }
    if !gpu_seen {
        return Err("--gpu <uuid> is required".into());
    }
    if tuning_seen == 0 {
        return Err("at least one tuning parameter is required".into());
    }
    validate_lease_pair(lease_pid, lease_start)?;
    Ok(())
}

pub fn validate_reset_args(args: &[String]) -> Result<(), String> {
    if args.first().map(String::as_str) != Some("reset") {
        return Err("first subcommand must be reset".into());
    }
    let mut i = 1;
    let mut gpu_seen = false;
    let mut lease_pid = false;
    let mut lease_start = false;
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
            "--lease-pid" | "--lease-start" => {
                validate_lease_arg(flag, value, &mut lease_pid, &mut lease_start)?;
            }
            "--pl" => {
                let pl: u32 = value
                    .parse()
                    .map_err(|_| "--pl: not a number".to_string())?;
                if !(50..=1000).contains(&pl) {
                    return Err("--pl out of range".into());
                }
            }
            other => return Err(format!("unknown flag {other}")),
        }
        i += 2;
    }
    if !gpu_seen {
        return Err("--gpu <uuid> is required".into());
    }
    validate_lease_pair(lease_pid, lease_start)?;
    Ok(())
}

/// Read back the applied power limit. This is the one target that is reliable
/// even when the card is idle; clock locks are checked under load by the
/// supervisor.
pub fn verify_power(uuid: &str, target: u32) -> anyhow::Result<f64> {
    let gpus =
        nvidia::query_all().map_err(|e| anyhow::anyhow!("cannot verify the power limit: {e}"))?;
    let gpu = gpus
        .iter()
        .find(|gpu| gpu.uuid == uuid)
        .ok_or_else(|| anyhow::anyhow!("GPU {uuid} disappeared while applying settings"))?;
    let actual = gpu
        .power_limit_w
        .ok_or_else(|| anyhow::anyhow!("GPU {}: nvidia-smi reported no power limit", gpu.index))?;
    if (actual - target as f64).abs() > 1.0 {
        anyhow::bail!(
            "GPU {}: power limit is {actual:.0} W, expected {target} W",
            gpu.index
        );
    }
    Ok(actual)
}

/// Apply a tuning profile to one GPU. Returns an error when the helper fails
/// or when the power limit did not actually take effect.
pub fn apply_tuning(gpu: &GpuStats, tuning: &GpuTuning) -> anyhow::Result<()> {
    let _lock = OcLock::acquire(LOCK_TIMEOUT)?;
    apply_tuning_locked(gpu, tuning)
}

/// Apply with the OC lock already held by the caller.
pub fn apply_tuning_locked(gpu: &GpuStats, tuning: &GpuTuning) -> anyhow::Result<()> {
    let args = apply_args(gpu, tuning);
    if !args
        .iter()
        .any(|arg| arg == "--pl" || arg == "--lgc" || arg == "--lmc")
    {
        return Ok(());
    }
    validate_apply_args(&args).map_err(|e| anyhow::anyhow!(e))?;
    crate::supervisor::log_line(&format!(
        "oc apply gpu={} args={}",
        gpu.index,
        args[1..].join(" ")
    ));
    let output = run_helper(&args)?;
    if let Some(pl) = tuning.power_limit_w {
        verify_power(&gpu.uuid, pl)
            .map_err(|e| anyhow::anyhow!("GPU {}: tuning was not applied: {e}", gpu.index))?;
    }
    crate::supervisor::log_line(&format!(
        "oc apply ok gpu={} {}{}",
        gpu.index,
        first_line(&output.stdout),
        if output.stderr.is_empty() {
            String::new()
        } else {
            format!(" (stderr: {})", first_line(&output.stderr))
        }
    ));
    Ok(())
}

/// Restore one GPU to the snapshot (or the driver default).
pub fn restore_gpu(snapshot: &GpuSnapshot, mode: RestoreMode) -> anyhow::Result<()> {
    let _lock = OcLock::acquire(LOCK_TIMEOUT)?;
    restore_gpu_locked(snapshot, mode)
}

/// Restore one GPU with the OC lock already held by the caller.
pub fn restore_gpu_locked(snapshot: &GpuSnapshot, mode: RestoreMode) -> anyhow::Result<()> {
    restore_gpu_locked_with_timeout(snapshot, mode, helper_timeout())
}

fn restore_gpu_locked_with_timeout(
    snapshot: &GpuSnapshot,
    mode: RestoreMode,
    timeout: Duration,
) -> anyhow::Result<()> {
    let target = snapshot.restore_power_w(mode).ok_or_else(|| {
        anyhow::anyhow!(
            "GPU {}: cannot determine the stock power limit",
            snapshot.index
        )
    })?;
    let args = reset_args(snapshot, mode);
    validate_reset_args(&args).map_err(|e| anyhow::anyhow!(e))?;
    crate::supervisor::log_line(&format!(
        "oc reset gpu={} mode={} args={}",
        snapshot.index,
        mode.as_str(),
        args[1..].join(" ")
    ));
    let output = run_helper_with_timeout(&args, timeout)?;
    verify_power(&snapshot.uuid, target).map_err(|e| {
        anyhow::anyhow!("GPU {}: stock restore was not applied: {e}", snapshot.index)
    })?;
    crate::supervisor::log_line(&format!(
        "oc reset ok gpu={} {}{}",
        snapshot.index,
        first_line(&output.stdout),
        if output.stderr.is_empty() {
            String::new()
        } else {
            format!(" (stderr: {})", first_line(&output.stderr))
        }
    ));
    Ok(())
}

/// Restore every applied card. On success the snapshot is cleared; on any
/// failure it is kept (and the caller records the error) so a retry is
/// possible.
pub fn restore_snapshot(snapshot: &OcSnapshot, mode: RestoreMode) -> anyhow::Result<()> {
    let _lock = OcLock::acquire(LOCK_TIMEOUT)?;
    restore_snapshot_locked(snapshot, mode)
}

/// Restore with a short per-helper timeout, for shutdown and emergency paths.
pub fn restore_snapshot_with_timeout(
    snapshot: &OcSnapshot,
    mode: RestoreMode,
    timeout: Duration,
) -> anyhow::Result<()> {
    let _lock = OcLock::acquire(LOCK_TIMEOUT)?;
    restore_snapshot_locked_with_timeout(snapshot, mode, timeout)
}

/// Restore with the OC lock already held by the caller.
pub fn restore_snapshot_locked(snapshot: &OcSnapshot, mode: RestoreMode) -> anyhow::Result<()> {
    restore_snapshot_locked_with_timeout(snapshot, mode, helper_timeout())
}

/// Restore with the lock held and an explicit helper timeout.
pub fn restore_snapshot_locked_with_timeout(
    snapshot: &OcSnapshot,
    mode: RestoreMode,
    timeout: Duration,
) -> anyhow::Result<()> {
    let gpus = snapshot.restore_gpus();
    if gpus.is_empty() {
        anyhow::bail!("snapshot has no GPUs to restore");
    }
    let mut first_error = None;
    for gpu in gpus {
        if let Err(err) = restore_gpu_locked_with_timeout(gpu, mode, timeout) {
            crate::supervisor::log_line(&format!(
                "oc reset failed gpu={} mode={}: {err}",
                gpu.index,
                mode.as_str()
            ));
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

#[derive(Debug, Clone)]
struct HelperOutput {
    stdout: String,
    stderr: String,
}

fn run_helper(args: &[String]) -> anyhow::Result<HelperOutput> {
    run_helper_with_timeout(args, helper_timeout())
}

fn run_helper_with_timeout(args: &[String], timeout: Duration) -> anyhow::Result<HelperOutput> {
    if !helper_installed() {
        anyhow::bail!(
            "polkit helper is not installed ({}); run the installer",
            HELPER_PATH
        );
    }
    let mut command = if let Some(helper) = std::env::var_os("TROMMEL_OC_HELPER") {
        Command::new(helper)
    } else if is_root() {
        // A daemon started as root (debugging, system service) must not nest
        // pkexec. The helper still validates its arguments and checks uid 0.
        Command::new(helper_path())
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
    let lease = HelperLease {
        daemon_pid: std::process::id(),
        daemon_start: crate::state::proc_start_time(std::process::id()).unwrap_or(0),
        helper_pid: child.id(),
        helper_start: crate::state::proc_start_time(child.id()).unwrap_or(0),
        created_at: crate::util::now_unix(),
    };
    if lease.daemon_start != 0 && lease.helper_start != 0 {
        write_lease(&lease);
    }
    let result = wait_for_helper(&mut child, timeout);
    clear_lease();
    result
}

fn wait_for_helper(
    child: &mut std::process::Child,
    timeout: Duration,
) -> anyhow::Result<HelperOutput> {
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                anyhow::bail!(
                    "GPU helper did not answer within {} s. If the password dialog was open, install the polkit rule once (scripts/install-polkit-rule.sh) and retry.",
                    timeout.as_secs()
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
        return Ok(HelperOutput { stdout, stderr });
    }
    let message = if !stderr.is_empty() { stderr } else { stdout };
    let code = status.code().unwrap_or(-1);
    if code == 126 || code == 127 {
        anyhow::bail!(
            "applying GPU settings was cancelled or authorization failed (pkexec code {code}); install the polkit rule with scripts/install-polkit-rule.sh"
        );
    }
    anyhow::bail!("trommel-oc: {}", first_line(&message))
}

#[cfg(unix)]
fn is_root() -> bool {
    // Avoid a libc dependency: id -u is cheap and available everywhere.
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|text| text.trim() == "0")
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_root() -> bool {
    false
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockKind {
    Core,
    Memory,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LockIssue {
    pub index: u32,
    pub uuid: String,
    pub kind: LockKind,
    pub want: u32,
    pub actual: u32,
}

/// Compare requested clock locks with the driver under load. Power is checked
/// synchronously; clocks need the miner running because an idle GPU may
/// legitimately sit at the locked frequency (or downclock below it).
pub fn runtime_lock_issues(snapshot: &OcSnapshot, gpus: &[GpuStats]) -> Vec<LockIssue> {
    let mut issues = Vec::new();
    for gpu in gpus {
        let Some(applied) = snapshot.applied_tuning.get(&gpu.uuid) else {
            continue;
        };
        if gpu.util_pct.unwrap_or(0) < 80 {
            continue;
        }
        if let (Some(want), Some(have)) = (applied.memory_clock_mhz, gpu.mem_mhz) {
            if want.abs_diff(have) > 100 {
                issues.push(LockIssue {
                    index: gpu.index,
                    uuid: gpu.uuid.clone(),
                    kind: LockKind::Memory,
                    want,
                    actual: have,
                });
            }
        }
        if let (Some(want), Some(have)) = (applied.core_clock_mhz, gpu.core_mhz) {
            if want.abs_diff(have) > 150 {
                issues.push(LockIssue {
                    index: gpu.index,
                    uuid: gpu.uuid.clone(),
                    kind: LockKind::Core,
                    want,
                    actual: have,
                });
            }
        }
    }
    issues
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
    fn high_memory_locks_are_accepted() {
        // The schema and the GUI allow real memory clocks (e.g. 9501 MHz);
        // the Rust validator used to cap them at 4000.
        let tuning = GpuTuning {
            memory_clock_mhz: Some(9501),
            ..Default::default()
        };
        let args = apply_args(&gpu(), &tuning);
        assert!(validate_apply_args(&args).is_ok());
    }

    fn snapshot(power: f64, default: f64) -> GpuSnapshot {
        GpuSnapshot {
            index: 0,
            uuid: "GPU-12345678-1234-1234-1234-123456789abc".into(),
            name: "RTX".into(),
            power_limit_w: Some(power),
            power_default_w: Some(default),
            power_min_w: Some(100.0),
            power_max_w: Some(400.0),
            core_mhz: None,
            mem_mhz: None,
            was_tuned: (power - default).abs() > 1.0,
        }
    }

    #[test]
    fn builds_reset_args() {
        let snapshot = snapshot(350.0, 350.0);
        let args = reset_args(&snapshot, RestoreMode::Snapshot);
        assert_eq!(
            &args[..5],
            &["reset", "--gpu", &snapshot.uuid, "--pl", "350"][..]
        );
        assert!(validate_reset_args(&args).is_ok());
    }

    #[test]
    fn reset_target_depends_on_mode() {
        let snapshot = snapshot(300.0, 350.0);
        assert_eq!(snapshot.restore_power_w(RestoreMode::Snapshot), Some(300));
        assert_eq!(snapshot.restore_power_w(RestoreMode::Defaults), Some(350));
        assert!(snapshot.was_tuned);
    }

    #[test]
    fn rejects_bad_arguments() {
        assert!(validate_apply_args(&s(&["apply", "--pl", "5"])).is_err());
        assert!(validate_apply_args(&s(&["apply", "--gpu", "nope", "--pl", "300"])).is_err());
        assert!(validate_apply_args(&s(&["apply", "--gpu", "GPU-abc", "--pl", "5"])).is_err());
        assert!(validate_reset_args(&s(&["reset", "--pl", "350"])).is_err());
        assert!(validate_apply_args(&s(&[
            "apply",
            "--gpu",
            "GPU-12345678-1234-1234-1234-123456789abc",
            "--lmc",
            "12001"
        ]))
        .is_err());
    }

    #[test]
    fn uuid_validation() {
        assert!(validate_uuid("GPU-12345678-1234-1234-1234-123456789abc").is_ok());
        assert!(validate_uuid("GPU-abc").is_err());
        assert!(validate_uuid("1234").is_err());
    }

    #[test]
    fn old_snapshot_defaults_to_applied_and_all_gpus() {
        let text = r#"{
            "boot_id": "boot",
            "created_at": 1,
            "gpus": [{
                "index": 0,
                "uuid": "GPU-12345678-1234-1234-1234-123456789abc",
                "name": "RTX",
                "power_limit_w": 350.0,
                "power_default_w": 350.0,
                "power_min_w": 100.0,
                "power_max_w": 400.0,
                "core_mhz": 1770,
                "mem_mhz": 810
            }]
        }"#;
        let snapshot: OcSnapshot = serde_json::from_str(text).unwrap();
        assert!(snapshot.applied);
        assert_eq!(snapshot.restore_gpus().len(), 1);
        assert!(!snapshot.gpus[0].was_tuned);
    }

    #[test]
    fn stale_snapshot_is_removed() {
        let snapshot = OcSnapshot {
            boot_id: "definitely-not-this-boot".into(),
            created_at: 1,
            applied: true,
            applied_gpus: Vec::new(),
            applied_tuning: BTreeMap::new(),
            last_error: None,
            gpus: Vec::new(),
        };
        save_snapshot(&snapshot).unwrap();
        assert!(cleanup_stale_snapshot());
        assert!(load_snapshot().is_none());
    }

    #[test]
    fn restore_mode_parses() {
        assert_eq!(RestoreMode::parse("snapshot"), Some(RestoreMode::Snapshot));
        assert_eq!(RestoreMode::parse("defaults"), Some(RestoreMode::Defaults));
        assert_eq!(RestoreMode::parse("nope"), None);
    }

    #[test]
    fn helper_lease_flags_are_validated() {
        let args = s(&[
            "apply",
            "--gpu",
            "GPU-12345678-1234-1234-1234-123456789abc",
            "--pl",
            "300",
            "--lease-pid",
            "1234",
            "--lease-start",
            "5678",
        ]);
        assert!(validate_apply_args(&args).is_ok());
        assert!(validate_apply_args(&s(&[
            "apply",
            "--gpu",
            "GPU-12345678-1234-1234-1234-123456789abc",
            "--pl",
            "300",
            "--lease-pid",
            "0",
            "--lease-start",
            "5678",
        ]))
        .is_err());
        assert!(validate_apply_args(&s(&[
            "apply",
            "--gpu",
            "GPU-12345678-1234-1234-1234-123456789abc",
            "--pl",
            "300",
            "--lease-pid",
            "abc",
            "--lease-start",
            "5678",
        ]))
        .is_err());
        assert!(validate_apply_args(&s(&[
            "apply",
            "--gpu",
            "GPU-12345678-1234-1234-1234-123456789abc",
            "--pl",
            "300",
            "--lease-pid",
            "1234",
        ]))
        .is_err());
        let reset = s(&[
            "reset",
            "--gpu",
            "GPU-12345678-1234-1234-1234-123456789abc",
            "--pl",
            "350",
            "--lease-pid",
            "1234",
            "--lease-start",
            "5678",
        ]);
        assert!(validate_reset_args(&reset).is_ok());
    }

    #[test]
    fn generated_args_carry_a_lease_for_this_process() {
        let args = apply_args(
            &gpu(),
            &GpuTuning {
                power_limit_w: Some(300),
                ..Default::default()
            },
        );
        assert!(validate_apply_args(&args).is_ok());
        assert!(args.windows(2).any(|w| w[0] == "--lease-pid"));
        assert!(args.windows(2).any(|w| w[0] == "--lease-start"));
    }

    #[test]
    fn nested_oc_lock_is_rejected() {
        let root = crate::paths::state_dir();
        let _ = std::fs::create_dir_all(&root);
        let first = OcLock::acquire(Duration::from_secs(1)).unwrap();
        let second = OcLock::acquire(Duration::from_secs(1));
        assert!(second.is_err());
        drop(first);
        assert!(OcLock::acquire(Duration::from_secs(1)).is_ok());
    }
}
