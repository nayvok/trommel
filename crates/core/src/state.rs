//! Runtime state (`~/.local/state/trommel/state.json`).

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::paths;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct State {
    pub running: bool,
    pub miner_pid: Option<u32>,
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
        self.miner_pid.map(pid_matches_miner).unwrap_or(false)
    }

    /// Drop stale data: running flag without a live process is corrected.
    pub fn normalize(mut self) -> Self {
        if self.running && !self.miner_alive() {
            self.running = false;
            self.miner_pid = None;
            self.started_at = None;
            self.restarts = 0;
            self.orphan = false;
        }
        self
    }
}

/// Basenames of the miner binaries we know about.
fn known_binary(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    ["wildrig", "srbminer", "onezero"]
        .iter()
        .any(|needle| name.contains(needle))
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

/// Scan `/proc` for an already running miner started outside our control.
pub fn find_external_miner() -> Option<u32> {
    let entries = std::fs::read_dir("/proc").ok()?;
    let own = std::process::id();
    let mut found: Option<u32> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let pid: u32 = match name.to_string_lossy().parse() {
            Ok(pid) => pid,
            Err(_) => continue,
        };
        if pid == own {
            continue;
        }
        if pid_matches_miner(pid) {
            found = Some(found.map_or(pid, |current| current.min(pid)));
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
            started_at: Some(1),
            restarts: 3,
            ..Default::default()
        };
        let normalized = state.normalize();
        assert!(!normalized.running);
        assert_eq!(normalized.miner_pid, None);
    }

    #[test]
    fn current_process_is_not_a_miner() {
        assert!(!pid_matches_miner(std::process::id()));
    }

    #[test]
    fn round_trip() {
        let state = State {
            running: true,
            miner_pid: Some(123),
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
}
