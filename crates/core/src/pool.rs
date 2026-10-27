//! Pool statistics (PearlHash account API).
//!
//! Endpoint: `https://pearlhash.xyz/api/account/<wallet>` (≈60 requests/min).
//! Real schema (verified 26.10.2026):
//!
//! ```json
//! {
//!   "connected_workers": [ ... ],
//!   "estimated_hashrate_15s": 0, "estimated_hashrate_60s": 0, "estimated_hashrate_5m": 0,
//!   "balance_transactions": [ { "amount": 0.057, "reason": "Epoch ... credit",
//!                               "timestamp": 1792426778726, "coin_type": "pearl" } ],
//!   "pending_rewards": { "total_pending_prl": 0.125, "total_pending_mdl": 0,
//!                        "epochs": [ { "amount": 0.011, "share": 1.6e-6, "immature_tx": 2 } ] }
//! }
//! ```
//!
//! `pending_rewards.total_pending_prl` is what is mined but not paid yet;
//! `balance_transactions` are the payouts already sent to the wallet. A generic
//! key-matching fallback keeps other pools working.

use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::minerapi::flatten_numbers;

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PoolStats {
    /// Pool fee from the config (percent), not from the API.
    pub fee_pct: f64,
    /// Mined but not paid out yet, PRL.
    pub balance: Option<f64>,
    /// Sum of the not-yet-matured epoch rewards, PRL.
    pub immature: Option<f64>,
    /// Total already paid out, PRL.
    pub paid: Option<f64>,
    /// Pool-side hash rate estimate of this wallet (H/s).
    pub hashrate: Option<f64>,
    /// Workers currently connected to the pool.
    pub workers_online: Option<u32>,
    /// Unix seconds of the last payout.
    pub last_payout_ts: Option<u64>,
    /// Minimum payout of the pool, PRL (для шкалы «до выплаты»).
    #[serde(default = "default_min_payout")]
    pub min_payout_prl: f64,
}

/// Minimum payout of the pool, in PRL (used for the "до выплаты" bar).
pub const MIN_PAYOUT_PRL: f64 = 1.0;

fn default_min_payout() -> f64 {
    MIN_PAYOUT_PRL
}
/// The account API is rate limited; cache it for a minute.
pub const CACHE_TTL: Duration = Duration::from_secs(60);

pub fn account_url(wallet: &str) -> String {
    format!("https://pearlhash.xyz/api/account/{wallet}")
}

/// Fetch pool statistics for the configured wallet (no caching).
pub fn query(cfg: &Config) -> anyhow::Result<PoolStats> {
    let wallet = cfg.wallet.trim();
    if wallet.is_empty() {
        anyhow::bail!("кошелёк не задан");
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(4))
        .timeout_read(std::time::Duration::from_secs(15))
        .timeout(std::time::Duration::from_secs(20))
        .build();
    let url = account_url(wallet);
    let mut last_error = None;
    for attempt in 0..2 {
        if attempt > 0 {
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        match agent.get(&url).set("Accept", "application/json").call() {
            Ok(response) => match response.into_string() {
                Ok(body) => {
                    let json: serde_json::Value = serde_json::from_str(&body)
                        .map_err(|e| anyhow::anyhow!("пул вернул не JSON: {e}"))?;
                    let mut stats = parse_json(&json);
                    stats.fee_pct = cfg.pool_fee_pct;
                    stats.min_payout_prl = MIN_PAYOUT_PRL;
                    return Ok(stats);
                }
                Err(e) => last_error = Some(format!("не удалось прочитать ответ пула: {e}")),
            },
            Err(e) => last_error = Some(format!("API пула недоступен: {e}")),
        }
    }
    anyhow::bail!(
        "{}",
        last_error.unwrap_or_else(|| "API пула недоступен".to_string())
    );
}

struct CacheEntry {
    wallet: String,
    at: Instant,
    value: Option<PoolStats>,
}

static CACHE: OnceLock<Mutex<Option<CacheEntry>>> = OnceLock::new();

fn cache() -> &'static Mutex<Option<CacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Cached pool query: at most one request per wallet per minute.
pub fn query_cached(cfg: &Config) -> Option<PoolStats> {
    let wallet = cfg.wallet.trim().to_string();
    if wallet.is_empty() {
        return None;
    }
    {
        let guard = cache().lock().ok()?;
        if let Some(entry) = guard.as_ref() {
            if entry.wallet == wallet && entry.at.elapsed() < CACHE_TTL {
                return entry.value.clone();
            }
        }
    }

    let value = query(cfg).ok();
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(CacheEntry {
            wallet,
            at: Instant::now(),
            value: value.clone(),
        });
    }
    value
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
    fn payout_progress_helper() {
        let stats = PoolStats {
            balance: Some(0.25),
            ..Default::default()
        };
        assert!((stats.balance.unwrap() / MIN_PAYOUT_PRL - 0.25).abs() < 1e-9);
    }
}
