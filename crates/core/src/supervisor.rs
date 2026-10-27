//! Miner process management: start, stop, restart-on-crash.
//!
//! Used by the daemon (`pearl-minerd`); deliberately synchronous — the daemon
//! calls it from blocking contexts.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::miners;
use crate::oc;
use crate::paths;
use crate::state::{self, State};
use crate::util;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Miner process spawned, with its PID.
    Started(u32),
    /// Miner exited; a restart is scheduled.
    Crashed { code: Option<i32>, restart_in: u64 },
    /// Miner exited and the auto-restart is disabled.
    Stopped { code: Option<i32> },
    /// Something failed (spawn error, restart failed, OC failed).
    Error(String),
    /// Mining stopped by the user.
    UserStopped,
}

impl Event {
    /// Short human-readable text for notifications.
    pub fn message(&self) -> Option<String> {
        match self {
            Event::Started(_) => Some("Майнинг запущен".into()),
            Event::Crashed { code, restart_in } => Some(format!(
                "Майнер упал (код {}) — перезапуск через {restart_in} с",
                code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
            )),
            Event::Stopped { code } => Some(format!(
                "Майнер остановлен (код {})",
                code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
            )),
            Event::Error(text) => Some(text.clone()),
            Event::UserStopped => {
                Some("Майнинг остановлен, частоты и питание возвращены в сток".into())
            }
        }
    }
}

pub struct Supervisor {
    child: Option<Child>,
    restart_at: Option<Instant>,
    pub state: State,
    pub log_path: PathBuf,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl Supervisor {
    pub fn new() -> Self {
        let state = State::load().normalize();
        Self {
            child: None,
            restart_at: None,
            state,
            log_path: paths::log_path(),
        }
    }

    pub fn is_running(&self) -> bool {
        self.child.is_some() || self.state.running
    }

    /// Start the miner: apply the OC profile, spawn the process, persist state.
    pub fn start(&mut self, cfg: &Config) -> anyhow::Result<()> {
        if self.is_running() {
            return Ok(());
        }
        if self.state.external {
            // Someone else's miner is already mining; nothing to do.
            return Ok(());
        }

        let mut warning: Option<String> = None;
        if let Err(err) = oc::apply(&cfg.gpu) {
            warning = Some(format!("Разгон не применён: {err}"));
        }

        self.spawn(cfg)?;

        self.state.running = true;
        self.state.started_at = Some(util::now_unix());
        self.state.restarts = 0;
        self.state.last_error = warning;
        self.state.profile = Some(cfg.miner_profile.clone());
        let _ = self.state.save();
        crate::pool::invalidate_cache();
        Ok(())
    }

    /// Stop the miner, restore stock clocks/power and forget the state.
    pub fn stop(&mut self, cfg: &Config) -> anyhow::Result<()> {
        self.restart_at = None;
        if let Some(mut child) = self.child.take() {
            terminate(&mut child);
            let _ = child.wait();
        } else if let Some(pid) = self.state.miner_pid {
            if state::pid_matches_miner(pid) {
                terminate_pid(pid);
            }
        }

        let reset_result = oc::reset(&cfg.gpu);

        self.state = State::default();
        State::clear_file();
        crate::pool::invalidate_cache();
        match reset_result {
            Ok(()) => Ok(()),
            Err(err) => Err(anyhow::anyhow!("{err}")),
        }
    }

    /// Periodic check: reap the child, schedule/produce restarts.
    pub fn tick(&mut self, cfg: &Config) -> Vec<Event> {
        let mut events = Vec::new();

        if let Some(mut child) = self.child.take() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let code = status.code();
                    self.state.miner_pid = None;
                    if cfg.auto_restart && self.state.running {
                        self.state.restarts += 1;
                        self.state.last_error = Some(format!(
                            "Майнер завершился (код {}), перезапуск через {} с",
                            code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
                            cfg.restart_delay_s
                        ));
                        let _ = self.state.save();
                        self.restart_at =
                            Some(Instant::now() + Duration::from_secs(cfg.restart_delay_s.max(1)));
                        events.push(Event::Crashed {
                            code,
                            restart_in: cfg.restart_delay_s.max(1),
                        });
                    } else {
                        self.state.running = false;
                        self.state.started_at = None;
                        self.state.last_error = Some(format!(
                            "Майнер завершился (код {}), автоперезапуск выключен",
                            code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
                        ));
                        let _ = self.state.save();
                        events.push(Event::Stopped { code });
                    }
                }
                Ok(None) => {
                    // Still running.
                    self.child = Some(child);
                }
                Err(err) => {
                    events.push(Event::Error(format!("не удалось опросить процесс: {err}")));
                    self.child = Some(child);
                }
            }
        } else if !self.state.running {
            // Detect a miner started behind our back (e.g. start-mining.sh).
            if let Some(pid) = state::find_external_miner() {
                self.state.external = true;
                self.state.running = true;
                self.state.miner_pid = Some(pid);
                self.state.started_at = Some(util::now_unix());
                let _ = self.state.save();
                events.push(Event::Started(pid));
            }
        }

        if let Some(at) = self.restart_at {
            if Instant::now() >= at && self.child.is_none() {
                self.restart_at = None;
                match self.spawn(cfg) {
                    Ok(()) => {
                        events.push(Event::Started(self.state.miner_pid.unwrap_or(0)));
                        let _ = self.state.save();
                    }
                    Err(err) => {
                        self.state.running = false;
                        self.state.last_error =
                            Some(format!("не удалось перезапустить майнер: {err}"));
                        let _ = self.state.save();
                        events.push(Event::Error(
                            self.state.last_error.clone().unwrap_or_default(),
                        ));
                    }
                }
            }
        }
        events
    }

    fn spawn(&mut self, cfg: &Config) -> anyhow::Result<()> {
        let binary = cfg.miner_path();
        if !binary.exists() {
            anyhow::bail!("бинарник майнера не найден: {}", binary.display());
        }
        if let Some(parent) = self.log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)?;
        let log_err = log.try_clone()?;

        let args = miners::build_args(cfg, cfg.miner_profile.trim(), &self.log_path);
        let mut command = Command::new(&binary);
        command
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        if let Some(dir) = binary.parent() {
            command.current_dir(dir);
        }

        let child = command
            .spawn()
            .map_err(|e| anyhow::anyhow!("не удалось запустить {}: {e}", binary.display()))?;
        let pid = child.id();
        self.child = Some(child);
        self.state.miner_pid = Some(pid);
        self.state.running = true;
        self.state.external = false;
        if self.state.started_at.is_none() {
            self.state.started_at = Some(util::now_unix());
        }
        self.state.profile = Some(cfg.miner_profile.clone());
        Ok(())
    }

    /// Take over a miner that is already running (started by a script).
    pub fn adopt_external(&mut self, cfg: &Config) -> bool {
        let Some(pid) = state::find_external_miner() else {
            return false;
        };
        self.state.external = true;
        self.state.running = true;
        self.state.miner_pid = Some(pid);
        self.state.started_at = Some(util::now_unix());
        self.state.profile = Some(cfg.miner_profile.clone());
        true
    }
}

/// SIGTERM, then SIGKILL after a grace period.
fn terminate(child: &mut Child) {
    terminate_pid(child.id());
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
}

/// Send SIGTERM (then SIGKILL) to a PID.
pub fn terminate_pid(pid: u32) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status();
    for _ in 0..50 {
        if !state::pid_matches_miner(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .status();
}

/// Append a line to the daemon log (journald captures stdout as well).
pub fn log_line(message: &str) {
    let path = paths::supervisor_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "[{}] {message}", util::iso8601_utc(util::now_unix()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_messages_are_human_readable() {
        assert_eq!(
            Event::Started(42).message().unwrap(),
            "Майнинг запущен".to_string()
        );
        assert!(Event::Crashed {
            code: Some(1),
            restart_in: 5
        }
        .message()
        .unwrap()
        .contains("перезапуск через 5 с"));
        assert!(Event::UserStopped.message().unwrap().contains("сток"));
    }

    #[test]
    fn start_fails_without_binary() {
        let mut cfg = Config::default();
        if let Some(entry) = cfg.miners.get_mut("wildrig") {
            entry.path = "/nonexistent/wildrig-multi".into();
        }
        let mut supervisor = Supervisor::new();
        supervisor.state = State::default();
        assert!(supervisor.start(&cfg).is_err());
    }

    #[test]
    fn stop_without_running_is_safe() {
        let mut supervisor = Supervisor::new();
        supervisor.state = State::default();
        let cfg = Config::default();
        // No helper installed and nothing running: the OC reset error is
        // reported, but the call does not panic.
        let _ = supervisor.stop(&cfg);
        assert!(!supervisor.state.running);
    }
}
