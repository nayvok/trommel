//! `pearl-minerd` — the background service.
//!
//! Owns the miner process, applies the OC profile, polls GPU + miner + pool and
//! pushes the status document over D-Bus (`org.pearl.Miner.Daemon`).

use std::time::Duration;

use pearl_miner_core::dbus::{self, Engine};
use pearl_miner_core::status::Status;
use pearl_miner_core::supervisor::log_line;
use pearl_miner_core::{paths, OBJECT_PATH};

#[tokio::main(flavor = "multi_thread", worker_threads = 2)]
async fn main() -> anyhow::Result<()> {
    paths::ensure_dirs().ok();

    let engine = Engine::new();
    let connection = zbus::connection::Builder::session()
        .name(pearl_miner_core::BUS_NAME)
        .serve_at(OBJECT_PATH, engine.clone())
        .build()
        .await?;

    let iface_ref = connection
        .object_server()
        .interface::<_, Engine>(OBJECT_PATH)
        .await?;

    log_line("сервис запущен");
    println!(
        "pearl-minerd {} запущен на {}",
        pearl_miner_core::VERSION,
        OBJECT_PATH
    );

    let (refresh_tx, mut refresh_rx) = tokio::sync::mpsc::channel::<()>(16);

    // Status refresher: periodic polling plus on-demand refresh requests.
    let refresher = {
        let engine = engine.clone();
        let iface_ref = iface_ref.clone();
        tokio::spawn(async move {
            let mut last_json = String::new();
            let mut overheated = false;

            loop {
                let (cfg, state) = {
                    let inner = engine.lock().await;
                    (inner.cfg.clone(), inner.sup.state.clone())
                };
                let interval = dbus::poll_interval(&cfg);
                let temp_limit = cfg.gpu.temp_limit;
                let notify_enabled = cfg.notifications;

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

                // Overheat notification, once per episode.
                let hot = status
                    .gpu
                    .as_ref()
                    .and_then(|gpu| gpu.temp_c)
                    .map(|temp| temp >= temp_limit)
                    .unwrap_or(false);
                if hot && !overheated && notify_enabled {
                    overheated = true;
                    let _ = Engine::emit_notification(
                        iface_ref.signal_emitter(),
                        "Pearl Miner: перегрев",
                        &format!(
                            "Температура достигла {} °C (порог {} °C). Майнер поставит паузу.",
                            status
                                .gpu
                                .as_ref()
                                .and_then(|gpu| gpu.temp_c)
                                .unwrap_or_default(),
                            temp_limit
                        ),
                    )
                    .await;
                } else if !hot && overheated {
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
                if cfg.notifications {
                    for (summary, body) in dbus::events_to_notifications(&events) {
                        let _ =
                            Engine::emit_notification(iface_ref.signal_emitter(), &summary, &body)
                                .await;
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
