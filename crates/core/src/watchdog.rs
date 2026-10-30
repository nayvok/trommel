//! Hashrate watchdog: detect a live but degraded miner and, only when the user
//! explicitly opted in, restart it through the existing supervisor path.
//!
//! The state machine takes samples and a wall-clock value; it never spawns a
//! process itself. `trommeld` executes `PerformRestart`/`Stop` and feeds the
//! result back with [`Watchdog::restart_finished`].

use crate::config::{Config, WatchdogAction};
use crate::i18n::tr;
use crate::supervisor::restart_delay_s;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WatchdogGpu {
    pub index: u32,
    pub enabled: bool,
    pub hashrate: Option<f64>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct WatchdogSample {
    pub running: bool,
    pub owned: bool,
    pub external: bool,
    pub busy: bool,
    /// A crash restart is already scheduled; do not fight the supervisor.
    pub restart_scheduled: bool,
    pub uptime_s: u64,
    pub started_at: u64,
    pub status_ts: u64,
    pub now: u64,
    pub poll_interval_s: u64,
    pub expected_hashrate: Option<f64>,
    pub total_hashrate: Option<f64>,
    pub source: String,
    pub per_gpu: Vec<WatchdogGpu>,
}

impl Action {
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Notify(message) | Self::Stop(message) => Some(message),
            Self::Restart { message, .. } => Some(message),
            Self::PerformRestart { .. } => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Notify(String),
    Restart {
        delay_s: u64,
        attempt: u32,
        message: String,
    },
    PerformRestart {
        attempt: u32,
    },
    Stop(String),
}

#[derive(Debug, Clone, PartialEq)]
enum Phase {
    Off,
    Warmup,
    Ok {
        since: u64,
    },
    Low {
        since: u64,
        notified: bool,
    },
    Zero {
        since: u64,
        notified: bool,
    },
    CardDown {
        since: u64,
        notified: bool,
    },
    WaitRestart {
        at: u64,
        attempt: u32,
        reason: String,
    },
    Exhausted,
}

pub struct Watchdog {
    phase: Phase,
    restart_count: u32,
    no_data_since: Option<u64>,
    last_no_data_notify: Option<u64>,
    last_stale_notify: Option<u64>,
    last_started_at: u64,
}

impl Default for Watchdog {
    fn default() -> Self {
        Self {
            phase: Phase::Off,
            restart_count: 0,
            no_data_since: None,
            last_no_data_notify: None,
            last_stale_notify: None,
            last_started_at: 0,
        }
    }
}

fn warmup_s(window_s: u64) -> u64 {
    window_s.clamp(30, 120)
}

fn stale_after_s(poll_interval_s: u64) -> u64 {
    (poll_interval_s.max(1) * 3).max(90)
}

fn reset_after_s() -> u64 {
    900
}

impl Watchdog {
    pub fn reset(&mut self) {
        self.phase = Phase::Off;
        self.no_data_since = None;
        self.last_no_data_notify = None;
        self.last_stale_notify = None;
        self.restart_count = 0;
    }

    /// Feed one sample. Actions are returned at most once per episode.
    pub fn observe(&mut self, cfg: &Config, sample: &WatchdogSample) -> Vec<Action> {
        let behavior = &cfg.behavior;
        if !behavior.watchdog_enabled || !sample.owned || sample.external || !sample.running {
            self.reset();
            return Vec::new();
        }
        if sample.busy || sample.restart_scheduled {
            // A crash restart or an OC operation is already in flight; keep
            // the timers but do not add a second action.
            return Vec::new();
        }
        if sample.started_at != self.last_started_at {
            self.last_started_at = sample.started_at;
            self.restart_count = 0;
            self.no_data_since = None;
            self.last_no_data_notify = None;
            self.last_stale_notify = None;
            self.phase = Phase::Warmup;
        }

        let now = sample.now;
        let window = behavior.watchdog_window_s.max(60);
        let warmup = warmup_s(window);
        let has_data = sample.source != "no data" && sample.total_hashrate.is_some();

        // Wait for the miner to load, and do not judge a freshly started card.
        if sample.uptime_s < warmup {
            if !has_data {
                self.no_data_since.get_or_insert(now);
            }
            self.phase = Phase::Warmup;
            return Vec::new();
        }

        if !has_data {
            let since = *self.no_data_since.get_or_insert(now);
            if now.saturating_sub(since) >= window {
                // Re-notify at most once per window, not on every second.
                let repeated = self
                    .last_no_data_notify
                    .map(|at| now.saturating_sub(at) < window)
                    .unwrap_or(false);
                self.phase = Phase::Warmup;
                if repeated {
                    return Vec::new();
                }
                self.last_no_data_notify = Some(now);
                return vec![Action::Notify(tr(
                    &format!(
                        "No miner data for {} min; check the miner API or log",
                        window / 60
                    ),
                    &format!(
                        "Нет данных майнера {} мин; проверьте API или лог майнера",
                        window / 60
                    ),
                ))];
            }
            self.phase = Phase::Warmup;
            return Vec::new();
        }
        self.no_data_since = None;

        if now.saturating_sub(sample.status_ts) > stale_after_s(sample.poll_interval_s) {
            self.phase = Phase::Warmup;
            let repeated = self
                .last_stale_notify
                .map(|at| now.saturating_sub(at) < window)
                .unwrap_or(false);
            if repeated {
                return Vec::new();
            }
            self.last_stale_notify = Some(now);
            return vec![Action::Notify(tr(
                "Status samples are stale; check trommeld",
                "Статус не обновляется; проверьте trommeld",
            ))];
        }
        self.last_stale_notify = None;

        if let Phase::WaitRestart { at, attempt, .. } = self.phase {
            if self.classify(sample, behavior, window).is_none() {
                self.phase = Phase::Ok { since: now };
                return vec![Action::Notify(tr(
                    "Hashrate recovered before the scheduled restart",
                    "Хешрейт восстановился до запланированного перезапуска",
                ))];
            }
            if now >= at {
                return vec![Action::PerformRestart { attempt }];
            }
            return Vec::new();
        }

        let Some((kind, reason)) = self.classify(sample, behavior, window) else {
            let since = match self.phase {
                Phase::Ok { since } => since,
                _ => now,
            };
            self.phase = Phase::Ok { since };
            if now.saturating_sub(since) >= reset_after_s() {
                self.restart_count = 0;
            }
            return Vec::new();
        };

        let (since, notified) = match self.phase {
            Phase::Low { since, notified } if kind == "low" => (since, notified),
            Phase::Zero { since, notified } if kind == "zero" => (since, notified),
            Phase::CardDown { since, notified } if kind == "card" => (since, notified),
            _ => (now, false),
        };
        let phase = match kind {
            "low" => Phase::Low { since, notified },
            "zero" => Phase::Zero { since, notified },
            _ => Phase::CardDown { since, notified },
        };
        self.phase = phase;
        if notified || now.saturating_sub(since) < window {
            return Vec::new();
        }
        self.phase = match kind {
            "low" => Phase::Low {
                since,
                notified: true,
            },
            "zero" => Phase::Zero {
                since,
                notified: true,
            },
            _ => Phase::CardDown {
                since,
                notified: true,
            },
        };

        match behavior.watchdog_action {
            WatchdogAction::Notify => vec![Action::Notify(reason)],
            WatchdogAction::Restart => {
                if self.restart_count >= behavior.watchdog_max_restarts {
                    self.phase = Phase::Exhausted;
                    return vec![Action::Stop(tr(
                        &format!(
                            "Mining stopped after {} watchdog restarts; check the miner log",
                            behavior.watchdog_max_restarts
                        ),
                        &format!(
                            "Майнинг остановлен после {} перезапусков сторожа; проверьте лог майнера",
                            behavior.watchdog_max_restarts
                        ),
                    ))];
                }
                let attempt = self.restart_count + 1;
                let delay = restart_delay_s(cfg, attempt);
                self.phase = Phase::WaitRestart {
                    at: now + delay,
                    attempt,
                    reason: reason.clone(),
                };
                vec![Action::Restart {
                    delay_s: delay,
                    attempt,
                    message: tr(
                        &format!(
                            "Restarting the miner in {delay} s ({attempt}/{}): {reason}",
                            behavior.watchdog_max_restarts
                        ),
                        &format!(
                            "Перезапуск майнера через {delay} с ({attempt}/{}): {reason}",
                            behavior.watchdog_max_restarts
                        ),
                    ),
                }]
            }
        }
    }

    /// The daemon executed (or failed to execute) `PerformRestart`.
    pub fn restart_finished(&mut self, cfg: &Config, success: bool) -> Vec<Action> {
        self.restart_count = self.restart_count.saturating_add(1);
        if success {
            self.phase = Phase::Warmup;
            return vec![Action::Notify(tr(
                "Watchdog restarted the miner; waiting for the warm-up",
                "Сторож перезапустил майнер; ждём прогрева",
            ))];
        }
        if self.restart_count >= cfg.behavior.watchdog_max_restarts {
            self.phase = Phase::Exhausted;
            return vec![Action::Stop(tr(
                "Watchdog could not restart the miner; mining is stopped",
                "Сторож не смог перезапустить майнер; майнинг остановлен",
            ))];
        }
        self.phase = Phase::Warmup;
        vec![Action::Notify(tr(
            "Watchdog restart failed; will try again after the next window",
            "Перезапуск сторожем не удался; следующая попытка после окна",
        ))]
    }

    fn classify(
        &self,
        sample: &WatchdogSample,
        behavior: &crate::config::Behavior,
        _window: u64,
    ) -> Option<(&'static str, String)> {
        let total = sample.total_hashrate?;
        let expected = sample.expected_hashrate.filter(|value| *value > 0.0);
        if total <= 0.0 {
            return Some((
                "zero",
                tr("The miner reports 0 H/s", "Майнер сообщает 0 H/s"),
            ));
        }
        let cards: Vec<u32> = sample
            .per_gpu
            .iter()
            .filter(|card| card.enabled)
            .filter(|card| {
                card.hashrate == Some(0.0)
                    || (card.hashrate.is_none()
                        && sample
                            .per_gpu
                            .iter()
                            .any(|other| other.enabled && other.hashrate.is_some()))
            })
            .map(|card| card.index)
            .collect();
        if !cards.is_empty() {
            let list = cards
                .iter()
                .map(|index| format!("GPU #{index}"))
                .collect::<Vec<_>>()
                .join(", ");
            return Some((
                "card",
                tr(
                    &format!("No hashrate from {list}"),
                    &format!("Нет хешрейта от {list}"),
                ),
            ));
        }
        let expected = expected?;
        let threshold = expected * behavior.watchdog_threshold_pct as f64 / 100.0;
        if total < threshold {
            let percent = (total / expected * 100.0).round() as u32;
            return Some((
                "low",
                tr(
                    &format!(
                        "Hashrate is {percent}% of expected ({:.0}% threshold)",
                        behavior.watchdog_threshold_pct
                    ),
                    &format!(
                        "Хешрейт {percent}% от ожидаемого (порог {:.0}%)",
                        behavior.watchdog_threshold_pct
                    ),
                ),
            ));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, WatchdogAction};

    fn cfg(action: WatchdogAction) -> Config {
        let mut cfg = Config::default();
        cfg.behavior.watchdog_enabled = true;
        cfg.behavior.watchdog_threshold_pct = 80;
        cfg.behavior.watchdog_window_s = 60;
        cfg.behavior.watchdog_action = action;
        cfg.behavior.watchdog_max_restarts = 3;
        cfg.behavior.restart_delay_s = 2;
        cfg
    }

    fn sample(now: u64, total: f64) -> WatchdogSample {
        WatchdogSample {
            running: true,
            owned: true,
            busy: false,
            external: false,
            restart_scheduled: false,
            uptime_s: 300,
            started_at: 1,
            status_ts: now,
            now,
            poll_interval_s: 1,
            expected_hashrate: Some(100.0),
            total_hashrate: Some(total),
            source: "api".into(),
            per_gpu: vec![
                WatchdogGpu {
                    index: 0,
                    enabled: true,
                    hashrate: Some(total),
                },
                WatchdogGpu {
                    index: 1,
                    enabled: true,
                    hashrate: Some(total),
                },
            ],
        }
    }

    #[test]
    fn disabled_external_and_stopped_are_ignored() {
        let mut watchdog = Watchdog::default();
        let mut disabled = cfg(WatchdogAction::Notify);
        disabled.behavior.watchdog_enabled = false;
        let mut s = sample(1_000, 1.0);
        s.uptime_s = 300;
        assert!(watchdog.observe(&disabled, &s).is_empty());

        let cfg = cfg(WatchdogAction::Notify);
        s.external = true;
        assert!(watchdog.observe(&cfg, &s).is_empty());
        s.external = false;
        s.owned = false;
        assert!(watchdog.observe(&cfg, &s).is_empty());
        s.owned = true;
        s.running = false;
        assert!(watchdog.observe(&cfg, &s).is_empty());
    }

    #[test]
    fn warmup_and_healthy_samples_do_nothing() {
        let mut watchdog = Watchdog::default();
        let cfg = cfg(WatchdogAction::Notify);
        let mut s = sample(1_000, 1.0);
        s.uptime_s = 10;
        assert!(watchdog.observe(&cfg, &s).is_empty());
        let s = sample(1_060, 100.0);
        assert!(watchdog.observe(&cfg, &s).is_empty());
    }

    #[test]
    fn low_hashrate_notifies_only_after_the_window() {
        let mut watchdog = Watchdog::default();
        let cfg = cfg(WatchdogAction::Notify);
        assert!(watchdog.observe(&cfg, &sample(1_000, 50.0)).is_empty());
        let actions = watchdog.observe(&cfg, &sample(1_060, 50.0));
        assert!(
            matches!(actions.as_slice(), [Action::Notify(_)]),
            "{actions:?}"
        );
        assert!(watchdog.observe(&cfg, &sample(1_070, 50.0)).is_empty());
    }

    #[test]
    fn recovery_before_the_window_cancels_the_alert() {
        let mut watchdog = Watchdog::default();
        let cfg = cfg(WatchdogAction::Notify);
        assert!(watchdog.observe(&cfg, &sample(1_000, 50.0)).is_empty());
        assert!(watchdog.observe(&cfg, &sample(1_030, 100.0)).is_empty());
        assert!(watchdog.observe(&cfg, &sample(1_100, 50.0)).is_empty());
        assert!(watchdog.observe(&cfg, &sample(1_140, 50.0)).is_empty());
    }

    #[test]
    fn restart_is_explicit_and_uses_the_existing_backoff() {
        let mut watchdog = Watchdog::default();
        let cfg = cfg(WatchdogAction::Restart);
        assert!(watchdog.observe(&cfg, &sample(1_000, 50.0)).is_empty());
        let actions = watchdog.observe(&cfg, &sample(1_060, 50.0));
        let Action::Restart {
            delay_s, attempt, ..
        } = &actions[0]
        else {
            panic!("expected restart, got {actions:?}");
        };
        assert_eq!(*attempt, 1);
        assert_eq!(*delay_s, 2);
        assert!(watchdog.observe(&cfg, &sample(1_061, 50.0)).is_empty());
        assert!(matches!(
            watchdog.observe(&cfg, &sample(1_062, 50.0)).as_slice(),
            [Action::PerformRestart { attempt: 1 }]
        ));
        let actions = watchdog.restart_finished(&cfg, true);
        assert!(
            matches!(actions.as_slice(), [Action::Notify(_)]),
            "{actions:?}"
        );
    }

    #[test]
    fn restart_limit_stops_instead_of_looping() {
        let mut watchdog = Watchdog::default();
        let mut cfg = cfg(WatchdogAction::Restart);
        cfg.behavior.watchdog_max_restarts = 1;
        assert!(watchdog.observe(&cfg, &sample(1_000, 50.0)).is_empty());
        let actions = watchdog.observe(&cfg, &sample(1_060, 50.0));
        assert!(matches!(actions[0], Action::Restart { attempt: 1, .. }));
        let actions = watchdog.restart_finished(&cfg, false);
        assert!(
            matches!(actions.as_slice(), [Action::Stop(_)]),
            "{actions:?}"
        );
    }

    #[test]
    fn expected_zero_skips_the_percentage_check() {
        let mut watchdog = Watchdog::default();
        let cfg = cfg(WatchdogAction::Notify);
        let mut s = sample(1_000, 50.0);
        s.expected_hashrate = None;
        assert!(watchdog.observe(&cfg, &s).is_empty());
        assert!(watchdog.observe(&cfg, &sample(1_060, 50.0)).is_empty());
    }

    #[test]
    fn zero_hashrate_is_detected_without_expected() {
        let mut watchdog = Watchdog::default();
        let cfg = cfg(WatchdogAction::Notify);
        let mut s = sample(1_000, 0.0);
        s.expected_hashrate = None;
        assert!(watchdog.observe(&cfg, &s).is_empty());
        s.now = 1_060;
        s.status_ts = 1_060;
        let actions = watchdog.observe(&cfg, &s);
        assert!(
            matches!(actions.as_slice(), [Action::Notify(_)]),
            "{actions:?}"
        );
    }

    #[test]
    fn a_zero_card_is_named_even_when_the_total_looks_healthy() {
        let mut watchdog = Watchdog::default();
        let cfg = cfg(WatchdogAction::Notify);
        let mut s = sample(1_000, 100.0);
        s.per_gpu[0].hashrate = Some(0.0);
        s.per_gpu[1].hashrate = Some(100.0);
        assert!(watchdog.observe(&cfg, &s).is_empty());
        s.now = 1_060;
        s.status_ts = 1_060;
        let actions = watchdog.observe(&cfg, &s);
        assert!(
            matches!(actions.as_slice(), [Action::Notify(message)] if message.contains("GPU #0")),
            "{actions:?}"
        );
    }

    #[test]
    fn stale_status_is_not_a_silent_restart() {
        let mut watchdog = Watchdog::default();
        let cfg = cfg(WatchdogAction::Restart);
        let mut s = sample(10_000, 1.0);
        s.status_ts = 1_000;
        let actions = watchdog.observe(&cfg, &s);
        assert!(
            matches!(actions.as_slice(), [Action::Notify(_)]),
            "{actions:?}"
        );
    }

    #[test]
    fn explicit_start_resets_the_restart_count() {
        let mut watchdog = Watchdog::default();
        let mut cfg = cfg(WatchdogAction::Restart);
        cfg.behavior.watchdog_max_restarts = 2;
        assert!(watchdog.observe(&cfg, &sample(1_000, 50.0)).is_empty());
        assert!(matches!(
            watchdog.observe(&cfg, &sample(1_060, 50.0))[0],
            Action::Restart { attempt: 1, .. }
        ));
        let mut restarted = sample(1_100, 50.0);
        restarted.started_at = 2;
        restarted.uptime_s = 0;
        watchdog.restart_finished(&cfg, true);
        assert!(watchdog.observe(&cfg, &restarted).is_empty());
    }
}
