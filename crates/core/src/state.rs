//! Runtime state (`~/.local/state/trommel/state.json`).
//!
//! The state file records the miner this installation spawned. Since 0.4.4 it
//! also records the absolute binary path (`miner_path`): a PID alone is not
//! enough to decide whether a process is ours, because PIDs are reused and a
//! custom build may have any file name. Stop/emergency-stop only signal a PID
//! when `/proc/<pid>` still points at the saved binary.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::config::Config;
use crate::paths;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct State {
    pub running: bool,
    pub miner_pid: Option<u32>,
    /// Absolute path of the binary we spawned. Additive field: old state files
    /// parse with `None` and fall back to the legacy name heuristic.
    pub miner_path: Option<String>,
    pub started_at: Option<u64>,
    pub restarts: u32,
    pub last_error: Option<String>,
    /// The miner was started outside of our control (legacy script/manual).
    pub external: bool,
    /// A miner we started before a daemon restart; monitored by PID.
    pub orphan: bool,
    /// Active preset at start time.
    pub preset_id: Option<String>,
    pub miner: Option<String>,
    pub miner_version: Option<String>,
    /// Legacy field kept so old state files parse.
    pub profile: Option<String>,
}

impl State {
    pub fn load() -> Self {
        let path = paths::state_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => serde_json::from_str(&text).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = paths::state_path();
        let text = serde_json::to_string_pretty(self)?;
        crate::config::write_atomic(&path, text.as_bytes())
    }

    pub fn clear_file() {
        let _ = std::fs::remove_file(paths::state_path());
    }

    pub fn uptime_s(&self) -> u64 {
        match (self.running, self.started_at) {
            (true, Some(started)) => crate::util::now_unix().saturating_sub(started),
            _ => 0,
        }
    }

    pub fn miner_alive(&self) -> bool {
        let Some(pid) = self.miner_pid else {
            return false;
        };
        match self.miner_path.as_deref() {
            Some(path) => pid_matches_binary(pid, Path::new(path)),
            // Old state files have no path; keep the conservative name check.
            None => pid_matches_miner(pid),
        }
    }

    /// Drop stale data: running flag without a live process is corrected.
    pub fn normalize(mut self) -> Self {
        if self.running && !self.miner_alive() {
            self.running = false;
            self.miner_pid = None;
            self.miner_path = None;
            self.started_at = None;
            self.restarts = 0;
            self.orphan = false;
            // A phantom "started by a script" state used to keep the UI in
            // "mining (external)" forever and made Start a silent no-op.
            self.external = false;
        }
        self
    }
}

/// Basenames of the miner binaries we have historically shipped.
const LEGACY_MINER_NAMES: [&str; 3] = ["wildrig", "srbminer", "onezero"];

fn known_binary(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    LEGACY_MINER_NAMES
        .iter()
        .any(|needle| name.contains(needle))
}

fn name_matches(name: &str, needles: &[String]) -> bool {
    let name = name.to_ascii_lowercase();
    needles.iter().any(|needle| {
        let needle = needle.trim().to_ascii_lowercase();
        needle.len() >= 3 && name.contains(&needle)
    })
}

fn cmdline_tokens(pid: u32) -> Vec<String> {
    match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(bytes) => String::from_utf8_lossy(&bytes)
            .split('\0')
            .filter(|part| !part.is_empty())
            .map(str::to_string)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Field 22 (`starttime`) of `/proc/<pid>/stat`.
///
/// The `comm` field is parenthesised and may contain spaces or parentheses, so
/// parsing starts after the last `)`. The field after `comm` is field 3, which
/// makes starttime the 20th whitespace-separated token of the remainder.
pub fn proc_start_time(pid: u32) -> Option<u64> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = text.rsplit_once(')')?.1;
    rest.split_whitespace().nth(19)?.parse().ok()
}

/// `true` while `/proc/<pid>` exists (the PID may have been reused).
pub fn pid_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

fn absolute(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf())
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    let left = std::fs::canonicalize(left).unwrap_or_else(|_| absolute(left));
    let right = std::fs::canonicalize(right).unwrap_or_else(|_| absolute(right));
    left == right
}

/// `true` when `pid` is the process we spawned from `path`.
///
/// Compares `/proc/<pid>/exe` (ELF and any other native executable) and the
/// first two `cmdline` tokens (a `#!` script appears as `interpreter script`).
/// A mismatch is never treated as "probably ours": killing a reused PID or a
/// user's own process is worse than leaving a stale state entry.
pub fn pid_matches_binary(pid: u32, path: &Path) -> bool {
    if proc_start_time(pid).is_none() {
        return false;
    }
    if let Ok(exe) = std::fs::read_link(format!("/proc/{pid}/exe")) {
        let text = exe.to_string_lossy();
        let text = text.strip_suffix(" (deleted)").unwrap_or(&text);
        if same_path(Path::new(text), path) {
            return true;
        }
    }
    let tokens = cmdline_tokens(pid);
    tokens
        .iter()
        .take(2)
        .any(|token| same_path(Path::new(token), path))
}

/// `true` when the process exists and its executable looks like a known miner.
pub fn pid_matches_miner(pid: u32) -> bool {
    let tokens = cmdline_tokens(pid);
    if tokens.is_empty() {
        return false;
    }
    let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    if known_binary(
        Path::new(&exe)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(""),
    ) {
        return true;
    }
    // Script-based miners: check the executable/script argument only, never
    // the whole shell command (config JSON contains miner names too).
    let shell = ["bash", "sh", "dash", "zsh", "fish"]
        .iter()
        .any(|name| exe.ends_with(&format!("/{name}")) || exe == *name);
    let candidate = if shell { tokens.get(1) } else { tokens.first() };
    candidate
        .map(|token| {
            known_binary(
                Path::new(token)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(token),
            )
        })
        .unwrap_or(false)
        && tokens
            .iter()
            .any(|token| token.contains("pearlhash") || token.contains("--algo"))
}

fn pid_matches_names(pid: u32, needles: &[String]) -> bool {
    let tokens = cmdline_tokens(pid);
    if tokens.is_empty() {
        return false;
    }
    let exe = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    let exe_name = Path::new(&exe)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("");
    if !exe_name.is_empty() && name_matches(exe_name, needles) {
        return true;
    }
    let shell = ["bash", "sh", "dash", "zsh", "fish"]
        .iter()
        .any(|name| exe.ends_with(&format!("/{name}")) || exe == *name);
    let candidate = if shell { tokens.get(1) } else { tokens.first() };
    candidate
        .map(|token| {
            name_matches(
                Path::new(token)
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or(token),
                needles,
            )
        })
        .unwrap_or(false)
        && tokens
            .iter()
            .any(|token| token.contains("pearlhash") || token.contains("--algo"))
}

/// Names that identify an installed miner, including custom binary/file names.
fn configured_miner_names(cfg: &Config) -> Vec<String> {
    let mut names: Vec<String> = LEGACY_MINER_NAMES.iter().map(|n| n.to_string()).collect();
    for install in cfg.miner_installs.values() {
        let path = Path::new(&install.path);
        for component in [
            path.file_name(),
            path.parent().and_then(Path::file_name),
            path.parent()
                .and_then(Path::parent)
                .and_then(Path::file_name),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(name) = component.to_str() {
                names.push(name.to_string());
            }
        }
        if !install.miner.is_empty() {
            names.push(install.miner.clone());
        }
    }
    names.sort();
    names.dedup();
    names
}

/// Scan `/proc` for an already running miner started outside our control.
///
/// `cfg` contributes the directory/binary names of every installed miner, so a
/// custom build (`my-wildrig-custom`) is found as well as the catalogue names.
/// `exclude_pid` skips a process the caller already owns. The current process
/// is always skipped.
///
/// `TROMMEL_NO_EXTERNAL_SCAN=1` is a test isolation hook: the e2e/UI smoke
/// daemons must not adopt (or stop) the real miner that may be running on the
/// same machine under the same user.
pub fn find_external_miner(cfg: &Config, exclude_pid: Option<u32>) -> Option<u32> {
    external_miner_pids(cfg, exclude_pid).into_iter().min()
}

/// All matching PIDs. The public lookup returns the lowest one; tests use the
/// full list so an unrelated miner already running on the same machine cannot
/// hide the process the test just started.
fn external_miner_pids(cfg: &Config, exclude_pid: Option<u32>) -> Vec<u32> {
    if std::env::var_os("TROMMEL_NO_EXTERNAL_SCAN").is_some() {
        return Vec::new();
    }
    let Some(entries) = std::fs::read_dir("/proc").ok() else {
        return Vec::new();
    };
    let own = std::process::id();
    let needles = configured_miner_names(cfg);
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid: u32 = match name.to_string_lossy().parse() {
            Ok(pid) => pid,
            Err(_) => continue,
        };
        if pid == own || Some(pid) == exclude_pid {
            continue;
        }
        if pid_matches_names(pid, &needles) || pid_matches_miner(pid) {
            found.push(pid);
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_stopped() {
        let state = State::default();
        assert!(!state.running);
        assert_eq!(state.uptime_s(), 0);
        assert!(!state.miner_alive());
    }

    #[test]
    fn stale_state_is_normalized() {
        let state = State {
            running: true,
            miner_pid: Some(4_000_000),
            miner_path: Some("/tmp/does-not-exist/wildrig".into()),
            started_at: Some(1),
            restarts: 3,
            external: true,
            ..Default::default()
        };
        let normalized = state.normalize();
        assert!(!normalized.running);
        assert!(!normalized.external);
        assert_eq!(normalized.miner_pid, None);
        assert_eq!(normalized.miner_path, None);
    }

    #[test]
    fn current_process_is_not_a_miner() {
        assert!(!pid_matches_miner(std::process::id()));
    }

    #[test]
    fn start_time_is_readable_for_self() {
        assert!(proc_start_time(std::process::id()).is_some());
        assert!(pid_alive(std::process::id()));
        assert!(!pid_alive(4_000_000));
    }

    #[test]
    fn elf_matches_by_exe_path_only() {
        let exe = std::fs::read_link("/proc/self/exe").unwrap();
        assert!(pid_matches_binary(std::process::id(), &exe));
        assert!(!pid_matches_binary(
            std::process::id(),
            Path::new("/usr/bin/not-our-binary")
        ));
        assert!(!pid_matches_binary(4_000_000, &exe));
    }

    #[cfg(unix)]
    #[test]
    fn script_matches_by_interpreter_argument() {
        use std::io::Write as _;
        let root = std::env::temp_dir().join(format!("trommel-script-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("my-custom-miner.sh");
        let mut file = std::fs::File::create(&script).unwrap();
        writeln!(file, "#!/bin/sh\nsleep 30").unwrap();
        drop(file);
        let mut child = std::process::Command::new("sh")
            .arg(&script)
            .arg("--algo")
            .arg("pearlhash")
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));
        assert!(pid_matches_binary(child.id(), &script));
        assert!(!pid_matches_binary(child.id(), Path::new("/bin/true")));
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn external_scan_uses_configured_binary_name_and_excludes() {
        use std::io::Write as _;
        let root = std::env::temp_dir().join(format!("trommel-external-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("trommel-test-customminer.sh");
        let mut file = std::fs::File::create(&script).unwrap();
        writeln!(file, "#!/bin/sh\nsleep 30").unwrap();
        drop(file);
        let mut child = std::process::Command::new("sh")
            .arg(&script)
            .arg("--algo")
            .arg("pearlhash")
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(100));

        let mut cfg = Config::default();
        cfg.add_install(crate::config::MinerInstall {
            miner: "custom".into(),
            version: "1.0.0".into(),
            path: script.to_string_lossy().into_owned(),
            ..Default::default()
        });
        // A real miner may already be running on the machine; the test only
        // checks that its own child is in the match set and that excluding it
        // removes it from that set.
        assert!(external_miner_pids(&cfg, None).contains(&child.id()));
        assert!(!external_miner_pids(&cfg, Some(child.id())).contains(&child.id()));

        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn round_trip() {
        let state = State {
            running: true,
            miner_pid: Some(123),
            miner_path: Some(
                "/home/user/.local/share/trommel/miners/wildrig/0.51.8/wildrig-multi".into(),
            ),
            started_at: Some(1_700_000_000),
            restarts: 2,
            last_error: Some("boom".into()),
            external: false,
            orphan: false,
            preset_id: Some("pearl-wildrig".into()),
            miner: Some("wildrig".into()),
            miner_version: Some("0.51.8".into()),
            profile: None,
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: State = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }

    #[test]
    fn old_state_without_miner_path_parses() {
        let text = r#"{"running":false,"miner_pid":null,"started_at":null}"#;
        let state: State = serde_json::from_str(text).unwrap();
        assert_eq!(state.miner_path, None);
        assert!(!state.running);
    }
}
