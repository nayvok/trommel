//! The status contract shared by the daemon, CLI and GUI.

use serde::{Deserialize, Serialize};
use std::path::Path;

use crate::catalog;
use crate::config::{Config, GpuProfile};
use crate::i18n::tr;
use crate::minerapi::{self, MinerStats};
use crate::nvidia::{self, GpuStats};
use crate::pool::{self, PoolStats};
use crate::state::State;
use crate::util;

pub const CONTRACT_VERSION: u32 = 5;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct StateView {
    pub running: bool,
    pub uptime_s: u64,
    pub restarts: u32,
    pub last_error: Option<String>,
    pub external: bool,
    pub preset_id: String,
    pub preset_name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct MinerInfo {
    pub id: String,
    pub name: String,
    pub version: String,
    pub short_description: String,
    pub license: String,
    pub source_url: String,
    pub verified: bool,
    pub installed: bool,
    pub custom: bool,
    pub miner_fee_pct: f64,
    pub pool_fee_pct: f64,
    pub total_fee_pct: f64,
    pub fee_configurable: bool,
    pub fee_source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CoinView {
    pub id: String,
    pub symbol: String,
    pub name: String,
    pub algo: String,
    pub wallet_prefix: String,
    pub hashrate_unit: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PoolView {
    pub name: String,
    pub url: String,
    pub fee_pct: f64,
    pub api_available: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct TotalView {
    pub hashrate: Option<f64>,
    /// Whole-rig `power.draw`: every detected card, including excluded ones.
    pub power_w: Option<f64>,
    /// Power of the participating cards only. Efficiency uses this number.
    #[serde(default)]
    pub mining_power_w: Option<f64>,
    /// Power per hashrate in the coin's display unit (`J/TH`, `J/MH`, ...).
    #[serde(default)]
    pub efficiency: Option<f64>,
    #[serde(default)]
    pub efficiency_unit: String,
    /// Deprecated compatibility field: filled only when the unit really is
    /// J/TH. New clients should read `efficiency` + `efficiency_unit`.
    pub efficiency_j_per_th: Option<f64>,
    pub temp_max_c: Option<u32>,
    pub gpus_enabled: u32,
    pub gpus_total: u32,
}

/// One card's role in the applied session. `enabled` is the profile switch;
/// `mining` is true only while this card is part of a running session that
/// Trommel owns (not an external miner, whose device set is unknown).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct GpuParticipation {
    pub index: u32,
    pub uuid: String,
    pub name: String,
    pub enabled: bool,
    pub mining: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OcView {
    /// Tuning from the current snapshot was applied (or an apply was started).
    pub applied: bool,
    /// The tuning is still on the cards while the miner is stopped. The GUI
    /// shows a banner with a Restore action in this state.
    pub needs_restore: bool,
    pub snapshot_available: bool,
    pub stale: bool,
    pub boot_id: String,
    pub restore_on_stop: bool,
    pub last_error: Option<String>,
    pub snapshot_created_at: Option<u64>,
    pub snapshot_boot_id: Option<String>,
    /// Summary for the first card; the full per-GPU targets are in `oc_report`.
    pub restore_power_w: Option<u32>,
    pub driver_default_w: Option<u32>,
    pub was_tuned: bool,
}

fn default_temp_limit() -> u32 {
    85
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Status {
    pub contract_version: u32,
    pub state: StateView,
    pub preset: Option<String>,
    pub miner_info: MinerInfo,
    pub gpus: Vec<GpuStats>,
    /// Explicit role of every detected card in the applied profile/session.
    #[serde(default)]
    pub participation: Vec<GpuParticipation>,
    /// First GPU, kept for older clients.
    pub gpu: Option<GpuStats>,
    pub gpu_error: Option<String>,
    pub total: TotalView,
    pub miner: MinerStats,
    pub pool: Option<PoolStats>,
    /// Applied coin metadata; empty when no preset is active.
    #[serde(default)]
    pub coin: CoinView,
    /// Applied pool endpoint and whether an account API is available.
    #[serde(default)]
    pub pool_view: Option<PoolView>,
    /// Last pool API error, when one occurred and the snapshot kept a value.
    #[serde(default)]
    pub pool_error: Option<String>,
    pub oc: OcView,
    pub warnings: Vec<String>,
    pub total_fee_pct: f64,
    pub expected_hashrate: f64,
    #[serde(default = "default_temp_limit")]
    pub temp_limit: u32,
    pub ts: u64,
    pub ts_iso: String,
    pub version: String,
}

/// Compare requested clock locks with what the driver actually reports.
///
/// A hard `-lgc`/`-lmc` lock should match exactly; the tolerances only absorb
/// rounding. Core is checked under load because an idle GPU legitimately
/// downclocks below the lock.
fn lock_warnings(cfg: &Config, gpus: &[GpuStats], running: bool, uptime_s: u64) -> Vec<String> {
    if !running {
        return Vec::new();
    }
    let Some(profile) = cfg.gpu_profile() else {
        return Vec::new();
    };
    let mut warnings = Vec::new();
    for gpu in gpus {
        let tuning = profile.tuning_for(&gpu.uuid);
        if !tuning.enabled {
            continue;
        }
        if let (Some(want), Some(have)) = (tuning.memory_clock_mhz, gpu.mem_mhz) {
            if want.abs_diff(have) > 50 {
                warnings.push(tr(
                    &format!(
                        "GPU #{}: memory lock {want} MHz is not active (now {have} MHz)",
                        gpu.index
                    ),
                    &format!(
                        "Карта #{}: лок памяти {want} MHz не применился (сейчас {have} MHz)",
                        gpu.index
                    ),
                ));
            }
        }
        if uptime_s >= 10 && gpu.util_pct.unwrap_or(0) >= 80 {
            if let (Some(want), Some(have)) = (tuning.core_clock_mhz, gpu.core_mhz) {
                if want.abs_diff(have) > 100 {
                    warnings.push(tr(
                        &format!(
                            "GPU #{}: core lock {want} MHz is not active (now {have} MHz)",
                            gpu.index
                        ),
                        &format!(
                            "Карта #{}: лок ядра {want} MHz не применился (сейчас {have} MHz)",
                            gpu.index
                        ),
                    ));
                }
            }
        }
    }
    warnings
}

/// Warn when the detected cards are all switched off, even if the profile
/// default would enable a card that is no longer present. `Config::warnings`
/// cannot know the detected UUIDs, so the status document adds this one.
fn enabled_gpu_warning(cfg: &Config, gpus: &[GpuStats]) -> Option<String> {
    if gpus.is_empty() {
        return None;
    }
    let profile = cfg.gpu_profile()?;
    if gpus.iter().any(|gpu| profile.tuning_for(&gpu.uuid).enabled) {
        return None;
    }
    Some(tr(
        "No GPU is selected for mining",
        "Ни одна видеокарта не выбрана для майнинга",
    ))
}

/// Build the per-card role list. `mining` belongs to the running session, not
/// the draft: a switch change must not make a card look idle before Apply.
fn participation_view(
    profile: Option<&GpuProfile>,
    gpus: &[GpuStats],
    running_ours: bool,
) -> Vec<GpuParticipation> {
    gpus.iter()
        .map(|gpu| {
            let enabled = profile
                .map(|profile| profile.tuning_for(&gpu.uuid).enabled)
                .unwrap_or(true);
            GpuParticipation {
                index: gpu.index,
                uuid: gpu.uuid.clone(),
                name: gpu.name.clone(),
                enabled,
                mining: enabled && running_ours,
            }
        })
        .collect()
}

/// User-facing notes for a partial participation: which cards are excluded
/// and whether the selected miner can honour that at all.
fn participation_warnings(cfg: &Config, gpus: &[GpuStats]) -> Vec<String> {
    let Some(profile) = cfg.gpu_profile() else {
        return Vec::new();
    };
    let excluded: Vec<&GpuStats> = gpus
        .iter()
        .filter(|gpu| !profile.tuning_for(&gpu.uuid).enabled)
        .collect();
    if excluded.is_empty() || excluded.len() == gpus.len() {
        return Vec::new();
    }
    let en = excluded
        .iter()
        .map(|gpu| format!("GPU #{}", gpu.index))
        .collect::<Vec<_>>()
        .join(", ");
    let ru = excluded
        .iter()
        .map(|gpu| format!("карта #{}", gpu.index))
        .collect::<Vec<_>>()
        .join(", ");
    let mut warnings = vec![tr(
        &format!("Excluded from mining: {en}"),
        &format!("Исключены из майнинга: {ru}"),
    )];
    if let Some(preset) = cfg.preset() {
        if !crate::miners::supports_device_selection(&preset.miner) {
            warnings.push(tr(
                &format!(
                    "The selected miner ({}) cannot choose GPUs; enable every card or choose a miner that documents device selection",
                    preset.miner
                ),
                &format!(
                    "Выбранный майнер ({}) не умеет выбирать видеокарты; включите все карты или выберите майнер с поддержкой выбора устройств",
                    preset.miner
                ),
            ));
        }
    }
    warnings
}

/// Rig power: `power.draw` summed over every detected card.
///
/// Cards excluded from mining still appear (an idle card draws power), and a
/// card whose reading is `N/A` is skipped, so the sum can be lower than a wall
/// meter by that card's draw.
fn total_power(gpus: &[GpuStats]) -> Option<f64> {
    gpus.iter()
        .filter_map(|gpu| gpu.power_w)
        .reduce(|a, b| a + b)
}

/// Power of the cards that participate in the session. Efficiency must not be
/// charged for an excluded idle card; the whole-rig number stays in
/// [`TotalView::power_w`].
fn participating_power(cfg: &Config, gpus: &[GpuStats]) -> Option<f64> {
    let profile = cfg.gpu_profile()?;
    gpus.iter()
        .filter(|gpu| profile.tuning_for(&gpu.uuid).enabled)
        .filter_map(|gpu| gpu.power_w)
        .reduce(|a, b| a + b)
}

/// Unit and scale used for hashrate display. Efficiency is expressed per this
/// unit, so a MH/s coin reports J/MH instead of the old hardcoded J/TH.
fn display_unit(hs: f64, preferred: &str) -> (&'static str, f64) {
    const UNITS: [(&str, f64); 6] = [
        ("H/s", 1.0),
        ("kH/s", 1e3),
        ("MH/s", 1e6),
        ("GH/s", 1e9),
        ("TH/s", 1e12),
        ("PH/s", 1e15),
    ];
    let preferred = preferred.trim();
    if !preferred.is_empty() && !preferred.eq_ignore_ascii_case("auto") {
        if let Some((unit, scale)) = UNITS.iter().find(|(unit, _)| *unit == preferred) {
            let value = hs / scale;
            if (0.01..1000.0).contains(&value) {
                return (unit, *scale);
            }
        }
    }
    let mut selected = UNITS[0];
    for (unit, scale) in UNITS {
        if hs >= scale {
            selected = (unit, scale);
        }
    }
    selected
}

fn efficiency_unit_name(unit: &str) -> String {
    let suffix = unit.trim_end_matches("/s");
    format!("J/{suffix}")
}

fn efficiency_view(
    cfg: &Config,
    gpus: &[GpuStats],
    hashrate: Option<f64>,
    preferred_unit: &str,
    external: bool,
) -> (Option<f64>, Option<f64>, String) {
    if external {
        return (None, None, String::new());
    }
    let (Some(power), Some(hashrate)) = (
        participating_power(cfg, gpus),
        hashrate.filter(|value| *value > 0.0),
    ) else {
        return (None, None, String::new());
    };
    if power <= 0.0 {
        return (None, None, String::new());
    }
    let (unit, scale) = display_unit(hashrate, preferred_unit);
    let efficiency = power / (hashrate / scale);
    let unit_name = efficiency_unit_name(unit);
    let legacy = (unit == "TH/s").then_some(efficiency);
    (Some(efficiency), legacy, unit_name)
}

/// Warn once when the watchdog is enabled but the preset cannot supply the
/// percentage baseline; the zero/stale/card checks still run.
fn watchdog_warning(cfg: &Config) -> Option<String> {
    if !cfg.behavior.watchdog_enabled {
        return None;
    }
    if cfg
        .preset()
        .and_then(|preset| preset.expected_hashrate)
        .filter(|value| *value > 0.0)
        .is_some()
    {
        return None;
    }
    Some(tr(
        "The hashrate watchdog is on, but the preset has no expected hashrate; only 0 H/s, a missing card and stale data are checked",
        "Сторож хешрейта включён, но в пресете не задан ожидаемый хешрейт; проверяются только 0 H/s, выпавшая карта и устаревшие данные",
    ))
}

/// One calm status warning for per-card data the mapping could not prove.
fn mapping_warning(report: &crate::minerapi::GpuMappingReport) -> Option<String> {
    if !report.unmapped_devices.is_empty() || !report.ambiguous_buses.is_empty() {
        let devices = if report.unmapped_devices.is_empty() {
            report
                .ambiguous_buses
                .iter()
                .map(|bus| format!("bus {bus}"))
                .collect::<Vec<_>>()
                .join(", ")
        } else {
            report.unmapped_devices.join(", ")
        };
        return Some(tr(
            &format!(
                "Per-card miner data could not be matched to a GPU ({devices}); those cards show —"
            ),
            &format!(
                "Данные майнера по картам не удалось сопоставить с GPU ({devices}); на этих картах —"
            ),
        ));
    }
    if !report.missing_cards.is_empty() {
        let cards = report
            .missing_cards
            .iter()
            .map(|index| format!("GPU #{index}"))
            .collect::<Vec<_>>()
            .join(", ");
        return Some(tr(
            &format!("The miner reports no per-card hashrate for {cards}"),
            &format!("Майнер не сообщает хешрейт для {cards}"),
        ));
    }
    None
}

/// Hottest readable card, ties resolved to the lowest index.
///
/// Every multi-card consumer (the `total` document, the overheat alert and the
/// temperature note) goes through this: reading only the first card misses a
/// hot second card, which is exactly what a dual-GPU rig hits.
pub fn hottest_gpu(gpus: &[GpuStats]) -> Option<&GpuStats> {
    gpus.iter()
        .filter(|gpu| gpu.temp_c.is_some())
        .max_by_key(|gpu| (gpu.temp_c.unwrap_or_default(), std::cmp::Reverse(gpu.index)))
}

/// One note while the hottest card sits at or above the profile limit.
fn temp_warning(gpus: &[GpuStats], temp_limit: u32) -> Option<String> {
    let gpu = hottest_gpu(gpus)?;
    let temp = gpu.temp_c?;
    (temp >= temp_limit).then(|| {
        format!(
            "{} #{}: {} °C ({} {} °C)",
            tr("GPU", "Карта"),
            gpu.index,
            temp,
            tr("limit", "порог"),
            temp_limit
        )
    })
}

impl Status {
    /// Hottest readable card, see [`hottest_gpu`].
    pub fn hottest_gpu(&self) -> Option<&GpuStats> {
        hottest_gpu(&self.gpus)
    }

    /// Build a status snapshot. Blocking: calls `nvidia-smi` and HTTP APIs.
    pub fn collect(cfg: &Config, state: &State) -> Self {
        let preset = cfg.preset();
        let gpus = nvidia::query_all();
        let (gpus, gpu_error) = match gpus {
            Ok(list) => (list, None),
            Err(err) => (Vec::new(), Some(err.to_string())),
        };
        let gpu = gpus.first().cloned();

        let driver = preset
            .and_then(|preset| catalog::miner(&preset.miner))
            .map(|miner| miner.driver)
            .unwrap_or_default();
        let mut miner = if state.running {
            minerapi::query_api_for(preset.map(|p| p.effective_api_port()).unwrap_or(0), driver)
                .unwrap_or_else(|_| {
                    let tail = read_tail(&crate::paths::log_path(), 96 * 1024);
                    let mut stats = minerapi::parse_log(&tail);
                    if stats.is_empty() {
                        stats.source = "no data".to_string();
                    }
                    stats
                })
        } else {
            MinerStats::default()
        };

        let (pool, pool_error) = if state.running || state.external {
            let snapshot = pool::query_cached(cfg);
            (snapshot.stats, snapshot.error)
        } else {
            (None, None)
        };

        let profile = cfg.gpu_profile();
        let temp_limit = cfg.behavior.temp_limit_c;
        let gpus_total = gpus.len() as u32;
        let running_ours = state.running && !state.external;
        let participation = match profile {
            Some(profile) => participation_view(Some(profile), &gpus, running_ours),
            None => Vec::new(),
        };
        let gpus_enabled = participation.iter().filter(|card| card.enabled).count() as u32;
        let participating_uuids: Vec<String> = participation
            .iter()
            .filter(|card| card.enabled)
            .map(|card| card.uuid.clone())
            .collect();
        let mapping = minerapi::map_to_gpus(&mut miner, &gpus, &participating_uuids);
        let power = total_power(&gpus);
        let mining_power = if state.external {
            None
        } else {
            participating_power(cfg, &gpus)
        };
        let temp_max = hottest_gpu(&gpus).and_then(|gpu| gpu.temp_c);
        let hashrate = miner.primary();
        let preferred_unit = preset
            .map(|preset| preset.hashrate_unit())
            .unwrap_or("auto");
        let (efficiency, efficiency_j_per_th, efficiency_unit) =
            efficiency_view(cfg, &gpus, hashrate, preferred_unit, state.external);

        let (miner_info, total_fee_pct, expected_hashrate) = match preset {
            Some(preset) => {
                let def = catalog::miner(&preset.miner);
                let install = cfg.install_for(preset);
                let info = MinerInfo {
                    id: preset.miner.clone(),
                    name: def
                        .map(|m| m.name.clone())
                        .unwrap_or_else(|| preset.miner.clone()),
                    version: preset.miner_version.clone(),
                    short_description: def
                        .map(|m| m.short_description().to_string())
                        .unwrap_or_default(),
                    license: def.map(|m| m.license.clone()).unwrap_or_default(),
                    source_url: def.map(|m| m.source_url.clone()).unwrap_or_default(),
                    verified: install.map(|i| i.verified).unwrap_or(false),
                    installed: install.is_some(),
                    custom: install.map(|i| i.custom).unwrap_or(false),
                    miner_fee_pct: preset.effective_miner_fee_pct(),
                    pool_fee_pct: preset.pool_fee_pct,
                    total_fee_pct: preset.total_fee_pct(),
                    fee_configurable: def
                        .map(|m| m.fee_for(&preset.algo).configurable)
                        .unwrap_or(false),
                    fee_source: if preset.miner_fee_override_pct.is_some() {
                        "override".into()
                    } else {
                        "catalog".into()
                    },
                };
                (
                    info,
                    preset.total_fee_pct(),
                    preset.expected_hashrate.unwrap_or(0.0),
                )
            }
            None => (MinerInfo::default(), 0.0, 0.0),
        };

        let snapshot = crate::oc::load_snapshot();
        let boot_id = crate::oc::current_boot_id();
        // A snapshot from another boot is diagnostic only: it must never make
        // the UI offer a restore or claim that settings are still applied.
        let fresh = snapshot
            .as_ref()
            .filter(|snapshot| snapshot.boot_id == boot_id);
        let applied = fresh.map(|s| s.applied).unwrap_or(false);
        let first = fresh.and_then(|s| s.gpus.first());
        let oc = OcView {
            applied,
            needs_restore: applied && !state.running,
            snapshot_available: fresh.is_some(),
            stale: snapshot.is_some() && fresh.is_none(),
            boot_id,
            restore_on_stop: cfg.behavior.restore_stock_on_stop,
            last_error: fresh.and_then(|s| s.last_error.clone()),
            snapshot_created_at: fresh.map(|s| s.created_at),
            snapshot_boot_id: fresh.map(|s| s.boot_id.clone()),
            restore_power_w: first
                .and_then(|gpu| gpu.restore_power_w(crate::oc::RestoreMode::Snapshot)),
            driver_default_w: first
                .and_then(|gpu| gpu.power_default_w)
                .map(|value| value.round() as u32),
            was_tuned: first.map(|gpu| gpu.was_tuned).unwrap_or(false),
        };

        let coin = preset
            .map(|preset| CoinView {
                id: preset.coin.clone(),
                symbol: preset.symbol().to_string(),
                name: preset.coin_name.clone(),
                algo: preset.algo.clone(),
                wallet_prefix: preset.wallet_prefix().to_string(),
                hashrate_unit: preset.hashrate_unit().to_string(),
            })
            .unwrap_or_default();
        let pool_view = preset.map(|preset| PoolView {
            name: pool::display_name(preset),
            url: preset.pool_url.clone(),
            fee_pct: preset.pool_fee_pct,
            api_available: pool::api_url(preset).is_some(),
        });

        let ts = util::now_unix();
        let mut warnings = cfg.warnings();
        if let Some(warning) = enabled_gpu_warning(cfg, &gpus) {
            if !warnings.contains(&warning) {
                warnings.push(warning);
            }
        }
        warnings.extend(participation_warnings(cfg, &gpus));
        if running_ours {
            if let Some(warning) = mapping_warning(&mapping) {
                warnings.push(warning);
            }
        }
        warnings.extend(lock_warnings(cfg, &gpus, running_ours, state.uptime_s()));
        if let Some(warning) = temp_warning(&gpus, temp_limit) {
            warnings.push(warning);
        }
        if let Some(error) = &pool_error {
            warnings.push(format!(
                "{} {error}",
                tr("Pool API error:", "Ошибка API пула:")
            ));
        }
        if let Some(warning) = watchdog_warning(cfg) {
            warnings.push(warning);
        }
        if oc.needs_restore {
            warnings.push(tr(
                "GPU power/clocks are still applied; use Restore stock in Settings",
                "Питание/частоты GPU всё ещё применены — верните сток в «Настройках»",
            ));
        }
        if let Some(err) = &oc.last_error {
            warnings.push(format!(
                "{} {err}",
                tr("GPU restore failed:", "Возврат стока не удался:")
            ));
        }
        Self {
            contract_version: CONTRACT_VERSION,
            state: StateView {
                running: state.running || state.external,
                uptime_s: state.uptime_s(),
                restarts: state.restarts,
                last_error: state.last_error.clone(),
                external: state.external,
                preset_id: state
                    .preset_id
                    .clone()
                    .or_else(|| cfg.active_preset.clone())
                    .unwrap_or_default(),
                preset_name: preset.map(|p| p.name.clone()).unwrap_or_default(),
            },
            preset: cfg.active_preset.clone(),
            miner_info,
            gpus,
            participation,
            gpu,
            gpu_error,
            total: TotalView {
                hashrate,
                power_w: power,
                mining_power_w: mining_power,
                efficiency,
                efficiency_unit,
                efficiency_j_per_th,
                temp_max_c: temp_max,
                gpus_enabled,
                gpus_total,
            },
            miner,
            pool,
            coin,
            pool_view,
            pool_error,
            oc,
            warnings,
            total_fee_pct,
            expected_hashrate,
            temp_limit,
            ts,
            ts_iso: util::iso8601_utc(ts),
            version: crate::VERSION.to_string(),
        }
    }

    /// Cheap snapshot without touching the network (used on stop).
    pub fn stopped(cfg: &Config) -> Self {
        let ts = util::now_unix();
        let preset = cfg.preset();
        Self {
            contract_version: CONTRACT_VERSION,
            state: StateView {
                running: false,
                preset_id: cfg.active_preset.clone().unwrap_or_default(),
                preset_name: preset.map(|p| p.name.clone()).unwrap_or_default(),
                ..Default::default()
            },
            preset: cfg.active_preset.clone(),
            coin: preset
                .map(|preset| CoinView {
                    id: preset.coin.clone(),
                    symbol: preset.symbol().to_string(),
                    name: preset.coin_name.clone(),
                    algo: preset.algo.clone(),
                    wallet_prefix: preset.wallet_prefix().to_string(),
                    hashrate_unit: preset.hashrate_unit().to_string(),
                })
                .unwrap_or_default(),
            pool_view: preset.map(|preset| PoolView {
                name: pool::display_name(preset),
                url: preset.pool_url.clone(),
                fee_pct: preset.pool_fee_pct,
                api_available: pool::api_url(preset).is_some(),
            }),
            total_fee_pct: cfg.total_fee_pct(),
            expected_hashrate: preset.and_then(|p| p.expected_hashrate).unwrap_or(0.0),
            temp_limit: cfg.behavior.temp_limit_c,
            ts,
            ts_iso: util::iso8601_utc(ts),
            version: crate::VERSION.to_string(),
            ..Default::default()
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_else(|_| "{}".to_string())
    }

    pub fn from_json(text: &str) -> anyhow::Result<Self> {
        Ok(serde_json::from_str(text)?)
    }

    pub fn acceptance_pct(&self) -> Option<f64> {
        let accepted = self.miner.accepted? as f64;
        let rejected = self.miner.rejected.unwrap_or(0) as f64;
        let total = accepted + rejected;
        if total <= 0.0 {
            return None;
        }
        Some(accepted / total * 100.0)
    }
}

/// Read at most `max_bytes` from the end of a file (log fallback).
pub fn read_tail(path: &Path, max_bytes: usize) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(_) => return String::new(),
    };
    let len = file.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes as u64);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buffer = Vec::new();
    if file.read_to_end(&mut buffer).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buffer).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_contract_round_trip() {
        let cfg = Config::default();
        let status = Status::stopped(&cfg);
        let json = status.to_json();
        let back = Status::from_json(&json).unwrap();
        assert_eq!(status, back);
        for key in [
            "\"state\"",
            "\"gpu\"",
            "\"gpus\"",
            "\"participation\"",
            "\"miner\"",
            "\"miner_info\"",
            "\"pool\"",
            "\"contract_version\"",
            "\"ts_iso\"",
            "\"total_fee_pct\"",
            "\"efficiency_unit\"",
            "\"mining_power_w\"",
        ] {
            assert!(json.contains(key), "missing key {key} in {json}");
        }
    }

    #[test]
    fn status_schema_documents_the_current_contract() {
        let schema: serde_json::Value =
            serde_json::from_str(include_str!("../../../docs/schema/status.schema.json")).unwrap();
        assert_eq!(schema["properties"]["contract_version"]["const"], 5);
        assert_eq!(CONTRACT_VERSION, 5);
        let miner = &schema["$defs"]["minerStats"]["properties"];
        assert!(miner["gpus"].is_object(), "schema missing miner.gpus");
        let total = &schema["$defs"]["totalView"]["properties"];
        assert!(total["efficiency"].is_object(), "schema missing efficiency");
        assert!(total["efficiency_unit"].is_object(), "schema missing unit");
    }

    #[test]
    fn stopped_status_has_no_metrics() {
        let status = Status::stopped(&Config::default());
        assert!(!status.state.running);
        assert!(status.gpu.is_none());
        assert!(status.gpus.is_empty());
        assert!(status.miner.is_empty());
        assert!(status.pool.is_none());
    }

    #[test]
    fn tail_of_missing_file_is_empty() {
        assert!(read_tail(Path::new("/nonexistent/file.log"), 1024).is_empty());
    }

    fn config_with_preset() -> Config {
        let mut cfg = Config::default();
        let preset = crate::config::Preset::new_pearl();
        cfg.presets.insert(preset.id.clone(), preset);
        cfg.active_preset = Some(crate::config::DEFAULT_PRESET_ID.to_string());
        cfg
    }

    fn tuned_config(uuid: &str) -> Config {
        let mut cfg = config_with_preset();
        let profile = &mut cfg.preset_mut().unwrap().gpu;
        profile.per_gpu.insert(
            uuid.to_string(),
            crate::config::GpuTuning {
                enabled: true,
                power_limit_w: Some(300),
                core_clock_mhz: Some(1620),
                memory_clock_mhz: Some(810),
                custom: true,
                ..Default::default()
            },
        );
        cfg
    }

    fn tuned_gpu(uuid: &str) -> GpuStats {
        GpuStats {
            index: 0,
            uuid: uuid.into(),
            core_mhz: Some(1620),
            mem_mhz: Some(810),
            util_pct: Some(100),
            ..Default::default()
        }
    }

    #[test]
    fn matching_locks_do_not_warn() {
        let uuid = "GPU-test";
        let cfg = tuned_config(uuid);
        let warnings = lock_warnings(&cfg, &[tuned_gpu(uuid)], true, 60);
        assert!(warnings.is_empty(), "{warnings:?}");
    }

    #[test]
    fn memory_mismatch_warns_even_at_idle() {
        let uuid = "GPU-test";
        let cfg = tuned_config(uuid);
        let mut gpu = tuned_gpu(uuid);
        gpu.mem_mhz = Some(9501);
        gpu.util_pct = Some(0);
        let warnings = lock_warnings(&cfg, &[gpu], true, 60);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("810"), "{warnings:?}");
        assert!(warnings[0].contains("9501"), "{warnings:?}");
    }

    #[test]
    fn core_mismatch_is_ignored_while_idle_or_starting() {
        let uuid = "GPU-test";
        let cfg = tuned_config(uuid);
        let mut gpu = tuned_gpu(uuid);
        gpu.core_mhz = Some(1770);
        gpu.util_pct = Some(10);
        assert!(lock_warnings(&cfg, &[gpu.clone()], true, 60).is_empty());
        gpu.util_pct = Some(100);
        assert!(lock_warnings(&cfg, &[gpu.clone()], true, 5).is_empty());
        let warnings = lock_warnings(&cfg, &[gpu], true, 60);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("1620"), "{warnings:?}");
        assert!(warnings[0].contains("1770"), "{warnings:?}");
    }

    #[test]
    fn disabled_gpu_does_not_warn() {
        let uuid = "GPU-test";
        let mut cfg = tuned_config(uuid);
        cfg.preset_mut()
            .unwrap()
            .gpu
            .per_gpu
            .get_mut(uuid)
            .unwrap()
            .enabled = false;
        let mut gpu = tuned_gpu(uuid);
        gpu.mem_mhz = Some(9501);
        assert!(lock_warnings(&cfg, &[gpu], true, 60).is_empty());
    }

    #[test]
    fn all_gpus_disabled_warns_even_with_an_enabled_default() {
        let uuid = "GPU-test";
        let mut cfg = tuned_config(uuid);
        cfg.preset_mut()
            .unwrap()
            .gpu
            .per_gpu
            .get_mut(uuid)
            .unwrap()
            .enabled = false;
        let warning = enabled_gpu_warning(&cfg, &[tuned_gpu(uuid)]).expect("warning");
        assert!(!warning.is_empty());
        assert!(enabled_gpu_warning(&cfg, &[]).is_none());
    }

    #[test]
    fn an_enabled_gpu_does_not_warn() {
        let uuid = "GPU-test";
        let cfg = tuned_config(uuid);
        assert!(enabled_gpu_warning(&cfg, &[tuned_gpu(uuid)]).is_none());
    }

    fn card(index: u32, temp: Option<u32>, power: Option<f64>) -> GpuStats {
        GpuStats {
            index,
            uuid: format!("GPU-{index}"),
            temp_c: temp,
            power_w: power,
            ..Default::default()
        }
    }

    #[test]
    fn rig_power_sums_every_card_including_disabled_ones() {
        let gpus = [
            card(0, Some(74), Some(145.6)),
            card(1, Some(81), Some(137.9)),
        ];
        assert_eq!(total_power(&gpus), Some(283.5));
        assert_eq!(total_power(&[]), None);
    }

    #[test]
    fn rig_power_skips_cards_without_a_reading() {
        let gpus = [card(0, Some(74), Some(145.6)), card(1, Some(81), None)];
        assert_eq!(total_power(&gpus), Some(145.6));
    }

    #[test]
    fn hottest_card_is_not_always_the_first() {
        let gpus = [
            card(0, Some(74), Some(145.6)),
            card(1, Some(81), Some(137.9)),
        ];
        assert_eq!(hottest_gpu(&gpus).map(|gpu| gpu.index), Some(1));
        assert_eq!(hottest_gpu(&[]), None);
        assert_eq!(hottest_gpu(&[card(0, None, Some(1.0))]), None);
    }

    #[test]
    fn tying_cards_report_the_lowest_index() {
        let gpus = [card(0, Some(70), None), card(1, Some(70), None)];
        assert_eq!(hottest_gpu(&gpus).map(|gpu| gpu.index), Some(0));
    }

    fn excluded_config(uuid: &str) -> Config {
        let mut cfg = config_with_preset();
        cfg.preset_mut().unwrap().gpu.per_gpu.insert(
            uuid.to_string(),
            crate::config::GpuTuning {
                enabled: false,
                ..Default::default()
            },
        );
        cfg
    }

    #[test]
    fn efficiency_uses_participating_power_and_the_coin_unit() {
        let uuid = "GPU-test";
        let cfg = tuned_config(uuid);
        let gpus = [
            card(0, Some(70), Some(300.0)),
            card(1, Some(60), Some(130.0)),
        ];
        let (efficiency, legacy, unit) =
            efficiency_view(&cfg, &gpus, Some(125.0e12), "TH/s", false);
        assert_eq!(unit, "J/TH");
        assert!((efficiency.unwrap() - 3.44).abs() < 1e-9);
        assert_eq!(legacy, efficiency);

        let mut excluded = cfg.clone();
        excluded.preset_mut().unwrap().gpu.per_gpu.insert(
            "GPU-1".into(),
            crate::config::GpuTuning {
                enabled: false,
                ..Default::default()
            },
        );
        let (efficiency, _, unit) =
            efficiency_view(&excluded, &gpus, Some(125.0e12), "TH/s", false);
        assert_eq!(unit, "J/TH");
        assert!((efficiency.unwrap() - 2.4).abs() < 1e-9);
    }

    #[test]
    fn efficiency_unit_follows_mh_coins_and_the_legacy_field_is_null() {
        let cfg = tuned_config("GPU-test");
        let gpus = [card(0, Some(70), Some(300.0))];
        let (efficiency, legacy, unit) = efficiency_view(&cfg, &gpus, Some(100.0e6), "MH/s", false);
        assert_eq!(unit, "J/MH");
        assert!((efficiency.unwrap() - 3.0).abs() < 1e-9);
        assert_eq!(legacy, None);
        assert_eq!(display_unit(100.0e6, "auto"), ("MH/s", 1e6));
        assert_eq!(display_unit(125.0e12, "auto"), ("TH/s", 1e12));
    }

    #[test]
    fn external_miner_has_no_efficiency() {
        let cfg = tuned_config("GPU-test");
        let gpus = [card(0, Some(70), Some(300.0))];
        let (efficiency, legacy, unit) = efficiency_view(&cfg, &gpus, Some(125.0e12), "TH/s", true);
        assert_eq!(efficiency, None);
        assert_eq!(legacy, None);
        assert!(unit.is_empty());
        assert_eq!(participating_power(&cfg, &gpus), Some(300.0));
    }

    #[test]
    fn watchdog_without_expected_hashrate_is_explained_once() {
        let mut cfg = config_with_preset();
        cfg.behavior.watchdog_enabled = true;
        cfg.preset_mut().unwrap().expected_hashrate = None;
        assert!(watchdog_warning(&cfg).is_some());
        cfg.preset_mut().unwrap().expected_hashrate = Some(125.0e12);
        assert!(watchdog_warning(&cfg).is_none());
        cfg.behavior.watchdog_enabled = false;
        assert!(watchdog_warning(&cfg).is_none());
    }

    #[test]
    fn mapping_warnings_name_the_unmatched_cards_once() {
        let report = crate::minerapi::GpuMappingReport {
            mapped: 1,
            unmapped_devices: vec!["gpu1".into()],
            ambiguous_buses: Vec::new(),
            missing_cards: Vec::new(),
        };
        let warning = mapping_warning(&report).expect("warning");
        assert!(warning.contains("gpu1"), "{warning}");
        assert!(warning.contains('—'), "{warning}");
        let missing = crate::minerapi::GpuMappingReport {
            mapped: 1,
            unmapped_devices: Vec::new(),
            ambiguous_buses: Vec::new(),
            missing_cards: vec![1],
        };
        let warning = mapping_warning(&missing).expect("warning");
        assert!(warning.contains("GPU #1"), "{warning}");
        assert!(mapping_warning(&crate::minerapi::GpuMappingReport::default()).is_none());
    }

    #[test]
    fn participation_marks_excluded_cards_and_only_mining_while_ours() {
        let cfg = excluded_config("GPU-1");
        let gpus = [
            card(0, Some(60), Some(300.0)),
            card(1, Some(55), Some(130.0)),
        ];
        let view = participation_view(cfg.gpu_profile(), &gpus, true);
        assert!(view[0].enabled && view[0].mining, "{view:?}");
        assert!(!view[1].enabled && !view[1].mining, "{view:?}");

        // Stopped: the card stays enabled but is not mining.
        let stopped = participation_view(cfg.gpu_profile(), &gpus, false);
        assert!(stopped[0].enabled && !stopped[0].mining, "{stopped:?}");

        // External miner: Trommel does not know its device set, so no card is
        // claimed as mining by this session.
        let external = participation_view(cfg.gpu_profile(), &gpus, false);
        assert!(external.iter().all(|card| !card.mining), "{external:?}");
    }

    #[test]
    fn participation_warning_names_the_excluded_card() {
        let cfg = excluded_config("GPU-1");
        let gpus = [card(0, None, None), card(1, None, None)];
        let warnings = participation_warnings(&cfg, &gpus);
        assert!(
            warnings.iter().any(|warning| warning.contains("#1")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().all(|warning| !warning.contains("#0")),
            "{warnings:?}"
        );
    }

    #[test]
    fn participation_warning_names_an_unsupported_miner() {
        let mut cfg = excluded_config("GPU-1");
        cfg.preset_mut().unwrap().miner = "private-build".into();
        let gpus = [card(0, None, None), card(1, None, None)];
        let warnings = participation_warnings(&cfg, &gpus);
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("private-build")),
            "{warnings:?}"
        );
    }

    #[test]
    fn an_all_excluded_rig_does_not_repeat_the_warning() {
        let mut cfg = excluded_config("GPU-1");
        cfg.preset_mut().unwrap().gpu.default.enabled = false;
        let gpus = [card(0, None, None), card(1, None, None)];
        assert!(participation_warnings(&cfg, &gpus).is_empty());
        assert!(enabled_gpu_warning(&cfg, &gpus).is_some());
    }

    #[test]
    fn a_hot_second_card_adds_the_temperature_note() {
        let gpus = [card(0, Some(74), None), card(1, Some(81), None)];
        let warning = temp_warning(&gpus, 80).expect("warning");
        assert!(warning.contains("81"), "{warning}");
        assert!(warning.contains("80"), "{warning}");
        assert!(warning.contains("#1"), "{warning}");
        // One degree under the limit is not a note, and an unreadable card
        // must not fake one.
        assert!(temp_warning(&gpus, 82).is_none());
        assert!(temp_warning(&[card(0, None, None)], 80).is_none());
    }
}
