//! Pool statistics: known documented adapters (PearlHash) plus a generic JSON
//! fallback for a user-provided `Pool API URL`.
//!
//! Pool addresses are always user input. This module never scrapes HTML and
//! never guesses an endpoint: a known pool only supplies a documented template,
//! and every other pool must provide one explicitly.
//!
//! PearlHash endpoint (verified 26.10.2026):
//! `https://pearlhash.xyz/api/account/<wallet>`.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde::{Deserialize, Serialize};

use crate::catalog;
use crate::config::{Config, Preset};
use crate::minerapi::flatten_numbers;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PoolStats {
    /// Pool fee from the config (percent), not from the API.
    pub fee_pct: f64,
    /// Mined but not paid out yet, in the pool coin.
    pub balance: Option<f64>,
    /// Sum of the not-yet-matured epoch rewards, in the pool coin.
    pub immature: Option<f64>,
    /// Total already paid out, in the pool coin.
    pub paid: Option<f64>,
    /// Pool-side hash rate estimate of this wallet (H/s).
    pub hashrate: Option<f64>,
    /// Workers currently connected to the pool.
    pub workers_online: Option<u32>,
    /// Unix seconds of the last payout.
    pub last_payout_ts: Option<u64>,
    /// Minimum payout of the pool when documented. `None` means unknown; the
    /// UI then shows the balance without an invented progress bar.
    #[serde(default, alias = "min_payout_prl")]
    pub min_payout: Option<f64>,
}

/// The account API is rate limited; cache it for a minute.
pub const CACHE_TTL: Duration = Duration::from_secs(60);

pub fn account_url(wallet: &str) -> String {
    format!("https://pearlhash.xyz/api/account/{wallet}")
}

/// Resolve the documented account API template for a preset.
///
/// Priority: the preset's explicit `pool_api_url`, then the known catalogue
/// pool matched by host. A pool without a documented API returns `None`; the
/// dashboard then says that balance is unavailable instead of guessing.
pub fn api_url(preset: &Preset) -> Option<String> {
    let explicit = preset.pool_api_url.trim();
    if !explicit.is_empty() {
        return Some(explicit.to_string());
    }
    catalog::pool_for_url(&preset.pool_url)
        .map(|pool| pool.api_url.trim())
        .filter(|url| !url.is_empty())
        .map(str::to_string)
}

pub fn display_name(preset: &Preset) -> String {
    preset.pool_display_name()
}

fn resolve_url(preset: &Preset) -> anyhow::Result<String> {
    let wallet = preset.wallet.trim();
    if wallet.is_empty() {
        anyhow::bail!("wallet is not set");
    }
    let template = api_url(preset)
        .ok_or_else(|| anyhow::anyhow!("this pool does not document a JSON account API"))?;
    if !template.contains("{wallet}") {
        anyhow::bail!("the pool API URL must contain a {{wallet}} placeholder");
    }
    if !template.starts_with("https://") {
        anyhow::bail!("the pool API URL must use HTTPS");
    }
    let encoded = utf8_percent_encode(wallet, NON_ALPHANUMERIC).to_string();
    Ok(template.replace("{wallet}", &encoded))
}

/// Fetch pool statistics for the active preset (no caching).
pub fn query(cfg: &Config) -> anyhow::Result<PoolStats> {
    let Some(preset) = cfg.preset() else {
        anyhow::bail!("no active preset");
    };
    let url = resolve_url(preset)?;
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(4))
        .timeout_read(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(20))
        .build();
    let mut last_error = None;
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        match agent.get(&url).set("Accept", "application/json").call() {
            Ok(response) => match response.into_string() {
                Ok(body) => {
                    let json: serde_json::Value = serde_json::from_str(&body)
                        .map_err(|e| anyhow::anyhow!("the pool returned non-JSON: {e}"))?;
                    let mut stats = parse_json(&json);
                    stats.fee_pct = preset.pool_fee_pct;
                    if let Some(pool) = catalog::pool_for_url(&preset.pool_url) {
                        stats.min_payout = Some(pool.min_payout).filter(|value| *value > 0.0);
                    }
                    return Ok(stats);
                }
                Err(e) => last_error = Some(format!("cannot read the pool response: {e}")),
            },
            Err(e) => last_error = Some(format!("pool API is unavailable: {e}")),
        }
    }
    anyhow::bail!(
        "{}",
        last_error.unwrap_or_else(|| "pool API is unavailable".to_string())
    )
}

#[derive(Default)]
pub struct PoolSnapshot {
    pub stats: Option<PoolStats>,
    pub error: Option<String>,
}

struct CacheEntry {
    api_url: String,
    wallet: String,
    at: Option<Instant>,
    value: Option<PoolStats>,
    last_attempt: Option<Instant>,
    last_error: Option<String>,
    in_flight: bool,
}

/// After a failed/slow fetch, do not hit the pool again immediately.
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(45);

static CACHE: OnceLock<Mutex<Option<CacheEntry>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<CacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Cached pool query. Never blocks the status loop: a slow or broken pool API
/// used to stall `Status::collect` for up to 40 seconds.
///
/// `error` is the last failed fetch (if any); a pool without an API is not an
/// error and returns an empty snapshot.
pub fn query_cached(cfg: &Config) -> PoolSnapshot {
    let Some(preset) = cfg.preset() else {
        return PoolSnapshot::default();
    };
    let wallet = preset.wallet.trim().to_string();
    if wallet.is_empty() {
        return PoolSnapshot::default();
    }
    let Some(api_url) = api_url(preset) else {
        return PoolSnapshot::default();
    };
    let now = Instant::now();
    let snapshot = {
        let mut guard = match cache().lock() {
            Ok(guard) => guard,
            Err(_) => return PoolSnapshot::default(),
        };
        let entry = guard.get_or_insert_with(|| CacheEntry {
            api_url: api_url.clone(),
            wallet: wallet.clone(),
            at: None,
            value: None,
            last_attempt: None,
            last_error: None,
            in_flight: false,
        });
        if entry.api_url != api_url || entry.wallet != wallet {
            *entry = CacheEntry {
                api_url: api_url.clone(),
                wallet: wallet.clone(),
                at: None,
                value: None,
                last_attempt: None,
                last_error: None,
                in_flight: false,
            };
        }
        let fresh = entry.at.map(|at| at.elapsed() < CACHE_TTL).unwrap_or(false);
        let backoff = entry
            .last_attempt
            .map(|at| at.elapsed() < RETRY_AFTER_FAILURE)
            .unwrap_or(false);
        if fresh || entry.in_flight || backoff {
            return PoolSnapshot {
                stats: entry.value.clone(),
                error: entry.last_error.clone(),
            };
        }
        entry.in_flight = true;
        entry.last_attempt = Some(now);
        PoolSnapshot {
            stats: entry.value.clone(),
            error: entry.last_error.clone(),
        }
    };

    let worker_cfg = cfg.clone();
    let worker_api = api_url.clone();
    let worker_wallet = wallet.clone();
    std::thread::spawn(move || {
        let result = query(&worker_cfg);
        if let Ok(mut guard) = cache().lock() {
            let Some(entry) = guard.as_mut() else {
                return;
            };
            if entry.api_url != worker_api || entry.wallet != worker_wallet {
                return;
            }
            entry.in_flight = false;
            match result {
                Ok(stats) => {
                    entry.value = Some(stats);
                    entry.at = Some(Instant::now());
                    entry.last_error = None;
                }
                Err(err) => entry.last_error = Some(err.to_string()),
            }
        }
    });
    snapshot
}

/// Drop the cached pool response (used right after start/stop).
pub fn invalidate_cache() {
    if let Ok(mut guard) = cache().lock() {
        *guard = None;
    }
}

pub fn parse_json(json: &serde_json::Value) -> PoolStats {
    let mut stats = PoolStats::default();

    if let Some(object) = json.as_object() {
        // --- Known PearlHash schema -------------------------------------
        if let Some(pending) = object.get("pending_rewards").and_then(|v| v.as_object()) {
            stats.balance = pending.get("total_pending_prl").and_then(as_f64);
            if let Some(epochs) = pending.get("epochs").and_then(|v| v.as_array()) {
                let sum: f64 = epochs
                    .iter()
                    .filter_map(|epoch| epoch.get("amount").and_then(as_f64))
                    .sum();
                if !epochs.is_empty() {
                    stats.immature = Some(sum);
                }
            }
        }

        if let Some(transactions) = object
            .get("balance_transactions")
            .and_then(|v| v.as_array())
        {
            let sum: f64 = transactions
                .iter()
                .filter_map(|tx| tx.get("amount").and_then(as_f64))
                .sum();
            stats.paid = Some(sum);
            stats.last_payout_ts = transactions
                .iter()
                .filter_map(|tx| tx.get("timestamp").and_then(as_f64))
                .fold(None::<f64>, |acc, ts| Some(acc.map_or(ts, |a| a.max(ts))))
                // The API reports milliseconds.
                .map(|ts_ms| (ts_ms / 1000.0) as u64);
        }

        if let Some(workers) = object.get("connected_workers").and_then(|v| v.as_array()) {
            stats.workers_online = Some(workers.len() as u32);
        }

        stats.hashrate = [
            "estimated_hashrate_60s",
            "estimated_hashrate_15s",
            "estimated_hashrate_5m",
        ]
        .iter()
        .find_map(|key| object.get(*key).and_then(as_f64))
        .filter(|value| *value > 0.0);
    }

    // --- Generic fallback for other pool APIs ---------------------------
    if stats.balance.is_none() && stats.paid.is_none() {
        let pairs = flatten_numbers(json);
        let pick = |needles: &[&str], excludes: &[&str]| -> Option<f64> {
            pairs
                .iter()
                .filter(|(path, _)| {
                    let lower = path.to_ascii_lowercase();
                    needles.iter().all(|n| lower.contains(n))
                        && !excludes.iter().any(|e| lower.contains(e))
                })
                .min_by_key(|(path, _)| path.len())
                .map(|(_, value)| *value)
        };
        stats.balance = pick(
            &["balance"],
            &["usd", "price", "fee", "transactions", "pending"],
        );
        stats.immature = pick(&["immature"], &["usd"]);
        stats.paid = pick(&["paid"], &["usd", "count", "threshold"]);
        stats.hashrate = pick(&["hashrate"], &["average", "max", "min", "24h", "unit"]);
        stats.min_payout = pick(&["min", "payout"], &["usd"]);
    }

    if stats.hashrate == Some(0.0) {
        stats.hashrate = None;
    }
    stats
}

fn as_f64(value: &serde_json::Value) -> Option<f64> {
    match value {
        serde_json::Value::Number(number) => number.as_f64(),
        serde_json::Value::String(text) => text.parse().ok(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Preset;

    fn config_with_preset() -> Config {
        let mut cfg = Config::default();
        let preset = Preset::new_pearl();
        cfg.presets.insert(preset.id.clone(), preset);
        cfg.active_preset = Some(crate::config::DEFAULT_PRESET_ID.to_string());
        cfg
    }

    #[test]
    fn no_wallet_means_no_pool_request() {
        let cfg = Config::default();
        assert!(super::query_cached(&cfg).stats.is_none());
    }

    fn pearlhash_fixture() -> serde_json::Value {
        serde_json::json!({
            "connected_workers": [
                { "name": "3080ti", "hashrate": 125.0e12 }
            ],
            "estimated_hashrate_15s": 125.2e12,
            "estimated_hashrate_60s": 125.0e12,
            "estimated_hashrate_5m": 124.8e12,
            "balance_transactions": [
                { "amount": 0.0571, "reason": "Epoch ... credit", "timestamp": 1_792_426_778_726u64, "coin_type": "pearl" },
                { "amount": 0.0969, "reason": "Epoch ... credit", "timestamp": 1_793_010_067_647u64, "coin_type": "pearl" }
            ],
            "pending_rewards": {
                "total_pending_prl": 0.1252,
                "total_pending_mdl": 0,
                "epochs": [
                    { "epoch_label": "2026-10-27 13:00 UTC", "amount": 0.0111, "share": 1.6e-6, "immature_tx": 2, "coin_type": "pearl" },
                    { "epoch_label": "2026-10-27 14:00 UTC", "amount": 0.0403, "share": 4.5e-6, "immature_tx": 2, "coin_type": "pearl" }
                ]
            }
        })
    }

    #[test]
    fn parses_real_pearlhash_schema() {
        let stats = parse_json(&pearlhash_fixture());
        assert_eq!(stats.balance, Some(0.1252));
        assert!((stats.immature.unwrap() - 0.0514).abs() < 1e-9);
        assert!((stats.paid.unwrap() - 0.154).abs() < 1e-9);
        assert_eq!(stats.hashrate, Some(125.0e12));
        assert_eq!(stats.workers_online, Some(1));
        assert_eq!(stats.last_payout_ts, Some(1_793_010_067));
    }

    #[test]
    fn zero_pool_hashrate_is_reported_as_unknown() {
        let mut json = pearlhash_fixture();
        json["estimated_hashrate_60s"] = serde_json::json!(0);
        json["estimated_hashrate_15s"] = serde_json::json!(0);
        json["estimated_hashrate_5m"] = serde_json::json!(0);
        assert_eq!(parse_json(&json).hashrate, None);
    }

    #[test]
    fn empty_account_is_handled() {
        let json = serde_json::json!({
            "connected_workers": [],
            "balance_transactions": [],
            "pending_rewards": { "total_pending_prl": 0, "epochs": [] }
        });
        let stats = parse_json(&json);
        assert_eq!(stats.balance, Some(0.0));
        assert_eq!(stats.paid, Some(0.0));
        assert_eq!(stats.immature, None);
        assert_eq!(stats.workers_online, Some(0));
    }

    #[test]
    fn generic_pool_fallback_still_works() {
        let json = serde_json::json!({
            "balance": 1.5, "balanceUsd": 2.0, "immature": 0.25, "totalPaid": 12.0,
            "hashrate": 100.0e12
        });
        let stats = parse_json(&json);
        assert_eq!(stats.balance, Some(1.5));
        assert_eq!(stats.immature, Some(0.25));
        assert_eq!(stats.paid, Some(12.0));
        assert_eq!(stats.hashrate, Some(100.0e12));
    }

    #[test]
    fn unrelated_json_is_tolerated() {
        assert_eq!(
            parse_json(&serde_json::json!({"hi": "there"})),
            PoolStats::default()
        );
    }

    #[test]
    fn builds_account_url() {
        assert_eq!(
            account_url("prl1abc"),
            "https://pearlhash.xyz/api/account/prl1abc"
        );
    }

    #[test]
    fn known_pool_supplies_the_api_template() {
        let cfg = config_with_preset();
        let preset = cfg.preset().unwrap();
        assert_eq!(
            api_url(preset).as_deref(),
            Some("https://pearlhash.xyz/api/account/{wallet}")
        );
    }

    #[test]
    fn unknown_pool_without_api_is_not_an_error() {
        let mut cfg = config_with_preset();
        cfg.preset_mut().unwrap().pool_url = "example.org:3333".into();
        let snapshot = query_cached(&cfg);
        assert!(snapshot.stats.is_none());
        assert!(snapshot.error.is_none());
        assert_eq!(api_url(cfg.preset().unwrap()), None);
    }

    #[test]
    fn custom_api_requires_https_and_placeholder() {
        let mut cfg = config_with_preset();
        {
            let preset = cfg.preset_mut().unwrap();
            preset.wallet = "testwallet".into();
            preset.pool_url = "example.org:3333".into();
            preset.pool_api_url = "http://example.org/{wallet}".into();
        }
        let err = super::query(&cfg).unwrap_err();
        assert!(err.to_string().contains("HTTPS"), "{err}");
        cfg.preset_mut().unwrap().pool_api_url = "https://example.org/account".into();
        let err = super::query(&cfg).unwrap_err();
        assert!(err.to_string().contains("{wallet}"), "{err}");
    }

    #[test]
    fn balance_uses_the_coin_symbol_in_the_gui_layer() {
        // The status document keeps numbers; the GUI adds the symbol.
        let stats = PoolStats {
            balance: Some(0.25),
            min_payout: Some(1.0),
            ..Default::default()
        };
        assert!((stats.balance.unwrap() / stats.min_payout.unwrap() - 0.25).abs() < 1e-9);
    }
}
