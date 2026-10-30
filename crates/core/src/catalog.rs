//! Built-in coin, pool and miner catalogue.
//!
//! The catalogue is data, not code: adding a miner version or a new option
//! should not require touching the UI. Binaries are never bundled; each
//! version points at an upstream release and may carry a SHA-256 checksum.
//!
//! v4 adds the capability data the UI needs:
//! - a driver name for the core argument shape,
//! - a per-algorithm fee table,
//! - documented local API flags and default port,
//! - an optional algorithm probe argument (`--list-algorithms` / `--help`).

use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::config::{Config, Preset};
use crate::i18n::is_ru;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum MinerDriver {
    /// WildRig Multi: the generic argument shape plus a documented per-GPU
    /// HTTP API.
    Wildrig,
    /// Custom/imported builds: `--algo/--url/--user/...`.
    #[default]
    Generic,
    /// SRBMiner-MULTI: `--algorithm/--pool/--wallet wallet.worker/...`.
    Srbminer,
    /// OneZeroMiner: `-a/-o/-u/-w/...`.
    Onezero,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptionKind {
    Int,
    Bool,
    Enum,
    String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum OptionScope {
    Global,
    Gpu,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MinerOption {
    pub key: String,
    pub label_en: String,
    pub label_ru: String,
    #[serde(default)]
    pub help_en: String,
    #[serde(default)]
    pub help_ru: String,
    pub kind: OptionKind,
    #[serde(default)]
    pub default: Option<String>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub flag: String,
    #[serde(default = "default_scope")]
    pub scope: OptionScope,
    #[serde(default)]
    pub advanced: bool,
}

fn default_scope() -> OptionScope {
    OptionScope::Global
}

impl MinerOption {
    pub fn label(&self) -> &str {
        if is_ru() {
            &self.label_ru
        } else {
            &self.label_en
        }
    }

    pub fn help(&self) -> &str {
        if is_ru() {
            &self.help_ru
        } else {
            &self.help_en
        }
    }

    pub fn default_value(&self) -> Option<serde_json::Value> {
        let raw = self.default.as_deref()?.trim();
        if raw.is_empty() {
            return None;
        }
        Some(match self.kind {
            OptionKind::Int => serde_json::json!(raw.parse::<i64>().ok()?),
            OptionKind::Bool => serde_json::json!(raw.eq_ignore_ascii_case("true")),
            OptionKind::Enum | OptionKind::String => serde_json::json!(raw),
        })
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct MinerVersion {
    pub version: String,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub archive: String,
    #[serde(default)]
    pub binary: String,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub released_at: String,
    #[serde(default)]
    pub notes: String,
}

/// Developer fee for one algorithm. A miner can charge different fees for
/// PearlHash and KawPow, so one global `fee_pct` would be a lie.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct MinerFee {
    pub algo: String,
    pub pct: f64,
    #[serde(default)]
    pub configurable: bool,
    #[serde(default)]
    pub flag: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct MinerDef {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub short_description_en: String,
    #[serde(default)]
    pub short_description_ru: String,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub license: String,
    #[serde(default)]
    pub source_url: String,
    /// Optional explicit coin whitelist. Empty means "algorithm-compatible":
    /// every catalogue coin whose algorithm the miner supports is shown.
    #[serde(default)]
    pub supported_coins: Vec<String>,
    #[serde(default)]
    pub supported_algos: Vec<String>,
    /// Fallback developer fee when `fees` has no entry for the algorithm.
    #[serde(default)]
    pub fee_pct: f64,
    #[serde(default)]
    pub fee_configurable: bool,
    #[serde(default)]
    pub fee_flag: String,
    /// Per-algorithm fee table; overrides the fallback above.
    #[serde(default)]
    pub fees: Vec<MinerFee>,
    #[serde(default)]
    pub per_gpu_stats: bool,
    #[serde(default)]
    pub per_gpu_devices: bool,
    /// Documented flag that selects the devices a miner may use (for example
    /// `--gpu-id`). Empty when the miner cannot choose devices.
    #[serde(default)]
    pub device_flag: String,
    #[serde(default)]
    pub driver: MinerDriver,
    /// Default local API port; `0` means the miner has no local API.
    #[serde(default)]
    pub api_default_port: u16,
    #[serde(default)]
    pub api_port_flag: String,
    #[serde(default)]
    pub api_enable_flag: String,
    #[serde(default)]
    pub api_worker_flag: String,
    /// Optional argument that prints supported algorithms, e.g.
    /// `--list-algorithms` or `--help`.
    #[serde(default)]
    pub algorithm_probe_arg: String,
    #[serde(default)]
    pub default_options: BTreeMap<String, String>,
    #[serde(default)]
    pub versions: Vec<MinerVersion>,
    #[serde(default)]
    pub options: Vec<MinerOption>,
}

impl MinerDef {
    pub fn short_description(&self) -> &str {
        if is_ru() {
            &self.short_description_ru
        } else {
            &self.short_description_en
        }
    }

    pub fn version(&self, version: &str) -> Option<&MinerVersion> {
        self.versions.iter().find(|v| v.version == version)
    }

    pub fn latest_version(&self) -> Option<&MinerVersion> {
        self.versions
            .iter()
            .find(|v| v.channel == "stable")
            .or_else(|| self.versions.first())
    }

    pub fn option(&self, key: &str) -> Option<&MinerOption> {
        self.options.iter().find(|o| o.key == key)
    }

    /// True when the catalogue documents a flag that selects devices.
    ///
    /// Both halves matter: a miner may report per-GPU statistics and still
    /// have no way to choose which cards it mines on. `per_gpu_devices` alone
    /// is not proof, so the flag must be present as well.
    pub fn selects_devices(&self) -> bool {
        self.per_gpu_devices && !self.device_flag.trim().is_empty()
    }

    /// Fee for one algorithm, falling back to the old single-value fields.
    pub fn fee_for(&self, algo: &str) -> MinerFee {
        self.fees
            .iter()
            .find(|fee| fee.algo.eq_ignore_ascii_case(algo))
            .cloned()
            .unwrap_or_else(|| MinerFee {
                algo: algo.to_string(),
                pct: self.fee_pct,
                configurable: self.fee_configurable,
                flag: self.fee_flag.clone(),
            })
    }

    /// True when the miner advertises the algorithm. An empty catalogue list
    /// means "unknown", not "unsupported".
    pub fn supports_algo(&self, algo: &str) -> bool {
        self.supported_algos.is_empty()
            || self
                .supported_algos
                .iter()
                .any(|item| item.eq_ignore_ascii_case(algo))
    }

    /// True when this coin is selectable for the miner. A non-empty
    /// `supported_coins` whitelist wins; otherwise the algorithm decides.
    pub fn supports_coin(&self, coin: &CoinDef) -> bool {
        if !self.supported_coins.is_empty() {
            return self
                .supported_coins
                .iter()
                .any(|id| id.eq_ignore_ascii_case(&coin.id));
        }
        coin.algos.iter().any(|algo| self.supports_algo(algo))
    }

    pub fn default_options(&self) -> BTreeMap<String, serde_json::Value> {
        let mut out = BTreeMap::new();
        for key in self.default_options.keys() {
            if let Some(option) = self.option(key) {
                if let Some(value) = option.default_value() {
                    out.insert(key.clone(), value);
                }
            }
        }
        for option in &self.options {
            if let Some(value) = option.default_value() {
                out.entry(option.key.clone()).or_insert(value);
            }
        }
        out
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct CoinDef {
    pub id: String,
    pub symbol: String,
    pub name: String,
    #[serde(default)]
    pub wallet_prefix: String,
    #[serde(default)]
    pub algos: Vec<String>,
    /// Preferred display unit for hashrates (`auto` or H/s … PH/s).
    #[serde(default = "default_hashrate_unit")]
    pub hashrate_unit: String,
    /// Catalogue data was checked against the upstream/miner documentation.
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub description_en: String,
    #[serde(default)]
    pub description_ru: String,
}

fn default_hashrate_unit() -> String {
    "auto".into()
}

impl CoinDef {
    pub fn description(&self) -> &str {
        if is_ru() {
            &self.description_ru
        } else {
            &self.description_en
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct PoolDef {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub fee_pct: f64,
    #[serde(default)]
    pub api_kind: String,
    /// Documented account JSON endpoint template with `{wallet}`.
    #[serde(default)]
    pub api_url: String,
    #[serde(default)]
    pub min_payout: f64,
    #[serde(default)]
    pub verified: bool,
    #[serde(default)]
    pub note_en: String,
    #[serde(default)]
    pub note_ru: String,
}

impl PoolDef {
    pub fn note(&self) -> &str {
        if is_ru() {
            &self.note_ru
        } else {
            &self.note_en
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
struct MinerFile {
    miners: Vec<MinerDef>,
}

#[derive(Debug, Clone, Deserialize)]
struct CoinFile {
    coins: Vec<CoinDef>,
}

#[derive(Debug, Clone, Deserialize)]
struct PoolFile {
    pools: Vec<PoolDef>,
}

#[derive(Debug, Clone)]
pub struct Catalog {
    pub miners: Vec<MinerDef>,
    pub coins: Vec<CoinDef>,
    pub pools: Vec<PoolDef>,
}

fn load() -> Catalog {
    let miners: MinerFile = toml::from_str(include_str!("../../../resources/catalog/miners.toml"))
        .expect("built-in miners.toml is invalid");
    let coins: CoinFile = toml::from_str(include_str!("../../../resources/catalog/coins.toml"))
        .expect("built-in coins.toml is invalid");
    let pools: PoolFile = toml::from_str(include_str!("../../../resources/catalog/pools.toml"))
        .expect("built-in pools.toml is invalid");
    Catalog {
        miners: miners.miners,
        coins: coins.coins,
        pools: pools.pools,
    }
}

pub fn catalog() -> &'static Catalog {
    static CATALOG: OnceLock<Catalog> = OnceLock::new();
    CATALOG.get_or_init(load)
}

pub fn miners() -> &'static [MinerDef] {
    &catalog().miners
}

pub fn miner(id: &str) -> Option<&'static MinerDef> {
    catalog().miners.iter().find(|m| m.id == id)
}

pub fn coins() -> &'static [CoinDef] {
    &catalog().coins
}

pub fn coin(id: &str) -> Option<&'static CoinDef> {
    catalog().coins.iter().find(|c| c.id == id)
}

pub fn pools() -> &'static [PoolDef] {
    &catalog().pools
}

pub fn pool(id: &str) -> Option<&'static PoolDef> {
    catalog().pools.iter().find(|p| p.id == id)
}

fn pool_host(url: &str) -> &str {
    let trimmed = url.trim();
    let after_scheme = trimmed
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(trimmed);
    after_scheme
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(after_scheme)
}

/// Known pool by endpoint host. Used for a hint and for the built-in API
/// adapter; it never replaces the user's address and there is no pool dropdown.
pub fn pool_for_url(url: &str) -> Option<&'static PoolDef> {
    let host = pool_host(url);
    catalog()
        .pools
        .iter()
        .find(|pool| pool_host(&pool.url).eq_ignore_ascii_case(host))
}

/// Coins selectable for a miner, according to its whitelist or algorithms.
pub fn coins_for_miner(miner_id: &str) -> Vec<&'static CoinDef> {
    let Some(def) = miner(miner_id) else {
        return coins().iter().collect();
    };
    coins()
        .iter()
        .filter(|coin| def.supports_coin(coin))
        .collect()
}

/// Algorithms a miner advertises, from the catalogue.
pub fn algorithms_for_miner(miner_id: &str) -> Vec<String> {
    miner(miner_id)
        .map(|def| def.supported_algos.clone())
        .unwrap_or_default()
}

/// Fee of the selected miner for an algorithm, from the catalogue.
pub fn miner_fee_pct(miner_id: &str, algo: &str) -> f64 {
    miner(miner_id)
        .map(|miner| miner.fee_for(algo).pct)
        .unwrap_or(0.0)
}

pub fn pool_fee_pct(pool_id: &str) -> f64 {
    pool(pool_id).map(|p| p.fee_pct).unwrap_or(0.0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallStatus {
    Installed {
        version: String,
        verified: bool,
        custom: bool,
    },
    OtherVersion {
        installed: Vec<String>,
    },
    NotInstalled,
}

pub fn installed_versions(cfg: &Config, miner_id: &str) -> Vec<String> {
    let mut versions: Vec<String> = cfg
        .miner_installs
        .values()
        .filter(|i| i.miner == miner_id && std::path::Path::new(&i.path).exists())
        .map(|i| i.version.clone())
        .collect();
    versions.sort();
    versions.dedup();
    versions
}

pub fn install_status(cfg: &Config, preset: &Preset) -> InstallStatus {
    if let Some(install) = cfg.install(&preset.miner, &preset.miner_version) {
        InstallStatus::Installed {
            version: install.version.clone(),
            verified: install.verified,
            custom: install.custom,
        }
    } else {
        let installed = installed_versions(cfg, &preset.miner);
        if installed.is_empty() {
            InstallStatus::NotInstalled
        } else {
            InstallStatus::OtherVersion { installed }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config_with_preset() -> Config {
        let mut cfg = Config::default();
        let preset = Preset::new_pearl();
        cfg.presets.insert(preset.id.clone(), preset);
        cfg.active_preset = Some(crate::config::DEFAULT_PRESET_ID.to_string());
        cfg
    }

    #[test]
    fn catalogue_has_pearl_miners() {
        assert!(miner("wildrig").is_some());
        assert!(miner("srbminer").is_some());
        assert!(miner("onezero").is_some());
        assert!(pool("pearlhash").is_some());
        assert!(coin("pearl").is_some());
    }

    #[test]
    fn wildrig_fee_is_zero_and_fixed() {
        let m = miner("wildrig").unwrap();
        let fee = m.fee_for("pearlhash");
        assert_eq!(fee.pct, 0.0);
        assert!(!fee.configurable);
        assert!(m.option("fan_speed").is_some());
    }

    #[test]
    fn per_algorithm_fees_are_used() {
        let m = miner("wildrig").unwrap();
        assert_ne!(m.fee_for("pearlhash").pct, m.fee_for("kawpow").pct);
        assert_eq!(m.fee_for("unknown-algo").pct, m.fee_pct);
    }

    #[test]
    fn drivers_and_api_are_catalogue_data() {
        let wildrig = miner("wildrig").unwrap();
        assert_eq!(wildrig.driver, MinerDriver::Wildrig);
        assert_eq!(wildrig.api_default_port, 4028);
        assert_eq!(wildrig.api_port_flag, "--api-port");
        let srb = miner("srbminer").unwrap();
        assert_eq!(srb.driver, MinerDriver::Srbminer);
        assert_eq!(srb.api_default_port, 21550);
        assert_eq!(srb.api_enable_flag, "--api-enable");
        assert_eq!(srb.algorithm_probe_arg, "--list-algorithms");
        let onezero = miner("onezero").unwrap();
        assert_eq!(onezero.driver, MinerDriver::Onezero);
        assert_eq!(onezero.api_default_port, 3001);
    }

    #[test]
    fn default_options_are_typed() {
        let m = miner("wildrig").unwrap();
        let defaults = m.default_options();
        assert_eq!(defaults.get("fan_speed"), Some(&serde_json::json!(0)));
    }

    #[test]
    fn install_status_tracks_exact_version() {
        let cfg = config_with_preset();
        let preset = cfg.preset().unwrap().clone();
        assert_eq!(install_status(&cfg, &preset), InstallStatus::NotInstalled);
    }

    #[test]
    fn documented_device_flags_match_upstream() {
        let cases = [
            ("wildrig", "--gpu-list"),
            ("srbminer", "--gpu-id"),
            ("onezero", "--devices"),
        ];
        for (id, flag) in cases {
            let def = miner(id).expect("catalogue entry");
            assert!(def.per_gpu_devices, "{id} should support device selection");
            assert_eq!(def.device_flag, flag, "{id}");
            assert!(def.selects_devices(), "{id}");
        }
    }

    #[test]
    fn a_miner_without_a_documented_flag_does_not_advertise_selection() {
        let mut def = miner("wildrig").unwrap().clone();
        def.device_flag.clear();
        assert!(!def.selects_devices());
    }

    #[test]
    fn coin_filter_uses_whitelist_or_algorithm() {
        let onezero = miner("onezero").unwrap();
        let pearl = coin("pearl").unwrap();
        assert!(onezero.supports_coin(pearl));
        let wildrig = miner("wildrig").unwrap();
        assert!(wildrig.supports_coin(pearl));
        assert!(coins_for_miner("onezero").iter().any(|c| c.id == "pearl"));
    }

    #[test]
    fn known_pool_matches_by_host() {
        assert_eq!(
            pool_for_url("pool.pearlhash.xyz:9000").map(|pool| pool.id.as_str()),
            Some("pearlhash")
        );
        assert!(pool_for_url("example.org:3333").is_none());
    }
}
