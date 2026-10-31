#![allow(deprecated)]
//! GTK4 + libadwaita UI.
//!
//! The GUI keeps a draft configuration; the daemon owns the applied one. All
//! privileged work (GPU tuning, miner process) happens in `trommeld`.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use futures_util::StreamExt;
use gtk::{gdk, gio, glib};

use trommel_core::catalog::{self, MinerOption, OptionKind};
use trommel_core::config::{
    Appearance, Behavior, ChangeClass, Config, ConfigDiff, GpuPreset, GpuTuning, MinerInstall,
    Preset,
};
use trommel_core::dbus::MinerProxy;
use trommel_core::i18n::{self, tr};
use trommel_core::nvidia::GpuStats;
use trommel_core::status::Status;
use trommel_core::util;
use trommel_core::versions;

#[derive(Debug)]
enum Cmd {
    Refresh,
    Toggle,
    Apply {
        json: String,
        mode: String,
        /// Do not clear the GUI draft's dirty flag. Used for background work
        /// (miner install, stock restore) that must not discard user edits.
        quiet: bool,
    },
    /// Apply a draft and start the miner in one step (hero Start).
    ApplyStart {
        json: String,
    },
    RestoreOc {
        mode: String,
    },
    Bench {
        minutes: u64,
    },
}

#[derive(Debug)]
enum Ev {
    Status(String),
    Config(String),
    ServiceDown(String),
    Error(String),
    Applied { message: String, quiet: bool },
    Busy(bool),
    Bench(String),
    Quit,
}

fn spawn_worker(emit: Arc<dyn Fn(Ev) + Send + Sync>) -> tokio::sync::mpsc::UnboundedSender<Cmd> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
    std::thread::Builder::new()
        .name("trommel-dbus".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    emit(Ev::Error(format!("cannot create runtime: {err}")));
                    return;
                }
            };
            runtime.block_on(async move {
                let mut proxy: Option<MinerProxy<'static>> = None;
                let mut ticker = tokio::time::interval(Duration::from_millis(1000));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
                loop {
                    tokio::select! {
                        _ = ticker.tick() => {
                            if proxy.is_none() {
                                proxy = connect().await.inspect_err(|err| {
                                    emit(Ev::ServiceDown(err.to_string()));
                                }).ok();
                            }
                            if let Some(client) = &proxy {
                                match client.status().await {
                                    Ok(json) => emit(Ev::Status(json)),
                                    Err(err) => {
                                        emit(Ev::ServiceDown(err.to_string()));
                                        proxy = None;
                                    }
                                }
                            }
                        }
                        command = rx.recv() => {
                            let Some(command) = command else { return };
                            if proxy.is_none() {
                                proxy = connect().await.ok();
                            }
                            let Some(client) = proxy.clone() else {
                                emit(Ev::ServiceDown("trommeld is not running".into()));
                                continue;
                            };
                            match command {
                                Cmd::Refresh => {
                                    if let Ok(json) = client.refresh().await { emit(Ev::Status(json)); }
                                    if let Ok(cfg) = client.get_config().await { emit(Ev::Config(cfg)); }
                                }
                                Cmd::Toggle => {
                                    emit(Ev::Busy(true));
                                    match client.toggle().await {
                                        Ok(_) => { if let Ok(json) = client.refresh().await { emit(Ev::Status(json)); } }
                                        Err(err) => emit(Ev::Error(err.to_string())),
                                    }
                                    emit(Ev::Busy(false));
                                }
                                Cmd::Apply { json, mode, quiet } => {
                                    emit(Ev::Busy(true));
                                    match client.apply_config(&json, &mode).await {
                                        Ok(_result) => emit(Ev::Applied {
                                            message: String::new(),
                                            quiet,
                                        }),
                                        Err(err) => emit(Ev::Error(err.to_string())),
                                    }
                                    emit(Ev::Busy(false));
                                }
                                Cmd::ApplyStart { json } => {
                                    emit(Ev::Busy(true));
                                    match client.apply_config(&json, "live").await {
                                        Ok(_) => match client.start().await {
                                            Ok(_) => {
                                                emit(Ev::Applied {
                                                    message: "Applied and started".into(),
                                                    quiet: false,
                                                });
                                                if let Ok(json) = client.refresh().await {
                                                    emit(Ev::Status(json));
                                                }
                                            }
                                            Err(err) => emit(Ev::Error(err.to_string())),
                                        },
                                        Err(err) => emit(Ev::Error(err.to_string())),
                                    }
                                    emit(Ev::Busy(false));
                                }
                                Cmd::RestoreOc { mode } => {
                                    emit(Ev::Busy(true));
                                    match client.restore_oc(&mode).await {
                                        Ok(report) => {
                                            let parsed = serde_json::from_str::<serde_json::Value>(&report).ok();
                                            let mode = parsed
                                                .as_ref()
                                                .and_then(|value| value["mode"].as_str())
                                                .unwrap_or("snapshot");
                                            let count = parsed
                                                .as_ref()
                                                .and_then(|value| value["restored"].as_u64())
                                                .unwrap_or(0);
                                            let message = if mode == "defaults" {
                                                tr(
                                                    &format!("Reset {count} GPU(s) to driver defaults"),
                                                    &format!("Сброшено на сток драйвера: {count} GPU"),
                                                ).to_string()
                                            } else {
                                                tr(
                                                    &format!("Restored {count} GPU(s) to the pre-mining snapshot"),
                                                    &format!("Возвращён снимок: {count} GPU"),
                                                ).to_string()
                                            };
                                            emit(Ev::Applied { message, quiet: true });
                                            if let Ok(json) = client.refresh().await {
                                                emit(Ev::Status(json));
                                            }
                                        }
                                        Err(err) => emit(Ev::Error(err.to_string())),
                                    }
                                    emit(Ev::Busy(false));
                                }
                                Cmd::Bench { minutes } => {
                                    let request = serde_json::json!({"seconds": minutes.saturating_mul(60)}).to_string();
                                    match client.run_benchmark(&request).await {
                                        Ok(json) => emit(Ev::Bench(json)),
                                        Err(err) => emit(Ev::Error(err.to_string())),
                                    }
                                }
                            }
                        }
                    }
                }
            });
        })
        .expect("cannot start D-Bus thread");
    tx
}

async fn connect() -> anyhow::Result<MinerProxy<'static>> {
    let connection = zbus::Connection::session().await?;
    MinerProxy::new(&connection).await.map_err(Into::into)
}

/// Close the window when the tray asks the daemon to quit.
///
/// This is a separate connection and thread on purpose: it must not disturb
/// the GUI worker, and a stopped daemon must not take the GUI down. Only an
/// explicit `QuitRequested` signal from a running daemon emits `Ev::Quit`;
/// losing the name owner just reconnects.
fn spawn_quit_listener(emit: Arc<dyn Fn(Ev) + Send + Sync>) {
    let _ = std::thread::Builder::new()
        .name("trommel-quit".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    emit(Ev::Error(format!(
                        "cannot create quit listener runtime: {err}"
                    )));
                    return;
                }
            };
            runtime.block_on(async move {
                loop {
                    let connection = match zbus::Connection::session().await {
                        Ok(connection) => connection,
                        Err(_) => {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        }
                    };
                    let bus = match zbus::fdo::DBusProxy::new(&connection).await {
                        Ok(bus) => bus,
                        Err(_) => {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        }
                    };
                    let Ok(name) = zbus::names::BusName::try_from(trommel_core::BUS_NAME) else {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    };
                    if !bus.name_has_owner(name).await.unwrap_or(false) {
                        tokio::time::sleep(Duration::from_secs(1)).await;
                        continue;
                    }
                    let proxy = match MinerProxy::new(&connection).await {
                        Ok(proxy) => proxy,
                        Err(_) => {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        }
                    };
                    let mut signals = match proxy.receive_quit_requested().await {
                        Ok(signals) => signals,
                        Err(_) => {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            continue;
                        }
                    };
                    match signals.next().await {
                        Some(_) => {
                            emit(Ev::Quit);
                            return;
                        }
                        None => {
                            tokio::time::sleep(Duration::from_secs(1)).await;
                        }
                    }
                }
            });
        });
}

// ---------------------------------------------------------------- widgets

struct OptionRow {
    key: String,
    widget: OptionWidget,
}

enum OptionWidget {
    Spin(adw::SpinRow),
    Switch(adw::SwitchRow),
    Entry(adw::EntryRow),
    Combo(adw::ComboRow, Vec<String>),
}

impl OptionRow {
    fn value(&self) -> serde_json::Value {
        match &self.widget {
            OptionWidget::Spin(row) => serde_json::json!(row.value() as i64),
            OptionWidget::Switch(row) => serde_json::json!(row.is_active()),
            OptionWidget::Entry(row) => serde_json::json!(row.text().to_string()),
            OptionWidget::Combo(row, values) => {
                let index = row.selected() as usize;
                serde_json::json!(values.get(index).cloned().unwrap_or_default())
            }
        }
    }
}

struct SettingsWidgets {
    auto_restart: adw::SwitchRow,
    restore_stock: adw::SwitchRow,
    notifications: adw::SwitchRow,
    start_trommel: adw::SwitchRow,
    tray_monochrome: adw::SwitchRow,
    start_login: adw::SwitchRow,
    simple_mode: adw::SwitchRow,
    poll: adw::SpinRow,
    language: adw::ComboRow,
    appearance: adw::ComboRow,
    temp_limit: adw::SpinRow,
    temp_resume: adw::SpinRow,
    diagnostics: gtk::Button,
}

/// The parts of the hero subtitle that do not tick. Uptime is recomputed from
/// the last status sample once per second, so a 2–10 s poll interval no longer
/// makes the clock jump in whole poll steps.
#[derive(Clone)]
struct HeroMeta {
    running: bool,
    hashrate_known: bool,
    restarts: u32,
    source: String,
    expected: Option<String>,
    actual: Option<String>,
    efficiency: Option<String>,
    uptime_s: u64,
    sampled_at: u64,
    /// External miners have no start time we own; do not invent a stopwatch.
    tick: bool,
}

struct App {
    window: adw::ApplicationWindow,
    toast: adw::ToastOverlay,
    stack: adw::ViewStack,
    banner: adw::Banner,
    oc_banner: adw::Banner,
    tx: tokio::sync::mpsc::UnboundedSender<Cmd>,
    status: RefCell<Option<Status>>,
    config: RefCell<Config>,
    applied: RefCell<Config>,
    dirty: Cell<bool>,
    /// A preset created by New but not saved yet. Cancel removes it again.
    new_preset_id: RefCell<Option<String>>,
    action_button: gtk::Button,
    action_label: gtk::Label,
    action_icon: gtk::Image,
    create_button: gtk::Button,
    hero_hashrate: gtk::Label,
    hero_line: gtk::Label,
    hero_meta: gtk::Label,
    hero_meta_state: RefCell<Option<HeroMeta>>,
    pill: gtk::Label,
    rig_preset: adw::ActionRow,
    rig_edit: gtk::Button,
    gpu_host: gtk::Box,
    gpu_sig: RefCell<String>,
    /// Rolling GPU metrics behind the sparklines in the cards.
    history: Rc<RefCell<History>>,
    sparklines: RefCell<Vec<SparkEntry>>,
    shares_group: adw::PreferencesGroup,
    shares: adw::ActionRow,
    pool_balance_row: adw::ActionRow,
    pool_balance_value: gtk::Label,
    pool_meta: adw::ActionRow,
    warnings: adw::ActionRow,
    warning_badge: gtk::Label,
    preset_group: adw::PreferencesGroup,
    preset_rows: RefCell<Vec<adw::ActionRow>>,
    settings: SettingsWidgets,
    /// Group inside the miner library dialog while it is open. The library is
    /// built on demand, so `refresh_miners` updates it only when this is Some.
    miner_library_group: RefCell<Option<adw::PreferencesGroup>>,
    miner_rows: RefCell<Vec<adw::ActionRow>>,
    /// "Preset versions" group inside the open library: one row per preset with
    /// a version switch (rollback / explicit move).
    miner_preset_group: RefCell<Option<adw::PreferencesGroup>>,
    miner_preset_rows: RefCell<Vec<adw::ActionRow>>,
    /// The open library dialog. A version switch closes it before the normal
    /// restart/stop decision is shown, so two modals never stack.
    miner_library_dialog: RefCell<Option<adw::Dialog>>,
    /// Last explicit Verify result per `miner@version` while the library is
    /// open. The toast can sit behind the modal, so the row keeps a chip.
    miner_verified: RefCell<BTreeMap<String, versions::BinaryStatus>>,
}

fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        r#"
        .trommel-gpu-card {
            background-color: @card_bg_color;
            border: none;
            border-radius: 12px;
            padding: 12px;
            box-shadow: 0 1px 2px rgba(0, 0, 0, 0.22), 0 6px 18px rgba(0, 0, 0, 0.12);
        }
        .trommel-gpu-card-wide {
            padding: 16px;
        }
        .trommel-hero {
            background-color: alpha(@accent_bg_color, 0.10);
            border: 1px solid alpha(@accent_bg_color, 0.22);
            border-radius: 16px;
            padding: 18px 20px;
            box-shadow: 0 10px 28px alpha(@accent_bg_color, 0.12);
        }
        .trommel-hero-caption {
            font-size: 0.7em;
            font-weight: 700;
            letter-spacing: 0.14em;
            opacity: 0.55;
        }
        .trommel-hero-hashrate {
            font-size: 2.6em;
            font-weight: 800;
            letter-spacing: -0.03em;
        }
        .trommel-hero-line { font-weight: 500; }
        .trommel-hero-rule {
            background-color: alpha(@card_fg_color, 0.12);
            min-height: 1px;
            margin-top: 6px;
            margin-bottom: 2px;
        }
        .trommel-hero-meta { opacity: 0.72; }
        .trommel-dialog-footer {
            padding: 10px 12px 12px;
            border-top: 1px solid alpha(@card_fg_color, 0.10);
        }
        .trommel-hero-action {
            font-weight: 600;
            min-height: 38px;
            padding: 2px 22px;
        }
        .trommel-gpu-title { font-weight: 700; }
        .trommel-gpu-stat {
            background-color: alpha(@card_fg_color, 0.06);
            border-radius: 10px;
            padding: 0;
        }
        .trommel-gpu-stat-body { padding: 6px 9px 0; }
        .trommel-gpu-stat-value {
            font-weight: 700;
            font-size: 1.05em;
            font-feature-settings: "tnum";
        }
        .trommel-gpu-stat-label { font-size: 0.72em; opacity: 0.65; }
        .trommel-gpu-stat-hint { font-size: 0.7em; opacity: 0.5; }
        .trommel-gpu-hashrate-row { padding: 0 2px; }
        .trommel-gpu-hashrate {
            font-size: 1.55em;
            font-weight: 800;
            letter-spacing: -0.02em;
            font-feature-settings: "tnum";
        }
        .trommel-gpu-chip {
            background-color: alpha(@card_fg_color, 0.07);
            border-radius: 999px;
            padding: 2px 8px;
            font-size: 0.9em;
            font-feature-settings: "tnum";
        }
        .trommel-hot { color: @error_color; }
        .trommel-warm { color: @warning_color; }
        .trommel-muted { opacity: 0.6; }
        .trommel-pill {
            border-radius: 999px;
            padding: 2px 10px;
            font-weight: 600;
        }
        .trommel-pill-running {
            background-color: alpha(@success_color, 0.16);
            color: @success_color;
        }
        .trommel-pill-external {
            background-color: alpha(@warning_color, 0.16);
            color: @warning_color;
        }
        .trommel-pill-stopped {
            background-color: alpha(@card_fg_color, 0.06);
            color: alpha(@card_fg_color, 0.7);
        }
        .trommel-pill-offline {
            background-color: alpha(@error_color, 0.16);
            color: @error_color;
        }
        /* Rounded actions stay compact: the same pill shape everywhere,
           without the oversized padding that made paired buttons uneven. */
        button.pill {
            min-height: 26px;
            padding: 3px 14px;
        }
        "#,
    );
    if let Some(display) = gdk::Display::default() {
        gtk::style_context_add_provider_for_display(
            &display,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn string_list(values: &[String]) -> gtk::StringList {
    let refs: Vec<&str> = values.iter().map(String::as_str).collect();
    gtk::StringList::new(&refs)
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(classes.to_vec())
        .build()
}

fn spin_row(title: &str, subtitle: &str, value: f64, min: f64, max: f64) -> adw::SpinRow {
    let adjustment = gtk::Adjustment::new(value, min, max, 1.0, 10.0, 0.0);
    let row = adw::SpinRow::new(Some(&adjustment), 1.0, 0);
    row.set_title(title);
    row.set_subtitle(subtitle);
    row
}

fn switch_row(title: &str, subtitle: &str) -> adw::SwitchRow {
    let row = adw::SwitchRow::new();
    row.set_title(title);
    row.set_subtitle(subtitle);
    row
}

/// A lock's value row is only meaningful while the lock is on. Hiding it also
/// stops the dialog from showing a stale 1620/810 next to "off = auto".
fn sync_lock_row(lock: &adw::SwitchRow, row: &adw::SpinRow, visible: bool) {
    row.set_visible(visible && lock.is_active());
    row.set_sensitive(lock.is_active());
}

fn clear_box(container: &gtk::Box) {
    while let Some(child) = container.first_child() {
        container.remove(&child);
    }
}

/// Dialog chrome rule: the header carries the title (and the close control)
/// only. Actions live in a footer below the scrolling content, so they never
/// scroll away and never crowd the title — secondary actions on the left, the
/// primary action on the right.
fn dialog_footer() -> gtk::Box {
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.add_css_class("trommel-dialog-footer");
    footer
}

/// A horizontal spacer for a [`dialog_footer`] that pushes the primary action
/// to the trailing edge.
fn dialog_footer_spacer() -> gtk::Box {
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    spacer
}

/// Compose a dialog body from its scrolling content and its action footer.
fn dialog_body(content: &impl IsA<gtk::Widget>, footer: &gtk::Box) -> gtk::Box {
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.append(content);
    body.append(footer);
    body
}

fn set_page_icon(stack: &adw::ViewStack, child: &impl IsA<gtk::Widget>, icon: &str) {
    let page = stack.page(child);
    page.set_icon_name(Some(icon));
}

// ------------------------------------------------------------------- run

/// Every environment variable that switches the app into a private screenshot
/// instance. A new capture hook must be added here too: otherwise that capture
/// runs as the single-instance app and can present a normal running window
/// instead of rendering its own one. The `screenshot_hooks_cover_every_capture`
/// test enforces the pairing.
const SCREENSHOT_HOOKS: [&str; 5] = [
    "TROMMEL_SCREENSHOT",
    "TROMMEL_TUNE_SCREENSHOT",
    "TROMMEL_INSTALL_SCREENSHOT",
    "TROMMEL_PRESET_SCREENSHOT",
    "TROMMEL_LIBRARY_SCREENSHOT",
];

fn screenshot_requested() -> bool {
    SCREENSHOT_HOOKS
        .iter()
        .any(|name| std::env::var_os(name).is_some())
}

pub fn run() -> anyhow::Result<()> {
    // Both screenshot modes run a private instance so they can capture a window
    // even when a normal Trommel instance is open.
    let screenshot = screenshot_requested();
    let app_id = if screenshot {
        format!("io.github.nayvok.Trommel.Screenshot{}", std::process::id())
    } else {
        trommel_core::APP_ID.to_string()
    };
    let mut builder = adw::Application::builder().application_id(&app_id);
    if screenshot {
        builder = builder.flags(gio::ApplicationFlags::NON_UNIQUE);
    }
    let app = builder.build();
    app.connect_activate(build_ui);
    app.run_with_args(&["trommel"]);
    Ok(())
}

fn build_ui(app: &adw::Application) {
    // GApplication is single-instance: a second launch must present the open
    // window, not build a second UI and a second D-Bus worker.
    if let Some(existing) = APP_SLOT.with(|slot| slot.borrow().clone()) {
        existing.window.present();
        return;
    }
    let cfg = Config::load_or_default();
    i18n::set_language(cfg.behavior.language);
    apply_appearance(cfg.behavior.appearance);
    // Screenshot and test hook: the environment wins over the stored scheme.
    if let Ok(scheme) = std::env::var("TROMMEL_COLOR_SCHEME") {
        adw::StyleManager::default().set_color_scheme(match scheme.as_str() {
            "dark" => adw::ColorScheme::ForceDark,
            "light" => adw::ColorScheme::ForceLight,
            _ => adw::ColorScheme::Default,
        });
    }

    install_css();

    let (event_tx, event_rx) = std::sync::mpsc::channel::<Ev>();
    let event_tx = Arc::new(event_tx);
    let emit: Arc<dyn Fn(Ev) + Send + Sync> = Arc::new(move |event| {
        let _ = event_tx.send(event);
    });
    let quit_emit = emit.clone();
    let tx = spawn_worker(emit);
    spawn_quit_listener(quit_emit);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Trommel")
        // The header carries navigation only (refresh, switcher, menu), so the
        // one-line view switcher fits from about 850 px in Russian, the wider
        // locale. 880 keeps a small margin without making the window feel wide;
        // below that the switcher stacks its labels under the icons (still in
        // the header) and only at ~740 it moves to the bottom bar.
        .default_width(880)
        .default_height(760)
        .build();
    window.set_size_request(400, 420);
    // Documentation/verification helper: render the window at another width.
    if let Ok(value) = std::env::var("TROMMEL_WINDOW_WIDTH") {
        if let Ok(width) = value.parse::<i32>() {
            window.set_default_width(width);
        }
    }
    if let Ok(value) = std::env::var("TROMMEL_WINDOW_HEIGHT") {
        if let Ok(height) = value.parse::<i32>() {
            window.set_default_height(height);
        }
    }

    let toast = adw::ToastOverlay::new();
    let stack = adw::ViewStack::new();
    stack.set_enable_transitions(true);
    stack.set_transition_duration(220);
    toast.set_child(Some(&stack));

    // Top-level navigation lives in the header bar: the three views are the
    // primary destinations, so the switcher is the title widget. It falls back
    // to the app title when the window is too narrow for the labels, and only
    // then the switcher bar reveals itself at the bottom. That is the stock
    // libadwaita adaptive pattern and keeps a single visible navigation
    // surface at any width. (AdwViewSwitcherTitle is deprecated since 1.4 in
    // favour of hand-wired AdwBreakpoint setters; it still is the concise
    // implementation of exactly this behaviour.)
    let title = adw::ViewSwitcherTitle::builder()
        .stack(&stack)
        .title("Trommel")
        .subtitle(tr("NVIDIA mining rig manager", "Менеджер NVIDIA-рига"))
        .build();
    let switcher_bar = adw::ViewSwitcherBar::builder().stack(&stack).build();
    {
        let switcher_bar = switcher_bar.clone();
        title.connect_notify_local(Some("title-visible"), move |title, _| {
            switcher_bar.set_reveal(title.is_title_visible());
        });
    }

    let header = adw::HeaderBar::new();
    header.set_centering_policy(adw::CenteringPolicy::Strict);
    header.set_title_widget(Some(&title));

    let refresh = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text(tr("Refresh", "Обновить"))
        .build();
    header.pack_start(&refresh);

    let menu_model = gio::Menu::new();
    menu_model.append(Some(&tr("Open log", "Открыть лог")), Some("app.log"));
    menu_model.append(Some(&tr("About Trommel", "О Trommel")), Some("app.about"));
    menu_model.append(Some(&tr("Quit", "Выход")), Some("app.quit"));
    let menu = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu_model)
        .build();
    header.pack_end(&menu);

    let banner = adw::Banner::builder()
        .title(tr(
            "You have unsaved changes",
            "Есть несохранённые изменения",
        ))
        .button_label(tr("Apply", "Применить"))
        .build();
    banner.set_revealed(false);
    let oc_banner = adw::Banner::builder()
        .title(tr(
            "GPU settings are still applied",
            "Настройки GPU всё ещё применены",
        ))
        .button_label(tr("Restore stock", "Вернуть сток"))
        .build();
    oc_banner.set_revealed(false);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_bottom_bar(&switcher_bar);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&banner);
    content.append(&oc_banner);
    content.append(&toast);
    toolbar.set_content(Some(&content));
    window.set_content(Some(&toolbar));

    // ------------------------------------------------------------ dashboard
    // A plain scrolled page with a wider clamp than AdwPreferencesPage: the
    // GPU cards can spread into two or three columns on a large window instead
    // of being pinned to a 600 px reading column.
    let dashboard = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vexpand(true)
        .build();
    let dashboard_clamp = adw::Clamp::builder()
        .maximum_size(900)
        .tightening_threshold(600)
        .build();
    let dashboard_box = gtk::Box::new(gtk::Orientation::Vertical, 18);
    dashboard_box.set_margin_top(18);
    dashboard_box.set_margin_bottom(24);
    dashboard_box.set_margin_start(12);
    dashboard_box.set_margin_end(12);
    dashboard_clamp.set_child(Some(&dashboard_box));
    dashboard.set_child(Some(&dashboard_clamp));

    let hero_group = adw::PreferencesGroup::new();
    let hero = gtk::Box::new(gtk::Orientation::Vertical, 6);
    hero.add_css_class("trommel-hero");
    hero.set_margin_top(10);
    hero.set_margin_bottom(12);
    hero.set_margin_start(6);
    hero.set_margin_end(6);
    let hero_top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let hero_left = gtk::Box::new(gtk::Orientation::Vertical, 0);
    hero_left.set_hexpand(true);
    let hero_caption = label(
        &tr("HASHRATE", "ХЕШРЕЙТ"),
        &["caption", "trommel-hero-caption"],
    );
    let hero_hashrate = label("—", &["trommel-hero-hashrate", "numeric", "dim-label"]);
    hero_left.append(&hero_caption);
    hero_left.append(&hero_hashrate);
    let pill = label(
        &tr("stopped", "остановлен"),
        &["caption", "trommel-pill", "trommel-pill-stopped"],
    );
    pill.set_valign(gtk::Align::Center);
    hero_top.append(&hero_left);
    hero_top.append(&pill);
    let hero_line = label(
        &tr("No preset", "Нет пресета"),
        &["caption", "trommel-hero-line"],
    );
    let hero_rule = gtk::Separator::new(gtk::Orientation::Horizontal);
    hero_rule.add_css_class("trommel-hero-rule");
    let hero_meta = label(
        "",
        &["caption", "dim-label", "numeric", "trommel-hero-meta"],
    );
    hero_line.set_ellipsize(gtk::pango::EllipsizeMode::End);
    hero_meta.set_ellipsize(gtk::pango::EllipsizeMode::End);
    // Start/Stop is the action of this page, not of the app chrome: it sits in
    // the hero footer so the card's brightest control is anchored to its
    // bottom edge and the header keeps navigation only (refresh, switcher,
    // menu), which stays single-line at narrower widths.
    let action_label = gtk::Label::new(Some(&tr("Start", "Запустить")));
    let action_icon = gtk::Image::from_icon_name("media-playback-start-symbolic");
    let action_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    action_content.set_halign(gtk::Align::Center);
    action_content.append(&action_icon);
    action_content.append(&action_label);
    let action_button = gtk::Button::builder()
        .css_classes(["suggested-action", "pill", "trommel-hero-action"])
        .child(&action_content)
        .valign(gtk::Align::Center)
        .build();
    // The button has a custom child (icon + label), so GTK cannot derive its
    // accessible name from a plain label property; set it explicitly and keep
    // it in sync with the visible text.
    set_action_text(&action_button, &action_label, &tr("Start", "Запустить"));
    let create_button = gtk::Button::builder()
        .label(tr("Create preset", "Создать пресет"))
        .css_classes(["flat", "pill"])
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    let hero_footer = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    hero_footer.set_margin_top(2);
    hero_meta.set_hexpand(true);
    hero_meta.set_valign(gtk::Align::Center);
    hero_footer.append(&hero_meta);
    hero_footer.append(&create_button);
    hero_footer.append(&action_button);
    hero.append(&hero_top);
    hero.append(&hero_line);
    hero.append(&hero_rule);
    hero.append(&hero_footer);
    hero_group.add(&hero);
    dashboard_box.append(&hero_group);

    let rig_group = adw::PreferencesGroup::builder()
        .title(tr("Rig", "Риг"))
        .description(tr(
            "Cards in the active mining preset. The switch includes a card in this preset's session; Apply stores the change.",
            "Карты активного пресета. Переключатель включает карту в сессию этого пресета; изменение сохраняется по Apply.",
        ))
        .build();
    let rig_preset = adw::ActionRow::builder()
        .title(tr("No active preset", "Нет активного пресета"))
        .subtitle(tr(
            "Create a preset to choose cards for mining",
            "Создайте пресет, чтобы выбрать карты для майнинга",
        ))
        .build();
    let rig_edit = gtk::Button::builder()
        .label(tr("Edit preset", "Изменить пресет"))
        .css_classes(["flat", "pill"])
        .valign(gtk::Align::Center)
        .build();
    rig_preset.add_suffix(&rig_edit);
    rig_group.add(&rig_preset);
    let gpu_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    gpu_host.set_hexpand(true);
    gpu_host.set_margin_top(6);
    gpu_host.set_margin_bottom(6);
    rig_group.add(&gpu_host);
    dashboard_box.append(&rig_group);

    let shares_group = adw::PreferencesGroup::builder()
        .title(tr("Pool and shares", "Пул и шары"))
        .build();
    let shares = adw::ActionRow::builder()
        .title(tr("Shares", "Шары"))
        .subtitle("—")
        .build();
    let pool_balance = adw::ActionRow::builder()
        .title(tr("Pool balance", "Баланс пула"))
        .build();
    let pool_balance_value = label("—", &["caption", "accent"]);
    pool_balance_value.set_valign(gtk::Align::Center);
    pool_balance.add_suffix(&pool_balance_value);
    let pool_meta = adw::ActionRow::builder()
        .title(tr("Pool", "Пул"))
        .subtitle("—")
        .build();
    shares_group.add(&shares);
    shares_group.add(&pool_balance);
    shares_group.add(&pool_meta);
    dashboard_box.append(&shares_group);

    let warnings_group = adw::PreferencesGroup::builder()
        .title(tr("Notes", "Замечания"))
        .build();
    let warnings = adw::ActionRow::builder().title("—").build();
    let warning_badge = label("✓", &["caption", "success"]);
    warning_badge.set_valign(gtk::Align::Center);
    warnings.add_suffix(&warning_badge);
    warnings_group.add(&warnings);
    dashboard_box.append(&warnings_group);

    stack.add_titled(&dashboard, Some("dashboard"), &tr("Dashboard", "Дашборд"));
    // Available in both Adwaita and Yaru; utilities-system-monitor-symbolic is Yaru-only.
    set_page_icon(&stack, &dashboard, "view-grid-symbolic");

    // --------------------------------------------------------------- presets
    let (presets_page, preset_group) = build_presets_page();
    stack.add_titled(&presets_page, Some("presets"), &tr("Presets", "Пресеты"));
    set_page_icon(&stack, &presets_page, "document-open-recent-symbolic");

    let (settings_page, settings) = build_settings_page();
    stack.add_titled(
        &settings_page,
        Some("settings"),
        &tr("Settings", "Настройки"),
    );
    set_page_icon(&stack, &settings_page, "preferences-system-symbolic");

    let app_state = Rc::new(App {
        window: window.clone(),
        toast: toast.clone(),
        stack: stack.clone(),
        banner: banner.clone(),
        oc_banner: oc_banner.clone(),
        tx,
        status: RefCell::new(None),
        applied: RefCell::new(cfg.clone()),
        config: RefCell::new(cfg),
        dirty: Cell::new(false),
        new_preset_id: RefCell::new(None),
        action_button: action_button.clone(),
        action_label: action_label.clone(),
        action_icon: action_icon.clone(),
        create_button: create_button.clone(),
        hero_hashrate,
        hero_line,
        hero_meta,
        hero_meta_state: RefCell::new(None),
        pill,
        rig_preset: rig_preset.clone(),
        rig_edit: rig_edit.clone(),
        gpu_host,
        gpu_sig: RefCell::new(String::new()),
        history: Rc::new(RefCell::new(History::default())),
        sparklines: RefCell::new(Vec::new()),
        shares_group: shares_group.clone(),
        shares: shares.clone(),
        pool_balance_row: pool_balance.clone(),
        pool_balance_value,
        pool_meta,
        warnings,
        warning_badge,
        preset_group,
        preset_rows: RefCell::new(Vec::new()),
        settings,
        miner_library_group: RefCell::new(None),
        miner_rows: RefCell::new(Vec::new()),
        miner_preset_group: RefCell::new(None),
        miner_preset_rows: RefCell::new(Vec::new()),
        miner_library_dialog: RefCell::new(None),
        miner_verified: RefCell::new(BTreeMap::new()),
    });
    APP_SLOT.with(|slot| *slot.borrow_mut() = Some(app_state.clone()));

    install_actions(app, &app_state);
    connect_signals(&app_state);
    refresh_preset_list(&app_state);
    load_settings(&app_state);
    load_daemon_autostart(&app_state);
    update_dirty(&app_state);

    {
        let app_state = app_state.clone();
        glib::timeout_add_local(Duration::from_millis(60), move || {
            while let Ok(event) = event_rx.try_recv() {
                handle_event(&app_state, event);
            }
            glib::ControlFlow::Continue
        });
    }

    {
        let app_state = app_state.clone();
        glib::timeout_add_seconds_local(1, move || {
            let anchor = app_state
                .hero_meta_state
                .borrow()
                .as_ref()
                .filter(|meta| meta.tick)
                .map(|meta| (meta.uptime_s, meta.sampled_at));
            if let Some((base, sampled_at)) = anchor {
                render_hero_meta(&app_state, uptime_since(base, sampled_at, util::now_unix()));
            }
            glib::ControlFlow::Continue
        });
    }

    window.present();
    gtk::prelude::GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
    let _ = app_state.tx.send(Cmd::Refresh);

    if let Some(path) = std::env::var_os("TROMMEL_SCREENSHOT") {
        let window = app_state.window.clone();
        let stack = app_state.stack.clone();
        let application = app.clone();
        let path = path.to_string_lossy().into_owned();
        if let Ok(page) = std::env::var("TROMMEL_SCREENSHOT_PAGE") {
            stack.set_visible_child_name(&page);
        }
        let delay = std::env::var("TROMMEL_SCREENSHOT_DELAY")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(4);
        let scroll = std::env::var("TROMMEL_SCREENSHOT_SCROLL").ok();
        let application: gtk::Application = application.upcast();
        glib::timeout_add_seconds_local(delay, move || {
            if scroll.as_deref() == Some("bottom") {
                // Give the scroll one frame to take effect before the snapshot.
                if let Some(page) = stack.visible_child() {
                    scroll_to_bottom(&page);
                }
                let window = window.clone();
                let path = path.clone();
                let application = application.clone();
                glib::timeout_add_local(Duration::from_millis(700), move || {
                    schedule_screenshot(window.clone(), path.clone(), application.clone(), 0);
                    glib::ControlFlow::Break
                });
            } else {
                schedule_screenshot(window.clone(), path.clone(), application.clone(), 0);
            }
            glib::ControlFlow::Break
        });
    }
    if std::env::var_os("TROMMEL_OPEN_PRESET").is_some() {
        let app_state = app_state.clone();
        glib::timeout_add_seconds_local(3, move || {
            let id = app_state
                .config
                .borrow()
                .active_preset
                .clone()
                .unwrap_or_default();
            preset_editor_dialog(&app_state, &id);
            glib::ControlFlow::Break
        });
    }
    if std::env::var_os("TROMMEL_OPEN_LIBRARY").is_some() {
        let app_state = app_state.clone();
        glib::timeout_add_seconds_local(3, move || {
            miner_library_dialog(&app_state, None, None);
            glib::ControlFlow::Break
        });
    }
    if std::env::var_os("TROMMEL_OPEN_INSTALL").is_some() {
        let app_state = app_state.clone();
        glib::timeout_add_seconds_local(4, move || {
            let id = catalog::miners()
                .first()
                .map(|miner| miner.id.clone())
                .unwrap_or_default();
            let version = catalog::miner(&id)
                .and_then(|miner| miner.latest_version())
                .map(|item| item.version.clone())
                .unwrap_or_default();
            miner_install_dialog(&app_state, &id, version, None);
            glib::ControlFlow::Break
        });
    }
    if std::env::var_os("TROMMEL_OPEN_TUNE").is_some() {
        let app_state = app_state.clone();
        glib::timeout_add_seconds_local(4, move || {
            let gpu = app_state
                .status
                .borrow()
                .as_ref()
                .and_then(|status| status.gpus.first().cloned());
            if let Some(gpu) = gpu {
                gpu_tune_dialog(&app_state, &gpu);
            }
            glib::ControlFlow::Break
        });
    }
}

thread_local! {
    static APP_SLOT: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

fn build_presets_page() -> (adw::PreferencesPage, adw::PreferencesGroup) {
    let page = adw::PreferencesPage::new();
    let list_group = adw::PreferencesGroup::builder()
        .title(tr("Mining presets", "Пресеты майнинга"))
        .description(tr(
            "A preset bundles coin, pool, wallet and the miner version it runs. Open a row to edit it.",
            "Пресет объединяет монету, пул, кошелёк и версию майнера. Откройте строку для правки.",
        ))
        .build();
    page.add(&list_group);

    let actions = adw::PreferencesGroup::new();
    let new_button = gtk::Button::builder()
        .label(tr("New preset", "Новый пресет"))
        .icon_name("list-add-symbolic")
        .css_classes(["flat", "pill"])
        .halign(gtk::Align::Center)
        .build();
    new_button.set_margin_top(8);
    new_button.set_margin_bottom(12);
    actions.add(&new_button);
    page.add(&actions);
    NEW_PRESET_BUTTON.with(|slot| *slot.borrow_mut() = Some(new_button));

    // The former Miners page is a dialog now. The row makes the library
    // reachable from the place that decides which miner a preset runs.
    let library_group = adw::PreferencesGroup::builder()
        .title(tr("Miner library", "Библиотека майнеров"))
        .description(tr(
            "Installed versions are global: install a build once and every preset that pins it sees it. The preset keeps its own miner and version.",
            "Установленные версии глобальны: установите сборку один раз, и все пресеты, закреплённые за ней, её увидят. Пресет сохраняет свои майнер и версию.",
        ))
        .build();
    let library_row = adw::ActionRow::builder()
        .title(tr(
            "Installed and supported miners",
            "Установленные и поддерживаемые майнеры",
        ))
        .subtitle(tr(
            "Open the library to install or inspect versions",
            "Откройте библиотеку, чтобы установить версию или посмотреть установленные",
        ))
        .activatable(true)
        .build();
    library_row.set_tooltip_text(Some(&tr(
        "Open the miner library",
        "Открыть библиотеку майнеров",
    )));
    let chevron = gtk::Button::builder()
        .icon_name("go-next-symbolic")
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .tooltip_text(tr("Open the miner library", "Открыть библиотеку майнеров"))
        .build();
    library_row.add_suffix(&chevron);
    library_group.add(&library_row);
    page.add(&library_group);
    MINER_LIBRARY_ROW.with(|slot| *slot.borrow_mut() = Some(library_row));
    MINER_LIBRARY_BUTTON.with(|slot| *slot.borrow_mut() = Some(chevron));
    (page, list_group)
}

thread_local! {
    static NEW_PRESET_BUTTON: RefCell<Option<gtk::Button>> = const { RefCell::new(None) };
    static MINER_LIBRARY_ROW: RefCell<Option<adw::ActionRow>> = const { RefCell::new(None) };
    static MINER_LIBRARY_BUTTON: RefCell<Option<gtk::Button>> = const { RefCell::new(None) };
}

fn build_settings_page() -> (adw::PreferencesPage, SettingsWidgets) {
    let page = adw::PreferencesPage::new();
    let behavior = adw::PreferencesGroup::builder()
        .title(tr("Behavior", "Поведение"))
        .build();
    let auto_restart = switch_row(
        &tr("Restart on crash", "Перезапускать при падении"),
        &tr(
            "Restart the miner after an unexpected exit",
            "Перезапускать майнер после падения",
        ),
    );
    let restore_stock = switch_row(
        &tr("Restore stock on stop", "Возвращать сток при остановке"),
        &tr(
            "Restore the captured power limit and unlock clocks on Stop (on by default)",
            "Возвращать снятый лимит питания и разлочивать частоты при остановке (включено по умолчанию)",
        ),
    );
    let notifications = switch_row(
        &tr("Notifications", "Уведомления"),
        &tr(
            "Show a desktop notification for crashes, errors and overheat",
            "Показывать уведомление при падении, ошибке и перегреве",
        ),
    );
    let start_trommel = switch_row(
        &tr("Start Trommel at login", "Запускать Trommel при входе"),
        &tr(
            "Run the background service and tray when you log in; mining starts only if enabled separately",
            "Запускать фоновый сервис и трей при входе; майнинг — только если включён отдельно",
        ),
    );
    let tray_monochrome = switch_row(
        &tr(
            "Use monochrome tray icon",
            "Использовать монохромную иконку в трее",
        ),
        &tr(
            "Follow the panel theme; turn off for the colour icon",
            "Следовать теме панели; выключите для цветной иконки",
        ),
    );
    let start_login = switch_row(
        &tr("Start mining at login", "Запускать майнинг при входе"),
        &tr(
            "Autostart the miner with the desktop session",
            "Автозапуск майнера вместе с сессией",
        ),
    );
    let simple_mode = switch_row(
        &tr("Simple mode", "Простой режим"),
        &tr(
            "Hide advanced miner options and extra arguments",
            "Скрыть продвинутые настройки майнера и extra args",
        ),
    );
    let temp_limit = spin_row(
        &tr("Temperature limit", "Порог температуры"),
        &tr(
            "°C — overheat warning; passed to miners that support a temperature limit",
            "°C — предупреждение о перегреве; передаётся майнерам с поддержкой лимита температуры",
        ),
        90.0,
        60.0,
        100.0,
    );
    let temp_resume = spin_row(
        &tr("Resume temperature", "Температура возобновления"),
        &tr(
            "°C — resume below this temperature",
            "°C — возобновление ниже этой температуры",
        ),
        65.0,
        40.0,
        95.0,
    );
    let poll = spin_row(
        &tr("Poll interval", "Интервал опроса"),
        &tr("Seconds", "Секунды"),
        2.0,
        1.0,
        60.0,
    );
    let language = adw::ComboRow::builder()
        .title(tr("Language", "Язык"))
        .subtitle(tr(
            "Restart the app to apply",
            "Перезапустите приложение для применения",
        ))
        .model(&gtk::StringList::new(&["System", "English", "Русский"]))
        .build();
    for row in [
        &auto_restart,
        &restore_stock,
        &notifications,
        &start_trommel,
        &tray_monochrome,
        &start_login,
        &simple_mode,
    ] {
        behavior.add(row);
    }
    let appearance = adw::ComboRow::builder()
        .title(tr("Appearance", "Оформление"))
        .subtitle(tr(
            "Colour scheme; System follows the desktop",
            "Цветовая схема; «Системная» следует за рабочим столом",
        ))
        .model(&gtk::StringList::new(&[
            i18n::tr_ref("System", "Системная"),
            i18n::tr_ref("Light", "Светлая"),
            i18n::tr_ref("Dark", "Тёмная"),
        ]))
        .build();
    behavior.add(&poll);
    behavior.add(&language);
    behavior.add(&appearance);
    page.add(&behavior);

    let gpu_group = adw::PreferencesGroup::builder()
        .title(tr("GPU", "GPU"))
        .description(tr(
            "Global temperature policy and reusable per-card tuning presets.",
            "Глобальная температурная политика и переиспользуемые пресеты настройки карт.",
        ))
        .build();
    gpu_group.add(&temp_limit);
    gpu_group.add(&temp_resume);
    let presets_row = adw::ActionRow::builder()
        .title(tr("GPU presets", "Пресеты GPU"))
        .subtitle(tr(
            "Per-card power and clock templates; manage or delete them here",
            "Шаблоны питания и частот для карт; управление и удаление",
        ))
        .build();
    let presets_button = gtk::Button::builder()
        .label(tr("Manage…", "Управление…"))
        .css_classes(["flat", "pill"])
        .valign(gtk::Align::Center)
        .build();
    presets_row.add_suffix(&presets_button);
    gpu_group.add(&presets_row);
    page.add(&gpu_group);
    GPU_PRESETS_BUTTON.with(|slot| *slot.borrow_mut() = Some(presets_button));

    let actions = adw::PreferencesGroup::builder()
        .title(tr("Actions", "Действия"))
        .build();
    let save = gtk::Button::builder()
        .label(tr("Save and apply", "Сохранить и применить"))
        .css_classes(["suggested-action", "pill"])
        .valign(gtk::Align::Center)
        .build();
    let save_row = adw::ActionRow::builder()
        .title(tr("Save settings", "Сохранить настройки"))
        .build();
    save_row.add_suffix(&save);
    let diagnostics = gtk::Button::builder()
        .label(tr("Copy diagnostics", "Скопировать диагностику"))
        .css_classes(["pill"])
        .valign(gtk::Align::Center)
        .build();
    let diagnostics_row = adw::ActionRow::builder()
        .title(tr("Share with an agent", "Передать агенту"))
        .subtitle(tr(
            "Copy a redacted JSON bundle for Codex/ChatGPT",
            "Скопировать обезличенный JSON для Codex/ChatGPT",
        ))
        .build();
    diagnostics_row.add_suffix(&diagnostics);
    let report = gtk::Button::builder()
        .label(tr("Report a problem", "Сообщить о проблеме"))
        .css_classes(["flat", "pill"])
        .valign(gtk::Align::Center)
        .build();
    let report_row = adw::ActionRow::builder()
        .title(tr("Report an issue", "Сообщить об ошибке"))
        .subtitle(tr(
            "Open a prefilled GitHub issue with redacted diagnostics",
            "Открыть GitHub issue с обезличенной диагностикой",
        ))
        .build();
    report_row.add_suffix(&report);
    let stock = gtk::Button::builder()
        .label(tr("Restore stock", "Вернуть сток"))
        .css_classes(["destructive-action", "pill"])
        .valign(gtk::Align::Center)
        .build();
    let stock_row = adw::ActionRow::builder()
        .title(tr("Restore stock now", "Вернуть сток сейчас"))
        .subtitle(tr(
            "Restore the pre-mining snapshot or reset to the driver default",
            "Вернуть снимок до майнинга или сбросить на сток драйвера",
        ))
        .build();
    stock_row.add_suffix(&stock);
    actions.add(&save_row);
    actions.add(&diagnostics_row);
    actions.add(&report_row);
    actions.add(&stock_row);
    page.add(&actions);
    SAVE_SETTINGS_BUTTON.with(|slot| *slot.borrow_mut() = Some(save));
    STOCK_BUTTON.with(|slot| *slot.borrow_mut() = Some(stock));
    REPORT_BUTTON.with(|slot| *slot.borrow_mut() = Some(report));

    (
        page,
        SettingsWidgets {
            auto_restart,
            restore_stock,
            notifications,
            start_trommel,
            tray_monochrome,
            start_login,
            simple_mode,
            poll,
            language,
            appearance,
            temp_limit,
            temp_resume,
            diagnostics,
        },
    )
}

thread_local! {
    static SAVE_SETTINGS_BUTTON: RefCell<Option<gtk::Button>> = const { RefCell::new(None) };
    static STOCK_BUTTON: RefCell<Option<gtk::Button>> = const { RefCell::new(None) };
    static GPU_PRESETS_BUTTON: RefCell<Option<gtk::Button>> = const { RefCell::new(None) };
    static REPORT_BUTTON: RefCell<Option<gtk::Button>> = const { RefCell::new(None) };
    /// Set while the autostart switch is loaded, so it does not invoke
    /// `systemctl` and does not fight the user's stored choice.
    static DAEMON_AUTOSTART_SYNC: Cell<bool> = const { Cell::new(false) };
}

// --------------------------------------------------------------- restore

/// Ask before changing power/clocks and explain exactly what will be written.
/// The daemon owns the snapshot; the status only carries a summary, so a
/// multi-GPU rig still gets a per-GPU report in the toast afterwards.
fn restore_stock_dialog(app: &Rc<App>) {
    let running = app
        .status
        .borrow()
        .as_ref()
        .map(|status| status.state.running)
        .unwrap_or(false);
    if running {
        app.toast.add_toast(adw::Toast::new(&tr(
            "Stop the miner before restoring the stock profile",
            "Сначала остановите майнер, затем верните сток",
        )));
        return;
    }
    let status = match app.status.borrow().clone() {
        Some(status) => status,
        None => {
            app.toast.add_toast(adw::Toast::new(&tr(
                "Status is not available yet; wait for the daemon",
                "Статус ещё не получен — дождитесь демона",
            )));
            return;
        }
    };
    let oc = status.oc.clone();
    let has_snapshot = oc.snapshot_available && !oc.stale && oc.restore_power_w.is_some();
    let snapshot_w = oc.restore_power_w;
    let default_w = oc.driver_default_w;
    let cards = status.gpus.len().max(1);
    let mut body = String::new();
    if has_snapshot {
        body.push_str(&tr(
            &format!(
                "Restore the values captured before mining: {} W, clocks unlocked.",
                snapshot_w.unwrap_or(0)
            ),
            &format!(
                "Вернуть значения, снятые до майнинга: {} W, частоты разлочены.",
                snapshot_w.unwrap_or(0)
            ),
        ));
        if oc.was_tuned {
            body.push(' ');
            body.push_str(&tr(
                &format!(
                    "The card already had a non-default limit before mining; the driver default is {} W.",
                    default_w.unwrap_or(0)
                ),
                &format!(
                    "До майнинга на карте уже был нестандартный лимит; сток драйвера — {} W.",
                    default_w.unwrap_or(0)
                ),
            ));
        }
    } else {
        body.push_str(&tr(
            &format!(
                "No usable pre-mining snapshot. Reset power to the driver default ({} W) and unlock clocks.",
                default_w.unwrap_or(0)
            ),
            &format!(
                "Подходящего снимка нет. Сбросить питание на сток драйвера ({} W) и разлочить частоты.",
                default_w.unwrap_or(0)
            ),
        ));
    }
    if cards > 1 {
        body.push(' ');
        body.push_str(&tr(
            &format!("Applies to all {cards} cards."),
            &format!("Применится ко всем картам: {cards}."),
        ));
    }
    if let Some(err) = &oc.last_error {
        body.push(' ');
        body.push_str(&tr(
            &format!("Last error: {err}"),
            &format!("Последняя ошибка: {err}"),
        ));
    }

    let dialog = adw::AlertDialog::builder()
        .heading(tr("Restore GPU stock?", "Вернуть сток GPU?"))
        .body(&body)
        .build();
    dialog.add_response("cancel", &tr("Cancel", "Отмена"));
    if has_snapshot {
        dialog.add_response("snapshot", &tr("Restore snapshot", "Вернуть снимок"));
        dialog.set_response_appearance("snapshot", adw::ResponseAppearance::Suggested);
    }
    let can_defaults = default_w.is_some() && (!has_snapshot || default_w != snapshot_w);
    if can_defaults || !has_snapshot {
        dialog.add_response("defaults", &tr("Driver defaults", "Сток драйвера"));
        if !has_snapshot {
            dialog.set_response_appearance("defaults", adw::ResponseAppearance::Suggested);
        }
    }
    if !has_snapshot && default_w.is_none() {
        dialog.set_body(&tr(
            "No snapshot and no power limit reported by nvidia-smi; nothing to restore.",
            "Нет ни снимка, ни лимита питания от nvidia-smi — возвращать нечего.",
        ));
        dialog.add_response("close", &tr("Close", "Закрыть"));
        dialog.set_default_response(Some("close"));
        dialog.set_close_response("close");
        dialog.choose(Some(&app.window), None::<&gio::Cancellable>, |_| {});
        return;
    }
    dialog.set_default_response(Some(if has_snapshot { "snapshot" } else { "defaults" }));
    dialog.set_close_response("cancel");
    let window = app.window.clone();
    let app = app.clone();
    dialog.choose(Some(&window), None::<&gio::Cancellable>, move |response| {
        let mode = match response.as_str() {
            "snapshot" => "snapshot",
            "defaults" => "defaults",
            _ => return,
        };
        let _ = app.tx.send(Cmd::RestoreOc {
            mode: mode.to_string(),
        });
    });
}

// --------------------------------------------------------------- signals

fn connect_signals(app: &Rc<App>) {
    {
        let app = app.clone();
        let button = app.action_button.clone();
        let label = app.action_label.clone();
        button.clone().connect_clicked(move |_| {
            let running = app
                .status
                .borrow()
                .as_ref()
                .map(|status| status.state.running)
                .unwrap_or(false);
            if running {
                let _ = app.tx.send(Cmd::Toggle);
                return;
            }
            if app.config.borrow().preset().is_none() {
                create_preset_alert(&app);
                return;
            }
            if let Some(problem) = preset_setup_problem(&app) {
                setup_problem_alert(&app, &problem);
                return;
            }
            // Apply the draft first: the header Start used to ask the daemon
            // to start with its old config, silently ignoring GPU changes the
            // user had just made in the editor.
            let json = serde_json::to_string(&*app.config.borrow()).unwrap_or_default();
            let _ = app.tx.send(Cmd::ApplyStart { json });
            label.set_text(&tr("Starting…", "Запускаю…"));
            button.set_sensitive(false);
        });
    }
    {
        let button = app.create_button.clone();
        let app = app.clone();
        button.connect_clicked(move |_| {
            app.stack.set_visible_child_name("presets");
            new_preset(&app);
        });
    }
    {
        let button = app.rig_edit.clone();
        let app = app.clone();
        button.connect_clicked(move |_| {
            if app.config.borrow().preset().is_some() {
                app.stack.set_visible_child_name("presets");
                if let Some(id) = app.config.borrow().active_preset.clone() {
                    preset_editor_dialog(&app, &id);
                }
            } else {
                app.stack.set_visible_child_name("presets");
                new_preset(&app);
            }
        });
    }
    {
        let app = app.clone();
        let banner = app.banner.clone();
        banner.connect_button_clicked(move |_| {
            save_and_apply(&app);
        });
    }
    {
        let app = app.clone();
        let banner = app.oc_banner.clone();
        banner.connect_button_clicked(move |_| {
            restore_stock_dialog(&app);
        });
    }
    if let Some(button) = NEW_PRESET_BUTTON.with(|slot| slot.borrow().clone()) {
        let app = app.clone();
        button.connect_clicked(move |_| new_preset(&app));
    }
    if let Some(row) = MINER_LIBRARY_ROW.with(|slot| slot.borrow().clone()) {
        let app = app.clone();
        row.connect_activated(move |_| miner_library_dialog(&app, None, None));
    }
    if let Some(button) = MINER_LIBRARY_BUTTON.with(|slot| slot.borrow().clone()) {
        let app = app.clone();
        button.connect_clicked(move |_| miner_library_dialog(&app, None, None));
    }
    if let Some(button) = SAVE_SETTINGS_BUTTON.with(|slot| slot.borrow().clone()) {
        let app = app.clone();
        button.connect_clicked(move |_| {
            save_settings(&app);
            save_and_apply(&app);
        });
    }
    if let Some(button) = STOCK_BUTTON.with(|slot| slot.borrow().clone()) {
        let app = app.clone();
        button.connect_clicked(move |_| {
            restore_stock_dialog(&app);
        });
    }
    if let Some(button) = GPU_PRESETS_BUTTON.with(|slot| slot.borrow().clone()) {
        let app = app.clone();
        button.connect_clicked(move |_| manage_gpu_presets_dialog(&app));
    }
    if let Some(button) = REPORT_BUTTON.with(|slot| slot.borrow().clone()) {
        let app = app.clone();
        button.connect_clicked(move |_| {
            open_issue(
                &app,
                &tr("Describe the problem here.", "Опишите проблему здесь."),
            );
        });
    }
    {
        let app = app.clone();
        let button = app.settings.diagnostics.clone();
        button.connect_clicked(move |_| copy_diagnostics(&app));
    }
    {
        // The simple-mode switch is part of the draft. The tune dialog and the
        // preset editor read the flag when they open, so nothing on this page
        // has to be rebuilt.
        let app = app.clone();
        let simple_row = app.settings.simple_mode.clone();
        simple_row.connect_notify_local(Some("active"), move |row, _| {
            if app.config.borrow().behavior.simple_mode == row.is_active() {
                return;
            }
            app.config.borrow_mut().behavior.simple_mode = row.is_active();
            mark_dirty(&app);
        });
    }
    {
        // Settings switches are part of the draft, exactly like the preset
        // editor. Without this, toggling "restore stock on stop" and pressing
        // the global Apply banner silently applied the old value: the Save
        // button was the only path that copied the switches into the config.
        let row = app.settings.auto_restart.clone();
        bind_switch_draft(
            app,
            &row,
            |behavior| behavior.auto_restart,
            |behavior, value| behavior.auto_restart = value,
        );
        let row = app.settings.restore_stock.clone();
        bind_switch_draft(
            app,
            &row,
            |behavior| behavior.restore_stock_on_stop,
            |behavior, value| behavior.restore_stock_on_stop = value,
        );
        let row = app.settings.notifications.clone();
        bind_switch_draft(
            app,
            &row,
            |behavior| behavior.notifications,
            |behavior, value| behavior.notifications = value,
        );
        let row = app.settings.start_login.clone();
        bind_switch_draft(
            app,
            &row,
            |behavior| behavior.start_mining_on_login,
            |behavior, value| behavior.start_mining_on_login = value,
        );
        let row = app.settings.tray_monochrome.clone();
        bind_switch_draft(
            app,
            &row,
            |behavior| behavior.tray_monochrome,
            |behavior, value| behavior.tray_monochrome = value,
        );
        let row = app.settings.poll.clone();
        bind_spin_draft(
            app,
            &row,
            |behavior| behavior.poll_interval_s,
            |behavior, value| behavior.poll_interval_s = value,
        );
        let row = app.settings.language.clone();
        bind_language_draft(app, &row);
    }
    {
        // Login autostart is a systemd user-unit property, not part of the
        // config draft. Apply it immediately and keep the switch honest.
        let app = app.clone();
        let row = app.settings.start_trommel.clone();
        row.connect_notify_local(Some("active"), move |row, _| {
            if DAEMON_AUTOSTART_SYNC.get() {
                return;
            }
            let enable = row.is_active();
            if let Err(err) = set_daemon_autostart(enable) {
                DAEMON_AUTOSTART_SYNC.set(true);
                row.set_active(!enable);
                DAEMON_AUTOSTART_SYNC.set(false);
                show_error_toast(&app, &err.to_string());
            }
        });
    }
    {
        // Appearance applies immediately; it is also part of the draft so it
        // survives a restart through the config file.
        let app = app.clone();
        let row = app.settings.appearance.clone();
        row.connect_notify_local(Some("selected"), move |row, _| {
            let appearance = match row.selected() {
                1 => Appearance::Light,
                2 => Appearance::Dark,
                _ => Appearance::System,
            };
            if app.config.borrow().behavior.appearance == appearance {
                return;
            }
            app.config.borrow_mut().behavior.appearance = appearance;
            apply_appearance(appearance);
            mark_dirty(&app);
        });
    }
}

fn install_actions(app: &adw::Application, state: &Rc<App>) {
    let about = gio::SimpleAction::new("about", None);
    {
        let window = state.window.clone();
        about.connect_activate(move |_, _| {
            adw::AboutDialog::builder()
                .application_name("Trommel")
                .application_icon(trommel_core::APP_ID)
                .developer_name("nayvok")
                .version(trommel_core::VERSION)
                .comments(tr(
                    "Manage NVIDIA mining rigs, miners and presets.",
                    "Управление NVIDIA-ригом, майнерами и пресетами.",
                ))
                .website("https://github.com/nayvok/trommel")
                .build()
                .present(Some(&window));
        });
    }
    app.add_action(&about);

    let log = gio::SimpleAction::new("log", None);
    {
        let toast = state.toast.clone();
        log.connect_activate(move |_, _| {
            let path = trommel_core::paths::log_path();
            if !path.exists() {
                toast.add_toast(adw::Toast::new(&tr(
                    "The miner log appears after the first start.",
                    "Лог майнера появится после первого запуска.",
                )));
                return;
            }
            let uri = gio::File::for_path(path).uri();
            let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
        });
    }
    app.add_action(&log);

    let quit = gio::SimpleAction::new("quit", None);
    {
        let app = app.clone();
        quit.connect_activate(move |_, _| app.quit());
    }
    app.add_action(&quit);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
}

// --------------------------------------------------------------- helpers

fn update_dirty(app: &Rc<App>) {
    app.banner.set_revealed(app.dirty.get());
}

fn mark_dirty(app: &Rc<App>) {
    app.dirty.set(true);
    update_dirty(app);
}

/// Keep a Settings switch in sync with the draft as soon as it is toggled.
///
/// The Settings page also has an explicit **Save and apply** button, but the
/// global unsaved-changes banner applies the draft directly. If the switches
/// only wrote into the draft from `save_settings`, that second path dropped
/// the change and the old value (for example “restore stock on stop = off”)
/// was saved while the switch on screen showed the new one.
fn bind_switch_draft<G, S>(app: &Rc<App>, row: &adw::SwitchRow, get: G, set: S)
where
    G: Fn(&Behavior) -> bool + 'static,
    S: Fn(&mut Behavior, bool) + 'static,
{
    let app = app.clone();
    row.connect_notify_local(Some("active"), move |row, _| {
        let value = row.is_active();
        if get(&app.config.borrow().behavior) == value {
            return;
        }
        set(&mut app.config.borrow_mut().behavior, value);
        mark_dirty(&app);
    });
}

fn bind_spin_draft<G, S>(app: &Rc<App>, row: &adw::SpinRow, get: G, set: S)
where
    G: Fn(&Behavior) -> u64 + 'static,
    S: Fn(&mut Behavior, u64) + 'static,
{
    let app = app.clone();
    row.connect_notify_local(Some("value"), move |row, _| {
        let value = row.value() as u64;
        if get(&app.config.borrow().behavior) == value {
            return;
        }
        set(&mut app.config.borrow_mut().behavior, value);
        mark_dirty(&app);
    });
}

fn bind_language_draft(app: &Rc<App>, row: &adw::ComboRow) {
    let app = app.clone();
    row.connect_notify_local(Some("selected"), move |row, _| {
        let value = match row.selected() {
            1 => i18n::Language::En,
            2 => i18n::Language::Ru,
            _ => i18n::Language::System,
        };
        if app.config.borrow().behavior.language == value {
            return;
        }
        app.config.borrow_mut().behavior.language = value;
        mark_dirty(&app);
    });
}

/// Errors are usually longer than one toast line. Show the short form with a
/// Details button; the dialog keeps the full text selectable and copyable.
fn show_error_toast(app: &Rc<App>, message: &str) {
    let toast = adw::Toast::builder()
        .title(message)
        .button_label(tr("Details", "Подробнее"))
        .timeout(10)
        .build();
    let app_for_details = app.clone();
    let message = message.to_string();
    toast.connect_button_clicked(move |_| show_error_details(&app_for_details, &message));
    app.toast.add_toast(toast);
}

fn show_error_details(app: &Rc<App>, message: &str) {
    // An AdwDialog, not a window: it lives inside the main window, inherits the
    // app style and uses libadwaita's spring presentation (the same bounce as
    // the alert dialogs) instead of the compositor's window-open scale.
    let dialog = adw::Dialog::builder()
        .title(tr("Error details", "Подробности ошибки"))
        .content_width(560)
        .content_height(320)
        .build();
    let header = adw::HeaderBar::new();
    // The explicit Close button is the only dismiss control; the automatic
    // dialog-title close would duplicate it.
    header.set_show_end_title_buttons(false);
    let copy = gtk::Button::builder()
        .label(tr("Copy", "Копировать"))
        .css_classes(["flat", "pill"])
        .build();
    let report = gtk::Button::builder()
        .label(tr("Report issue", "Сообщить об ошибке"))
        .css_classes(["flat", "pill"])
        .build();
    let close = gtk::Button::builder()
        .label(tr("Close", "Закрыть"))
        .css_classes(["suggested-action", "pill"])
        .build();
    let footer = dialog_footer();
    footer.append(&copy);
    footer.append(&report);
    footer.append(&dialog_footer_spacer());
    footer.append(&close);
    let view = gtk::TextView::builder()
        .editable(false)
        .cursor_visible(false)
        .wrap_mode(gtk::WrapMode::WordChar)
        .left_margin(16)
        .right_margin(16)
        .top_margin(12)
        .bottom_margin(12)
        .build();
    view.buffer().set_text(message);
    let scroll = gtk::ScrolledWindow::builder()
        .child(&view)
        .hexpand(true)
        .vexpand(true)
        .build();
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&dialog_body(&scroll, &footer)));
    dialog.set_child(Some(&toolbar));
    {
        let dialog = dialog.clone();
        close.connect_clicked(move |_| {
            dialog.close();
        });
    }
    {
        let copy_button = copy.clone();
        let message = message.to_string();
        copy.connect_clicked(move |_| {
            if let Some(display) = gdk::Display::default() {
                display.clipboard().set_text(&message);
            }
            copy_button.set_label(&tr("Copied", "Скопировано"));
        });
    }
    {
        let app = app.clone();
        let message = message.to_string();
        let report_button = report.clone();
        report.connect_clicked(move |_| {
            if open_issue(&app, &message) {
                report_button.set_label(&tr("Issue opened", "Issue открыт"));
            } else {
                report_button.set_label(&tr("Copy and open manually", "Скопируйте и откройте"));
            }
        });
    }
    dialog.present(Some(&app.window));
}

fn open_issue(app: &Rc<App>, message: &str) -> bool {
    let url = {
        let cfg = app.config.borrow();
        let status = app.status.borrow();
        trommel_core::diagnostics::issue_url(&cfg, status.as_ref(), message)
    };
    let launched =
        gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>).is_ok();
    if !launched {
        let text = {
            let cfg = app.config.borrow();
            let status = app.status.borrow();
            serde_json::to_string_pretty(&trommel_core::diagnostics::report(&cfg, status.as_ref()))
                .unwrap_or_default()
        };
        if let Some(display) = gdk::Display::default() {
            display.clipboard().set_text(&text);
        }
    }
    launched
}

fn miner_names() -> Vec<String> {
    catalog::miners().iter().map(|m| m.name.clone()).collect()
}

fn miner_index(id: &str) -> u32 {
    catalog::miners()
        .iter()
        .position(|m| m.id == id)
        .unwrap_or(0) as u32
}

/// Catalogue coins selectable for a miner, followed by a custom entry.
fn coin_choices(miner_id: &str) -> Vec<Option<&'static catalog::CoinDef>> {
    let mut choices: Vec<Option<&'static catalog::CoinDef>> = catalog::coins_for_miner(miner_id)
        .into_iter()
        .map(Some)
        .collect();
    choices.push(None);
    choices
}

fn coin_labels(miner_id: &str) -> Vec<String> {
    coin_choices(miner_id)
        .into_iter()
        .map(|coin| match coin {
            Some(coin) => format!("{} · {}", coin.symbol, coin.name),
            None => tr("Custom…", "Своя монета…").to_string(),
        })
        .collect()
}

fn coin_index(miner_id: &str, coin_id: &str) -> u32 {
    coin_choices(miner_id)
        .iter()
        .position(|coin| coin.map(|coin| coin.id == coin_id).unwrap_or(false))
        .unwrap_or_else(|| coin_choices(miner_id).len().saturating_sub(1)) as u32
}

const HASHRATE_UNITS: [&str; 7] = ["auto", "H/s", "kH/s", "MH/s", "GH/s", "TH/s", "PH/s"];

fn hashrate_unit_index(unit: &str) -> u32 {
    HASHRATE_UNITS
        .iter()
        .position(|item| item.eq_ignore_ascii_case(unit))
        .unwrap_or(0) as u32
}

fn selected_coin(miner_id: &str, selected: u32) -> Option<&'static catalog::CoinDef> {
    coin_choices(miner_id)
        .get(selected as usize)
        .copied()
        .flatten()
}

/// A known coin owns its algorithm: show it as a read-only fact instead of a
/// free-text field that can drift from the catalogue. A custom coin still
/// needs the editable field.
fn sync_algo_row(
    miner_id: &str,
    coin: &adw::ComboRow,
    algo: &adw::EntryRow,
    algo_row: &adw::ActionRow,
) {
    let known = selected_coin(miner_id, coin.selected()).is_some();
    algo.set_editable(!known);
    algo.set_visible(!known);
    algo_row.set_visible(known);
    algo_row.set_subtitle(&if known {
        format!(
            "{} · {}",
            algo.text(),
            tr(
                "set automatically for a known coin",
                "подставляется автоматически для известной монеты"
            )
        )
    } else {
        String::new()
    });
    algo.set_tooltip_text(Some(&if known {
        tr(
            "Set automatically for a known coin",
            "Подставляется автоматически для известной монеты",
        )
    } else {
        tr("Required for a custom coin", "Обязательно для своей монеты")
    }));
}

fn coin_label(cfg: &Config, miner_id: &str, selected: u32) -> String {
    match selected_coin(miner_id, selected) {
        Some(coin) => format!("{} · {}", coin.symbol, coin.name),
        None => cfg
            .preset()
            .map(|preset| format!("{} · {}", preset.symbol(), preset.coin_name))
            .unwrap_or_else(|| tr("Custom coin", "Своя монета").to_string()),
    }
}

fn apply_coin_selection(preset: &mut Preset, miner_id: &str, selected: u32, algo: &str) {
    if let Some(coin) = selected_coin(miner_id, selected) {
        preset.coin = coin.id.clone();
        preset.coin_symbol = coin.symbol.clone();
        preset.coin_name = coin.name.clone();
        preset.wallet_prefix = coin.wallet_prefix.clone();
        preset.hashrate_unit = coin.hashrate_unit.clone();
        if !algo.trim().is_empty() {
            preset.algo = algo.trim().to_string();
        } else if preset.algo.is_empty() {
            preset.algo = coin.algos.first().cloned().unwrap_or_default();
        }
    } else {
        if catalog::coin(&preset.coin).is_some() {
            preset.coin = "custom".into();
        }
        preset.algo = algo.trim().to_string();
    }
    if let Some(def) = catalog::miner(miner_id) {
        let fee = def.fee_for(&preset.algo);
        preset.miner_fee_pct = fee.pct;
        if !fee.configurable {
            preset.miner_fee_override_pct = None;
        }
    }
}

fn rebuild_editor_coins(miner_id: &str, coin_row: &adw::ComboRow, preferred: &str) {
    coin_row.set_model(Some(&string_list(&coin_labels(miner_id))));
    coin_row.set_selected(coin_index(miner_id, preferred));
}

/// Selectable versions for a miner: catalogue versions first, then installed
/// builds that are not in the catalogue (imported binaries can carry any name).
fn miner_version_list(cfg: &Config, def: &catalog::MinerDef) -> Vec<String> {
    let mut versions: Vec<String> = def
        .versions
        .iter()
        .map(|version| version.version.clone())
        .collect();
    for installed in catalog::installed_versions(cfg, &def.id) {
        if !versions.contains(&installed) {
            versions.push(installed);
        }
    }
    versions
}

/// Version currently selected in a miner/version combo pair.
fn selected_version(
    app: &Rc<App>,
    miner_row: &adw::ComboRow,
    version_row: &adw::ComboRow,
) -> String {
    let Some(def) = catalog::miners().get(miner_row.selected() as usize) else {
        return String::new();
    };
    let versions = miner_version_list(&app.config.borrow(), def);
    versions
        .get(version_row.selected() as usize)
        .cloned()
        .unwrap_or_default()
}

/// Explain why a preset cannot start: it pins a miner version that is not
/// installed. `None` means the exact version is present.
fn preset_install_hint(cfg: &Config, preset: &Preset) -> Option<(String, String)> {
    match catalog::install_status(cfg, preset) {
        catalog::InstallStatus::Installed { .. } => None,
        catalog::InstallStatus::OtherVersion { installed } => Some((
            format!(
                "{} {} · {} {}",
                tr(
                    "Pinned version not installed:",
                    "Закреплённая версия не установлена:"
                ),
                preset.miner_version,
                tr("installed:", "установлено:"),
                installed.join(", ")
            ),
            tr(
                "Install the pinned version in the miner library",
                "Установите закреплённую версию в библиотеке майнеров",
            )
            .to_string(),
        )),
        catalog::InstallStatus::NotInstalled => Some((
            tr(
                "Miner version not installed.",
                "Версия майнера не установлена.",
            )
            .to_string(),
            tr(
                "Install it in the miner library",
                "Установите её в библиотеке майнеров",
            )
            .to_string(),
        )),
    }
}

fn refresh_preset_list(app: &Rc<App>) {
    for row in app.preset_rows.borrow_mut().drain(..) {
        app.preset_group.remove(&row);
    }
    let cfg = app.config.borrow();
    if cfg.presets.is_empty() {
        let empty = adw::ActionRow::builder()
            .title(tr("No mining presets yet", "Пресетов майнинга пока нет"))
            .subtitle(tr(
                "Create a preset to choose a coin, pool, wallet, miner and GPU settings.",
                "Создайте пресет, чтобы выбрать монету, пул, кошелёк, майнер и настройки GPU.",
            ))
            .build();
        app.preset_group.add(&empty);
        app.preset_rows.borrow_mut().push(empty);
    }
    for (id, preset) in &cfg.presets {
        let install_hint = preset_install_hint(&cfg, preset);
        let mut subtitle = format!(
            "{} · {} · {} {}",
            preset.symbol(),
            preset.pool_display_name(),
            preset.miner,
            preset.miner_version
        );
        if let Some((hint, _)) = &install_hint {
            subtitle.push('\n');
            subtitle.push_str(hint);
        }
        let row = adw::ActionRow::builder()
            .title(&preset.name)
            .subtitle(subtitle)
            .build();
        row.set_subtitle_lines(2);
        row.set_activatable(true);
        row.set_tooltip_text(Some(&tr(
            "Open the preset editor",
            "Открыть редактор пресета",
        )));
        if let Some((_, tooltip)) = &install_hint {
            let install = gtk::Button::builder()
                .label(tr("Install…", "Установить…"))
                .css_classes(["flat", "pill"])
                .valign(gtk::Align::Center)
                .tooltip_text(tooltip)
                .build();
            let app_install = app.clone();
            let target = MinerLibraryTarget::from_preset(preset);
            install.connect_clicked(move |_| {
                miner_library_dialog(&app_install, Some(target.clone()), None);
            });
            row.add_suffix(&install);
        }
        if Some(id) == cfg.active_preset.as_ref() {
            let badge = label(&tr("Active", "Активен"), &["caption", "accent"]);
            badge.set_valign(gtk::Align::Center);
            row.add_suffix(&badge);
        } else {
            let use_button = gtk::Button::builder()
                .label(tr("Use", "Использовать"))
                .css_classes(["flat", "pill"])
                .valign(gtk::Align::Center)
                .build();
            let app_use = app.clone();
            let id_use = id.clone();
            use_button.connect_clicked(move |_| {
                let previous_active = app_use.config.borrow().active_preset.clone();
                {
                    let mut cfg = app_use.config.borrow_mut();
                    cfg.active_preset = Some(id_use.clone());
                }
                mark_dirty(&app_use);
                refresh_preset_list(&app_use);
                let app_cancel = app_use.clone();
                let on_cancel = Rc::new(move || {
                    app_cancel.config.borrow_mut().active_preset = previous_active.clone();
                    let dirty = *app_cancel.config.borrow() != *app_cancel.applied.borrow();
                    app_cancel.dirty.set(dirty);
                    update_dirty(&app_cancel);
                    refresh_preset_list(&app_cancel);
                    app_cancel.toast.add_toast(adw::Toast::new(&tr(
                        "Preset switch cancelled",
                        "Переключение пресета отменено",
                    )));
                });
                save_and_apply_with(&app_use, Some(on_cancel));
            });
            row.add_suffix(&use_button);
        }
        // A real button, not a decorative image: clicking the arrow must
        // open the editor even when the click lands outside the row's own
        // activation area.
        let chevron = gtk::Button::builder()
            .icon_name("go-next-symbolic")
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .tooltip_text(tr("Open the preset editor", "Открыть редактор пресета"))
            .build();
        let app_chev = app.clone();
        let id_chev = id.clone();
        chevron.connect_clicked(move |_| preset_editor_dialog(&app_chev, &id_chev));
        row.add_suffix(&chevron);
        let app_row = app.clone();
        let id_row = id.clone();
        row.connect_activated(move |_| preset_editor_dialog(&app_row, &id_row));
        app.preset_group.add(&row);
        app.preset_rows.borrow_mut().push(row);
    }
}

/// Rebuild the version combo for the selected miner. Installed builds are
/// marked with a check, including versions that are not in the catalogue.
fn preset_editor_versions(
    app: &Rc<App>,
    miner_row: &adw::ComboRow,
    version_row: &adw::ComboRow,
    preferred: &str,
) {
    let Some(def) = catalog::miners().get(miner_row.selected() as usize) else {
        return;
    };
    let cfg = app.config.borrow();
    let installed = catalog::installed_versions(&cfg, &def.id);
    let versions = miner_version_list(&cfg, def);
    drop(cfg);
    let labels: Vec<String> = versions
        .iter()
        .map(|version| {
            if installed.contains(version) {
                format!("{version} ✓")
            } else {
                version.clone()
            }
        })
        .collect();
    version_row.set_model(Some(&string_list(&labels)));
    let index = versions
        .iter()
        .position(|version| version == preferred)
        .unwrap_or(0);
    version_row.set_selected(index as u32);
}

/// Rebuild the miner-specific option rows for the selected miner.
fn rebuild_editor_options(
    app: &Rc<App>,
    id: &str,
    miner_row: &adw::ComboRow,
    miner_fee: &adw::SpinRow,
    box_: &gtk::Box,
    options: &RefCell<Vec<OptionRow>>,
) {
    clear_box(box_);
    options.borrow_mut().clear();
    let Some(def) = catalog::miners().get(miner_row.selected() as usize) else {
        return;
    };
    let values = app
        .config
        .borrow()
        .presets
        .get(id)
        .map(|preset| preset.miner_options.clone())
        .unwrap_or_default();
    for option in &def.options {
        let value = values
            .get(&option.key)
            .cloned()
            .or_else(|| option.default_value());
        let (widget, row) = build_option(option, value.as_ref());
        box_.append(&widget);
        options.borrow_mut().push(OptionRow {
            key: option.key.clone(),
            widget: row,
        });
    }
    if def.options.is_empty() {
        let hint = label(
            &tr(
                "No miner-specific options.",
                "Специфических настроек у майнера нет.",
            ),
            &["dim-label"],
        );
        hint.set_margin_top(8);
        hint.set_margin_bottom(8);
        box_.append(&hint);
    }
    miner_fee.set_sensitive(def.fee_configurable);
    let fee_hint = if def.fee_configurable {
        tr("Passed to the miner", "Передаётся майнеру")
    } else {
        tr(
            "Fixed by the miner; shown for reference",
            "Фиксирована майнером; показана справочно",
        )
    };
    miner_fee.set_subtitle(&fee_hint);
}

/// Refresh description, fee breakdown and installation state of the editor.
#[allow(clippy::too_many_arguments)]
fn preset_editor_info(
    app: &Rc<App>,
    miner_row: &adw::ComboRow,
    version_row: &adw::ComboRow,
    algo: &adw::EntryRow,
    pool_fee: &adw::SpinRow,
    miner_fee: &adw::SpinRow,
    info_desc: &adw::ActionRow,
    info_fee: &adw::ActionRow,
    info_install: &adw::ActionRow,
    state: &gtk::Label,
) {
    let Some(def) = catalog::miners().get(miner_row.selected() as usize) else {
        return;
    };
    let version = selected_version(app, miner_row, version_row);
    let install = app.config.borrow().install(&def.id, &version).cloned();
    info_desc.set_subtitle(def.short_description());
    let fee = def.fee_for(algo.text().trim());
    let miner_fee_pct = if fee.configurable {
        miner_fee.value()
    } else {
        fee.pct
    };
    info_fee.set_subtitle(&format!(
        "{}: {:.2}% · {}: {:.2}% · {}: <b>{:.2}%</b>",
        tr("miner", "майнер"),
        fee.pct,
        tr("pool", "пул"),
        pool_fee.value(),
        tr("total", "всего"),
        miner_fee_pct + pool_fee.value()
    ));
    match install {
        Some(install) => {
            info_install.set_subtitle(&install.path);
            state.set_text(&tr("installed", "установлен"));
            state.set_css_classes(&["caption", "success"]);
        }
        None => {
            info_install.set_subtitle(&tr(
                "One installation is shared by every preset; download or import a binary.",
                "Установка одна для всех пресетов; скачайте или импортируйте бинарник.",
            ));
            state.set_text(&tr("not installed", "не установлен"));
            state.set_css_classes(&["caption", "warning"]);
        }
    }
    miner_fee.set_sensitive(fee.configurable);
}

/// Modal editor for one preset. The main window keeps the list; everything
/// else happens here, so there is no in-page "Back" navigation.
fn preset_editor_dialog(app: &Rc<App>, id: &str) {
    let Some(base) = app.config.borrow().presets.get(id).cloned() else {
        return;
    };
    let simple = app.config.borrow().behavior.simple_mode;

    let dialog = adw::Dialog::builder()
        .title(tr("Preset", "Пресет"))
        .content_width(600)
        .content_height(700)
        .build();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(
        &base.name,
        &tr("Preset", "Пресет"),
    )));
    // Cancel is the dismiss control, so the header needs no second close
    // button; Save sits in the footer with the rest of the actions.
    header.set_show_end_title_buttons(false);
    let cancel = gtk::Button::builder()
        .label(tr("Cancel", "Отмена"))
        .css_classes(["flat", "pill"])
        .build();
    let save = gtk::Button::builder()
        .label(tr("Save", "Сохранить"))
        .css_classes(["suggested-action", "pill"])
        .build();
    let footer = dialog_footer();
    footer.append(&cancel);
    footer.append(&dialog_footer_spacer());
    footer.append(&save);

    let page = adw::PreferencesPage::new();
    page.set_vexpand(true);

    let miner_group = adw::PreferencesGroup::builder()
        .title(tr("Miner", "Майнер"))
        .build();
    let info_desc = adw::ActionRow::builder()
        .title(tr("Description", "Описание"))
        .use_markup(true)
        .build();
    let info_fee = adw::ActionRow::builder()
        .title(tr("Fee", "Комиссия"))
        .use_markup(true)
        .build();
    let info_install = adw::ActionRow::builder()
        .title(tr("Installation", "Установка"))
        .build();
    info_install.set_subtitle_lines(2);
    let install_state = label("", &["caption"]);
    install_state.set_valign(gtk::Align::Center);
    info_install.add_suffix(&install_state);
    let install_button = gtk::Button::builder()
        .label(tr("Miner library…", "Библиотека…"))
        .css_classes(["flat", "pill"])
        .valign(gtk::Align::Center)
        .tooltip_text(tr(
            "Install or inspect miner versions",
            "Установить или посмотреть версии майнеров",
        ))
        .build();
    info_install.add_suffix(&install_button);

    let name = adw::EntryRow::builder()
        .title(tr("Name", "Название"))
        .text(&base.name)
        .build();
    let coin = adw::ComboRow::builder()
        .title(tr("Coin", "Монета"))
        .model(&string_list(&coin_labels(&base.miner)))
        .selected(coin_index(&base.miner, &base.coin))
        .build();
    let algo = adw::EntryRow::builder()
        .title(tr("Algorithm", "Алгоритм"))
        .text(&base.algo)
        .build();
    let algo_row = adw::ActionRow::builder()
        .title(tr("Algorithm", "Алгоритм"))
        .build();
    let pool_url = adw::EntryRow::builder()
        .title(tr("Pool address", "Адрес пула"))
        .text(&base.pool_url)
        .build();
    let miner = adw::ComboRow::builder()
        .title(tr("Miner", "Майнер"))
        .model(&string_list(&miner_names()))
        .selected(miner_index(&base.miner))
        .build();
    let version = adw::ComboRow::builder()
        .title(tr("Version", "Версия"))
        .build();
    let install_version = gtk::Button::builder()
        .label(tr("Install…", "Установить…"))
        .css_classes(["flat", "pill"])
        .valign(gtk::Align::Center)
        .tooltip_text(tr(
            "Install any version, including one that is not in the catalogue",
            "Установить любую версию, включая отсутствующую в каталоге",
        ))
        .build();
    version.add_suffix(&install_version);
    let wallet = adw::EntryRow::builder()
        .title(tr("Wallet", "Кошелёк"))
        .text(&base.wallet)
        .build();
    let worker = adw::EntryRow::builder()
        .title(tr("Worker", "Воркер"))
        .text(&base.worker)
        .build();
    let pool_fee = spin_row(
        &tr("Pool fee", "Комиссия пула"),
        &tr(
            "Used for effective hashrate; the pool sets the real fee",
            "Для расчёта effective; реальную ставку задаёт пул",
        ),
        base.pool_fee_pct,
        0.0,
        20.0,
    );
    let miner_fee = spin_row(
        &tr("Miner fee override", "Переопределение комиссии майнера"),
        &tr(
            "Editable only when the miner supports it",
            "Редактируется только если майнер это поддерживает",
        ),
        base.miner_fee_override_pct.unwrap_or(base.miner_fee_pct),
        0.0,
        100.0,
    );
    let coin_symbol = adw::EntryRow::builder()
        .title(tr("Coin symbol", "Символ монеты"))
        .text(&base.coin_symbol)
        .build();
    let coin_name = adw::EntryRow::builder()
        .title(tr("Coin name", "Название монеты"))
        .text(&base.coin_name)
        .build();
    let wallet_prefix = adw::EntryRow::builder()
        .title(tr("Wallet prefix", "Префикс кошелька"))
        .text(&base.wallet_prefix)
        .build();
    let unit = adw::ComboRow::builder()
        .title(tr("Hashrate unit", "Единица хешрейта"))
        .subtitle(tr(
            "Preferred display unit; auto scales when the value is far away",
            "Предпочтительная единица; auto подбирает масштаб, если значение далеко",
        ))
        .model(&string_list(
            &HASHRATE_UNITS
                .iter()
                .map(|unit| unit.to_string())
                .collect::<Vec<_>>(),
        ))
        .selected(hashrate_unit_index(&base.hashrate_unit))
        .build();
    let custom_group = adw::PreferencesGroup::builder()
        .title(tr("Custom coin", "Своя монета"))
        .description(tr(
            "Used when the selected miner does not know this coin yet. Leave the prefix empty to skip address validation.",
            "Используется, когда выбранный майнер ещё не знает эту монету. Пустой префикс отключает проверку адреса.",
        ))
        .build();
    custom_group.add(&coin_symbol);
    custom_group.add(&coin_name);
    custom_group.add(&wallet_prefix);
    custom_group.add(&unit);
    custom_group.set_visible(selected_coin(&base.miner, coin.selected()).is_none());
    let pool_hint = adw::ActionRow::builder()
        .title(tr("Known pool", "Известный пул"))
        .visible(false)
        .build();
    let use_known_fee = gtk::Button::builder()
        .label(tr("Use fee", "Взять комиссию"))
        .css_classes(["flat", "pill"])
        .valign(gtk::Align::Center)
        .visible(false)
        .build();
    pool_hint.add_suffix(&use_known_fee);
    let pool_api_url = adw::EntryRow::builder()
        .title(tr("Pool API URL", "API пула"))
        .text(&base.pool_api_url)
        .build();
    pool_api_url.set_tooltip_text(Some(&tr(
        "Optional documented JSON endpoint with a {wallet} placeholder; HTTPS only. A known pool fills this automatically.",
        "Необязательный документированный JSON endpoint с подстановкой {wallet}; только HTTPS. Для известного пула подставляется автоматически.",
    )));
    let api_port = spin_row(
        &tr("Miner API port", "Порт API майнера"),
        &tr(
            "0 = the miner catalogue default; used for hashrate and shares",
            "0 = значение из каталога; используется для хешрейта и шаров",
        ),
        base.api_port as f64,
        0.0,
        65535.0,
    );
    let extra = adw::EntryRow::builder()
        .title(tr("Extra arguments", "Доп. аргументы"))
        .text(&base.extra_args)
        .build();
    // Summary first: identity and the numbers that decide the rest of the form.
    // The editor is not a wizard; the summary is a read-only orientation line.
    let summary_group = adw::PreferencesGroup::builder()
        .title(tr("Preset", "Пресет"))
        .build();
    let gpu_summary = {
        let status = app.status.borrow();
        let total = status
            .as_ref()
            .map(|status| status.participation.len())
            .unwrap_or(0);
        let enabled = status
            .as_ref()
            .map(|status| {
                status
                    .participation
                    .iter()
                    .filter(|card| card.enabled)
                    .count()
            })
            .unwrap_or(0);
        let tuning = if base.gpu.has_tuning() {
            tr("tuned", "настроено")
        } else {
            tr("stock", "сток")
        };
        if total > 0 {
            format!("{enabled}/{total} {} · {tuning}", tr("cards", "карт"))
        } else {
            format!(
                "{tuning} · {}",
                tr(
                    "cards are configured on the dashboard",
                    "карты настраиваются на дашборде"
                )
            )
        }
    };
    let summary_row = adw::ActionRow::builder()
        .title(tr("Summary", "Сводка"))
        .subtitle(format!(
            "{} · {} {} · {gpu_summary}",
            coin_label(&app.config.borrow(), &base.miner, coin.selected()),
            base.miner,
            base.miner_version
        ))
        .build();
    summary_row.set_subtitle_lines(2);
    summary_group.add(&name);
    summary_group.add(&summary_row);

    let coin_group = adw::PreferencesGroup::builder()
        .title(tr("Coin and pool", "Монета и пул"))
        .build();
    coin_group.add(&coin);
    coin_group.add(&algo);
    coin_group.add(&algo_row);
    coin_group.add(&pool_url);
    coin_group.add(&pool_fee);
    coin_group.add(&pool_hint);

    let wallet_group = adw::PreferencesGroup::builder()
        .title(tr("Wallet", "Кошелёк"))
        .build();
    wallet_group.add(&wallet);
    wallet_group.add(&worker);

    miner_group.add(&miner);
    miner_group.add(&version);
    miner_group.add(&info_desc);
    miner_group.add(&info_fee);
    miner_group.add(&info_install);

    let gpu_group = adw::PreferencesGroup::builder()
        .title(tr("GPU", "GPU"))
        .description(tr(
            "Cards, power and clocks belong to this preset. Tune them on the dashboard; the values are applied with the session.",
            "Карты, питание и частоты принадлежат этому пресету. Настраивайте их на дашборде; значения применяются вместе с сессией.",
        ))
        .build();
    let gpu_row = adw::ActionRow::builder()
        .title(tr("GPU settings", "Настройки GPU"))
        .subtitle(&gpu_summary)
        .build();
    gpu_row.set_subtitle_lines(2);
    gpu_group.add(&gpu_row);

    let advanced_group = adw::PreferencesGroup::builder()
        .title(tr("Advanced", "Дополнительно"))
        .description(tr(
            "Optional endpoint, API and miner arguments. Simple mode hides this group.",
            "Необязательный endpoint, API и аргументы майнера. Простой режим скрывает эту группу.",
        ))
        .build();
    advanced_group.add(&miner_fee);
    advanced_group.add(&pool_api_url);
    advanced_group.add(&api_port);
    advanced_group.add(&extra);
    let options_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    advanced_group.add(&options_box);

    page.add(&summary_group);
    page.add(&coin_group);
    page.add(&custom_group);
    page.add(&wallet_group);
    page.add(&miner_group);
    page.add(&gpu_group);
    page.add(&advanced_group);
    advanced_group.set_visible(!simple);
    extra.set_visible(!simple);
    miner_fee.set_visible(!simple);
    pool_api_url.set_visible(!simple);
    api_port.set_visible(!simple);

    let actions_group = adw::PreferencesGroup::new();
    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_halign(gtk::Align::Center);
    actions.set_margin_top(8);
    actions.set_margin_bottom(8);
    let duplicate = gtk::Button::builder()
        .label(tr("Duplicate", "Дублировать"))
        .css_classes(["flat", "pill"])
        .build();
    let delete = gtk::Button::builder()
        .label(tr("Delete…", "Удалить…"))
        .css_classes(["destructive-action", "pill"])
        .build();
    actions.append(&duplicate);
    actions.append(&delete);
    actions_group.add(&actions);
    page.add(&actions_group);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&dialog_body(&page, &footer)));
    dialog.set_child(Some(&toolbar));

    let options: Rc<RefCell<Vec<OptionRow>>> = Rc::new(RefCell::new(Vec::new()));
    rebuild_editor_coins(&base.miner, &coin, &base.coin);
    sync_algo_row(&base.miner, &coin, &algo, &algo_row);
    preset_editor_versions(app, &miner, &version, &base.miner_version);
    rebuild_editor_options(app, id, &miner, &miner_fee, &options_box, &options);
    preset_editor_info(
        app,
        &miner,
        &version,
        &algo,
        &pool_fee,
        &miner_fee,
        &info_desc,
        &info_fee,
        &info_install,
        &install_state,
    );

    let refresh_summary: Rc<dyn Fn()> = {
        let app = app.clone();
        let summary_row = summary_row.clone();
        let coin = coin.clone();
        let miner = miner.clone();
        let version = version.clone();
        let gpu_summary = gpu_summary.clone();
        Rc::new(move || {
            let miner_id = miner_names()
                .get(miner.selected() as usize)
                .cloned()
                .unwrap_or_default();
            let version_text = selected_version(&app, &miner, &version);
            summary_row.set_subtitle(&format!(
                "{} · {} {} · {gpu_summary}",
                coin_label(&app.config.borrow(), &miner_id, coin.selected()),
                miner_id,
                version_text
            ));
        })
    };

    let refresh_info: Rc<dyn Fn()> = {
        let app = app.clone();
        let miner = miner.clone();
        let version = version.clone();
        let algo = algo.clone();
        let pool_fee = pool_fee.clone();
        let miner_fee = miner_fee.clone();
        let info_desc = info_desc.clone();
        let info_fee = info_fee.clone();
        let info_install = info_install.clone();
        let install_state = install_state.clone();
        let refresh_summary = refresh_summary.clone();
        Rc::new(move || {
            preset_editor_info(
                &app,
                &miner,
                &version,
                &algo,
                &pool_fee,
                &miner_fee,
                &info_desc,
                &info_fee,
                &info_install,
                &install_state,
            );
            refresh_summary();
        })
    };
    refresh_summary();

    {
        let app = app.clone();
        let id = id.to_string();
        let version_row = version.clone();
        let coin_row = coin.clone();
        let algo_entry = algo.clone();
        let algorithm_row = algo_row.clone();
        let custom_group = custom_group.clone();
        let miner_fee = miner_fee.clone();
        let options_box = options_box.clone();
        let options = options.clone();
        let refresh_info = refresh_info.clone();
        miner.connect_notify_local(Some("selected"), move |row, _| {
            let def = catalog::miners().get(row.selected() as usize);
            if let Some(def) = def {
                rebuild_editor_coins(&def.id, &coin_row, "");
                custom_group.set_visible(selected_coin(&def.id, coin_row.selected()).is_none());
                sync_algo_row(&def.id, &coin_row, &algo_entry, &algorithm_row);
            }
            preset_editor_versions(&app, row, &version_row, "");
            rebuild_editor_options(&app, &id, row, &miner_fee, &options_box, &options);
            refresh_info();
        });
    }
    {
        let refresh_version = refresh_info.clone();
        version.connect_notify_local(Some("selected"), move |_, _| refresh_version());
        let refresh_algo = refresh_info.clone();
        let algorithm_row = algo_row.clone();
        algo.connect_changed(move |entry| {
            algorithm_row.set_subtitle(&format!(
                "{} · {}",
                entry.text(),
                tr(
                    "set automatically for a known coin",
                    "подставляется автоматически для известной монеты"
                )
            ));
            refresh_algo();
        });
    }
    {
        let app = app.clone();
        let miner = miner.clone();
        let version = version.clone();
        let refresh_info = refresh_info.clone();
        install_version.connect_clicked(move |_| {
            let Some(def) = catalog::miners().get(miner.selected() as usize) else {
                return;
            };
            let version_text = selected_version(&app, &miner, &version);
            let app_done = app.clone();
            let miner_done = miner.clone();
            let version_done = version.clone();
            let refresh_done = refresh_info.clone();
            let on_done: Rc<dyn Fn()> = Rc::new(move || {
                let current = selected_version(&app_done, &miner_done, &version_done);
                preset_editor_versions(&app_done, &miner_done, &version_done, &current);
                refresh_done();
            });
            miner_install_dialog(&app, &def.id, version_text, Some(on_done));
        });
    }
    {
        let pool_url_widget = pool_url.clone();
        let pool_hint = pool_hint.clone();
        let use_known_fee_widget = use_known_fee.clone();
        let pool_fee = pool_fee.clone();
        let update_hint = Rc::new(
            move || match catalog::pool_for_url(&pool_url_widget.text()) {
                Some(pool) => {
                    pool_hint.set_visible(true);
                    pool_hint.set_title(&format!(
                        "{}: {}",
                        tr("Known pool", "Известный пул"),
                        pool.name
                    ));
                    pool_hint.set_subtitle(&format!(
                        "{} {:.2}% · {}",
                        tr("suggested fee", "предлагаемая комиссия"),
                        pool.fee_pct,
                        if pool.api_url.is_empty() {
                            tr("no account API", "нет API аккаунта")
                        } else {
                            tr("account API available", "API аккаунта доступен")
                        }
                    ));
                    use_known_fee_widget.set_visible(true);
                }
                None => {
                    pool_hint.set_visible(false);
                    use_known_fee_widget.set_visible(false);
                }
            },
        );
        let update_hint_for_url = update_hint.clone();
        let refresh_summary_for_url = refresh_summary.clone();
        let pool_url_for_signal = pool_url.clone();
        pool_url_for_signal.connect_changed(move |_| {
            update_hint_for_url();
            refresh_summary_for_url();
        });
        {
            let pool_fee = pool_fee.clone();
            let pool_url = pool_url.clone();
            let use_known_fee = use_known_fee.clone();
            use_known_fee.connect_clicked(move |_| {
                if let Some(pool) = catalog::pool_for_url(&pool_url.text()) {
                    pool_fee.set_value(pool.fee_pct);
                }
            });
        }
        update_hint();
    }
    {
        let miner = miner.clone();
        let coin = coin.clone();
        let algo = algo.clone();
        let algorithm_row = algo_row.clone();
        let custom_group = custom_group.clone();
        let coin_symbol = coin_symbol.clone();
        let coin_name = coin_name.clone();
        let wallet_prefix = wallet_prefix.clone();
        let unit = unit.clone();
        let refresh_info = refresh_info.clone();
        coin.connect_notify_local(Some("selected"), move |row, _| {
            let Some(def) = catalog::miners().get(miner.selected() as usize) else {
                return;
            };
            if let Some(selected) = selected_coin(&def.id, row.selected()) {
                custom_group.set_visible(false);
                if let Some(first) = selected.algos.first() {
                    if algo.text().trim().is_empty()
                        || !selected.algos.contains(&algo.text().trim().to_string())
                    {
                        algo.set_text(first);
                    }
                }
                coin_symbol.set_text(&selected.symbol);
                coin_name.set_text(&selected.name);
                wallet_prefix.set_text(&selected.wallet_prefix);
                unit.set_selected(hashrate_unit_index(&selected.hashrate_unit));
            } else {
                custom_group.set_visible(true);
            }
            sync_algo_row(&def.id, row, &algo, &algorithm_row);
            refresh_info();
        });
    }
    {
        let refresh_summary = refresh_summary.clone();
        name.connect_changed(move |_| refresh_summary());
    }
    {
        let refresh = refresh_info.clone();
        pool_fee.connect_notify_local(Some("value"), move |_, _| refresh());
        let refresh = refresh_info.clone();
        miner_fee.connect_notify_local(Some("value"), move |_, _| refresh());
    }
    {
        let app = app.clone();
        let miner = miner.clone();
        let version = version.clone();
        let name = name.clone();
        let coin = coin.clone();
        let pool_url = pool_url.clone();
        let refresh_info = refresh_info.clone();
        install_button.connect_clicked(move |_| {
            let Some(def) = catalog::miners().get(miner.selected() as usize) else {
                return;
            };
            let version_text = selected_version(&app, &miner, &version);
            let mut preset_name = name.text().trim().to_string();
            if preset_name.is_empty() {
                preset_name = tr("Preset", "Пресет").to_string();
            }
            let target = MinerLibraryTarget {
                name: preset_name,
                subtitle: format!(
                    "{} · {} · {} {}",
                    coin_label(&app.config.borrow(), &def.id, coin.selected()),
                    pool_url.text(),
                    def.id,
                    version_text
                ),
                miner: def.id.clone(),
                version: version_text,
            };
            let refresh_editor: Rc<dyn Fn()> = {
                let app = app.clone();
                let miner = miner.clone();
                let version = version.clone();
                let refresh_info = refresh_info.clone();
                Rc::new(move || {
                    let current = selected_version(&app, &miner, &version);
                    preset_editor_versions(&app, &miner, &version, &current);
                    refresh_info();
                })
            };
            miner_library_dialog(&app, Some(target), Some(refresh_editor));
        });
    }
    {
        let app = app.clone();
        let dialog = dialog.clone();
        let id = id.to_string();
        let name = name.clone();
        let coin = coin.clone();
        let algo = algo.clone();
        let pool_url = pool_url.clone();
        let pool_api_url = pool_api_url.clone();
        let api_port = api_port.clone();
        let coin_symbol = coin_symbol.clone();
        let coin_name = coin_name.clone();
        let wallet_prefix = wallet_prefix.clone();
        let unit = unit.clone();
        let miner = miner.clone();
        let version = version.clone();
        let wallet = wallet.clone();
        let worker = worker.clone();
        let extra = extra.clone();
        let pool_fee = pool_fee.clone();
        let miner_fee = miner_fee.clone();
        let options = options.clone();
        save.connect_clicked(move |_| {
            let Some(def) = catalog::miners().get(miner.selected() as usize) else {
                return;
            };
            let miner_id = def.id.clone();
            let version_text = selected_version(&app, &miner, &version);
            let mut values = BTreeMap::new();
            for row in options.borrow().iter() {
                values.insert(row.key.clone(), row.value());
            }
            let mut preset_name = name.text().trim().to_string();
            if preset_name.is_empty() {
                preset_name = tr("Preset", "Пресет").to_string();
            }
            {
                let mut cfg = app.config.borrow_mut();
                if let Some(preset) = cfg.presets.get_mut(&id) {
                    preset.name = preset_name;
                    preset.pool_url = pool_url.text().to_string();
                    preset.pool_fee_pct = pool_fee.value();
                    let miner_id_for_coin = miner_id.clone();
                    apply_coin_selection(
                        preset,
                        &miner_id_for_coin,
                        coin.selected(),
                        algo.text().as_str(),
                    );
                    if selected_coin(&miner_id_for_coin, coin.selected()).is_none() {
                        preset.coin = "custom".into();
                        preset.coin_symbol = coin_symbol.text().trim().to_string();
                        preset.coin_name = coin_name.text().trim().to_string();
                        preset.wallet_prefix = wallet_prefix.text().trim().to_string();
                        preset.hashrate_unit = HASHRATE_UNITS[unit.selected() as usize].to_string();
                    }
                    preset.pool_api_url = pool_api_url.text().to_string();
                    preset.api_port = api_port.value() as u16;
                    preset.miner = miner_id;
                    preset.miner_version = version_text;
                    preset.wallet = wallet.text().to_string();
                    preset.worker = worker.text().to_string();
                    preset.extra_args = extra.text().to_string();
                    preset.miner_options = values;
                    preset.miner_fee_pct = def.fee_for(algo.text().trim()).pct;
                    preset.miner_fee_override_pct = if def.fee_for(algo.text().trim()).configurable
                    {
                        Some(miner_fee.value())
                    } else {
                        None
                    };
                    preset.updated_at = util::now_unix();
                }
            }
            *app.new_preset_id.borrow_mut() = None;
            mark_dirty(&app);
            refresh_preset_list(&app);
            dialog.close();
            save_and_apply(&app);
        });
    }
    {
        let app = app.clone();
        let dialog = dialog.clone();
        let id = id.to_string();
        duplicate.connect_clicked(move |_| {
            if let Some(new_id) = duplicate_preset(&app, &id) {
                dialog.close();
                preset_editor_dialog(&app, &new_id);
            }
        });
    }
    {
        let app = app.clone();
        let dialog = dialog.clone();
        let id = id.to_string();
        delete.connect_clicked(move |_| delete_preset(&app, &dialog, &id));
    }
    {
        let app = app.clone();
        let dialog = dialog.clone();
        let id = id.to_string();
        cancel.connect_clicked(move |_| {
            discard_new_preset(&app, &id);
            dialog.close();
        });
    }
    {
        let app = app.clone();
        let id = id.to_string();
        dialog.connect_closed(move |_| discard_new_preset(&app, &id));
    }
    dialog.present(Some(&app.window));
    if let Some(path) = std::env::var_os("TROMMEL_PRESET_SCREENSHOT") {
        let path = path.to_string_lossy().into_owned();
        let capture = app.window.clone();
        let application = app.window.application();
        glib::timeout_add_seconds_local(2, move || {
            if let Some(application) = application.clone() {
                schedule_screenshot(capture.clone(), path.clone(), application, 0);
            }
            glib::ControlFlow::Break
        });
    }
}

fn discard_new_preset(app: &Rc<App>, id: &str) {
    if app.new_preset_id.borrow().as_deref() != Some(id) {
        return;
    }
    {
        let mut cfg = app.config.borrow_mut();
        cfg.presets.remove(id);
        if cfg.active_preset.as_deref() == Some(id) {
            cfg.active_preset = cfg.presets.keys().next().cloned();
        }
    }
    *app.new_preset_id.borrow_mut() = None;
    let dirty = *app.config.borrow() != *app.applied.borrow();
    app.dirty.set(dirty);
    update_dirty(app);
    refresh_preset_list(app);
}

fn new_preset(app: &Rc<App>) {
    let id = {
        let mut cfg = app.config.borrow_mut();
        let now = util::now_unix();
        let mut id = format!("preset-{now}");
        let mut suffix = 1;
        while cfg.presets.contains_key(&id) {
            id = format!("preset-{now}-{suffix}");
            suffix += 1;
        }
        let mut preset = Preset::new_draft();
        preset.id = id.clone();
        preset.name = tr("New preset", "Новый пресет");
        preset.created_at = now;
        preset.updated_at = now;
        cfg.presets.insert(id.clone(), preset);
        cfg.active_preset = Some(id.clone());
        id
    };
    *app.new_preset_id.borrow_mut() = Some(id.clone());
    mark_dirty(app);
    refresh_preset_list(app);
    preset_editor_dialog(app, &id);
}

fn duplicate_preset(app: &Rc<App>, id: &str) -> Option<String> {
    let new_id = {
        let mut cfg = app.config.borrow_mut();
        let base = cfg.presets.get(id).cloned()?;
        let now = util::now_unix();
        let mut new_id = format!("preset-{now}");
        let mut suffix = 1;
        while cfg.presets.contains_key(&new_id) {
            new_id = format!("preset-{now}-{suffix}");
            suffix += 1;
        }
        let mut copy = base;
        copy.id = new_id.clone();
        copy.name = format!("{} {}", copy.name, tr("(copy)", "(копия)"));
        copy.created_at = util::now_unix();
        copy.updated_at = copy.created_at;
        cfg.presets.insert(new_id.clone(), copy);
        cfg.active_preset = Some(new_id.clone());
        new_id
    };
    mark_dirty(app);
    refresh_preset_list(app);
    Some(new_id)
}

fn delete_preset(app: &Rc<App>, editor: &adw::Dialog, id: &str) {
    let Some(preset) = app.config.borrow().presets.get(id).cloned() else {
        return;
    };
    if app.config.borrow().presets.len() <= 1 {
        // A toast would be painted behind the modal dialog, so say it in place.
        let alert = adw::AlertDialog::builder()
            .heading(tr(
                "At least one preset is required",
                "Нужен хотя бы один пресет",
            ))
            .body(tr(
                "Create another preset first, then delete this one.",
                "Сначала создайте другой пресет, потом удаляйте этот.",
            ))
            .build();
        alert.add_response("close", &tr("Close", "Закрыть"));
        alert.set_close_response("close");
        alert.present(Some(&app.window));
        return;
    }
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Delete preset?", "Удалить пресет?"))
        .body(format!(
            "{} «{}». {}",
            tr("This removes", "Удаляется"),
            preset.name,
            tr(
                "The daemon keeps mining with the previously applied preset until you apply another one.",
                "Сервис продолжит майнить с ранее применённым пресетом, пока вы не примените другой."
            )
        ))
        .build();
    dialog.add_response("cancel", &tr("Cancel", "Отмена"));
    dialog.add_response("delete", &tr("Delete", "Удалить"));
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");
    let app = app.clone();
    let editor = editor.clone();
    let parent = app.window.clone();
    let id = id.to_string();
    dialog.choose(Some(&parent), None::<&gio::Cancellable>, move |response| {
        if response != "delete" {
            return;
        }
        {
            let mut cfg = app.config.borrow_mut();
            cfg.presets.remove(&id);
            if cfg.active_preset.as_deref() == Some(id.as_str()) {
                cfg.active_preset = cfg.presets.keys().next().cloned();
            }
        }
        mark_dirty(&app);
        refresh_preset_list(&app);
        editor.close();
    });
}

fn build_option(
    option: &MinerOption,
    value: Option<&serde_json::Value>,
) -> (gtk::Widget, OptionWidget) {
    match option.kind {
        OptionKind::Int => {
            let current = value.and_then(|v| v.as_i64()).unwrap_or(0) as f64;
            let row = spin_row(
                option.label(),
                option.help(),
                current,
                option.min.unwrap_or(0.0),
                option.max.unwrap_or(100.0),
            );
            (row.clone().upcast(), OptionWidget::Spin(row))
        }
        OptionKind::Bool => {
            let row = switch_row(option.label(), option.help());
            row.set_active(value.and_then(|v| v.as_bool()).unwrap_or(false));
            (row.clone().upcast(), OptionWidget::Switch(row))
        }
        OptionKind::Enum => {
            let values = if option.values.is_empty() {
                vec![option.default.clone().unwrap_or_default()]
            } else {
                option.values.clone()
            };
            let refs: Vec<&str> = values.iter().map(String::as_str).collect();
            let row = adw::ComboRow::builder()
                .title(option.label())
                .subtitle(option.help())
                .model(&gtk::StringList::new(&refs))
                .build();
            if let Some(text) = value.and_then(|v| v.as_str()) {
                if let Some(index) = values.iter().position(|v| v == text) {
                    row.set_selected(index as u32);
                }
            }
            (row.clone().upcast(), OptionWidget::Combo(row, values))
        }
        OptionKind::String => {
            let row = adw::EntryRow::builder().title(option.label()).build();
            if let Some(text) = value.and_then(|v| v.as_str()) {
                row.set_text(text);
            }
            (row.clone().upcast(), OptionWidget::Entry(row))
        }
    }
}

/// The preset context shown at the top of the miner library. The editor passes
/// its current draft, so the library installs exactly the miner/version the
/// user selected, not the last saved one.
#[derive(Clone)]
struct MinerLibraryTarget {
    name: String,
    subtitle: String,
    miner: String,
    version: String,
}

impl MinerLibraryTarget {
    fn from_preset(preset: &Preset) -> Self {
        Self {
            name: preset.name.clone(),
            subtitle: format!(
                "{} · {} · {} {}",
                preset.symbol(),
                preset.pool_display_name(),
                preset.miner,
                preset.miner_version
            ),
            miner: preset.miner.clone(),
            version: preset.miner_version.clone(),
        }
    }
}

/// The former Miners page, now a modal library opened from the presets page
/// and the preset editor. Catalogue browsing, install state and the A/B
/// benchmark live here; the install dialog itself stays the only installer.
fn miner_library_dialog(
    app: &Rc<App>,
    target: Option<MinerLibraryTarget>,
    on_close: Option<Rc<dyn Fn()>>,
) {
    let dialog = adw::Dialog::builder()
        .title(tr("Miner library", "Библиотека майнеров"))
        .content_width(640)
        .content_height(720)
        .build();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(
        &tr("Miner library", "Библиотека майнеров"),
        &tr(
            "Installed versions are shared by every preset",
            "Установленные версии общие для всех пресетов",
        ),
    )));
    header.set_show_end_title_buttons(false);
    let close = gtk::Button::builder()
        .label(tr("Close", "Закрыть"))
        .css_classes(["flat", "pill"])
        .build();
    let footer = dialog_footer();
    footer.append(&dialog_footer_spacer());
    footer.append(&close);

    let page = adw::PreferencesPage::new();
    page.set_vexpand(true);

    // When opened from a preset, put that preset first: it owns the miner and
    // version, while the library only stores the build. The direct Install
    // button avoids making the user pick the same miner from the list again.
    if let Some(target) = target.clone() {
        let group = adw::PreferencesGroup::builder()
            .title(tr("Preset", "Пресет"))
            .description(tr(
                "This preset keeps its own miner and version; the library installs the build.",
                "Пресет хранит свои майнер и версию; библиотека устанавливает сборку.",
            ))
            .build();
        let row = adw::ActionRow::builder()
            .title(&target.name)
            .subtitle(&target.subtitle)
            .build();
        row.set_subtitle_lines(2);
        let state = label("", &["caption"]);
        state.set_valign(gtk::Align::Center);
        row.add_suffix(&state);
        let install = gtk::Button::builder()
            .label(tr("Install…", "Установить…"))
            .css_classes(["flat", "pill"])
            .valign(gtk::Align::Center)
            .tooltip_text(tr(
                "Install the version this preset pins",
                "Установить версию, закреплённую за пресетом",
            ))
            .build();
        row.add_suffix(&install);
        group.add(&row);
        page.add(&group);

        let update_state: Rc<dyn Fn()> = {
            let app = app.clone();
            let target = target.clone();
            let state = state.clone();
            Rc::new(move || {
                let cfg = app.config.borrow();
                match cfg.install(&target.miner, &target.version) {
                    Some(install) => {
                        let mut text = tr("installed", "установлен").to_string();
                        if install.custom {
                            text.push_str(" · ");
                            text.push_str(&tr("custom", "своя сборка"));
                        }
                        if !install.verified {
                            text.push_str(" · ");
                            text.push_str(&tr("unverified", "без SHA-256"));
                        }
                        state.set_text(&text);
                        state.set_css_classes(&["caption", "success"]);
                    }
                    None => {
                        let installed = catalog::installed_versions(&cfg, &target.miner);
                        if installed.is_empty() {
                            state.set_text(&tr("not installed", "не установлен"));
                        } else {
                            state.set_text(&format!(
                                "{} {}",
                                tr("installed:", "установлено:"),
                                installed.join(", ")
                            ));
                        }
                        state.set_css_classes(&["caption", "warning"]);
                    }
                }
                state.set_tooltip_text(Some(&tr(
                    "The exact version the preset references must be installed",
                    "Должна быть установлена именно та версия, на которую ссылается пресет",
                )));
            })
        };
        update_state();

        let app_install = app.clone();
        let update_done = update_state.clone();
        install.connect_clicked(move |_| {
            let on_done: Rc<dyn Fn()> = {
                let app = app_install.clone();
                let update = update_done.clone();
                Rc::new(move || {
                    refresh_preset_list(&app);
                    refresh_miners(&app);
                    update();
                })
            };
            miner_install_dialog(
                &app_install,
                &target.miner,
                target.version.clone(),
                Some(on_done),
            );
        });
    }

    let catalog_group = adw::PreferencesGroup::builder()
        .title(tr("Installed versions", "Установленные версии"))
        .description(tr(
            "Every version is shared by all presets. A version row names the presets that pin it; install, update, verify and remove from here. Installing a build never rewrites a preset.",
            "Каждая версия общая для всех пресетов. В строке версии видно, какие пресеты её закрепили; здесь же установка, обновление, проверка и удаление. Установка сборки не переписывает пресеты.",
        ))
        .build();
    page.add(&catalog_group);
    *app.miner_library_group.borrow_mut() = Some(catalog_group);

    // The rollback/move surface: one row per preset, never a bulk rewrite.
    let preset_versions_group = adw::PreferencesGroup::builder()
        .title(tr("Preset versions", "Версии в пресетах"))
        .description(tr(
            "Switch one preset to another installed version (rollback or forward). Only the preset you pick changes; other presets keep their pins.",
            "Переведите один пресет на другую установленную версию (откат или вперёд). Меняется только выбранный пресет; остальные сохраняют закрепления.",
        ))
        .build();
    page.add(&preset_versions_group);
    *app.miner_preset_group.borrow_mut() = Some(preset_versions_group);

    refresh_miners(app);

    let bench_group = adw::PreferencesGroup::builder()
        .title(tr("A/B benchmark", "A/B сравнение"))
        .build();
    let minutes = spin_row(
        tr("Minutes per miner", "Минут на майнер").as_str(),
        "",
        3.0,
        1.0,
        30.0,
    );
    let run = gtk::Button::builder()
        .label(tr("Run benchmark", "Сравнить"))
        .css_classes(["flat", "pill"])
        .valign(gtk::Align::Center)
        .build();
    minutes.add_suffix(&run);
    bench_group.add(&minutes);
    page.add(&bench_group);
    {
        let tx = app.tx.clone();
        let minutes = minutes.clone();
        run.connect_clicked(move |_| {
            let _ = tx.send(Cmd::Bench {
                minutes: minutes.value() as u64,
            });
        });
    }

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&dialog_body(&page, &footer)));
    dialog.set_child(Some(&toolbar));

    {
        let dialog = dialog.clone();
        close.connect_clicked(move |_| {
            dialog.close();
        });
    }
    {
        let app = app.clone();
        dialog.connect_closed(move |_| {
            app.miner_library_group.borrow_mut().take();
            app.miner_rows.borrow_mut().clear();
            app.miner_preset_group.borrow_mut().take();
            app.miner_preset_rows.borrow_mut().clear();
            app.miner_library_dialog.borrow_mut().take();
            app.miner_verified.borrow_mut().clear();
            if let Some(callback) = &on_close {
                callback();
            }
        });
    }

    *app.miner_library_dialog.borrow_mut() = Some(dialog.clone());
    dialog.present(Some(&app.window));
    if let Some(path) = std::env::var_os("TROMMEL_LIBRARY_SCREENSHOT") {
        let path = path.to_string_lossy().into_owned();
        let capture = app.window.clone();
        let application = app.window.application();
        glib::timeout_add_seconds_local(2, move || {
            if let Some(application) = application.clone() {
                schedule_screenshot(capture.clone(), path.clone(), application, 0);
            }
            glib::ControlFlow::Break
        });
    }
}

/// Rebuild the version manager and the preset switch rows while the library is
/// open. It only renders the draft: nothing is installed, removed or re-pinned
/// here.
fn refresh_miners(app: &Rc<App>) {
    let Some(group) = app.miner_library_group.borrow().clone() else {
        return;
    };
    for row in app.miner_rows.borrow_mut().drain(..) {
        group.remove(&row);
    }
    // Clone: rebuilding combos must not hold the draft borrow across GTK
    // notifications that may ask for it again.
    let cfg = app.config.borrow().clone();

    // Miners with installed versions come first: the manager is opened to act
    // on a build, not to browse an unused catalogue entry.
    let mut miner_ids = versions::miner_ids(&cfg);
    miner_ids.sort_by(|a, b| {
        let a_installed = cfg
            .miner_installs
            .values()
            .any(|install| &install.miner == a);
        let b_installed = cfg
            .miner_installs
            .values()
            .any(|install| &install.miner == b);
        b_installed.cmp(&a_installed).then_with(|| a.cmp(b))
    });
    for miner_id in miner_ids {
        let def = catalog::miner(&miner_id);
        let name = def
            .map(|miner| miner.name.clone())
            .unwrap_or_else(|| miner_id.clone());
        let installed = versions::installed(&cfg, &miner_id);
        let users = versions::presets_for_miner(&cfg, &miner_id);
        let mut subtitle = def
            .map(|miner| miner.short_description().to_string())
            .unwrap_or_else(|| tr("Custom build", "Своя сборка"));
        if !users.is_empty() {
            subtitle.push('\n');
            subtitle.push_str(&format!(
                "{}: {}",
                tr("Used by", "Используется"),
                users
                    .iter()
                    .map(|preset| format!("{} ({})", preset.name, preset.miner_version))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let row = adw::ActionRow::builder()
            .title(&name)
            .subtitle(&subtitle)
            .build();
        row.set_activatable(true);
        row.set_subtitle_lines(2);
        let hint = if installed.is_empty() {
            tr(
                "Click to install a version. Install once and every preset can use it.",
                "Нажмите, чтобы установить версию. Одна установка доступна всем пресетам.",
            )
        } else {
            tr(
                "Click to install or update. Installed versions are shared by every preset.",
                "Нажмите, чтобы установить или обновить. Установленные версии общие для пресетов.",
            )
        };
        row.set_tooltip_text(Some(&hint));
        if installed.is_empty() {
            let state = label(
                &tr("not installed", "не установлен"),
                &["caption", "warning"],
            );
            state.set_valign(gtk::Align::Center);
            row.add_suffix(&state);
        } else {
            let versions_label = label(
                &installed
                    .iter()
                    .map(|item| item.version.clone())
                    .collect::<Vec<_>>()
                    .join(" · "),
                &["caption", "success"],
            );
            versions_label.set_valign(gtk::Align::Center);
            row.add_suffix(&versions_label);
        }
        let chevron = gtk::Button::builder()
            .icon_name("go-next-symbolic")
            .css_classes(["flat"])
            .valign(gtk::Align::Center)
            .tooltip_text(tr(
                "Install or update this miner",
                "Установить или обновить майнер",
            ))
            .build();
        let app_row = app.clone();
        let miner_row_id = miner_id.clone();
        let version = versions::latest_version(&miner_id).unwrap_or_default();
        {
            let app_chev = app.clone();
            let miner_chev = miner_id.clone();
            let version_chev = version.clone();
            chevron.connect_clicked(move |_| {
                miner_install_dialog(&app_chev, &miner_chev, version_chev.clone(), None);
            });
        }
        row.add_suffix(&chevron);
        row.connect_activated(move |_| {
            miner_install_dialog(&app_row, &miner_row_id, version.clone(), None);
        });
        group.add(&row);
        app.miner_rows.borrow_mut().push(row);

        // One explicit row per installed version: usage, binary state and the
        // verify/remove actions. The install dialog above stays the only
        // installer, so "update" is the same explicit install flow.
        for item in installed {
            let subtitle = version_row_subtitle(&item);
            let version_row = adw::ActionRow::builder()
                .title(&item.version)
                .subtitle(&subtitle)
                .build();
            version_row.set_subtitle_lines(2);
            version_row.add_prefix(&gtk::Image::from_icon_name(
                "application-x-executable-symbolic",
            ));
            if item.custom {
                let chip = label(&tr("custom", "своя сборка"), &["caption"]);
                chip.set_valign(gtk::Align::Center);
                version_row.add_suffix(&chip);
            }
            if !item.status.is_ok() {
                let chip = label(item.status.as_str(), &["caption", "warning"]);
                chip.set_valign(gtk::Align::Center);
                version_row.add_suffix(&chip);
            }
            if let Some(status) = app.miner_verified.borrow().get(&item.key) {
                let chip = label(
                    &tr("verified", "проверено"),
                    &[
                        "caption",
                        if status.is_ok() { "success" } else { "warning" },
                    ],
                );
                chip.set_valign(gtk::Align::Center);
                chip.set_tooltip_text(Some(&tr(
                    "Result of the explicit Verify action",
                    "Результат явной проверки",
                )));
                version_row.add_suffix(&chip);
            }
            let verify = gtk::Button::builder()
                .icon_name("object-select-symbolic")
                .css_classes(["flat"])
                .valign(gtk::Align::Center)
                .tooltip_text(tr("Verify the binary", "Проверить бинарник"))
                .build();
            {
                let app = app.clone();
                let miner = item.miner.clone();
                let version = item.version.clone();
                verify.connect_clicked(move |_| verify_miner_version(&app, &miner, &version));
            }
            version_row.add_suffix(&verify);
            if let Some(def) = catalog::miner(&item.miner) {
                if !def.algorithm_probe_arg.is_empty() {
                    let check = gtk::Button::builder()
                        .icon_name("system-search-symbolic")
                        .css_classes(["flat"])
                        .valign(gtk::Align::Center)
                        .tooltip_text(tr(
                            "Ask the installed binary which algorithms it advertises",
                            "Спросить установленный бинарник, какие алгоритмы он заявляет",
                        ))
                        .build();
                    {
                        let app = app.clone();
                        let miner = item.miner.clone();
                        let version = item.version.clone();
                        let path = item.path.clone();
                        let argument = def.algorithm_probe_arg.clone();
                        check.connect_clicked(move |_| {
                            check_miner_capabilities(&app, &miner, &version, &path, &argument);
                        });
                    }
                    version_row.add_suffix(&check);
                }
            }
            let remove = gtk::Button::builder()
                .icon_name("user-trash-symbolic")
                .css_classes(["flat"])
                .valign(gtk::Align::Center)
                .tooltip_text(tr(
                    "Remove this version (blocked while a preset uses it)",
                    "Удалить версию (заблокировано, пока её использует пресет)",
                ))
                .build();
            {
                let app = app.clone();
                let miner = item.miner.clone();
                let version = item.version.clone();
                remove
                    .connect_clicked(move |_| remove_miner_version_dialog(&app, &miner, &version));
            }
            version_row.add_suffix(&remove);
            group.add(&version_row);
            app.miner_rows.borrow_mut().push(version_row);
        }
    }

    refresh_preset_version_rows(app, &cfg);
}

fn check_miner_capabilities(app: &Rc<App>, miner: &str, version: &str, path: &str, argument: &str) {
    match versions::probe_algorithms(std::path::Path::new(path), argument) {
        Ok(found) => {
            let name = catalog::miner(miner)
                .map(|def| def.name.clone())
                .unwrap_or_else(|| miner.to_string());
            let body = if found.is_empty() {
                tr(
                    "The miner answered, but no known algorithm names were found in its output.",
                    "Майнер ответил, но известных названий алгоритмов в выводе не найдено.",
                )
                .to_string()
            } else {
                found.join(" · ")
            };
            let dialog = adw::AlertDialog::builder()
                .heading(format!("{name} {version}"))
                .body(body)
                .build();
            dialog.add_response("close", &tr("Close", "Закрыть"));
            dialog.set_close_response("close");
            dialog.present(Some(&app.window));
        }
        Err(err) => show_error_details(app, &err.to_string()),
    }
}

/// Subtitle of one installed-version row: who pins it, binary state and the
/// recorded-checksum fact (never a computed one).
fn version_row_subtitle(item: &versions::InstalledVersion) -> String {
    let mut parts = Vec::new();
    if item.used_by.is_empty() {
        parts.push(tr(
            "No preset uses this version",
            "Ни один пресет не использует эту версию",
        ));
    } else {
        parts.push(format!(
            "{}: {}",
            tr("Used by", "Используется"),
            item.used_by
                .iter()
                .map(|preset| format!("{} ({})", preset.name, preset.miner_version))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    match item.status {
        versions::BinaryStatus::Missing => {
            parts.push(tr("binary missing", "бинарник отсутствует").to_string())
        }
        versions::BinaryStatus::NotAFile => parts.push(tr("not a file", "не файл").to_string()),
        versions::BinaryStatus::NotExecutable => {
            parts.push(tr("not executable", "не исполняем").to_string())
        }
        versions::BinaryStatus::NotReadable => {
            parts.push(tr("not readable", "недоступен для чтения").to_string())
        }
        versions::BinaryStatus::NotRecorded => {
            parts.push(tr("no install record", "нет записи об установке").to_string())
        }
        versions::BinaryStatus::Ok => {}
    }
    if item.sha256_verified {
        parts.push(tr("SHA-256 verified", "SHA-256 проверен").to_string());
    } else if item.recorded_sha256.is_some() {
        parts.push(
            tr(
                "SHA-256 recorded, not verified",
                "SHA-256 записан, не проверен",
            )
            .to_string(),
        );
    } else {
        parts.push(tr("no SHA-256 recorded", "SHA-256 не записан").to_string());
    }
    parts.join(" · ")
}

/// One switch per preset: the explicit rollback/move surface. The combo never
/// applies on its own; a confirmation then pins exactly that preset.
fn refresh_preset_version_rows(app: &Rc<App>, cfg: &Config) {
    let Some(group) = app.miner_preset_group.borrow().clone() else {
        return;
    };
    for row in app.miner_preset_rows.borrow_mut().drain(..) {
        group.remove(&row);
    }
    let mut presets: Vec<Preset> = cfg.presets.values().cloned().collect();
    presets.sort_by(|a, b| a.name.cmp(&b.name));
    for preset in presets {
        let usable: Vec<String> = versions::installed(cfg, &preset.miner)
            .into_iter()
            .filter(|item| item.status.is_ok())
            .map(|item| item.version)
            .collect();
        let missing = !usable.contains(&preset.miner_version);
        let mut labels: Vec<String> = Vec::new();
        let mut targets: Vec<Option<String>> = Vec::new();
        if missing {
            labels.push(format!(
                "{} ({})",
                preset.miner_version,
                tr("missing", "нет")
            ));
            targets.push(None);
        }
        for version in &usable {
            labels.push(version.clone());
            targets.push(Some(version.clone()));
        }
        if labels.is_empty() {
            continue;
        }
        let current_index = targets
            .iter()
            .position(|target| target.as_deref() == Some(preset.miner_version.as_str()))
            .unwrap_or(0);
        let combo = adw::ComboRow::builder()
            .title(&preset.name)
            .subtitle(format!(
                "{} · {} {}",
                preset.miner,
                tr("pinned", "закреплено"),
                preset.miner_version
            ))
            .build();
        combo.set_model(Some(&string_list(&labels)));
        combo.set_selected(current_index as u32);
        {
            let app = app.clone();
            let preset_id = preset.id.clone();
            let preset_name = preset.name.clone();
            let miner = preset.miner.clone();
            let current_version = preset.miner_version.clone();
            let guard = Rc::new(Cell::new(false));
            let guard_for_revert = guard.clone();
            let combo_weak = combo.downgrade();
            combo.connect_notify_local(Some("selected"), move |row, _| {
                if guard.get() {
                    return;
                }
                let index = row.selected() as usize;
                let Some(Some(target)) = targets.get(index).cloned() else {
                    return;
                };
                if target == current_version {
                    return;
                }
                let revert: Rc<dyn Fn()> = {
                    let guard = guard_for_revert.clone();
                    let combo_weak = combo_weak.clone();
                    Rc::new(move || {
                        guard.set(true);
                        if let Some(combo) = combo_weak.upgrade() {
                            combo.set_selected(current_index as u32);
                        }
                        guard.set(false);
                    })
                };
                confirm_preset_version(
                    &app,
                    preset_id.clone(),
                    preset_name.clone(),
                    miner.clone(),
                    current_version.clone(),
                    target,
                    Some(revert),
                );
            });
        }
        group.add(&combo);
        app.miner_preset_rows.borrow_mut().push(combo.upcast());
    }
}

/// Ask before pinning one preset; on cancel the combo goes back to the pin.
fn confirm_preset_version(
    app: &Rc<App>,
    preset_id: String,
    preset_name: String,
    miner: String,
    from: String,
    to: String,
    on_cancel: Option<Rc<dyn Fn()>>,
) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Switch preset version?", "Сменить версию пресета?"))
        .body(format!(
            "{}: {from} → {to}.\n\n{}",
            preset_name,
            tr(
                "Only this preset changes. Other presets keep their pinned versions; a running miner continues on its session until the change is applied.",
                "Меняется только этот пресет. Другие пресеты сохраняют закреплённые версии; работающий майнер продолжает сессию до применения изменения."
            )
        ))
        .build();
    dialog.add_response("cancel", &tr("Cancel", "Отмена"));
    dialog.add_response("switch", &tr("Switch", "Сменить"));
    dialog.set_response_appearance("switch", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("switch"));
    dialog.set_close_response("cancel");
    let app = app.clone();
    let window = app.window.clone();
    dialog.choose(Some(&window), None::<&gio::Cancellable>, move |response| {
        if response != "switch" {
            if let Some(cancel) = &on_cancel {
                cancel();
            }
            return;
        }
        let change = {
            let mut cfg = app.config.borrow_mut();
            versions::pin_preset(&mut cfg, &preset_id, &to)
        };
        match change {
            Ok(Some(change)) => {
                mark_dirty(&app);
                refresh_preset_list(&app);
                close_miner_library(&app);
                save_and_apply(&app);
                app.toast.add_toast(adw::Toast::new(&tr(
                    &format!(
                        "{} now uses {} {}; other presets are unchanged.",
                        change.name, miner, change.to
                    ),
                    &format!(
                        "{} теперь использует {} {}; другие пресеты не изменены.",
                        change.name, miner, change.to
                    ),
                )));
            }
            Ok(None) => {}
            Err(err) => show_error_toast(&app, &err.to_string()),
        }
    });
}

fn close_miner_library(app: &Rc<App>) {
    if let Some(dialog) = app.miner_library_dialog.borrow().clone() {
        dialog.close();
    }
}

/// Check one binary and report the result without inventing a checksum.
fn verify_miner_version(app: &Rc<App>, miner: &str, version: &str) {
    let report = versions::verify(&app.config.borrow(), miner, version);
    app.miner_verified.borrow_mut().insert(
        trommel_core::config::install_key(miner, version),
        report.status,
    );
    let name = catalog::miner(miner)
        .map(|def| def.name.clone())
        .unwrap_or_else(|| miner.to_string());
    let message = format!("{name} {version}: {}", report.message);
    if report.status.is_ok() {
        app.toast.add_toast(adw::Toast::new(&message));
    } else {
        show_error_toast(app, &message);
    }
    refresh_miners(app);
}

/// Remove an unused version only after an explicit confirmation; a version any
/// preset still pins is refused with the list of presets.
fn remove_miner_version_dialog(app: &Rc<App>, miner: &str, version: &str) {
    // Plan, delete and persist from the same snapshot. The draft (`config`) and
    // the applied config can differ; mixing them produced a plan that did not
    // describe the record actually deleted.
    let snapshot = app.applied.borrow().clone();
    let plan = versions::remove_plan(&snapshot, miner, version);
    let name = catalog::miner(miner)
        .map(|def| def.name.clone())
        .unwrap_or_else(|| miner.to_string());
    if !plan.used_by.is_empty() {
        let users = plan
            .used_by
            .iter()
            .map(|preset| format!("{} ({})", preset.name, preset.miner_version))
            .collect::<Vec<_>>()
            .join("\n");
        let dialog = adw::AlertDialog::builder()
            .heading(tr("This version is in use", "Эта версия используется"))
            .body(format!(
                "{} {version} {}:\n\n{users}\n\n{}",
                name,
                tr("is pinned by", "закреплена в пресетах:"),
                tr(
                    "Switch those presets to another installed version first. Removing a build never rewrites a preset.",
                    "Сначала переведите эти пресеты на другую установленную версию. Удаление сборки не переписывает пресеты."
                )
            ))
            .build();
        dialog.add_response("close", &tr("Close", "Закрыть"));
        dialog.set_default_response(Some("close"));
        dialog.set_close_response("close");
        dialog.choose(Some(&app.window), None::<&gio::Cancellable>, |_| {});
        return;
    }
    if !plan.allowed {
        show_error_toast(
            app,
            &tr(
                "This version is not installed.",
                "Эта версия не установлена.",
            ),
        );
        refresh_miners(app);
        return;
    }
    let detail = if plan.delete_dir.is_some() {
        tr(
            "The managed binary is deleted.",
            "Управляемый бинарник будет удалён.",
        )
    } else {
        tr(
            "The recorded binary is outside the managed store; only the install record is removed.",
            "Записанный бинарник находится вне управляемого хранилища; будет удалена только запись об установке.",
        )
    };
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Remove miner version?", "Удалить версию майнера?"))
        .body(format!(
            "{name} {version}\n\n{detail} {}",
            tr(
                "No preset is changed and no other version is removed.",
                "Ни один пресет не изменится, другие версии останутся."
            )
        ))
        .build();
    dialog.add_response("cancel", &tr("Cancel", "Отмена"));
    dialog.add_response("remove", &tr("Remove", "Удалить"));
    dialog.set_response_appearance("remove", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let app = app.clone();
    let miner = miner.to_string();
    let version = version.to_string();
    let window = app.window.clone();
    dialog.choose(Some(&window), None::<&gio::Cancellable>, move |response| {
        if response != "remove" {
            return;
        }
        // One snapshot for the check, the filesystem removal and the saved
        // config. A failed `save()` is reported before any success toast and
        // leaves both the GUI copies and the file untouched.
        match versions::remove_and_persist(
            &snapshot,
            &miner,
            &version,
            &trommel_core::download::miner_root(),
            |cfg| cfg.save(),
        ) {
            Ok((next, report)) => {
                *app.applied.borrow_mut() = next.clone();
                app.config.borrow_mut().remove_install(&miner, &version);
                if let Ok(json) = serde_json::to_string(&next) {
                    let _ = app.tx.send(Cmd::Apply {
                        json,
                        mode: "live".into(),
                        quiet: true,
                    });
                }
                refresh_preset_list(&app);
                refresh_miners(&app);
                let message = if report.removed_files {
                    format!(
                        "{} {version} {}",
                        name,
                        tr(
                            "removed with its managed files; presets are unchanged.",
                            "удалена вместе с файлами; пресеты не изменены."
                        )
                    )
                } else {
                    format!(
                        "{} {version} {}",
                        name,
                        tr(
                            "install record removed; the external binary was left untouched.",
                            "запись об установке удалена; внешний бинарник оставлен без изменений."
                        )
                    )
                };
                app.toast.add_toast(adw::Toast::new(&message));
            }
            Err(err) => {
                show_error_toast(&app, &err.to_string());
                refresh_miners(&app);
            }
        }
    });
}

/// After a successful install, list the presets that still pin another version
/// and let the user pick which ones move. Everything stays as it was by
/// default, including the close/escape path.
fn miner_update_choice_dialog(app: &Rc<App>, miner: &str, version: &str) {
    let plan = versions::update_plan(&app.config.borrow(), miner, version);
    if plan.affected.is_empty() {
        return;
    }
    let name = catalog::miner(miner)
        .map(|def| def.name.clone())
        .unwrap_or_else(|| miner.to_string());
    let dialog = adw::AlertDialog::builder()
        .heading(tr(
            "Move presets to this version?",
            "Перевести пресеты на эту версию?",
        ))
        .body(format!(
            "{} {version} {}.\n\n{}",
            name,
            tr(
                "is installed; every preset keeps its pinned version by default.",
                "установлена; по умолчанию каждый пресет сохраняет закреплённую версию."
            ),
            tr(
                "Select the presets to move to the new version:",
                "Отметьте пресеты, которые нужно перевести на новую версию:"
            )
        ))
        .build();
    let list = gtk::Box::new(gtk::Orientation::Vertical, 6);
    list.set_margin_top(12);
    list.set_margin_bottom(12);
    list.set_margin_start(12);
    list.set_margin_end(12);
    let mut checks: Vec<(String, gtk::CheckButton)> = Vec::new();
    for preset in &plan.affected {
        let check = gtk::CheckButton::with_label(&format!(
            "{} — {} → {version}",
            preset.name, preset.miner_version
        ));
        check.set_active(false);
        list.append(&check);
        checks.push((preset.id.clone(), check));
    }
    dialog.set_extra_child(Some(&list));
    dialog.set_content_width(460);
    dialog.add_response(
        "keep",
        &tr("Keep preset versions", "Оставить версии пресетов"),
    );
    dialog.add_response("move", &tr("Move selected", "Перевести выбранные"));
    dialog.set_response_appearance("move", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("keep"));
    dialog.set_close_response("keep");
    let app = app.clone();
    let miner = miner.to_string();
    let version = version.to_string();
    let window = app.window.clone();
    dialog.choose(Some(&window), None::<&gio::Cancellable>, move |response| {
        if response != "move" {
            return;
        }
        let ids: Vec<String> = checks
            .iter()
            .filter(|(_, check)| check.is_active())
            .map(|(id, _)| id.clone())
            .collect();
        if ids.is_empty() {
            app.toast.add_toast(adw::Toast::new(&tr(
                "No preset was selected; every pin is unchanged.",
                "Ни один пресет не выбран; закрепления не изменены.",
            )));
            return;
        }
        let changes = {
            let mut cfg = app.config.borrow_mut();
            versions::pin_presets(&mut cfg, &ids, &miner, &version)
        };
        match changes {
            Ok(changes) if !changes.is_empty() => {
                mark_dirty(&app);
                refresh_preset_list(&app);
                close_miner_library(&app);
                save_and_apply(&app);
                let names = changes
                    .iter()
                    .map(|change| change.name.clone())
                    .collect::<Vec<_>>()
                    .join(", ");
                app.toast.add_toast(adw::Toast::new(&tr(
                    &format!("Moved {names} to {version}; other presets keep their versions."),
                    &format!(
                        "{names} переведены на {version}; остальные пресеты сохранили версии."
                    ),
                )));
            }
            Ok(_) => {}
            Err(err) => show_error_toast(&app, &err.to_string()),
        }
    });
}

fn copy_diagnostics(app: &Rc<App>) {
    let report = {
        let cfg = app.config.borrow();
        let status = app.status.borrow();
        trommel_core::diagnostics::report(&cfg, status.as_ref())
    };
    let text = serde_json::to_string_pretty(&report).unwrap_or_default();
    if let Some(display) = gdk::Display::default() {
        display.clipboard().set_text(&text);
        app.toast.add_toast(adw::Toast::new(&tr(
            "Diagnostics copied (wallet redacted).",
            "Диагностика скопирована (кошелёк скрыт).",
        )));
    } else {
        app.toast.add_toast(adw::Toast::new(&tr(
            "No clipboard available.",
            "Буфер обмена недоступен.",
        )));
    }
}

fn gpu_tune_dialog(app: &Rc<App>, gpu: &GpuStats) {
    if app.config.borrow().preset().is_none() {
        create_preset_alert(app);
        return;
    }
    let uuid = gpu.uuid.clone();
    let name = gpu.name.clone();
    let (profile, active_preset_name) = {
        let cfg = app.config.borrow();
        (
            cfg.gpu_profile().cloned().unwrap_or_default(),
            cfg.preset()
                .map(|preset| preset.name.clone())
                .unwrap_or_default(),
        )
    };
    let tuning = profile.tuning_for(&uuid);
    let enabled = tuning.enabled;
    let base_tuning = tuning.clone();
    let selected_preset: Rc<RefCell<String>> = Rc::new(RefCell::new(tuning.gpu_preset.clone()));
    let saved_presets: Vec<_> = app
        .config
        .borrow()
        .gpu_presets
        .values()
        .filter(|preset| preset.gpu_uuid == uuid || preset.gpu_name == name)
        .cloned()
        .collect();
    let (same_uuids, all_uuids) = {
        let status = app.status.borrow();
        match status.as_ref() {
            Some(status) => {
                let all: Vec<String> = status.gpus.iter().map(|gpu| gpu.uuid.clone()).collect();
                let same: Vec<String> = status
                    .gpus
                    .iter()
                    .filter(|item| item.name == name)
                    .map(|item| item.uuid.clone())
                    .collect();
                (same, all)
            }
            None => (vec![uuid.clone()], vec![uuid.clone()]),
        }
    };

    let dialog = adw::Dialog::builder()
        .title(format!("{name} · {}", tr("GPU tuning", "Настройка GPU")))
        .content_width(540)
        .build();
    let header = adw::HeaderBar::new();
    header.set_show_end_title_buttons(false);
    let title = label(&format!("GPU #{} · {name}", gpu.index), &["heading"]);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    header.set_title_widget(Some(&title));

    let cancel = gtk::Button::builder()
        .label(tr("Cancel", "Отмена"))
        .css_classes(["flat", "pill"])
        .build();
    let presets_popover = gtk::Popover::new();
    let presets_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    presets_box.set_margin_top(6);
    presets_box.set_margin_bottom(6);
    presets_box.set_margin_start(6);
    presets_box.set_margin_end(6);
    let presets = gtk::MenuButton::builder()
        .label(tr("GPU presets", "Пресеты GPU"))
        .tooltip_text(tr(
            "Stock or a saved per-card tuning; nothing is applied until Save",
            "Сток или сохранённая настройка карты; применяется только по Save",
        ))
        .css_classes(["flat", "pill"])
        .always_show_arrow(true)
        .popover(&presets_popover)
        .build();
    let save = gtk::Button::builder()
        .label(tr("Save", "Сохранить"))
        .css_classes(["suggested-action", "pill"])
        .build();
    let footer = dialog_footer();
    footer.append(&cancel);
    footer.append(&presets);
    footer.append(&dialog_footer_spacer());
    footer.append(&save);
    let page = adw::PreferencesPage::new();
    page.set_vexpand(true);

    let group = adw::PreferencesGroup::builder()
        .title(format!("GPU #{} · {name}", gpu.index))
        .description(format!(
            "{}: {active_preset_name}",
            tr("Active preset", "Активный пресет")
        ))
        .build();
    let now = adw::ActionRow::builder()
        .title(tr("Now", "Сейчас"))
        .subtitle(format!(
            "{} · {} · {}",
            gpu.power_w
                .map(|value| format!("{value:.0} W"))
                .unwrap_or_else(|| tr("power —", "питание —")),
            match (gpu.core_mhz, gpu.mem_mhz) {
                (Some(core), Some(mem)) => format!(
                    "{} {} MHz · {} {} MHz",
                    tr("core", "ядро"),
                    core,
                    tr("memory", "память"),
                    mem
                ),
                _ => tr("clocks —", "частоты —"),
            },
            gpu.temp_c
                .map(|value| format!("{value} °C"))
                .unwrap_or_else(|| tr("temp —", "темп. —"))
        ))
        .build();
    now.set_activatable(false);
    group.add(&now);
    let power = spin_row(
        &tr("Power limit", "Лимит питания"),
        "W — 0 = auto",
        tuning.power_limit_w.unwrap_or(0) as f64,
        0.0,
        1000.0,
    );
    let core_now = gpu
        .core_mhz
        .map(|value| format!("{value} MHz"))
        .unwrap_or_else(|| tr("unknown", "неизвестно"));
    let memory_now = gpu
        .mem_mhz
        .map(|value| format!("{value} MHz"))
        .unwrap_or_else(|| tr("unknown", "неизвестно"));
    let core_lock = switch_row(
        &tr("Lock core clock", "Лочить частоту ядра"),
        &if tuning.core_clock_mhz.is_some() {
            tr(
                "Locked. Turn off for driver auto.",
                "Лок включён. Выключите для авто драйвера.",
            )
        } else {
            tr(
                &format!("Off = driver auto (now {core_now})."),
                &format!("Выкл. = авто драйвера (сейчас {core_now})."),
            )
        },
    );
    core_lock.set_active(tuning.core_clock_mhz.is_some());
    let core = spin_row(
        &tr("Core clock", "Частота ядра"),
        "MHz",
        tuning.core_clock_mhz.unwrap_or(1620) as f64,
        100.0,
        4000.0,
    );
    core.set_sensitive(core_lock.is_active());
    let memory_lock = switch_row(
        &tr("Lock memory clock", "Лочить частоту памяти"),
        &if tuning.memory_clock_mhz.is_some() {
            tr(
                "Locked. A low lock can save power; it can hurt other coins.",
                "Лок включён. Низкий лок экономит питание, но может навредить другим монетам.",
            )
        } else {
            tr(
                &format!("Off = driver auto (now {memory_now})."),
                &format!("Выкл. = авто драйвера (сейчас {memory_now})."),
            )
        },
    );
    memory_lock.set_active(tuning.memory_clock_mhz.is_some());
    let memory = spin_row(
        &tr("Memory clock", "Частота памяти"),
        "MHz",
        tuning.memory_clock_mhz.unwrap_or(810) as f64,
        100.0,
        12000.0,
    );
    memory.set_sensitive(memory_lock.is_active());
    let simple = app.config.borrow().behavior.simple_mode;
    core_lock.set_visible(!simple);
    memory_lock.set_visible(!simple);
    group.add(&power);
    group.add(&core_lock);
    group.add(&core);
    group.add(&memory_lock);
    group.add(&memory);
    sync_lock_row(&core_lock, &core, !simple);
    sync_lock_row(&memory_lock, &memory, !simple);

    let source_hint = label("", &["caption", "dim-label"]);
    source_hint.set_margin_start(6);
    source_hint.set_margin_top(6);
    source_hint.set_wrap(true);
    group.add(&source_hint);
    let global_hint = label(
        &tr(
            "Temperature limit is global (Settings → GPU); fan speed is a miner option in the preset editor.",
            "Порог температуры — глобальный (Настройки → GPU); вентилятор — опция майнера в редакторе пресета.",
        ),
        &["caption", "dim-label"],
    );
    global_hint.set_margin_start(6);
    global_hint.set_margin_top(4);
    global_hint.set_wrap(true);
    group.add(&global_hint);
    page.add(&group);

    let mut apply_similar = None;
    let mut apply_all = None;
    if all_uuids.len() > 1 {
        let group = adw::PreferencesGroup::builder()
            .title(tr("Apply to", "Применить к"))
            .build();
        let similar = switch_row(
            &tr("Similar GPUs", "Одинаковые карты"),
            &tr(
                "Also apply to GPUs with the same model",
                "Также применить к картам той же модели",
            ),
        );
        similar.set_visible(same_uuids.len() > 1);
        let all = switch_row(
            &tr("All GPUs", "Все карты"),
            &tr(
                "Apply to every detected GPU",
                "Применить ко всем обнаруженным картам",
            ),
        );
        group.add(&similar);
        group.add(&all);
        let hint = label(
            &tr(
                "Each card has its own settings. Use the switches to copy these values; copied cards become custom.",
                "У каждой карты свои настройки. Переключатели копируют значения; копии становятся custom.",
            ),
            &["caption", "dim-label"],
        );
        hint.set_margin_start(6);
        hint.set_margin_top(4);
        hint.set_wrap(true);
        group.add(&hint);
        page.add(&group);
        apply_similar = Some(similar);
        apply_all = Some(all);
    }
    let apply_similar = apply_similar;
    let apply_all = apply_all;
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&dialog_body(&page, &footer)));
    dialog.set_child(Some(&toolbar));

    let make_tuning: Rc<dyn Fn() -> GpuTuning> = {
        let power = power.clone();
        let core = core.clone();
        let core_lock = core_lock.clone();
        let memory = memory.clone();
        let memory_lock = memory_lock.clone();
        let base_tuning = base_tuning.clone();
        Rc::new(move || {
            let mut tuning = base_tuning.clone();
            tuning.custom = true;
            tuning.gpu_preset.clear();
            tuning.power_limit_w = nonzero(power.value() as u32);
            tuning.core_clock_mhz = if core_lock.is_active() && core.value() as u32 >= 100 {
                Some(core.value() as u32)
            } else {
                None
            };
            tuning.memory_clock_mhz = if memory_lock.is_active() && memory.value() as u32 >= 100 {
                Some(memory.value() as u32)
            } else {
                None
            };
            tuning
        })
    };
    let set_tuning: Rc<dyn Fn(&GpuTuning)> = {
        let power = power.clone();
        let core = core.clone();
        let core_lock = core_lock.clone();
        let memory = memory.clone();
        let memory_lock = memory_lock.clone();
        Rc::new(move |tuning: &GpuTuning| {
            power.set_value(tuning.power_limit_w.unwrap_or(0) as f64);
            core_lock.set_active(tuning.core_clock_mhz.is_some());
            core.set_value(tuning.core_clock_mhz.unwrap_or(1620) as f64);
            memory_lock.set_active(tuning.memory_clock_mhz.is_some());
            memory.set_value(tuning.memory_clock_mhz.unwrap_or(810) as f64);
        })
    };
    let describe_source: Rc<dyn Fn()> = {
        let app = app.clone();
        let selected = selected_preset.clone();
        let base_tuning = base_tuning.clone();
        let make_tuning = make_tuning.clone();
        let source_hint = source_hint.clone();
        Rc::new(move || {
            let current = make_tuning();
            let selected_id = selected.borrow().clone();
            let text = if !selected_id.is_empty() {
                let name = app
                    .config
                    .borrow()
                    .gpu_presets
                    .get(&selected_id)
                    .map(|preset| preset.name.clone())
                    .unwrap_or_else(|| tr("Unknown preset", "Неизвестный пресет").to_string());
                format!("{}: {name}", tr("GPU preset", "Пресет GPU"))
            } else if current.has_tuning() || current.enabled != base_tuning.enabled {
                tr(
                    "Custom values for this card; they are stored with the active mining preset.",
                    "Свои значения для этой карты; они хранятся в активном пресете майнинга.",
                )
                .to_string()
            } else {
                tr(
                    "Stock: driver auto power and clocks. The card still belongs to the active preset.",
                    "Сток: авто-питание и частоты драйвера. Карта по-прежнему относится к активному пресету.",
                )
                .to_string()
            };
            source_hint.set_text(&tr(&text, &text));
        })
    };
    describe_source();

    let stock = gtk::Button::builder()
        .label(tr("Stock (default)", "Сток (по умолчанию)"))
        .tooltip_text(tr(
            "Driver auto power, no clock locks",
            "Авто-питание драйвера, без локов частот",
        ))
        .css_classes(["flat"])
        .halign(gtk::Align::Fill)
        .build();
    {
        let set_tuning = set_tuning.clone();
        let selected = selected_preset.clone();
        let describe = describe_source.clone();
        let popover = presets_popover.clone();
        stock.connect_clicked(move |_| {
            let tuning = GpuTuning {
                enabled,
                ..Default::default()
            };
            set_tuning(&tuning);
            selected.borrow_mut().clear();
            describe();
            popover.popdown();
        });
    }
    presets_box.append(&stock);
    if !saved_presets.is_empty() {
        presets_box.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        for preset in &saved_presets {
            let selected_id = selected_preset.borrow().clone();
            let check = if !selected_id.is_empty() && selected_id == preset.id {
                "✓ "
            } else {
                ""
            };
            let button = gtk::Button::builder()
                .label(format!("{check}{}", preset.name))
                .tooltip_text(format!(
                    "{} · {}",
                    preset.gpu_name,
                    tuning_summary(&preset.tuning)
                ))
                .css_classes(["flat"])
                .halign(gtk::Align::Fill)
                .build();
            let set_tuning = set_tuning.clone();
            let selected = selected_preset.clone();
            let describe = describe_source.clone();
            let id = preset.id.clone();
            let preset_tuning = preset.tuning.clone();
            let popover = presets_popover.clone();
            button.connect_clicked(move |_| {
                set_tuning(&preset_tuning);
                *selected.borrow_mut() = id.clone();
                describe();
                popover.popdown();
            });
            presets_box.append(&button);
        }
    }
    let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
    presets_box.append(&separator);
    let save_as = gtk::Button::builder()
        .label(tr("Save as GPU preset…", "Сохранить как пресет GPU…"))
        .css_classes(["flat"])
        .halign(gtk::Align::Fill)
        .build();
    {
        let app = app.clone();
        let uuid = uuid.clone();
        let gpu_name = name.clone();
        let make_tuning = make_tuning.clone();
        let selected = selected_preset.clone();
        let describe = describe_source.clone();
        save_as.connect_clicked(move |_| {
            let tuning = make_tuning();
            let app = app.clone();
            let uuid = uuid.clone();
            let gpu_name = gpu_name.clone();
            let selected = selected.clone();
            let describe = describe.clone();
            save_gpu_preset_dialog(
                &app,
                &uuid,
                &gpu_name,
                &tuning,
                Rc::new(move |id| {
                    *selected.borrow_mut() = id;
                    describe();
                }),
            );
        });
    }
    presets_box.append(&save_as);
    presets_popover.set_child(Some(&presets_box));

    {
        let core_row = core.clone();
        let memory_row = memory.clone();
        let core_now = core_now.clone();
        let memory_now = memory_now.clone();
        core_lock.connect_notify_local(Some("active"), move |row, _| {
            row.set_subtitle(&if row.is_active() {
                tr(
                    "Locked. Turn off for driver auto.",
                    "Лок включён. Выключите для авто драйвера.",
                )
            } else {
                tr(
                    &format!("Off = driver auto (now {core_now})."),
                    &format!("Выкл. = авто драйвера (сейчас {core_now})."),
                )
            });
            sync_lock_row(row, &core_row, true);
        });
        memory_lock.connect_notify_local(Some("active"), move |row, _| {
            row.set_subtitle(&if row.is_active() {
                tr(
                    "Locked. A low lock can save power; it can hurt other coins.",
                    "Лок включён. Низкий лок экономит питание, но может навредить другим монетам.",
                )
            } else {
                tr(
                    &format!("Off = driver auto (now {memory_now})."),
                    &format!("Выкл. = авто драйвера (сейчас {memory_now})."),
                )
            });
            sync_lock_row(row, &memory_row, true);
        });
        let describe = describe_source.clone();
        power.connect_notify_local(Some("value"), move |_, _| describe());
        let describe = describe_source.clone();
        core.connect_notify_local(Some("value"), move |_, _| describe());
        let describe = describe_source.clone();
        memory.connect_notify_local(Some("value"), move |_, _| describe());
        let describe = describe_source.clone();
        core_lock.connect_notify_local(Some("active"), move |_, _| describe());
        let describe = describe_source.clone();
        memory_lock.connect_notify_local(Some("active"), move |_, _| describe());
    }
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
    }
    {
        let app = app.clone();
        let dialog = dialog.clone();
        let uuid = uuid.clone();
        let same_uuids = same_uuids.clone();
        let all_uuids = all_uuids.clone();
        let selected = selected_preset.clone();
        let make_tuning = make_tuning.clone();
        save.connect_clicked(move |_| {
            let mut tuning = make_tuning();
            let mut targets = vec![uuid.clone()];
            if apply_all
                .as_ref()
                .map(|row| row.is_active())
                .unwrap_or(false)
            {
                targets = all_uuids.clone();
            } else if apply_similar
                .as_ref()
                .map(|row| row.is_active())
                .unwrap_or(false)
            {
                targets.extend(same_uuids.iter().cloned());
            }
            targets.sort();
            targets.dedup();
            let selected_id = selected.borrow().clone();
            if !selected_id.is_empty() {
                if let Some(preset) = app.config.borrow().gpu_presets.get(&selected_id) {
                    if !same_tuning(&preset.tuning, &tuning) {
                        tuning.gpu_preset.clear();
                    } else {
                        tuning.gpu_preset = selected_id;
                    }
                } else {
                    tuning.gpu_preset.clear();
                }
            }
            {
                let mut cfg = app.config.borrow_mut();
                let Some(profile) = cfg.gpu_profile_mut() else {
                    return;
                };
                let before = profile.tuning_for(&uuid);
                for target in &targets {
                    let mut value = tuning.clone();
                    if target != &uuid {
                        value.gpu_preset.clear();
                    }
                    profile.per_gpu.insert(target.clone(), value);
                }
                let after = profile.tuning_for(&uuid);
                let only_metadata = same_values(&before, &after)
                    && targets.len() == 1
                    && before.gpu_preset == after.gpu_preset;
                drop(cfg);
                if only_metadata {
                    dialog.close();
                    return;
                }
            }
            mark_dirty(&app);
            dialog.close();
            save_and_apply(&app);
        });
    }
    dialog.present(Some(&app.window));
    if let Some(path) = std::env::var_os("TROMMEL_TUNE_SCREENSHOT") {
        let path = path.to_string_lossy().into_owned();
        let capture = app.window.clone();
        let application = app.window.application();
        glib::timeout_add_seconds_local(2, move || {
            if let Some(application) = application.clone() {
                schedule_screenshot(capture.clone(), path.clone(), application, 0);
            }
            glib::ControlFlow::Break
        });
    }
}

fn same_values(left: &GpuTuning, right: &GpuTuning) -> bool {
    left.enabled == right.enabled
        && left.power_limit_w == right.power_limit_w
        && left.core_clock_mhz == right.core_clock_mhz
        && left.memory_clock_mhz == right.memory_clock_mhz
}

fn same_tuning(left: &GpuTuning, right: &GpuTuning) -> bool {
    same_values(left, right)
}

fn tuning_summary(tuning: &GpuTuning) -> String {
    let power = tuning
        .power_limit_w
        .map(|value| format!("{value} W"))
        .unwrap_or_else(|| tr("auto power", "авто-питание").to_string());
    let core = tuning
        .core_clock_mhz
        .map(|value| format!("{value} MHz core"))
        .unwrap_or_else(|| tr("auto core", "авто ядро").to_string());
    let memory = tuning
        .memory_clock_mhz
        .map(|value| format!("{value} MHz mem"))
        .unwrap_or_else(|| tr("auto mem", "авто память").to_string());
    format!("{power} · {core} · {memory}")
}

fn save_gpu_preset_dialog(
    app: &Rc<App>,
    gpu_uuid: &str,
    gpu_name: &str,
    tuning: &GpuTuning,
    on_saved: Rc<dyn Fn(String)>,
) {
    let dialog = adw::Dialog::builder()
        .title(tr("Save GPU preset", "Сохранить пресет GPU"))
        .content_width(420)
        .build();
    let header = adw::HeaderBar::new();
    header.set_show_end_title_buttons(false);
    let name = adw::EntryRow::builder()
        .title(tr("Preset name", "Название пресета"))
        .build();
    let group = adw::PreferencesGroup::builder()
        .title(format!("{gpu_name} · {gpu_uuid}"))
        .description(tuning_summary(tuning))
        .build();
    group.add(&name);
    let page = adw::PreferencesPage::new();
    page.add(&group);
    let cancel = gtk::Button::builder()
        .label(tr("Cancel", "Отмена"))
        .css_classes(["flat", "pill"])
        .build();
    let save = gtk::Button::builder()
        .label(tr("Save", "Сохранить"))
        .css_classes(["suggested-action", "pill"])
        .build();
    let status = label("", &["caption", "dim-label"]);
    let footer = dialog_footer();
    footer.append(&cancel);
    footer.append(&status);
    footer.append(&dialog_footer_spacer());
    footer.append(&save);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&dialog_body(&page, &footer)));
    dialog.set_child(Some(&toolbar));
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
    }
    {
        let app = app.clone();
        let dialog = dialog.clone();
        let name = name.clone();
        let status = status.clone();
        let gpu_uuid = gpu_uuid.to_string();
        let gpu_name = gpu_name.to_string();
        let mut tuning = tuning.clone();
        tuning.custom = true;
        tuning.gpu_preset.clear();
        save.connect_clicked(move |_| {
            let preset_name = name.text().trim().to_string();
            if preset_name.is_empty() {
                set_status_text(
                    &status,
                    &tr("Enter a preset name.", "Введите название."),
                    "warning",
                );
                return;
            }
            if app
                .config
                .borrow()
                .gpu_presets
                .values()
                .any(|preset| preset.name.eq_ignore_ascii_case(&preset_name))
            {
                set_status_text(
                    &status,
                    &tr(
                        "A GPU preset with this name already exists.",
                        "Пресет GPU с таким именем уже есть.",
                    ),
                    "warning",
                );
                return;
            }
            let now = util::now_unix();
            let id = format!("gpu-{now}-{}", app.config.borrow().gpu_presets.len());
            let preset = GpuPreset {
                id: id.clone(),
                name: preset_name,
                gpu_uuid: gpu_uuid.clone(),
                gpu_name: gpu_name.clone(),
                tuning: tuning.clone(),
                created_at: now,
                updated_at: now,
            };
            app.config
                .borrow_mut()
                .gpu_presets
                .insert(id.clone(), preset);
            mark_dirty(&app);
            dialog.close();
            on_saved(id);
            app.toast.add_toast(adw::Toast::new(&tr(
                "GPU preset saved; press Save to store it in this mining preset.",
                "Пресет GPU сохранён; нажмите Save, чтобы закрепить его за этим пресетом.",
            )));
        });
    }
    dialog.present(Some(&app.window));
}

fn manage_gpu_presets_dialog(app: &Rc<App>) {
    let dialog = adw::Dialog::builder()
        .title(tr("GPU presets", "Пресеты GPU"))
        .content_width(540)
        .content_height(520)
        .build();
    let header = adw::HeaderBar::new();
    header.set_show_end_title_buttons(false);
    let close = gtk::Button::builder()
        .label(tr("Close", "Закрыть"))
        .css_classes(["flat", "pill"])
        .build();
    let footer = dialog_footer();
    footer.append(&dialog_footer_spacer());
    footer.append(&close);
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::builder()
        .title(tr("Saved GPU presets", "Сохранённые пресеты GPU"))
        .description(tr(
            "Per-card power and clock templates. Deleting one keeps the values already stored in mining presets.",
            "Шаблоны питания и частот для карт. Удаление не меняет значения, уже сохранённые в пресетах майнинга.",
        ))
        .build();
    let presets: Vec<_> = app.config.borrow().gpu_presets.values().cloned().collect();
    if presets.is_empty() {
        group.add(
            &adw::ActionRow::builder()
                .title(tr("No saved GPU presets", "Нет сохранённых пресетов GPU"))
                .subtitle(tr(
                    "Open a card's Tune dialog and use Save as GPU preset.",
                    "Откройте настройку карты и выберите «Сохранить как пресет GPU».",
                ))
                .build(),
        );
    }
    for preset in presets {
        let used = app
            .config
            .borrow()
            .presets
            .values()
            .filter(|mining| {
                mining
                    .gpu
                    .per_gpu
                    .values()
                    .any(|tuning| tuning.gpu_preset == preset.id)
            })
            .count();
        let row = adw::ActionRow::builder()
            .title(&preset.name)
            .subtitle(format!(
                "{} · {} · {} {}",
                preset.gpu_name,
                tuning_summary(&preset.tuning),
                tr("used by", "используется"),
                used
            ))
            .build();
        let rename = gtk::Button::builder()
            .label(tr("Rename", "Переименовать"))
            .css_classes(["flat", "pill"])
            .valign(gtk::Align::Center)
            .build();
        let delete = gtk::Button::builder()
            .label(tr("Delete…", "Удалить…"))
            .css_classes(["destructive-action", "pill"])
            .valign(gtk::Align::Center)
            .build();
        row.add_suffix(&rename);
        row.add_suffix(&delete);
        group.add(&row);
        {
            let app = app.clone();
            let preset = preset.clone();
            rename.connect_clicked(move |_| rename_gpu_preset_dialog(&app, &preset));
        }
        {
            let app = app.clone();
            let dialog = dialog.clone();
            let preset = preset.clone();
            delete.connect_clicked(move |_| {
                delete_gpu_preset_dialog(&app, &preset, Some(&dialog));
            });
        }
    }
    page.add(&group);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&dialog_body(&page, &footer)));
    dialog.set_child(Some(&toolbar));
    {
        let dialog = dialog.clone();
        close.connect_clicked(move |_| {
            dialog.close();
        });
    }
    dialog.present(Some(&app.window));
}

fn rename_gpu_preset_dialog(app: &Rc<App>, preset: &GpuPreset) {
    let dialog = adw::Dialog::builder()
        .title(tr("Rename GPU preset", "Переименовать пресет GPU"))
        .content_width(420)
        .build();
    let header = adw::HeaderBar::new();
    header.set_show_end_title_buttons(false);
    let name = adw::EntryRow::builder()
        .title(tr("Preset name", "Название пресета"))
        .text(&preset.name)
        .build();
    let group = adw::PreferencesGroup::new();
    group.add(&name);
    let page = adw::PreferencesPage::new();
    page.add(&group);
    let cancel = gtk::Button::builder()
        .label(tr("Cancel", "Отмена"))
        .css_classes(["flat", "pill"])
        .build();
    let save = gtk::Button::builder()
        .label(tr("Rename", "Переименовать"))
        .css_classes(["suggested-action", "pill"])
        .build();
    let footer = dialog_footer();
    footer.append(&cancel);
    footer.append(&dialog_footer_spacer());
    footer.append(&save);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&dialog_body(&page, &footer)));
    dialog.set_child(Some(&toolbar));
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
    }
    {
        let app = app.clone();
        let dialog = dialog.clone();
        let id = preset.id.clone();
        let name = name.clone();
        save.connect_clicked(move |_| {
            let next = name.text().trim().to_string();
            if next.is_empty() {
                return;
            }
            if let Some(preset) = app.config.borrow_mut().gpu_presets.get_mut(&id) {
                preset.name = next;
                preset.updated_at = util::now_unix();
            }
            mark_dirty(&app);
            dialog.close();
            save_and_apply(&app);
        });
    }
    dialog.present(Some(&app.window));
}

fn delete_gpu_preset_dialog(app: &Rc<App>, preset: &GpuPreset, parent: Option<&adw::Dialog>) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Delete GPU preset?", "Удалить пресет GPU?"))
        .body(format!(
            "{} «{}». {}",
            tr("This removes", "Удаляется"),
            preset.name,
            tr(
                "Values already stored in mining presets stay; only the saved template and its links are removed.",
                "Значения в пресетах майнинга останутся; удаляется только сохранённый шаблон и ссылки на него.",
            )
        ))
        .build();
    dialog.add_response("cancel", &tr("Cancel", "Отмена"));
    dialog.add_response("delete", &tr("Delete", "Удалить"));
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_close_response("cancel");
    let app = app.clone();
    let preset = preset.clone();
    let parent = parent.cloned();
    let window = app.window.clone();
    dialog.choose(Some(&window), None::<&gio::Cancellable>, move |response| {
        if response != "delete" {
            return;
        }
        {
            let mut cfg = app.config.borrow_mut();
            cfg.gpu_presets.remove(&preset.id);
            for mining in cfg.presets.values_mut() {
                for tuning in mining.gpu.per_gpu.values_mut() {
                    if tuning.gpu_preset == preset.id {
                        tuning.gpu_preset.clear();
                    }
                }
            }
        }
        mark_dirty(&app);
        if let Some(parent) = parent {
            parent.close();
        }
        save_and_apply(&app);
    });
}

// ------------------------------------------------------- miner installation

/// Fill the download fields from a catalogue version, if the version is known.
fn prefill_miner_source(
    miner_id: &str,
    version: &str,
    url: &adw::EntryRow,
    sha: &adw::EntryRow,
    archive: &adw::EntryRow,
    binary: &adw::EntryRow,
) {
    let Some(item) = catalog::miner(miner_id).and_then(|def| def.version(version)) else {
        return;
    };
    if !item.url.is_empty() {
        url.set_text(&item.url);
    }
    sha.set_text(&item.sha256);
    if !item.archive.is_empty() {
        archive.set_text(&item.archive);
    }
    if !item.binary.is_empty() {
        binary.set_text(&item.binary);
    }
}

/// The one place that installs a miner. The miner library and the preset
/// editor open this dialog, so version handling, download, import and the
/// installed state live here exactly once.
fn miner_install_dialog(
    app: &Rc<App>,
    miner_id: &str,
    version: String,
    on_done: Option<Rc<dyn Fn()>>,
) {
    let Some(def) = catalog::miner(miner_id) else {
        return;
    };
    let installed_versions = catalog::installed_versions(&app.config.borrow(), miner_id);
    let mut choices: Vec<(String, Option<catalog::MinerVersion>)> = def
        .versions
        .iter()
        .map(|item| {
            let label = if installed_versions.contains(&item.version) {
                format!("{} ✓", item.version)
            } else {
                item.version.clone()
            };
            (label, Some(item.clone()))
        })
        .collect();
    for custom in &installed_versions {
        if def.version(custom).is_none() {
            choices.push((format!("{custom} ✓"), None));
        }
    }

    let dialog = adw::Dialog::builder()
        .title(tr("Install miner", "Установка майнера"))
        .content_width(580)
        .build();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(
        &def.name,
        &tr("Install miner", "Установка майнера"),
    )));
    // Cancel dismisses the dialog and the header needs no second close
    // button; all actions live in the footer.
    let cancel = gtk::Button::builder()
        .label(tr("Cancel", "Отмена"))
        .css_classes(["flat", "pill"])
        .build();
    header.set_show_end_title_buttons(false);

    let page = adw::PreferencesPage::new();

    // Two groups: what to install, then how. The download fields sit in an
    // expander because a catalogue version fills them anyway, and the dialog
    // has to fit a 760 px window without scrolling away its own footer.
    let version_group = adw::PreferencesGroup::builder()
        .title(tr("Version", "Версия"))
        .description(tr(
            "Pick a catalogue build, or type the name of a build you downloaded upstream.",
            "Выберите сборку из каталога или введите название скачанной вручную.",
        ))
        .build();
    let version_combo = adw::ComboRow::builder()
        .title(tr("Catalogue version", "Версия из каталога"))
        .subtitle(tr(
            "Fills the URL and SHA-256",
            "Подставляет ссылку и SHA-256",
        ))
        .model(&string_list(
            &choices
                .iter()
                .map(|(label, _)| label.clone())
                .collect::<Vec<_>>(),
        ))
        .build();
    if let Some(index) = choices
        .iter()
        .position(|(_, item)| item.as_ref().map(|v| v.version == version).unwrap_or(false))
    {
        version_combo.set_selected(index as u32);
    }
    let version_entry = adw::EntryRow::builder()
        .title(tr("Version name", "Название версии"))
        .text(&version)
        .build();
    version_group.add(&version_combo);
    version_group.add(&version_entry);
    page.add(&version_group);

    let install_group = adw::PreferencesGroup::builder()
        .title(tr("Installation", "Установка"))
        .description(tr(
            "One install serves every preset. HTTPS only; a SHA-256 is checked when the catalogue lists one.",
            "Одна установка служит всем пресетам. Только HTTPS; SHA-256 сверяется, если он указан в каталоге.",
        ))
        .build();
    let state_row = adw::ActionRow::builder()
        .title(tr("State", "Состояние"))
        .build();
    let state_label = label("", &["caption"]);
    state_row.add_suffix(&state_label);
    install_group.add(&state_row);
    // Importing a local binary is the second source for the same install. As a
    // row it stays readable at any window width; as a footer button it pushed
    // the primary action past the dialog edge on narrow windows.
    let import = adw::ActionRow::builder()
        .title(tr("Import binary", "Импортировать бинарник"))
        .subtitle(tr(
            "Install a binary from disk",
            "Установить бинарник с диска",
        ))
        .activatable(true)
        .build();
    let import_icon = gtk::Image::from_icon_name("document-open-symbolic");
    import_icon.add_css_class("dim-label");
    import.add_suffix(&import_icon);
    install_group.add(&import);
    // The upstream link belongs to the version it documents, not to the window
    // chrome: an activatable row beats a stray icon in the header.
    if !def.source_url.is_empty() {
        let release = adw::ActionRow::builder()
            .title(tr("Release page", "Страница релиза"))
            .subtitle(def.source_url.clone())
            .activatable(true)
            .build();
        release.set_subtitle_lines(1);
        let icon = gtk::Image::from_icon_name("web-browser-symbolic");
        icon.add_css_class("dim-label");
        release.add_suffix(&icon);
        let source_url = def.source_url.clone();
        release.connect_activated(move |_| {
            let _ =
                gio::AppInfo::launch_default_for_uri(&source_url, None::<&gio::AppLaunchContext>);
        });
        install_group.add(&release);
    }
    let url_entry = adw::EntryRow::builder()
        .title("URL")
        .text(
            def.version(&version)
                .map(|item| item.url.clone())
                .unwrap_or_default(),
        )
        .build();
    let sha_entry = adw::EntryRow::builder()
        .title("SHA-256")
        .text(
            def.version(&version)
                .map(|item| item.sha256.clone())
                .unwrap_or_default(),
        )
        .build();
    // The expander subtitle carries the URL, so tucking the fields away hides
    // nothing the user needs before pressing the primary action.
    let details = adw::ExpanderRow::builder()
        .title(tr("Download source", "Источник загрузки"))
        .subtitle(tr("URL, checksum and archive", "URL, сумма и архив"))
        .build();
    details.set_subtitle_lines(1);
    let archive_entry = adw::EntryRow::builder()
        .title(tr("Archive", "Архив"))
        .text(
            def.version(&version)
                .map(|item| item.archive.clone())
                .unwrap_or_else(|| "binary".into()),
        )
        .build();
    let binary_entry = adw::EntryRow::builder()
        .title(tr("Binary", "Бинарник"))
        .text(
            def.version(&version)
                .map(|item| item.binary.clone())
                .unwrap_or_else(|| "wildrig-multi".into()),
        )
        .build();
    details.add_row(&url_entry);
    details.add_row(&sha_entry);
    details.add_row(&archive_entry);
    details.add_row(&binary_entry);
    install_group.add(&details);
    page.add(&install_group);
    {
        let details = details.clone();
        let url_row = url_entry.clone();
        let refresh = move || {
            let url = url_row.text();
            let url = url.trim();
            if url.is_empty() {
                details.set_subtitle(&tr("URL, checksum and archive", "URL, сумма и архив"));
            } else {
                details.set_subtitle(url);
            }
        };
        refresh();
        let refresh = Rc::new(refresh);
        url_entry.connect_notify_local(Some("text"), move |_, _| refresh());
    }

    let footer = dialog_footer();
    let status = label("", &["caption", "dim-label"]);
    status.set_hexpand(true);
    status.set_valign(gtk::Align::Center);
    // Long download/import messages must not widen the footer: ellipsize them
    // and keep the full text in the tooltip (set_status_text does that).
    status.set_ellipsize(gtk::pango::EllipsizeMode::End);
    // A short label keeps the footer on one line down to the smallest window;
    // the dialog title already says the action installs a miner.
    let download = gtk::Button::builder()
        .label(tr("Install", "Установить"))
        .css_classes(["suggested-action", "pill"])
        .build();
    footer.append(&cancel);
    footer.append(&status);
    footer.append(&download);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.set_vexpand(true);
    content.append(&page);
    content.append(&footer);
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));
    dialog.set_child(Some(&toolbar));

    let update_state: Rc<dyn Fn()> = {
        let app = app.clone();
        let miner_id = miner_id.to_string();
        let state_row = state_row.clone();
        let state_label = state_label.clone();
        let version_entry = version_entry.clone();
        Rc::new(move || {
            let version = version_entry.text().trim().to_string();
            let install = app.config.borrow().install(&miner_id, &version).cloned();
            match install {
                Some(install) => {
                    state_row.set_subtitle(&install.path);
                    state_label.set_text(&tr("installed", "установлен"));
                    state_label.set_css_classes(&["caption", "success"]);
                }
                None => {
                    // The group description already explains that one install
                    // serves every preset; repeating it here just made the row
                    // taller than its neighbours.
                    state_row.set_subtitle("");
                    state_label.set_text(&tr("not installed", "не установлен"));
                    state_label.set_css_classes(&["caption", "warning"]);
                }
            }
        })
    };
    update_state();

    let finish_install: Rc<dyn Fn(MinerInstall)> = {
        let app = app.clone();
        let dialog = dialog.clone();
        let on_done = on_done.clone();
        Rc::new(move |install: MinerInstall| {
            let miner_id = install.miner.clone();
            let version = install.version.clone();
            {
                let mut cfg = app.config.borrow_mut();
                cfg.add_install(install.clone());
            }
            // The daemon owns the applied config. Persist the install there
            // and tell a running daemon, otherwise Start would still see the
            // old in-memory config and claim the miner is not installed.
            let applied = {
                let mut applied = app.applied.borrow_mut();
                applied.add_install(install);
                let _ = applied.save();
                applied.clone()
            };
            if let Ok(json) = serde_json::to_string(&applied) {
                let _ = app.tx.send(Cmd::Apply {
                    json,
                    mode: "live".into(),
                    quiet: true,
                });
            }
            refresh_miners(&app);
            if let Some(callback) = &on_done {
                callback();
            }
            app.toast.add_toast(adw::Toast::new(&tr(
                "Miner installed. It is available in every preset.",
                "Майнер установлен. Он доступен во всех пресетах.",
            )));
            dialog.close();
            // The install itself never changes a pin. Offer the explicit
            // choice only after the dialog is gone, so one modal is open.
            miner_update_choice_dialog(&app, &miner_id, &version);
        })
    };

    let miner_key = miner_id.to_string();
    {
        let choices = choices.clone();
        let version_entry = version_entry.clone();
        let url_entry = url_entry.clone();
        let sha_entry = sha_entry.clone();
        let archive_entry = archive_entry.clone();
        let binary_entry = binary_entry.clone();
        let update_state = update_state.clone();
        version_combo.connect_notify_local(Some("selected"), move |row, _| {
            let index = row.selected() as usize;
            let Some((_, Some(item))) = choices.get(index) else {
                return;
            };
            version_entry.set_text(&item.version);
            prefill_miner_source(
                &miner_key,
                &item.version,
                &url_entry,
                &sha_entry,
                &archive_entry,
                &binary_entry,
            );
            update_state();
        });
    }
    {
        let update_state = update_state.clone();
        version_entry.connect_notify_local(Some("text"), move |_, _| update_state());
    }
    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| {
            dialog.close();
        });
    }
    {
        let parent = app.window.clone();
        let miner_id = miner_id.to_string();
        let version_entry = version_entry.clone();
        let finish_install = finish_install.clone();
        let status = status.clone();
        import.connect_activated(move |row| {
            let dialog = gtk::FileChooserNative::builder()
                .title(tr("Select miner binary", "Выберите бинарник майнера"))
                .action(gtk::FileChooserAction::Open)
                .build();
            dialog.set_transient_for(Some(&parent));
            let miner_id = miner_id.clone();
            let version_entry = version_entry.clone();
            let finish_install = finish_install.clone();
            let status = status.clone();
            let row = row.clone();
            dialog.connect_response(move |dialog, response| {
                if response == gtk::ResponseType::Accept {
                    if let Some(path) = dialog.file().and_then(|file| file.path()) {
                        let mut version = version_entry.text().trim().to_string();
                        if version.is_empty() {
                            version = tr("custom", "своя").to_string();
                        }
                        row.set_sensitive(false);
                        row.set_subtitle(&tr("Importing…", "Импортирую…"));
                        set_status_text(
                            &status,
                            &tr(
                                "Copying the binary into the miner store…",
                                "Копирую бинарник в хранилище майнеров…",
                            ),
                            "accent",
                        );
                        let (tx, rx) = std::sync::mpsc::channel::<Result<MinerInstall, String>>();
                        let miner_id_thread = miner_id.clone();
                        let version_thread = version.clone();
                        std::thread::spawn(move || {
                            let root = trommel_core::download::miner_root();
                            let result = trommel_core::download::import_binary(
                                &miner_id_thread,
                                &version_thread,
                                &path,
                                &root,
                            )
                            .map_err(|err| err.to_string());
                            let _ = tx.send(result);
                        });
                        let row = row.clone();
                        let status = status.clone();
                        let finish_install = finish_install.clone();
                        glib::timeout_add_local(Duration::from_millis(100), move || {
                            match rx.try_recv() {
                                Ok(Ok(install)) => {
                                    finish_install(install);
                                    glib::ControlFlow::Break
                                }
                                Ok(Err(error)) => {
                                    set_status_text(&status, &error, "error");
                                    row.set_sensitive(true);
                                    row.set_subtitle(&tr(
                                        "Install a binary you already have on disk",
                                        "Установить бинарник, который уже есть на диске",
                                    ));
                                    glib::ControlFlow::Break
                                }
                                Err(std::sync::mpsc::TryRecvError::Empty) => {
                                    glib::ControlFlow::Continue
                                }
                                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                                    set_status_text(
                                        &status,
                                        &tr(
                                            "Import thread stopped unexpectedly.",
                                            "Поток импорта неожиданно завершился.",
                                        ),
                                        "error",
                                    );
                                    row.set_sensitive(true);
                                    row.set_subtitle(&tr(
                                        "Install a binary you already have on disk",
                                        "Установить бинарник, который уже есть на диске",
                                    ));
                                    glib::ControlFlow::Break
                                }
                            }
                        });
                    }
                }
                dialog.destroy();
            });
            dialog.show();
        });
    }
    {
        let miner_id = miner_id.to_string();
        let source_url = def.source_url.clone();
        let version_entry = version_entry.clone();
        let url_entry = url_entry.clone();
        let sha_entry = sha_entry.clone();
        let archive_entry = archive_entry.clone();
        let binary_entry = binary_entry.clone();
        let status = status.clone();
        let finish_install = finish_install.clone();
        download.connect_clicked(move |button| {
            let version = version_entry.text().trim().to_string();
            let url = url_entry.text().trim().to_string();
            if version.is_empty() {
                set_status_text(
                    &status,
                    &tr(
                        "Enter a version name first.",
                        "Сначала введите название версии.",
                    ),
                    "error",
                );
                return;
            }
            if !url.starts_with("https://") {
                set_status_text(
                    &status,
                    &tr(
                        "Only HTTPS URLs are accepted.",
                        "Принимаются только HTTPS-ссылки.",
                    ),
                    "error",
                );
                return;
            }
            button.set_sensitive(false);
            button.set_label(&tr("Downloading…", "Скачиваю…"));
            set_status_text(&status, &tr("Downloading…", "Скачиваю…"), "accent");
            let request = trommel_core::download::DownloadRequest {
                miner: miner_id.clone(),
                version,
                url,
                sha256: sha_entry.text().to_string(),
                archive: archive_entry.text().to_string(),
                binary: binary_entry.text().to_string(),
                source_url: source_url.clone(),
            };
            let root = trommel_core::download::miner_root();
            let (tx, rx) = std::sync::mpsc::channel::<Result<MinerInstall, String>>();
            std::thread::spawn(move || {
                let result =
                    trommel_core::download::install(&request, &root).map_err(|err| err.to_string());
                let _ = tx.send(result);
            });
            let button = button.clone();
            let status = status.clone();
            let finish_install = finish_install.clone();
            glib::timeout_add_local(Duration::from_millis(200), move || match rx.try_recv() {
                Ok(Ok(install)) => {
                    finish_install(install);
                    glib::ControlFlow::Break
                }
                Ok(Err(error)) => {
                    set_status_text(&status, &error, "error");
                    button.set_sensitive(true);
                    button.set_label(&tr("Install", "Установить"));
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    set_status_text(
                        &status,
                        &tr(
                            "Download thread stopped unexpectedly.",
                            "Поток загрузки неожиданно завершился.",
                        ),
                        "error",
                    );
                    glib::ControlFlow::Break
                }
            });
        });
    }
    dialog.present(Some(&app.window));
    if let Some(path) = std::env::var_os("TROMMEL_INSTALL_SCREENSHOT") {
        let path = path.to_string_lossy().into_owned();
        let capture = app.window.clone();
        let application = app.window.application();
        glib::timeout_add_seconds_local(2, move || {
            if let Some(application) = application.clone() {
                schedule_screenshot(capture.clone(), path.clone(), application, 0);
            }
            glib::ControlFlow::Break
        });
    }
}

/// Set text plus one semantic colour class on a status label.
fn set_status_text(widget: &gtk::Label, text: &str, class: &str) {
    widget.set_text(text);
    // Footer status lines ellipsize on narrow dialogs; the tooltip still carries
    // the whole message.
    widget.set_tooltip_text((!text.is_empty()).then_some(text));
    if class.is_empty() {
        widget.set_css_classes(&["caption", "dim-label"]);
    } else {
        widget.set_css_classes(&["caption", class]);
    }
}

fn load_settings(app: &Rc<App>) {
    let cfg = app.config.borrow();
    let behavior = &cfg.behavior;
    app.settings.auto_restart.set_active(behavior.auto_restart);
    app.settings
        .restore_stock
        .set_active(behavior.restore_stock_on_stop);
    app.settings
        .notifications
        .set_active(behavior.notifications);
    app.settings
        .tray_monochrome
        .set_active(behavior.tray_monochrome);
    app.settings
        .start_login
        .set_active(behavior.start_mining_on_login);
    app.settings.simple_mode.set_active(behavior.simple_mode);
    app.settings.poll.set_value(behavior.poll_interval_s as f64);
    app.settings.language.set_selected(match behavior.language {
        i18n::Language::System => 0,
        i18n::Language::En => 1,
        i18n::Language::Ru => 2,
    });
    app.settings
        .appearance
        .set_selected(match behavior.appearance {
            Appearance::System => 0,
            Appearance::Light => 1,
            Appearance::Dark => 2,
        });
    app.settings
        .temp_limit
        .set_value(cfg.behavior.temp_limit_c as f64);
    app.settings
        .temp_resume
        .set_value(cfg.behavior.temp_resume_c as f64);
}

fn apply_appearance(appearance: Appearance) {
    adw::StyleManager::default().set_color_scheme(match appearance {
        Appearance::System => adw::ColorScheme::Default,
        Appearance::Light => adw::ColorScheme::ForceLight,
        Appearance::Dark => adw::ColorScheme::ForceDark,
    });
}

fn save_settings(app: &Rc<App>) {
    let mut cfg = app.config.borrow_mut();
    cfg.behavior.auto_restart = app.settings.auto_restart.is_active();
    cfg.behavior.restore_stock_on_stop = app.settings.restore_stock.is_active();
    cfg.behavior.notifications = app.settings.notifications.is_active();
    cfg.behavior.tray_monochrome = app.settings.tray_monochrome.is_active();
    cfg.behavior.start_mining_on_login = app.settings.start_login.is_active();
    cfg.behavior.simple_mode = app.settings.simple_mode.is_active();
    cfg.behavior.poll_interval_s = app.settings.poll.value() as u64;
    cfg.behavior.language = match app.settings.language.selected() {
        1 => i18n::Language::En,
        2 => i18n::Language::Ru,
        _ => i18n::Language::System,
    };
    cfg.behavior.appearance = match app.settings.appearance.selected() {
        1 => Appearance::Light,
        2 => Appearance::Dark,
        _ => Appearance::System,
    };
    cfg.behavior.temp_limit_c = app.settings.temp_limit.value() as u32;
    cfg.behavior.temp_resume_c = app.settings.temp_resume.value() as u32;
    set_mining_autostart_unit(app.settings.start_login.is_active());
    mark_dirty(app);
}

fn load_daemon_autostart(app: &Rc<App>) {
    DAEMON_AUTOSTART_SYNC.set(true);
    app.settings
        .start_trommel
        .set_active(daemon_autostart_enabled());
    DAEMON_AUTOSTART_SYNC.set(false);
}

fn daemon_autostart_enabled() -> bool {
    // Deterministic screenshots/tests: the host unit state must not leak into
    // a private instance's Settings page.
    match std::env::var("TROMMEL_AUTOSTART_STATE").as_deref() {
        Ok("on") => return true,
        Ok("off") => return false,
        _ => {}
    }
    std::process::Command::new("systemctl")
        .args(["--user", "is-enabled", "--quiet", "trommeld.service"])
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn set_daemon_autostart(enable: bool) -> anyhow::Result<()> {
    let action = if enable { "enable" } else { "disable" };
    let output = std::process::Command::new("systemctl")
        .args(["--user", action, "trommeld.service"])
        .output()
        .map_err(|err| anyhow::anyhow!("cannot run systemctl: {err}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "systemctl --user {action} trommeld.service failed: {}",
            stderr.trim()
        );
    }
    Ok(())
}

fn set_mining_autostart_unit(enable: bool) {
    let action = if enable { "enable" } else { "disable" };
    let _ = std::process::Command::new("systemctl")
        .args(["--user", action, "trommel-autostart.service"])
        .output();
}

fn nonzero(value: u32) -> Option<u32> {
    if value == 0 {
        None
    } else {
        Some(value)
    }
}

/// A concrete reason Start cannot proceed, with a button that opens the
/// preset editor instead of an old quick-setup wizard.
struct SetupProblem {
    heading: String,
    body: String,
}

fn preset_setup_problem(app: &Rc<App>) -> Option<SetupProblem> {
    let cfg = app.config.borrow();
    let preset = cfg.preset()?;
    if preset.wallet.trim().is_empty() {
        return Some(SetupProblem {
            heading: tr("Add a wallet", "Добавьте кошелёк"),
            body: tr(
                "This preset has no wallet address yet. Open the preset, fill in the wallet and save it.",
                "В пресете ещё нет адреса кошелька. Откройте пресет, укажите кошелёк и сохраните.",
            ),
        });
    }
    if !preset.pool_url.contains(':') {
        return Some(SetupProblem {
            heading: tr("Add a pool address", "Добавьте адрес пула"),
            body: tr(
                "The preset has no valid pool address (expected host:port). Open the preset and enter the endpoint your pool gave you.",
                "В пресете нет корректного адреса пула (ожидается host:port). Откройте пресет и укажите endpoint, который выдал пул.",
            ),
        });
    }
    if !cfg
        .miner_path(preset)
        .map(|path| path.exists())
        .unwrap_or(false)
    {
        return Some(SetupProblem {
            heading: tr("Install the miner version", "Установите версию майнера"),
            body: format!(
                "{} {} {}",
                tr(
                    "The preset pins a miner version that is not installed:",
                    "Пресет закрепляет неустановленную версию майнера:"
                ),
                preset.miner,
                preset.miner_version
            ),
        });
    }
    None
}

fn create_preset_alert(app: &Rc<App>) {
    let dialog = adw::AlertDialog::builder()
        .heading(tr("Create a mining preset first", "Сначала создайте пресет"))
        .body(tr(
            "A preset stores the coin, pool, wallet, miner and GPU settings. Create one to start mining.",
            "Пресет хранит монету, пул, кошелёк, майнер и настройки GPU. Создайте его, чтобы начать майнинг.",
        ))
        .build();
    dialog.add_response("cancel", &tr("Cancel", "Отмена"));
    dialog.add_response("create", &tr("Create preset", "Создать пресет"));
    dialog.set_response_appearance("create", adw::ResponseAppearance::Suggested);
    dialog.set_close_response("cancel");
    let app = app.clone();
    let window = app.window.clone();
    dialog.choose(Some(&window), None::<&gio::Cancellable>, move |response| {
        if response == "create" {
            app.stack.set_visible_child_name("presets");
            new_preset(&app);
        }
    });
}

fn setup_problem_alert(app: &Rc<App>, problem: &SetupProblem) {
    let dialog = adw::AlertDialog::builder()
        .heading(&problem.heading)
        .body(&problem.body)
        .build();
    dialog.add_response("cancel", &tr("Cancel", "Отмена"));
    dialog.add_response("open", &tr("Open preset", "Открыть пресет"));
    dialog.set_response_appearance("open", adw::ResponseAppearance::Suggested);
    dialog.set_close_response("cancel");
    let app = app.clone();
    let window = app.window.clone();
    dialog.choose(Some(&window), None::<&gio::Cancellable>, move |response| {
        if response == "open" {
            app.stack.set_visible_child_name("presets");
            if let Some(id) = app.config.borrow().active_preset.clone() {
                preset_editor_dialog(&app, &id);
            }
        }
    });
}

fn save_and_apply(app: &Rc<App>) {
    save_and_apply_with(app, None);
}

/// Apply the draft. While mining, a restart/stop confirmation is shown; when
/// it is cancelled, `on_cancel` restores whatever the caller changed
/// optimistically (the GPU participation switch is the main case).
fn save_and_apply_with(app: &Rc<App>, on_cancel: Option<Rc<dyn Fn()>>) {
    // The editor/settings callbacks commit first; this only decides the mode.
    let draft = app.config.borrow().clone();
    let applied = app.applied.borrow().clone();
    let running = app
        .status
        .borrow()
        .as_ref()
        .map(|status| status.state.running)
        .unwrap_or(false);
    let diff = applied.diff(&draft);
    if running && diff.class == ChangeClass::RestartRequired {
        let switching_preset = applied.active_preset != draft.active_preset;
        let body = apply_alert_body(&diff, switching_preset);
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Apply changes?", "Применить изменения?"))
            .body(&body)
            .build();
        dialog.add_response("cancel", &tr("Cancel", "Отмена"));
        dialog.add_response("stop", &tr("Apply and stop", "Применить и остановить"));
        dialog.add_response(
            "restart",
            &tr("Apply and restart", "Применить и перезапустить"),
        );
        dialog.set_response_appearance("stop", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("restart"));
        dialog.set_close_response("cancel");
        let app = app.clone();
        let window = app.window.clone();
        let draft = draft.clone();
        dialog.choose(Some(&window), None::<&gio::Cancellable>, move |response| {
            if response == "restart" || response == "stop" {
                let mode = if response == "restart" {
                    "restart"
                } else {
                    "stop"
                };
                let json = serde_json::to_string(&draft).unwrap_or_default();
                let _ = app.tx.send(Cmd::Apply {
                    json,
                    mode: mode.into(),
                    quiet: false,
                });
            } else if let Some(on_cancel) = on_cancel {
                on_cancel();
            }
        });
    } else {
        let json = serde_json::to_string(&draft).unwrap_or_default();
        let _ = app.tx.send(Cmd::Apply {
            json,
            mode: "live".into(),
            quiet: false,
        });
    }
}

fn handle_event(app: &Rc<App>, event: Ev) {
    match event {
        Ev::Status(json) => {
            if let Ok(status) = Status::from_json(&json) {
                apply_status(app, status);
            }
        }
        Ev::Config(json) => {
            if let Ok(cfg) = serde_json::from_str::<Config>(&json) {
                let cfg = cfg.normalize();
                *app.applied.borrow_mut() = cfg.clone();
                // The daemon owns the applied config, not the draft. A
                // Refresh while the user is editing must not silently throw
                // those edits away.
                if !app.dirty.get() {
                    *app.config.borrow_mut() = cfg;
                    load_settings(app);
                    refresh_preset_list(app);
                    refresh_miners(app);
                }
            }
        }
        Ev::ServiceDown(message) => {
            app.hero_line
                .set_text(&tr("Service is not running", "Сервис не запущен"));
            app.hero_line.set_tooltip_text(Some(&message));
            app.action_button.set_sensitive(false);
            app.pill.set_text(&tr("offline", "нет связи"));
            app.pill
                .set_css_classes(&["caption", "trommel-pill", "trommel-pill-offline"]);
            // Stop the local stopwatch: without a live daemon it would keep
            // inventing uptime from the last sample.
            *app.hero_meta_state.borrow_mut() = None;
        }
        Ev::Error(message) => {
            show_error_toast(app, &message);
        }
        Ev::Applied { message, quiet } => {
            if !quiet {
                app.dirty.set(false);
                update_dirty(app);
            }
            let text = if message.trim().is_empty() {
                tr("Applied", "Применено").to_string()
            } else {
                message
            };
            app.toast.add_toast(adw::Toast::new(&text));
            let _ = app.tx.send(Cmd::Refresh);
        }
        Ev::Busy(busy) => {
            app.action_button
                .set_sensitive(!busy && start_block_reason(app).is_none());
            if busy {
                set_action_text(
                    &app.action_button,
                    &app.action_label,
                    &tr("Waiting for authorization…", "Жду подтверждения…"),
                );
            } else {
                let running = app
                    .status
                    .borrow()
                    .as_ref()
                    .map(|status| status.state.running)
                    .unwrap_or(false);
                set_action_text(
                    &app.action_button,
                    &app.action_label,
                    &tr(
                        if running { "Stop" } else { "Start" },
                        if running {
                            "Остановить"
                        } else {
                            "Запустить"
                        },
                    ),
                );
            }
        }
        Ev::Bench(json) => {
            if let Ok(report) = trommel_core::bench::BenchReport::from_json(&json) {
                if let Some(best) = report.best() {
                    app.toast.add_toast(adw::Toast::new(&format!(
                        "{}: {}",
                        tr("Best miner", "Лучший майнер"),
                        best.name
                    )));
                } else {
                    app.toast.add_toast(adw::Toast::new(&tr(
                        "Benchmark finished without a winner",
                        "Сравнение завершено без победителя",
                    )));
                }
            }
        }
        Ev::Quit => {
            if let Some(application) = app.window.application() {
                application.quit();
            }
        }
    }
}

/// Metrics that get a sparkline in the GPU cards. Core, memory, fan and load
/// are values, not trends: four extra graphs made the card noisy without
/// explaining anything. The order is the sample layout in [`History`];
/// `index()` must stay in sync with it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Metric {
    Hashrate,
    Power,
    Temp,
}

impl Metric {
    fn index(self) -> usize {
        match self {
            Self::Hashrate => 0,
            Self::Power => 1,
            Self::Temp => 2,
        }
    }

    fn unit(self) -> &'static str {
        match self {
            Self::Hashrate => "H/s",
            Self::Power => "W",
            Self::Temp => "°C",
        }
    }

    /// CSS colour name used for the line: hot tiles keep their red tint.
    fn color_key(self, value_class: &str) -> &'static str {
        match value_class {
            "trommel-hot" => "error_color",
            "trommel-warm" => "warning_color",
            _ => "accent_bg_color",
        }
    }
}

const METRIC_COUNT: usize = 3;
/// The only metrics that draw a sparkline on a card, in display order.
#[cfg(test)]
const SPARK_METRICS: [Metric; METRIC_COUNT] = [Metric::Hashrate, Metric::Power, Metric::Temp];
/// How much history a sparkline shows and how many samples it keeps at most
/// (a 1 s GUI poll over 5 minutes is 300 samples).
const HISTORY_SECS: u64 = 300;
const HISTORY_MAX: usize = 360;
const SPARK_HEIGHT: i32 = 24;

type GpuSample = (u64, [Option<f64>; METRIC_COUNT]);

/// Miner-side record for one `nvidia-smi` card. A record without a mapped
/// index/uuid is ignored: the card shows `—`, never another card's number.
fn gpu_miner_record<'a>(
    status: &'a Status,
    gpu: &GpuStats,
) -> Option<&'a trommel_core::minerapi::GpuMinerStats> {
    status.miner.gpus.iter().find(|record| {
        record.uuid.as_deref() == Some(gpu.uuid.as_str())
            || (record.uuid.is_none() && record.index == Some(gpu.index))
    })
}

fn gpu_miner_hashrate(status: &Status, gpu: &GpuStats) -> Option<f64> {
    gpu_miner_record(status, gpu).and_then(|record| {
        record
            .hashrate_10s
            .or(record.hashrate_60s)
            .or(record.hashrate_15m)
    })
}

#[derive(Default)]
struct History {
    gpus: BTreeMap<String, VecDeque<GpuSample>>,
}

impl History {
    /// Store one status snapshot per GPU. The daemon reports the same `ts`
    /// while its own poll has not advanced, so repeated timestamps replace the
    /// last sample instead of widening the graph with duplicates.
    fn record(&mut self, status: &Status) {
        let ts = status.ts;
        for gpu in &status.gpus {
            let samples = self.gpus.entry(gpu.uuid.clone()).or_default();
            let values = [
                gpu_miner_hashrate(status, gpu),
                gpu.power_w,
                gpu.temp_c.map(f64::from),
            ];
            match samples.back_mut() {
                Some((last_ts, last)) if *last_ts == ts => *last = values,
                _ => samples.push_back((ts, values)),
            }
            while let Some((old, _)) = samples.front() {
                if ts.saturating_sub(*old) <= HISTORY_SECS {
                    break;
                }
                samples.pop_front();
            }
            while samples.len() > HISTORY_MAX {
                samples.pop_front();
            }
        }
    }

    fn series(&self, uuid: &str, metric: Metric) -> Vec<(u64, f64)> {
        let Some(samples) = self.gpus.get(uuid) else {
            return Vec::new();
        };
        samples
            .iter()
            .filter_map(|(ts, values)| values[metric.index()].map(|value| (*ts, value)))
            .collect()
    }
}

/// A live sparkline; `apply_status` redraws every registered area once per
/// poll, so the graph keeps moving without rebuilding the whole card.
struct SparkEntry {
    area: glib::WeakRef<gtk::DrawingArea>,
    uuid: String,
    metric: Metric,
    label: String,
}

fn spark_color(area: &gtk::DrawingArea, key: &str) -> gdk::RGBA {
    let context = area.style_context();
    if let Some(color) = context.lookup_color(key) {
        return color;
    }
    match key {
        "error_color" => context
            .lookup_color("error_bg_color")
            .unwrap_or(gdk::RGBA::new(0.88, 0.11, 0.14, 1.0)),
        "warning_color" => context
            .lookup_color("warning_bg_color")
            .unwrap_or(gdk::RGBA::new(0.90, 0.42, 0.0, 1.0)),
        _ => gdk::RGBA::new(0.21, 0.52, 0.89, 1.0),
    }
}

/// Draw a smooth area sparkline: a faint wash under a thin accent line.
///
/// The x axis is wall-clock time, so a stalled daemon leaves a visible gap
/// instead of stretching the old samples over the whole width.
fn draw_sparkline(
    area: &gtk::DrawingArea,
    ctx: &gtk::cairo::Context,
    width: i32,
    height: i32,
    series: &[(u64, f64)],
    color_key: &str,
) {
    if series.len() < 2 || width <= 4 || height <= 4 {
        return;
    }
    let (first_ts, _) = series[0];
    let (last_ts, _) = series[series.len() - 1];
    if last_ts <= first_ts {
        return;
    }
    let (mut lo, mut hi) = series.iter().fold(
        (f64::INFINITY, f64::NEG_INFINITY),
        |(lo, hi), (_, value)| (lo.min(*value), hi.max(*value)),
    );
    // A flat series would divide by zero; give it a unit window so the line
    // sits in the middle of the tile instead of jumping between edges.
    if hi - lo < 1.0 {
        let mid = (hi + lo) / 2.0;
        lo = mid - 1.0;
        hi = mid + 1.0;
    }
    let w = width as f64;
    let h = height as f64;
    // The wash reaches the tile's rounded floor; the plot keeps only a 2 px
    // margin, because a line that floats far above the bottom edge reads as
    // detached from the tile. At 2 px the corner rounding trims at most ~1 px
    // off the last sample.
    let radius = 10.0_f64.min(h / 2.0).min(w / 2.0);
    let top = 3.0;
    let bottom = 2.0;
    let usable = (h - bottom - top).max(1.0);
    let x_of = |ts: u64| (ts.saturating_sub(first_ts) as f64) * w / (last_ts - first_ts) as f64;
    let y_of = |value: f64| top + (1.0 - (value - lo) / (hi - lo)) * usable;
    let color = spark_color(area, color_key);
    let (red, green, blue) = (
        f64::from(color.red()),
        f64::from(color.green()),
        f64::from(color.blue()),
    );

    let points: Vec<(f64, f64)> = series
        .iter()
        .map(|(ts, value)| (x_of(*ts), y_of(*value)))
        .collect();
    let (start_x, start_y) = points[0];
    let (end_x, _) = points[points.len() - 1];

    let _ = ctx.save();
    // The chart is the floor of the tile: square at the top, rounded at the
    // bottom so the wash follows the card's 10 px corner radius.
    ctx.move_to(0.0, 0.0);
    ctx.line_to(w, 0.0);
    ctx.line_to(w, h - radius);
    ctx.arc(
        w - radius,
        h - radius,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    ctx.line_to(radius, h);
    ctx.arc(
        radius,
        h - radius,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    ctx.close_path();
    ctx.clip();

    // Soft wash under the line: strongest at the baseline, gone at the top.
    let gradient = gtk::cairo::LinearGradient::new(0.0, top, 0.0, h);
    gradient.add_color_stop_rgba(0.0, red, green, blue, 0.0);
    gradient.add_color_stop_rgba(1.0, red, green, blue, 0.12);
    // Overdraw by one pixel into the clip so antialiasing cannot leave a
    // hairline of tile background at the bottom or right edge.
    ctx.move_to(start_x, h + 1.0);
    ctx.line_to(start_x, start_y);
    spark_curve(ctx, &points);
    ctx.line_to(end_x + 1.0, h + 1.0);
    ctx.close_path();
    let _ = ctx.set_source(&gradient);
    let _ = ctx.fill();

    ctx.set_line_join(gtk::cairo::LineJoin::Round);
    ctx.set_line_cap(gtk::cairo::LineCap::Round);
    // A wider, almost transparent pass gives the line a faint glow instead of
    // a hard 1 px stroke that disappears on light backgrounds.
    for (width, alpha) in [(4.0, 0.10), (1.4, 0.85)] {
        ctx.move_to(start_x, start_y);
        spark_curve(ctx, &points);
        ctx.set_line_width(width);
        ctx.set_source_rgba(red, green, blue, alpha);
        let _ = ctx.stroke();
    }
    let _ = ctx.restore();
}

/// Trace a Catmull-Rom spline through the samples so a handful of points still
/// reads as a curve rather than a zigzag. The caller owns the initial
/// `move_to`.
fn spark_curve(ctx: &gtk::cairo::Context, points: &[(f64, f64)]) {
    let count = points.len();
    if count < 3 {
        for (x, y) in points {
            ctx.line_to(*x, *y);
        }
        return;
    }
    for index in 0..count - 1 {
        let p0 = points[index.saturating_sub(1)];
        let p1 = points[index];
        let p2 = points[index + 1];
        let p3 = points[(index + 2).min(count - 1)];
        ctx.curve_to(
            p1.0 + (p2.0 - p0.0) / 6.0,
            p1.1 + (p2.1 - p0.1) / 6.0,
            p2.0 - (p3.0 - p1.0) / 6.0,
            p2.1 - (p3.1 - p1.1) / 6.0,
            p2.0,
            p2.1,
        );
    }
}

fn sparkline(
    app: &Rc<App>,
    uuid: &str,
    metric: Metric,
    value_class: &str,
    label_text: &str,
) -> gtk::DrawingArea {
    let history = app.history.clone();
    let uuid_owned = uuid.to_string();
    let color_key = metric.color_key(value_class).to_string();
    let area = gtk::DrawingArea::builder()
        .height_request(SPARK_HEIGHT)
        .hexpand(true)
        .valign(gtk::Align::End)
        .build();
    area.add_css_class("trommel-spark");
    // The chart is decorative for pointer users (it has a tooltip) but still
    // needs a name for screen readers and the UI smoke test.
    area.set_accessible_role(gtk::AccessibleRole::Img);
    let accessible = tr(
        &format!("{label_text} chart"),
        &format!("График: {label_text}"),
    );
    area.update_property(&[gtk::accessible::Property::Label(&accessible)]);
    area.set_draw_func(move |area, ctx, width, height| {
        let series = history.borrow().series(&uuid_owned, metric);
        draw_sparkline(area, ctx, width, height, &series, &color_key);
    });
    app.sparklines.borrow_mut().push(SparkEntry {
        area: area.downgrade(),
        uuid: uuid.to_string(),
        metric,
        label: label_text.to_string(),
    });
    area
}

/// Redraw the graphs and refresh their "last N min: low–high" tooltips.
fn update_sparklines(app: &Rc<App>) {
    let history = app.history.borrow();
    let mut entries = app.sparklines.borrow_mut();
    entries.retain(|entry| entry.area.upgrade().is_some());
    for entry in entries.iter() {
        let Some(area) = entry.area.upgrade() else {
            continue;
        };
        let series = history.series(&entry.uuid, entry.metric);
        area.set_tooltip_text(spark_tooltip(&series, &entry.label, entry.metric).as_deref());
        area.queue_draw();
    }
}

fn spark_tooltip(series: &[(u64, f64)], label: &str, metric: Metric) -> Option<String> {
    let first = series.first()?.0;
    let last = series.last()?.0;
    if series.len() < 2 || last <= first {
        return None;
    }
    let lo = series
        .iter()
        .map(|(_, value)| *value)
        .fold(f64::INFINITY, f64::min);
    let hi = series
        .iter()
        .map(|(_, value)| *value)
        .fold(f64::NEG_INFINITY, f64::max);
    let minutes = (last - first).div_ceil(60).max(1);
    let window = tr(&format!("last {minutes} min"), &format!("за {minutes} мин"));
    if metric == Metric::Hashrate {
        return Some(format!(
            "{label} · {window}: {}–{}",
            util::format_hashrate(lo),
            util::format_hashrate(hi)
        ));
    }
    Some(format!(
        "{label} · {window}: {lo:.0}–{hi:.0} {}",
        metric.unit()
    ))
}

fn gpu_stat(
    spark: Option<gtk::DrawingArea>,
    label_text: &str,
    value_text: &str,
    hint: Option<&str>,
    value_class: &str,
) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
    box_.add_css_class("trommel-gpu-stat");
    box_.set_hexpand(true);
    let body = gtk::Box::new(gtk::Orientation::Vertical, 0);
    body.add_css_class("trommel-gpu-stat-body");
    let mut classes = vec!["trommel-gpu-stat-value"];
    if !value_class.is_empty() {
        classes.push(value_class);
    }
    body.append(&label(value_text, &classes));
    body.append(&label(label_text, &["trommel-gpu-stat-label"]));
    if let Some(hint) = hint {
        let hint_label = label(hint, &["trommel-gpu-stat-hint"]);
        hint_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        hint_label.set_tooltip_text(Some(hint));
        body.append(&hint_label);
    }
    // A grid row is as tall as its tallest tile, and a row-mate that carries a
    // hint line is taller than a tile without one. Give the leftover height to
    // the text body, not to the holder: the holder then keeps its natural
    // height and every graph in the row keeps the same height and still ends on
    // the tile floor. Letting the holder expand instead left the chart pinned
    // to the top of the extra space.
    body.set_vexpand(true);
    box_.append(&body);
    if let Some(spark) = spark {
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        holder.set_margin_top(4);
        holder.append(&spark);
        box_.append(&holder);
    }
    box_
}

/// The per-card hashrate is the card's visual anchor: one large value and its
/// trend above the six live-value bubbles. It is not a seventh equal bubble.
fn gpu_hashrate_row(app: &Rc<App>, gpu: &GpuStats, status: &Status) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 2);
    box_.add_css_class("trommel-gpu-hashrate-row");
    let top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let left = gtk::Box::new(gtk::Orientation::Vertical, 0);
    left.set_hexpand(true);
    left.append(&label(
        &tr("HASHRATE", "ХЕШРЕЙТ"),
        &["caption", "trommel-hero-caption"],
    ));
    let unit = if status.coin.hashrate_unit.is_empty() {
        "auto"
    } else {
        status.coin.hashrate_unit.as_str()
    };
    let value = gpu_miner_hashrate(status, gpu);
    let (value_text, value_class, tip) = match value {
        Some(value) if value > 0.0 => (
            format_hashrate_value(Some(value), unit).unwrap_or_else(|| "—".into()),
            "accent",
            tr(
                "Current per-card hashrate reported by the miner",
                "Текущий хешрейт карты по данным майнера",
            ),
        ),
        Some(_) => (
            zero_hashrate_text(unit),
            "trommel-hot",
            tr(
                "The miner reports zero hashrate for this card",
                "Майнер сообщает нулевой хешрейт для этой карты",
            ),
        ),
        None => (
            "—".to_string(),
            "dim-label",
            if status.state.running {
                tr(
                    "The miner does not report per-card hashrate for this GPU",
                    "Майнер не сообщает хешрейт для этой карты",
                )
            } else {
                tr("Miner is stopped", "Майнер остановлен")
            },
        ),
    };
    let requested = "trommel-gpu-hashrate";
    let value_label = label(&value_text, &[requested, "numeric", value_class]);
    value_label.set_tooltip_text(Some(&tip));
    left.append(&value_label);
    top.append(&left);
    if matches!(value, Some(0.0)) {
        top.append(&gpu_chip(&tr("no hashrate", "нет хешрейта"), "trommel-hot"));
    }
    box_.append(&top);
    let spark = sparkline(app, &gpu.uuid, Metric::Hashrate, value_class, "Hashrate");
    let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
    holder.set_margin_top(4);
    holder.append(&spark);
    box_.append(&holder);
    box_
}

fn make_gpu_card(app: &Rc<App>, gpu: &GpuStats, wide: bool, status: &Status) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("trommel-gpu-card");
    if wide {
        // A one-card rig should use the whole row instead of a lonely 240 px
        // tile with empty space to the right.
        card.add_css_class("trommel-gpu-card-wide");
        card.set_hexpand(true);
        card.set_halign(gtk::Align::Fill);
    } else {
        card.set_hexpand(true);
        card.set_size_request(240, -1);
    }
    card.set_tooltip_text(Some(&gpu.uuid));

    let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let titles = gtk::Box::new(gtk::Orientation::Vertical, 1);
    titles.set_hexpand(true);
    let title = label(&format!("GPU #{}", gpu.index), &["trommel-gpu-title"]);
    let name = label(&gpu.name, &["caption", "dim-label"]);
    name.set_ellipsize(gtk::pango::EllipsizeMode::End);
    name.set_max_width_chars(if wide { 48 } else { 14 });
    titles.append(&title);
    titles.append(&name);
    header.append(&titles);

    let toggle = gtk::Switch::new();
    let has_preset = app.config.borrow().preset().is_some();
    let enabled = app
        .config
        .borrow()
        .gpu_profile()
        .map(|profile| profile.tuning_for(&gpu.uuid).enabled)
        .unwrap_or(false);
    toggle.set_active(enabled);
    toggle.set_sensitive(has_preset);
    toggle.set_valign(gtk::Align::Center);
    toggle.set_tooltip_text(Some(&if has_preset {
        tr(
            "Include this card in the active preset. An excluded card is not passed to the miner and is not tuned.",
            "Включить карту в активный пресет. Исключённая карта не передаётся майнеру и не разгоняется.",
        )
    } else {
        tr(
            "Create a mining preset first; GPU participation belongs to the active preset.",
            "Сначала создайте пресет майнинга; участие карты относится к активному пресету.",
        )
    }));
    let switch_label = tr(
        &format!("GPU {} mining", gpu.index),
        &format!("Карта {} в майнинге", gpu.index),
    );
    toggle.update_property(&[gtk::accessible::Property::Label(&switch_label)]);
    {
        let app = app.clone();
        let uuid = gpu.uuid.clone();
        let updating = Rc::new(Cell::new(false));
        let updating_for_cancel = updating.clone();
        let weak = toggle.downgrade();
        toggle.connect_notify_local(Some("active"), move |toggle, _| {
            if updating.get() {
                return;
            }
            let new_enabled = toggle.is_active();
            let (previous, previous_enabled) = {
                let cfg = app.config.borrow();
                let profile = cfg.gpu_profile();
                (
                    profile.and_then(|profile| profile.per_gpu.get(&uuid).cloned()),
                    profile
                        .map(|profile| profile.tuning_for(&uuid).enabled)
                        .unwrap_or(true),
                )
            };
            if previous_enabled == new_enabled {
                return;
            }
            {
                let mut cfg = app.config.borrow_mut();
                if let Some(profile) = cfg.gpu_profile_mut() {
                    let custom = profile.is_custom(&uuid);
                    let mut tuning = profile.tuning_for(&uuid);
                    tuning.custom = custom;
                    tuning.enabled = new_enabled;
                    profile.per_gpu.insert(uuid.clone(), tuning);
                } else {
                    return;
                }
            }
            mark_dirty(&app);
            let app_for_cancel = app.clone();
            let uuid_for_cancel = uuid.clone();
            let weak_for_cancel = weak.clone();
            let updating_for_cancel = updating_for_cancel.clone();
            let on_cancel = Rc::new(move || {
                {
                    let mut cfg = app_for_cancel.config.borrow_mut();
                    if let Some(profile) = cfg.gpu_profile_mut() {
                        match previous.clone() {
                            Some(previous) => {
                                profile.per_gpu.insert(uuid_for_cancel.clone(), previous);
                            }
                            None => {
                                profile.per_gpu.remove(&uuid_for_cancel);
                            }
                        }
                    }
                }
                if let Some(toggle) = weak_for_cancel.upgrade() {
                    updating_for_cancel.set(true);
                    toggle.set_active(previous_enabled);
                    updating_for_cancel.set(false);
                }
                let dirty = *app_for_cancel.config.borrow() != *app_for_cancel.applied.borrow();
                app_for_cancel.dirty.set(dirty);
                update_dirty(&app_for_cancel);
                app_for_cancel.toast.add_toast(adw::Toast::new(&tr(
                    "Change cancelled",
                    "Изменение отменено",
                )));
            });
            save_and_apply_with(&app, Some(on_cancel));
        });
    }
    header.append(&toggle);
    card.append(&header);
    card.append(&gpu_hashrate_row(app, gpu, status));

    let temp = gpu
        .temp_c
        .map(|value| format!("{value} °C"))
        .unwrap_or_else(|| "—".into());
    let temp_class = match gpu.temp_c {
        Some(value) if value >= 80 => "trommel-hot",
        Some(value) if value >= 70 => "trommel-warm",
        _ => "",
    };
    let power = gpu
        .power_w
        .map(|value| format!("{value:.0} W"))
        .unwrap_or_else(|| "—".into());
    let tuning = app
        .config
        .borrow()
        .gpu_profile()
        .map(|profile| profile.tuning_for(&gpu.uuid))
        .unwrap_or_default();
    let power_class = if tuning
        .power_limit_w
        .zip(gpu.power_limit_w)
        .map(|(want, have)| (want as f64 - have).abs() > 1.0)
        .unwrap_or(false)
    {
        "trommel-hot"
    } else {
        ""
    };
    let power_hint = tuning
        .power_limit_w
        .map(|value| format!("{} {value} W", tr("target", "цель")))
        .or_else(|| {
            gpu.power_limit_w
                .map(|value| format!("{} {value:.0} W", tr("limit", "лимит")))
        });
    let core_text = gpu
        .core_mhz
        .map(|value| format!("{value} MHz"))
        .unwrap_or_else(|| "— MHz".into());
    let core_hint = tuning
        .core_clock_mhz
        .map(|value| format!("{} {value}", tr("lock", "лок")));
    // Clocks idle far below a lock when the card is not mining; only flag a
    // mismatch while the GPU is actually loaded.
    let loaded = gpu.util_pct.unwrap_or(0) >= 50;
    let core_class = if loaded
        && tuning
            .core_clock_mhz
            .zip(gpu.core_mhz)
            .map(|(want, have)| want.abs_diff(have) > 100)
            .unwrap_or(false)
    {
        "trommel-hot"
    } else {
        ""
    };
    let memory_text = gpu
        .mem_mhz
        .map(|value| format!("{value} MHz"))
        .unwrap_or_else(|| "— MHz".into());
    let memory_hint = tuning
        .memory_clock_mhz
        .map(|value| format!("{} {value}", tr("lock", "лок")));
    let memory_class = if loaded
        && tuning
            .memory_clock_mhz
            .zip(gpu.mem_mhz)
            .map(|(want, have)| want.abs_diff(have) > 50)
            .unwrap_or(false)
    {
        "trommel-hot"
    } else {
        ""
    };
    let fan = gpu
        .fan_pct
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "—".into());
    let utilization = gpu
        .util_pct
        .map(|value| format!("{value}%"))
        .unwrap_or_else(|| "—".into());
    let tuning_label = {
        let cfg = app.config.borrow();
        match cfg.gpu_profile() {
            None => tr("no preset", "нет пресета"),
            Some(profile) => {
                let tuning = profile.tuning_for(&gpu.uuid);
                if !tuning.gpu_preset.is_empty() {
                    tr("gpu preset", "пресет GPU")
                } else if tuning.has_tuning() {
                    tr("tuned", "настроено")
                } else {
                    tr("stock", "сток")
                }
            }
        }
    };
    let stats = gtk::Grid::new();
    stats.set_column_spacing(if wide { 10 } else { 8 });
    stats.set_row_spacing(if wide { 10 } else { 8 });
    stats.set_column_homogeneous(true);
    // One card gets three columns (two rows); a rig of several cards keeps
    // the compact 2 x 3 block.
    let columns: i32 = if wide { 3 } else { 2 };
    // Power and temperature are trends; core, memory, fan and load are plain
    // live values. Keeping the two charts in the first row leaves the other
    // rows compact without changing the card's structure between updates.
    let stats_data = [
        (
            Some(Metric::Power),
            tr("Power", "Питание"),
            power,
            power_hint.as_deref(),
            power_class,
        ),
        (
            Some(Metric::Temp),
            tr("Temp", "Темп."),
            temp,
            None,
            temp_class,
        ),
        (
            None,
            tr("Core", "Ядро"),
            core_text,
            core_hint.as_deref(),
            core_class,
        ),
        (
            None,
            tr("Memory", "Память"),
            memory_text,
            memory_hint.as_deref(),
            memory_class,
        ),
        (None, tr("Fan", "Кулер"), fan, None, ""),
        (None, tr("Load", "Загрузка"), utilization, None, ""),
    ];
    for (index, (metric, label_text, value_text, hint, class)) in stats_data.iter().enumerate() {
        let spark = metric.map(|metric| sparkline(app, &gpu.uuid, metric, class, label_text));
        stats.attach(
            &gpu_stat(spark, label_text, value_text, *hint, class),
            index as i32 % columns,
            index as i32 / columns,
            1,
            1,
        );
    }
    card.append(&stats);

    let role = status
        .participation
        .iter()
        .find(|card| card.uuid == gpu.uuid);
    let role_enabled = role.map(|card| card.enabled).unwrap_or(enabled);
    let mining = role.map(|card| card.mining).unwrap_or(false);
    // The chip describes the applied session, not the draft switch. A card
    // whose switch was just turned off still says "mining" until Apply.
    let (role_text, role_class) = if !has_preset {
        (String::new(), "")
    } else if status.state.external {
        (tr("external", "вручную"), "trommel-warm")
    } else if !role_enabled {
        (tr("excluded", "исключена"), "trommel-muted")
    } else if mining {
        (tr("mining", "майнит"), "success")
    } else if status.state.running {
        (tr("not mining", "не майнит"), "trommel-warm")
    } else {
        (String::new(), "")
    };

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    if !role_text.is_empty() {
        footer.append(&gpu_chip(&role_text, role_class));
    }
    footer.append(&gpu_chip(&tuning_label, "trommel-muted"));
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    footer.append(&spacer);
    let tune = gtk::Button::builder()
        .label(tr("Tune", "Настроить"))
        .css_classes(["pill", "flat"])
        .sensitive(has_preset)
        .tooltip_text(if has_preset {
            tr(
                "Tune this card inside the active preset",
                "Настроить карту в активном пресете",
            )
        } else {
            tr(
                "Create a mining preset first",
                "Сначала создайте пресет майнинга",
            )
        })
        .build();
    {
        let app = app.clone();
        let gpu = gpu.clone();
        tune.connect_clicked(move |_| gpu_tune_dialog(&app, &gpu));
    }
    footer.append(&tune);
    card.append(&footer);
    card.upcast()
}

fn refresh_gpu_cards(app: &Rc<App>, status: &Status) {
    let gpus = &status.gpus;
    // The registry only holds weak refs, but the old areas are about to be
    // destroyed, so drop their entries now instead of on the next tick.
    app.sparklines.borrow_mut().clear();
    clear_box(&app.gpu_host);
    if gpus.is_empty() {
        let empty = label(
            &tr("No NVIDIA GPU detected", "NVIDIA GPU не обнаружен"),
            &["dim-label"],
        );
        empty.set_halign(gtk::Align::Center);
        empty.set_margin_top(8);
        empty.set_margin_bottom(8);
        app.gpu_host.append(&empty);
        return;
    }
    if gpus.len() == 1 {
        app.gpu_host
            .append(&make_gpu_card(app, &gpus[0], true, status));
        return;
    }
    // WrapBox rather than FlowBox: on a wide window the cards share the row
    // instead of hugging the left edge, and on a narrow one they wrap.
    let wrap = adw::WrapBox::builder()
        .child_spacing(10)
        .line_spacing(10)
        .justify(adw::JustifyMode::Fill)
        .build();
    for gpu in gpus {
        wrap.append(&make_gpu_card(app, gpu, false, status));
    }
    app.gpu_host.append(&wrap);
}

fn gpu_chip(text: &str, class: &str) -> gtk::Label {
    let chip = label(text, &["caption", "trommel-gpu-chip"]);
    if !class.is_empty() {
        chip.add_css_class(class);
    }
    chip
}

fn has_selected_gpu(app: &Rc<App>, gpus: &[GpuStats]) -> bool {
    if gpus.is_empty() {
        return true;
    }
    let profile = app
        .config
        .borrow()
        .gpu_profile()
        .cloned()
        .unwrap_or_default();
    gpus.iter().any(|gpu| profile.tuning_for(&gpu.uuid).enabled)
}

/// Why Start is disabled, or `None` when it is allowed.
///
/// Besides "every card is excluded", this catches the case the daemon would
/// refuse anyway: a partial exclusion on a miner without a documented device
/// flag. Explaining it in the button tooltip is better than a failed start.
fn start_block_reason(app: &Rc<App>) -> Option<String> {
    let status = app.status.borrow();
    let status = status.as_ref()?;
    if status.state.running || status.gpus.is_empty() {
        return None;
    }
    if !has_selected_gpu(app, &status.gpus) {
        return Some(tr(
            "All GPUs are excluded from mining; enable one on the dashboard before starting",
            "Все карты исключены из майнинга — включите хотя бы одну на дашборде перед запуском",
        ));
    }
    let cfg = app.config.borrow().clone();
    let preset = cfg.preset()?;
    trommel_core::miners::device_plan(&cfg, preset, &status.gpus)
        .err()
        .map(|err| err.to_string())
}

/// Keep the hero action's visible text and its accessible name in sync: the
/// button wraps an icon and a label, so there is no `label` property to derive
/// the name from.
fn set_action_text(button: &gtk::Button, label: &gtk::Label, text: &str) {
    label.set_text(text);
    button.update_property(&[gtk::accessible::Property::Label(text)]);
}

fn update_start_sensitivity(app: &Rc<App>, reason: Option<String>) {
    app.action_button.set_sensitive(reason.is_none());
    app.action_button.set_tooltip_text(reason.as_deref());
}

/// Tooltip text: one header line plus one line per card, when there is any.
fn card_tooltip(header: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        header.to_string()
    } else {
        format!("{header}\n\n{}", lines.join("\n"))
    }
}

/// Uptime at `now`, anchored to the daemon's last sample.
fn uptime_since(base: u64, sampled_at: u64, now: u64) -> u64 {
    base.saturating_add(now.saturating_sub(sampled_at))
}

/// Uptime to paint when a status snapshot arrives.
///
/// The daemon refreshes its cached status on `behavior.poll_interval_s`, while
/// the GUI asks for a status every second. A cached snapshot still carries the
/// time it was collected (`ts`), so extend its uptime to now; painting
/// `state.uptime_s` directly rewound the stopwatch to the last daemon sample
/// and produced 46, 47, 46, 47, 48, 49, 48, 49, 50.
fn status_uptime(status: &Status, now: u64) -> u64 {
    uptime_since(status.state.uptime_s, status.ts, now)
}

/// Repaint only the hero subtitle. Called on every status and once per second
/// in between, so the uptime is a real clock rather than a poll counter.
fn render_hero_meta(app: &App, uptime_s: u64) {
    let Some(meta) = app.hero_meta_state.borrow().clone() else {
        return;
    };
    if !meta.running {
        app.hero_meta
            .set_text(&tr("miner is stopped", "майнер остановлен"));
    } else if !meta.hashrate_known {
        app.hero_meta.set_text(&format!(
            "{} · {}: {}",
            tr("waiting for miner data", "ожидаю данные от майнера"),
            tr("uptime", "аптайм"),
            util::human_duration(uptime_s)
        ));
    } else {
        let mut parts = vec![format!(
            "{}: {}",
            tr("uptime", "аптайм"),
            util::human_duration(uptime_s)
        )];
        if meta.restarts > 0 {
            parts.push(format!(
                "{}: {}",
                tr("restarts", "перезапусков"),
                meta.restarts
            ));
        }
        if let Some(expected) = &meta.expected {
            parts.push(format!("{} {expected}", tr("expected", "ожидалось")));
        }
        if let Some(actual) = &meta.actual {
            parts.push(format!("{} {actual}", tr("actual", "сейчас")));
        }
        if let Some(efficiency) = &meta.efficiency {
            parts.push(efficiency.clone());
        }
        app.hero_meta.set_text(&parts.join(" · "));
        app.hero_meta.set_tooltip_text(Some(&format!(
            "{}: {}",
            tr("Miner data source", "Источник данных майнера"),
            meta.source
        )));
    }
}

/// What the pool balance row shows. `—` means "no account data"; only an API
/// value is allowed to render a number, so a missing endpoint never looks like
/// a zero balance.
struct BalanceView {
    text: String,
    class: &'static str,
    tip: String,
}

fn pool_balance_view(status: &Status, has_wallet: bool) -> BalanceView {
    let symbol = if status.coin.symbol.is_empty() {
        "—"
    } else {
        status.coin.symbol.as_str()
    };
    if let Some(balance) = status.pool.as_ref().and_then(|pool| pool.balance) {
        return BalanceView {
            text: format!("{balance:.3} {symbol}"),
            class: "accent",
            tip: tr("Pending pool balance", "Баланс пула к выплате").to_string(),
        };
    }
    let api_available = status
        .pool_view
        .as_ref()
        .map(|view| view.api_available)
        .unwrap_or(false);
    let tip = if let Some(error) = &status.pool_error {
        format!("{} {error}", tr("Pool API error:", "Ошибка API пула:"))
    } else if !has_wallet {
        tr("Wallet is not set", "Кошелёк не задан").to_string()
    } else if !api_available {
        tr(
            "This pool has no documented account API; balance is unavailable",
            "У этого пула нет документированного API аккаунта — баланс недоступен",
        )
        .to_string()
    } else {
        tr(
            "No pool account data yet; the miner is not running or the API has not answered",
            "Данных аккаунта пула пока нет: майнер не запущен или API не ответил",
        )
        .to_string()
    };
    BalanceView {
        text: "—".to_string(),
        class: "dim-label",
        tip,
    }
}

fn zero_hashrate_text(unit: &str) -> String {
    let unit = unit.trim();
    if unit.is_empty() || unit.eq_ignore_ascii_case("auto") {
        "0 H/s".to_string()
    } else {
        format!("0 {unit}")
    }
}

/// Format a hashrate without hiding a real zero. `None` is unknown (`—`), a
/// real `0.0` from the miner is still a fact worth showing.
fn format_hashrate_value(value: Option<f64>, unit: &str) -> Option<String> {
    value.map(|value| {
        if value > 0.0 {
            util::format_hashrate_in(value, unit)
        } else {
            zero_hashrate_text(unit)
        }
    })
}

fn shares_known(status: &Status) -> bool {
    status.miner.accepted.is_some()
        || status.miner.rejected.is_some()
        || status.miner.stale.is_some()
}

fn shares_summary(status: &Status) -> String {
    if !shares_known(status) {
        return "—".to_string();
    }
    let accepted = status.miner.accepted.unwrap_or(0);
    let rejected = status.miner.rejected.unwrap_or(0);
    let stale = status.miner.stale.unwrap_or(0);
    let acceptance = status
        .acceptance_pct()
        .map(|pct| format!("{pct:.2}%"))
        .unwrap_or_else(|| "—".into());
    tr(
        &format!(
            "{accepted} accepted · {rejected} rejected · {stale} stale · {acceptance} acceptance"
        ),
        &format!(
            "{accepted} принято · {rejected} отклонено · {stale} устарело · {acceptance} принятие"
        ),
    )
}

/// Hero identity: coin, pool and fee. The preset name belongs to the Active
/// preset row, not here.
fn hero_identity(status: &Status) -> String {
    let coin = if !status.coin.symbol.is_empty() {
        status.coin.symbol.as_str()
    } else if !status.coin.name.is_empty() {
        status.coin.name.as_str()
    } else {
        "—"
    };
    let pool = status
        .pool_view
        .as_ref()
        .map(|view| view.name.as_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("—");
    tr(
        &format!("{coin} · {pool} · {:.2}% fee", status.total_fee_pct),
        &format!("{coin} · {pool} · комиссия {:.2}%", status.total_fee_pct),
    )
}

/// Subtitle of the Active preset row: the applied session context and the rig
/// summary that used to live in invisible widgets.
fn active_preset_summary(status: &Status, cfg: &Config) -> String {
    let enabled = status
        .participation
        .iter()
        .filter(|card| card.enabled)
        .count();
    let total = status.participation.len();
    let tuning = cfg
        .preset()
        .map(|preset| preset.gpu.has_tuning())
        .unwrap_or(false);
    let mut parts = Vec::new();
    let miner = if status.miner_info.name.is_empty() {
        status.miner_info.id.clone()
    } else {
        status.miner_info.name.clone()
    };
    if !miner.is_empty() {
        parts.push(format!("{} {}", miner, status.miner_info.version));
    }
    parts.push(tr(
        &format!("{enabled}/{total} cards"),
        &format!("{enabled}/{total} карт"),
    ));
    parts.push(if tuning {
        tr("tuned", "настроено").to_string()
    } else {
        tr("stock", "сток").to_string()
    });
    if let Some(power) = status.total.power_w {
        parts.push(format!("{power:.0} W"));
    }
    if let Some(temp) = status.total.temp_max_c {
        parts.push(format!("{temp} °C"));
    }
    parts.join(" · ")
}

fn rig_tooltip(status: &Status) -> String {
    let mut lines = Vec::new();
    for gpu in &status.gpus {
        let role = status
            .participation
            .iter()
            .find(|card| card.uuid == gpu.uuid);
        let role_text = match role {
            Some(card) if !card.enabled => tr("excluded", "исключена"),
            Some(card) if card.mining => tr("mining", "майнит"),
            _ => tr("selected", "выбрана"),
        };
        let power = gpu
            .power_w
            .map(|value| format!("{value:.0} W"))
            .unwrap_or_else(|| "—".into());
        let temp = gpu
            .temp_c
            .map(|value| format!("{value} °C"))
            .unwrap_or_else(|| "—".into());
        lines.push(format!(
            "{} #{} · {role_text} · {power} · {temp}",
            tr("Card", "Карта"),
            gpu.index
        ));
    }
    card_tooltip(
        &tr(
            "Applied session: cards, whole-rig power (including excluded cards) and hottest temperature.",
            "Применённая сессия: карты, мощность всего рига (включая исключённые) и самая горячая карта.",
        ),
        &lines,
    )
}

/// Body of the restart/stop alert. `ConfigDiff` carries the class and reasons,
/// not old→new values, so this never invents a value it cannot know.
fn apply_alert_body(diff: &ConfigDiff, switching_preset: bool) -> String {
    let mut body = String::new();
    if switching_preset {
        body.push_str(&tr(
            "The active preset will change. ",
            "Активный пресет изменится. ",
        ));
    }
    if !diff.reasons.is_empty() {
        body.push_str(&tr(
            "These changes need a restart:\n",
            "Эти изменения требуют перезапуска:\n",
        ));
        for reason in &diff.reasons {
            body.push_str(&format!("• {reason}\n"));
        }
        body.push('\n');
    }
    body.push_str(&tr(
        "Apply and restart stops the miner, restores the stock GPU state, applies the configuration and starts it again. Apply and stop saves the configuration and keeps the miner stopped.",
        "«Применить и перезапустить» остановит майнер, вернёт сток GPU, применит конфигурацию и запустит его снова. «Применить и остановить» сохранит конфигурацию и оставит майнер остановленным.",
    ));
    body
}

fn apply_status(app: &Rc<App>, status: Status) {
    let running = status.state.running;
    // The sparklines read this rolling window; a fresh status may repeat the
    // previous timestamp, in which case `record` replaces the last sample.
    app.history.borrow_mut().record(&status);
    let blocked = start_block_reason(app);
    update_start_sensitivity(app, blocked);
    let action_text = if running {
        tr("Stop", "Остановить")
    } else {
        tr("Start", "Запустить")
    };
    set_action_text(&app.action_button, &app.action_label, &action_text);
    app.action_icon.set_icon_name(Some(if running {
        "media-playback-stop-symbolic"
    } else {
        "media-playback-start-symbolic"
    }));
    app.action_button.set_css_classes(&[if running {
        "destructive-action"
    } else {
        "suggested-action"
    }]);
    let (pill_text, pill_class) = if running {
        if status.state.external {
            (tr("external", "вручную"), "trommel-pill-external")
        } else {
            (tr("mining", "майнится"), "trommel-pill-running")
        }
    } else {
        (tr("stopped", "остановлен"), "trommel-pill-stopped")
    };
    if !app.config.borrow().preset().is_some() {
        app.pill.set_text(&tr("no preset", "нет пресета"));
        app.pill
            .set_css_classes(&["caption", "trommel-pill", "trommel-pill-stopped"]);
    } else {
        app.pill.set_text(&pill_text);
        app.pill
            .set_css_classes(&["caption", "trommel-pill", pill_class]);
    }
    let hashrate_value = status.total.hashrate.or(status.miner.primary());
    let unit = if status.coin.hashrate_unit.is_empty() {
        "auto"
    } else {
        status.coin.hashrate_unit.as_str()
    };
    let has_preset = app.config.borrow().preset().is_some();
    let hashrate = if !has_preset {
        "—".to_string()
    } else {
        format_hashrate_value(hashrate_value, unit).unwrap_or_else(|| {
            if running {
                // Running but no miner report yet: unknown is honest. A zero
                // placeholder belongs to the stopped state only.
                "—".to_string()
            } else {
                zero_hashrate_text(unit)
            }
        })
    };
    app.hero_hashrate.set_text(&hashrate);
    app.create_button.set_visible(!has_preset);
    app.hero_hashrate
        .set_tooltip_text(Some(&if hashrate_value.is_some() {
            tr("Current miner hashrate", "Текущий хешрейт майнера")
        } else if running {
            tr(
                "The miner has not reported a hashrate yet",
                "Майнер ещё не сообщил хешрейт",
            )
        } else {
            tr("Miner is stopped", "Майнер остановлен")
        }));
    app.hero_hashrate.set_css_classes(&[
        "trommel-hero-hashrate",
        "numeric",
        if hashrate_value.is_some_and(|value| value > 0.0) {
            "accent"
        } else {
            "dim-label"
        },
    ]);
    if !has_preset {
        app.hero_line.set_text(&tr(
            "Create a mining preset to start",
            "Создайте пресет майнинга, чтобы начать",
        ));
    } else {
        app.hero_line.set_text(&hero_identity(&status));
    }
    let expected = (status.expected_hashrate > 0.0)
        .then(|| util::format_hashrate_in(status.expected_hashrate, unit));
    let actual = hashrate_value.and_then(|value| {
        let text = format_hashrate_value(Some(value), unit)?;
        let percent = if status.expected_hashrate > 0.0 {
            format!(" ({:.0}%)", value / status.expected_hashrate * 100.0)
        } else {
            String::new()
        };
        Some(format!("{text}{percent}"))
    });
    let efficiency = status
        .total
        .efficiency
        .zip(Some(status.total.efficiency_unit.clone()))
        .filter(|(_, unit)| !unit.is_empty())
        .map(|(value, unit)| format!("{value:.2} {unit}"));
    *app.hero_meta_state.borrow_mut() = Some(HeroMeta {
        running,
        hashrate_known: hashrate_value.is_some(),
        restarts: status.state.restarts,
        source: status.miner.source.clone(),
        expected,
        actual,
        efficiency,
        uptime_s: status.state.uptime_s,
        sampled_at: status.ts,
        tick: running && !status.state.external,
    });
    render_hero_meta(app, status_uptime(&status, util::now_unix()));
    {
        let cfg = app.config.borrow();
        if let Some(preset) = cfg.preset() {
            app.rig_preset.set_title(&format!(
                "{}: {}",
                tr("Active preset", "Активный пресет"),
                preset.name
            ));
            app.rig_preset
                .set_subtitle(&active_preset_summary(&status, &cfg));
            app.rig_preset.set_tooltip_text(Some(&rig_tooltip(&status)));
            app.rig_edit.set_sensitive(true);
            app.rig_edit
                .set_label(&tr("Edit preset", "Изменить пресет"));
        } else {
            app.rig_preset
                .set_title(&tr("No active preset", "Нет активного пресета"));
            app.rig_preset.set_subtitle(&tr(
                "Create a preset to choose cards for mining",
                "Создайте пресет, чтобы выбрать карты для майнинга",
            ));
            app.rig_preset.set_tooltip_text(None);
            app.rig_edit
                .set_label(&tr("Create preset", "Создать пресет"));
            app.rig_edit.set_sensitive(true);
        }
    }
    app.shares_group.set_visible(has_preset);
    app.shares.set_subtitle(&shares_summary(&status));
    let has_wallet = app
        .config
        .borrow()
        .preset()
        .map(|preset| !preset.wallet.trim().is_empty())
        .unwrap_or(false);
    let balance = pool_balance_view(&status, has_wallet);
    let symbol = if status.coin.symbol.is_empty() {
        "—"
    } else {
        status.coin.symbol.as_str()
    };
    app.pool_balance_value.set_text(&balance.text);
    app.pool_balance_value
        .set_css_classes(&["caption", balance.class]);
    app.pool_balance_value.set_tooltip_text(Some(&balance.tip));
    app.pool_balance_row.set_tooltip_text(Some(&balance.tip));
    if let Some(view) = &status.pool_view {
        app.pool_meta.set_title(&view.name);
        app.pool_meta.set_subtitle(&format!(
            "{} · {} {:.2}% · {}",
            view.url,
            tr("pool fee", "комиссия пула"),
            view.fee_pct,
            if view.api_available {
                tr("API on", "API вкл.")
            } else {
                tr("no API", "без API")
            }
        ));
        app.pool_meta.set_tooltip_text(Some(&tr(
            "Pool endpoint and total pool fee; a known pool supplies its documented JSON API automatically",
            "Адрес пула и комиссия пула; известный пул подставляет документированный JSON API автоматически",
        )));
    } else {
        app.pool_meta.set_title(&tr("Pool", "Пул"));
        app.pool_meta
            .set_subtitle(&tr("Pool is not selected", "Пул не выбран"));
    }
    if let Some(pool) = &status.pool {
        let mut parts = Vec::new();
        if let Some(paid) = pool.paid {
            parts.push(format!("{} {paid:.3} {symbol}", tr("paid", "выплачено")));
        }
        if let Some(immature) = pool.immature {
            parts.push(format!(
                "{} {immature:.3} {symbol}",
                tr("immature", "незрелых")
            ));
        }
        app.pool_balance_row.set_subtitle(&parts.join(" · "));
        let mut details = Vec::new();
        if let Some(workers) = pool.workers_online {
            details.push(format!("{} {workers}", tr("workers", "воркеров")));
        }
        if let Some(min_payout) = pool.min_payout {
            details.push(format!(
                "{} {min_payout} {symbol}",
                tr("min payout", "мин. выплата")
            ));
        }
        app.pool_balance_row
            .set_tooltip_text(Some(&card_tooltip(&balance.tip, &details)));
    } else {
        let subtitle = if !has_wallet {
            tr("Wallet is not set", "Кошелёк не задан")
        } else if !status
            .pool_view
            .as_ref()
            .map(|view| view.api_available)
            .unwrap_or(false)
        {
            tr(
                "This pool has no documented account API",
                "У этого пула нет документированного API аккаунта",
            )
        } else {
            tr(
                "No account data yet; the endpoint is shown above",
                "Данных аккаунта пока нет; endpoint показан выше",
            )
        };
        app.pool_balance_row.set_subtitle(&subtitle);
    }
    app.oc_banner.set_revealed(status.oc.needs_restore);
    if status.oc.needs_restore {
        let title = if status.oc.last_error.is_some() {
            tr(
                "Stock restore failed; GPU settings may still be applied",
                "Возврат стока не удался — настройки GPU могли остаться",
            )
        } else if !status.oc.restore_on_stop {
            tr(
                "GPU settings are still applied (restore on stop is off)",
                "Настройки GPU всё ещё применены (возврат при остановке выключен)",
            )
        } else {
            tr(
                "GPU settings are still applied",
                "Настройки GPU всё ещё применены",
            )
        };
        app.oc_banner.set_title(&title);
    }
    if status.warnings.is_empty() {
        app.warnings.set_title(&tr("No notes", "Замечаний нет"));
        app.warning_badge.set_text("✓");
        app.warning_badge.set_css_classes(&["caption", "success"]);
    } else {
        app.warnings.set_title(&status.warnings.join(" · "));
        app.warning_badge.set_text("⚠");
        app.warning_badge.set_css_classes(&["caption", "warning"]);
    }

    let signature = format!(
        "{:?}|{}|{}|{}|{}",
        app.config.borrow().gpu_profile(),
        status.state.running,
        status.state.external,
        status
            .participation
            .iter()
            .map(|card| format!("{}:{}:{}", card.index, card.enabled, card.mining))
            .collect::<Vec<_>>()
            .join("|"),
        status
            .gpus
            .iter()
            .map(|gpu| {
                format!(
                    "{}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}",
                    gpu.uuid,
                    gpu.temp_c,
                    gpu.power_w,
                    gpu.core_mhz,
                    gpu.mem_mhz,
                    gpu.util_pct,
                    gpu.fan_pct,
                    gpu_miner_hashrate(&status, gpu)
                )
            })
            .collect::<Vec<_>>()
            .join("|")
    );
    if *app.gpu_sig.borrow() != signature {
        *app.gpu_sig.borrow_mut() = signature;
        refresh_gpu_cards(app, &status);
    }
    update_sparklines(app);
    *app.status.borrow_mut() = Some(status);
}

// --------------------------------------------------------------- screenshot

/// Scroll the first scrolled window inside `widget` to its bottom.
///
/// `AdwPreferencesPage` owns an internal `GtkScrolledWindow`; GTK does not
/// expose it, so the screenshot helper walks the widget tree once.
fn scroll_to_bottom(widget: &gtk::Widget) -> bool {
    if let Some(scrolled) = widget.downcast_ref::<gtk::ScrolledWindow>() {
        let adjustment = scrolled.vadjustment();
        adjustment.set_value(adjustment.upper());
        return true;
    }
    let mut child = widget.first_child();
    while let Some(node) = child {
        if scroll_to_bottom(&node) {
            return true;
        }
        child = node.next_sibling();
    }
    false
}

fn schedule_screenshot<W>(window: W, path: String, application: gtk::Application, attempt: u32)
where
    W: IsA<gtk::Widget> + IsA<gtk::Native> + Clone + 'static,
{
    match save_widget_png(&window, &path) {
        Ok(()) => application.quit(),
        Err(err) if attempt < 60 => {
            eprintln!("screenshot attempt {} failed: {err}", attempt + 1);
            window.queue_draw();
            glib::timeout_add_local(Duration::from_millis(400), move || {
                schedule_screenshot(
                    window.clone(),
                    path.clone(),
                    application.clone(),
                    attempt + 1,
                );
                glib::ControlFlow::Break
            });
        }
        Err(err) => {
            eprintln!("screenshot failed: {err}");
            application.quit();
        }
    }
}

fn save_widget_png<W>(window: &W, path: &str) -> anyhow::Result<()>
where
    W: IsA<gtk::Widget> + IsA<gtk::Native>,
{
    let width = window.width();
    let height = window.height();
    if width <= 0 || height <= 0 {
        anyhow::bail!("window is not rendered yet");
    }
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width as f64, height as f64);
    let node = snapshot
        .to_node()
        .ok_or_else(|| anyhow::anyhow!("empty render node"))?;
    let renderer = window
        .renderer()
        .ok_or_else(|| anyhow::anyhow!("window has no renderer"))?;
    let texture = renderer.render_texture(&node, None);
    texture
        .save_to_png(path)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        active_preset_summary, apply_alert_body, format_hashrate_value, hero_identity,
        pool_balance_view, shares_known, shares_summary, status_uptime, uptime_since, Metric,
        SCREENSHOT_HOOKS, SPARK_METRICS,
    };
    use std::collections::BTreeSet;
    use std::sync::Once;
    use trommel_core::config::{ChangeClass, Config, ConfigDiff, Preset};
    use trommel_core::i18n::{set_language, Language};
    use trommel_core::pool::PoolStats;
    use trommel_core::status::{GpuParticipation, PoolView, Status};

    #[test]
    fn uptime_advances_between_status_samples() {
        assert_eq!(uptime_since(0, 1_000, 1_000), 0);
        assert_eq!(uptime_since(42, 1_000, 1_003), 45);
        assert_eq!(uptime_since(42, 1_000, 1_001), 43);
        // A clock that steps backwards must not wrap the stopwatch.
        assert_eq!(uptime_since(42, 1_000, 900), 42);
    }

    #[test]
    fn cached_status_does_not_rewind_uptime() {
        let mut status = Status::default();
        status.state.running = true;
        status.state.uptime_s = 46;
        status.ts = 1_000;
        // The daemon hands the same cached snapshot to every GUI poll between
        // two refreshes. Extending its `ts` keeps the stopwatch moving instead
        // of resetting it to 46 each time.
        assert_eq!(status_uptime(&status, 1_001), 47);
        assert_eq!(status_uptime(&status, 1_002), 48);
        assert_eq!(status_uptime(&status, 900), 46);
    }

    fn use_english() {
        static ONCE: Once = Once::new();
        ONCE.call_once(|| set_language(Language::En));
    }

    fn status_with_preset() -> (Config, Status) {
        let mut cfg = Config::default();
        let preset = Preset::new_pearl();
        cfg.presets.insert(preset.id.clone(), preset);
        cfg.active_preset = Some(trommel_core::config::DEFAULT_PRESET_ID.to_string());
        let mut status = Status::default();
        status.coin.symbol = "PRL".into();
        status.total_fee_pct = 1.0;
        status.pool_view = Some(PoolView {
            name: "PearlHash".into(),
            url: "pool.pearlhash.xyz:9000".into(),
            fee_pct: 1.0,
            api_available: true,
        });
        status.miner_info.name = "WildRig Multi".into();
        status.miner_info.version = "0.51.8".into();
        status.total.power_w = Some(430.0);
        status.total.temp_max_c = Some(81);
        status.participation = vec![
            GpuParticipation {
                index: 0,
                uuid: "GPU-1".into(),
                name: "RTX 3080 Ti".into(),
                enabled: true,
                mining: true,
            },
            GpuParticipation {
                index: 1,
                uuid: "GPU-2".into(),
                name: "RTX 3060".into(),
                enabled: true,
                mining: true,
            },
        ];
        (cfg, status)
    }

    #[test]
    fn missing_pool_data_is_not_a_zero_balance() {
        use_english();
        let (_, mut status) = status_with_preset();
        status.pool = None;
        let view = pool_balance_view(&status, true);
        assert_eq!(view.text, "—");
        assert!(!view.text.contains("0.000"), "{:?}", view.text);
        assert!(view.tip.contains("No pool account data"), "{}", view.tip);
    }

    #[test]
    fn a_real_pool_zero_is_shown() {
        let (_, mut status) = status_with_preset();
        status.pool = Some(PoolStats {
            balance: Some(0.0),
            ..Default::default()
        });
        let view = pool_balance_view(&status, true);
        assert_eq!(view.text, "0.000 PRL");
    }

    #[test]
    fn a_pool_error_does_not_invent_a_balance() {
        use_english();
        let (_, mut status) = status_with_preset();
        status.pool = None;
        status.pool_error = Some("pool API is unavailable".into());
        let view = pool_balance_view(&status, true);
        assert_eq!(view.text, "—");
        assert!(view.tip.contains("unavailable"), "{}", view.tip);
    }

    #[test]
    fn hero_identity_has_coin_pool_and_fee_but_not_the_preset_name() {
        use_english();
        let (_, status) = status_with_preset();
        let text = hero_identity(&status);
        assert!(text.contains("PRL"), "{text}");
        assert!(text.contains("PearlHash"), "{text}");
        assert!(text.contains("1.00%"), "{text}");
        assert!(!text.contains("Pearl · WildRig"), "{text}");
    }

    #[test]
    fn active_preset_summary_carries_cards_power_and_temperature() {
        use_english();
        let (cfg, status) = status_with_preset();
        let text = active_preset_summary(&status, &cfg);
        assert!(text.contains("2/2"), "{text}");
        assert!(text.contains("430 W"), "{text}");
        assert!(text.contains("81 °C"), "{text}");
        assert!(text.contains("WildRig Multi 0.51.8"), "{text}");
    }

    #[test]
    fn apply_alert_lists_the_known_reasons() {
        use_english();
        let diff = ConfigDiff {
            class: ChangeClass::RestartRequired,
            reasons: vec!["Preset settings changed".into()],
        };
        let body = apply_alert_body(&diff, false);
        assert!(body.contains("Preset settings changed"), "{body}");
        assert!(body.contains("Apply and restart"), "{body}");
        assert!(!body.contains("0 W"), "{body}");
    }

    #[test]
    fn unknown_shares_are_a_dash_not_zero() {
        use_english();
        let (_, mut status) = status_with_preset();
        assert!(!shares_known(&status));
        assert_eq!(shares_summary(&status), "—");
        status.miner.accepted = Some(0);
        assert!(shares_known(&status));
        assert!(shares_summary(&status).contains("0 accepted"));
    }

    #[test]
    fn a_real_zero_hashrate_is_not_hidden() {
        assert_eq!(
            format_hashrate_value(Some(0.0), "TH/s").as_deref(),
            Some("0 TH/s")
        );
        assert_eq!(
            format_hashrate_value(Some(0.0), "auto").as_deref(),
            Some("0 H/s")
        );
        assert_eq!(format_hashrate_value(None, "TH/s"), None);
    }

    #[test]
    fn gpu_hashrate_only_uses_a_mapped_record() {
        use super::gpu_miner_hashrate;
        use trommel_core::minerapi::GpuMinerStats;
        use trommel_core::nvidia::GpuStats;

        let gpu = GpuStats {
            index: 0,
            uuid: "GPU-0".into(),
            ..Default::default()
        };
        let mut status = Status::default();
        status.miner.gpus = vec![GpuMinerStats {
            index: Some(0),
            uuid: Some("GPU-0".into()),
            device: "gpu0".into(),
            hashrate_10s: Some(100.0e12),
            ..Default::default()
        }];
        assert_eq!(gpu_miner_hashrate(&status, &gpu), Some(100.0e12));

        status.miner.gpus[0].index = None;
        status.miner.gpus[0].uuid = None;
        assert_eq!(gpu_miner_hashrate(&status, &gpu), None);
    }

    #[test]
    fn hashrate_and_the_two_trend_metrics_keep_sparklines() {
        assert_eq!(
            SPARK_METRICS,
            [Metric::Hashrate, Metric::Power, Metric::Temp]
        );
    }

    /// Every `TROMMEL_*_SCREENSHOT` literal handled in this file must also be
    /// in [`SCREENSHOT_HOOKS`]. A capture that is missing there runs under the
    /// normal application id, so GApplication may hand the request to an
    /// already running Trommel window instead of rendering a private one.
    #[test]
    fn screenshot_hooks_cover_every_capture() {
        let source = include_str!("gui.rs");
        let mut found = BTreeSet::new();
        for token in
            source.split(|c: char| !(c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit()))
        {
            if token.starts_with("TROMMEL_") && token.ends_with("_SCREENSHOT") {
                found.insert(token);
            }
        }
        let declared: BTreeSet<&str> = SCREENSHOT_HOOKS.into_iter().collect();
        assert_eq!(
            found, declared,
            "add the screenshot hook to SCREENSHOT_HOOKS in run()"
        );
    }
}
