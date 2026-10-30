//! `trommel emergency-stop`: the last-resort teardown path.
//!
//! Called by systemd `ExecStopPost`, by the guard process after a `SIGKILL`ed
//! daemon, by startup recovery and manually. It is idempotent: only the miner
//! recorded in `state.json` (matched by absolute binary path) is stopped, and
//! only a current-boot snapshot with `applied = true` is restored. A failed
//! restore keeps both the snapshot and `last_error`, so the next attempt can
//! retry instead of pretending the stock state came back.

use std::path::Path;
use std::time::Duration;

use crate::oc::{self, RestoreMode};
use crate::state::{self, State};
use crate::supervisor::{log_line, terminate_pid};

/// Short timeout for the emergency restore helpers. The normal interactive
/// path allows for a polkit password dialog; a teardown must not hang on one.
pub const EMERGENCY_HELPER_TIMEOUT: Duration = Duration::from_secs(20);
/// How long to wait for an in-flight helper before terminating it.
pub const LEASE_WAIT: Duration = Duration::from_secs(10);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    ServiceStop,
    Guard,
    Startup,
    Manual,
}

impl Reason {
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "service-stop" | "service" | "systemd" => Some(Self::ServiceStop),
            "guard" => Some(Self::Guard),
            "startup" | "recovery" => Some(Self::Startup),
            "manual" | "" => Some(Self::Manual),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ServiceStop => "service-stop",
            Self::Guard => "guard",
            Self::Startup => "startup",
            Self::Manual => "manual",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub stopped_pid: Option<u32>,
    pub restored: usize,
    pub skipped_external: bool,
}

impl Outcome {
    pub fn summary(&self, reason: Reason) -> String {
        let stopped = self
            .stopped_pid
            .map(|pid| format!("pid {pid}"))
            .unwrap_or_else(|| "no miner".into());
        format!(
            "emergency-stop reason={} stopped={} restored={} gpu(s){}",
            reason.as_str(),
            stopped,
            self.restored,
            if self.skipped_external {
                " (external miner left untouched)"
            } else {
                ""
            }
        )
    }
}

/// Startup recovery: stop an orphaned miner from a previous session, restore
/// an applied snapshot and never resume mining. External miners are left
/// alone. A restore failure is returned but does not block daemon startup; the
/// snapshot stays for a later retry.
pub fn startup_recovery() -> anyhow::Result<Outcome> {
    run(Reason::Startup)
}

/// Run the idempotent emergency teardown.
pub fn run(reason: Reason) -> anyhow::Result<Outcome> {
    crate::paths::ensure_dirs().ok();
    // Serialize with the daemon and with another emergency-stop. A live daemon
    // holding the lock for an apply is allowed to finish (bounded by the
    // lock timeout); a dead one releases it immediately.
    let _lock = oc::acquire_lock()?;

    if oc::cleanup_stale_snapshot() {
        log_line("emergency-stop: removed a stale snapshot from a previous boot");
    }

    // An in-flight helper may still write power/clocks. Wait for it, then
    // terminate it before restoring; a late apply must never overwrite the
    // restore. The helper also checks our PID/start-time lease and aborts when
    // the daemon is gone.
    if !oc::wait_for_lease(LEASE_WAIT) {
        let killed = oc::kill_lease_processes();
        log_line(&format!(
            "emergency-stop: in-flight helper did not finish in {} s; terminated {killed} process(es)",
            LEASE_WAIT.as_secs()
        ));
    }

    let cfg = crate::config::Config::load_or_default();
    if !cfg.behavior.restore_stock_on_stop {
        log_line("emergency-stop: restore_stock_on_stop=false; a fail-safe restore still runs");
    }
    let state = State::load();
    let mut outcome = Outcome {
        skipped_external: state.external,
        ..Default::default()
    };
    let mut errors: Vec<String> = Vec::new();

    if state.external {
        log_line("emergency-stop: external miner left untouched");
    }

    let mut miner_still_alive = false;
    if state.running && !state.external {
        if let Some(pid) = state.miner_pid {
            match state.miner_path.as_deref().filter(|path| !path.is_empty()) {
                Some(path) if miner_is_ours(&state, pid) => {
                    terminate_pid(pid, Path::new(path));
                    // `pid_maybe_alive` treats an unreadable `/proc` as
                    // "still there": clearing the state on a read race would
                    // forget a miner that is still running.
                    if state::pid_maybe_alive(pid) {
                        miner_still_alive = true;
                        errors.push(format!("miner pid {pid} did not terminate"));
                    } else {
                        outcome.stopped_pid = Some(pid);
                    }
                }
                Some(_) => {
                    log_line(&format!(
                        "emergency-stop: state PID {pid} does not match the saved binary; not signalling it"
                    ));
                }
                // A legacy state without `miner_path` cannot prove that the
                // PID is ours. Never signal by name: drop the stale record
                // with a warning instead of killing a user's process.
                None => {
                    log_line(&format!(
                        "emergency-stop: legacy state has no miner_path for PID {pid}; not signalling it (ownership is unproven)"
                    ));
                }
            }
        }
    }

    // Clear the state only after the miner is gone. A stuck miner keeps its
    // record so a later attempt still knows what to stop.
    if !miner_still_alive {
        State::clear_file();
    }

    if let Some(snapshot) = oc::current_boot_snapshot().filter(|snapshot| snapshot.applied) {
        outcome.restored = snapshot.restore_gpus().len();
        if let Err(err) = oc::restore_snapshot_locked_with_timeout(
            &snapshot,
            RestoreMode::Snapshot,
            EMERGENCY_HELPER_TIMEOUT,
        ) {
            oc::record_error(&err.to_string());
            errors.push(err.to_string());
        }
    }

    log_line(&outcome.summary(reason));
    if errors.is_empty() {
        Ok(outcome)
    } else {
        anyhow::bail!("{}: {}", outcome.summary(reason), errors.join("; "))
    }
}

fn miner_is_ours(state: &State, pid: u32) -> bool {
    match state.miner_path.as_deref().filter(|path| !path.is_empty()) {
        Some(path) => state::pid_matches_binary(pid, Path::new(path)),
        // No path means no proof. See `State::miner_alive`: the name
        // heuristic is a display hint, never an ownership proof.
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Both tests here write the single test state file, so they must not
    /// overlap; the production path is exercised end-to-end by `e2e.sh`.
    static STATE_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn reasons_parse() {
        assert_eq!(Reason::parse("service-stop"), Some(Reason::ServiceStop));
        assert_eq!(Reason::parse("guard"), Some(Reason::Guard));
        assert_eq!(Reason::parse("startup"), Some(Reason::Startup));
        assert_eq!(Reason::parse("manual"), Some(Reason::Manual));
        assert_eq!(Reason::parse("bogus"), None);
    }

    #[test]
    fn summary_names_the_reason() {
        let outcome = Outcome {
            stopped_pid: Some(42),
            restored: 2,
            skipped_external: false,
        };
        let text = outcome.summary(Reason::Guard);
        assert!(text.contains("reason=guard"));
        assert!(text.contains("pid 42"));
        assert!(text.contains("restored=2"));
    }

    #[test]
    fn emergency_stop_on_empty_state_is_idempotent() {
        let _guard = STATE_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        // The unit-test XDG root is isolated by `paths`; no daemon, no state.
        State::clear_file();
        oc::clear_snapshot();
        let first = run(Reason::Manual);
        assert!(first.is_ok(), "{first:?}");
        let second = run(Reason::Manual);
        assert!(second.is_ok(), "{second:?}");
    }

    #[cfg(unix)]
    #[test]
    fn legacy_state_without_path_is_never_signalled_by_name() {
        use std::io::Write as _;
        let _guard = STATE_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let root =
            std::env::temp_dir().join(format!("trommel-emergency-legacy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let script = root.join("wildrig-legacy.sh");
        let mut file = std::fs::File::create(&script).unwrap();
        writeln!(file, "#!/bin/sh\nsleep 30").unwrap();
        drop(file);
        let mut child = std::process::Command::new("sh")
            .arg(&script)
            .arg("--algo")
            .arg("pearlhash")
            .spawn()
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(150));
        // The process looks exactly like a known miner by name, but the state
        // has no path: ownership is unproven, so no SIGTERM may be sent.
        let legacy = State {
            running: true,
            miner_pid: Some(child.id()),
            miner_path: None,
            external: false,
            ..Default::default()
        };
        legacy.save().unwrap();
        let outcome = run(Reason::Startup).expect("emergency-stop");
        assert!(
            state::pid_alive(child.id()),
            "a legacy state killed a process by name"
        );
        assert_eq!(outcome.stopped_pid, None);
        assert!(
            !crate::paths::state_path().exists(),
            "the stale legacy state must be cleared"
        );
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_dir_all(&root);
    }
}
