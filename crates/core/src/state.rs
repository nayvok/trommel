//! Runtime state (`~/.local/state/pearl-miner/state.json`).

use serde::{Deserialize, Serialize};

use crate::paths;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct State {
    pub running: bool,
    /// PID of the miner process we supervise.
    pub miner_pid: Option<u32>,
    /// Unix timestamp of the first start of the current session.
    pub started_at: Option<u64>,
    /// Automatic restarts since `started_at`.
    pub restarts: u32,
    /// Last error shown in the UI (miner crash, missing permission, ...).
    pub last_error: Option<String>,
    /// The miner runs but was not started by us (e.g. by `start-mining.sh`).
    pub external: bool,
    /// Miner profile id of the running process.
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

    /// Remove the state file (after a clean stop).
    pub fn clear_file() {
        let _ = std::fs::remove_file(paths::state_path());
    }

    /// Uptime in seconds, when running.
    pub fn uptime_s(&self) -> u64 {
        match (self.running, self.started_at) {
            (true, Some(started)) => crate::util::now_unix().saturating_sub(started),
            _ => 0,
        }
    }

    /// `true` when the recorded miner PID is alive *and* is a miner process.
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
        }
        self
    }
}

/// `true` when the process exists and its command line looks like a miner.
pub fn pid_matches_miner(pid: u32) -> bool {
    let cmdline = match std::fs::read(format!("/proc/{pid}/cmdline")) {
        Ok(bytes) => bytes,
        Err(_) => return false,
    };
    let line = String::from_utf8_lossy(&cmdline).replace('\0', " ");
    let lowered = line.to_ascii_lowercase();
    lowered.contains("wildrig")
        || lowered.contains("srbminer")
        || lowered.contains("onezero")
        || lowered.contains("pearlhash")
}

/// Scan `/proc` for an already running Pearl miner (started outside our control).
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
        if let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) {
            let line = String::from_utf8_lossy(&cmdline).replace('\0', " ");
            let lowered = line.to_ascii_lowercase();
            let is_miner = (lowered.contains("wildrig")
                || lowered.contains("srbminer")
                || lowered.contains("onezero"))
                && lowered.contains("pearlhash");
            if is_miner {
                // Prefer the outermost (supervisor) process: lowest PID wins.
                found = Some(found.map_or(pid, |current| current.min(pid)));
            }
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
            miner_pid: Some(4_000_000), // never exists
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
            profile: Some("wildrig".into()),
        };
        let json = serde_json::to_string(&state).unwrap();
        let back: State = serde_json::from_str(&json).unwrap();
        assert_eq!(state, back);
    }
}
