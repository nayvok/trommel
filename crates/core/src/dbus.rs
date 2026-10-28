//! D-Bus service `io.github.nayvok.Trommel.Daemon`: the single source of truth
//! shared by the daemon, CLI and GUI.

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
use crate::oc::{self, RestoreMode};
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
    oc_busy: Arc<AtomicBool>,
}

/// Rejects a second concurrent GPU operation (Start/Stop/Restore). The guard
/// is released on every return path.
struct BusyGuard {
    flag: Arc<AtomicBool>,
}

impl BusyGuard {
    fn acquire(flag: &Arc<AtomicBool>) -> Option<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .ok()
            .map(|_| Self { flag: flag.clone() })
    }
}

impl Drop for BusyGuard {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::Release);
    }
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
        // A snapshot from a previous boot can never describe this boot's
        // stock state; drop it before any UI/CLI call sees it.
        if crate::oc::cleanup_stale_snapshot() {
            crate::supervisor::log_line("removed a stale OC snapshot from a previous boot");
        }
        Self {
            inner: Arc::new(Mutex::new(Inner {
                cfg,
                sup,
                status,
                last_report: None,
            })),
            running,
            oc_busy: Arc::new(AtomicBool::new(false)),
        }
    }

    fn busy_guard(&self) -> zbus::fdo::Result<BusyGuard> {
        BusyGuard::acquire(&self.oc_busy).ok_or_else(|| {
            zbus::fdo::Error::Failed(
                "another GPU operation is already waiting for authorization".into(),
            )
        })
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

    /// Non-blocking view of the config/state used by the status refresher.
    /// While an OC call waits for authorization the main mutex is held; the
    /// daemon must keep serving the last status instead of freezing.
    pub fn try_snapshot(&self) -> Option<(Config, State)> {
        self.inner
            .try_lock()
            .ok()
            .map(|inner| (inner.cfg.clone(), inner.sup.state.clone()))
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
        let _busy = self.busy_guard()?;
        let mut inner = self.inner.lock().await;
        let cfg = inner.cfg.clone();
        let result = tokio::task::block_in_place(|| inner.sup.start(&cfg));
        self.running
            .store(inner.sup.is_running(), Ordering::Relaxed);
        match result {
            Ok(()) => Ok(()),
            Err(err) => Err(zbus::fdo::Error::Failed(err.to_string())),
        }
    }

    /// Stop mining and restore the pre-mining snapshot.
    async fn stop(&self) -> zbus::fdo::Result<()> {
        let _busy = self.busy_guard()?;
        let mut inner = self.inner.lock().await;
        let cfg = inner.cfg.clone();
        let result = tokio::task::block_in_place(|| inner.sup.stop(&cfg));
        self.running.store(false, Ordering::Relaxed);
        match result {
            Ok(()) => Ok(()),
            Err(err) => {
                // The miner is stopped but the stock restore failed. Keep the
                // snapshot and surface the error instead of pretending success.
                inner.status.state.last_error = Some(err.to_string());
                Err(zbus::fdo::Error::Failed(err.to_string()))
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
        let _busy = self.busy_guard()?;
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
                crate::pool::invalidate_cache();
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
        let _busy = self.busy_guard()?;
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

    /// A read-only snapshot of what Start applied and what Restore would do.
    ///
    /// Safe to call while mining: it only reads files and nvidia-smi. The GUI
    /// uses it to show the exact target values before asking for confirmation.
    async fn oc_report(&self) -> String {
        let (cfg, running) = {
            let inner = self.inner.lock().await;
            (inner.cfg.clone(), inner.sup.is_running())
        };
        build_oc_report(&cfg, running, self.oc_busy.load(Ordering::Acquire))
    }

    /// Restore GPU state. `mode` is `snapshot` (values captured before mining)
    /// or `defaults` (the driver's default power limit and unlocked clocks).
    ///
    /// Returns a JSON report. Refuses while a miner is running.
    async fn restore_oc(&self, mode: &str) -> zbus::fdo::Result<String> {
        let _busy = self.busy_guard()?;
        let mode = RestoreMode::parse(mode.trim()).ok_or_else(|| {
            zbus::fdo::Error::InvalidArgs("mode must be snapshot|defaults".into())
        })?;
        let (running, restore_on_stop) = {
            let inner = self.inner.lock().await;
            (
                inner.sup.is_running(),
                inner.cfg.behavior.restore_stock_on_stop,
            )
        };
        if running {
            return Err(zbus::fdo::Error::Failed(
                "stop mining before restoring the stock profile".into(),
            ));
        }
        let report = tokio::task::block_in_place(|| restore_oc_blocking(mode, restore_on_stop))
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
        Ok(report)
    }

    /// Compatibility wrapper used by older clients: restore the snapshot when
    /// one exists, otherwise reset to the driver defaults.
    async fn reset_oc(&self) -> zbus::fdo::Result<()> {
        self.restore_oc("snapshot").await.map(|_| ())
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
            Event::Started(_) => i18n::tr("Trommel: start", "Trommel: старт"),
            Event::Crashed { .. } => i18n::tr("Trommel: restart", "Trommel: перезапуск"),
            Event::Stopped { .. } => {
                i18n::tr("Trommel: miner stopped", "Trommel: майнер остановлен")
            }
            Event::Error(_) => i18n::tr("Trommel: error", "Trommel: ошибка"),
            Event::Warning(_) => i18n::tr("Trommel: warning", "Trommel: предупреждение"),
            Event::UserStopped => i18n::tr("Trommel: stop", "Trommel: стоп"),
        };
        out.push((summary, body));
    }
    out
}

/// Show a desktop notification through the freedesktop notification service.
///
/// The daemon is the only long-lived process, so it talks to
/// `org.freedesktop.Notifications` itself instead of relying on a shell
/// applet. The call runs in its own task and is bounded by a timeout, so a
/// missing service (CI, a private test bus) or a hung notification daemon can
/// never stall the status or supervisor loops; failures are ignored.
pub fn notify_desktop(connection: &zbus::Connection, summary: &str, body: &str) {
    let connection = connection.clone();
    let summary = summary.to_string();
    let body = body.to_string();
    tokio::spawn(async move {
        let hints = std::collections::HashMap::<&str, zbus::Variant>::new();
        let args = (
            "Trommel",
            0u32,
            crate::APP_ID,
            summary.as_str(),
            body.as_str(),
            &[] as &[&str],
            &hints,
            8000i32,
        );
        let request = connection.call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &args,
        );
        let _ = tokio::time::timeout(Duration::from_secs(2), request).await;
    });
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
    fn oc_report(&self) -> zbus::Result<String>;
    fn restore_oc(&self, mode: &str) -> zbus::Result<String>;
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

fn build_oc_report(cfg: &Config, running: bool, busy: bool) -> String {
    let snapshot = oc::load_snapshot();
    let boot_id = oc::current_boot_id();
    let fresh = snapshot
        .as_ref()
        .filter(|snapshot| snapshot.boot_id == boot_id);
    let stale = snapshot.is_some() && fresh.is_none();
    let current = crate::nvidia::query_all().unwrap_or_default();
    let snapshot_targets: Vec<serde_json::Value> = fresh
        .map(|snapshot| {
            snapshot
                .restore_gpus()
                .into_iter()
                .map(|gpu| {
                    serde_json::json!({
                        "index": gpu.index,
                        "uuid": gpu.uuid,
                        "name": gpu.name,
                        "power_w": gpu.restore_power_w(RestoreMode::Snapshot),
                        "driver_default_w": gpu.power_default_w.map(|w| w.round() as u32),
                        "was_tuned": gpu.was_tuned,
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    let default_targets: Vec<serde_json::Value> = current
        .iter()
        .map(|gpu| {
            serde_json::json!({
                "index": gpu.index,
                "uuid": gpu.uuid,
                "name": gpu.name,
                "power_w": gpu.power_default_w.or(gpu.power_limit_w).map(|w| w.round() as u32),
                "driver_default_w": gpu.power_default_w.map(|w| w.round() as u32),
            })
        })
        .collect();
    serde_json::json!({
        "busy": busy,
        "running": running,
        "restore_on_stop": cfg.behavior.restore_stock_on_stop,
        "snapshot_available": fresh.is_some(),
        "stale": stale,
        "applied": fresh.map(|s| s.applied).unwrap_or(false),
        "needs_restore": fresh
            .map(|s| s.applied && !running)
            .unwrap_or(false),
        "snapshot": snapshot,
        "current": current,
        "targets": {
            "snapshot": snapshot_targets,
            "defaults": default_targets,
        },
    })
    .to_string()
}

fn restore_oc_blocking(mode: RestoreMode, restore_on_stop: bool) -> anyhow::Result<String> {
    let current = oc::current_boot_snapshot();
    let (snapshot, effective_mode) = match (mode, current) {
        (RestoreMode::Snapshot, Some(snapshot)) if snapshot.applied => {
            (snapshot, RestoreMode::Snapshot)
        }
        (RestoreMode::Snapshot, _) => (
            oc::driver_default_snapshot()
                .ok_or_else(|| anyhow::anyhow!("nvidia-smi is not available; cannot reset"))?,
            RestoreMode::Defaults,
        ),
        (RestoreMode::Defaults, _) => (
            oc::driver_default_snapshot()
                .ok_or_else(|| anyhow::anyhow!("nvidia-smi is not available; cannot reset"))?,
            RestoreMode::Defaults,
        ),
    };
    let count = snapshot.restore_gpus().len();
    let targets: Vec<serde_json::Value> = snapshot
        .restore_gpus()
        .into_iter()
        .map(|gpu| {
            serde_json::json!({
                "index": gpu.index,
                "uuid": gpu.uuid,
                "name": gpu.name,
                "power_w": gpu.restore_power_w(effective_mode),
                "was_tuned": gpu.was_tuned,
            })
        })
        .collect();
    let result = oc::restore_snapshot(&snapshot, effective_mode);
    if let Err(ref err) = result {
        oc::record_error(&err.to_string());
    }
    result?;
    let target = match effective_mode {
        RestoreMode::Snapshot => "the pre-mining snapshot",
        RestoreMode::Defaults => "driver defaults",
    };
    Ok(serde_json::json!({
        "ok": true,
        "mode": effective_mode.as_str(),
        "restored": count,
        "restore_on_stop": restore_on_stop,
        "targets": targets,
        "message": format!("Restored {count} GPU(s) to {target}"),
    })
    .to_string())
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
