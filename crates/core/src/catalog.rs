//! Built-in coin, pool and miner catalogue.
//!
//! The catalogue is data, not code: adding a miner version or a new option
//! should not require touching the UI. Binaries are never bundled; each
//! version points at an upstream release and may carry a SHA-256 checksum.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::sync::OnceLock;

use crate::config::{Config, Preset};
use crate::i18n::is_ru;

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
    #[serde(default)]
    pub supported_coins: Vec<String>,
    #[serde(default)]
    pub supported_algos: Vec<String>,
    #[serde(default)]
    pub fee_pct: f64,
    #[serde(default)]
    pub fee_configurable: bool,
    #[serde(default)]
    pub fee_flag: String,
    #[serde(default)]
    pub per_gpu_stats: bool,
    #[serde(default)]
    pub per_gpu_devices: bool,
    /// Documented flag that selects the devices a miner may use (for example
    /// `--gpu-id`). Empty when the miner cannot choose devices.
    #[serde(default)]
    pub device_flag: String,
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

#[derive(Debug, Clone, Deserialize)]
pub struct CoinDef {
    pub id: String,
    pub symbol: String,
    pub name: String,
    pub wallet_prefix: String,
    #[serde(default)]
    pub algos: Vec<String>,
    #[serde(default)]
    pub description_en: String,
    #[serde(default)]
    pub description_ru: String,
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

#[derive(Debug, Clone, Deserialize)]
pub struct PoolDef {
    pub id: String,
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub fee_pct: f64,
    #[serde(default)]
    pub api_kind: String,
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

/// Fee of the selected miner for its algorithm, from the catalogue.
pub fn miner_fee_pct(miner_id: &str) -> f64 {
    miner(miner_id).map(|m| m.fee_pct).unwrap_or(0.0)
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
        assert_eq!(m.fee_pct, 0.0);
        assert!(!m.fee_configurable);
        assert!(m.option("fan_speed").is_some());
    }

    #[test]
    fn default_options_are_typed() {
        let m = miner("wildrig").unwrap();
        let defaults = m.default_options();
        assert_eq!(defaults.get("fan_speed"), Some(&serde_json::json!(0)));
    }

    #[test]
    fn install_status_tracks_exact_version() {
        let cfg = Config::default();
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
}
