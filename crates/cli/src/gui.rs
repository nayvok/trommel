#![allow(deprecated)]
//! GTK4 + libadwaita UI.
//!
//! The GUI keeps a draft configuration; the daemon owns the applied one. All
//! privileged work (GPU tuning, miner process) happens in `trommeld`.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;
use gtk::{gdk, gio, glib};

use trommel_core::catalog::{self, MinerOption, OptionKind};
use trommel_core::config::{Appearance, Behavior, ChangeClass, Config, MinerInstall, Preset};
use trommel_core::dbus::MinerProxy;
use trommel_core::i18n::{self, tr};
use trommel_core::nvidia::GpuStats;
use trommel_core::status::Status;
use trommel_core::util;

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
    /// Apply a draft and start the miner in one step (quick setup).
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
    start_login: adw::SwitchRow,
    simple_mode: adw::SwitchRow,
    poll: adw::SpinRow,
    language: adw::ComboRow,
    appearance: adw::ComboRow,
    temp_limit: adw::SpinRow,
    temp_resume: adw::SpinRow,
    diagnostics: gtk::Button,
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
    action_button: gtk::Button,
    action_label: gtk::Label,
    quick_button: gtk::Button,
    hero_hashrate: gtk::Label,
    hero_line: gtk::Label,
    hero_meta: gtk::Label,
    pill: gtk::Label,
    total_power: adw::ActionRow,
    total_temp: adw::ActionRow,
    gpu_host: gtk::Box,
    gpu_sig: RefCell<String>,
    shares_ok: gtk::Label,
    shares_bad: gtk::Label,
    pool_balance_value: gtk::Label,
    pool_meta: adw::ActionRow,
    warnings: adw::ActionRow,
    warning_badge: gtk::Label,
    preset_group: adw::PreferencesGroup,
    preset_rows: RefCell<Vec<adw::ActionRow>>,
    settings: SettingsWidgets,
    miners_group: adw::PreferencesGroup,
    miner_rows: RefCell<Vec<adw::ActionRow>>,
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
        .trommel-gpu-title { font-weight: 700; }
        .trommel-gpu-stat {
            background-color: alpha(@card_fg_color, 0.06);
            border-radius: 10px;
            padding: 6px 9px;
        }
        .trommel-gpu-stat-value {
            font-weight: 700;
            font-size: 1.05em;
            font-feature-settings: "tnum";
        }
        .trommel-gpu-stat-label { font-size: 0.72em; opacity: 0.65; }
        .trommel-gpu-stat-hint { font-size: 0.7em; opacity: 0.5; }
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

fn set_page_icon(stack: &adw::ViewStack, child: &impl IsA<gtk::Widget>, icon: &str) {
    let page = stack.page(child);
    page.set_icon_name(Some(icon));
}

// ------------------------------------------------------------------- run

pub fn run() -> anyhow::Result<()> {
    // Both screenshot modes run a private instance so they can capture a window
    // even when a normal Trommel instance is open.
    let screenshot = std::env::var_os("TROMMEL_SCREENSHOT").is_some()
        || std::env::var_os("TROMMEL_WIZARD_SCREENSHOT").is_some()
        || std::env::var_os("TROMMEL_TUNE_SCREENSHOT").is_some()
        || std::env::var_os("TROMMEL_INSTALL_SCREENSHOT").is_some()
        || std::env::var_os("TROMMEL_PRESET_SCREENSHOT").is_some();
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
    let tx = spawn_worker(emit);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Trommel")
        .default_width(680)
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

    let title = adw::WindowTitle::new(
        "Trommel",
        &tr("NVIDIA mining rig manager", "Менеджер NVIDIA-рига"),
    );
    // One navigation surface, always available: the bottom switcher bar. The
    // header keeps the window title, so the switcher can never be duplicated
    // at the top and disappear at the bottom on narrow windows.
    let switcher_bar = adw::ViewSwitcherBar::builder().stack(&stack).build();
    switcher_bar.set_reveal(true);

    let header = adw::HeaderBar::new();
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

    let action_label = gtk::Label::new(Some(&tr("Start", "Запустить")));
    let action_button = gtk::Button::builder()
        .css_classes(["suggested-action", "pill"])
        .child(&action_label)
        .build();
    header.pack_end(&action_button);

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
    let hero_top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
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
    let quick_button = gtk::Button::builder()
        .label(tr("Quick setup", "Быстрая настройка"))
        .css_classes(["suggested-action", "pill"])
        .halign(gtk::Align::Start)
        .build();
    quick_button.set_visible(false);
    hero.append(&hero_top);
    hero.append(&hero_line);
    hero.append(&hero_rule);
    hero.append(&hero_meta);
    hero.append(&quick_button);
    hero_group.add(&hero);
    dashboard_box.append(&hero_group);

    let rig_group = adw::PreferencesGroup::builder()
        .title(tr("Rig", "Риг"))
        .description(tr("Per-GPU live state", "Живое состояние каждой карты"))
        .build();
    let gpu_host = gtk::Box::new(gtk::Orientation::Vertical, 0);
    gpu_host.set_hexpand(true);
    gpu_host.set_margin_top(6);
    gpu_host.set_margin_bottom(6);
    rig_group.add(&gpu_host);
    dashboard_box.append(&rig_group);

    let totals_group = adw::PreferencesGroup::builder()
        .title(tr("Totals", "Итого"))
        .build();
    let total_power = adw::ActionRow::builder()
        .title(tr("Power", "Мощность"))
        .subtitle("—")
        .build();
    let total_temp = adw::ActionRow::builder()
        .title(tr("Max temperature", "Макс. температура"))
        .subtitle("—")
        .build();
    totals_group.add(&total_power);
    totals_group.add(&total_temp);
    dashboard_box.append(&totals_group);

    let shares_group = adw::PreferencesGroup::builder()
        .title(tr("Shares and pool", "Шары и пул"))
        .build();
    let shares = adw::ActionRow::builder()
        .title(tr("Shares", "Шары"))
        .build();
    let shares_ok = label("0", &["caption", "success"]);
    let shares_bad = label("0", &["caption", "error"]);
    let shares_value = gtk::Box::new(gtk::Orientation::Horizontal, 4);
    shares_value.set_valign(gtk::Align::Center);
    shares_value.append(&shares_ok);
    shares_value.append(&label("/", &["caption", "dim-label"]));
    shares_value.append(&shares_bad);
    shares.add_suffix(&shares_value);
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

    let (miners_page, miners_group) = build_miners_page();
    stack.add_titled(&miners_page, Some("miners"), &tr("Miners", "Майнеры"));
    set_page_icon(&stack, &miners_page, "applications-engineering-symbolic");

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
        action_button: action_button.clone(),
        action_label: action_label.clone(),
        quick_button: quick_button.clone(),
        hero_hashrate,
        hero_line,
        hero_meta,
        pill,
        total_power,
        total_temp,
        gpu_host,
        gpu_sig: RefCell::new(String::new()),
        shares_ok,
        shares_bad,
        pool_balance_value,
        pool_meta,
        warnings,
        warning_badge,
        preset_group,
        preset_rows: RefCell::new(Vec::new()),
        settings,
        miners_group,
        miner_rows: RefCell::new(Vec::new()),
    });
    APP_SLOT.with(|slot| *slot.borrow_mut() = Some(app_state.clone()));

    install_actions(app, &app_state);
    connect_signals(&app_state);
    refresh_preset_list(&app_state);
    refresh_miners(&app_state);
    load_settings(&app_state);
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
    if std::env::var_os("TROMMEL_OPEN_WIZARD").is_some() {
        let app_state = app_state.clone();
        glib::timeout_add_seconds_local(3, move || {
            quick_setup_wizard(&app_state);
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
            "A preset bundles coin, pool, wallet, miner and GPU profile. Open a row to edit it.",
            "Пресет объединяет монету, пул, кошелёк, майнер и профиль GPU. Откройте строку для правки.",
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
    (page, list_group)
}

thread_local! {
    static NEW_PRESET_BUTTON: RefCell<Option<gtk::Button>> = const { RefCell::new(None) };
}

fn build_miners_page() -> (adw::PreferencesPage, adw::PreferencesGroup) {
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::builder()
        .title(tr("Installed and supported miners", "Установленные и поддерживаемые майнеры"))
        .description(tr(
            "Install once and reuse across all presets. Import a binary or open the upstream release page.",
            "Установите один раз и используйте во всех пресетах. Импортируйте бинарник или откройте страницу релиза.",
        ))
        .build();
    page.add(&group);

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
    BENCH_MINUTES.with(|slot| *slot.borrow_mut() = Some(minutes));
    BENCH_BUTTON.with(|slot| *slot.borrow_mut() = Some(run));
    (page, group)
}

thread_local! {
    static BENCH_MINUTES: RefCell<Option<adw::SpinRow>> = const { RefCell::new(None) };
    static BENCH_BUTTON: RefCell<Option<gtk::Button>> = const { RefCell::new(None) };
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
        &start_login,
        &simple_mode,
    ] {
        behavior.add(row);
    }
    behavior.add(&temp_limit);
    behavior.add(&temp_resume);
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
    actions.add(&stock_row);
    page.add(&actions);
    SAVE_SETTINGS_BUTTON.with(|slot| *slot.borrow_mut() = Some(save));
    STOCK_BUTTON.with(|slot| *slot.borrow_mut() = Some(stock));

    (
        page,
        SettingsWidgets {
            auto_restart,
            restore_stock,
            notifications,
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
            // A clean install has a preset but no wallet/miner. Starting it
            // would only produce a toast; the wizard explains both steps.
            if needs_setup(&app) {
                quick_setup_wizard(&app);
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
    {
        let app = app.clone();
        let button = app.quick_button.clone();
        button.connect_clicked(move |_| {
            quick_setup_wizard(&app);
        });
    }
    if let Some(button) = NEW_PRESET_BUTTON.with(|slot| slot.borrow().clone()) {
        let app = app.clone();
        button.connect_clicked(move |_| new_preset(&app));
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
    if let Some(button) = BENCH_BUTTON.with(|slot| slot.borrow().clone()) {
        let tx = app.tx.clone();
        button.connect_clicked(move |_| {
            let minutes = BENCH_MINUTES
                .with(|slot| slot.borrow().as_ref().map(|r| r.value() as u64))
                .unwrap_or(3);
            let _ = tx.send(Cmd::Bench { minutes });
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
    let window = adw::Window::builder()
        .title(tr("Error details", "Подробности ошибки"))
        .modal(true)
        .transient_for(&app.window)
        .default_width(560)
        .default_height(320)
        .build();
    let header = adw::HeaderBar::new();
    let copy = gtk::Button::with_label(&tr("Copy", "Копировать"));
    let close = gtk::Button::with_label(&tr("Close", "Закрыть"));
    header.pack_end(&copy);
    header.pack_end(&close);
    window.set_titlebar(Some(&header));
    {
        let window_for_key = window.clone();
        let key = gtk::EventControllerKey::new();
        key.connect_key_pressed(move |_, key, _, _| {
            if key == gdk::Key::Escape {
                window_for_key.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        window.add_controller(key);
    }
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
    window.set_content(Some(&scroll));
    {
        let window = window.clone();
        close.connect_clicked(move |_| window.close());
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
    window.present();
}

fn pool_names() -> Vec<String> {
    catalog::pools().iter().map(|p| p.name.clone()).collect()
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

fn pool_index(id: &str) -> u32 {
    catalog::pools()
        .iter()
        .position(|p| p.id == id)
        .unwrap_or(0) as u32
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

fn refresh_preset_list(app: &Rc<App>) {
    for row in app.preset_rows.borrow_mut().drain(..) {
        app.preset_group.remove(&row);
    }
    let cfg = app.config.borrow();
    for (id, preset) in &cfg.presets {
        let row = adw::ActionRow::builder()
            .title(&preset.name)
            .subtitle(format!(
                "{} · {} · {} {}",
                preset.coin, preset.pool, preset.miner, preset.miner_version
            ))
            .build();
        row.set_activatable(true);
        row.set_tooltip_text(Some(&tr(
            "Open the preset editor",
            "Открыть редактор пресета",
        )));
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
                {
                    let mut cfg = app_use.config.borrow_mut();
                    cfg.active_preset = Some(id_use.clone());
                }
                mark_dirty(&app_use);
                refresh_preset_list(&app_use);
                save_and_apply(&app_use);
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
    let miner_fee_pct = if def.fee_configurable {
        miner_fee.value()
    } else {
        def.fee_pct
    };
    info_fee.set_subtitle(&format!(
        "{}: {:.2}% · {}: {:.2}% · {}: <b>{:.2}%</b>",
        tr("miner", "майнер"),
        def.fee_pct,
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
    miner_fee.set_sensitive(def.fee_configurable);
}

/// Modal editor for one preset. The main window keeps the list; everything
/// else happens here, so there is no in-page "Back" navigation.
fn preset_editor_dialog(app: &Rc<App>, id: &str) {
    let Some(base) = app.config.borrow().presets.get(id).cloned() else {
        return;
    };
    let simple = app.config.borrow().behavior.simple_mode;

    let window = gtk::Window::builder()
        .title(tr("Preset", "Пресет"))
        .modal(true)
        .transient_for(&app.window)
        .default_width(600)
        .default_height(700)
        .build();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(
        &base.name,
        &tr("Preset", "Пресет"),
    )));
    let cancel = gtk::Button::with_label(&tr("Cancel", "Отмена"));
    header.pack_start(&cancel);
    let save = gtk::Button::builder()
        .label(tr("Save", "Сохранить"))
        .css_classes(["suggested-action", "pill"])
        .build();
    header.pack_end(&save);
    window.set_titlebar(Some(&header));

    let page = adw::PreferencesPage::new();

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
        .label(tr("Install…", "Установить…"))
        .css_classes(["flat", "pill"])
        .valign(gtk::Align::Center)
        .build();
    info_install.add_suffix(&install_button);
    miner_group.add(&info_desc);
    miner_group.add(&info_fee);
    miner_group.add(&info_install);
    page.add(&miner_group);

    let form_group = adw::PreferencesGroup::builder()
        .title(tr("Preset", "Пресет"))
        .build();
    let name = adw::EntryRow::builder()
        .title(tr("Name", "Название"))
        .text(&base.name)
        .build();
    let pool = adw::ComboRow::builder()
        .title(tr("Pool", "Пул"))
        .model(&string_list(&pool_names()))
        .selected(pool_index(&base.pool))
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
    let extra = adw::EntryRow::builder()
        .title(tr("Extra arguments", "Доп. аргументы"))
        .text(&base.extra_args)
        .build();
    form_group.add(&name);
    form_group.add(&pool);
    form_group.add(&pool_url);
    form_group.add(&miner);
    form_group.add(&version);
    form_group.add(&wallet);
    form_group.add(&worker);
    form_group.add(&pool_fee);
    form_group.add(&miner_fee);
    form_group.add(&extra);
    page.add(&form_group);

    let options_group = adw::PreferencesGroup::builder()
        .title(tr("Miner options", "Настройки майнера"))
        .description(tr(
            "Defaults come from the selected miner version.",
            "Значения по умолчанию берутся из выбранной версии майнера.",
        ))
        .build();
    let options_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    options_group.add(&options_box);
    page.add(&options_group);
    options_group.set_visible(!simple);
    extra.set_visible(!simple);
    miner_fee.set_visible(!simple);

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
    window.set_child(Some(&page));

    let options: Rc<RefCell<Vec<OptionRow>>> = Rc::new(RefCell::new(Vec::new()));
    preset_editor_versions(app, &miner, &version, &base.miner_version);
    rebuild_editor_options(app, id, &miner, &miner_fee, &options_box, &options);
    preset_editor_info(
        app,
        &miner,
        &version,
        &pool_fee,
        &miner_fee,
        &info_desc,
        &info_fee,
        &info_install,
        &install_state,
    );

    let refresh_info: Rc<dyn Fn()> = {
        let app = app.clone();
        let miner = miner.clone();
        let version = version.clone();
        let pool_fee = pool_fee.clone();
        let miner_fee = miner_fee.clone();
        let info_desc = info_desc.clone();
        let info_fee = info_fee.clone();
        let info_install = info_install.clone();
        let install_state = install_state.clone();
        Rc::new(move || {
            preset_editor_info(
                &app,
                &miner,
                &version,
                &pool_fee,
                &miner_fee,
                &info_desc,
                &info_fee,
                &info_install,
                &install_state,
            );
        })
    };

    {
        let app = app.clone();
        let id = id.to_string();
        let version_row = version.clone();
        let miner_fee = miner_fee.clone();
        let options_box = options_box.clone();
        let options = options.clone();
        let refresh_info = refresh_info.clone();
        miner.connect_notify_local(Some("selected"), move |row, _| {
            preset_editor_versions(&app, row, &version_row, "");
            rebuild_editor_options(&app, &id, row, &miner_fee, &options_box, &options);
            refresh_info();
        });
    }
    {
        let refresh_info = refresh_info.clone();
        version.connect_notify_local(Some("selected"), move |_, _| refresh_info());
    }
    {
        let pool_url = pool_url.clone();
        let pool_fee = pool_fee.clone();
        pool.connect_notify_local(Some("selected"), move |row, _| {
            if let Some(def) = catalog::pools().get(row.selected() as usize) {
                pool_url.set_text(&def.url);
                pool_fee.set_value(def.fee_pct);
            }
        });
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
        let refresh_info = refresh_info.clone();
        install_button.connect_clicked(move |_| {
            let Some(def) = catalog::miners().get(miner.selected() as usize) else {
                return;
            };
            let miner_id = def.id.clone();
            let selected = selected_version(&app, &miner, &version);
            let app_done = app.clone();
            let miner_done = miner.clone();
            let version_done = version.clone();
            let refresh_done = refresh_info.clone();
            let version_text = selected.clone();
            let on_done: Rc<dyn Fn()> = Rc::new(move || {
                preset_editor_versions(&app_done, &miner_done, &version_done, &version_text);
                refresh_done();
            });
            miner_install_dialog(&app, &miner_id, selected, Some(on_done));
        });
    }
    {
        let app = app.clone();
        let window = window.clone();
        let id = id.to_string();
        let name = name.clone();
        let pool = pool.clone();
        let pool_url = pool_url.clone();
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
            let pool_id = catalog::pools()
                .get(pool.selected() as usize)
                .map(|item| item.id.clone())
                .unwrap_or_else(|| "pearlhash".into());
            let mut preset_name = name.text().trim().to_string();
            if preset_name.is_empty() {
                preset_name = tr("Preset", "Пресет").to_string();
            }
            {
                let mut cfg = app.config.borrow_mut();
                if let Some(preset) = cfg.presets.get_mut(&id) {
                    preset.name = preset_name;
                    preset.pool = pool_id;
                    preset.pool_url = pool_url.text().to_string();
                    preset.pool_fee_pct = pool_fee.value();
                    preset.miner = miner_id;
                    preset.miner_version = version_text;
                    preset.wallet = wallet.text().to_string();
                    preset.worker = worker.text().to_string();
                    preset.extra_args = extra.text().to_string();
                    preset.miner_options = values;
                    preset.miner_fee_pct = def.fee_pct;
                    preset.miner_fee_override_pct = if def.fee_configurable {
                        Some(miner_fee.value())
                    } else {
                        None
                    };
                    preset.updated_at = util::now_unix();
                }
            }
            mark_dirty(&app);
            refresh_preset_list(&app);
            refresh_miners(&app);
            window.close();
            save_and_apply(&app);
        });
    }
    {
        let app = app.clone();
        let window = window.clone();
        let id = id.to_string();
        duplicate.connect_clicked(move |_| {
            if let Some(new_id) = duplicate_preset(&app, &id) {
                window.close();
                preset_editor_dialog(&app, &new_id);
            }
        });
    }
    {
        let app = app.clone();
        let window = window.clone();
        let id = id.to_string();
        delete.connect_clicked(move |_| delete_preset(&app, &window, &id));
    }
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    window.present();
    if let Some(path) = std::env::var_os("TROMMEL_PRESET_SCREENSHOT") {
        let path = path.to_string_lossy().into_owned();
        let capture = window.clone();
        let application = app.window.application();
        glib::timeout_add_seconds_local(2, move || {
            if let Some(application) = application.clone() {
                schedule_window_screenshot(capture.clone(), path.clone(), application, 0);
            }
            glib::ControlFlow::Break
        });
    }
}

fn new_preset(app: &Rc<App>) {
    let id = {
        let mut cfg = app.config.borrow_mut();
        let base = cfg.preset().cloned().unwrap_or_else(Preset::new_pearl);
        let now = util::now_unix();
        let mut id = format!("preset-{now}");
        let mut suffix = 1;
        while cfg.presets.contains_key(&id) {
            id = format!("preset-{now}-{suffix}");
            suffix += 1;
        }
        let mut preset = base;
        preset.id = id.clone();
        preset.name = tr("New preset", "Новый пресет");
        preset.created_at = util::now_unix();
        preset.updated_at = preset.created_at;
        cfg.presets.insert(id.clone(), preset);
        cfg.active_preset = Some(id.clone());
        id
    };
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

fn delete_preset(app: &Rc<App>, window: &gtk::Window, id: &str) {
    let Some(preset) = app.config.borrow().presets.get(id).cloned() else {
        return;
    };
    if app.config.borrow().presets.len() <= 1 {
        app.toast.add_toast(adw::Toast::new(&tr(
            "At least one preset is required.",
            "Нужен хотя бы один пресет.",
        )));
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
    let window = window.clone();
    let parent = window.clone();
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
        window.close();
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

fn refresh_miners(app: &Rc<App>) {
    for row in app.miner_rows.borrow_mut().drain(..) {
        app.miners_group.remove(&row);
    }
    let cfg = app.config.borrow();
    for miner in catalog::miners() {
        let installed = catalog::installed_versions(&cfg, &miner.id);
        let row = adw::ActionRow::builder()
            .title(&miner.name)
            .subtitle(miner.short_description())
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
        if !installed.is_empty() {
            let versions = label(&installed.join(" · "), &["caption", "success"]);
            versions.set_valign(gtk::Align::Center);
            row.add_suffix(&versions);
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
        let miner_id = miner.id.clone();
        let version = miner
            .latest_version()
            .map(|item| item.version.clone())
            .unwrap_or_default();
        {
            let app_chev = app.clone();
            let miner_chev = miner.id.clone();
            let version_chev = version.clone();
            chevron.connect_clicked(move |_| {
                miner_install_dialog(&app_chev, &miner_chev, version_chev.clone(), None);
            });
        }
        row.add_suffix(&chevron);
        row.connect_activated(move |_| {
            miner_install_dialog(&app_row, &miner_id, version.clone(), None);
        });
        app.miners_group.add(&row);
        app.miner_rows.borrow_mut().push(row);
    }
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
    let uuid = gpu.uuid.clone();
    let name = gpu.name.clone();
    let default_power = gpu.power_default_w.or(gpu.power_limit_w);
    let profile = app
        .config
        .borrow()
        .gpu_profile()
        .cloned()
        .unwrap_or_default();
    let tuning = profile.tuning_for(&uuid);
    let custom = profile.is_custom(&uuid);
    let enabled = tuning.enabled;
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

    let window = gtk::Window::builder()
        .title(format!("{name} · {}", tr("GPU tuning", "Настройка GPU")))
        .modal(true)
        .transient_for(&app.window)
        .default_width(540)
        .build();
    let header = adw::HeaderBar::new();
    let title = label(&format!("GPU #{} · {name}", gpu.index), &["heading"]);
    title.set_ellipsize(gtk::pango::EllipsizeMode::End);
    header.set_title_widget(Some(&title));
    let presets_popover = gtk::Popover::new();
    let presets_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
    presets_box.set_margin_top(6);
    presets_box.set_margin_bottom(6);
    presets_box.set_margin_start(6);
    presets_box.set_margin_end(6);
    let recommended = gtk::Button::builder()
        .label(tr("Recommended", "Рекомендуемый"))
        .tooltip_text(tr(
            "80 % of the stock power limit; clock locks are turned off",
            "80 % от стокового лимита питания; локи частот выключаются",
        ))
        .css_classes(["flat"])
        .halign(gtk::Align::Fill)
        .build();
    let profile_default = gtk::Button::builder()
        .label(tr("Profile default", "Профиль по умолчанию"))
        .tooltip_text(tr(
            "Remove this card's override; it will follow the built-in profile (auto power, no clock locks)",
            "Убрать настройку этой карты; останутся базовые значения профиля (авто-питание, без локов частот)",
        ))
        .css_classes(["flat"])
        .halign(gtk::Align::Fill)
        .visible(custom)
        .build();
    presets_box.append(&recommended);
    presets_box.append(&profile_default);
    presets_popover.set_child(Some(&presets_box));
    {
        let popover = presets_popover.clone();
        recommended.connect_clicked(move |_| popover.popdown());
    }
    let presets = gtk::MenuButton::builder()
        .icon_name("view-more-symbolic")
        .tooltip_text(tr("Tuning presets", "Пресеты настройки"))
        .css_classes(["flat"])
        .popover(&presets_popover)
        .build();
    let save = gtk::Button::builder()
        .label(tr("Save", "Сохранить"))
        .css_classes(["suggested-action", "pill"])
        .build();
    header.pack_end(&save);
    header.pack_end(&presets);
    window.set_titlebar(Some(&header));
    let page = adw::PreferencesPage::new();

    let group = adw::PreferencesGroup::builder()
        .title(format!("GPU #{} · {name}", gpu.index))
        .description(uuid.clone())
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

    let source_hint = label(
        &if custom {
            tr(
                "These values are this card's own override.",
                "Эти значения — собственная настройка этой карты.",
            )
        } else {
            tr(
                "These values come from the default profile. Saving creates a per-card override.",
                "Эти значения взяты из профиля по умолчанию. Сохранение создаст настройку для этой карты.",
            )
        },
        &["caption", "dim-label"],
    );
    source_hint.set_margin_start(6);
    source_hint.set_margin_top(6);
    source_hint.set_wrap(true);
    group.add(&source_hint);
    let global_hint = label(
        &tr(
            "Temperature limit is global (Settings → Behavior); fan speed is a miner option in the preset editor.",
            "Порог температуры — глобальный (Настройки → Поведение); вентилятор — опция майнера в редакторе пресета.",
        ),
        &["caption", "dim-label"],
    );
    global_hint.set_margin_start(6);
    global_hint.set_margin_top(4);
    global_hint.set_wrap(true);
    group.add(&global_hint);
    let base_tuning = tuning.clone();
    page.add(&group);

    // "Apply to" only makes sense with more than one card. On a single-GPU
    // machine the old group contained nothing but a sentence, which looked
    // like a broken, half-cut section at the bottom of the dialog.
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
                "Each card has its own settings. Use the switches to copy this profile.",
                "У каждой карты свои настройки. Переключатели копируют этот профиль.",
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
    window.set_child(Some(&page));

    {
        let power_row = power.clone();
        let core_row = core.clone();
        let memory_row = memory.clone();
        let core_lock_row = core_lock.clone();
        let memory_lock_row = memory_lock.clone();
        let base = default_power.unwrap_or(300.0);
        recommended.connect_clicked(move |_| {
            power_row.set_value((base * 0.8).max(50.0).round());
            core_lock_row.set_active(false);
            memory_lock_row.set_active(false);
            core_row.set_value(1620.0);
            memory_row.set_value(810.0);
        });
    }
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
    }
    {
        let app = app.clone();
        let window = window.clone();
        let uuid = uuid.clone();
        profile_default.connect_clicked(move |_| {
            {
                let mut cfg = app.config.borrow_mut();
                let profile = cfg.gpu_profile_mut();
                profile.per_gpu.remove(&uuid);
            }
            mark_dirty(&app);
            window.close();
            app.toast.add_toast(adw::Toast::new(&tr(
                "Card follows the profile default again",
                "Карта снова использует профиль по умолчанию",
            )));
            save_and_apply(&app);
        });
    }
    {
        let app = app.clone();
        let window = window.clone();
        let uuid = uuid.clone();
        let same_uuids = same_uuids.clone();
        let all_uuids = all_uuids.clone();
        let base_tuning = base_tuning.clone();
        save.connect_clicked(move |_| {
            let core_value = core.value() as u32;
            let memory_value = memory.value() as u32;
            // Keep the fields this dialog does not own (fan/temperature are
            // global and are edited elsewhere).
            let mut tuning = base_tuning.clone();
            tuning.enabled = enabled;
            tuning.custom = true;
            tuning.power_limit_w = nonzero(power.value() as u32);
            tuning.core_clock_mhz = if core_lock.is_active() && core_value >= 100 {
                Some(core_value)
            } else {
                None
            };
            tuning.memory_clock_mhz = if memory_lock.is_active() && memory_value >= 100 {
                Some(memory_value)
            } else {
                None
            };
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
            {
                let mut cfg = app.config.borrow_mut();
                let profile = cfg.gpu_profile_mut();
                for target in &targets {
                    profile.per_gpu.insert(target.clone(), tuning.clone());
                }
            }
            mark_dirty(&app);
            window.close();
            app.toast.add_toast(adw::Toast::new(&format!(
                "{}: {}",
                tr(
                    "GPU tuning saved; applying…",
                    "Настройка GPU сохранена; применяю…"
                ),
                targets.len()
            )));
            save_and_apply(&app);
        });
    }
    window.present();
    if let Some(path) = std::env::var_os("TROMMEL_TUNE_SCREENSHOT") {
        let path = path.to_string_lossy().into_owned();
        let capture = window.clone();
        let application = app.window.application();
        glib::timeout_add_seconds_local(2, move || {
            if let Some(application) = application.clone() {
                schedule_window_screenshot(capture.clone(), path.clone(), application, 0);
            }
            glib::ControlFlow::Break
        });
    }
}

// ------------------------------------------------------------ quick setup

/// One card row of the quick-setup wizard: the enable switch, its power limit
/// and the stock limit used by the "80%" helper.
struct SetupCard {
    uuid: String,
    row: adw::ExpanderRow,
    power: adw::SpinRow,
    stock_w: Option<f64>,
}

/// Subtitle of a wizard card row: stock limit plus the currently chosen limit,
/// so filling the 80 % value is visible without expanding the row.
fn setup_card_subtitle(stock: Option<f64>, limit_w: u32) -> String {
    let limit = if limit_w == 0 {
        tr("auto", "авто")
    } else {
        format!("{limit_w} W")
    };
    match stock {
        Some(watts) => format!(
            "{}: {:.0} W · {}: {}",
            tr("stock", "сток"),
            watts,
            tr("limit", "лимит"),
            limit
        ),
        None => format!("{}: {}", tr("limit", "лимит"), limit),
    }
}

fn string_list(values: &[String]) -> gtk::StringList {
    let refs: Vec<&str> = values.iter().map(String::as_str).collect();
    gtk::StringList::new(&refs)
}

fn setup_miner_versions(app: &Rc<App>, miner_row: &adw::ComboRow, version_row: &adw::ComboRow) {
    let previous = version_row.selected() as usize;
    let Some(def) = catalog::miners().get(miner_row.selected() as usize) else {
        version_row.set_model(Some(&string_list(&[])));
        return;
    };
    let installed = catalog::installed_versions(&app.config.borrow(), &def.id);
    let labels: Vec<String> = miner_version_list(&app.config.borrow(), def)
        .into_iter()
        .map(|version| {
            if installed.contains(&version) {
                format!("{version} ✓")
            } else {
                version
            }
        })
        .collect();
    version_row.set_model(Some(&string_list(&labels)));
    if !labels.is_empty() {
        version_row.set_selected(previous.min(labels.len() - 1) as u32);
    }
}

fn setup_miner_state(
    app: &Rc<App>,
    miner_row: &adw::ComboRow,
    version_row: &adw::ComboRow,
    install_row: &adw::ActionRow,
    fee_row: &adw::ActionRow,
) {
    let Some(def) = catalog::miners().get(miner_row.selected() as usize) else {
        install_row.set_subtitle(&tr("Choose a miner", "Выберите майнера"));
        fee_row.set_subtitle("—");
        return;
    };
    let version = selected_version(app, miner_row, version_row);
    let installed = app.config.borrow().install(&def.id, &version).is_some();
    let state = if installed {
        tr("Installed", "Установлен")
    } else {
        tr("Not installed", "Не установлен")
    };
    install_row.set_subtitle(&format!("{state} · {} {version}", def.id));
    install_row.set_subtitle_lines(2);
    fee_row.set_subtitle(&format!(
        "{:.2}% · {}",
        def.fee_pct,
        if def.fee_configurable {
            tr("editable per preset", "меняется в пресете")
        } else {
            tr("fixed by the miner", "фиксирована майнером")
        }
    ));
}

fn show_setup_step(
    step: u32,
    stack: &gtk::Stack,
    step_label: &gtk::Label,
    back: &gtk::Button,
    next: &gtk::Button,
    status: &gtk::Label,
) {
    let pages = ["cards", "miner", "wallet"];
    let titles = [
        tr("Step 1 of 3 · Cards", "Шаг 1 из 3 · Карты"),
        tr("Step 2 of 3 · Miner", "Шаг 2 из 3 · Майнер"),
        tr("Step 3 of 3 · Wallet", "Шаг 3 из 3 · Кошелёк"),
    ];
    let index = (step as usize).min(pages.len() - 1);
    stack.set_visible_child_name(pages[index]);
    step_label.set_text(&titles[index]);
    let back_text = if index == 0 {
        tr("Cancel", "Отмена")
    } else {
        tr("Back", "Назад")
    };
    back.set_label(&back_text);
    let next_text = if index == pages.len() - 1 {
        tr("Start mining", "Запустить майнинг")
    } else {
        tr("Next", "Далее")
    };
    next.set_label(&next_text);
    set_status_text(status, "", "");
}

/// Three-step wizard for a first start: cards → miner → wallet → Start.
fn quick_setup_wizard(app: &Rc<App>) {
    let gpus: Vec<GpuStats> = app
        .status
        .borrow()
        .as_ref()
        .map(|status| status.gpus.clone())
        .unwrap_or_default();
    let (preset, profile) = {
        let cfg = app.config.borrow();
        (
            cfg.preset().cloned().unwrap_or_else(Preset::new_pearl),
            cfg.gpu_profile().cloned().unwrap_or_default(),
        )
    };

    let mut builder = gtk::Window::builder()
        .title(tr("Quick setup", "Быстрая настройка"))
        .modal(true)
        .transient_for(&app.window)
        .default_width(620)
        .default_height(680);
    if let Some(application) = app.window.application() {
        builder = builder.application(&application);
    }
    let window = builder.build();
    let header = adw::HeaderBar::new();
    let step_label = label(&tr("Step 1 of 3", "Шаг 1 из 3"), &["caption", "dim-label"]);
    step_label.set_halign(gtk::Align::Center);
    let back = gtk::Button::builder()
        .label(tr("Cancel", "Отмена"))
        .css_classes(["flat", "pill"])
        .build();
    let next = gtk::Button::builder()
        .label(tr("Next", "Далее"))
        .css_classes(["suggested-action", "pill"])
        .build();
    header.set_title_widget(Some(&step_label));
    header.pack_start(&back);
    header.pack_end(&next);
    window.set_titlebar(Some(&header));

    let stack = gtk::Stack::new();
    stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);
    stack.set_transition_duration(180);
    stack.set_vexpand(true);

    // Step 1 — cards and power limits.
    let cards: Rc<RefCell<Vec<SetupCard>>> = Rc::new(RefCell::new(Vec::new()));
    let cards_page = adw::PreferencesPage::new();
    let cards_group = adw::PreferencesGroup::builder()
        .title(tr("Cards to mine on", "Карты для майнинга"))
        .description(tr(
            "Each card keeps its own power limit. Untick a card to leave it untouched.",
            "У каждой карты свой лимит питания. Снимите галочку, чтобы не трогать карту.",
        ))
        .build();
    for gpu in &gpus {
        // The wizard writes per-card overrides, so it must start from the
        // per-card value (or "auto"), not from a default profile that may have
        // been tuned for another model.
        let explicit = profile.per_gpu.get(&gpu.uuid).cloned();
        let stock = gpu.power_default_w.or(gpu.power_limit_w);
        let limit = explicit
            .as_ref()
            .and_then(|tuning| tuning.power_limit_w)
            .unwrap_or(0);
        let enabled = explicit
            .as_ref()
            .map(|tuning| tuning.enabled)
            .unwrap_or(true);
        let row = adw::ExpanderRow::builder()
            .title(format!("GPU #{} · {}", gpu.index, gpu.name))
            .subtitle(setup_card_subtitle(stock, limit))
            .build();
        row.set_show_enable_switch(true);
        row.set_enable_expansion(enabled);
        row.set_expanded(false);
        let power = spin_row(
            &tr("Power limit", "Лимит питания"),
            &tr("W — 0 = driver auto", "Вт — 0 = авто драйвера"),
            limit as f64,
            0.0,
            1000.0,
        );
        row.add_row(&power);
        {
            let row = row.clone();
            power.connect_notify_local(Some("value"), move |spin, _| {
                row.set_subtitle(&setup_card_subtitle(stock, spin.value() as u32));
            });
        }
        cards_group.add(&row);
        cards.borrow_mut().push(SetupCard {
            uuid: gpu.uuid.clone(),
            row,
            power,
            stock_w: stock,
        });
    }
    if gpus.is_empty() {
        cards_group.add(
            &adw::ActionRow::builder()
                .title(tr("No GPU detected", "Карты не обнаружены"))
                .subtitle(tr(
                    "The service is offline or nvidia-smi failed. The preset is still saved.",
                    "Сервис недоступен или nvidia-smi не отвечает. Пресет всё равно сохранится.",
                ))
                .build(),
        );
    }
    cards_page.add(&cards_group);

    let safe_group = adw::PreferencesGroup::builder()
        .title(tr("Safe start", "Безопасный старт"))
        .description(tr(
            "Recommended is 80% of the stock power limit with clock locks off. It is a safe start, not a promise of maximum hashrate.",
            "«Рекомендуемый» — 80% стокового лимита питания без локов частот. Это безопасный старт, а не обещание максимального хешрейта.",
        ))
        .build();
    let fill_row = adw::ActionRow::builder()
        .title(tr("80% of stock power", "80% стокового питания"))
        .subtitle(tr(
            "Fills the limit row of every ticked card",
            "Заполняет лимит каждой отмеченной карты",
        ))
        .build();
    let fill = gtk::Button::builder()
        .label(tr("Apply 80 %", "Поставить 80 %"))
        .css_classes(["flat", "pill"])
        .valign(gtk::Align::Center)
        .build();
    fill_row.add_suffix(&fill);
    safe_group.add(&fill_row);
    let auto_row = adw::ActionRow::builder()
        .title(tr("Driver auto", "Авто драйвера"))
        .subtitle(tr(
            "No power limit; the driver decides",
            "Без лимита питания; решает драйвер",
        ))
        .build();
    let clear = gtk::Button::builder()
        .label(tr("Use auto", "Авто"))
        .css_classes(["flat", "pill"])
        .valign(gtk::Align::Center)
        .build();
    auto_row.add_suffix(&clear);
    safe_group.add(&auto_row);
    cards_page.add(&safe_group);
    stack.add_named(&cards_page, Some("cards"));
    {
        let cards = cards.clone();
        let fill_row = fill_row.clone();
        fill.connect_clicked(move |_| {
            let mut applied = Vec::new();
            for card in cards.borrow().iter() {
                if !card.row.enables_expansion() {
                    continue;
                }
                let Some(stock) = card.stock_w else {
                    continue;
                };
                let watts = (stock * 0.8).round().max(50.0);
                card.power.set_value(watts);
                card.row.set_expanded(true);
                applied.push(format!("{watts:.0} W"));
            }
            if applied.is_empty() {
                fill_row.set_subtitle(&tr(
                    "Tick a card with a known stock limit first.",
                    "Сначала отметьте карту с известным стоковым лимитом.",
                ));
            } else {
                fill_row.set_subtitle(&format!(
                    "{}: {}",
                    tr("Applied", "Применено"),
                    applied.join(" · ")
                ));
            }
        });
    }
    {
        let cards = cards.clone();
        clear.connect_clicked(move |_| {
            for card in cards.borrow().iter() {
                card.power.set_value(0.0);
            }
        });
    }

    // Step 2 — miner and version.
    let miner_page = adw::PreferencesPage::new();
    let miner_group = adw::PreferencesGroup::builder()
        .title(tr("Miner", "Майнер"))
        .description(tr(
            "Install once; every preset can reuse it. Only one miner runs at a time.",
            "Установите один раз; все пресеты могут его использовать. Одновременно работает один майнер.",
        ))
        .build();
    let miner_row = adw::ComboRow::builder()
        .title(tr("Miner", "Майнер"))
        .model(&string_list(&miner_names()))
        .selected(miner_index(&preset.miner))
        .build();
    let version_row = adw::ComboRow::builder()
        .title(tr("Version", "Версия"))
        .build();
    let fee_row = adw::ActionRow::builder()
        .title(tr("Fee", "Комиссия"))
        .build();
    let install_row = adw::ActionRow::builder()
        .title(tr("Installation", "Установка"))
        .build();
    let install_button = gtk::Button::builder()
        .label(tr("Install…", "Установить…"))
        .css_classes(["flat", "pill"])
        .valign(gtk::Align::Center)
        .build();
    install_row.add_suffix(&install_button);
    miner_group.add(&miner_row);
    miner_group.add(&version_row);
    miner_group.add(&fee_row);
    miner_group.add(&install_row);
    miner_page.add(&miner_group);
    stack.add_named(&miner_page, Some("miner"));
    setup_miner_versions(app, &miner_row, &version_row);
    if let Some(def) = catalog::miner(&preset.miner) {
        let versions = miner_version_list(&app.config.borrow(), def);
        if let Some(index) = versions.iter().position(|v| *v == preset.miner_version) {
            version_row.set_selected(index as u32);
        }
    }
    setup_miner_state(app, &miner_row, &version_row, &install_row, &fee_row);
    {
        let app = app.clone();
        let miner_row = miner_row.clone();
        let version_row = version_row.clone();
        let install_row = install_row.clone();
        let fee_row = fee_row.clone();
        miner_row.connect_notify_local(Some("selected"), move |row, _| {
            setup_miner_versions(&app, row, &version_row);
            setup_miner_state(&app, row, &version_row, &install_row, &fee_row);
        });
    }
    {
        let app = app.clone();
        let miner_row = miner_row.clone();
        let install_row = install_row.clone();
        let fee_row = fee_row.clone();
        version_row.connect_notify_local(Some("selected"), move |row, _| {
            setup_miner_state(&app, &miner_row, row, &install_row, &fee_row);
        });
    }
    {
        let app = app.clone();
        let miner_row = miner_row.clone();
        let version_row = version_row.clone();
        let install_row = install_row.clone();
        let fee_row = fee_row.clone();
        install_button.connect_clicked(move |_| {
            let id = catalog::miners()
                .get(miner_row.selected() as usize)
                .map(|def| def.id.clone())
                .unwrap_or_else(|| "wildrig".into());
            let version = selected_version(&app, &miner_row, &version_row);
            let app_done = app.clone();
            let miner_done = miner_row.clone();
            let version_done = version_row.clone();
            let install_done = install_row.clone();
            let fee_done = fee_row.clone();
            let on_done: Rc<dyn Fn()> = Rc::new(move || {
                setup_miner_state(
                    &app_done,
                    &miner_done,
                    &version_done,
                    &install_done,
                    &fee_done,
                );
            });
            miner_install_dialog(&app, &id, version, Some(on_done));
        });
    }

    // Step 3 — wallet and pool.
    let wallet_page = adw::PreferencesPage::new();
    let form_group = adw::PreferencesGroup::builder()
        .title(tr("Wallet and pool", "Кошелёк и пул"))
        .description(tr(
            "Nothing reaches the daemon until you press Start mining.",
            "Ничего не передаётся сервису, пока вы не нажмёте «Запустить майнинг».",
        ))
        .build();
    let name_row = adw::EntryRow::builder()
        .title(tr("Preset name", "Название пресета"))
        .text(&preset.name)
        .build();
    let wallet_row = adw::EntryRow::builder()
        .title(tr("Wallet", "Кошелёк"))
        .text(&preset.wallet)
        .build();
    let worker_row = adw::EntryRow::builder()
        .title(tr("Worker", "Воркер"))
        .text(&preset.worker)
        .build();
    let pool_row = adw::ComboRow::builder()
        .title(tr("Pool", "Пул"))
        .model(&string_list(&pool_names()))
        .selected(pool_index(&preset.pool))
        .build();
    let pool_url_row = adw::EntryRow::builder()
        .title(tr("Pool address", "Адрес пула"))
        .text(&preset.pool_url)
        .build();
    form_group.add(&name_row);
    form_group.add(&wallet_row);
    form_group.add(&worker_row);
    form_group.add(&pool_row);
    form_group.add(&pool_url_row);
    wallet_page.add(&form_group);
    let summary_group = adw::PreferencesGroup::new();
    summary_group.add(
        &adw::ActionRow::builder()
            .title(tr("On Start", "По кнопке «Запустить»"))
            .subtitle(tr(
                "Trommel saves the preset, applies the power limits and starts the miner.",
                "Trommel сохранит пресет, применит лимиты питания и запустит майнер.",
            ))
            .build(),
    );
    wallet_page.add(&summary_group);
    stack.add_named(&wallet_page, Some("wallet"));
    {
        let pool_url_row = pool_url_row.clone();
        pool_row.connect_notify_local(Some("selected"), move |row, _| {
            if let Some(pool) = catalog::pools().get(row.selected() as usize) {
                pool_url_row.set_text(&pool.url);
            }
        });
    }

    let status = label("", &["caption", "dim-label"]);
    status.set_margin_top(8);
    status.set_margin_bottom(10);
    status.set_margin_start(12);
    status.set_margin_end(12);
    status.set_wrap(true);
    status.set_halign(gtk::Align::Start);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&stack);
    content.append(&status);
    window.set_child(Some(&content));

    let step = Rc::new(Cell::new(0u32));
    {
        let window = window.clone();
        let step = step.clone();
        let stack = stack.clone();
        let step_label = step_label.clone();
        let back_button = back.clone();
        let next_button = next.clone();
        let status = status.clone();
        back.connect_clicked(move |_| {
            if step.get() == 0 {
                window.close();
                return;
            }
            step.set(step.get() - 1);
            show_setup_step(
                step.get(),
                &stack,
                &step_label,
                &back_button,
                &next_button,
                &status,
            );
        });
    }
    {
        let app = app.clone();
        let window = window.clone();
        let step = step.clone();
        let stack = stack.clone();
        let step_label = step_label.clone();
        let back_button = back.clone();
        let next_button = next.clone();
        let status = status.clone();
        let cards = cards.clone();
        let miner_row = miner_row.clone();
        let version_row = version_row.clone();
        let install_row = install_row.clone();
        let fee_row = fee_row.clone();
        let name_row = name_row.clone();
        let wallet_row = wallet_row.clone();
        let worker_row = worker_row.clone();
        let pool_row = pool_row.clone();
        let pool_url_row = pool_url_row.clone();
        next.connect_clicked(move |_| match step.get() {
            0 => {
                let picked = cards
                    .borrow()
                    .iter()
                    .any(|card| card.row.enables_expansion());
                if !picked && !cards.borrow().is_empty() {
                    set_status_text(
                        &status,
                        &tr(
                            "Tick at least one card, or leave the wizard and tune cards later.",
                            "Отметьте хотя бы одну карту или настройте карты позже.",
                        ),
                        "warning",
                    );
                    return;
                }
                step.set(1);
                show_setup_step(1, &stack, &step_label, &back_button, &next_button, &status);
            }
            1 => {
                setup_miner_state(&app, &miner_row, &version_row, &install_row, &fee_row);
                step.set(2);
                show_setup_step(2, &stack, &step_label, &back_button, &next_button, &status);
            }
            _ => {
                finish_quick_setup(
                    &app,
                    &window,
                    &cards,
                    &miner_row,
                    &version_row,
                    &name_row,
                    &wallet_row,
                    &worker_row,
                    &pool_row,
                    &pool_url_row,
                    &status,
                );
            }
        });
    }

    // Documentation screenshots can open a later step directly.
    if let Some(initial) = std::env::var("TROMMEL_WIZARD_STEP")
        .ok()
        .and_then(|value| value.parse::<u32>().ok())
    {
        let initial = initial.min(2);
        if initial > 0 {
            step.set(initial);
            show_setup_step(initial, &stack, &step_label, &back, &next, &status);
        }
    }
    window.present();
    if let Some(path) = std::env::var_os("TROMMEL_WIZARD_SCREENSHOT") {
        let path = path.to_string_lossy().into_owned();
        let capture = window.clone();
        let application = app.window.application();
        glib::timeout_add_seconds_local(3, move || {
            if let Some(application) = application.clone() {
                schedule_window_screenshot(capture.clone(), path.clone(), application, 0);
            }
            glib::ControlFlow::Break
        });
    }
}

/// Commit the wizard draft, then start the miner (or offer apply when running).
#[allow(clippy::too_many_arguments)]
fn finish_quick_setup(
    app: &Rc<App>,
    window: &gtk::Window,
    cards: &Rc<RefCell<Vec<SetupCard>>>,
    miner_row: &adw::ComboRow,
    version_row: &adw::ComboRow,
    name_row: &adw::EntryRow,
    wallet_row: &adw::EntryRow,
    worker_row: &adw::EntryRow,
    pool_row: &adw::ComboRow,
    pool_url_row: &adw::EntryRow,
    status: &gtk::Label,
) -> bool {
    let Some(def) = catalog::miners()
        .get(miner_row.selected() as usize)
        .cloned()
    else {
        set_status_text(
            status,
            &tr("Choose a miner.", "Выберите майнера."),
            "warning",
        );
        return false;
    };
    let version = selected_version(app, miner_row, version_row);
    if app.config.borrow().install(&def.id, &version).is_none() {
        set_status_text(
            status,
            &tr(
                "Install the selected miner version first — use Install.",
                "Сначала установите выбранную версию майнера — кнопка «Установить».",
            ),
            "warning",
        );
        return false;
    }
    let wallet = wallet_row.text().trim().to_string();
    if wallet.is_empty() {
        set_status_text(
            status,
            &tr(
                "Enter the wallet address — mining cannot start without it.",
                "Введите адрес кошелька — без него майнинг не запустится.",
            ),
            "warning",
        );
        return false;
    }
    let pool_url = pool_url_row.text().trim().to_string();
    if !pool_url.contains(':') {
        set_status_text(
            status,
            &tr(
                "The pool address needs a port, for example pool.pearlhash.xyz:9000.",
                "В адресе пула нужен порт, например pool.pearlhash.xyz:9000.",
            ),
            "warning",
        );
        return false;
    }
    let pool_id = catalog::pools()
        .get(pool_row.selected() as usize)
        .map(|pool| pool.id.clone())
        .unwrap_or_else(|| "pearlhash".into());
    let running = app
        .status
        .borrow()
        .as_ref()
        .map(|status| status.state.running)
        .unwrap_or(false);
    let mut preset_name = name_row.text().trim().to_string();
    if preset_name.is_empty() {
        preset_name = tr("Quick setup", "Быстрая настройка").to_string();
    }
    {
        let mut cfg = app.config.borrow_mut();
        let id = cfg
            .active_preset
            .clone()
            .unwrap_or_else(|| trommel_core::config::DEFAULT_PRESET_ID.to_string());
        let mut preset = cfg
            .presets
            .get(&id)
            .cloned()
            .unwrap_or_else(Preset::new_pearl);
        preset.id = id.clone();
        preset.name = preset_name;
        preset.wallet = wallet;
        preset.worker = worker_row.text().to_string();
        preset.pool = pool_id.clone();
        preset.pool_url = pool_url;
        preset.pool_fee_pct = catalog::pool_fee_pct(&pool_id);
        if preset.miner != def.id {
            preset.miner_options = def.default_options();
        }
        preset.miner = def.id.clone();
        preset.miner_version = version;
        preset.miner_fee_pct = def.fee_pct;
        preset.miner_fee_override_pct = None;
        preset.updated_at = util::now_unix();
        cfg.presets.insert(id.clone(), preset);
        cfg.active_preset = Some(id);
        {
            let profile = cfg.gpu_profile_mut();
            let default_power = profile.default.power_limit_w;
            let default_enabled = profile.default.enabled;
            for card in cards.borrow().iter() {
                let was_custom = profile.is_custom(&card.uuid);
                let mut tuning = profile.tuning_for(&card.uuid);
                tuning.enabled = card.row.enables_expansion();
                tuning.power_limit_w = nonzero(card.power.value() as u32);
                // Only an actual difference from the default is an override;
                // otherwise the card keeps following the profile defaults.
                tuning.custom = was_custom
                    || tuning.power_limit_w != default_power
                    || tuning.enabled != default_enabled;
                profile.per_gpu.insert(card.uuid.clone(), tuning);
            }
        }
    }
    mark_dirty(app);
    refresh_preset_list(app);
    window.close();
    app.stack.set_visible_child_name("dashboard");
    if running {
        app.toast.add_toast(adw::Toast::new(&tr(
            "Setup saved; apply it to the running miner.",
            "Настройка сохранена; примените её к запущенному майнеру.",
        )));
        save_and_apply(app);
    } else {
        let json = serde_json::to_string(&*app.config.borrow()).unwrap_or_default();
        let _ = app.tx.send(Cmd::ApplyStart { json });
        app.toast.add_toast(adw::Toast::new(&tr(
            "Starting the miner…",
            "Запускаю майнер…",
        )));
    }
    true
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

/// The one place that installs a miner. The Miners page, the preset editor and
/// the quick-setup wizard all open this dialog, so version handling, download,
/// import and the installed state live here exactly once.
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

    let window = gtk::Window::builder()
        .title(tr("Install miner", "Установка майнера"))
        .modal(true)
        .transient_for(&app.window)
        .default_width(580)
        .build();
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&adw::WindowTitle::new(
        &def.name,
        &tr("Install miner", "Установка майнера"),
    )));
    let cancel = gtk::Button::with_label(&tr("Cancel", "Отмена"));
    header.pack_start(&cancel);
    if !def.source_url.is_empty() {
        let release = gtk::Button::builder()
            .icon_name("web-browser-symbolic")
            .tooltip_text(tr("Release page", "Страница релиза"))
            .css_classes(["flat"])
            .build();
        let source_url = def.source_url.clone();
        release.connect_clicked(move |_| {
            let _ =
                gio::AppInfo::launch_default_for_uri(&source_url, None::<&gio::AppLaunchContext>);
        });
        header.pack_end(&release);
    }
    window.set_titlebar(Some(&header));

    let page = adw::PreferencesPage::new();

    let version_group = adw::PreferencesGroup::builder()
        .title(tr("Version", "Версия"))
        .description(tr(
            "Miners are updated often. Pick a catalogue version, or type the version name of a build you downloaded upstream.",
            "Майнеры часто обновляются. Выберите версию из каталога или введите название версии сборки, скачанной самостоятельно.",
        ))
        .build();
    let version_combo = adw::ComboRow::builder()
        .title(tr("Catalogue version", "Версия из каталога"))
        .subtitle(tr(
            "Fills the URL and checksum",
            "Подставляет ссылку и контрольную сумму",
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
    let state_row = adw::ActionRow::builder()
        .title(tr("Installation", "Установка"))
        .build();
    let state_label = label("", &["caption"]);
    state_row.add_suffix(&state_label);
    version_group.add(&version_combo);
    version_group.add(&version_entry);
    version_group.add(&state_row);
    page.add(&version_group);

    let source_group = adw::PreferencesGroup::builder()
        .title(tr("Download", "Загрузка"))
        .description(tr(
            "HTTPS only. A SHA-256 is checked when provided; archives are extracted without path escapes.",
            "Только HTTPS. SHA-256 проверяется, если указан; архивы распаковываются без выхода за каталог.",
        ))
        .build();
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
    let details = adw::ExpanderRow::builder()
        .title(tr("Archive details", "Параметры архива"))
        .subtitle(tr("binary, tar.gz or zip", "binary, tar.gz или zip"))
        .build();
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
    details.add_row(&archive_entry);
    details.add_row(&binary_entry);
    source_group.add(&url_entry);
    source_group.add(&sha_entry);
    source_group.add(&details);
    page.add(&source_group);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.set_margin_top(10);
    footer.set_margin_bottom(12);
    footer.set_margin_start(12);
    footer.set_margin_end(12);
    let status = label("", &["caption", "dim-label"]);
    status.set_hexpand(true);
    let import = gtk::Button::builder()
        .label(tr("Import binary…", "Импортировать бинарник…"))
        .css_classes(["flat", "pill"])
        .build();
    let download = gtk::Button::builder()
        .label(tr("Download and install", "Скачать и установить"))
        .css_classes(["suggested-action", "pill"])
        .build();
    footer.append(&status);
    footer.append(&import);
    footer.append(&download);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    page.set_vexpand(true);
    content.append(&page);
    content.append(&footer);
    window.set_child(Some(&content));

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
                    state_row.set_subtitle(&tr(
                        "Not installed; one installation is shared by every preset.",
                        "Не установлен; одна установка доступна всем пресетам.",
                    ));
                    state_label.set_text(&tr("not installed", "не установлен"));
                    state_label.set_css_classes(&["caption", "warning"]);
                }
            }
        })
    };
    update_state();

    let finish_install: Rc<dyn Fn(MinerInstall)> = {
        let app = app.clone();
        let window = window.clone();
        let on_done = on_done.clone();
        Rc::new(move |install: MinerInstall| {
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
            window.close();
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
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    {
        let window = window.clone();
        let miner_id = miner_id.to_string();
        let version_entry = version_entry.clone();
        let finish_install = finish_install.clone();
        let status = status.clone();
        import.connect_clicked(move |button| {
            let dialog = gtk::FileChooserNative::builder()
                .title(tr("Select miner binary", "Выберите бинарник майнера"))
                .action(gtk::FileChooserAction::Open)
                .build();
            dialog.set_transient_for(Some(&window));
            let miner_id = miner_id.clone();
            let version_entry = version_entry.clone();
            let finish_install = finish_install.clone();
            let status = status.clone();
            let button = button.clone();
            dialog.connect_response(move |dialog, response| {
                if response == gtk::ResponseType::Accept {
                    if let Some(path) = dialog.file().and_then(|file| file.path()) {
                        let mut version = version_entry.text().trim().to_string();
                        if version.is_empty() {
                            version = tr("custom", "своя").to_string();
                        }
                        button.set_sensitive(false);
                        button.set_label(&tr("Importing…", "Импортирую…"));
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
                        let button = button.clone();
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
                                    button.set_sensitive(true);
                                    button.set_label(&tr(
                                        "Import binary…",
                                        "Импортировать бинарник…",
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
                                    button.set_sensitive(true);
                                    button.set_label(&tr(
                                        "Import binary…",
                                        "Импортировать бинарник…",
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
                    button.set_label(&tr("Download and install", "Скачать и установить"));
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
    window.present();
    if let Some(path) = std::env::var_os("TROMMEL_INSTALL_SCREENSHOT") {
        let path = path.to_string_lossy().into_owned();
        let capture = window.clone();
        let application = app.window.application();
        glib::timeout_add_seconds_local(2, move || {
            if let Some(application) = application.clone() {
                schedule_window_screenshot(capture.clone(), path.clone(), application, 0);
            }
            glib::ControlFlow::Break
        });
    }
}

/// Set text plus one semantic colour class on a status label.
fn set_status_text(widget: &gtk::Label, text: &str, class: &str) {
    widget.set_text(text);
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
    if let Some(profile) = cfg.gpu_profile() {
        app.settings
            .temp_limit
            .set_value(profile.default.temp_limit_c as f64);
        app.settings
            .temp_resume
            .set_value(profile.default.temp_resume_c as f64);
    }
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
    let profile = cfg.gpu_profile_mut();
    profile.default.temp_limit_c = app.settings.temp_limit.value() as u32;
    profile.default.temp_resume_c = app.settings.temp_resume.value() as u32;
    set_autostart_unit(app.settings.start_login.is_active());
    mark_dirty(app);
}

fn set_autostart_unit(enable: bool) {
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

/// True when a first start would only fail validation: no preset, no wallet,
/// or no miner binary on disk.
fn needs_setup(app: &Rc<App>) -> bool {
    let cfg = app.config.borrow();
    let Some(preset) = cfg.preset() else {
        return true;
    };
    if preset.wallet.trim().is_empty() {
        return true;
    }
    !cfg.miner_path(preset)
        .map(|path| path.exists())
        .unwrap_or(false)
}

fn save_and_apply(app: &Rc<App>) {
    save_and_apply_with(app, None);
}

/// Apply the draft. While mining, a restart/stop confirmation is shown; when
/// it is cancelled, `on_cancel` restores whatever the caller changed
/// optimistically (the GPU enable switch is the main case).
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
        let dialog = adw::AlertDialog::builder()
            .heading(tr("Apply changes?", "Применить изменения?"))
            .body(tr(
                "The miner is running. GPU/miner changes need a restart. Apply and restart, or apply and stop?",
                "Майнер запущен. Изменения GPU/майнера требуют перезапуска. Применить и перезапустить или применить и остановить?",
            ))
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
                .set_sensitive(!busy && !start_blocked(app));
            if busy {
                app.action_label
                    .set_text(&tr("Waiting for authorization…", "Жду подтверждения…"));
            } else {
                let running = app
                    .status
                    .borrow()
                    .as_ref()
                    .map(|status| status.state.running)
                    .unwrap_or(false);
                app.action_label.set_text(&tr(
                    if running { "Stop" } else { "Start" },
                    if running {
                        "Остановить"
                    } else {
                        "Запустить"
                    },
                ));
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
    }
}

fn gpu_stat(label_text: &str, value_text: &str, hint: Option<&str>, value_class: &str) -> gtk::Box {
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
    box_.add_css_class("trommel-gpu-stat");
    box_.set_hexpand(true);
    let mut classes = vec!["trommel-gpu-stat-value"];
    if !value_class.is_empty() {
        classes.push(value_class);
    }
    box_.append(&label(value_text, &classes));
    box_.append(&label(label_text, &["trommel-gpu-stat-label"]));
    if let Some(hint) = hint {
        let hint_label = label(hint, &["trommel-gpu-stat-hint"]);
        hint_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        hint_label.set_tooltip_text(Some(hint));
        box_.append(&hint_label);
    }
    box_
}

fn make_gpu_card(app: &Rc<App>, gpu: &GpuStats, wide: bool) -> gtk::Widget {
    let card = gtk::Box::new(gtk::Orientation::Vertical, 8);
    card.add_css_class("trommel-gpu-card");
    if wide {
        // A one-card rig should use the whole row instead of a lonely 240 px
        // tile with empty space to the right.
        card.add_css_class("trommel-gpu-card-wide");
        card.set_hexpand(true);
        card.set_halign(gtk::Align::Fill);
    } else {
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
    let enabled = app
        .config
        .borrow()
        .gpu_profile()
        .map(|profile| profile.tuning_for(&gpu.uuid).enabled)
        .unwrap_or(true);
    toggle.set_active(enabled);
    toggle.set_valign(gtk::Align::Center);
    toggle.set_tooltip_text(Some(&tr(
        "Enable this GPU in the active profile",
        "Включить эту карту в активном профиле",
    )));
    let switch_label = tr(
        &format!("GPU {} enable", gpu.index),
        &format!("Карта {} включена", gpu.index),
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
                let profile = cfg.gpu_profile_mut();
                let custom = profile.is_custom(&uuid);
                let mut tuning = profile.tuning_for(&uuid);
                tuning.custom = custom;
                tuning.enabled = new_enabled;
                profile.per_gpu.insert(uuid.clone(), tuning);
            }
            mark_dirty(&app);
            let app_for_cancel = app.clone();
            let uuid_for_cancel = uuid.clone();
            let weak_for_cancel = weak.clone();
            let updating_for_cancel = updating_for_cancel.clone();
            let on_cancel = Rc::new(move || {
                {
                    let mut cfg = app_for_cancel.config.borrow_mut();
                    let profile = cfg.gpu_profile_mut();
                    match previous.clone() {
                        Some(previous) => {
                            profile.per_gpu.insert(uuid_for_cancel.clone(), previous);
                        }
                        None => {
                            profile.per_gpu.remove(&uuid_for_cancel);
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
    let tuning_label = if app
        .config
        .borrow()
        .gpu_profile()
        .map(|profile| profile.is_custom(&gpu.uuid))
        .unwrap_or(false)
    {
        tr("custom", "своя")
    } else {
        tr("profile default", "профиль")
    };
    let stats = gtk::Grid::new();
    stats.set_column_spacing(if wide { 10 } else { 8 });
    stats.set_row_spacing(if wide { 10 } else { 8 });
    stats.set_column_homogeneous(true);
    // One card gets three columns (two rows); a rig of several cards keeps
    // the compact 2 x 3 block.
    let columns: i32 = if wide { 3 } else { 2 };
    let stats_data = [
        (
            tr("Power", "Питание"),
            power,
            power_hint.as_deref(),
            power_class,
        ),
        (
            tr("Core", "Ядро"),
            core_text,
            core_hint.as_deref(),
            core_class,
        ),
        (
            tr("Memory", "Память"),
            memory_text,
            memory_hint.as_deref(),
            memory_class,
        ),
        (tr("Temp", "Темп."), temp, None, temp_class),
        (tr("Fan", "Кулер"), fan, None, ""),
        (tr("Load", "Загрузка"), utilization, None, ""),
    ];
    for (index, (label_text, value_text, hint, class)) in stats_data.iter().enumerate() {
        stats.attach(
            &gpu_stat(label_text, value_text, *hint, class),
            index as i32 % columns,
            index as i32 / columns,
            1,
            1,
        );
    }
    card.append(&stats);

    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    footer.append(&gpu_chip(&tuning_label, "trommel-muted"));
    let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    spacer.set_hexpand(true);
    footer.append(&spacer);
    let tune = gtk::Button::builder()
        .label(tr("Tune", "Настроить"))
        .css_classes(["pill", "flat"])
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

fn refresh_gpu_cards(app: &Rc<App>, gpus: &[GpuStats]) {
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
        app.gpu_host.append(&make_gpu_card(app, &gpus[0], true));
        return;
    }
    let flow = gtk::FlowBox::new();
    flow.set_selection_mode(gtk::SelectionMode::None);
    flow.set_max_children_per_line(3);
    flow.set_min_children_per_line(1);
    flow.set_homogeneous(true);
    flow.set_column_spacing(10);
    flow.set_row_spacing(10);
    flow.set_hexpand(true);
    for gpu in gpus {
        let child = gtk::FlowBoxChild::new();
        child.set_child(Some(&make_gpu_card(app, gpu, false)));
        flow.insert(&child, -1);
    }
    app.gpu_host.append(&flow);
}

fn gpu_chip(text: &str, class: &str) -> gtk::Label {
    let chip = label(text, &["caption", "trommel-gpu-chip"]);
    if !class.is_empty() {
        chip.add_css_class(class);
    }
    chip
}

fn has_enabled_gpu(app: &Rc<App>, gpus: &[GpuStats]) -> bool {
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

fn start_blocked(app: &Rc<App>) -> bool {
    let status = app.status.borrow();
    match status.as_ref() {
        Some(status) if !status.state.running => !has_enabled_gpu(app, &status.gpus),
        _ => false,
    }
}

fn update_start_sensitivity(app: &Rc<App>, blocked: bool) {
    app.action_button.set_sensitive(!blocked);
    let tooltip = blocked.then(|| {
        tr(
            "All GPUs are disabled; enable one on the dashboard before starting",
            "Все карты выключены — включите хотя бы одну на дашборде перед запуском",
        )
    });
    app.action_button.set_tooltip_text(tooltip.as_deref());
}

fn apply_status(app: &Rc<App>, status: Status) {
    let running = status.state.running;
    let blocked = !running && !has_enabled_gpu(app, &status.gpus);
    update_start_sensitivity(app, blocked);
    let action_text = if running {
        tr("Stop", "Остановить")
    } else {
        tr("Start", "Запустить")
    };
    app.action_label.set_text(&action_text);
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
    app.pill.set_text(&pill_text);
    app.pill
        .set_css_classes(&["caption", "trommel-pill", pill_class]);
    let hashrate_value = status
        .total
        .hashrate
        .or(status.miner.primary())
        .filter(|value| *value > 0.0);
    let hashrate = hashrate_value
        .map(util::format_hashrate)
        .unwrap_or_else(|| "0 TH/s".into());
    app.hero_hashrate.set_text(&hashrate);
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
        if hashrate_value.is_some() {
            "accent"
        } else {
            "dim-label"
        },
    ]);
    if status.miner_info.name.is_empty() {
        app.hero_line
            .set_text(&tr("Setup is not finished", "Настройка не завершена"));
    } else {
        app.hero_line.set_text(&format!(
            "{} · {} {} · {:.2}%",
            status.state.preset_name,
            status.miner_info.name,
            status.miner_info.version,
            status.miner_info.total_fee_pct
        ));
    }
    if !running {
        app.hero_meta
            .set_text(&tr("miner is stopped", "майнер остановлен"));
    } else if hashrate_value.is_none() {
        app.hero_meta.set_text(&format!(
            "{} · {}: {}",
            tr("waiting for miner data", "ожидаю данные от майнера"),
            tr("uptime", "аптайм"),
            util::human_duration(status.state.uptime_s)
        ));
    } else {
        app.hero_meta.set_text(&format!(
            "{}: {} · {}: {} · {}",
            tr("uptime", "аптайм"),
            util::human_duration(status.state.uptime_s),
            tr("restarts", "перезапусков"),
            status.state.restarts,
            status.miner.source
        ));
    }
    app.quick_button.set_visible(
        !running
            && (status.miner_info.id.is_empty()
                || !status.miner_info.installed
                || status.warnings.iter().any(|warning| {
                    warning.to_lowercase().contains("wallet") || warning.contains("Кошелёк")
                })),
    );
    app.total_power.set_subtitle(&format!(
        "{} W",
        status
            .total
            .power_w
            .map(|v| format!("{v:.0}"))
            .unwrap_or_else(|| "—".into())
    ));
    app.total_temp.set_subtitle(&format!(
        "{} °C",
        status
            .total
            .temp_max_c
            .map(|v| v.to_string())
            .unwrap_or_else(|| "—".into())
    ));
    app.shares_ok
        .set_text(&status.miner.accepted.unwrap_or(0).to_string());
    app.shares_bad
        .set_text(&status.miner.rejected.unwrap_or(0).to_string());
    let pool_balance = status.pool.as_ref().and_then(|pool| pool.balance);
    let (balance_text, balance_class, balance_tip) = if let Some(balance) = pool_balance {
        (
            format!("{balance:.3} PRL"),
            "accent",
            tr("Pending pool balance", "Баланс пула к выплате"),
        )
    } else {
        let has_wallet = app
            .config
            .borrow()
            .preset()
            .map(|preset| !preset.wallet.trim().is_empty())
            .unwrap_or(false);
        if has_wallet {
            (
                "0.000 PRL".to_string(),
                "dim-label",
                tr(
                    "No pool data yet; the miner is not running",
                    "Данных пула пока нет; майнер не запущен",
                ),
            )
        } else {
            (
                "0 PRL".to_string(),
                "dim-label",
                tr("Wallet is not set", "Кошелёк не задан"),
            )
        }
    };
    app.pool_balance_value.set_text(&balance_text);
    app.pool_balance_value
        .set_css_classes(&["caption", balance_class]);
    app.pool_balance_value.set_tooltip_text(Some(&balance_tip));
    let (pool_endpoint, pool_fee) = app
        .config
        .borrow()
        .preset()
        .map(|preset| {
            (
                preset.pool_url.clone(),
                status
                    .pool
                    .as_ref()
                    .map(|pool| pool.fee_pct)
                    .unwrap_or(preset.pool_fee_pct),
            )
        })
        .unwrap_or_default();
    if pool_endpoint.is_empty() {
        app.pool_meta
            .set_subtitle(&tr("Pool is not selected", "Пул не выбран"));
    } else {
        app.pool_meta
            .set_subtitle(&format!("{pool_endpoint} · {pool_fee:.2}%"));
    }
    app.pool_meta.set_tooltip_text(Some(&tr(
        "Pool endpoint and total pool fee",
        "Адрес пула и комиссия пула",
    )));
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
        "{:?}|{}",
        app.config.borrow().gpu_profile(),
        status
            .gpus
            .iter()
            .map(|gpu| {
                format!(
                    "{}:{:?}:{:?}:{:?}",
                    gpu.uuid, gpu.temp_c, gpu.power_w, gpu.util_pct
                )
            })
            .collect::<Vec<_>>()
            .join("|")
    );
    if *app.gpu_sig.borrow() != signature {
        *app.gpu_sig.borrow_mut() = signature;
        refresh_gpu_cards(app, &status.gpus);
    }
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

/// Screenshot helper for a transient window (the quick-setup wizard).
fn schedule_window_screenshot(
    window: gtk::Window,
    path: String,
    application: gtk::Application,
    attempt: u32,
) {
    schedule_screenshot(window, path, application, attempt);
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
