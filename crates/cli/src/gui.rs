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
use gtk::{gio, glib};

use trommel_core::catalog::{self, MinerOption, OptionKind};
use trommel_core::config::{ChangeClass, Config, GpuTuning, MinerInstall, Preset};
use trommel_core::dbus::MinerProxy;
use trommel_core::i18n::{self, tr};
use trommel_core::status::Status;
use trommel_core::util;

#[derive(Debug)]
enum Cmd {
    Refresh,
    Toggle,
    Apply { json: String, mode: String },
    ResetOc,
    Bench { minutes: u64 },
}

#[derive(Debug)]
enum Ev {
    Status(String),
    Config(String),
    ServiceDown(String),
    Error(String),
    Applied(String),
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
                                    match client.toggle().await {
                                        Ok(_) => { if let Ok(json) = client.refresh().await { emit(Ev::Status(json)); } }
                                        Err(err) => emit(Ev::Error(err.to_string())),
                                    }
                                }
                                Cmd::Apply { json, mode } => {
                                    match client.apply_config(&json, &mode).await {
                                        Ok(result) => emit(Ev::Applied(result)),
                                        Err(err) => emit(Ev::Error(err.to_string())),
                                    }
                                }
                                Cmd::ResetOc => {
                                    match client.reset_oc().await {
                                        Ok(_) => emit(Ev::Applied("Stock settings restored".into())),
                                        Err(err) => emit(Ev::Error(err.to_string())),
                                    }
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

struct PresetEditor {
    stack: gtk::Stack,
    preset_box: gtk::Box,
    name: adw::EntryRow,
    pool: adw::ComboRow,
    pool_url: adw::EntryRow,
    miner: adw::ComboRow,
    version: adw::ComboRow,
    wallet: adw::EntryRow,
    worker: adw::EntryRow,
    pool_fee: adw::SpinRow,
    miner_fee: adw::SpinRow,
    extra: adw::EntryRow,
    info_desc: adw::ActionRow,
    info_fee: adw::ActionRow,
    info_install: adw::ActionRow,
    install_button: gtk::Button,
    download_button: gtk::Button,
    options_box: gtk::Box,
    options: RefCell<Vec<OptionRow>>,
    current_id: RefCell<String>,
    syncing: Cell<bool>,
    delete_button: gtk::Button,
    duplicate_button: gtk::Button,
}

struct SettingsWidgets {
    auto_restart: adw::SwitchRow,
    restore_stock: adw::SwitchRow,
    notifications: adw::SwitchRow,
    start_login: adw::SwitchRow,
    poll: adw::SpinRow,
    language: adw::ComboRow,
    power: adw::SpinRow,
    core: adw::SpinRow,
    memory: adw::SpinRow,
    temp_limit: adw::SpinRow,
    temp_resume: adw::SpinRow,
    panel_mode: adw::ComboRow,
    quick_toggle: adw::SwitchRow,
    live_activity: adw::SwitchRow,
    extension_state: adw::ActionRow,
    extension_enable: gtk::Button,
    extension_manager: gtk::Button,
}

struct App {
    window: adw::ApplicationWindow,
    toast: adw::ToastOverlay,
    stack: adw::ViewStack,
    banner: adw::Banner,
    tx: tokio::sync::mpsc::UnboundedSender<Cmd>,
    status: RefCell<Option<Status>>,
    config: RefCell<Config>,
    applied: RefCell<Config>,
    dirty: Cell<bool>,
    action_button: gtk::Button,
    action_label: gtk::Label,
    hero_hashrate: gtk::Label,
    hero_line: gtk::Label,
    hero_meta: gtk::Label,
    pill: gtk::Label,
    total_power: adw::ActionRow,
    total_temp: adw::ActionRow,
    gpu_box: gtk::Box,
    gpu_sig: RefCell<String>,
    shares: adw::ActionRow,
    pool_balance: adw::ActionRow,
    pool_meta: adw::ActionRow,
    warnings: adw::ActionRow,
    editor: PresetEditor,
    settings: SettingsWidgets,
    miners_box: gtk::Box,
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
    let mut builder = adw::Application::builder().application_id(trommel_core::APP_ID);
    if std::env::var_os("TROMMEL_SCREENSHOT").is_some() {
        builder = builder.flags(gio::ApplicationFlags::NON_UNIQUE);
    }
    let app = builder.build();
    app.connect_activate(build_ui);
    app.run_with_args(&["trommel"]);
    Ok(())
}

fn build_ui(app: &adw::Application) {
    if let Ok(scheme) = std::env::var("TROMMEL_COLOR_SCHEME") {
        adw::StyleManager::default().set_color_scheme(match scheme.as_str() {
            "dark" => adw::ColorScheme::ForceDark,
            "light" => adw::ColorScheme::ForceLight,
            _ => adw::ColorScheme::Default,
        });
    }

    let cfg = Config::load_or_default();
    i18n::set_language(cfg.behavior.language);

    let (event_tx, event_rx) = std::sync::mpsc::channel::<Ev>();
    let event_tx = Arc::new(event_tx);
    let emit: Arc<dyn Fn(Ev) + Send + Sync> = Arc::new(move |event| {
        let _ = event_tx.send(event);
    });
    let tx = spawn_worker(emit);

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Trommel")
        .default_width(760)
        .default_height(820)
        .build();

    let toast = adw::ToastOverlay::new();
    let stack = adw::ViewStack::new();
    toast.set_child(Some(&stack));

    let title = adw::ViewSwitcherTitle::builder()
        .stack(&stack)
        .title("Trommel")
        .subtitle(tr("NVIDIA mining rig manager", "Менеджер NVIDIA-рига"))
        .build();
    let switcher_bar = adw::ViewSwitcherBar::builder().stack(&stack).build();
    title
        .bind_property("title-visible", &switcher_bar, "reveal")
        .sync_create()
        .build();

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

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_bottom_bar(&switcher_bar);
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&banner);
    content.append(&toast);
    toolbar.set_content(Some(&content));
    window.set_content(Some(&toolbar));

    // ------------------------------------------------------------ dashboard
    let dashboard = adw::PreferencesPage::new();

    let hero_group = adw::PreferencesGroup::new();
    let hero = gtk::Box::new(gtk::Orientation::Vertical, 6);
    hero.set_margin_top(10);
    hero.set_margin_bottom(12);
    hero.set_margin_start(6);
    hero.set_margin_end(6);
    let hero_top = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let hero_hashrate = label("—", &["title-1", "numeric", "dim-label"]);
    hero_hashrate.set_hexpand(true);
    let pill = label(
        &tr("stopped", "остановлен"),
        &["caption", "pearl-pill-label"],
    );
    pill.set_valign(gtk::Align::Center);
    hero_top.append(&hero_hashrate);
    hero_top.append(&pill);
    let hero_line = label(&tr("No preset", "Нет пресета"), &["caption"]);
    let hero_meta = label("", &["caption", "dim-label", "numeric"]);
    hero.append(&hero_top);
    hero.append(&hero_line);
    hero.append(&hero_meta);
    hero_group.add(&hero);
    dashboard.add(&hero_group);

    let rig_group = adw::PreferencesGroup::builder()
        .title(tr("Rig", "Риг"))
        .description(tr("Per-GPU live state", "Живое состояние каждой карты"))
        .build();
    let gpu_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    rig_group.add(&gpu_box);
    dashboard.add(&rig_group);

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
    dashboard.add(&totals_group);

    let shares_group = adw::PreferencesGroup::builder()
        .title(tr("Shares and pool", "Шары и пул"))
        .build();
    let shares = adw::ActionRow::builder()
        .title(tr("Shares", "Шары"))
        .subtitle("—")
        .build();
    let pool_balance = adw::ActionRow::builder()
        .title(tr("Pool balance", "Баланс пула"))
        .subtitle("—")
        .build();
    let pool_meta = adw::ActionRow::builder()
        .title(tr("Pool", "Пул"))
        .subtitle("—")
        .build();
    shares_group.add(&shares);
    shares_group.add(&pool_balance);
    shares_group.add(&pool_meta);
    dashboard.add(&shares_group);

    let warnings_group = adw::PreferencesGroup::builder()
        .title(tr("Notes", "Замечания"))
        .build();
    let warnings = adw::ActionRow::builder().title("—").build();
    warnings_group.add(&warnings);
    dashboard.add(&warnings_group);

    stack.add_titled(&dashboard, Some("dashboard"), &tr("Dashboard", "Дашборд"));
    set_page_icon(&stack, &dashboard, "utilities-system-monitor-symbolic");

    // --------------------------------------------------------------- presets
    let (presets_page, editor) = build_presets_page();
    stack.add_titled(&presets_page, Some("presets"), &tr("Presets", "Пресеты"));
    set_page_icon(&stack, &presets_page, "document-open-recent-symbolic");

    let (miners_page, miners_box) = build_miners_page();
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
        tx,
        status: RefCell::new(None),
        applied: RefCell::new(cfg.clone()),
        config: RefCell::new(cfg),
        dirty: Cell::new(false),
        action_button: action_button.clone(),
        action_label: action_label.clone(),
        hero_hashrate,
        hero_line,
        hero_meta,
        pill,
        total_power,
        total_temp,
        gpu_box,
        gpu_sig: RefCell::new(String::new()),
        shares,
        pool_balance,
        pool_meta,
        warnings,
        editor,
        settings,
        miners_box,
    });
    APP_SLOT.with(|slot| *slot.borrow_mut() = Some(app_state.clone()));

    install_actions(app, &app_state);
    connect_signals(&app_state);
    refresh_preset_editor(&app_state);
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
        glib::timeout_add_seconds_local(delay, move || {
            let _ = save_window_png(&window, &path);
            application.quit();
            glib::ControlFlow::Break
        });
    }
}

thread_local! {
    static APP_SLOT: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

fn build_presets_page() -> (adw::PreferencesPage, PresetEditor) {
    let page = adw::PreferencesPage::new();
    let stack = gtk::Stack::new();

    let list_page = gtk::Box::new(gtk::Orientation::Vertical, 12);
    list_page.set_margin_top(12);
    list_page.set_margin_bottom(12);
    let list_group = adw::PreferencesGroup::builder()
        .title(tr("Mining presets", "Пресеты майнинга"))
        .description(tr(
            "A preset bundles coin, pool, wallet, miner and GPU profile.",
            "Пресет объединяет монету, пул, кошелёк, майнер и профиль GPU.",
        ))
        .build();
    let preset_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    list_group.add(&preset_box);
    list_page.append(&list_group);
    let new_button = gtk::Button::builder()
        .label(tr("New preset", "Новый пресет"))
        .css_classes(["suggested-action", "pill"])
        .halign(gtk::Align::Center)
        .build();
    list_page.append(&new_button);
    stack.add_named(&list_page, Some("list"));

    let editor_page = adw::PreferencesPage::new();
    let back = gtk::Button::builder()
        .label(tr("Back", "Назад"))
        .css_classes(["flat"])
        .build();
    let editor_header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    editor_header.set_margin_top(12);
    editor_header.set_margin_start(12);
    editor_header.set_margin_end(12);
    editor_header.append(&back);
    let header_group = adw::PreferencesGroup::new();
    header_group.add(&editor_header);
    editor_page.add(&header_group);

    let info_group = adw::PreferencesGroup::builder()
        .title(tr("Miner", "Майнер"))
        .build();
    let info_desc = adw::ActionRow::builder()
        .title(tr("Description", "Описание"))
        .build();
    let info_fee = adw::ActionRow::builder()
        .title(tr("Fee", "Комиссия"))
        .build();
    let info_install = adw::ActionRow::builder()
        .title(tr("Installation", "Установка"))
        .build();
    let install_button = gtk::Button::builder()
        .label(tr("Import", "Импорт"))
        .css_classes(["pill"])
        .valign(gtk::Align::Center)
        .build();
    let download_button = gtk::Button::builder()
        .label(tr("Download", "Скачать"))
        .css_classes(["pill"])
        .valign(gtk::Align::Center)
        .build();
    info_install.add_suffix(&download_button);
    info_install.add_suffix(&install_button);
    info_group.add(&info_desc);
    info_group.add(&info_fee);
    info_group.add(&info_install);
    editor_page.add(&info_group);

    let form_group = adw::PreferencesGroup::builder()
        .title(tr("Preset", "Пресет"))
        .build();
    let name = adw::EntryRow::builder()
        .title(tr("Name", "Название"))
        .build();
    let pool = adw::ComboRow::builder().title(tr("Pool", "Пул")).build();
    let pool_url = adw::EntryRow::builder()
        .title(tr("Pool address", "Адрес пула"))
        .build();
    let miner = adw::ComboRow::builder()
        .title(tr("Miner", "Майнер"))
        .build();
    let version = adw::ComboRow::builder()
        .title(tr("Version", "Версия"))
        .build();
    let wallet = adw::EntryRow::builder()
        .title(tr("Wallet", "Кошелёк"))
        .build();
    let worker = adw::EntryRow::builder()
        .title(tr("Worker", "Воркер"))
        .build();
    let pool_fee = spin_row(
        &tr("Pool fee", "Комиссия пула"),
        &tr(
            "Used for effective hashrate; the pool sets the real fee",
            "Для расчёта effective; реальную ставку задаёт пул",
        ),
        1.0,
        0.0,
        20.0,
    );
    let miner_fee = spin_row(
        &tr("Miner fee override", "Переопределение комиссии майнера"),
        &tr(
            "Editable only when the miner supports it",
            "Редактируется только если майнер это поддерживает",
        ),
        0.0,
        0.0,
        100.0,
    );
    let extra = adw::EntryRow::builder()
        .title(tr("Extra arguments", "Доп. аргументы"))
        .build();
    form_group.add(&name);
    form_group.add(&pool);
    form_group.add(&pool_url);
    form_group.add(&miner);
    form_group.add(&version);
    form_group.add(&wallet);
    form_group.add(&worker);
    form_group.add(&extra);
    form_group.add(&pool_fee);
    form_group.add(&miner_fee);
    editor_page.add(&form_group);

    let options_group = adw::PreferencesGroup::builder()
        .title(tr("Miner options", "Настройки майнера"))
        .description(tr(
            "Defaults come from the selected miner version.",
            "Значения по умолчанию берутся из выбранной версии майнера.",
        ))
        .build();
    let options_box = gtk::Box::new(gtk::Orientation::Vertical, 0);
    options_group.add(&options_box);
    editor_page.add(&options_group);

    let actions = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    actions.set_margin_top(12);
    actions.set_margin_bottom(18);
    actions.set_halign(gtk::Align::Center);
    let save = gtk::Button::builder()
        .label(tr("Save and apply", "Сохранить и применить"))
        .css_classes(["suggested-action", "pill"])
        .build();
    let duplicate_button = gtk::Button::builder()
        .label(tr("Duplicate", "Дублировать"))
        .css_classes(["pill"])
        .build();
    let delete_button = gtk::Button::builder()
        .label(tr("Delete", "Удалить"))
        .css_classes(["destructive-action", "pill"])
        .build();
    actions.append(&save);
    actions.append(&duplicate_button);
    actions.append(&delete_button);
    let actions_group = adw::PreferencesGroup::new();
    actions_group.add(&actions);
    editor_page.add(&actions_group);

    stack.add_named(&editor_page, Some("editor"));
    let stack_group = adw::PreferencesGroup::new();
    stack_group.add(&stack);
    page.add(&stack_group);

    let editor = PresetEditor {
        stack: stack.clone(),
        preset_box,
        name,
        pool,
        pool_url,
        miner,
        version,
        wallet,
        worker,
        pool_fee,
        miner_fee,
        extra,
        info_desc,
        info_fee,
        info_install,
        install_button,
        download_button,
        options_box,
        options: RefCell::new(Vec::new()),
        current_id: RefCell::new(String::new()),
        syncing: Cell::new(false),
        delete_button,
        duplicate_button,
    };
    // The new/back/save/delete buttons are connected after the Rc<App> exists.
    NEW_PRESET_BUTTON.with(|slot| *slot.borrow_mut() = Some(new_button));
    BACK_BUTTON.with(|slot| *slot.borrow_mut() = Some(back));
    SAVE_BUTTON.with(|slot| *slot.borrow_mut() = Some(save));
    (page, editor)
}

thread_local! {
    static NEW_PRESET_BUTTON: RefCell<Option<gtk::Button>> = const { RefCell::new(None) };
    static BACK_BUTTON: RefCell<Option<gtk::Button>> = const { RefCell::new(None) };
    static SAVE_BUTTON: RefCell<Option<gtk::Button>> = const { RefCell::new(None) };
}

fn build_miners_page() -> (adw::PreferencesPage, gtk::Box) {
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::builder()
        .title(tr("Installed and supported miners", "Установленные и поддерживаемые майнеры"))
        .description(tr(
            "Install once and reuse across all presets. Import a binary or open the upstream release page.",
            "Установите один раз и используйте во всех пресетах. Импортируйте бинарник или откройте страницу релиза.",
        ))
        .build();
    let box_ = gtk::Box::new(gtk::Orientation::Vertical, 0);
    group.add(&box_);
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
        .css_classes(["pill"])
        .build();
    let row = adw::ActionRow::new();
    row.add_suffix(&run);
    bench_group.add(&minutes);
    bench_group.add(&row);
    page.add(&bench_group);
    BENCH_MINUTES.with(|slot| *slot.borrow_mut() = Some(minutes));
    BENCH_BUTTON.with(|slot| *slot.borrow_mut() = Some(run));
    (page, box_)
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
            "Restore the power limit captured before mining",
            "Возвращать лимит питания, снятый до майнинга",
        ),
    );
    let notifications = switch_row(
        &tr("Notifications", "Уведомления"),
        &tr(
            "Start, stop and overheat notifications",
            "Старт, стоп и перегрев",
        ),
    );
    let start_login = switch_row(
        &tr("Start mining at login", "Запускать майнинг при входе"),
        &tr(
            "Autostart the miner with the desktop session",
            "Автозапуск майнера вместе с сессией",
        ),
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
    for row in [&auto_restart, &restore_stock, &notifications, &start_login] {
        behavior.add(row);
    }
    behavior.add(&poll);
    behavior.add(&language);
    page.add(&behavior);

    let gpu = adw::PreferencesGroup::builder()
        .title(tr("GPU defaults", "Настройки GPU по умолчанию"))
        .description(tr(
            "Per-GPU overrides are stored in the config; this is the default profile.",
            "Переопределения по картам хранятся в конфиге; здесь — профиль по умолчанию.",
        ))
        .build();
    let power = spin_row(
        &tr("Power limit", "Лимит питания"),
        "W",
        300.0,
        50.0,
        1000.0,
    );
    let core = spin_row(
        &tr("Core clock lock", "Лок ядра"),
        "MHz",
        1620.0,
        0.0,
        4000.0,
    );
    let memory = spin_row(
        &tr("Memory clock lock", "Лок памяти"),
        "MHz",
        810.0,
        0.0,
        12000.0,
    );
    let temp_limit = spin_row(
        &tr("Temperature limit", "Порог температуры"),
        "°C",
        90.0,
        60.0,
        100.0,
    );
    let temp_resume = spin_row(
        &tr("Resume temperature", "Температура возобновления"),
        "°C",
        65.0,
        40.0,
        95.0,
    );
    for row in [&power, &core, &memory, &temp_limit, &temp_resume] {
        gpu.add(row);
    }
    page.add(&gpu);

    let extension = adw::PreferencesGroup::builder()
        .title(tr("GNOME integration", "Интеграция с GNOME"))
        .description(tr(
            "The panel applet is optional; mining works without it. Settings here are shared with the extension via GSettings.",
            "Апплет в панели необязателен; майнинг работает и без него. Настройки здесь общие с расширением через GSettings.",
        ))
        .build();
    let panel_mode = adw::ComboRow::builder()
        .title(tr("Panel label", "Подпись в панели"))
        .model(&gtk::StringList::new(&[
            tr("Icon only", "Только иконка").as_str(),
            tr("Hashrate", "Хешрейт").as_str(),
            tr("Temperature", "Температура").as_str(),
            tr("Hashrate and temperature", "Хешрейт и температура").as_str(),
        ]))
        .build();
    let quick_toggle = switch_row(
        &tr("Quick Settings toggle", "Переключатель в Quick Settings"),
        &tr(
            "Start/stop mining from the system menu",
            "Старт/стоп из системного меню",
        ),
    );
    let live_activity = switch_row(
        &tr("Live activity card", "Живая карточка"),
        &tr(
            "Show hashrate and temperature in the popup",
            "Показывать хешрейт и температуру в поповере",
        ),
    );
    let extension_state = adw::ActionRow::builder()
        .title(tr("GNOME Shell extension", "Расширение GNOME Shell"))
        .subtitle(tr("Checking…", "Проверяю…"))
        .build();
    let extension_enable = gtk::Button::builder()
        .label(tr("Enable", "Включить"))
        .css_classes(["pill"])
        .valign(gtk::Align::Center)
        .build();
    let extension_manager = gtk::Button::builder()
        .label(tr("Extension Manager", "Менеджер расширений"))
        .css_classes(["flat"])
        .valign(gtk::Align::Center)
        .build();
    extension_state.add_suffix(&extension_manager);
    extension_state.add_suffix(&extension_enable);
    extension.add(&panel_mode);
    extension.add(&quick_toggle);
    extension.add(&live_activity);
    extension.add(&extension_state);
    page.add(&extension);

    let actions = adw::PreferencesGroup::builder()
        .title(tr("Actions", "Действия"))
        .build();
    let save = gtk::Button::builder()
        .label(tr("Save and apply", "Сохранить и применить"))
        .css_classes(["suggested-action", "pill"])
        .build();
    let save_row = adw::ActionRow::builder()
        .title(tr("Save settings", "Сохранить настройки"))
        .build();
    save_row.add_suffix(&save);
    let stock = gtk::Button::builder()
        .label(tr("Restore stock", "Вернуть сток"))
        .css_classes(["destructive-action", "pill"])
        .build();
    let stock_row = adw::ActionRow::builder()
        .title(tr("Restore stock now", "Вернуть сток сейчас"))
        .subtitle(tr(
            "Restores the snapshot captured before mining",
            "Возвращает снимок, снятый до майнинга",
        ))
        .build();
    stock_row.add_suffix(&stock);
    actions.add(&save_row);
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
            poll,
            language,
            power,
            core,
            memory,
            temp_limit,
            temp_resume,
            panel_mode,
            quick_toggle,
            live_activity,
            extension_state,
            extension_enable,
            extension_manager,
        },
    )
}

thread_local! {
    static SAVE_SETTINGS_BUTTON: RefCell<Option<gtk::Button>> = const { RefCell::new(None) };
    static STOCK_BUTTON: RefCell<Option<gtk::Button>> = const { RefCell::new(None) };
}

// --------------------------------------------------------------- signals

fn connect_signals(app: &Rc<App>) {
    {
        let tx = app.tx.clone();
        app.action_button.connect_clicked(move |_| {
            let _ = tx.send(Cmd::Toggle);
        });
    }
    {
        let app = app.clone();
        let banner = app.banner.clone();
        banner.connect_button_clicked(move |_| {
            save_and_apply(&app);
        });
    }
    if let Some(button) = NEW_PRESET_BUTTON.with(|slot| slot.borrow().clone()) {
        let app = app.clone();
        button.connect_clicked(move |_| new_preset(&app));
    }
    if let Some(button) = BACK_BUTTON.with(|slot| slot.borrow().clone()) {
        let app = app.clone();
        button.connect_clicked(move |_| {
            app.editor.stack.set_visible_child_name("list");
            refresh_preset_list(&app);
        });
    }
    if let Some(button) = SAVE_BUTTON.with(|slot| slot.borrow().clone()) {
        let app = app.clone();
        button.connect_clicked(move |_| {
            save_editor(&app);
            save_and_apply(&app);
        });
    }
    if let Some(button) = SAVE_SETTINGS_BUTTON.with(|slot| slot.borrow().clone()) {
        let app = app.clone();
        button.connect_clicked(move |_| {
            save_settings(&app);
            save_and_apply(&app);
        });
    }
    if let Some(button) = STOCK_BUTTON.with(|slot| slot.borrow().clone()) {
        let tx = app.tx.clone();
        button.connect_clicked(move |_| {
            let _ = tx.send(Cmd::ResetOc);
        });
    }
    {
        let app = app.clone();
        let button = app.editor.install_button.clone();
        button.connect_clicked(move |_| {
            let miner = catalog::miners()
                .get(app.editor.miner.selected() as usize)
                .map(|m| m.id.clone())
                .unwrap_or_else(|| "wildrig".into());
            let version = catalog::miners()
                .get(app.editor.miner.selected() as usize)
                .and_then(|m| m.versions.get(app.editor.version.selected() as usize))
                .map(|v| v.version.clone())
                .unwrap_or_default();
            import_miner(&app, &miner, &version);
        });
    }
    {
        let app = app.clone();
        let button = app.editor.download_button.clone();
        button.connect_clicked(move |_| {
            let miner = catalog::miners()
                .get(app.editor.miner.selected() as usize)
                .map(|m| m.id.clone())
                .unwrap_or_else(|| "wildrig".into());
            let version = catalog::miners()
                .get(app.editor.miner.selected() as usize)
                .and_then(|m| m.versions.get(app.editor.version.selected() as usize))
                .cloned();
            download_dialog(&app, &miner, version);
        });
    }
    {
        let app = app.clone();
        let button = app.settings.extension_enable.clone();
        button.connect_clicked(move |_| enable_extension(&app));
    }
    {
        let app = app.clone();
        let button = app.settings.extension_manager.clone();
        button.connect_clicked(move |_| open_extension_manager(&app));
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
    {
        let app = app.clone();
        let button = app.editor.delete_button.clone();
        button.connect_clicked(move |_| delete_preset(&app));
    }
    {
        let app = app.clone();
        let button = app.editor.duplicate_button.clone();
        button.connect_clicked(move |_| duplicate_preset(&app));
    }
    {
        let app = app.clone();
        let miner = app.editor.miner.clone();
        miner.connect_notify_local(Some("selected"), move |_, _| {
            if app.editor.syncing.get() {
                return;
            }
            update_versions(&app);
            rebuild_options(&app);
            update_miner_info(&app);
        });
    }
    {
        let app = app.clone();
        let pool = app.editor.pool.clone();
        pool.connect_notify_local(Some("selected"), move |_, _| {
            if app.editor.syncing.get() {
                return;
            }
            if let Some(pool) = catalog::pools().get(app.editor.pool.selected() as usize) {
                app.editor.pool_url.set_text(&pool.url);
                app.editor.pool_fee.set_value(pool.fee_pct);
            }
        });
    }
    {
        let app = app.clone();
        let version = app.editor.version.clone();
        version.connect_notify_local(Some("selected"), move |_, _| {
            if app.editor.syncing.get() {
                return;
            }
            update_miner_info(&app);
            rebuild_options(&app);
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
    log.connect_activate(|_, _| {
        let path = trommel_core::paths::log_path();
        let uri = gio::File::for_path(path).uri();
        let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
    });
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

fn refresh_preset_list(app: &Rc<App>) {
    clear_box(&app.editor.preset_box);
    let cfg = app.config.borrow();
    for (id, preset) in &cfg.presets {
        let row = adw::ActionRow::builder()
            .title(&preset.name)
            .subtitle(format!(
                "{} · {} · {} {}",
                preset.coin, preset.pool, preset.miner, preset.miner_version
            ))
            .build();
        if Some(id) == cfg.active_preset.as_ref() {
            let badge = gtk::Label::new(Some(&tr("Active", "Активен")));
            badge.add_css_class("accent");
            row.add_suffix(&badge);
        }
        let edit = gtk::Button::builder()
            .label(tr("Edit", "Изменить"))
            .css_classes(["flat"])
            .build();
        let use_button = gtk::Button::builder()
            .label(tr("Use", "Использовать"))
            .css_classes(["pill"])
            .build();
        row.add_suffix(&edit);
        row.add_suffix(&use_button);
        let app_edit = app.clone();
        let id_edit = id.clone();
        edit.connect_clicked(move |_| {
            load_preset_into_editor(&app_edit, &id_edit);
            app_edit.editor.stack.set_visible_child_name("editor");
        });
        let app_use = app.clone();
        let id_use = id.clone();
        use_button.connect_clicked(move |_| {
            app_use.config.borrow_mut().active_preset = Some(id_use.clone());
            load_preset_into_editor(&app_use, &id_use);
            mark_dirty(&app_use);
            refresh_preset_list(&app_use);
            refresh_preset_editor(&app_use);
        });
        app.editor.preset_box.append(&row);
    }
}

fn refresh_preset_editor(app: &Rc<App>) {
    let id = app
        .config
        .borrow()
        .active_preset
        .clone()
        .unwrap_or_default();
    load_preset_into_editor(app, &id);
}

fn load_preset_into_editor(app: &Rc<App>, id: &str) {
    let Some(preset) = app.config.borrow().presets.get(id).cloned() else {
        return;
    };
    app.editor.syncing.set(true);
    *app.editor.current_id.borrow_mut() = id.to_string();
    app.editor.name.set_text(&preset.name);
    app.editor.pool.set_model(Some(&gtk::StringList::new(
        &pool_names().iter().map(String::as_str).collect::<Vec<_>>(),
    )));
    app.editor.miner.set_model(Some(&gtk::StringList::new(
        &miner_names().iter().map(String::as_str).collect::<Vec<_>>(),
    )));
    app.editor.pool.set_selected(pool_index(&preset.pool));
    app.editor.miner.set_selected(miner_index(&preset.miner));
    app.editor.pool_url.set_text(&preset.pool_url);
    app.editor.wallet.set_text(&preset.wallet);
    app.editor.worker.set_text(&preset.worker);
    app.editor.pool_fee.set_value(preset.pool_fee_pct);
    app.editor.miner_fee.set_value(
        preset
            .miner_fee_override_pct
            .unwrap_or(preset.miner_fee_pct),
    );
    let fee_configurable = catalog::miner(&preset.miner)
        .map(|m| m.fee_configurable)
        .unwrap_or(false);
    app.editor.miner_fee.set_sensitive(fee_configurable);
    let fee_hint = if fee_configurable {
        tr("Passed to the miner", "Передаётся майнеру")
    } else {
        tr(
            "Fixed by the miner; shown for reference",
            "Фиксирована майнером; показана справочно",
        )
    };
    app.editor.miner_fee.set_subtitle(&fee_hint);
    app.editor.extra.set_text(&preset.extra_args);
    update_versions(app);
    if let Some(index) = catalog::miner(&preset.miner).and_then(|m| {
        m.versions
            .iter()
            .position(|v| v.version == preset.miner_version)
    }) {
        app.editor.version.set_selected(index as u32);
    }
    app.editor.syncing.set(false);
    rebuild_options(app);
    update_miner_info(app);
}

fn update_versions(app: &Rc<App>) {
    let index = app.editor.miner.selected() as usize;
    let versions = catalog::miners()
        .get(index)
        .map(|m| {
            m.versions
                .iter()
                .map(|v| {
                    let installed = app.config.borrow().install(&m.id, &v.version).is_some();
                    if installed {
                        format!("{} ✓", v.version)
                    } else {
                        v.version.clone()
                    }
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let refs: Vec<&str> = versions.iter().map(String::as_str).collect();
    app.editor
        .version
        .set_model(Some(&gtk::StringList::new(&refs)));
    if !refs.is_empty() {
        app.editor.version.set_selected(0);
    }
}

fn rebuild_options(app: &Rc<App>) {
    clear_box(&app.editor.options_box);
    app.editor.options.borrow_mut().clear();
    let Some(miner) = catalog::miners().get(app.editor.miner.selected() as usize) else {
        return;
    };
    let preset = app
        .config
        .borrow()
        .presets
        .get(&*app.editor.current_id.borrow())
        .cloned();
    let values = preset
        .as_ref()
        .map(|p| p.miner_options.clone())
        .unwrap_or_default();
    for option in &miner.options {
        let value = values
            .get(&option.key)
            .cloned()
            .or_else(|| option.default_value());
        let (row_widget, option_widget) = build_option(option, value.as_ref());
        app.editor.options_box.append(&row_widget);
        app.editor.options.borrow_mut().push(OptionRow {
            key: option.key.clone(),
            widget: option_widget,
        });
    }
    if miner.options.is_empty() {
        let label = gtk::Label::new(Some(&tr(
            "No miner-specific options.",
            "Специфических настроек у майнера нет.",
        )));
        label.add_css_class("dim-label");
        label.set_margin_top(8);
        label.set_margin_bottom(8);
        app.editor.options_box.append(&label);
    }
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

fn update_miner_info(app: &Rc<App>) {
    let miner = catalog::miners().get(app.editor.miner.selected() as usize);
    let version_index = app.editor.version.selected() as usize;
    let preset = app
        .config
        .borrow()
        .presets
        .get(&*app.editor.current_id.borrow())
        .cloned();
    let Some(miner) = miner else {
        app.editor.info_desc.set_subtitle("");
        app.editor.info_fee.set_subtitle("");
        app.editor.info_install.set_subtitle("");
        return;
    };
    let version = miner.versions.get(version_index);
    let selected_version = version
        .map(|v| v.version.clone())
        .or_else(|| preset.as_ref().map(|p| p.miner_version.clone()))
        .unwrap_or_default();
    let installed = app
        .config
        .borrow()
        .install(&miner.id, &selected_version)
        .is_some();
    let fee = preset
        .as_ref()
        .map(|p| p.total_fee_pct())
        .unwrap_or(miner.fee_pct);
    app.editor.info_desc.set_subtitle(miner.short_description());
    app.editor.miner_fee.set_sensitive(miner.fee_configurable);
    let fee_hint = if miner.fee_configurable {
        tr("Passed to the miner", "Передаётся майнеру")
    } else {
        tr(
            "Fixed by the miner; shown for reference",
            "Фиксирована майнером; показана справочно",
        )
    };
    app.editor.miner_fee.set_subtitle(&fee_hint);
    app.editor.info_fee.set_subtitle(&format!(
        "{}: {:.2}% · {}: {:.2}% · {}: {:.2}%",
        tr("miner", "майнер"),
        miner.fee_pct,
        tr("pool", "пул"),
        preset.as_ref().map(|p| p.pool_fee_pct).unwrap_or(0.0),
        tr("total", "всего"),
        fee
    ));
    let install_text = if installed {
        tr("Installed", "Установлен").to_string()
    } else {
        tr("Not installed", "Не установлен").to_string()
    };
    app.editor.info_install.set_subtitle(&format!(
        "{install_text} · {} {}",
        selected_version,
        version
            .map(|v| if v.verified { "✓" } else { "⚠" })
            .unwrap_or("")
    ));
    app.editor.install_button.set_sensitive(!installed);
    app.editor.install_button.set_visible(!installed);
}

fn refresh_miners(app: &Rc<App>) {
    clear_box(&app.miners_box);
    let cfg = app.config.borrow();
    for miner in catalog::miners() {
        let installed = catalog::installed_versions(&cfg, &miner.id);
        let subtitle = format!(
            "{} · {} {:.2}% · {} {}",
            miner.short_description(),
            tr("fee", "комиссия"),
            miner.fee_pct,
            tr("installed", "установлено"),
            if installed.is_empty() {
                "—".to_string()
            } else {
                installed.join(", ")
            }
        );
        let row = adw::ActionRow::builder()
            .title(&miner.name)
            .subtitle(&subtitle)
            .build();
        let import = gtk::Button::builder()
            .label(tr("Import binary", "Импорт бинарника"))
            .css_classes(["pill"])
            .build();
        let source = gtk::Button::builder()
            .label(tr("Release page", "Страница релиза"))
            .css_classes(["flat"])
            .build();
        row.add_suffix(&source);
        row.add_suffix(&import);
        let app_import = app.clone();
        let miner_id = miner.id.clone();
        let version = miner
            .latest_version()
            .map(|v| v.version.clone())
            .unwrap_or_default();
        import.connect_clicked(move |_| import_miner(&app_import, &miner_id, &version));
        let url = miner.source_url.clone();
        source.connect_clicked(move |_| {
            let _ = gio::AppInfo::launch_default_for_uri(&url, None::<&gio::AppLaunchContext>);
        });
        app.miners_box.append(&row);
    }
}

fn import_miner(app: &Rc<App>, miner: &str, version: &str) {
    let dialog = gtk::FileChooserNative::builder()
        .title(tr("Select miner binary", "Выберите бинарник майнера"))
        .action(gtk::FileChooserAction::Open)
        .build();
    dialog.connect_response({
        let app = app.clone();
        let miner = miner.to_string();
        let version = version.to_string();
        move |dialog, response| {
            if response == gtk::ResponseType::Accept {
                if let Some(file) = dialog.file() {
                    if let Some(path) = file.path() {
                        let install = MinerInstall {
                            miner: miner.clone(),
                            version: version.clone(),
                            path: path.to_string_lossy().into_owned(),
                            sha256: String::new(),
                            source_url: String::new(),
                            verified: false,
                            custom: true,
                            installed_at: util::now_unix(),
                        };
                        {
                            let mut cfg = app.config.borrow_mut();
                            cfg.add_install(install);
                            let _ = cfg.save();
                        }
                        refresh_miners(&app);
                        refresh_preset_editor(&app);
                        app.toast.add_toast(adw::Toast::new(&tr(
                            "Miner imported. It is now available in every preset.",
                            "Майнер импортирован. Теперь он доступен во всех пресетах.",
                        )));
                    }
                }
            }
            dialog.destroy();
        }
    });
    dialog.show();
}

fn extension_settings() -> Option<gio::Settings> {
    let source = gio::SettingsSchemaSource::default()?;
    let schema = source.lookup("org.gnome.shell.extensions.trommel", true)?;
    Some(gio::Settings::new_full(
        &schema,
        None::<&gio::SettingsBackend>,
        None,
    ))
}

fn refresh_extension_state(app: &Rc<App>) {
    let info = std::process::Command::new("gnome-extensions")
        .args(["info", "trommel@nayvok.github.io"])
        .output();
    let enabled = std::process::Command::new("gnome-extensions")
        .args(["list", "--enabled"])
        .output()
        .map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .any(|line| line.trim() == "trommel@nayvok.github.io")
        })
        .unwrap_or(false);
    let text = match info {
        Ok(output) => format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ),
        Err(_) => String::new(),
    };
    if text.contains("не найден") || text.contains("not found") || text.is_empty() {
        app.settings.extension_state.set_subtitle(&tr(
            "Not installed — install the package or run install.sh",
            "Не установлено — установите пакет или запустите install.sh",
        ));
        app.settings.extension_enable.set_sensitive(false);
        return;
    }
    let state_text = if enabled {
        tr("Enabled", "Включено")
    } else {
        tr(
            "Disabled — enable it here or in Extension Manager",
            "Выключено — включите здесь или в менеджере расширений",
        )
    };
    app.settings.extension_state.set_subtitle(&state_text);
    app.settings.extension_enable.set_sensitive(!enabled);
}

fn enable_extension(app: &Rc<App>) {
    let output = std::process::Command::new("gnome-extensions")
        .args(["enable", "trommel@nayvok.github.io"])
        .output();
    match output {
        Ok(output) if output.status.success() => {
            refresh_extension_state(app);
            app.toast.add_toast(adw::Toast::new(&tr(
                "Extension enabled. Log out and back in if the panel icon does not appear.",
                "Расширение включено. Если иконка не появилась — перелогиньтесь.",
            )));
        }
        Ok(output) => {
            let text = String::from_utf8_lossy(&output.stderr).to_string();
            app.toast.add_toast(adw::Toast::new(&text));
        }
        Err(err) => app.toast.add_toast(adw::Toast::new(&err.to_string())),
    }
}

fn open_extension_manager(app: &Rc<App>) {
    let launched = std::process::Command::new("gtk-launch")
        .arg("com.mattjakeman.ExtensionManager")
        .spawn()
        .is_ok();
    if !launched {
        app.toast.add_toast(adw::Toast::new(&tr(
            "Extension Manager is not installed.",
            "Менеджер расширений не установлен.",
        )));
    }
}

fn gpu_tune_dialog(app: &Rc<App>, uuid: &str, name: &str) {
    let Some(profile) = app.config.borrow().gpu_profile().cloned() else {
        return;
    };
    let tuning = profile.tuning_for(uuid);
    let enabled = tuning.enabled;
    let window = gtk::Window::builder()
        .title(format!("{name} · {}", tr("GPU tuning", "Настройка GPU")))
        .modal(true)
        .transient_for(&app.window)
        .default_width(520)
        .build();
    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    let save = gtk::Button::builder()
        .label(tr("Save", "Сохранить"))
        .css_classes(["suggested-action"])
        .build();
    header.pack_end(&save);
    toolbar.add_top_bar(&header);
    let page = adw::PreferencesPage::new();
    let group = adw::PreferencesGroup::builder().title(name).build();
    let power = spin_row(
        &tr("Power limit", "Лимит питания"),
        "W",
        tuning.power_limit_w.unwrap_or(0) as f64,
        0.0,
        1000.0,
    );
    let core = spin_row(
        &tr("Core clock", "Частота ядра"),
        "MHz",
        tuning.core_clock_mhz.unwrap_or(0) as f64,
        0.0,
        4000.0,
    );
    let memory = spin_row(
        &tr("Memory clock", "Частота памяти"),
        "MHz",
        tuning.memory_clock_mhz.unwrap_or(0) as f64,
        0.0,
        12000.0,
    );
    let fan = spin_row(
        &tr("Fan", "Вентилятор"),
        "%",
        tuning.fan_pct.unwrap_or(0) as f64,
        0.0,
        100.0,
    );
    let temp = spin_row(
        &tr("Temperature limit", "Порог температуры"),
        "°C",
        tuning.temp_limit_c as f64,
        60.0,
        100.0,
    );
    for row in [&power, &core, &memory, &fan, &temp] {
        group.add(row);
    }
    page.add(&group);
    toolbar.set_content(Some(&page));
    window.set_child(Some(&toolbar));
    {
        let app = app.clone();
        let window = window.clone();
        let uuid = uuid.to_string();
        save.connect_clicked(move |_| {
            let tuning = GpuTuning {
                enabled,
                power_limit_w: nonzero(power.value() as u32),
                core_clock_mhz: nonzero(core.value() as u32),
                memory_clock_mhz: nonzero(memory.value() as u32),
                fan_pct: nonzero(fan.value() as u32),
                temp_limit_c: temp.value() as u32,
                temp_resume_c: 65,
            };
            app.config
                .borrow_mut()
                .gpu_profile_mut()
                .per_gpu
                .insert(uuid.clone(), tuning);
            mark_dirty(&app);
            window.close();
            app.toast.add_toast(adw::Toast::new(&tr(
                "GPU tuning saved to the draft. Apply to use it.",
                "Настройка GPU сохранена в draft. Примените, чтобы использовать.",
            )));
        });
    }
    window.present();
}

fn download_dialog(app: &Rc<App>, miner: &str, version: Option<catalog::MinerVersion>) {
    let dialog = gtk::Window::builder()
        .title(tr("Download miner", "Скачать майнер"))
        .modal(true)
        .transient_for(&app.window)
        .default_width(520)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 8);
    content.set_margin_top(16);
    content.set_margin_bottom(16);
    content.set_margin_start(16);
    content.set_margin_end(16);

    let title = gtk::Label::builder()
        .label(format!(
            "{miner} {}",
            version.as_ref().map(|v| v.version.as_str()).unwrap_or("")
        ))
        .xalign(0.0)
        .css_classes(["title-4"])
        .build();
    content.append(&title);

    let url = gtk::Entry::builder()
        .placeholder_text(tr(
            "Direct HTTPS archive or binary URL",
            "Прямая HTTPS-ссылка на архив или бинарник",
        ))
        .text(version.as_ref().map(|v| v.url.clone()).unwrap_or_default())
        .build();
    let sha = gtk::Entry::builder()
        .placeholder_text(tr("SHA-256 (recommended)", "SHA-256 (рекомендуется)"))
        .text(
            version
                .as_ref()
                .map(|v| v.sha256.clone())
                .unwrap_or_default(),
        )
        .build();
    let binary = gtk::Entry::builder()
        .placeholder_text(tr(
            "Binary name inside the archive",
            "Имя бинарника внутри архива",
        ))
        .text(
            version
                .as_ref()
                .map(|v| v.binary.clone())
                .unwrap_or_else(|| "wildrig-multi".into()),
        )
        .build();
    let archive = gtk::Entry::builder()
        .placeholder_text(tr("binary, tar.gz or zip", "binary, tar.gz или zip"))
        .text(
            version
                .as_ref()
                .map(|v| v.archive.clone())
                .unwrap_or_else(|| "binary".into()),
        )
        .build();
    for (label_text, widget) in [
        (tr("URL", "URL"), &url),
        (tr("SHA-256", "SHA-256"), &sha),
        (tr("Binary", "Бинарник"), &binary),
        (tr("Archive", "Архив"), &archive),
    ] {
        let row = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let label = gtk::Label::builder()
            .label(label_text)
            .xalign(0.0)
            .css_classes(["caption", "dim-label"])
            .build();
        row.append(&label);
        row.append(widget);
        content.append(&row);
    }

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    buttons.set_halign(gtk::Align::End);
    buttons.set_margin_top(8);
    let cancel = gtk::Button::with_label(&tr("Cancel", "Отмена"));
    let start = gtk::Button::builder()
        .label(tr("Download", "Скачать"))
        .css_classes(["suggested-action"])
        .build();
    buttons.append(&cancel);
    buttons.append(&start);
    content.append(&buttons);
    dialog.set_child(Some(&content));

    {
        let dialog = dialog.clone();
        cancel.connect_clicked(move |_| dialog.close());
    }
    {
        let dialog = dialog.clone();
        let app = app.clone();
        let miner = miner.to_string();
        let url = url.clone();
        let sha = sha.clone();
        let binary = binary.clone();
        let archive = archive.clone();
        let start_button = start.clone();
        start.connect_clicked(move |_| {
            let url_text = url.text().to_string();
            if !url_text.starts_with("https://") {
                app.toast.add_toast(adw::Toast::new(&tr(
                    "Only HTTPS URLs are accepted.",
                    "Принимаются только HTTPS-ссылки.",
                )));
                return;
            }
            start_button.set_sensitive(false);
            start_button.set_label(&tr("Downloading…", "Скачиваю…"));
            let request = trommel_core::download::DownloadRequest {
                miner: miner.clone(),
                version: version
                    .as_ref()
                    .map(|v| v.version.clone())
                    .unwrap_or_default(),
                url: url_text,
                sha256: sha.text().to_string(),
                archive: archive.text().to_string(),
                binary: binary.text().to_string(),
                source_url: String::new(),
            };
            let (tx, rx) = std::sync::mpsc::channel::<Result<MinerInstall, String>>();
            let root = trommel_core::download::miner_root();
            std::thread::spawn(move || {
                let result =
                    trommel_core::download::install(&request, &root).map_err(|e| e.to_string());
                let _ = tx.send(result);
            });
            let app = app.clone();
            let dialog = dialog.clone();
            glib::timeout_add_local(Duration::from_millis(200), move || match rx.try_recv() {
                Ok(Ok(install)) => {
                    {
                        let mut cfg = app.config.borrow_mut();
                        cfg.add_install(install);
                        let _ = cfg.save();
                    }
                    refresh_miners(&app);
                    refresh_preset_editor(&app);
                    app.toast.add_toast(adw::Toast::new(&tr(
                        "Miner installed. It is available in every preset.",
                        "Майнер установлен. Он доступен во всех пресетах.",
                    )));
                    dialog.close();
                    glib::ControlFlow::Break
                }
                Ok(Err(error)) => {
                    app.toast.add_toast(adw::Toast::new(&error));
                    dialog.close();
                    glib::ControlFlow::Break
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    app.toast.add_toast(adw::Toast::new(&tr(
                        "Download thread stopped unexpectedly.",
                        "Поток загрузки неожиданно завершился.",
                    )));
                    dialog.close();
                    glib::ControlFlow::Break
                }
            });
        });
    }
    dialog.present();
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
    app.settings.poll.set_value(behavior.poll_interval_s as f64);
    app.settings.language.set_selected(match behavior.language {
        i18n::Language::System => 0,
        i18n::Language::En => 1,
        i18n::Language::Ru => 2,
    });
    if let Some(profile) = cfg.gpu_profile() {
        app.settings
            .power
            .set_value(profile.default.power_limit_w.unwrap_or(0) as f64);
        app.settings
            .core
            .set_value(profile.default.core_clock_mhz.unwrap_or(0) as f64);
        app.settings
            .memory
            .set_value(profile.default.memory_clock_mhz.unwrap_or(0) as f64);
        app.settings
            .temp_limit
            .set_value(profile.default.temp_limit_c as f64);
        app.settings
            .temp_resume
            .set_value(profile.default.temp_resume_c as f64);
    }
    if let Some(settings) = extension_settings() {
        let mode = settings.string("panel-mode");
        app.settings.panel_mode.set_selected(match mode.as_str() {
            "icon" => 0,
            "hashrate" => 1,
            "temperature" => 2,
            _ => 3,
        });
        app.settings
            .quick_toggle
            .set_active(settings.boolean("quick-toggle-enabled"));
        app.settings
            .live_activity
            .set_active(settings.boolean("live-activity-enabled"));
    } else {
        app.settings.panel_mode.set_sensitive(false);
        app.settings.quick_toggle.set_sensitive(false);
        app.settings.live_activity.set_sensitive(false);
    }
    refresh_extension_state(app);
}

fn save_settings(app: &Rc<App>) {
    let mut cfg = app.config.borrow_mut();
    cfg.behavior.auto_restart = app.settings.auto_restart.is_active();
    cfg.behavior.restore_stock_on_stop = app.settings.restore_stock.is_active();
    cfg.behavior.notifications = app.settings.notifications.is_active();
    cfg.behavior.start_mining_on_login = app.settings.start_login.is_active();
    cfg.behavior.poll_interval_s = app.settings.poll.value() as u64;
    cfg.behavior.language = match app.settings.language.selected() {
        1 => i18n::Language::En,
        2 => i18n::Language::Ru,
        _ => i18n::Language::System,
    };
    let profile = cfg.gpu_profile_mut();
    profile.default.power_limit_w = nonzero(app.settings.power.value() as u32);
    profile.default.core_clock_mhz = nonzero(app.settings.core.value() as u32);
    profile.default.memory_clock_mhz = nonzero(app.settings.memory.value() as u32);
    profile.default.temp_limit_c = app.settings.temp_limit.value() as u32;
    profile.default.temp_resume_c = app.settings.temp_resume.value() as u32;
    set_autostart_unit(app.settings.start_login.is_active());
    if let Some(settings) = extension_settings() {
        let mode = match app.settings.panel_mode.selected() {
            0 => "icon",
            1 => "hashrate",
            2 => "temperature",
            _ => "both",
        };
        let _ = settings.set_string("panel-mode", mode);
        let _ = settings.set_boolean(
            "quick-toggle-enabled",
            app.settings.quick_toggle.is_active(),
        );
        let _ = settings.set_boolean(
            "live-activity-enabled",
            app.settings.live_activity.is_active(),
        );
    }
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

fn save_editor(app: &Rc<App>) {
    let id = app.editor.current_id.borrow().clone();
    let miner_id = catalog::miners()
        .get(app.editor.miner.selected() as usize)
        .map(|m| m.id.clone())
        .unwrap_or_else(|| "wildrig".into());
    let version = catalog::miners()
        .get(app.editor.miner.selected() as usize)
        .and_then(|m| m.versions.get(app.editor.version.selected() as usize))
        .map(|v| v.version.clone())
        .unwrap_or_else(|| "0.51.8".into());
    let mut options = BTreeMap::new();
    for row in app.editor.options.borrow().iter() {
        options.insert(row.key.clone(), row.value());
    }
    let pool = catalog::pools()
        .get(app.editor.pool.selected() as usize)
        .map(|p| p.id.clone())
        .unwrap_or_else(|| "pearlhash".into());
    let mut cfg = app.config.borrow_mut();
    if let Some(preset) = cfg.presets.get_mut(&id) {
        preset.name = app.editor.name.text().to_string();
        preset.pool = pool;
        preset.pool_url = app.editor.pool_url.text().to_string();
        preset.pool_fee_pct = app.editor.pool_fee.value();
        preset.miner = miner_id.clone();
        preset.miner_version = version;
        preset.wallet = app.editor.wallet.text().to_string();
        preset.worker = app.editor.worker.text().to_string();
        preset.extra_args = app.editor.extra.text().to_string();
        preset.miner_options = options;
        let def = catalog::miner(&miner_id);
        preset.miner_fee_pct = def.map(|m| m.fee_pct).unwrap_or(0.0);
        preset.miner_fee_override_pct = if def.map(|m| m.fee_configurable).unwrap_or(false) {
            Some(app.editor.miner_fee.value())
        } else {
            None
        };
        preset.updated_at = util::now_unix();
    }
    drop(cfg);
    mark_dirty(app);
    refresh_preset_list(app);
    refresh_miners(app);
}

fn new_preset(app: &Rc<App>) {
    let mut cfg = app.config.borrow_mut();
    let base = cfg.preset().cloned().unwrap_or_else(Preset::new_pearl);
    let id = format!("preset-{}", util::now_unix());
    let mut preset = base;
    preset.id = id.clone();
    preset.name = tr("New preset", "Новый пресет");
    preset.created_at = util::now_unix();
    preset.updated_at = preset.created_at;
    cfg.presets.insert(id.clone(), preset);
    cfg.active_preset = Some(id.clone());
    drop(cfg);
    load_preset_into_editor(app, &id);
    app.editor.stack.set_visible_child_name("editor");
    mark_dirty(app);
    refresh_preset_list(app);
}

fn duplicate_preset(app: &Rc<App>) {
    let id = app.editor.current_id.borrow().clone();
    let mut cfg = app.config.borrow_mut();
    let Some(base) = cfg.presets.get(&id).cloned() else {
        return;
    };
    let new_id = format!("preset-{}", util::now_unix());
    let mut copy = base;
    copy.id = new_id.clone();
    copy.name = format!("{} {}", copy.name, tr("(copy)", "(копия)"));
    copy.created_at = util::now_unix();
    copy.updated_at = copy.created_at;
    cfg.presets.insert(new_id.clone(), copy);
    cfg.active_preset = Some(new_id.clone());
    drop(cfg);
    load_preset_into_editor(app, &new_id);
    app.editor.stack.set_visible_child_name("editor");
    mark_dirty(app);
    refresh_preset_list(app);
}

fn delete_preset(app: &Rc<App>) {
    let id = app.editor.current_id.borrow().clone();
    let mut cfg = app.config.borrow_mut();
    if cfg.presets.len() <= 1 {
        drop(cfg);
        app.toast.add_toast(adw::Toast::new(&tr(
            "At least one preset is required.",
            "Нужен хотя бы один пресет.",
        )));
        return;
    }
    cfg.presets.remove(&id);
    if cfg.active_preset.as_deref() == Some(id.as_str()) {
        cfg.active_preset = cfg.presets.keys().next().cloned();
    }
    let next = cfg.active_preset.clone().unwrap_or_default();
    drop(cfg);
    load_preset_into_editor(app, &next);
    app.editor.stack.set_visible_child_name("list");
    mark_dirty(app);
    refresh_preset_list(app);
}

fn save_and_apply(app: &Rc<App>) {
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
                });
            }
        });
    } else {
        let json = serde_json::to_string(&draft).unwrap_or_default();
        let _ = app.tx.send(Cmd::Apply {
            json,
            mode: "live".into(),
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
                *app.config.borrow_mut() = cfg;
                load_settings(app);
                refresh_preset_list(app);
                refresh_miners(app);
            }
        }
        Ev::ServiceDown(message) => {
            app.hero_line
                .set_text(&tr("Service is not running", "Сервис не запущен"));
            app.hero_line.set_tooltip_text(Some(&message));
            app.action_button.set_sensitive(false);
            app.pill.set_text(&tr("offline", "нет связи"));
        }
        Ev::Error(message) => {
            app.toast.add_toast(adw::Toast::new(&message));
        }
        Ev::Applied(message) => {
            app.dirty.set(false);
            update_dirty(app);
            app.toast
                .add_toast(adw::Toast::new(&tr("Applied", "Применено")));
            let _ = message;
            let _ = app.tx.send(Cmd::Refresh);
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

fn apply_status(app: &Rc<App>, status: Status) {
    let running = status.state.running;
    app.action_button.set_sensitive(true);
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
    let pill_text = if running {
        if status.state.external {
            tr("external", "вручную")
        } else {
            tr("mining", "майнится")
        }
    } else {
        tr("stopped", "остановлен")
    };
    app.pill.set_text(&pill_text);
    let hashrate = status
        .total
        .hashrate
        .or(status.miner.primary())
        .map(util::format_hashrate)
        .unwrap_or_else(|| "—".into());
    app.hero_hashrate.set_text(&hashrate);
    app.hero_line.set_text(&format!(
        "{} · {} {} · {:.2}%",
        status.state.preset_name,
        status.miner_info.name,
        status.miner_info.version,
        status.miner_info.total_fee_pct
    ));
    app.hero_meta.set_text(&format!(
        "{}: {} · {}: {} · {}",
        tr("uptime", "аптайм"),
        util::human_duration(status.state.uptime_s),
        tr("restarts", "перезапусков"),
        status.state.restarts,
        status.miner.source
    ));
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
    app.shares.set_subtitle(&format!(
        "{} / {}",
        status.miner.accepted.unwrap_or(0),
        status.miner.rejected.unwrap_or(0)
    ));
    if let Some(pool) = &status.pool {
        app.pool_balance.set_subtitle(&format!(
            "{} PRL",
            pool.balance
                .map(|v| format!("{v:.3}"))
                .unwrap_or_else(|| "—".into())
        ));
        app.pool_meta.set_subtitle(&format!(
            "{} · {:.2}%",
            status.state.preset_name, pool.fee_pct
        ));
    } else {
        app.pool_balance.set_subtitle("—");
        app.pool_meta.set_subtitle("—");
    }
    if status.warnings.is_empty() {
        app.warnings.set_title(&tr("No notes", "Замечаний нет"));
    } else {
        app.warnings.set_title(&status.warnings.join(" · "));
    }

    let signature = status
        .gpus
        .iter()
        .map(|gpu| {
            format!(
                "{}:{:?}:{:?}:{:?}",
                gpu.uuid, gpu.temp_c, gpu.power_w, gpu.util_pct
            )
        })
        .collect::<Vec<_>>()
        .join("|");
    if *app.gpu_sig.borrow() != signature {
        *app.gpu_sig.borrow_mut() = signature;
        clear_box(&app.gpu_box);
        if status.gpus.is_empty() {
            let label = gtk::Label::new(Some(&tr(
                "No NVIDIA GPU detected",
                "NVIDIA GPU не обнаружен",
            )));
            label.add_css_class("dim-label");
            app.gpu_box.append(&label);
        }
        for gpu in &status.gpus {
            let row = adw::ActionRow::builder()
                .title(format!("GPU #{} · {}", gpu.index, gpu.name))
                .subtitle(format!(
                    "{} °C · {} W / {} W · {}% · {} / {} MHz · {}",
                    gpu.temp_c
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "—".into()),
                    gpu.power_w
                        .map(|v| format!("{v:.0}"))
                        .unwrap_or_else(|| "—".into()),
                    gpu.power_limit_w
                        .map(|v| format!("{v:.0}"))
                        .unwrap_or_else(|| "—".into()),
                    gpu.fan_pct
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "—".into()),
                    gpu.core_mhz
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "—".into()),
                    gpu.mem_mhz
                        .map(|v| v.to_string())
                        .unwrap_or_else(|| "—".into()),
                    gpu.uuid
                ))
                .build();
            let tune = gtk::Button::builder()
                .label(tr("Tune", "Настроить"))
                .css_classes(["flat"])
                .build();
            let app_tune = app.clone();
            let uuid = gpu.uuid.clone();
            let name = gpu.name.clone();
            tune.connect_clicked(move |_| gpu_tune_dialog(&app_tune, &uuid, &name));
            let toggle = gtk::Switch::new();
            toggle.set_active(
                app.config
                    .borrow()
                    .gpu_profile()
                    .map(|p| p.tuning_for(&gpu.uuid).enabled)
                    .unwrap_or(true),
            );
            toggle.set_valign(gtk::Align::Center);
            {
                let app = app.clone();
                let uuid = gpu.uuid.clone();
                toggle.connect_notify_local(Some("active"), move |toggle, _| {
                    let mut cfg = app.config.borrow_mut();
                    let profile = cfg.gpu_profile_mut();
                    let mut tuning = profile.tuning_for(&uuid);
                    tuning.enabled = toggle.is_active();
                    profile.per_gpu.insert(uuid.clone(), tuning);
                    drop(cfg);
                    app.dirty.set(true);
                    update_dirty(&app);
                });
            }
            row.add_suffix(&tune);
            row.add_suffix(&toggle);
            app.gpu_box.append(&row);
        }
    }
    *app.status.borrow_mut() = Some(status);
}

// --------------------------------------------------------------- screenshot

fn save_window_png(window: &adw::ApplicationWindow, path: &str) -> anyhow::Result<()> {
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
