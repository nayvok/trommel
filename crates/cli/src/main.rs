//! `pearl-miner` — command line client and GUI entry point.

use std::process::Command as ProcessCommand;

use anyhow::anyhow;
use clap::{Parser, Subcommand};
use pearl_miner_core::config::Config;
use pearl_miner_core::dbus::MinerProxy;
use pearl_miner_core::paths;
use pearl_miner_core::status::Status;
use pearl_miner_core::util;

#[cfg(feature = "gui")]
mod gui;

#[derive(Parser)]
#[command(
    name = "pearl-miner",
    version = pearl_miner_core::VERSION,
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
                "фоновый сервис не запущен ({err}). Запусти: systemctl --user start pearl-minerd"
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
    match zbus::names::BusName::try_from(pearl_miner_core::BUS_NAME) {
        Ok(name) => bus.name_has_owner(name).await.unwrap_or(false),
        Err(_) => false,
    }
}

fn spawn_daemon() -> anyhow::Result<()> {
    for candidate in ["pearl-minerd", "pearl-miner-daemon"] {
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
    anyhow::bail!("pearl-minerd не найден в PATH")
}

fn run_daemon() -> anyhow::Result<()> {
    let status = ProcessCommand::new("pearl-minerd").status()?;
    if !status.success() {
        anyhow::bail!("pearl-minerd завершился с кодом {:?}", status.code());
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
        "Pearl Miner {} — {}{}",
        status.version,
        state,
        if status.state.running {
            format!(" ({})", util::human_duration(status.state.uptime_s))
        } else {
            String::new()
        }
    );

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

    match &status.gpu {
        Some(gpu) => println!(
            "GPU:      {} · {} °C · {} W · вентилятор {}% · {} / {} MHz",
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
        ),
        None => println!(
            "GPU:      недоступен ({})",
            status
                .gpu_error
                .clone()
                .unwrap_or_else(|| "нет данных".into())
        ),
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
    let report = match pearl_miner_core::bench::BenchReport::from_json(json) {
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
            entry.dev_fee_pct,
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
