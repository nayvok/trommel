//! Guard process: an independent supervisor for manual daemon runs.
//!
//! systemd stops the unit with `ExecStopPost` even when the daemon is killed,
//! but a daemon started from a terminal has no such net. `trommel guard` is a
//! tiny child of the daemon (started in its own session via `setsid`) that
//! watches the daemon PID **and** `/proc/<pid>/stat` start time. If the daemon
//! disappears without disarming the guard, the guard runs
//! `trommel emergency-stop --reason=guard`.
//!
//! The guard is disarmed with a token that names the exact daemon
//! (`guard.disarm`); a new daemon does not delete an old token, so a stale
//! guard can never mistake the new daemon for the one it was watching.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::emergency::{self, Reason};
use crate::paths;
use crate::state;
use crate::supervisor::log_line;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuardPid {
    pub pid: u32,
    pub start: u64,
    pub daemon_pid: u32,
    pub daemon_start: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisarmToken {
    pub daemon_pid: u32,
    pub daemon_start: u64,
}

pub fn guard_pid_path() -> PathBuf {
    paths::state_dir().join("guard.pid")
}

pub fn disarm_path() -> PathBuf {
    paths::state_dir().join("guard.disarm")
}

fn write_json<T: Serialize>(path: &Path, value: &T) {
    if let Ok(text) = serde_json::to_string_pretty(value) {
        let _ = crate::config::write_atomic(path, text.as_bytes());
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn load_guard_pid() -> Option<GuardPid> {
    read_json(&guard_pid_path())
}

fn read_disarm() -> Option<DisarmToken> {
    read_json(&disarm_path())
}

fn disarm_matches(token: Option<DisarmToken>, daemon_pid: u32, daemon_start: u64) -> bool {
    matches!(
        token,
        Some(token) if token.daemon_pid == daemon_pid && token.daemon_start == daemon_start
    )
}

/// Locate the `trommel` binary that owns the hidden `guard` subcommand.
pub fn guard_binary() -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("TROMMEL_GUARD_BIN") {
        let path = PathBuf::from(value);
        if path.exists() {
            return Some(path);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        let sibling = exe.with_file_name("trommel");
        if sibling.exists() {
            return Some(sibling);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            let candidate = dir.join("trommel");
            if candidate.exists() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Start the guard for this daemon. Returns the guard PID when it was started.
///
/// `TROMMEL_NO_GUARD=1` disables the guard (tests and headless e2e).
pub fn spawn_guard() -> Option<u32> {
    if std::env::var_os("TROMMEL_NO_GUARD").is_some() {
        return None;
    }
    let daemon_pid = std::process::id();
    let daemon_start = state::proc_start_time(daemon_pid)?;
    let binary = guard_binary()?;
    // Startup recovery already handled the previous session. A guard left by
    // that session must not fire a second emergency-stop while this daemon is
    // serving clients.
    if let Some(old) = load_guard_pid() {
        if state::proc_start_time(old.pid) == Some(old.start) {
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(old.pid.to_string())
                .status();
        }
    }
    let _ = std::fs::remove_file(guard_pid_path());

    let mut command = Command::new("setsid");
    command
        .arg(&binary)
        .arg("guard")
        .arg("--daemon-pid")
        .arg(daemon_pid.to_string())
        .arg("--daemon-start")
        .arg(daemon_start.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    match command.spawn() {
        Ok(child) => Some(child.id()),
        Err(err) => {
            log_line(&format!(
                "guard: cannot start via setsid ({err}); starting without a new session"
            ));
            Command::new(&binary)
                .arg("guard")
                .arg("--daemon-pid")
                .arg(daemon_pid.to_string())
                .arg("--daemon-start")
                .arg(daemon_start.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .ok()
                .map(|child| child.id())
        }
    }
}

/// Disarm and, when possible, stop the guard during a graceful shutdown.
///
/// The token is what prevents a double restore; signalling the PID is only an
/// optimisation to avoid leaving a process behind for a second.
pub fn disarm_and_stop() {
    let daemon_pid = std::process::id();
    let Some(daemon_start) = state::proc_start_time(daemon_pid) else {
        return;
    };
    write_json(
        &disarm_path(),
        &DisarmToken {
            daemon_pid,
            daemon_start,
        },
    );
    if let Some(guard) = load_guard_pid() {
        if guard.daemon_pid == daemon_pid
            && guard.daemon_start == daemon_start
            && state::proc_start_time(guard.pid) == Some(guard.start)
        {
            let _ = Command::new("kill")
                .arg("-TERM")
                .arg(guard.pid.to_string())
                .status();
        }
    }
    let _ = std::fs::remove_file(guard_pid_path());
}

/// Guard loop. Blocks until the daemon is gone and disarmed, or after running
/// the emergency stop.
pub fn run(daemon_pid: u32, daemon_start: u64) -> anyhow::Result<()> {
    if daemon_pid == 0 || daemon_start == 0 {
        anyhow::bail!("guard: --daemon-pid and --daemon-start must be positive");
    }
    let own_pid = std::process::id();
    let own_start = state::proc_start_time(own_pid).unwrap_or(0);
    if own_start != 0 {
        write_json(
            &guard_pid_path(),
            &GuardPid {
                pid: own_pid,
                start: own_start,
                daemon_pid,
                daemon_start,
            },
        );
    }
    log_line(&format!(
        "guard: watching daemon pid={daemon_pid} start={daemon_start}"
    ));

    loop {
        std::thread::sleep(Duration::from_millis(500));
        if disarm_matches(read_disarm(), daemon_pid, daemon_start) {
            log_line("guard: disarmed by a graceful daemon shutdown");
            break;
        }
        if state::proc_start_time(daemon_pid) == Some(daemon_start) {
            continue;
        }
        // The daemon is gone. Give a normal exit a moment to write the token.
        std::thread::sleep(Duration::from_secs(2));
        if disarm_matches(read_disarm(), daemon_pid, daemon_start) {
            log_line("guard: disarmed during the shutdown grace period");
            break;
        }
        log_line(&format!(
            "guard: daemon pid={daemon_pid} disappeared; running emergency-stop"
        ));
        match emergency::run(Reason::Guard) {
            Ok(outcome) => log_line(&outcome.summary(Reason::Guard)),
            Err(err) => log_line(&format!("guard: emergency-stop failed: {err}")),
        }
        break;
    }

    // Remove the PID file only if it still describes this guard; a newer
    // daemon may already have replaced it.
    if let Some(guard) = load_guard_pid() {
        if guard.pid == own_pid && guard.start == own_start {
            let _ = std::fs::remove_file(guard_pid_path());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disarm_token_matches_only_its_daemon() {
        let token = Some(DisarmToken {
            daemon_pid: 10,
            daemon_start: 20,
        });
        assert!(disarm_matches(token, 10, 20));
        assert!(!disarm_matches(token, 11, 20));
        assert!(!disarm_matches(token, 10, 21));
        assert!(!disarm_matches(None, 10, 20));
    }
}
