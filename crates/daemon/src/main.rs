//! `trommeld` — the background service.
//!
//! Owns the miner process, applies the OC profile, polls GPU + miner + pool and
//! pushes the status document over D-Bus (`io.github.nayvok.Trommel.Daemon`).

use std::time::Duration;

use trommel_core::dbus::{self, Engine};
use trommel_core::i18n::tr;
use trommel_core::status::Status;
use trommel_core::supervisor::log_line;
use trommel_core::{paths, OBJECT_PATH};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    paths::ensure_dirs().ok();

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

    log_line("сервис запущен");
    println!(
        "trommeld {} запущен на {}",
        trommel_core::VERSION,
        OBJECT_PATH
    );

    let (refresh_tx, mut refresh_rx) = tokio::sync::mpsc::channel::<()>(16);

    // Status refresher: periodic polling plus on-demand refresh requests.
    let refresher = {
        let engine = engine.clone();
        let iface_ref = iface_ref.clone();
        let connection = connection.clone();
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
                let temp_limit = cfg
                    .gpu_profile()
                    .map(|p| p.default.temp_limit_c)
                    .unwrap_or(90);
                let notify_enabled = cfg.behavior.notifications;

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

    // Shutdown: leave the miner running (as `start-mining.sh` would); the next
    // daemon instance adopts it as an external process.
    let shutdown = async {
        let mut term = signal_stream(tokio::signal::unix::SignalKind::terminate());
        let mut int = signal_stream(tokio::signal::unix::SignalKind::interrupt());
        tokio::select! {
            _ = term.recv() => {}
            _ = int.recv() => {}
        }
    };

    tokio::select! {
        _ = shutdown => {}
        _ = supervisor_loop => {}
        _ = refresher => {}
    }

    log_line("сервис остановлен");
    Ok(())
}

fn signal_stream(kind: tokio::signal::unix::SignalKind) -> tokio::signal::unix::Signal {
    tokio::signal::unix::signal(kind).expect("не удалось подписаться на сигнал")
}
