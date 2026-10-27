//! D-Bus service `org.pearl.Miner.Daemon`: the single source of truth shared by the
//! daemon, the CLI/GUI and the GNOME Shell extension.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::Mutex;
use zbus::interface;
use zbus::object_server::SignalEmitter;

use crate::bench::{self, BenchReport};
use crate::config::Config;
use crate::state::State;
use crate::status::Status;
use crate::supervisor::{Event, Supervisor};

/// Mutable daemon state, guarded by a tokio mutex.
pub struct Inner {
    pub cfg: Config,
    pub sup: Supervisor,
    pub status: Status,
    pub last_report: Option<BenchReport>,
}

#[derive(Clone)]
pub struct Engine {
    inner: Arc<Mutex<Inner>>,
    running: Arc<AtomicBool>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        let cfg = Config::load_or_default();
        let sup = Supervisor::new();
        let state = sup.state.clone();
        let status = Status::collect(&cfg, &state);
        let running = Arc::new(AtomicBool::new(status.state.running));
        Self {
            inner: Arc::new(Mutex::new(Inner {
                cfg,
                sup,
                status,
                last_report: None,
            })),
            running,
        }
    }

    pub async fn lock(&self) -> tokio::sync::MutexGuard<'_, Inner> {
        self.inner.lock().await
    }

    /// Replace the cached status (called by the daemon after a refresh).
    pub async fn set_status(&self, status: Status) {
        self.running.store(status.state.running, Ordering::Relaxed);
        self.inner.lock().await.status = status;
    }

    pub fn running_flag(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    pub async fn cfg(&self) -> Config {
        self.inner.lock().await.cfg.clone()
    }

    pub async fn status_json(&self) -> String {
        self.inner.lock().await.status.to_json()
    }
}

#[derive(Debug, Deserialize)]
pub struct BenchRequest {
    #[serde(default)]
    pub profiles: Vec<String>,
    #[serde(default = "default_bench_seconds")]
    pub seconds: u64,
}

fn default_bench_seconds() -> u64 {
    180
}

#[interface(name = "org.pearl.Miner")]
impl Engine {
    /// Start mining (applies the OC profile first).
    async fn start(&self) -> zbus::fdo::Result<()> {
        let mut inner = self.inner.lock().await;
        let cfg = inner.cfg.clone();
        let result = tokio::task::block_in_place(|| inner.sup.start(&cfg));
        match result {
            Ok(()) => {
                self.running.store(true, Ordering::Relaxed);
                Ok(())
            }
            Err(err) => Err(zbus::fdo::Error::Failed(err.to_string())),
        }
    }

    /// Stop mining and restore stock clocks/power.
    async fn stop(&self) -> zbus::fdo::Result<()> {
        let mut inner = self.inner.lock().await;
        let cfg = inner.cfg.clone();
        let result = tokio::task::block_in_place(|| inner.sup.stop(&cfg));
        self.running.store(false, Ordering::Relaxed);
        match result {
            Ok(()) => Ok(()),
            // Mining stopped, only the OC reset failed: report it as a warning
            // through the status instead of failing the whole call.
            Err(err) => {
                inner.status.state.last_error = Some(err.to_string());
                Ok(())
            }
        }
    }

    /// Start when stopped, stop when running.
    async fn toggle(&self) -> zbus::fdo::Result<bool> {
        let running = self.running.load(Ordering::Relaxed);
        if running {
            self.stop().await?;
        } else {
            self.start().await?;
        }
        Ok(!running)
    }

    /// Cached status document (JSON, see [`crate::status::Status`]).
    async fn status(&self) -> String {
        self.status_json().await
    }

    /// Force an immediate refresh and return the new status.
    async fn refresh(&self) -> String {
        let (cfg, state) = {
            let inner = self.inner.lock().await;
            (inner.cfg.clone(), inner.sup.state.clone())
        };
        let status = tokio::task::spawn_blocking(move || Status::collect(&cfg, &state))
            .await
            .unwrap_or_default();
        self.set_status(status).await;
        self.status_json().await
    }

    async fn get_config(&self) -> String {
        let cfg = self.inner.lock().await.cfg.clone();
        serde_json::to_string_pretty(&cfg).unwrap_or_else(|_| "{}".into())
    }

    /// Save a new configuration; a running miner is restarted to pick it up.
    async fn set_config(&self, json: &str) -> zbus::fdo::Result<()> {
        let cfg: Config = serde_json::from_str(json)
            .map_err(|e| zbus::fdo::Error::InvalidArgs(format!("конфиг не разобран: {e}")))?;
        cfg.save()
            .map_err(|e| zbus::fdo::Error::Failed(format!("не удалось сохранить конфиг: {e}")))?;

        let was_running = self.running.load(Ordering::Relaxed);
        let mut inner = self.inner.lock().await;
        inner.cfg = cfg.clone();
        if was_running {
            tokio::task::block_in_place(|| {
                let _ = inner.sup.stop(&cfg);
                let _ = inner.sup.start(&cfg);
            });
        }
        Ok(())
    }

    /// Run the A/B benchmark; mining is paused for the duration.
    async fn run_benchmark(&self, request: &str) -> zbus::fdo::Result<String> {
        let request: BenchRequest = if request.trim().is_empty() {
            BenchRequest {
                profiles: Vec::new(),
                seconds: default_bench_seconds(),
            }
        } else {
            serde_json::from_str(request)
                .map_err(|e| zbus::fdo::Error::InvalidArgs(format!("запрос не разобран: {e}")))?
        };

        let was_running = self.running.load(Ordering::Relaxed);
        let mut inner = self.inner.lock().await;
        let cfg = inner.cfg.clone();
        let profiles = if request.profiles.is_empty() {
            cfg.miners
                .keys()
                .filter(|id| !cfg.miner(id).path.trim().is_empty())
                .cloned()
                .collect::<Vec<_>>()
        } else {
            request.profiles.clone()
        };

        let report = tokio::task::block_in_place(|| {
            if was_running {
                let _ = inner.sup.stop(&cfg);
            }
            let report = bench::run(&cfg, &profiles, request.seconds);
            if was_running {
                let _ = inner.sup.start(&cfg);
            }
            report
        });

        let _ = bench::save(&report);
        let json = report.to_json();
        inner.last_report = Some(report);
        Ok(json)
    }

    /// Restore stock clocks and power limit right now (does not stop mining).
    async fn reset_oc(&self) -> zbus::fdo::Result<()> {
        let inner = self.inner.lock().await;
        let gpu = inner.cfg.gpu.clone();
        let result = tokio::task::block_in_place(|| crate::oc::reset(&gpu));
        match result {
            Ok(()) => Ok(()),
            Err(err) => Err(zbus::fdo::Error::Failed(err.to_string())),
        }
    }

    /// Ask the desktop to open the GUI application.
    async fn open_app(&self) -> zbus::fdo::Result<()> {
        let app = std::process::Command::new("gtk-launch")
            .arg(crate::APP_ID)
            .spawn();
        match app {
            Ok(_) => Ok(()),
            Err(_) => {
                let fallback = std::process::Command::new("pearl-miner").arg("gui").spawn();
                fallback.map(|_| ()).map_err(|e| {
                    zbus::fdo::Error::Failed(format!("не удалось открыть приложение: {e}"))
                })
            }
        }
    }

    /// Quit the daemon (systemd may restart it; the GUI does not use this).
    async fn quit(&self) -> zbus::fdo::Result<()> {
        std::process::exit(0);
    }

    #[zbus(property)]
    fn running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }

    #[zbus(property)]
    fn version(&self) -> String {
        crate::VERSION.to_string()
    }

    /// Pushed to the extension/GUI whenever the status document changes.
    #[zbus(signal)]
    async fn status_changed(emitter: &SignalEmitter<'_>, json: &str) -> zbus::Result<()>;

    /// Human-readable notifications (started, crashed, overheated...).
    #[zbus(signal)]
    async fn notification(
        emitter: &SignalEmitter<'_>,
        summary: &str,
        body: &str,
    ) -> zbus::Result<()>;
}

impl Engine {
    /// Emit `StatusChanged` (used by the daemon; the signal fn itself is
    /// generated privately by the `#[interface]` macro).
    pub async fn emit_status_changed(emitter: &SignalEmitter<'_>, json: &str) -> zbus::Result<()> {
        Self::status_changed(emitter, json).await
    }

    /// Emit a desktop notification request to the extension/GUI.
    pub async fn emit_notification(
        emitter: &SignalEmitter<'_>,
        summary: &str,
        body: &str,
    ) -> zbus::Result<()> {
        Self::notification(emitter, summary, body).await
    }
}

/// Event → notification text helper used by the daemon loop.
pub fn events_to_notifications(events: &[Event]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for event in events {
        let body = match event.message() {
            Some(body) => body,
            None => continue,
        };
        let summary = match event {
            Event::Started(_) => "Pearl Miner: запуск",
            Event::Crashed { .. } => "Pearl Miner: аварийный перезапуск",
            Event::Stopped { .. } => "Pearl Miner: майнер остановлен",
            Event::Error(_) => "Pearl Miner: ошибка",
            Event::UserStopped => "Pearl Miner: стоп",
        };
        out.push((summary.to_string(), body));
    }
    out
}

/// Refresh interval guard shared by the daemon loop and tests.
pub fn poll_interval(cfg: &Config) -> Duration {
    Duration::from_secs(cfg.poll_interval_s.clamp(1, 60))
}

/// Convenience wrapper for clients that want a fresh snapshot without the
/// signal plumbing (used by `pearl-miner status`).
pub async fn fetch_status(connection: &zbus::Connection) -> anyhow::Result<Status> {
    let proxy = MinerProxy::new(connection).await?;
    let json = proxy.refresh().await?;
    Status::from_json(&json)
}

/// Snapshot of the supervisor state for the `status --json` CLI path.
pub fn state_snapshot() -> State {
    State::load().normalize()
}

#[zbus::proxy(
    default_service = "org.pearl.Miner.Daemon",
    default_path = "/org/pearl/Miner",
    interface = "org.pearl.Miner"
)]
pub trait Miner {
    fn start(&self) -> zbus::Result<()>;
    fn stop(&self) -> zbus::Result<()>;
    fn toggle(&self) -> zbus::Result<bool>;
    fn status(&self) -> zbus::Result<String>;
    fn refresh(&self) -> zbus::Result<String>;
    fn get_config(&self) -> zbus::Result<String>;
    fn set_config(&self, json: &str) -> zbus::Result<()>;
    fn run_benchmark(&self, request: &str) -> zbus::Result<String>;
    fn reset_oc(&self) -> zbus::Result<()>;
    fn open_app(&self) -> zbus::Result<()>;
    fn quit(&self) -> zbus::Result<()>;

    #[zbus(signal)]
    fn status_changed(&self, json: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    fn notification(&self, summary: &str, body: &str) -> zbus::Result<()>;

    #[zbus(property)]
    fn running(&self) -> zbus::Result<bool>;

    #[zbus(property)]
    fn version(&self) -> zbus::Result<String>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_request_defaults() {
        let request: BenchRequest = serde_json::from_str("{}").unwrap();
        assert!(request.profiles.is_empty());
        assert_eq!(request.seconds, 180);

        let request: BenchRequest =
            serde_json::from_str(r#"{"profiles":["wildrig"],"seconds":60}"#).unwrap();
        assert_eq!(request.profiles, vec!["wildrig".to_string()]);
        assert_eq!(request.seconds, 60);
    }

    #[test]
    fn notifications_are_grouped_by_event() {
        let events = vec![
            Event::Started(1),
            Event::Crashed {
                code: Some(1),
                restart_in: 5,
            },
        ];
        let messages = events_to_notifications(&events);
        assert_eq!(messages.len(), 2);
        assert!(messages[0].0.contains("запуск"));
        assert!(messages[1].1.contains("перезапуск"));
    }

    #[test]
    fn poll_interval_is_clamped() {
        let mut cfg = Config::default();
        cfg.poll_interval_s = 0;
        assert_eq!(poll_interval(&cfg), Duration::from_secs(1));
        cfg.poll_interval_s = 500;
        assert_eq!(poll_interval(&cfg), Duration::from_secs(60));
    }
}
