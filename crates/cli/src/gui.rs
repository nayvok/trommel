//! GTK4 + libadwaita приложение: дашборд, настройки, сравнение майнеров.
//!
//! Вся работа с майнером идёт через фоновый сервис `org.pearl.Miner.Daemon`
//! (D-Bus): приложение никогда не запускает nvidia-smi само.
//!
//! Оформление — по гайдлайнам GNOME: адаптивная шапка (`AdwViewSwitcherTitle`
//! плюс нижний `AdwViewSwitcherBar`), цвета и акцент берутся из темы
//! (`@accent_color`, `@warning_color` и т.п.), ничего не захардкожено под
//! светлую или тёмную тему.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use adw::prelude::*;

use gtk::{gdk, gio, glib};

use pearl_miner_core::bench::BenchReport;
use pearl_miner_core::config::Config;
use pearl_miner_core::dbus::MinerProxy;
use pearl_miner_core::paths;
use pearl_miner_core::status::Status;
use pearl_miner_core::util;

// ------------------------------------------------------------------ каналы

#[derive(Debug)]
#[allow(dead_code)] // Start/Stop используются CLI, приложению достаточно Toggle
enum Cmd {
    Start,
    Stop,
    Toggle,
    Refresh,
    GetConfig,
    SetConfig(String),
    ResetOc,
    Bench { profiles: Vec<String>, seconds: u64 },
}

#[derive(Debug)]
enum Ev {
    Status(String),
    Config(String),
    Bench(String),
    Info(String),
    Error(String),
    ServiceDown(String),
}

/// Запускает фоновый поток с tokio и подключением к демону.
fn spawn_worker(emit: Arc<dyn Fn(Ev) + Send + Sync>) -> tokio::sync::mpsc::UnboundedSender<Cmd> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Cmd>();
    std::thread::Builder::new()
        .name("pearl-dbus".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(err) => {
                    emit(Ev::Error(format!("не удалось создать runtime: {err}")));
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
                                proxy = connect().await.map_err(|err| {
                                    emit(Ev::ServiceDown(err.to_string()));
                                    err
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
                                emit(Ev::ServiceDown(
                                    "сервис pearl-minerd не запущен".into()
                                ));
                                continue;
                            };
                            let result = handle(&client, command).await;
                            match result {
                                Ok(ev) => { if let Some(ev) = ev { emit(ev) } }
                                Err(err) => emit(Ev::Error(err.to_string())),
                            }
                        }
                    }
                }
            });
        })
        .expect("не удалось запустить фоновый поток");
    tx
}

async fn connect() -> anyhow::Result<MinerProxy<'static>> {
    let connection = zbus::Connection::session().await?;
    MinerProxy::new(&connection).await.map_err(|e| e.into())
}

async fn handle(client: &MinerProxy<'_>, command: Cmd) -> anyhow::Result<Option<Ev>> {
    Ok(match command {
        Cmd::Start => {
            client.start().await?;
            Some(Ev::Info("Майнинг запущен".into()))
        }
        Cmd::Stop => {
            client.stop().await?;
            Some(Ev::Info("Майнинг остановлен".into()))
        }
        Cmd::Toggle => {
            let running = client.toggle().await?;
            Some(Ev::Info(if running {
                "Майнинг запущен".into()
            } else {
                "Майнинг остановлен".into()
            }))
        }
        Cmd::Refresh => Some(Ev::Status(client.refresh().await?)),
        Cmd::GetConfig => Some(Ev::Config(client.get_config().await?)),
        Cmd::SetConfig(json) => {
            client.set_config(&json).await?;
            Some(Ev::Info("Настройки сохранены".into()))
        }
        Cmd::ResetOc => {
            client.reset_oc().await?;
            Some(Ev::Info(
                "Частоты и питание возвращены в сток (350 W)".into(),
            ))
        }
        Cmd::Bench { profiles, seconds } => {
            let request = serde_json::json!({
                "profiles": profiles,
                "seconds": seconds,
            });
            Some(Ev::Bench(client.run_benchmark(&request.to_string()).await?))
        }
    })
}

// ------------------------------------------------------------------- строки

/// Строка «подпись → значение» в списке.
struct ValueRow {
    row: adw::ActionRow,
    value: gtk::Label,
}

impl ValueRow {
    fn new(title: &str, subtitle: &str) -> Self {
        let value = gtk::Label::builder()
            .label("—")
            .css_classes(["numeric", "dim-label"])
            .build();
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(subtitle)
            .build();
        row.add_suffix(&value);
        Self { row, value }
    }

    fn set(&self, text: impl Into<String>) {
        self.value.set_label(&text.into());
        self.value.remove_css_class("dim-label");
    }

    fn set_dim(&self, text: impl Into<String>) {
        self.value.set_label(&text.into());
        self.value.add_css_class("dim-label");
    }
}

/// Строка с полосой прогресса. `GtkProgressBar` вместо `GtkLevelBar`: у второго
/// своя семантика «предупреждений» и он сам краснеет у максимума.
struct MeterRow {
    row: adw::ActionRow,
    bar: gtk::ProgressBar,
    value: gtk::Label,
}

/// Тон полосы: обычный (акцент), предупреждение, ошибка.
#[derive(Clone, Copy, PartialEq)]
enum Tone {
    Cool,
    Warm,
    Hot,
}

impl MeterRow {
    fn new(title: &str, subtitle: &str) -> Self {
        let bar = gtk::ProgressBar::builder()
            .valign(gtk::Align::Center)
            .width_request(120)
            .fraction(0.0)
            .css_classes(["pearl-meter"])
            .build();
        let value = gtk::Label::builder()
            .label("—")
            .css_classes(["numeric"])
            .build();
        let row = adw::ActionRow::builder()
            .title(title)
            .subtitle(subtitle)
            .build();
        row.add_suffix(&value);
        row.add_suffix(&bar);
        Self { row, bar, value }
    }

    fn set(&self, fraction: f64, text: impl Into<String>) {
        self.bar.set_fraction(fraction.clamp(0.0, 1.0));
        self.value.set_label(&text.into());
    }

    fn set_tone(&self, tone: Tone) {
        for class in ["pearl-cool", "pearl-warm", "pearl-hot"] {
            self.bar.remove_css_class(class);
        }
        self.bar.add_css_class(match tone {
            Tone::Cool => "pearl-cool",
            Tone::Warm => "pearl-warm",
            Tone::Hot => "pearl-hot",
        });
    }

    /// Полоса не нужна, когда данных нет — пустой серый прямоугольник только шумит.
    fn show_bar(&self, visible: bool) {
        self.bar.set_visible(visible);
    }
}

// ------------------------------------------------------------- приложение

struct App {
    window: adw::ApplicationWindow,
    toast: adw::ToastOverlay,
    stack: adw::ViewStack,
    /// Заголовок в шапке (с профилем и комиссией в подзаголовке).
    /// `AdwViewSwitcherTitle` помечен в libadwaita 1.4 как deprecated, но это
    /// по-прежнему единственный готовый адаптивный вариант: он сам прячет табы
    /// в шапке и показывает нижний переключатель, когда окно узкое.
    #[allow(deprecated)]
    title: adw::ViewSwitcherTitle,
    /// «content» | «offline»
    shell: gtk::Stack,
    banner: adw::Banner,
    tx: tokio::sync::mpsc::UnboundedSender<Cmd>,
    status: RefCell<Option<Status>>,
    config: RefCell<Config>,
    history: RefCell<Vec<(f64, Option<u32>)>>,
    /// Внутренняя синхронизация полей: не помечать «изменено».
    syncing: Cell<bool>,
    dirty: Cell<bool>,

    // шапка
    action_button: gtk::Button,
    action_label: gtk::Label,

    // дашборд
    pill: gtk::Label,
    uptime: gtk::Label,
    hero_hashrate: gtk::Label,
    hero_windows: gtk::Label,
    load_bar: gtk::ProgressBar,
    load_value: gtk::Label,
    spark: gtk::DrawingArea,
    spark_mode: RefCell<SparkMode>,

    rows: DashRows,
    settings: SettingsWidgets,
    bench: BenchWidgets,
}

#[derive(Clone, Copy, PartialEq)]
enum SparkMode {
    Hashrate,
    Temp,
}

struct DashRows {
    temp: MeterRow,
    power: MeterRow,
    fan: ValueRow,
    clocks: ValueRow,
    utilization: ValueRow,
    gpu_name: ValueRow,
    accepted: ValueRow,
    rejected: ValueRow,
    acceptance: ValueRow,
    balance: ValueRow,
    immature: ValueRow,
    paid: ValueRow,
    pool_hashrate: ValueRow,
    warnings: ValueRow,
}

struct SettingsWidgets {
    power_limit: adw::SpinRow,
    stock_power_limit: adw::SpinRow,
    core_clock: adw::SpinRow,
    memory_clock: adw::SpinRow,
    fan_speed: adw::SpinRow,
    temp_limit: adw::SpinRow,
    temp_resume: adw::SpinRow,
    wallet: adw::EntryRow,
    pool: adw::EntryRow,
    worker: adw::EntryRow,
    algo: adw::EntryRow,
    api_port: adw::SpinRow,
    expected: adw::SpinRow,
    auto_restart: adw::SwitchRow,
    notifications: adw::SwitchRow,
    autostart: adw::SwitchRow,
    poll_interval: adw::SpinRow,
    apply_button: gtk::Button,
    reset_oc_button: gtk::Button,
    open_log_button: gtk::Button,
}

struct BenchWidgets {
    profile_combo: adw::ComboRow,
    apply_profile_button: gtk::Button,
    wildrig_path: adw::EntryRow,
    srbminer_path: adw::EntryRow,
    onezero_path: adw::EntryRow,
    save_paths_button: gtk::Button,
    minutes: adw::SpinRow,
    run_button: gtk::Button,
    entries: Vec<BenchEntryRow>,
}

struct BenchEntryRow {
    id: String,
    row: adw::ActionRow,
    apply: gtk::Button,
}

// ---------------------------------------------------------------- запуск

pub fn run() -> anyhow::Result<()> {
    let app = adw::Application::builder()
        .application_id(pearl_miner_core::APP_ID)
        .build();
    // Тёмная/светлая тема и акцент — системные: libadwaita подхватывает их сам,
    // принудительно ничего не выставляем (иначе ломается выбор пользователя).
    app.connect_activate(build_ui);
    // Не передаём «gui» в GApplication: иначе GTK воспримет слово как файл.
    app.run_with_args(&["pearl-miner"]);
    Ok(())
}

fn build_ui(app: &adw::Application) {
    // Отладочный форс темы (PEARL_MINER_COLOR_SCHEME=dark|light) — нужен, чтобы
    // скриншоты в docs/ снимались детерминированно в обеих темах.
    if let Ok(scheme) = std::env::var("PEARL_MINER_COLOR_SCHEME") {
        adw::StyleManager::default().set_color_scheme(match scheme.as_str() {
            "dark" => adw::ColorScheme::ForceDark,
            "light" => adw::ColorScheme::ForceLight,
            _ => adw::ColorScheme::Default,
        });
    }

    // События из фонового потока приходят по каналу, а в GTK попадают
    // через таймер главного цикла (Rc нельзя передавать между потоками).
    let (event_tx, event_rx) = std::sync::mpsc::channel::<Ev>();
    let event_tx = Arc::new(event_tx);
    let emit: Arc<dyn Fn(Ev) + Send + Sync> = Arc::new(move |event| {
        let _ = event_tx.send(event);
    });

    let tx = spawn_worker(emit);
    let app_state = build_app(app, tx);
    APP_SLOT.with(|slot| *slot.borrow_mut() = Some(app_state.clone()));

    install_actions(app, &app_state);

    {
        let app_state = app_state.clone();
        glib::timeout_add_local(Duration::from_millis(60), move || {
            while let Ok(event) = event_rx.try_recv() {
                app_state.handle_event(event);
            }
            glib::ControlFlow::Continue
        });
    }

    app_state.window.present();
    // Иначе GTK сам ставит фокус на первую спин-строку и она светится рамкой.
    gtk::prelude::GtkWindowExt::set_focus(&app_state.window, None::<&gtk::Widget>);
    app_state.refresh_config();
    let _ = app_state.tx.send(Cmd::Refresh);

    // Отладочный снимок окна в PNG: PEARL_MINER_SCREENSHOT=/путь/файл.png
    // (используется при проверке вёрстки, никак не влияет на обычную работу).
    if let Some(path) = std::env::var_os("PEARL_MINER_SCREENSHOT") {
        let window = app_state.window.clone();
        let stack = app_state.stack.clone();
        let application = app.clone();
        let path = path.to_string_lossy().into_owned();
        if let Ok(page) = std::env::var("PEARL_MINER_SCREENSHOT_PAGE") {
            stack.set_visible_child_name(&page);
        }
        // Задержка нужна, чтобы спарклайн успел набрать историю.
        let delay = std::env::var("PEARL_MINER_SCREENSHOT_DELAY")
            .ok()
            .and_then(|value| value.parse::<u32>().ok())
            .unwrap_or(4);
        glib::timeout_add_seconds_local(delay, move || {
            // Первая отрисовка в свежей сессии иногда приходит с пустым
            // render node — пробуем несколько раз.
            let mut saved = false;
            let mut last = String::new();
            for attempt in 0..12 {
                match save_window_png(&window, &path) {
                    Ok(()) => {
                        eprintln!("снимок окна сохранён: {path}");
                        saved = true;
                        break;
                    }
                    Err(err) => {
                        last = err.to_string();
                        if attempt < 11 {
                            std::thread::sleep(Duration::from_millis(250));
                        }
                    }
                }
            }
            if !saved {
                eprintln!("не удалось сохранить снимок окна: {last}");
            }
            // Скриншот — служебный режим: сразу выходим, не висим таймаутом.
            application.quit();
            glib::ControlFlow::Break
        });
    }
}

/// Действия приложения: «О программе», «Открыть лог», «Выход».
fn install_actions(app: &adw::Application, state: &Rc<App>) {
    let about = gio::SimpleAction::new("about", None);
    {
        let window = state.window.clone();
        about.connect_activate(move |_, _| {
            adw::AboutDialog::builder()
                .application_name("Pearl Miner")
                .application_icon(pearl_miner_core::APP_ID)
                .developer_name("Pearl Miner")
                .version(pearl_miner_core::VERSION)
                .comments("Майнинг Pearl (PRL): старт/стоп, статистика и разгон из одного окна.")
                .website("https://pearlhash.xyz")
                .build()
                .present(Some(&window));
        });
    }
    app.add_action(&about);

    let log = gio::SimpleAction::new("log", None);
    log.connect_activate(|_, _| {
        let path = paths::log_path();
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

/// Рендерит окно в PNG (для проверки внешнего вида без скриншотов экрана).
fn save_window_png(window: &adw::ApplicationWindow, path: &str) -> anyhow::Result<()> {
    let width = window.width();
    let height = window.height();
    if width <= 0 || height <= 0 {
        anyhow::bail!("окно ещё не отрисовано");
    }
    let paintable = gtk::WidgetPaintable::new(Some(window));
    let snapshot = gtk::Snapshot::new();
    paintable.snapshot(&snapshot, width as f64, height as f64);
    let node = snapshot
        .to_node()
        .ok_or_else(|| anyhow::anyhow!("пустой render node"))?;
    let renderer = window
        .renderer()
        .ok_or_else(|| anyhow::anyhow!("окно не имеет рендерера"))?;
    let texture = renderer.render_texture(&node, None);
    texture
        .save_to_png(path)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(())
}

/// CSS: только то, чего нет в Adwaita, и на именах цветов темы.
fn install_css() {
    let provider = gtk::CssProvider::new();
    provider.load_from_string(
        r#"
        progressbar.pearl-meter > trough > progress { background-color: @accent_bg_color; }
        progressbar.pearl-cool  > trough > progress { background-color: @accent_bg_color; }
        progressbar.pearl-warm  > trough > progress { background-color: @warning_color; }
        progressbar.pearl-hot   > trough > progress { background-color: @error_color; }
        progressbar.pearl-meter > trough { min-height: 6px; border-radius: 999px; }
        progressbar.pearl-meter > trough > progress { min-height: 6px; border-radius: 999px; }

        label.numeric { font-feature-settings: "tnum"; }
        label.pearl-pill-label {
            padding: 2px 10px;
            border-radius: 999px;
            background-color: alpha(currentColor, 0.12);
        }
        label.pearl-hero { font-weight: 800; }
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

/// Цвет из темы по имени (`accent_bg_color`, `window_fg_color`, …).
#[allow(deprecated)]
fn theme_color<W: IsA<gtk::Widget>>(widget: &W, name: &str) -> gdk::RGBA {
    widget
        .style_context()
        .lookup_color(name)
        .unwrap_or_else(|| gdk::RGBA::new(0.5, 0.5, 0.5, 1.0))
}

fn build_app(app: &adw::Application, tx: tokio::sync::mpsc::UnboundedSender<Cmd>) -> Rc<App> {
    install_css();
    let stack = adw::ViewStack::new();
    let toast = adw::ToastOverlay::new();
    toast.set_child(Some(&stack));

    // Адаптивная шапка: при достаточной ширине — табы в заголовке, при узкой —
    // обычный заголовок и переключатель внизу (гайдлайны GNOME, AdwViewSwitcherTitle).
    #[allow(deprecated)]
    let title = adw::ViewSwitcherTitle::builder()
        .stack(&stack)
        .title("Pearl Miner")
        .subtitle("Pearl (PRL) · WildRig")
        .build();
    let switcher_bar = adw::ViewSwitcherBar::builder().stack(&stack).build();
    title
        .bind_property("title-visible", &switcher_bar, "reveal")
        .sync_create()
        .build();

    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&title));

    let refresh_button = gtk::Button::builder()
        .icon_name("view-refresh-symbolic")
        .tooltip_text("Обновить")
        .build();
    header.pack_start(&refresh_button);

    let menu_model = gio::Menu::new();
    menu_model.append(Some("О программе"), Some("app.about"));
    menu_model.append(Some("Открыть лог"), Some("app.log"));
    menu_model.append(Some("Выход"), Some("app.quit"));
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&menu_model)
        .tooltip_text("Меню")
        .build();
    header.pack_end(&menu_button);

    let action_label = gtk::Label::builder().label("Запустить").build();
    let action_button = gtk::Button::builder()
        .css_classes(["suggested-action"])
        .child(&action_label)
        .tooltip_text("Запустить или остановить майнинг")
        .build();
    header.pack_end(&action_button);

    // Плашка «есть несохранённые изменения» — вместо молчаливой потери правок.
    let banner = adw::Banner::builder()
        .title("Есть несохранённые изменения")
        .button_label("Применить")
        .build();
    banner.set_revealed(false);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.add_bottom_bar(&switcher_bar);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&banner);
    content.append(&toast);
    toolbar.set_content(Some(&content));

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title("Pearl Miner")
        .default_width(560)
        .default_height(780)
        .content(&toolbar)
        .build();

    // ------------------------------------------------------- дашборд
    let dashboard = adw::PreferencesPage::new();

    // --- герой: хешрейт, окна, статус, загрузка от ожидаемой
    let hero_group = adw::PreferencesGroup::new();
    let hero_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(10)
        .margin_bottom(12)
        .margin_start(6)
        .margin_end(6)
        .build();

    let hero_top = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .build();
    let hero_hashrate = gtk::Label::builder()
        .label("—")
        .css_classes(["title-1", "numeric", "pearl-hero", "dim-label"])
        .xalign(0.0)
        .hexpand(true)
        .build();
    let pill = gtk::Label::builder()
        .label("остановлен")
        .css_classes(["caption", "pearl-pill-label"])
        .valign(gtk::Align::Center)
        .build();
    hero_top.append(&hero_hashrate);
    hero_top.append(&pill);

    let hero_windows = gtk::Label::builder()
        .label("10s — · 60s — · 15m —")
        .css_classes(["caption", "dim-label", "numeric"])
        .xalign(0.0)
        .build();

    let uptime = gtk::Label::builder()
        .label("остановлен")
        .css_classes(["caption", "dim-label", "numeric"])
        .xalign(0.0)
        .build();

    let load_header = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_top(4)
        .build();
    let load_title = gtk::Label::builder()
        .label("Загрузка от ожидаемой")
        .css_classes(["caption", "dim-label"])
        .xalign(0.0)
        .hexpand(true)
        .build();
    let load_value = gtk::Label::builder()
        .label("—")
        .css_classes(["caption", "dim-label", "numeric"])
        .build();
    load_header.append(&load_title);
    load_header.append(&load_value);

    let load_bar = gtk::ProgressBar::builder()
        .fraction(0.0)
        .css_classes(["pearl-meter"])
        .build();

    hero_box.append(&hero_top);
    hero_box.append(&hero_windows);
    hero_box.append(&uptime);
    hero_box.append(&load_header);
    hero_box.append(&load_bar);
    hero_group.add(&hero_box);
    dashboard.add(&hero_group);

    // --- активность (спарклайн)
    let chart_group = adw::PreferencesGroup::builder().title("Активность").build();
    let chart_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(6)
        .margin_bottom(10)
        .margin_start(6)
        .margin_end(6)
        .build();

    let mode_group = adw::ToggleGroup::new();
    mode_group.add(
        adw::Toggle::builder()
            .label("Хешрейт")
            .name("hashrate")
            .build(),
    );
    mode_group.add(
        adw::Toggle::builder()
            .label("Температура")
            .name("temp")
            .build(),
    );
    mode_group.set_active_name(Some("hashrate"));
    mode_group.add_css_class("round");
    mode_group.set_halign(gtk::Align::Start);

    let spark = gtk::DrawingArea::builder()
        .content_height(110)
        .hexpand(true)
        .build();

    chart_box.append(&mode_group);
    chart_box.append(&spark);
    chart_group.add(&chart_box);
    dashboard.add(&chart_group);

    // --- GPU
    let gpu_group = adw::PreferencesGroup::builder().title("Видеокарта").build();
    let temp = MeterRow::new("Температура", "");
    let power = MeterRow::new("Мощность", "");
    let fan = ValueRow::new("Вентилятор", "");
    let clocks = ValueRow::new("Частоты", "ядро / память");
    let utilization = ValueRow::new("Загрузка GPU", "");
    let gpu_name = ValueRow::new("Модель", "");
    temp.show_bar(false);
    power.show_bar(false);
    for row in [
        &gpu_name.row,
        &temp.row,
        &power.row,
        &fan.row,
        &clocks.row,
        &utilization.row,
    ] {
        gpu_group.add(row);
    }
    dashboard.add(&gpu_group);

    // --- шары
    let shares_group = adw::PreferencesGroup::builder().title("Шары").build();
    let accepted = ValueRow::new("Принято", "");
    let rejected = ValueRow::new("Отклонено", "");
    let acceptance = ValueRow::new("Доля принятых", "");
    for row in [&accepted.row, &rejected.row, &acceptance.row] {
        shares_group.add(row);
    }
    dashboard.add(&shares_group);

    // --- пул
    let pool_group = adw::PreferencesGroup::builder()
        .title("Пул PearlHash")
        .build();
    let balance = ValueRow::new("Накоплено", "ещё не выплачено, PRL");
    let immature = ValueRow::new("Незрелые эпохи", "PRL");
    let paid = ValueRow::new("Выплачено всего", "PRL");
    let pool_hashrate = ValueRow::new("Хешрейт на пуле", "");
    let warnings = ValueRow::new("Замечания", "");
    for row in [
        &balance.row,
        &immature.row,
        &paid.row,
        &pool_hashrate.row,
        &warnings.row,
    ] {
        pool_group.add(row);
    }
    dashboard.add(&pool_group);

    // --- «сервис не запущен» вместо простыни пустых строк
    let offline = adw::StatusPage::builder()
        .icon_name("network-offline-symbolic")
        .title("Сервис не запущен")
        .description(
            "Майнером управляет фоновый сервис pearl-minerd. \
             Запусти его — данные появятся сами.",
        )
        .build();
    let start_service_button = gtk::Button::builder()
        .label("Запустить сервис")
        .css_classes(["suggested-action", "pill"])
        .halign(gtk::Align::Center)
        .build();
    offline.set_child(Some(&start_service_button));

    let shell = gtk::Stack::new();
    shell.add_named(&dashboard, Some("content"));
    shell.add_named(&offline, Some("offline"));
    shell.set_visible_child_name("offline");
    stack.add_titled(&shell, Some("dashboard"), "Дашборд");
    set_page_icon(&stack, &shell, "utilities-system-monitor-symbolic");

    // ----------------------------------------------------- настройки
    let (settings_page, settings) = build_settings_page();
    stack.add_titled(&settings_page, Some("settings"), "Настройки");
    set_page_icon(&stack, &settings_page, "preferences-system-symbolic");

    // -------------------------------------------------------- майнеры
    let (miner_page, bench) = build_miner_page();
    stack.add_titled(&miner_page, Some("miner"), "Майнер");
    set_page_icon(&stack, &miner_page, "applications-engineering-symbolic");

    let app_state = App {
        window: window.clone(),
        toast: toast.clone(),
        stack: stack.clone(),
        title,
        shell: shell.clone(),
        banner: banner.clone(),
        tx: tx.clone(),
        status: RefCell::new(None),
        config: RefCell::new(Config::load_or_default()),
        history: RefCell::new(Vec::new()),
        syncing: Cell::new(false),
        dirty: Cell::new(false),
        action_button: action_button.clone(),
        action_label: action_label.clone(),
        pill,
        uptime,
        hero_hashrate,
        hero_windows,
        load_bar,
        load_value,
        spark,
        spark_mode: RefCell::new(SparkMode::Hashrate),
        rows: DashRows {
            temp,
            power,
            fan,
            clocks,
            utilization,
            gpu_name,
            accepted,
            rejected,
            acceptance,
            balance,
            immature,
            paid,
            pool_hashrate,
            warnings,
        },
        settings,
        bench,
    };
    let rc = Rc::new(app_state);

    // --- сигналы ---------------------------------------------------
    {
        let app = rc.clone();
        action_button.connect_clicked(move |_| {
            let _ = app.tx.send(Cmd::Toggle);
            app.set_action_busy();
        });
    }
    {
        let app = rc.clone();
        refresh_button.connect_clicked(move |_| {
            let _ = app.tx.send(Cmd::Refresh);
        });
    }
    {
        let tx = rc.tx.clone();
        start_service_button.connect_clicked(move |_| start_daemon_service(&tx));
    }
    {
        let app = Rc::downgrade(&rc);
        mode_group.connect_notify_local(Some("active-name"), move |group, _| {
            let Some(app) = app.upgrade() else { return };
            let mode = match group.active_name().as_deref() {
                Some("temp") => SparkMode::Temp,
                _ => SparkMode::Hashrate,
            };
            *app.spark_mode.borrow_mut() = mode;
            app.spark.queue_draw();
        });
    }
    {
        let app = rc.clone();
        rc.spark.set_draw_func(move |_, cr, width, height| {
            app.paint_spark(cr, width, height);
        });
    }
    {
        let app = Rc::downgrade(&rc);
        rc.settings.apply_button.connect_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                app.save_settings();
            }
        });
    }
    {
        let app = Rc::downgrade(&rc);
        rc.banner.connect_button_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                app.save_settings();
            }
        });
    }
    {
        let app = Rc::downgrade(&rc);
        rc.settings.reset_oc_button.connect_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                let _ = app.tx.send(Cmd::ResetOc);
            }
        });
    }
    {
        let app = Rc::downgrade(&rc);
        rc.settings.open_log_button.connect_clicked(move |_| {
            if app.upgrade().is_some() {
                let path = paths::log_path();
                let uri = gio::File::for_path(path).uri();
                let _ = gio::AppInfo::launch_default_for_uri(&uri, None::<&gio::AppLaunchContext>);
            }
        });
    }
    {
        let app = Rc::downgrade(&rc);
        rc.bench.run_button.connect_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                app.run_bench();
            }
        });
    }
    {
        let app = Rc::downgrade(&rc);
        rc.bench.apply_profile_button.connect_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                app.apply_profile_from_combo();
            }
        });
    }
    {
        let app = Rc::downgrade(&rc);
        rc.bench.save_paths_button.connect_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                app.save_miner_paths();
            }
        });
    }
    for entry in &rc.bench.entries {
        let app = Rc::downgrade(&rc);
        let id = entry.id.clone();
        entry.apply.connect_clicked(move |_| {
            if let Some(app) = app.upgrade() {
                app.set_profile(&id);
            }
        });
    }

    wire_dirty(&rc);

    // таймер «майнит N · перезапусков M»
    {
        let weak = Rc::downgrade(&rc);
        glib::timeout_add_seconds_local(1, move || {
            let Some(app) = weak.upgrade() else {
                return glib::ControlFlow::Break;
            };
            app.tick();
            glib::ControlFlow::Continue
        });
    }

    stack.set_visible_child_name("dashboard");
    rc.apply_config_to_widgets();

    rc
}

/// Иконка вкладки. Без неё libadwaita рисует заглушку `image-missing`.
fn set_page_icon(stack: &adw::ViewStack, child: &impl IsA<gtk::Widget>, icon: &str) {
    stack.page(child).set_icon_name(Some(icon));
}

thread_local! {
    /// Держим единственный экземпляр приложения живым на время работы окна.
    static APP_SLOT: RefCell<Option<Rc<App>>> = const { RefCell::new(None) };
}

/// Помечает правки в настройках как несохранённые.
fn wire_dirty(rc: &Rc<App>) {
    let mark: Rc<dyn Fn()> = {
        let weak = Rc::downgrade(rc);
        Rc::new(move || {
            if let Some(app) = weak.upgrade() {
                app.mark_dirty();
            }
        })
    };

    for row in [
        &rc.settings.power_limit,
        &rc.settings.stock_power_limit,
        &rc.settings.core_clock,
        &rc.settings.memory_clock,
        &rc.settings.fan_speed,
        &rc.settings.temp_limit,
        &rc.settings.temp_resume,
        &rc.settings.api_port,
        &rc.settings.expected,
        &rc.settings.poll_interval,
    ] {
        let mark = mark.clone();
        row.connect_value_notify(move |_| mark());
    }
    for row in [
        &rc.settings.wallet,
        &rc.settings.pool,
        &rc.settings.worker,
        &rc.settings.algo,
    ] {
        let mark = mark.clone();
        row.connect_changed(move |_| mark());
    }
    for row in [
        &rc.settings.auto_restart,
        &rc.settings.notifications,
        &rc.settings.autostart,
    ] {
        let mark = mark.clone();
        row.connect_active_notify(move |_| mark());
    }
}

/// Поднимает фоновый сервис (systemd --user, иначе бинарь из PATH).
fn start_daemon_service(tx: &tokio::sync::mpsc::UnboundedSender<Cmd>) {
    let tx = tx.clone();
    std::thread::spawn(move || {
        let started = std::process::Command::new("systemctl")
            .args(["--user", "start", "pearl-minerd.service"])
            .status()
            .map(|status| status.success())
            .unwrap_or(false);
        if !started {
            let _ = std::process::Command::new("pearl-minerd")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        }
        std::thread::sleep(Duration::from_millis(900));
        let _ = tx.send(Cmd::Refresh);
    });
}

// ------------------------------------------------------------ настройки

fn build_settings_page() -> (adw::PreferencesPage, SettingsWidgets) {
    let page = adw::PreferencesPage::new();

    let power_group = adw::PreferencesGroup::builder()
        .title("Питание и разгон")
        .description("0 в поле «не трогать» означает «оставить как есть».")
        .build();

    let power_limit = spin_row(
        "Лимит питания при майнинге",
        "Ватты (сток 350)",
        300.0,
        50.0,
        600.0,
    );
    let stock_power_limit = spin_row(
        "Стоковый лимит",
        "Ватты после остановки",
        350.0,
        50.0,
        600.0,
    );
    let core_clock = spin_row("Лок ядра", "МГц, 0 = не трогать", 1620.0, 0.0, 4000.0);
    let memory_clock = spin_row("Лок памяти", "МГц, 0 = не трогать", 810.0, 0.0, 12000.0);
    let fan_speed = spin_row("Вентилятор", "% (0 = авто)", 0.0, 0.0, 100.0);
    let temp_limit = spin_row("Порог температуры", "°C, выше — пауза", 90.0, 60.0, 100.0);
    let temp_resume = spin_row("Возобновление", "°C, ниже — снова майнит", 65.0, 40.0, 95.0);
    for row in [
        &power_limit,
        &stock_power_limit,
        &core_clock,
        &memory_clock,
        &fan_speed,
        &temp_limit,
        &temp_resume,
    ] {
        power_group.add(row);
    }
    page.add(&power_group);

    let miner_group = adw::PreferencesGroup::builder()
        .title("Майнинг")
        .description("Кошелёк, пул и воркер — то, что уходит в майнер.")
        .build();
    let wallet = adw::EntryRow::builder().title("Кошелёк").build();
    let pool = adw::EntryRow::builder().title("Пул").build();
    let worker = adw::EntryRow::builder().title("Воркер").build();
    let algo = adw::EntryRow::builder().title("Алгоритм").build();
    let api_port = spin_row(
        "Порт API майнера",
        "0 = читать из лога",
        4028.0,
        0.0,
        65535.0,
    );
    let expected = spin_row("Ожидаемый хешрейт", "TH/s, для шкалы", 125.0, 1.0, 5000.0);
    for row in [&wallet, &pool, &worker, &algo] {
        miner_group.add(row);
    }
    miner_group.add(&api_port);
    miner_group.add(&expected);
    page.add(&miner_group);

    let behaviour_group = adw::PreferencesGroup::builder().title("Поведение").build();
    let auto_restart = switch_row("Перезапускать при падении", "Как в start-mining.sh");
    let notifications = switch_row("Оповещения", "Старт, стоп, перегрев");
    let autostart = switch_row("Начинать майнинг при входе", "systemd --user unit");
    let poll_interval = spin_row("Интервал опроса", "Секунды", 2.0, 1.0, 60.0);
    for row in [&auto_restart, &notifications, &autostart] {
        behaviour_group.add(row);
    }
    behaviour_group.add(&poll_interval);
    page.add(&behaviour_group);

    let actions_group = adw::PreferencesGroup::builder().title("Действия").build();

    let apply_button = gtk::Button::builder()
        .label("Применить")
        .css_classes(["suggested-action", "pill"])
        .valign(gtk::Align::Center)
        .build();
    let apply_row = adw::ActionRow::builder()
        .title("Сохранить изменения")
        .subtitle("Настройки применяются сразу, без перезапуска окна")
        .build();
    apply_row.add_suffix(&apply_button);

    let reset_oc_button = gtk::Button::builder()
        .label("Вернуть сток")
        .css_classes(["destructive-action", "pill"])
        .valign(gtk::Align::Center)
        .build();
    let reset_row = adw::ActionRow::builder()
        .title("Сброс разгона")
        .subtitle("350 W и разблокированные частоты — перед играми")
        .build();
    reset_row.add_suffix(&reset_oc_button);

    let open_log_button = gtk::Button::builder()
        .label("Открыть")
        .css_classes(["pill"])
        .valign(gtk::Align::Center)
        .build();
    let log_row = adw::ActionRow::builder()
        .title("Лог майнера")
        .subtitle("Вывод WildRig Multi")
        .build();
    log_row.add_suffix(&open_log_button);

    actions_group.add(&apply_row);
    actions_group.add(&reset_row);
    actions_group.add(&log_row);
    page.add(&actions_group);

    (
        page,
        SettingsWidgets {
            power_limit,
            stock_power_limit,
            core_clock,
            memory_clock,
            fan_speed,
            temp_limit,
            temp_resume,
            wallet,
            pool,
            worker,
            algo,
            api_port,
            expected,
            auto_restart,
            notifications,
            autostart,
            poll_interval,
            apply_button,
            reset_oc_button,
            open_log_button,
        },
    )
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

// -------------------------------------------------------------- майнеры

fn build_miner_page() -> (adw::PreferencesPage, BenchWidgets) {
    let page = adw::PreferencesPage::new();

    let profile_group = adw::PreferencesGroup::builder()
        .title("Профиль майнера")
        .description(
            "WildRig — 0% комиссии на pearlhash и уже установлен. \
             SRBMiner (0.85%) и OneZeroMiner (1%) включаются только после \
             собственного бенчмарка: если прирост меньше комиссии, смысла нет.",
        )
        .build();
    let profile_combo = adw::ComboRow::builder()
        .title("Активный майнер")
        .model(&gtk::StringList::new(&[
            "WildRig Multi (0%)",
            "SRBMiner-MULTI (0.85%)",
            "OneZeroMiner (1%)",
        ]))
        .build();
    let apply_profile_button = gtk::Button::builder()
        .label("Сохранить")
        .css_classes(["pill"])
        .valign(gtk::Align::Center)
        .build();
    let apply_row = adw::ActionRow::builder()
        .title("Применить профиль")
        .subtitle("Сохранение перезапустит майнер, если он работает")
        .build();
    apply_row.add_suffix(&apply_profile_button);
    profile_group.add(&profile_combo);
    profile_group.add(&apply_row);
    page.add(&profile_group);

    let paths_group = adw::PreferencesGroup::builder()
        .title("Пути к майнерам")
        .description("Нужны для сравнения. Пустое поле — профиль не участвует в тесте.")
        .build();
    let wildrig_path = adw::EntryRow::builder().title("WildRig Multi").build();
    let srbminer_path = adw::EntryRow::builder().title("SRBMiner-MULTI").build();
    let onezero_path = adw::EntryRow::builder().title("OneZeroMiner").build();
    let save_paths_button = gtk::Button::builder()
        .label("Сохранить пути")
        .css_classes(["pill"])
        .valign(gtk::Align::Center)
        .build();
    let save_paths_row = adw::ActionRow::new();
    save_paths_row.add_suffix(&save_paths_button);
    paths_group.add(&wildrig_path);
    paths_group.add(&srbminer_path);
    paths_group.add(&onezero_path);
    paths_group.add(&save_paths_row);
    page.add(&paths_group);

    let bench_group = adw::PreferencesGroup::builder()
        .title("Сравнение A/B")
        .description(
            "Проверяет выбранные профили по очереди на твоём пуле и считает \
             effective = raw × (1 − комиссия). На время теста майнинг останавливается.",
        )
        .build();
    let minutes = spin_row("Длительность одного прогона", "Минуты", 3.0, 1.0, 30.0);
    let run_button = gtk::Button::builder()
        .label("Сравнить майнеры")
        .css_classes(["suggested-action", "pill"])
        .halign(gtk::Align::Center)
        .build();
    let run_row = adw::ActionRow::new();
    run_row.add_suffix(&run_button);
    bench_group.add(&minutes);
    bench_group.add(&run_row);
    page.add(&bench_group);

    let results_group = adw::PreferencesGroup::builder()
        .title("Результаты")
        .description("Пусто, пока бенчмарк не запускался.")
        .build();
    let mut entries = Vec::new();
    for (id, name, fee) in [
        ("wildrig", "WildRig Multi", "0%"),
        ("srbminer", "SRBMiner-MULTI", "0.85%"),
        ("onezero", "OneZeroMiner", "1%"),
    ] {
        let apply = gtk::Button::builder()
            .label("Сделать основным")
            .css_classes(["pill", "flat"])
            .valign(gtk::Align::Center)
            .build();
        let row = adw::ActionRow::builder()
            .title(name)
            .subtitle(format!("комиссия {fee}"))
            .build();
        row.add_suffix(&apply);
        results_group.add(&row);
        entries.push(BenchEntryRow {
            id: id.to_string(),
            row,
            apply,
        });
    }
    page.add(&results_group);

    (
        page,
        BenchWidgets {
            profile_combo,
            apply_profile_button,
            wildrig_path,
            srbminer_path,
            onezero_path,
            save_paths_button,
            minutes,
            run_button,
            entries,
        },
    )
}

// ------------------------------------------------------------- поведение

impl App {
    fn handle_event(&self, event: Ev) {
        match event {
            Ev::Status(json) => {
                if let Ok(status) = Status::from_json(&json) {
                    self.shell.set_visible_child_name("content");
                    self.apply_status(&status);
                }
            }
            Ev::Config(json) => {
                if let Ok(config) = serde_json::from_str::<Config>(&json) {
                    *self.config.borrow_mut() = config;
                    self.apply_config_to_widgets();
                }
            }
            Ev::Bench(json) => {
                if let Ok(report) = BenchReport::from_json(&json) {
                    self.apply_bench(&report);
                }
            }
            Ev::Info(text) => {
                self.toast.add_toast(adw::Toast::new(&text));
                let _ = self.tx.send(Cmd::Refresh);
            }
            Ev::Error(text) => {
                self.action_button.set_sensitive(true);
                self.action_label.set_label("Запустить");
                // Когда открыта страница «Сервис не запущен», тост дублирует её.
                let offline = self.shell.visible_child_name().as_deref() == Some("offline");
                if !offline {
                    self.toast
                        .add_toast(adw::Toast::new(&humanize_error(&text)));
                }
                eprintln!("pearl-miner: {text}");
            }
            Ev::ServiceDown(text) => {
                self.pill.set_label("нет связи");
                self.pill.remove_css_class("accent");
                self.pill.remove_css_class("warning");
                self.pill.add_css_class("error");
                self.hero_hashrate.set_label("—");
                self.hero_hashrate.add_css_class("dim-label");
                self.action_button.set_sensitive(false);
                self.action_button.remove_css_class("suggested-action");
                self.action_button.remove_css_class("destructive-action");
                self.action_label.set_label("Нет связи");
                self.shell.set_visible_child_name("offline");
                eprintln!("pearl-miner: {text}");
            }
        }
    }

    fn refresh_config(&self) {
        let _ = self.tx.send(Cmd::GetConfig);
    }

    fn mark_dirty(&self) {
        if self.syncing.get() {
            return;
        }
        self.dirty.set(true);
        self.banner.set_revealed(true);
    }

    fn clear_dirty(&self) {
        self.dirty.set(false);
        self.banner.set_revealed(false);
    }

    fn tick(&self) {
        let status = self.status.borrow();
        if let Some(status) = status.as_ref() {
            if status.state.running {
                self.uptime.set_label(&format!(
                    "майнит {} · перезапусков {}",
                    util::human_duration(status.state.uptime_s),
                    status.state.restarts
                ));
            }
        }
    }

    fn set_action_busy(&self) {
        self.action_label.set_label("Переключаю…");
        self.action_button.set_sensitive(false);
    }

    fn apply_status(&self, status: &Status) {
        *self.status.borrow_mut() = Some(status.clone());

        let running = status.state.running;
        self.action_button.set_sensitive(true);
        self.action_label.set_label(if running {
            "Остановить"
        } else {
            "Запустить"
        });
        self.action_button.remove_css_class("suggested-action");
        self.action_button.remove_css_class("destructive-action");
        self.action_button.add_css_class(if running {
            "destructive-action"
        } else {
            "suggested-action"
        });

        // Пилюля состояния
        self.pill.remove_css_class("error");
        self.pill.remove_css_class("warning");
        self.pill.remove_css_class("accent");
        if running {
            if status.state.last_error.is_some() {
                self.pill.set_label("работает с ошибкой");
                self.pill.add_css_class("warning");
            } else if status.state.external {
                self.pill.set_label("запущен вручную");
                self.pill.add_css_class("warning");
            } else {
                self.pill.set_label("майнится");
                self.pill.add_css_class("accent");
            }
        } else {
            self.pill.set_label("остановлен");
        }

        // Заголовок окна: профиль и комиссия вместо невидимого WindowTitle.
        let profile_name = match status.state.profile.as_str() {
            "srbminer" => "SRBMiner-MULTI",
            "onezero" => "OneZeroMiner",
            _ => "WildRig Multi",
        };
        #[allow(deprecated)]
        self.title.set_subtitle(&format!(
            "{profile_name} · комиссия {:.2}%",
            status.total_fee_pct
        ));

        self.uptime.set_visible(running);
        if running {
            self.uptime.set_label(&format!(
                "майнит {} · перезапусков {}",
                util::human_duration(status.state.uptime_s),
                status.state.restarts
            ));
        }

        // Хешрейт
        let primary = status.miner.primary();
        match primary {
            Some(value) => {
                self.hero_hashrate.set_label(&util::format_hashrate(value));
                self.hero_hashrate.remove_css_class("dim-label");
            }
            None => {
                self.hero_hashrate
                    .set_label(if running { "прогрев…" } else { "—" });
                self.hero_hashrate.add_css_class("dim-label");
            }
        }

        let h10 = status
            .miner
            .hashrate_10s
            .map(util::format_hashrate)
            .unwrap_or_else(|| "—".into());
        let h60 = status
            .miner
            .hashrate_60s
            .map(util::format_hashrate)
            .unwrap_or_else(|| "—".into());
        let h15 = status
            .miner
            .hashrate_15m
            .map(util::format_hashrate)
            .unwrap_or_else(|| "—".into());
        self.hero_windows
            .set_label(&format!("10s {h10} · 60s {h60} · 15m {h15}"));

        let load = match (primary, status.expected_hashrate) {
            (Some(value), expected) if expected > 0.0 => {
                (value / expected * 100.0).clamp(0.0, 100.0)
            }
            _ => 0.0,
        };
        self.load_bar.set_fraction(load / 100.0);
        self.load_value.set_label(&format!("{load:.0}%"));

        // GPU
        match status.gpu.as_ref() {
            Some(gpu) => {
                self.rows.temp.show_bar(true);
                self.rows.power.show_bar(true);

                let temp = gpu.temp_c;
                let limit = status.temp_limit as f64;
                self.rows.temp.set(
                    temp.unwrap_or(0) as f64 / 100.0,
                    match temp {
                        Some(value) => format!("{value} / {limit:.0} °C"),
                        None => "—".into(),
                    },
                );
                self.rows.temp.set_tone(match temp {
                    Some(value) if value as f64 >= limit.min(85.0) => Tone::Hot,
                    Some(value) if value as f64 >= 75.0 => Tone::Warm,
                    _ => Tone::Cool,
                });

                let power = gpu.power_w.unwrap_or(0.0);
                let plimit = gpu
                    .power_limit_w
                    .filter(|limit| *limit > 0.0)
                    .unwrap_or(400.0);
                self.rows.power.set(
                    power / plimit,
                    match gpu.power_w {
                        Some(value) if gpu.power_limit_w.is_some() => {
                            format!("{value:.0} / {:.0} W", gpu.power_limit_w.unwrap_or(0.0))
                        }
                        Some(value) => format!("{value:.0} W"),
                        None => "—".into(),
                    },
                );
                self.rows.power.set_tone(Tone::Cool);
                self.rows.power.row.set_subtitle(&match gpu.power_limit_w {
                    Some(limit) if limit > 0.0 => format!("лимит {limit:.0} W"),
                    _ => "лимит не задан".to_string(),
                });

                self.rows.fan.set(
                    gpu.fan_pct
                        .map(|value| format!("{value}%"))
                        .unwrap_or_else(|| "—".into()),
                );
                self.rows.clocks.set(format!(
                    "{} / {} MHz",
                    gpu.core_mhz
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "—".into()),
                    gpu.mem_mhz
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "—".into())
                ));
                self.rows.utilization.set(
                    gpu.util_pct
                        .map(|value| format!("{value}%"))
                        .unwrap_or_else(|| "—".into()),
                );
                self.rows.gpu_name.set(gpu.name.clone());
            }
            None => {
                let reason = status
                    .gpu_error
                    .clone()
                    .unwrap_or_else(|| "нет данных".into());
                self.rows.temp.show_bar(false);
                self.rows.power.show_bar(false);
                self.rows.gpu_name.set_dim("недоступна");
                self.rows.gpu_name.row.set_subtitle(&truncate(&reason, 120));
                self.rows.temp.set(0.0, "—");
                self.rows.power.set(0.0, "—");
                self.rows.power.row.set_subtitle("");
                self.rows.fan.set_dim("—");
                self.rows.clocks.set_dim("—");
                self.rows.utilization.set_dim("—");
            }
        }

        // Шары
        self.rows.accepted.set(
            status
                .miner
                .accepted
                .map(|value| value.to_string())
                .unwrap_or_else(|| "—".into()),
        );
        self.rows.rejected.set(
            status
                .miner
                .rejected
                .map(|value| value.to_string())
                .unwrap_or_else(|| "—".into()),
        );
        self.rows.acceptance.set(
            status
                .acceptance_pct()
                .map(|value| format!("{value:.2}%"))
                .unwrap_or_else(|| "—".into()),
        );

        // Пул
        match status.pool.as_ref() {
            Some(pool) => {
                self.rows.balance.set(
                    pool.balance
                        .map(|value| format!("{value:.3} PRL"))
                        .unwrap_or_else(|| "—".into()),
                );
                if pool.min_payout_prl > 0.0 {
                    self.rows
                        .balance
                        .row
                        .set_subtitle(&format!("выплата от {:.0} PRL", pool.min_payout_prl));
                }
                self.rows.immature.set(
                    pool.immature
                        .map(|value| format!("{value:.3} PRL"))
                        .unwrap_or_else(|| "—".into()),
                );
                self.rows.paid.set(
                    pool.paid
                        .map(|value| format!("{value:.3} PRL"))
                        .unwrap_or_else(|| "—".into()),
                );
                self.rows.pool_hashrate.set(
                    pool.hashrate
                        .map(util::format_hashrate)
                        .unwrap_or_else(|| "—".into()),
                );
            }
            None => {
                for row in [
                    &self.rows.balance,
                    &self.rows.immature,
                    &self.rows.paid,
                    &self.rows.pool_hashrate,
                ] {
                    row.set_dim("—");
                }
            }
        }

        // Замечания: ошибка важнее предупреждений.
        let mut notes: Vec<String> = Vec::new();
        if let Some(error) = &status.state.last_error {
            notes.push(error.clone());
        }
        notes.extend(status.warnings.iter().cloned());
        if notes.is_empty() {
            self.rows.warnings.set_dim("нет");
        } else {
            self.rows.warnings.set(truncate(&notes.join("; "), 160));
        }

        // История спарклайна
        {
            let mut history = self.history.borrow_mut();
            history.push((
                primary.unwrap_or(0.0),
                status.gpu.as_ref().and_then(|gpu| gpu.temp_c),
            ));
            if history.len() > 60 {
                let excess = history.len() - 60;
                history.drain(0..excess);
            }
        }
        self.spark.queue_draw();
    }

    /// Рисует график активности. Все цвета — из темы, поэтому светлая,
    /// тёмная и любой акцент работают без отдельных палитр.
    fn paint_spark(&self, cr: &gtk::cairo::Context, width: i32, height: i32) {
        let w = width as f64;
        let h = height as f64;
        let fg = theme_color(&self.spark, "window_fg_color");
        let accent = theme_color(&self.spark, "accent_bg_color");
        let warning = theme_color(&self.spark, "warning_color");

        // Карточка под графиком
        cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.06);
        rounded_rect(cr, 0.0, 0.0, w, h, 12.0);
        let _ = cr.fill();

        let mode = *self.spark_mode.borrow();
        let history = self.history.borrow();

        let values: Vec<f64> = history
            .iter()
            .map(|(hashrate, temp)| match mode {
                SparkMode::Hashrate => *hashrate,
                SparkMode::Temp => temp.map(|value| value as f64).unwrap_or(0.0),
            })
            .collect();

        let usable: Vec<f64> = values
            .iter()
            .cloned()
            .filter(|value| mode == SparkMode::Temp || *value > 0.0)
            .collect();

        // Подпись сверху, чтобы линия её не перекрывала.
        let caption_top = 10.0;
        let (caption, color) = if usable.is_empty() {
            (
                match mode {
                    SparkMode::Hashrate => "Ждём данные от майнера".to_string(),
                    SparkMode::Temp => "Ждём данные о температуре".to_string(),
                },
                None,
            )
        } else {
            let max = usable.iter().cloned().fold(f64::MIN, f64::max);
            let min = usable.iter().cloned().fold(f64::MAX, f64::min);
            (
                match mode {
                    SparkMode::Hashrate => format!(
                        "макс {} · мин {}",
                        util::format_hashrate(max),
                        util::format_hashrate(min)
                    ),
                    SparkMode::Temp => format!("{min:.0}–{max:.0} °C"),
                },
                Some((max, min)),
            )
        };

        cr.select_font_face(
            "Sans",
            gtk::cairo::FontSlant::Normal,
            gtk::cairo::FontWeight::Normal,
        );
        cr.set_font_size(11.0);
        cr.set_source_rgba(fg.red() as f64, fg.green() as f64, fg.blue() as f64, 0.62);
        let _ = cr.move_to(12.0, caption_top + 11.0);
        let _ = cr.show_text(&caption);

        let Some((max, min)) = color else { return };
        if values.len() < 2 {
            return;
        }

        // Диапазон с запасом: иначе стабильный хешрейт липнет к верхнему краю.
        let hi = if max > 0.0 { max * 1.05 } else { 1.0 };
        let lo = if min > 0.0 {
            (min * 0.95).min(hi - 1.0)
        } else {
            0.0
        };
        let span = (hi - lo).max(1e-9);

        let pad = 12.0;
        let top = caption_top + 22.0;
        let bottom = h - 10.0;
        let usable_h = (bottom - top).max(1.0);
        let step = (w - pad * 2.0) / (values.len().max(2) - 1) as f64;

        let points: Vec<(f64, f64)> = values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                let normalized = ((value - lo) / span).clamp(0.0, 1.0);
                (
                    pad + step * index as f64,
                    top + usable_h * (1.0 - normalized),
                )
            })
            .collect();

        let (red, green, blue) = match mode {
            SparkMode::Hashrate => (
                accent.red() as f64,
                accent.green() as f64,
                accent.blue() as f64,
            ),
            SparkMode::Temp => (
                warning.red() as f64,
                warning.green() as f64,
                warning.blue() as f64,
            ),
        };

        // Рисуем внутри скруглённой карточки, иначе заливка лезет в углы.
        let _ = cr.save();
        rounded_rect(cr, 1.0, 1.0, w - 2.0, h - 2.0, 11.0);
        cr.clip();

        cr.move_to(points[0].0, bottom);
        for (x, y) in &points {
            cr.line_to(*x, *y);
        }
        cr.line_to(points[points.len() - 1].0, bottom);
        cr.close_path();
        cr.set_source_rgba(red, green, blue, 0.20);
        let _ = cr.fill_preserve();
        cr.set_source_rgba(red, green, blue, 0.55);
        let _ = cr.stroke();

        // Сама линия поверх заливки
        cr.set_line_width(2.0);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        cr.set_source_rgba(red, green, blue, 0.95);
        cr.move_to(points[0].0, points[0].1);
        for (x, y) in points.iter().skip(1) {
            cr.line_to(*x, *y);
        }
        let _ = cr.stroke();
        let _ = cr.restore();
    }

    fn apply_bench(&self, report: &BenchReport) {
        for entry in &self.bench.entries {
            let found = report.entries.iter().find(|item| item.profile == entry.id);
            match found {
                Some(result) => {
                    if let Some(error) = &result.error {
                        entry.row.set_subtitle(&format!("ошибка: {error}"));
                        entry.apply.set_sensitive(false);
                        continue;
                    }
                    let raw = result
                        .raw_hps
                        .map(util::format_hashrate)
                        .unwrap_or_else(|| "—".into());
                    let effective = result
                        .effective_hps
                        .map(util::format_hashrate)
                        .unwrap_or_else(|| "—".into());
                    let best = report
                        .best()
                        .map(|item| item.profile == entry.id)
                        .unwrap_or(false);
                    entry.row.set_subtitle(&format!(
                        "raw {raw} · комиссия {:.2}% · effective {effective}{}",
                        result.dev_fee_pct,
                        if best { " · лучший" } else { "" }
                    ));
                    entry.apply.set_sensitive(true);
                }
                None => {
                    entry.row.set_subtitle("не тестировался");
                    entry.apply.set_sensitive(false);
                }
            }
        }
        if let Some(best) = report.best() {
            self.toast.add_toast(adw::Toast::new(&format!(
                "Лучший: {} ({})",
                best.name,
                best.effective_hps
                    .map(util::format_hashrate)
                    .unwrap_or_default()
            )));
        }
    }

    fn run_bench(&self) {
        let minutes = self.bench.minutes.value() as u64;
        let profiles: Vec<String> = ["wildrig", "srbminer", "onezero"]
            .iter()
            .filter(|id| {
                let cfg = self.config.borrow();
                !cfg.miner(id).path.trim().is_empty()
            })
            .map(|id| id.to_string())
            .collect();
        if profiles.len() < 2 {
            self.toast.add_toast(adw::Toast::new(
                "Для сравнения нужны пути минимум к двум майнерам — заполни поля выше",
            ));
            return;
        }
        self.toast.add_toast(adw::Toast::new(
            "Запускаю бенчмарк, майнинг на это время встанет",
        ));
        let _ = self.tx.send(Cmd::Bench {
            profiles,
            seconds: minutes.saturating_mul(60),
        });
    }

    fn apply_profile_from_combo(&self) {
        let id = match self.bench.profile_combo.selected() {
            1 => "srbminer",
            2 => "onezero",
            _ => "wildrig",
        };
        self.set_profile(id);
    }

    fn set_profile(&self, id: &str) {
        let mut cfg = self.config.borrow().clone();
        cfg.miner_profile = id.to_string();
        *self.config.borrow_mut() = cfg.clone();
        match serde_json::to_string(&cfg) {
            Ok(json) => {
                let _ = self.tx.send(Cmd::SetConfig(json));
                let index = match id {
                    "srbminer" => 1,
                    "onezero" => 2,
                    _ => 0,
                };
                self.bench.profile_combo.set_selected(index);
            }
            Err(err) => {
                self.toast
                    .add_toast(adw::Toast::new(&format!("Не удалось сохранить: {err}")));
            }
        }
    }

    fn save_miner_paths(&self) {
        let mut cfg = self.config.borrow().clone();
        for (id, row) in [
            ("wildrig", &self.bench.wildrig_path),
            ("srbminer", &self.bench.srbminer_path),
            ("onezero", &self.bench.onezero_path),
        ] {
            let path = row.text().to_string();
            let entry = cfg.miners.entry(id.to_string()).or_default();
            entry.path = path;
        }
        *self.config.borrow_mut() = cfg.clone();
        match serde_json::to_string(&cfg) {
            Ok(json) => {
                let _ = self.tx.send(Cmd::SetConfig(json));
                self.toast.add_toast(adw::Toast::new("Пути сохранены"));
            }
            Err(err) => {
                self.toast
                    .add_toast(adw::Toast::new(&format!("Не удалось сохранить: {err}")));
            }
        }
    }

    fn apply_config_to_widgets(&self) {
        self.syncing.set(true);
        let cfg = self.config.borrow();

        self.settings
            .power_limit
            .set_value(cfg.gpu.power_limit.unwrap_or(0) as f64);
        self.settings
            .stock_power_limit
            .set_value(cfg.gpu.stock_power_limit as f64);
        self.settings
            .core_clock
            .set_value(cfg.gpu.core_clock.unwrap_or(0) as f64);
        self.settings
            .memory_clock
            .set_value(cfg.gpu.memory_clock.unwrap_or(0) as f64);
        self.settings
            .fan_speed
            .set_value(cfg.gpu.fan_speed.unwrap_or(0) as f64);
        self.settings
            .temp_limit
            .set_value(cfg.gpu.temp_limit as f64);
        self.settings
            .temp_resume
            .set_value(cfg.gpu.temp_resume as f64);

        self.settings.wallet.set_text(&cfg.wallet);
        self.settings.wallet.set_tooltip_text(Some(&cfg.wallet));
        self.settings.pool.set_text(&cfg.pool);
        self.settings
            .pool
            .set_tooltip_text(Some(&format!("{} · воркер {}", cfg.pool, cfg.worker)));
        self.settings.worker.set_text(&cfg.worker);
        self.settings.algo.set_text(&cfg.algo);
        self.settings.api_port.set_value(cfg.api_port as f64);
        self.settings
            .expected
            .set_value((cfg.expected_hashrate / 1e12).max(1.0));

        self.settings.auto_restart.set_active(cfg.auto_restart);
        self.settings.notifications.set_active(cfg.notifications);
        self.settings.autostart.set_active(cfg.autostart_on_boot);
        self.settings
            .poll_interval
            .set_value(cfg.poll_interval_s as f64);

        self.bench.wildrig_path.set_text(&cfg.miner("wildrig").path);
        self.bench
            .srbminer_path
            .set_text(&cfg.miner("srbminer").path);
        self.bench.onezero_path.set_text(&cfg.miner("onezero").path);

        let index = match cfg.miner_profile.as_str() {
            "srbminer" => 1,
            "onezero" => 2,
            _ => 0,
        };
        self.bench.profile_combo.set_selected(index);

        self.syncing.set(false);
    }

    /// Собирает конфиг из полей формы.
    fn config_from_widgets(&self) -> Config {
        let mut cfg = self.config.borrow().clone();
        let power = self.settings.power_limit.value() as u32;
        cfg.gpu.power_limit = (power > 0).then_some(power);
        cfg.gpu.stock_power_limit = self.settings.stock_power_limit.value() as u32;
        let core = self.settings.core_clock.value() as u32;
        cfg.gpu.core_clock = (core > 0).then_some(core);
        let mem = self.settings.memory_clock.value() as u32;
        cfg.gpu.memory_clock = (mem > 0).then_some(mem);
        let fan = self.settings.fan_speed.value() as u32;
        cfg.gpu.fan_speed = (fan > 0).then_some(fan);
        cfg.gpu.temp_limit = self.settings.temp_limit.value() as u32;
        cfg.gpu.temp_resume = self.settings.temp_resume.value() as u32;

        cfg.wallet = self.settings.wallet.text().to_string();
        cfg.pool = self.settings.pool.text().to_string();
        cfg.worker = self.settings.worker.text().to_string();
        cfg.algo = self.settings.algo.text().to_string();
        cfg.api_port = self.settings.api_port.value() as u16;
        cfg.expected_hashrate = self.settings.expected.value() * 1e12;

        cfg.auto_restart = self.settings.auto_restart.is_active();
        cfg.notifications = self.settings.notifications.is_active();
        cfg.poll_interval_s = self.settings.poll_interval.value() as u64;

        for (id, row) in [
            ("wildrig", &self.bench.wildrig_path),
            ("srbminer", &self.bench.srbminer_path),
            ("onezero", &self.bench.onezero_path),
        ] {
            let path = row.text().to_string();
            let entry = cfg.miners.entry(id.to_string()).or_default();
            entry.path = path;
        }

        cfg.miner_profile = match self.bench.profile_combo.selected() {
            1 => "srbminer",
            2 => "onezero",
            _ => "wildrig",
        }
        .to_string();

        cfg
    }

    fn save_settings(&self) {
        let mut cfg = self.config_from_widgets();

        let autostart = self.settings.autostart.is_active();
        if autostart != cfg.autostart_on_boot {
            std::thread::spawn(move || set_autostart_unit(autostart));
        }
        cfg.autostart_on_boot = autostart;

        let warnings = cfg.warnings();
        *self.config.borrow_mut() = cfg.clone();
        match serde_json::to_string(&cfg) {
            Ok(json) => {
                let _ = self.tx.send(Cmd::SetConfig(json));
                self.clear_dirty();
                if let Some(first) = warnings.first() {
                    self.toast
                        .add_toast(adw::Toast::new(&format!("Внимание: {first}")));
                } else {
                    self.toast.add_toast(adw::Toast::new("Настройки сохранены"));
                }
            }
            Err(err) => {
                self.toast.add_toast(adw::Toast::new(&format!(
                    "Не удалось собрать конфиг: {err}"
                )));
            }
        }
    }
}

/// Включает/выключает автозапуск майнинга (systemd --user unit).
fn set_autostart_unit(enable: bool) {
    let action = if enable { "enable" } else { "disable" };
    let _ = std::process::Command::new("systemctl")
        .args(["--user", action, "pearl-miner-autostart.service"])
        .status();
}

/// Короткое объяснение ошибки вместо сырого текста D-Bus.
fn humanize_error(text: &str) -> String {
    let lower = text.to_ascii_lowercase();
    if lower.contains("serviceunknown")
        || lower.contains("not provided")
        || lower.contains("not registered")
        || lower.contains("no owner")
    {
        return "Сервис pearl-minerd не запущен".into();
    }
    if lower.contains("timed out") || lower.contains("timeout") {
        return "Сервис не ответил вовремя".into();
    }
    format!("Не получилось: {}", truncate(text, 100))
}

/// Обрезает длинный текст для подписей (ошибки nvidia-smi бывают огромными).
fn truncate(text: &str, max_chars: usize) -> String {
    let cleaned: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() <= max_chars {
        cleaned
    } else {
        let cut: String = cleaned.chars().take(max_chars.saturating_sub(1)).collect();
        format!("{cut}…")
    }
}

fn rounded_rect(cr: &gtk::cairo::Context, x: f64, y: f64, width: f64, height: f64, radius: f64) {
    let radius = radius.min(width / 2.0).min(height / 2.0);
    cr.new_sub_path();
    cr.arc(
        width - radius + x,
        y + radius,
        radius,
        -std::f64::consts::FRAC_PI_2,
        0.0,
    );
    cr.arc(
        width - radius + x,
        height - radius + y,
        radius,
        0.0,
        std::f64::consts::FRAC_PI_2,
    );
    cr.arc(
        x + radius,
        height - radius + y,
        radius,
        std::f64::consts::FRAC_PI_2,
        std::f64::consts::PI,
    );
    cr.arc(
        x + radius,
        y + radius,
        radius,
        std::f64::consts::PI,
        3.0 * std::f64::consts::FRAC_PI_2,
    );
    cr.close_path();
}
