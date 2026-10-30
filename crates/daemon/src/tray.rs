//! StatusNotifierItem (system tray) for `trommeld`.
//!
//! The tray lives in the background daemon, not in the GTK window: it exists
//! without an open window, survives a GUI crash and controls the miner through
//! the same `Engine` path as D-Bus. `ksni` implements both
//! `org.kde.StatusNotifierItem` and `com.canonical.dbusmenu`; the menu itself
//! is rendered by the desktop panel (KDE natively, GNOME through the
//! AppIndicator extension).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use ksni::menu::{MenuItem, StandardItem};
use ksni::{Category, ToolTip, Tray, TrayMethods};
use tokio::sync::{mpsc, Notify};
use trommel_core::dbus::{self, Engine};
use trommel_core::i18n::{self, tr, Language};
use trommel_core::status::Status;
use trommel_core::supervisor::log_line;
use trommel_core::util;

/// Commands the panel menu sends to the daemon's command task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayCmd {
    Toggle,
    ShowWindow,
}

struct TrommelTray {
    running: bool,
    busy: bool,
    monochrome: bool,
    /// Top line of the menu: what the rig is doing and, while mining, the
    /// current hashrate. The panel renders it as a disabled item.
    header: String,
    tx: mpsc::UnboundedSender<TrayCmd>,
    quit: Arc<Notify>,
}

impl Tray for TrommelTray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        trommel_core::APP_ID.to_string()
    }

    fn title(&self) -> String {
        "Trommel".to_string()
    }

    fn category(&self) -> Category {
        Category::ApplicationStatus
    }

    fn icon_name(&self) -> String {
        icon_name(self.monochrome)
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            icon_name: icon_name(false),
            title: "Trommel".to_string(),
            description: self.header.clone(),
            ..ToolTip::default()
        }
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        let header = StandardItem::<Self> {
            label: self.header.clone(),
            enabled: false,
            icon_name: icon_name(false),
            ..StandardItem::default()
        };
        let toggle_tx = self.tx.clone();
        let toggle = StandardItem::<Self> {
            label: if self.running {
                tr("Stop mining", "Остановить майнинг")
            } else {
                tr("Start mining", "Запустить майнинг")
            },
            enabled: !self.busy,
            icon_name: if self.running {
                "media-playback-stop-symbolic".to_string()
            } else {
                "media-playback-start-symbolic".to_string()
            },
            activate: Box::new(move |_| {
                let _ = toggle_tx.send(TrayCmd::Toggle);
            }),
            ..StandardItem::default()
        };
        let show_tx = self.tx.clone();
        let show = StandardItem::<Self> {
            label: tr("Show window", "Показать окно"),
            icon_name: "window-new-symbolic".to_string(),
            activate: Box::new(move |_| {
                let _ = show_tx.send(TrayCmd::ShowWindow);
            }),
            ..StandardItem::default()
        };
        let quit = self.quit.clone();
        let quit_item = StandardItem::<Self> {
            label: tr("Quit", "Выход"),
            icon_name: "application-exit-symbolic".to_string(),
            activate: Box::new(move |_| quit.notify_one()),
            ..StandardItem::default()
        };
        vec![
            header.into(),
            MenuItem::Separator,
            toggle.into(),
            show.into(),
            quit_item.into(),
        ]
    }
}

fn icon_name(monochrome: bool) -> String {
    if monochrome {
        format!("{}-symbolic", trommel_core::APP_ID)
    } else {
        format!("{}-tray", trommel_core::APP_ID)
    }
}

fn initial_header(running: bool) -> String {
    if running {
        tr("Mining", "Майнинг")
    } else {
        tr("Mining stopped", "Майнинг остановлен")
    }
}

/// Build the status line used as the menu header and the tooltip body.
fn header_text(status: &Status) -> String {
    if !status.state.running {
        return tr("Mining stopped", "Майнинг остановлен");
    }
    let mut parts = vec![tr("Mining", "Майнинг")];
    if !status.state.preset_name.is_empty() {
        parts.push(status.state.preset_name.clone());
    }
    if let Some(hashrate) = status.total.hashrate.filter(|value| *value > 0.0) {
        parts.push(util::format_hashrate_compact(hashrate));
    }
    parts.join(" · ")
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TrayState {
    running: bool,
    busy: bool,
    monochrome: bool,
    language: Language,
    header: String,
}

/// Handle owned by the daemon loops. It keeps the last state so the panel is
/// only updated when something actually changed (the hashrate in the header
/// changes on most status ticks, which is intentional).
pub struct TrayHandle {
    handle: Arc<ksni::Handle<TrommelTray>>,
    last: Mutex<TrayState>,
}

impl TrayHandle {
    pub async fn sync(&self, status: &Status, monochrome: bool, busy: bool) {
        let header = header_text(status);
        let state = TrayState {
            running: status.state.running,
            busy,
            monochrome,
            language: i18n::current(),
            header,
        };
        let changed = {
            let mut last = self
                .last
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if *last == state {
                false
            } else {
                *last = state.clone();
                true
            }
        };
        if !changed {
            return;
        }
        let TrayState {
            running,
            busy,
            monochrome,
            language: _,
            header,
        } = state;
        let _ = self
            .handle
            .update(move |tray| {
                tray.running = running;
                tray.busy = busy;
                tray.monochrome = monochrome;
                tray.header = header;
            })
            .await;
    }

    pub async fn shutdown(&self) {
        let _ = self.handle.shutdown().await;
    }
}

/// Spawn the tray service and its action task.
///
/// `assume_sni_available(true)` makes a missing watcher a soft condition: the
/// service waits for the AppIndicator extension (or KDE) to appear instead of
/// failing the daemon's startup.
pub async fn spawn(
    engine: Engine,
    connection: zbus::Connection,
    refresh: mpsc::Sender<()>,
    quit: Arc<Notify>,
    monochrome: bool,
) -> anyhow::Result<Arc<TrayHandle>> {
    let running = engine.running_flag();
    let busy = engine.is_busy();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let tray = TrommelTray {
        running,
        busy,
        monochrome,
        header: initial_header(running),
        tx,
        quit,
    };
    let handle = Arc::new(tray.assume_sni_available(true).spawn().await?);
    let state = TrayState {
        running,
        busy,
        monochrome,
        language: i18n::current(),
        header: initial_header(running),
    };
    let tray_handle = Arc::new(TrayHandle {
        handle: handle.clone(),
        last: Mutex::new(state),
    });

    tokio::spawn(async move {
        command_loop(engine, connection, handle, refresh, &mut rx).await;
    });
    Ok(tray_handle)
}

async fn command_loop(
    engine: Engine,
    connection: zbus::Connection,
    handle: Arc<ksni::Handle<TrommelTray>>,
    refresh: mpsc::Sender<()>,
    rx: &mut mpsc::UnboundedReceiver<TrayCmd>,
) {
    while let Some(command) = rx.recv().await {
        match command {
            TrayCmd::Toggle => {
                let _ = handle.update(|tray| tray.busy = true).await;
                let result = engine.tray_toggle().await;
                let running = engine.running_flag();
                let _ = handle
                    .update(move |tray| {
                        tray.busy = false;
                        tray.running = running;
                    })
                    .await;
                match result {
                    Ok(started) => {
                        log_line(if started {
                            "tray: mining started"
                        } else {
                            "tray: mining stopped"
                        });
                        let _ = refresh.send(()).await;
                    }
                    Err(err) => {
                        log_line(&format!("tray: toggle failed: {err}"));
                        let cfg = engine.cfg().await;
                        if cfg.behavior.notifications {
                            let summary = tr(
                                "Trommel: cannot toggle mining",
                                "Trommel: не удалось переключить майнинг",
                            );
                            dbus::notify_desktop(&connection, &summary, &err.to_string());
                        }
                    }
                }
            }
            TrayCmd::ShowWindow => {
                if let Err(err) = dbus::launch_gui() {
                    log_line(&format!("tray: cannot open the app: {err}"));
                    let cfg = engine.cfg().await;
                    if cfg.behavior.notifications {
                        let summary = tr(
                            "Trommel: cannot open the window",
                            "Trommel: не удалось открыть окно",
                        );
                        dbus::notify_desktop(&connection, &summary, &err.to_string());
                    }
                }
            }
        }
    }
}

/// The tray command uses the same timeout-friendly wrapper as the status loop.
pub async fn sync_with_timeout(handle: &TrayHandle, status: &Status, monochrome: bool, busy: bool) {
    let _ = tokio::time::timeout(
        Duration::from_millis(500),
        handle.sync(status, monochrome, busy),
    )
    .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_tray(running: bool, busy: bool, monochrome: bool) -> TrommelTray {
        let (tx, _rx) = mpsc::unbounded_channel();
        TrommelTray {
            running,
            busy,
            monochrome,
            header: if running {
                "Mining".into()
            } else {
                "Mining stopped".into()
            },
            tx,
            quit: Arc::new(Notify::new()),
        }
    }

    fn standard(items: &[MenuItem<TrommelTray>], index: usize) -> &StandardItem<TrommelTray> {
        match &items[index] {
            MenuItem::Standard(item) => item,
            _ => panic!("expected a standard item at {index}"),
        }
    }

    #[test]
    fn menu_has_status_start_show_and_quit() {
        let tray = test_tray(false, false, true);
        let menu = tray.menu();
        assert_eq!(standard(&menu, 0).label, "Mining stopped");
        assert!(!standard(&menu, 0).enabled);
        assert_eq!(
            standard(&menu, 2).label,
            tr("Start mining", "Запустить майнинг")
        );
        assert_eq!(standard(&menu, 3).label, tr("Show window", "Показать окно"));
        assert_eq!(standard(&menu, 4).label, tr("Quit", "Выход"));
    }

    #[test]
    fn running_menu_offers_stop() {
        let tray = test_tray(true, false, true);
        let menu = tray.menu();
        assert_eq!(
            standard(&menu, 2).label,
            tr("Stop mining", "Остановить майнинг")
        );
    }

    #[test]
    fn busy_disables_only_the_toggle() {
        let tray = test_tray(false, true, true);
        let menu = tray.menu();
        assert!(!standard(&menu, 2).enabled);
        assert!(standard(&menu, 3).enabled);
        assert!(standard(&menu, 4).enabled);
    }

    #[tokio::test]
    async fn actions_send_commands_and_wake_quit() {
        let mut tray = test_tray(false, false, true);
        let (tx, mut rx) = mpsc::unbounded_channel();
        tray.tx = tx;
        let quit = tray.quit.clone();
        let menu = tray.menu();
        (standard(&menu, 2).activate)(&mut tray);
        (standard(&menu, 3).activate)(&mut tray);
        assert_eq!(rx.try_recv(), Ok(TrayCmd::Toggle));
        assert_eq!(rx.try_recv(), Ok(TrayCmd::ShowWindow));
        let notified = quit.notified();
        (standard(&menu, 4).activate)(&mut tray);
        assert!(tokio::time::timeout(Duration::from_millis(100), notified)
            .await
            .is_ok());
    }

    #[test]
    fn icon_follows_the_style() {
        assert_eq!(
            icon_name(true),
            format!("{}-symbolic", trommel_core::APP_ID)
        );
        assert_eq!(icon_name(false), format!("{}-tray", trommel_core::APP_ID));
    }

    #[test]
    fn header_reports_stop_preset_and_hashrate() {
        let mut status = Status::default();
        assert_eq!(
            header_text(&status),
            tr("Mining stopped", "Майнинг остановлен")
        );
        status.state.running = true;
        status.state.preset_name = "Pearl".into();
        status.total.hashrate = Some(125.0e12);
        let header = header_text(&status);
        assert!(header.contains("Pearl"), "{header}");
        assert!(header.contains("125.0T"), "{header}");
    }
}
