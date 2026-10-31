//! `trommeld` — the background service.
//!
//! Owns the miner process, applies the OC profile, polls GPU + miner + pool and
//! pushes the status document over D-Bus (`io.github.nayvok.Trommel.Daemon`).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Notify;
use trommel_core::dbus::{self, Engine};
use trommel_core::i18n::tr;
use trommel_core::status::Status;
use trommel_core::supervisor::log_line;
use trommel_core::{paths, OBJECT_PATH};

mod tray;

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    paths::ensure_dirs().ok();

    // Final net after a crash: stop an orphaned miner from the previous
    // session, restore an applied snapshot, and never resume mining. This runs
    // before the D-Bus name is owned, so a client can not observe a rig that
    // is halfway through recovery.
    match trommel_core::emergency::startup_recovery() {
        Ok(outcome) => log_line(&format!(
            "startup: {}",
            outcome.summary(trommel_core::emergency::Reason::Startup)
        )),
        Err(err) => log_line(&format!("startup recovery did not complete: {err}")),
    }

    let engine = Engine::new();
    let connection = zbus::connection::Builder::session()
        .name(trommel_core::BUS_NAME)
        .serve_at(OBJECT_PATH, engine.clone())
        .build()
        .await?;

    let iface_ref = connection
        .object_server()
        .interface::<_, Engine>(OBJECT_PATH)
        .await?;

    // Independent supervisor for manual runs: it survives our SIGKILL and
    // runs emergency-stop if we disappear without disarming it. A guard is
    // only reported when its ready marker proves it is watching this daemon.
    match trommel_core::guard::spawn_guard() {
        trommel_core::guard::GuardOutcome::Started(pid) => {
            log_line(&format!("guard started pid={pid}"))
        }
        trommel_core::guard::GuardOutcome::Disabled => {
            log_line("guard not started (TROMMEL_NO_GUARD set)")
        }
        trommel_core::guard::GuardOutcome::Supervised => {
            log_line("guard not started (systemd supervises the daemon)")
        }
        trommel_core::guard::GuardOutcome::Unavailable => {
            log_line("guard warning: no usable trommel binary; a SIGKILL will not restore stock")
        }
        trommel_core::guard::GuardOutcome::NotReady => {
            log_line("guard warning: the child did not become ready; not treated as started")
        }
    }

    log_line("сервис запущен");
    println!(
        "trommeld {} запущен на {}",
        trommel_core::VERSION,
        OBJECT_PATH
    );

    let (refresh_tx, mut refresh_rx) = tokio::sync::mpsc::channel::<()>(16);
    let quit = Arc::new(Notify::new());

    // The tray belongs to the daemon: it exists without an open window and
    // controls the miner through the same Engine path as D-Bus.
    let tray = if std::env::var_os("TROMMEL_NO_TRAY").is_some() {
        log_line("tray not started (TROMMEL_NO_TRAY set)");
        None
    } else {
        let cfg = engine.cfg().await;
        match tray::spawn(
            engine.clone(),
            connection.clone(),
            refresh_tx.clone(),
            quit.clone(),
            cfg.behavior.tray_monochrome,
        )
        .await
        {
            Ok(handle) => {
                log_line("tray started");
                Some(handle)
            }
            Err(err) => {
                log_line(&format!("tray not started: {err}"));
                None
            }
        }
    };

    // Status refresher: periodic polling plus on-demand refresh requests.
    let refresher = {
        let engine = engine.clone();
        let iface_ref = iface_ref.clone();
        let connection = connection.clone();
        let tray = tray.clone();
        tokio::spawn(async move {
            let mut last_json = String::new();
            let mut overheated = false;

            loop {
                let (cfg, state) = match engine.try_snapshot() {
                    Some(snapshot) => snapshot,
                    None => {
                        // A Start/Stop/Restore is waiting for the polkit
                        // dialog; keep the previous status and retry shortly.
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        continue;
                    }
                };
                let interval = dbus::poll_interval(&cfg);
                let temp_limit = cfg.behavior.temp_limit_c;
                let notify_enabled = cfg.behavior.notifications;
                let tray_cfg = cfg.clone();

                let status = match tokio::task::spawn_blocking(move || {
                    Status::collect(&cfg, &state)
                })
                .await
                {
                    Ok(status) => status,
                    Err(err) => {
                        log_line(&format!("сбор статуса упал: {err}"));
                        Status::default()
                    }
                };

                let json = status.to_json();
                if json != last_json {
                    last_json = json.clone();
                    let _ = Engine::emit_status_changed(iface_ref.signal_emitter(), &json).await;
                }
                engine.set_status(status.clone()).await;
                if let Some(tray) = &tray {
                    tray::sync_with_timeout(
                        tray,
                        &status,
                        tray_cfg.behavior.tray_monochrome,
                        engine.is_busy(),
                    )
                    .await;
                }

                // Overheat notification, once per episode. The hot card can be
                // any card: on a two-GPU rig the duty card is often the second
                // one, and reading only `status.gpu` silenced the alert.
                let hot = status
                    .hottest_gpu()
                    .filter(|gpu| gpu.temp_c.unwrap_or_default() >= temp_limit);
                if let (Some(gpu), true) = (&hot, notify_enabled) {
                    if !overheated {
                        overheated = true;
                        let temp = gpu.temp_c.unwrap_or_default();
                        let index = gpu.index;
                        let summary = tr("Trommel: overheat", "Trommel: перегрев");
                        let body = format!(
                            "{} {temp} °C ({} #{index}, {} {temp_limit} °C). {}",
                            tr("Temperature reached", "Температура достигла"),
                            tr("card", "карта"),
                            tr("limit", "порог"),
                            tr("The miner will pause.", "Майнер поставит паузу."),
                        );
                        let _ =
                            Engine::emit_notification(iface_ref.signal_emitter(), &summary, &body)
                                .await;
                        dbus::notify_desktop(&connection, &summary, &body);
                    }
                } else if hot.is_none() && overheated {
                    overheated = false;
                }

                tokio::select! {
                    _ = tokio::time::sleep(interval) => {}
                    Some(()) = refresh_rx.recv() => {}
                }
            }
        })
    };

    // Supervisor loop: reaps the miner, restarts it, detects foreign miners.
    let supervisor_loop = {
        let engine = engine.clone();
        let iface_ref = iface_ref.clone();
        let connection = connection.clone();
        let refresh_tx = refresh_tx.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(Duration::from_secs(1));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                ticker.tick().await;
                let events = {
                    let mut inner = engine.lock().await;
                    let cfg = inner.cfg.clone();
                    tokio::task::block_in_place(|| inner.sup.tick(&cfg))
                };
                let watchdog_actions = engine.watchdog_tick().await;
                for action in &watchdog_actions {
                    let Some(message) = action.message() else {
                        continue;
                    };
                    log_line(&format!("watchdog: {message}"));
                }
                if !watchdog_actions.is_empty() {
                    let cfg = engine.cfg().await;
                    if cfg.behavior.notifications {
                        for action in &watchdog_actions {
                            let Some(message) = action.message() else {
                                continue;
                            };
                            let summary =
                                tr("Trommel: hashrate watchdog", "Trommel: сторож хешрейта");
                            let _ = Engine::emit_notification(
                                iface_ref.signal_emitter(),
                                &summary,
                                message,
                            )
                            .await;
                            dbus::notify_desktop(&connection, &summary, message);
                        }
                    }
                    let _ = refresh_tx.send(()).await;
                }
                if events.is_empty() {
                    continue;
                }
                for event in &events {
                    log_line(&format!("{event:?}"));
                }
                let cfg = engine.cfg().await;
                if cfg.behavior.notifications {
                    for (summary, body) in dbus::events_to_notifications(&events) {
                        let _ =
                            Engine::emit_notification(iface_ref.signal_emitter(), &summary, &body)
                                .await;
                        dbus::notify_desktop(&connection, &summary, &body);
                    }
                }
                let _ = refresh_tx.send(()).await;
            }
        })
    };

    // Shutdown: SIGTERM/SIGINT or a closed session bus. Stop only our miner,
    // force a short-timeout snapshot restore, then let the guard go. A lost
    // session bus means the control plane is gone; mining must not continue
    // unattended.
    let mut bus_closed = false;
    let shutdown = async {
        let mut term = signal_stream(tokio::signal::unix::SignalKind::terminate());
        let mut int = signal_stream(tokio::signal::unix::SignalKind::interrupt());
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
            _ = connection.closed() => {
                bus_closed = true;
            }
        }
    };

    let mut quit_by_tray = false;
    tokio::select! {
        _ = shutdown => {}
        _ = quit.notified() => {
            quit_by_tray = true;
        }
        _ = supervisor_loop => {}
        _ = refresher => {}
    }

    if quit_by_tray {
        log_line("tray: quit requested");
        let _ = Engine::emit_quit_requested(iface_ref.signal_emitter()).await;
    }
    if bus_closed {
        log_line("сессионная шина закрыта; останавливаю майнер и возвращаю сток");
    }
    // Restore first, disarm after. The guard stays armed during the shutdown
    // window on purpose: if this process is SIGKILLed mid-restore, the guard
    // runs the same idempotent emergency-stop and the OC flock serializes the
    // two. Disarming first would leave a killed daemon with applied clocks and
    // no one to put them back.
    match engine.shutdown().await {
        Ok(()) => log_line("shutdown: miner stopped and stock restore attempted"),
        Err(err) => {
            log_line(&format!(
                "shutdown: graceful stop failed ({err}); running emergency-stop in-process"
            ));
            if let Err(emergency) =
                trommel_core::emergency::run(trommel_core::emergency::Reason::ServiceStop)
            {
                log_line(&format!("shutdown: emergency-stop failed: {emergency}"));
            }
        }
    }
    if let Some(tray) = &tray {
        tray.shutdown().await;
    }
    trommel_core::guard::disarm_and_stop();
    log_line("сервис остановлен");
    Ok(())
}

fn signal_stream(kind: tokio::signal::unix::SignalKind) -> tokio::signal::unix::Signal {
    tokio::signal::unix::signal(kind).expect("не удалось подписаться на сигнал")
}
