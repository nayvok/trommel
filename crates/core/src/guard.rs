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
use std::time::{Duration, Instant};

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
///
/// Only an explicit `TROMMEL_GUARD_BIN` or the sibling of the running
/// `trommeld` is accepted. The old `$PATH` fallback could start an unrelated
/// `trommel` (another checkout, a stale package) as our supervisor; a guard
/// that is not the same build is worse than no guard.
fn select_guard_binary(env_bin: Option<PathBuf>, exe: Option<PathBuf>) -> Option<PathBuf> {
    if let Some(path) = env_bin.filter(|path| path.is_file()) {
        return Some(path);
    }
    let exe = exe?;
    let sibling = exe.with_file_name("trommel");
    sibling.is_file().then_some(sibling)
}

pub fn guard_binary() -> Option<PathBuf> {
    select_guard_binary(
        std::env::var_os("TROMMEL_GUARD_BIN").map(PathBuf::from),
        std::env::current_exe().ok(),
    )
}

/// `true` when systemd supervises this process and already runs
/// `ExecStopPost`, so a second in-daemon guard would be redundant.
fn systemd_supervision(invocation_id: Option<&str>, journal_stream: Option<&str>) -> bool {
    invocation_id.is_some() || journal_stream.is_some()
}

/// What happened when the daemon tried to start a guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GuardOutcome {
    Started(u32),
    /// `TROMMEL_NO_GUARD` is set (tests, headless e2e).
    Disabled,
    /// systemd already runs `ExecStopPost` for this unit.
    Supervised,
    /// No trustworthy `trommel` binary was found.
    Unavailable,
    /// The child was spawned but never wrote its ready marker.
    NotReady,
}

/// Is the guard PID file the one this daemon just started?
fn guard_ready(guard: Option<&GuardPid>, daemon_pid: u32, daemon_start: u64) -> bool {
    matches!(
        guard,
        Some(guard)
            if guard.daemon_pid == daemon_pid
                && guard.daemon_start == daemon_start
                && state::proc_start_time(guard.pid) == Some(guard.start)
    )
}

/// Start the guard for this daemon.
///
/// The PID file written by the guard itself is the ready marker: the daemon
/// waits for it (bounded) and only reports a started guard once the file names
/// this daemon and the recorded guard PID is alive. A spawned-but-not-ready
/// child is killed and reported as `NotReady`, so the log never claims a guard
/// that is not actually watching.
pub fn spawn_guard() -> GuardOutcome {
    if std::env::var_os("TROMMEL_NO_GUARD").is_some() {
        return GuardOutcome::Disabled;
    }
    if systemd_supervision(
        std::env::var("INVOCATION_ID").ok().as_deref(),
        std::env::var("JOURNAL_STREAM").ok().as_deref(),
    ) {
        log_line("guard not started: systemd supervises this daemon (ExecStopPost is the net)");
        return GuardOutcome::Supervised;
    }
    let daemon_pid = std::process::id();
    let Some(daemon_start) = state::proc_start_time(daemon_pid) else {
        log_line("guard not started: cannot read /proc/<pid>/stat for the daemon");
        return GuardOutcome::Unavailable;
    };
    let Some(binary) = guard_binary() else {
        log_line(
            "guard not started: no trommel binary next to trommeld (set TROMMEL_GUARD_BIN); \
             a SIGKILL will not be followed by an automatic emergency-stop",
        );
        return GuardOutcome::Unavailable;
    };
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
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            log_line(&format!(
                "guard: cannot start via setsid ({err}); starting without a new session"
            ));
            match Command::new(&binary)
                .arg("guard")
                .arg("--daemon-pid")
                .arg(daemon_pid.to_string())
                .arg("--daemon-start")
                .arg(daemon_start.to_string())
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(child) => child,
                Err(err) => {
                    log_line(&format!(
                        "guard not started: cannot execute {}: {err}",
                        binary.display()
                    ));
                    return GuardOutcome::Unavailable;
                }
            }
        }
    };

    // Ready marker: the guard writes guard.pid before it starts watching. Do
    // not report success before that file names this daemon and its PID is
    // alive; a guard that died at startup must not look like a safety net.
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if guard_ready(load_guard_pid().as_ref(), daemon_pid, daemon_start) {
            let pid = load_guard_pid().map(|guard| guard.pid).unwrap_or(0);
            return GuardOutcome::Started(pid);
        }
        if let Ok(Some(_)) = child.try_wait() {
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    log_line("guard did not report ready within 2 s; not treating it as started");
    GuardOutcome::NotReady
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

    #[test]
    fn guard_binary_ignores_path_and_requires_a_sibling() {
        let root = std::env::temp_dir().join(format!("trommel-guard-bin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let exe = root.join("trommeld");
        std::fs::write(&exe, b"fake").unwrap();

        // No sibling and no explicit override: None, even though `trommel`
        // may exist in $PATH on this machine.
        assert_eq!(select_guard_binary(None, Some(exe.clone())), None);

        let sibling = root.join("trommel");
        std::fs::write(&sibling, b"fake").unwrap();
        assert_eq!(
            select_guard_binary(None, Some(exe.clone())),
            Some(sibling.clone())
        );

        // An explicit override wins and must be a file, not a directory.
        assert_eq!(
            select_guard_binary(Some(sibling.clone()), Some(exe.clone())),
            Some(sibling.clone())
        );
        let sub = root.join("sub");
        std::fs::create_dir_all(&sub).unwrap();
        let lone = sub.join("trommeld");
        std::fs::write(&lone, b"fake").unwrap();
        assert_eq!(select_guard_binary(Some(root.clone()), Some(lone)), None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn systemd_supervision_is_detected_by_environment_markers() {
        assert!(systemd_supervision(Some("abc"), None));
        assert!(systemd_supervision(None, Some("1:2")));
        assert!(!systemd_supervision(None, None));
    }

    #[test]
    fn ready_marker_must_name_this_daemon_and_a_live_guard() {
        let own_pid = std::process::id();
        let own_start = state::proc_start_time(own_pid).unwrap();
        let matching = GuardPid {
            pid: own_pid,
            start: own_start,
            daemon_pid: 100,
            daemon_start: 200,
        };
        assert!(guard_ready(Some(&matching), 100, 200));
        assert!(!guard_ready(Some(&matching), 101, 200));
        assert!(!guard_ready(Some(&matching), 100, 201));
        // A recorded guard whose process is gone (or was replaced) is not a
        // ready guard.
        let dead = GuardPid {
            pid: 4_000_000,
            start: 1,
            daemon_pid: 100,
            daemon_start: 200,
        };
        assert!(!guard_ready(Some(&dead), 100, 200));
        assert!(!guard_ready(None, 100, 200));
    }
}
