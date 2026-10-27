//! `trommel` — command line client and GUI entry point.

use std::process::Command as ProcessCommand;

use anyhow::anyhow;
use clap::{Parser, Subcommand};
use trommel_core::config::Config;
use trommel_core::dbus::MinerProxy;
use trommel_core::paths;
use trommel_core::status::Status;
use trommel_core::util;

#[cfg(feature = "gui")]
mod gui;

#[derive(Parser)]
#[command(
    name = "trommel",
    version = trommel_core::VERSION,
    about = "Pearl (PRL) miner: быстрый старт/стоп, статистика и настройки"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,
}

#[derive(Subcommand)]
enum Cmd {
    /// Запустить майнинг (применяет разгон)
    Start,
    /// Остановить майнинг и вернуть сток (350 W, частоты разблокированы)
    Stop,
    /// Переключить: запущен → стоп, остановлен → старт
    Toggle,
    /// Показать статус
    Status {
        /// Вывести JSON (контракт для расширения и GUI)
        #[arg(long)]
        json: bool,
    },
    /// Сравнить майнеры (A/B) и показать effective hashrate
    Bench {
        /// Длительность теста одного майнера, минуты
        #[arg(long, default_value_t = 3)]
        minutes: u64,
        /// Список профилей через запятую (по умолчанию — все настроенные)
        #[arg(long, default_value = "")]
        miners: String,
    },
    /// Открыть приложение (GTK4/libadwaita)
    Gui,
    /// Запустить фоновый сервис в терминале (обычно его поднимает systemd)
    Daemon,
    /// Работа с файлом настроек
    Config {
        #[command(subcommand)]
        command: ConfigCmd,
    },
    /// Работа с пресетами
    Preset {
        #[command(subcommand)]
        command: PresetCmd,
    },
    /// Работа с каталогом и установкой майнеров
    Miner {
        #[command(subcommand)]
        command: MinerCmd,
    },
}

#[derive(Subcommand)]
enum PresetCmd {
    /// Показать пресеты
    List,
    /// Сделать пресет активным
    Use {
        id: String,
        /// Перезапустить майнер после смены
        #[arg(long)]
        restart: bool,
        /// Остановить майнер после смены
        #[arg(long)]
        stop: bool,
    },
    /// Показать активный пресет
    Show,
}

#[derive(Subcommand)]
enum MinerCmd {
    /// Показать каталог и установленные версии
    List,
    /// Зарегистрировать существующий бинарник
    Import {
        miner: String,
        version: String,
        path: String,
    },
    /// Скачать и установить бинарник по HTTPS
    Install {
        miner: String,
        version: String,
        url: String,
        #[arg(long)]
        sha256: String,
        #[arg(long, default_value = "binary")]
        archive: String,
        #[arg(long)]
        binary: String,
    },
}

#[derive(Subcommand)]
enum ConfigCmd {
    /// Напечатать путь к config.json
    Path,
    /// Напечатать текущий конфиг
    Get,
    /// Сбросить конфиг к значениям по умолчанию
    Reset,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        None => open_gui(),
        Some(Cmd::Gui) => open_gui(),
        Some(Cmd::Daemon) => run_daemon(),
        Some(Cmd::Preset { command }) => run_preset_command(command).await,
        Some(Cmd::Miner { command }) => run_miner_command(command).await,
        Some(Cmd::Config { command }) => match command {
            ConfigCmd::Path => {
                println!("{}", paths::config_path().display());
                Ok(())
            }
            ConfigCmd::Get => {
                let cfg = Config::load()?;
                println!("{}", serde_json::to_string_pretty(&cfg)?);
                Ok(())
            }
            ConfigCmd::Reset => {
                let cfg = Config::default();
                cfg.save()?;
                println!("Конфиг сброшен: {}", paths::config_path().display());
                Ok(())
            }
        },
        Some(Cmd::Status { json }) => {
            let proxy = connect().await?;
            let raw = proxy.refresh().await?;
            if json {
                println!("{raw}");
            } else {
                print_human(&Status::from_json(&raw)?);
            }
            Ok(())
        }
        Some(Cmd::Start) => {
            let proxy = connect().await?;
            proxy.start().await?;
            println!("Майнинг запущен");
            Ok(())
        }
        Some(Cmd::Stop) => {
            let proxy = connect().await?;
            proxy.stop().await?;
            println!("Майнинг остановлен, питание и частоты возвращены в сток");
            Ok(())
        }
        Some(Cmd::Toggle) => {
            let proxy = connect().await?;
            let now_running = proxy.toggle().await?;
            println!(
                "{}",
                if now_running {
                    "Майнинг запущен"
                } else {
                    "Майнинг остановлен"
                }
            );
            Ok(())
        }
        Some(Cmd::Bench { minutes, miners }) => {
            let proxy = connect().await?;
            let profiles: Vec<String> = miners
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect();
            let request = serde_json::json!({
                "profiles": profiles,
                "seconds": minutes.saturating_mul(60),
            });
            println!(
                "Тестирую майнеры (~{minutes} мин на каждый, майнинг на это время встанет)..."
            );
            let json = proxy.run_benchmark(&request.to_string()).await?;
            print_bench(&json);
            Ok(())
        }
    }
}

/// Connect to the daemon, starting it if needed.
async fn connect() -> anyhow::Result<MinerProxy<'static>> {
    let connection = zbus::Connection::session().await?;
    let bus = zbus::fdo::DBusProxy::new(&connection).await?;
    let has_owner = name_has_owner(&bus).await;

    if !has_owner {
        // Nothing is running: try to start the daemon (D-Bus activation or PATH).
        if let Err(err) = spawn_daemon() {
            return Err(anyhow!(
                "фоновый сервис не запущен ({err}). Запусти: systemctl --user start trommeld"
            ));
        }
        for _ in 0..40 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            if name_has_owner(&bus).await {
                break;
            }
        }
    }

    MinerProxy::new(&connection)
        .await
        .map_err(|e| anyhow!("не удалось подключиться к сервису: {e}"))
}

async fn name_has_owner(bus: &zbus::fdo::DBusProxy<'_>) -> bool {
    match zbus::names::BusName::try_from(trommel_core::BUS_NAME) {
        Ok(name) => bus.name_has_owner(name).await.unwrap_or(false),
        Err(_) => false,
    }
}

fn spawn_daemon() -> anyhow::Result<()> {
    for candidate in ["trommeld", "trommel-daemon"] {
        let found = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("command -v {candidate}"))
            .output()
            .map(|out| out.status.success())
            .unwrap_or(false);
        if found {
            std::process::Command::new(candidate)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            return Ok(());
        }
    }
    anyhow::bail!("trommeld не найден в PATH")
}

fn run_daemon() -> anyhow::Result<()> {
    let status = ProcessCommand::new("trommeld").status()?;
    if !status.success() {
        anyhow::bail!("trommeld завершился с кодом {:?}", status.code());
    }
    Ok(())
}

fn open_gui() -> anyhow::Result<()> {
    #[cfg(feature = "gui")]
    {
        gui::run()
    }
    #[cfg(not(feature = "gui"))]
    {
        anyhow::bail!(
            "эта сборка собрана без GUI. Пересобери с ключом --features gui (см. install.sh)"
        )
    }
}

async fn run_preset_command(command: PresetCmd) -> anyhow::Result<()> {
    let mut cfg = Config::load_or_default();
    match command {
        PresetCmd::List => {
            for (id, preset) in &cfg.presets {
                let active = if cfg.active_preset.as_deref() == Some(id.as_str()) {
                    "*"
                } else {
                    " "
                };
                println!(
                    "{active} {:<20} {:<24} {} {} · {:.2}%",
                    id,
                    preset.name,
                    preset.miner,
                    preset.miner_version,
                    preset.total_fee_pct()
                );
            }
            Ok(())
        }
        PresetCmd::Show => {
            let preset = cfg
                .preset()
                .cloned()
                .ok_or_else(|| anyhow!("no active preset"))?;
            println!("{}", serde_json::to_string_pretty(&preset)?);
            Ok(())
        }
        PresetCmd::Use { id, restart, stop } => {
            if !cfg.presets.contains_key(&id) {
                anyhow::bail!("unknown preset {id}");
            }
            cfg.active_preset = Some(id.clone());
            cfg.save()?;
            let mode = if stop {
                "stop"
            } else if restart {
                "restart"
            } else {
                "live"
            };
            if let Ok(proxy) = connect().await {
                let json = serde_json::to_string(&cfg)?;
                match proxy.apply_config(&json, mode).await {
                    Ok(_) => println!("active preset: {id} ({mode})"),
                    Err(err) => eprintln!("preset saved, but apply failed: {err}"),
                }
            } else {
                println!("active preset: {id}");
            }
            Ok(())
        }
    }
}

async fn run_miner_command(command: MinerCmd) -> anyhow::Result<()> {
    let mut cfg = Config::load_or_default();
    match command {
        MinerCmd::List => {
            for miner in trommel_core::catalog::miners() {
                let installed = trommel_core::catalog::installed_versions(&cfg, &miner.id);
                println!(
                    "{:<12} {:<22} {:>6.2}%  {}",
                    miner.id,
                    miner.name,
                    miner.fee_pct,
                    if installed.is_empty() {
                        "not installed".to_string()
                    } else {
                        format!("installed: {}", installed.join(", "))
                    }
                );
            }
            Ok(())
        }
        MinerCmd::Import {
            miner,
            version,
            path,
        } => {
            let path =
                std::fs::canonicalize(&path).map_err(|e| anyhow!("cannot resolve {path}: {e}"))?;
            if !path.is_file() {
                anyhow::bail!("not a file: {}", path.display());
            }
            cfg.add_install(trommel_core::config::MinerInstall {
                miner: miner.clone(),
                version: version.clone(),
                path: path.to_string_lossy().into_owned(),
                sha256: String::new(),
                source_url: String::new(),
                verified: false,
                custom: true,
                installed_at: trommel_core::util::now_unix(),
            });
            cfg.save()?;
            println!("imported {miner} {version}");
            Ok(())
        }
        MinerCmd::Install {
            miner,
            version,
            url,
            sha256,
            archive,
            binary,
        } => {
            let request = trommel_core::download::DownloadRequest {
                miner: miner.clone(),
                version: version.clone(),
                url: url.clone(),
                sha256,
                archive,
                binary,
                source_url: url,
            };
            let root = trommel_core::download::miner_root();
            let install = trommel_core::download::install(&request, &root)?;
            cfg.add_install(install);
            cfg.save()?;
            println!("installed {miner} {version}");
            Ok(())
        }
    }
}

fn print_human(status: &Status) {
    let state = if status.state.running {
        if status.state.external {
            "майнится (запущен вручную)"
        } else {
            "майнится"
        }
    } else {
        "остановлен"
    };
    println!(
        "Trommel {} — {}{}",
        status.version,
        state,
        if status.state.running {
            format!(" ({})", util::human_duration(status.state.uptime_s))
        } else {
            String::new()
        }
    );

    if !status.miner_info.name.is_empty() {
        println!(
            "Пресет:   {} · {} {} · комиссия {:.2}% (майнер {:.2}% + пул {:.2}%)",
            status.state.preset_name,
            status.miner_info.name,
            status.miner_info.version,
            status.miner_info.total_fee_pct,
            status.miner_info.miner_fee_pct,
            status.miner_info.pool_fee_pct
        );
    }

    if status.state.running {
        println!(
            "Хешрейт:  {} (10s) · {} (60s) · {} (15m) [{}]",
            status
                .miner
                .hashrate_10s
                .map(util::format_hashrate)
                .unwrap_or_else(|| "—".into()),
            status
                .miner
                .hashrate_60s
                .map(util::format_hashrate)
                .unwrap_or_else(|| "—".into()),
            status
                .miner
                .hashrate_15m
                .map(util::format_hashrate)
                .unwrap_or_else(|| "—".into()),
            status.miner.source,
        );
    }

    if status.gpus.is_empty() {
        println!(
            "GPU:      недоступен ({})",
            status
                .gpu_error
                .clone()
                .unwrap_or_else(|| "нет данных".into())
        );
    } else {
        for gpu in &status.gpus {
            println!(
                "GPU #{}:   {} · {} °C · {} W · вентилятор {}% · {} / {} MHz · {}",
                gpu.index,
                gpu.name,
                gpu.temp_c
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "—".into()),
                gpu.power_w
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
            );
        }
    }

    if let (Some(accepted), Some(acceptance)) = (status.miner.accepted, status.acceptance_pct()) {
        println!(
            "Шары:     {accepted} принято · {} отклонено ({acceptance:.1}%)",
            status.miner.rejected.unwrap_or(0)
        );
    }

    if let Some(pool) = &status.pool {
        println!(
            "Пул:      баланс {} PRL · незрелых {} · выплачено {}",
            pool.balance
                .map(|v| format!("{v:.3}"))
                .unwrap_or_else(|| "—".into()),
            pool.immature
                .map(|v| format!("{v:.3}"))
                .unwrap_or_else(|| "—".into()),
            pool.paid
                .map(|v| format!("{v:.3}"))
                .unwrap_or_else(|| "—".into()),
        );
    }

    if let Some(error) = &status.state.last_error {
        println!("Ошибка:   {error}");
    }
    for warning in &status.warnings {
        println!("Внимание: {warning}");
    }
}

fn print_bench(json: &str) {
    let report = match trommel_core::bench::BenchReport::from_json(json) {
        Ok(report) => report,
        Err(err) => {
            println!("Не удалось разобрать отчёт: {err}");
            return;
        }
    };
    println!();
    println!(
        "{:<14} {:>14} {:>8} {:>14}",
        "Майнер", "Raw", "Fee", "Effective"
    );
    for entry in &report.entries {
        if let Some(error) = &entry.error {
            println!("{:<14} {}", entry.name, error);
            continue;
        }
        println!(
            "{:<14} {:>14} {:>7.2}% {:>14}",
            entry.name,
            entry
                .raw_hps
                .map(util::format_hashrate)
                .unwrap_or_else(|| "—".into()),
            entry.total_fee_pct,
            entry
                .effective_hps
                .map(util::format_hashrate)
                .unwrap_or_else(|| "—".into()),
        );
    }
    if let Some(best) = report.best() {
        println!();
        println!(
            "Лучший по effective: {} ({})",
            best.name,
            util::format_hashrate(best.effective_hps.unwrap_or_default())
        );
    }
}
