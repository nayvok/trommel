//! D-Bus service `io.github.nayvok.Trommel.Daemon`: the single source of truth
//! shared by the daemon, CLI/GUI and the GNOME Shell extension.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use tokio::sync::Mutex;
use zbus::interface;
use zbus::object_server::SignalEmitter;

use crate::bench::{self, BenchReport};
use crate::config::{ApplyMode, ChangeClass, Config};
use crate::i18n;
use crate::state::State;
use crate::status::Status;
use crate::supervisor::{Event, Supervisor};

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
        i18n::set_language(cfg.behavior.language);
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

#[interface(name = "io.github.nayvok.Trommel")]
impl Engine {
    /// Start mining (snapshots stock, applies the GPU profile).
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

    /// Stop mining and restore the pre-mining snapshot.
    async fn stop(&self) -> zbus::fdo::Result<()> {
        let mut inner = self.inner.lock().await;
        let cfg = inner.cfg.clone();
        let result = tokio::task::block_in_place(|| inner.sup.stop(&cfg));
        self.running.store(false, Ordering::Relaxed);
        match result {
            Ok(()) => Ok(()),
            Err(err) => {
                inner.status.state.last_error = Some(err.to_string());
                Ok(())
            }
        }
    }

    async fn toggle(&self) -> zbus::fdo::Result<bool> {
        let running = self.running.load(Ordering::Relaxed);
        if running {
            self.stop().await?;
        } else {
            self.start().await?;
        }
        Ok(!running)
    }

    async fn status(&self) -> String {
        self.status_json().await
    }

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

    /// Apply a new config. `mode` is `live`, `restart` or `stop`.
    ///
    /// A running miner whose profile changed cannot be updated live: the call
    /// fails with a clear error so the GUI can offer restart/stop.
    async fn apply_config(&self, json: &str, mode: &str) -> zbus::fdo::Result<String> {
        let next: Config = serde_json::from_str(json)
            .map_err(|e| zbus::fdo::Error::InvalidArgs(format!("config is not valid JSON: {e}")))?;
        let mode = ApplyMode::parse(mode).ok_or_else(|| {
            zbus::fdo::Error::InvalidArgs("mode must be live|restart|stop".into())
        })?;
        let mut inner = self.inner.lock().await;
        let old = inner.cfg.clone();
        let diff = old.diff(&next);
        let was_running = inner.sup.is_running();

        if was_running && diff.class == ChangeClass::RestartRequired && mode == ApplyMode::Live {
            return Err(zbus::fdo::Error::Failed(
                "restart required: GPU/miner settings changed".into(),
            ));
        }

        if was_running && mode != ApplyMode::Live {
            if let Err(err) = tokio::task::block_in_place(|| inner.sup.stop(&old)) {
                return Err(zbus::fdo::Error::Failed(err.to_string()));
            }
        }

        let start_result = if was_running && mode == ApplyMode::Restart {
            tokio::task::block_in_place(|| inner.sup.start(&next))
        } else {
            Ok(())
        };

        match start_result {
            Ok(()) => {
                next.save()
                    .map_err(|e| zbus::fdo::Error::Failed(format!("cannot save config: {e}")))?;
                i18n::set_language(next.behavior.language);
                inner.cfg = next;
                self.running
                    .store(inner.sup.is_running(), Ordering::Relaxed);
                Ok(serde_json::json!({
                    "applied": true,
                    "restarted": was_running && mode == ApplyMode::Restart,
                    "stopped": was_running && mode == ApplyMode::Stop,
                    "changes": diff.reasons,
                })
                .to_string())
            }
            Err(err) => {
                // Keep the old config and make a best effort to restore stock.
                let _ = old.save();
                let _ = tokio::task::block_in_place(|| inner.sup.stop(&old));
                inner.cfg = old;
                self.running.store(false, Ordering::Relaxed);
                Err(zbus::fdo::Error::Failed(err.to_string()))
            }
        }
    }

    /// Compatibility wrapper used by older clients: restart when running.
    async fn set_config(&self, json: &str) -> zbus::fdo::Result<()> {
        self.apply_config(json, "restart").await.map(|_| ())
    }

    async fn run_benchmark(&self, request: &str) -> zbus::fdo::Result<String> {
        let request: BenchRequest = if request.trim().is_empty() {
            BenchRequest {
                profiles: Vec::new(),
                seconds: default_bench_seconds(),
            }
        } else {
            serde_json::from_str(request)
                .map_err(|e| zbus::fdo::Error::InvalidArgs(format!("request is not valid: {e}")))?
        };

        let was_running = self.running.load(Ordering::Relaxed);
        let mut inner = self.inner.lock().await;
        let cfg = inner.cfg.clone();
        let profiles = if request.profiles.is_empty() {
            crate::catalog::miners()
                .iter()
                .filter(|m| {
                    cfg.install(
                        &m.id,
                        &m.versions
                            .first()
                            .map(|v| v.version.clone())
                            .unwrap_or_default(),
                    )
                    .is_some()
                })
                .map(|m| m.id.clone())
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

    /// Restore the snapshot captured before the current mining session.
    async fn reset_oc(&self) -> zbus::fdo::Result<()> {
        let result = tokio::task::block_in_place(|| {
            let Some(snapshot) = crate::oc::load_snapshot() else {
                return Ok(());
            };
            crate::oc::restore_snapshot(&snapshot)
        });
        result.map_err(|e| zbus::fdo::Error::Failed(e.to_string()))
    }

    async fn restore_stock(&self) -> zbus::fdo::Result<()> {
        self.reset_oc().await
    }

    async fn open_app(&self) -> zbus::fdo::Result<()> {
        let app = std::process::Command::new("gtk-launch")
            .arg(crate::APP_ID)
            .spawn();
        match app {
            Ok(_) => Ok(()),
            Err(_) => {
                let fallback = std::process::Command::new("trommel").arg("gui").spawn();
                fallback
                    .map(|_| ())
                    .map_err(|e| zbus::fdo::Error::Failed(format!("cannot open app: {e}")))
            }
        }
    }

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

    #[zbus(property)]
    fn contract_version(&self) -> u32 {
        crate::status::CONTRACT_VERSION
    }

    #[zbus(signal)]
    async fn status_changed(emitter: &SignalEmitter<'_>, json: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn notification(
        emitter: &SignalEmitter<'_>,
        summary: &str,
        body: &str,
    ) -> zbus::Result<()>;
}

impl Engine {
    pub async fn emit_status_changed(emitter: &SignalEmitter<'_>, json: &str) -> zbus::Result<()> {
        Self::status_changed(emitter, json).await
    }

    pub async fn emit_notification(
        emitter: &SignalEmitter<'_>,
        summary: &str,
        body: &str,
    ) -> zbus::Result<()> {
        Self::notification(emitter, summary, body).await
    }
}

pub fn events_to_notifications(events: &[Event]) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for event in events {
        let Some(body) = event.message() else {
            continue;
        };
        let summary = match event {
            Event::Started(_) => "Trommel: start",
            Event::Crashed { .. } => "Trommel: restart",
            Event::Stopped { .. } => "Trommel: miner stopped",
            Event::Error(_) => "Trommel: error",
            Event::UserStopped => "Trommel: stop",
        };
        out.push((summary.to_string(), body));
    }
    out
}

pub fn poll_interval(cfg: &Config) -> Duration {
    Duration::from_secs(cfg.behavior.poll_interval_s.clamp(1, 60))
}

pub async fn fetch_status(connection: &zbus::Connection) -> anyhow::Result<Status> {
    let proxy = MinerProxy::new(connection).await?;
    let json = proxy.refresh().await?;
    Status::from_json(&json)
}

pub fn state_snapshot() -> State {
    State::load().normalize()
}

#[zbus::proxy(
    default_service = "io.github.nayvok.Trommel.Daemon",
    default_path = "/io/github/nayvok/Trommel",
    interface = "io.github.nayvok.Trommel"
)]
pub trait Miner {
    fn start(&self) -> zbus::Result<()>;
    fn stop(&self) -> zbus::Result<()>;
    fn toggle(&self) -> zbus::Result<bool>;
    fn status(&self) -> zbus::Result<String>;
    fn refresh(&self) -> zbus::Result<String>;
    fn get_config(&self) -> zbus::Result<String>;
    fn apply_config(&self, json: &str, mode: &str) -> zbus::Result<String>;
    fn set_config(&self, json: &str) -> zbus::Result<()>;
    fn run_benchmark(&self, request: &str) -> zbus::Result<String>;
    fn reset_oc(&self) -> zbus::Result<()>;
    fn restore_stock(&self) -> zbus::Result<()>;
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

    #[zbus(property)]
    fn contract_version(&self) -> zbus::Result<u32>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_request_defaults() {
        let request: BenchRequest = serde_json::from_str("{}").unwrap();
        assert!(request.profiles.is_empty());
        assert_eq!(request.seconds, 180);
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
    }

    #[test]
    fn poll_interval_is_clamped() {
        let mut cfg = Config::default();
        cfg.behavior.poll_interval_s = 0;
        assert_eq!(poll_interval(&cfg), Duration::from_secs(1));
        cfg.behavior.poll_interval_s = 500;
        assert_eq!(poll_interval(&cfg), Duration::from_secs(60));
    }
}
