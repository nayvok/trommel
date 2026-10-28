//! Miner process management: start, stop, restart-on-crash, stock restore.
//!
//! Used by `trommeld`; deliberately synchronous so the daemon can call it from
//! blocking contexts.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::{Config, GpuTuning};
use crate::i18n::tr;
use crate::miners;
use crate::nvidia::GpuStats;
use crate::oc::{self, AppliedTuning, OcSnapshot, RestoreMode};
use crate::paths;
use crate::state::{self, State};
use crate::util;

#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    Started(u32),
    Crashed { code: Option<i32>, restart_in: u64 },
    Stopped { code: Option<i32> },
    Error(String),
    Warning(String),
    UserStopped,
}

impl Event {
    pub fn message(&self) -> Option<String> {
        match self {
            Event::Started(_) => Some(tr("Mining started", "Майнинг запущен")),
            Event::Crashed { code, restart_in } => Some(format!(
                "{} ({}) — {} {restart_in} {}",
                tr("Miner crashed", "Майнер упал"),
                code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
                tr("restarting in", "перезапуск через"),
                tr("s", "с")
            )),
            Event::Stopped { code } => Some(format!(
                "{} ({})",
                tr("Miner stopped", "Майнер остановлен"),
                code.map(|c| c.to_string()).unwrap_or_else(|| "?".into())
            )),
            Event::Error(text) | Event::Warning(text) => Some(text.clone()),
            Event::UserStopped => Some(tr(
                "Mining stopped; stock clocks and power restored",
                "Майнинг остановлен; частоты и питание возвращены в сток",
            )),
        }
    }
}

pub struct Supervisor {
    child: Option<Child>,
    restart_at: Option<Instant>,
    /// When to check that the applied clock locks are actually honoured.
    verify_at: Option<Instant>,
    /// Give the miner time to load the GPU; stop retrying after this.
    verify_until: Option<Instant>,
    verified: bool,
    pub state: State,
    pub log_path: PathBuf,
}

impl Default for Supervisor {
    fn default() -> Self {
        Self::new()
    }
}

fn verify_delay() -> Duration {
    std::env::var("TROMMEL_OC_VERIFY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(|ms| Duration::from_millis(ms.max(1000)))
        .unwrap_or(Duration::from_secs(15))
}

impl Supervisor {
    pub fn new() -> Self {
        let mut state = State::load().normalize();
        // The daemon restarted while a miner it spawned is still alive. We
        // lost the Child handle, so monitor/adopt it by PID; otherwise its
        // death would never be noticed and Stop would not know it is ours.
        if state.running && !state.external && state.miner_alive() {
            state.orphan = true;
        }
        let now = Instant::now();
        let (verify_at, verify_until) = if state.running && !state.external {
            (
                Some(now + verify_delay()),
                Some(now + Duration::from_secs(300)),
            )
        } else {
            (None, None)
        };
        Self {
            child: None,
            restart_at: None,
            verify_at,
            verify_until,
            verified: false,
            state,
            log_path: paths::log_path(),
        }
    }

    pub fn is_running(&self) -> bool {
        self.child.is_some() || self.state.running
    }

    /// Start the miner: snapshot stock, apply the GPU profile, spawn.
    pub fn start(&mut self, cfg: &Config) -> anyhow::Result<()> {
        if self.child.is_some() || self.state.miner_alive() {
            anyhow::bail!(tr(
                "The miner is already running; stop it before starting again",
                "Майнер уже запущен — сначала остановите его"
            ));
        }
        // A stale running flag must not make Start a silent no-op.
        if self.state.running {
            self.state = State::default();
            let _ = self.state.save();
        }
        // A stale external flag must not survive a Start attempt. Clear it
        // before the external scan so a real script miner is still reported
        // by the scan below (and never silently treated as "already ours").
        if self.state.external {
            self.state = State::default();
            let _ = self.state.save();
        }
        // Never spawn a second miner next to a script/manual one. We do not
        // kill it either: stopping an external process is the user's call.
        if let Some(pid) = state::find_external_miner() {
            anyhow::bail!(
                "{} {} — {}",
                tr(
                    "An external miner is already running (PID",
                    "Внешний майнер уже работает (PID"
                ),
                pid,
                tr(
                    "stop that process first, then press Start",
                    "остановите его, затем нажмите «Запустить»"
                )
            );
        }
        let preset = cfg
            .preset()
            .ok_or_else(|| anyhow::anyhow!(tr("No active preset", "Нет активного пресета")))?;
        if preset.wallet.trim().is_empty() {
            anyhow::bail!(tr(
                "Wallet is not set; open a preset and add it",
                "Кошелёк не задан — откройте пресет и укажите его"
            ));
        }
        if !preset.pool_url.contains(':') {
            anyhow::bail!(tr(
                "Pool address must include a port",
                "В адресе пула должен быть порт"
            ));
        }
        let binary = cfg.miner_path(preset).ok_or_else(|| {
            anyhow::anyhow!(
                "{} {} {}",
                tr("Miner is not installed:", "Майнер не установлен:"),
                preset.miner,
                preset.miner_version
            )
        })?;
        if !binary.exists() {
            anyhow::bail!(
                "{} {}",
                tr("Miner binary not found:", "Бинарник майнера не найден:"),
                binary.display()
            );
        }

        let gpus = crate::nvidia::query_all().map_err(|e| {
            anyhow::anyhow!(tr("GPU query failed:", "Ошибка опроса GPU:") + " " + &e.to_string())
        })?;
        let profile = cfg.gpu_profile().cloned().unwrap_or_default();
        if !gpus.iter().any(|gpu| profile.tuning_for(&gpu.uuid).enabled) {
            anyhow::bail!(tr(
                "No GPU is enabled in the active profile",
                "В активном профиле не включено ни одной видеокарты"
            ));
        }

        // Only cards that actually ask for a change are touched. An enabled
        // card with no power/clock values is not a tuning session and needs
        // neither a snapshot nor root.
        let targets: Vec<(&GpuStats, GpuTuning)> = gpus
            .iter()
            .filter_map(|gpu| {
                let tuning = profile.tuning_for(&gpu.uuid);
                (tuning.enabled && tuning.has_tuning()).then_some((gpu, tuning))
            })
            .collect();

        if targets.is_empty() {
            // A stock profile must really be stock: if a previous session left
            // applied settings behind (restore-on-stop was off), put the
            // snapshot back before spawning.
            if let Some(snapshot) = oc::current_boot_snapshot() {
                if snapshot.applied {
                    log_line("stock profile: restoring the previous applied snapshot first");
                    if let Err(err) = restore_with_snapshot(&snapshot, RestoreMode::Snapshot) {
                        oc::record_error(&err.to_string());
                        anyhow::bail!(
                            "{} {err}",
                            tr(
                                "Cannot start on stock: restoring the previous settings failed:",
                                "Не удалось начать на стоке: возврат предыдущих настроек не удался:"
                            )
                        );
                    }
                } else {
                    oc::clear_snapshot();
                }
            }
            return self.spawn_and_record(cfg);
        }

        let mut snapshot = match oc::current_boot_snapshot() {
            Some(existing) if existing.applied || existing.gpus.iter().any(|g| g.was_tuned) => {
                existing
            }
            _ => oc::snapshot()?,
        };
        if snapshot.gpus.iter().any(|gpu| gpu.was_tuned) {
            log_line(
                "oc snapshot: a card already had a non-default power limit; Restore returns to that value (use driver defaults for the stock one)",
            );
        }
        // Record the intent before calling the helper: if we crash halfway,
        // Stop still knows which cards to put back.
        snapshot.applied = true;
        snapshot.last_error = None;
        for (gpu, tuning) in &targets {
            if !snapshot.applied_gpus.iter().any(|uuid| uuid == &gpu.uuid) {
                snapshot.applied_gpus.push(gpu.uuid.clone());
            }
            snapshot
                .applied_tuning
                .insert(gpu.uuid.clone(), AppliedTuning::from(tuning));
        }
        oc::save_snapshot(&snapshot)?;

        let mut applied: Vec<String> = Vec::new();
        for (gpu, tuning) in &targets {
            // Register the card *before* applying: the helper applies --pl,
            // --lgc and --lmc in order, so a failure halfway must restore this
            // card as well, not just the previous ones.
            applied.push(gpu.uuid.clone());
            if let Err(err) = oc::apply_tuning(gpu, tuning) {
                match rollback(&snapshot, &applied) {
                    Ok(()) => oc::clear_snapshot(),
                    Err(rollback_err) => {
                        oc::record_error(&format!("{err}; rollback failed: {rollback_err}"))
                    }
                }
                anyhow::bail!(
                    "{} {}",
                    tr("GPU tuning was not applied:", "Разгон не применён:"),
                    err
                );
            }
        }

        if let Err(err) = self.spawn(cfg) {
            match rollback(&snapshot, &applied) {
                Ok(()) => oc::clear_snapshot(),
                Err(rollback_err) => {
                    oc::record_error(&format!("{err}; rollback failed: {rollback_err}"))
                }
            }
            return Err(err);
        }

        self.state.running = true;
        self.state.external = false;
        self.state.orphan = false;
        self.state.last_error = None;
        self.state.started_at = Some(util::now_unix());
        self.state.restarts = 0;
        self.state.preset_id = cfg.active_preset.clone();
        self.state.miner = Some(preset.miner.clone());
        self.state.miner_version = Some(preset.miner_version.clone());
        self.state.profile = Some(preset.miner.clone());
        let _ = self.state.save();
        self.schedule_verify();
        if !cfg.behavior.restore_stock_on_stop {
            log_line(
                "oc tuning applied with restore_stock_on_stop=false; Stop will leave it applied",
            );
        }
        crate::pool::invalidate_cache();
        Ok(())
    }

    fn spawn_and_record(&mut self, cfg: &Config) -> anyhow::Result<()> {
        let preset = cfg.preset().cloned();
        self.spawn(cfg)?;
        // No tuning was applied, so there is nothing to verify.
        self.verify_at = None;
        self.verify_until = None;
        self.verified = true;
        self.state.last_error = None;
        self.state.started_at = Some(util::now_unix());
        self.state.restarts = 0;
        self.state.preset_id = cfg.active_preset.clone();
        if let Some(preset) = preset {
            self.state.miner = Some(preset.miner.clone());
            self.state.miner_version = Some(preset.miner_version.clone());
            self.state.profile = Some(preset.miner);
        }
        let _ = self.state.save();
        crate::pool::invalidate_cache();
        Ok(())
    }

    fn schedule_verify(&mut self) {
        let now = Instant::now();
        self.verified = false;
        self.verify_at = Some(now + verify_delay());
        self.verify_until = Some(now + Duration::from_secs(300));
    }

    /// Stop the miner and restore the snapshot (when enabled).
    pub fn stop(&mut self, cfg: &Config) -> anyhow::Result<()> {
        self.stop_internal(cfg, false)
    }

    fn stop_internal(&mut self, cfg: &Config, force_restore: bool) -> anyhow::Result<()> {
        self.restart_at = None;
        self.verify_at = None;
        self.verify_until = None;
        self.verified = true;
        if let Some(mut child) = self.child.take() {
            terminate(&mut child);
            let _ = child.wait();
        } else if let Some(pid) = self.state.miner_pid {
            if state::pid_matches_miner(pid) {
                terminate_pid(pid);
            }
        }

        let restore = cfg.behavior.restore_stock_on_stop || force_restore;
        let reset_result = if restore {
            match oc::current_boot_snapshot().filter(|snapshot| snapshot.applied) {
                Some(snapshot) => {
                    if force_restore && !cfg.behavior.restore_stock_on_stop {
                        log_line("oc restore forced after a failed clock check");
                    }
                    let result = restore_with_snapshot(&snapshot, RestoreMode::Snapshot);
                    if let Err(ref err) = result {
                        oc::record_error(&err.to_string());
                    }
                    result
                }
                None => Ok(()),
            }
        } else {
            log_line("oc restore skipped: restore_stock_on_stop=false (snapshot kept)");
            Ok(())
        };

        self.state = State::default();
        State::clear_file();
        crate::pool::invalidate_cache();
        reset_result
    }

    pub fn tick(&mut self, cfg: &Config) -> Vec<Event> {
        let mut events = Vec::new();

        if let Some(mut child) = self.child.take() {
            match child.try_wait() {
                Ok(Some(status)) => {
                    let code = status.code();
                    self.state.miner_pid = None;
                    self.verify_at = None;
                    if cfg.behavior.auto_restart && self.state.running {
                        self.state.restarts += 1;
                        self.state.last_error = Some(format!(
                            "{} ({}) — {} {} {}",
                            tr("Miner exited", "Майнер завершился"),
                            code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
                            tr("restarting in", "перезапуск через"),
                            cfg.behavior.restart_delay_s.max(1),
                            tr("s", "с")
                        ));
                        let _ = self.state.save();
                        self.restart_at = Some(
                            Instant::now()
                                + Duration::from_secs(cfg.behavior.restart_delay_s.max(1)),
                        );
                        events.push(Event::Crashed {
                            code,
                            restart_in: cfg.behavior.restart_delay_s.max(1),
                        });
                    } else {
                        self.state.running = false;
                        self.state.started_at = None;
                        self.state.last_error = Some(format!(
                            "{} ({}) — {}",
                            tr("Miner exited", "Майнер завершился"),
                            code.map(|c| c.to_string()).unwrap_or_else(|| "?".into()),
                            tr("auto-restart is off", "автоперезапуск выключен")
                        ));
                        if let Err(err) = restore_stock(cfg) {
                            self.state.last_error =
                                Some(format!("{}; {err}", text_of(&self.state)));
                            events.push(Event::Error(err.to_string()));
                        }
                        let _ = self.state.save();
                        events.push(Event::Stopped { code });
                    }
                }
                Ok(None) => self.child = Some(child),
                Err(err) => {
                    events.push(Event::Error(format!(
                        "{} {err}",
                        tr("Unable to poll miner:", "Не удалось опросить майнер:")
                    )));
                    self.child = Some(child);
                }
            }
        } else if self.state.external {
            if !self.state.miner_alive() {
                // The legacy script/manual miner is gone. Do not touch its
                // power/clocks: they were not ours to restore.
                self.state = State::default();
                let _ = self.state.save();
                events.push(Event::Stopped { code: None });
            }
        } else if self.state.running && self.state.orphan {
            if !self.state.miner_alive() {
                self.state.miner_pid = None;
                self.state.orphan = false;
                if cfg.behavior.auto_restart && self.state.running {
                    self.state.restarts += 1;
                    self.state.last_error = Some(format!(
                        "{} — {} {} {}",
                        tr("Miner exited", "Майнер завершился"),
                        tr("restarting in", "перезапуск через"),
                        cfg.behavior.restart_delay_s.max(1),
                        tr("s", "с")
                    ));
                    self.restart_at = Some(
                        Instant::now() + Duration::from_secs(cfg.behavior.restart_delay_s.max(1)),
                    );
                    let _ = self.state.save();
                    events.push(Event::Crashed {
                        code: None,
                        restart_in: cfg.behavior.restart_delay_s.max(1),
                    });
                } else {
                    self.state.running = false;
                    self.state.started_at = None;
                    if let Err(err) = restore_stock(cfg) {
                        self.state.last_error = Some(format!("{}; {err}", text_of(&self.state)));
                        events.push(Event::Error(err.to_string()));
                    }
                    let _ = self.state.save();
                    events.push(Event::Stopped { code: None });
                }
            }
        } else if self.state.running && !self.state.miner_alive() {
            let restore = restore_stock(cfg);
            self.state = State::default();
            if let Err(err) = restore {
                self.state.last_error = Some(err.to_string());
                events.push(Event::Error(err.to_string()));
            }
            let _ = self.state.save();
            events.push(Event::Stopped { code: None });
        } else if !self.state.running {
            if let Some(pid) = state::find_external_miner() {
                self.state.external = true;
                self.state.running = true;
                self.state.miner_pid = Some(pid);
                self.state.started_at = Some(util::now_unix());
                let _ = self.state.save();
                events.push(Event::Started(pid));
            }
        }

        if let Some(at) = self.restart_at {
            if Instant::now() >= at && self.child.is_none() {
                self.restart_at = None;
                match self.spawn(cfg) {
                    Ok(()) => {
                        self.schedule_verify();
                        events.push(Event::Started(self.state.miner_pid.unwrap_or(0)));
                        let _ = self.state.save();
                    }
                    Err(err) => {
                        self.state.running = false;
                        self.state.last_error = Some(format!(
                            "{} {err}",
                            tr(
                                "Unable to restart miner:",
                                "Не удалось перезапустить майнер:"
                            )
                        ));
                        if let Err(reset) = restore_stock(cfg) {
                            events.push(Event::Error(reset.to_string()));
                        }
                        let _ = self.state.save();
                        events.push(Event::Error(
                            self.state.last_error.clone().unwrap_or_default(),
                        ));
                    }
                }
            }
        }

        events.extend(self.check_applied_locks(cfg));
        events
    }

    /// One-shot runtime check: a clock lock that the driver ignored must not
    /// be reported as applied. Memory is decisive under load; an unmatched
    /// core clock is a warning because it legitimately fluctuates.
    fn check_applied_locks(&mut self, cfg: &Config) -> Vec<Event> {
        let Some(at) = self.verify_at else {
            return Vec::new();
        };
        if self.verified || Instant::now() < at {
            return Vec::new();
        }
        if !self.state.running || self.state.external {
            self.verify_at = None;
            return Vec::new();
        }
        let Some(snapshot) = oc::current_boot_snapshot() else {
            self.verify_at = None;
            return Vec::new();
        };
        if snapshot.applied_tuning.is_empty() {
            self.verify_at = None;
            self.verified = true;
            return Vec::new();
        }
        let Ok(gpus) = crate::nvidia::query_all() else {
            self.verify_at = Some(Instant::now() + Duration::from_secs(2));
            return Vec::new();
        };
        let loaded = gpus.iter().any(|gpu| gpu.util_pct.unwrap_or(0) >= 80);
        if !loaded {
            let until = self
                .verify_until
                .unwrap_or_else(|| Instant::now() + Duration::from_secs(300));
            if Instant::now() < until {
                self.verify_at = Some(Instant::now() + Duration::from_secs(2));
                return Vec::new();
            }
            log_line("oc verify skipped: the GPU never reached load");
            self.verify_at = None;
            self.verified = true;
            return Vec::new();
        }

        let issues = oc::runtime_lock_issues(&snapshot, &gpus);
        let mut events = Vec::new();
        if let Some(issue) = issues
            .iter()
            .find(|issue| issue.kind == oc::LockKind::Memory)
        {
            let message = format!(
                "{} #{}: {} {} MHz {} ({} {} MHz). {}",
                tr("GPU", "Карта"),
                issue.index,
                tr("memory lock", "лок памяти"),
                issue.want,
                tr("was not applied", "не применился"),
                tr("now", "сейчас"),
                issue.actual,
                tr(
                    "Remove the memory lock from the profile or check the NVIDIA driver.",
                    "Уберите лок памяти из профиля или проверьте драйвер NVIDIA."
                )
            );
            log_line(&format!("oc verify failed: {message}"));
            let reset = self.stop_internal(cfg, true);
            self.state.last_error = Some(message.clone());
            self.state.running = false;
            let _ = self.state.save();
            if let Err(err) = reset {
                events.push(Event::Error(format!("{message}; {err}")));
            } else {
                events.push(Event::Error(message));
            }
        } else {
            for issue in issues {
                let message = format!(
                    "{} #{}: {} {} MHz {} ({} {} MHz)",
                    tr("GPU", "Карта"),
                    issue.index,
                    tr("core lock", "лок ядра"),
                    issue.want,
                    tr("was not applied", "не применился"),
                    tr("now", "сейчас"),
                    issue.actual
                );
                log_line(&format!("oc verify warning: {message}"));
                events.push(Event::Warning(message));
            }
        }
        self.verify_at = None;
        self.verified = true;
        events
    }

    fn spawn(&mut self, cfg: &Config) -> anyhow::Result<()> {
        let preset = cfg
            .preset()
            .ok_or_else(|| anyhow::anyhow!(tr("No active preset", "Нет активного пресета")))?;
        let binary = cfg
            .miner_path(preset)
            .ok_or_else(|| anyhow::anyhow!(tr("Miner is not installed", "Майнер не установлен")))?;
        if !binary.exists() {
            anyhow::bail!(
                "{} {}",
                tr("Miner binary not found:", "Бинарник майнера не найден:"),
                binary.display()
            );
        }
        if let Some(parent) = self.log_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_path)?;
        let log_err = log.try_clone()?;

        let args = miners::build_args(cfg, preset, &self.log_path)?;
        let mut command = Command::new(&binary);
        command
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log_err));
        if let Some(dir) = binary.parent() {
            command.current_dir(dir);
        }
        for (key, value) in &preset.env {
            command.env(key, value);
        }

        let child = command.spawn().map_err(|e| {
            anyhow::anyhow!(
                "{} {}: {e}",
                tr("Unable to start", "Не удалось запустить"),
                binary.display()
            )
        })?;
        let pid = child.id();
        self.child = Some(child);
        self.state.miner_pid = Some(pid);
        self.state.running = true;
        self.state.external = false;
        self.state.orphan = false;
        if self.state.started_at.is_none() {
            self.state.started_at = Some(util::now_unix());
        }
        self.state.preset_id = cfg.active_preset.clone();
        self.state.miner = Some(preset.miner.clone());
        self.state.miner_version = Some(preset.miner_version.clone());
        self.state.profile = Some(preset.miner.clone());
        Ok(())
    }

    /// Take over a miner that is already running (started by a script).
    pub fn adopt_external(&mut self, cfg: &Config) -> bool {
        let Some(pid) = state::find_external_miner() else {
            return false;
        };
        self.state.external = true;
        self.state.running = true;
        self.state.miner_pid = Some(pid);
        self.state.started_at = Some(util::now_unix());
        self.state.preset_id = cfg.active_preset.clone();
        true
    }
}

fn rollback(snapshot: &OcSnapshot, uuids: &[String]) -> anyhow::Result<()> {
    let mut first_error = None;
    for uuid in uuids {
        let Some(gpu) = snapshot.for_uuid(uuid) else {
            continue;
        };
        if let Err(err) = oc::restore_gpu(gpu, RestoreMode::Snapshot) {
            log_line(&format!("oc rollback failed gpu={}: {err}", gpu.index));
            if first_error.is_none() {
                first_error = Some(err);
            }
        }
    }
    match first_error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

fn restore_with_snapshot(snapshot: &OcSnapshot, mode: RestoreMode) -> anyhow::Result<()> {
    let result = oc::restore_snapshot(snapshot, mode);
    if let Err(ref err) = result {
        log_line(&format!("oc restore failed mode={}: {err}", mode.as_str()));
    } else {
        log_line(&format!("oc restore ok mode={}", mode.as_str()));
    }
    result
}

/// Best-effort stock restore after the miner is gone for good. Keeps the
/// snapshot when the helper fails, so a later Stop can retry.
fn restore_stock(cfg: &Config) -> anyhow::Result<()> {
    if !cfg.behavior.restore_stock_on_stop {
        return Ok(());
    }
    let Some(snapshot) = oc::current_boot_snapshot().filter(|snapshot| snapshot.applied) else {
        return Ok(());
    };
    let result = restore_with_snapshot(&snapshot, RestoreMode::Snapshot);
    if let Err(ref err) = result {
        oc::record_error(&err.to_string());
    }
    result
}

fn text_of(state: &State) -> String {
    state.last_error.clone().unwrap_or_default()
}

/// SIGTERM, then SIGKILL after a grace period.
fn terminate(child: &mut Child) {
    terminate_pid(child.id());
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(_)) => return,
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(_) => break,
        }
    }
    let _ = child.kill();
}

/// Send SIGTERM (then SIGKILL) to a PID.
pub fn terminate_pid(pid: u32) {
    let _ = Command::new("kill")
        .arg("-TERM")
        .arg(pid.to_string())
        .status();
    for _ in 0..50 {
        if !state::pid_matches_miner(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = Command::new("kill")
        .arg("-KILL")
        .arg(pid.to_string())
        .status();
}

/// Append a line to the daemon log (journald captures stdout as well).
pub fn log_line(message: &str) {
    let path = paths::supervisor_log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "[{}] {message}", util::iso8601_utc(util::now_unix()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_messages_are_human_readable() {
        assert!(!Event::Started(42).message().unwrap().is_empty());
        assert!(Event::Crashed {
            code: Some(1),
            restart_in: 5
        }
        .message()
        .unwrap()
        .contains('5'));
        assert!(!Event::UserStopped.message().unwrap().is_empty());
        assert!(!Event::Warning("lock".into()).message().unwrap().is_empty());
    }

    #[test]
    fn start_fails_without_install() {
        let mut cfg = Config::default();
        let mut supervisor = Supervisor::new();
        supervisor.state = State::default();
        assert!(supervisor.start(&cfg).is_err());
        // Installing a non-existent path still fails at the existence check.
        cfg.add_install(crate::config::MinerInstall {
            miner: "wildrig".into(),
            version: "0.51.8".into(),
            path: "/nonexistent/wildrig-multi".into(),
            ..Default::default()
        });
        assert!(supervisor.start(&cfg).is_err());
    }

    #[test]
    fn dead_external_state_does_not_silently_block_start() {
        let mut supervisor = Supervisor::new();
        supervisor.state = State {
            running: false,
            external: true,
            miner_pid: Some(4_000_000),
            ..Default::default()
        };
        let cfg = Config::default();
        // No wallet/install: start must reach that validation error, not
        // return Ok because a stale external flag said "already running".
        assert!(supervisor.start(&cfg).is_err());
        assert!(!supervisor.state.external);
    }

    #[test]
    fn stop_without_running_is_safe() {
        let mut supervisor = Supervisor::new();
        supervisor.state = State::default();
        let mut cfg = Config::default();
        // Never try to restore a real GPU / pkexec from a unit test.
        cfg.behavior.restore_stock_on_stop = false;
        let _ = supervisor.stop(&cfg);
        assert!(!supervisor.state.running);
    }

    #[test]
    fn already_running_is_not_a_silent_noop() {
        let mut supervisor = Supervisor::new();
        supervisor.state = State {
            running: true,
            miner_pid: Some(std::process::id()),
            ..Default::default()
        };
        // A real miner PID is not needed: the child/state check fires first.
        let cfg = Config::default();
        assert!(supervisor.start(&cfg).is_err());
    }
}
