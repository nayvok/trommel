//! Configuration v4: presets with their own GPU settings, named GPU presets,
//! miner installs and behavior.
//!
//! The on-disk file is `$XDG_CONFIG_HOME/trommel/config.json`. Version 1 (the
//! old flat config) and versions 2/3 (shared GPU profiles) are migrated once.
//!
//! Ownership rules introduced in v4:
//! - GPU power/clocks/participation belong to a mining preset (`Preset::gpu`).
//! - `GpuPreset` is a named, reusable per-card tuning template.
//! - A fresh config has no presets at all; the first run creates one.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::i18n::{tr, Language};

pub const SCHEMA_VERSION: u32 = 4;
pub const DEFAULT_PRESET_ID: &str = "pearl-wildrig";
pub const DEFAULT_POOL_URL: &str = "pool.pearlhash.xyz:9000";

/// Build the stable key used in `miner_installs`.
pub fn install_key(miner: &str, version: &str) -> String {
    format!("{miner}@{version}")
}

/// GPU tuning for one card, or the fallback for cards without an entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GpuTuning {
    pub enabled: bool,
    pub power_limit_w: Option<u32>,
    pub core_clock_mhz: Option<u32>,
    pub memory_clock_mhz: Option<u32>,
    /// Only meaningful inside `GpuProfile::per_gpu`.
    ///
    /// `false` means "inherit power/clocks from the profile default"; `enabled`
    /// is still per card. The tuning dialog writes `true` for an explicit
    /// per-card value (including an explicit stock reset).
    #[serde(skip_serializing_if = "is_false")]
    pub custom: bool,
    /// Id of the named `GpuPreset` this card follows, if any. Metadata only:
    /// it never changes what is applied and is ignored by `Config::diff`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gpu_preset: String,
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl Default for GpuTuning {
    fn default() -> Self {
        Self {
            enabled: true,
            power_limit_w: None,
            core_clock_mhz: None,
            memory_clock_mhz: None,
            custom: false,
            gpu_preset: String::new(),
        }
    }
}

impl GpuTuning {
    /// True when this entry asks the helper to change something.
    pub fn has_tuning(&self) -> bool {
        self.power_limit_w.is_some()
            || self.core_clock_mhz.is_some()
            || self.memory_clock_mhz.is_some()
    }
}

/// Per-preset GPU settings. The `default` is a fallback for cards without a
/// concrete per-UUID entry; all cards created through the UI get an entry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct GpuProfile {
    pub default: GpuTuning,
    /// Per-GPU overrides keyed by NVIDIA UUID (stable across reordering).
    pub per_gpu: BTreeMap<String, GpuTuning>,
}

impl GpuProfile {
    /// Effective tuning for one card.
    ///
    /// A per-card entry with `custom = false` only remembers the enable switch;
    /// power and clocks come from the profile default. Only an explicit custom
    /// override (`custom = true`, written by the tuning dialog) replaces the
    /// default values.
    pub fn tuning_for(&self, uuid: &str) -> GpuTuning {
        match self.per_gpu.get(uuid) {
            Some(tuning) if tuning.custom => {
                let mut effective = tuning.clone();
                effective.custom = false;
                effective
            }
            Some(tuning) => {
                let mut effective = self.default.clone();
                effective.enabled = tuning.enabled;
                effective.custom = false;
                effective.gpu_preset = tuning.gpu_preset.clone();
                effective
            }
            None => {
                let mut effective = self.default.clone();
                effective.custom = false;
                effective
            }
        }
    }

    /// True when the card has an explicit override instead of inheriting the
    /// default profile.
    pub fn is_custom(&self, uuid: &str) -> bool {
        self.per_gpu.get(uuid).map(|t| t.custom).unwrap_or(false)
    }

    /// True when any card or the fallback asks for power/clocks.
    pub fn has_tuning(&self) -> bool {
        self.default.has_tuning() || self.per_gpu.values().any(GpuTuning::has_tuning)
    }
}

/// A saved, reusable tuning for one card. It is bound to the card UUID and
/// model it was captured from, but can be applied to another card by copying
/// its values.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct GpuPreset {
    pub id: String,
    pub name: String,
    pub gpu_uuid: String,
    pub gpu_name: String,
    pub tuning: GpuTuning,
    pub created_at: u64,
    pub updated_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Preset {
    pub id: String,
    pub name: String,
    /// Catalogue coin id, or a free-form id for a custom coin.
    pub coin: String,
    /// Effective display data; filled from the catalogue when the coin is known.
    pub coin_symbol: String,
    pub coin_name: String,
    pub wallet_prefix: String,
    /// Preferred display unit (`auto`, `H/s`, `MH/s`, `TH/s`, ...).
    pub hashrate_unit: String,
    pub algo: String,
    pub miner: String,
    pub miner_version: String,
    /// Pool endpoint is the only source of truth; no catalogue id is stored.
    pub pool_url: String,
    pub pool_fee_pct: f64,
    /// Optional documented JSON account endpoint with a `{wallet}` placeholder.
    pub pool_api_url: String,
    pub wallet: String,
    pub worker: String,
    /// GPU participation and tuning are owned by this preset.
    pub gpu: GpuProfile,
    /// 0 means "use the miner catalogue default".
    pub api_port: u16,
    pub expected_hashrate: Option<f64>,
    /// Values of miner-specific options (see the catalog descriptors).
    pub miner_options: BTreeMap<String, serde_json::Value>,
    /// Only used when the selected miner allows a fee override.
    pub miner_fee_override_pct: Option<f64>,
    /// Cached catalog fee for this preset (miner/algorithm).
    pub miner_fee_pct: f64,
    pub extra_args: String,
    pub env: BTreeMap<String, String>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl Preset {
    /// A data-driven template for the first preset: first catalogue coin and
    /// miner, latest catalogue version, stock GPU profile, empty wallet/pool.
    pub fn new_draft() -> Self {
        let coin = crate::catalog::coins().first();
        let miner = crate::catalog::miners().first();
        let version = miner
            .and_then(|miner| miner.latest_version())
            .map(|version| version.version.clone())
            .unwrap_or_default();
        let mut preset = Self {
            id: String::new(),
            name: tr("New preset", "Новый пресет"),
            coin: coin.map(|coin| coin.id.clone()).unwrap_or_default(),
            coin_symbol: coin.map(|coin| coin.symbol.clone()).unwrap_or_default(),
            coin_name: coin.map(|coin| coin.name.clone()).unwrap_or_default(),
            wallet_prefix: coin
                .map(|coin| coin.wallet_prefix.clone())
                .unwrap_or_default(),
            hashrate_unit: coin
                .map(|coin| coin.hashrate_unit.clone())
                .unwrap_or_else(|| "auto".into()),
            algo: coin
                .and_then(|coin| coin.algos.first().cloned())
                .unwrap_or_default(),
            miner: miner.map(|miner| miner.id.clone()).unwrap_or_default(),
            miner_version: version,
            pool_url: String::new(),
            pool_fee_pct: 0.0,
            pool_api_url: String::new(),
            wallet: String::new(),
            worker: String::new(),
            gpu: GpuProfile::default(),
            api_port: 0,
            expected_hashrate: None,
            miner_options: BTreeMap::new(),
            miner_fee_override_pct: None,
            miner_fee_pct: 0.0,
            extra_args: String::new(),
            env: BTreeMap::new(),
            created_at: 0,
            updated_at: 0,
        };
        if let Some(def) = miner {
            preset.miner_options = def.default_options();
            preset.miner_fee_pct = def.fee_for(&preset.algo).pct;
        }
        preset
    }

    /// Historical Pearl/WildRig template used by migrations and tests.
    pub fn new_pearl() -> Self {
        let mut preset = Self::new_draft();
        preset.id = DEFAULT_PRESET_ID.to_string();
        preset.name = tr("Pearl · WildRig", "Pearl · WildRig");
        preset.coin = "pearl".into();
        preset.coin_symbol = "PRL".into();
        preset.coin_name = "Pearl".into();
        preset.wallet_prefix = "prl1".into();
        preset.hashrate_unit = "TH/s".into();
        preset.algo = "pearlhash".into();
        preset.miner = "wildrig".into();
        preset.miner_version = crate::catalog::miner("wildrig")
            .and_then(|miner| miner.latest_version())
            .map(|version| version.version.clone())
            .unwrap_or_default();
        preset.pool_url = DEFAULT_POOL_URL.into();
        preset.pool_fee_pct = 1.0;
        preset.miner_fee_pct = crate::catalog::miner("wildrig")
            .map(|miner| miner.fee_for("pearlhash").pct)
            .unwrap_or(0.0);
        preset
    }

    pub fn effective_miner_fee_pct(&self) -> f64 {
        self.miner_fee_override_pct.unwrap_or(self.miner_fee_pct)
    }

    pub fn total_fee_pct(&self) -> f64 {
        self.effective_miner_fee_pct() + self.pool_fee_pct
    }

    /// Display symbol, falling back to the catalogue and finally the coin id.
    pub fn symbol(&self) -> &str {
        if !self.coin_symbol.trim().is_empty() {
            return self.coin_symbol.trim();
        }
        crate::catalog::coin(&self.coin)
            .map(|coin| coin.symbol.as_str())
            .unwrap_or_else(|| self.coin.as_str())
    }

    /// Display unit for hashrates.
    pub fn hashrate_unit(&self) -> &str {
        let unit = self.hashrate_unit.trim();
        if !unit.is_empty() {
            return unit;
        }
        crate::catalog::coin(&self.coin)
            .map(|coin| coin.hashrate_unit.as_str())
            .unwrap_or("auto")
    }

    /// Expected wallet prefix, or an empty string when the coin does not
    /// declare one (custom coins may opt out of validation).
    pub fn wallet_prefix(&self) -> &str {
        if !self.wallet_prefix.trim().is_empty() {
            return self.wallet_prefix.trim();
        }
        crate::catalog::coin(&self.coin)
            .map(|coin| coin.wallet_prefix.as_str())
            .unwrap_or("")
    }

    /// API port actually used: the preset override or the miner catalogue
    /// default. `0` disables the API for an unknown/custom miner.
    pub fn effective_api_port(&self) -> u16 {
        if self.api_port > 0 {
            self.api_port
        } else {
            crate::catalog::miner(&self.miner)
                .map(|miner| miner.api_default_port)
                .unwrap_or(0)
        }
    }

    /// Human-readable pool name: a known catalogue pool by host, else the host.
    pub fn pool_display_name(&self) -> String {
        crate::catalog::pool_for_url(&self.pool_url)
            .map(|pool| pool.name.clone())
            .unwrap_or_else(|| pool_host(&self.pool_url))
    }

    /// True when the preset references a catalogue miner that does not list the
    /// selected algorithm. Advisory only: a newer build may support more.
    pub fn algo_supported(&self) -> bool {
        match crate::catalog::miner(&self.miner) {
            Some(miner) => miner.supports_algo(&self.algo),
            None => true,
        }
    }

    /// Runtime-relevant equality: timestamps and GPU-preset bookkeeping do not
    /// require a miner restart.
    pub fn runtime_eq(&self, other: &Preset) -> bool {
        fn normalized(preset: &Preset) -> Preset {
            let mut copy = preset.clone();
            copy.created_at = 0;
            copy.updated_at = 0;
            for tuning in copy.gpu.per_gpu.values_mut() {
                tuning.gpu_preset.clear();
            }
            copy
        }
        normalized(self) == normalized(other)
    }

    /// Fill display fields from the catalogue, refresh the algorithm fee and
    /// discard fields that are no longer valid for the selected miner.
    pub fn normalize(&mut self) {
        if self.hashrate_unit.trim().is_empty() {
            self.hashrate_unit = crate::catalog::coin(&self.coin)
                .map(|coin| coin.hashrate_unit.clone())
                .unwrap_or_else(|| "auto".into());
        }
        if self.coin_symbol.trim().is_empty() {
            self.coin_symbol = crate::catalog::coin(&self.coin)
                .map(|coin| coin.symbol.clone())
                .unwrap_or_else(|| self.coin.to_uppercase());
        }
        if self.coin_name.trim().is_empty() {
            self.coin_name = crate::catalog::coin(&self.coin)
                .map(|coin| coin.name.clone())
                .unwrap_or_else(|| self.coin.clone());
        }
        if self.wallet_prefix.trim().is_empty() {
            self.wallet_prefix = crate::catalog::coin(&self.coin)
                .map(|coin| coin.wallet_prefix.clone())
                .unwrap_or_default();
        }
        if !matches!(
            self.hashrate_unit.as_str(),
            "auto" | "H/s" | "kH/s" | "MH/s" | "GH/s" | "TH/s" | "PH/s"
        ) {
            self.hashrate_unit = "auto".into();
        }
        self.pool_fee_pct = self.pool_fee_pct.clamp(0.0, 100.0);
        if let Some(def) = crate::catalog::miner(&self.miner) {
            let fee = def.fee_for(&self.algo);
            self.miner_fee_pct = fee.pct;
            if !fee.configurable {
                self.miner_fee_override_pct = None;
            }
        }
    }
}

fn pool_host(url: &str) -> String {
    let trimmed = url.trim();
    trimmed
        .rsplit_once(':')
        .map(|(host, _)| host)
        .unwrap_or(trimmed)
        .to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct MinerInstall {
    pub miner: String,
    pub version: String,
    pub path: String,
    pub sha256: String,
    pub source_url: String,
    pub verified: bool,
    pub custom: bool,
    pub installed_at: u64,
}

/// Colour scheme preference. `System` follows the desktop setting.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Appearance {
    #[default]
    System,
    Light,
    Dark,
}

/// What the hashrate watchdog does after a sustained low reading.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum WatchdogAction {
    /// Safe default: notify, never touch the running miner.
    #[default]
    Notify,
    /// Explicit opt-in: stop and start the miner (same supervisor path).
    Restart,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Behavior {
    pub auto_restart: bool,
    pub restart_delay_s: u64,
    pub poll_interval_s: u64,
    pub restore_stock_on_stop: bool,
    pub start_mining_on_login: bool,
    pub notifications: bool,
    /// Use the monochrome symbolic icon in the tray; off selects the colour one.
    pub tray_monochrome: bool,
    pub language: Language,
    /// Colour scheme of the app window.
    pub appearance: Appearance,
    /// Hide advanced miner options and extra arguments in the UI.
    pub simple_mode: bool,
    /// Global temperature policy passed to miners that support the flag.
    pub temp_limit_c: u32,
    pub temp_resume_c: u32,
    /// Hashrate watchdog. Off by default; notify-only when enabled.
    pub watchdog_enabled: bool,
    /// Percentage of the preset's expected hashrate considered healthy.
    pub watchdog_threshold_pct: u32,
    /// How long the hashrate must stay low before an action.
    pub watchdog_window_s: u64,
    pub watchdog_action: WatchdogAction,
    /// Consecutive restarts before stopping and notifying.
    pub watchdog_max_restarts: u32,
}

impl Default for Behavior {
    fn default() -> Self {
        Self {
            auto_restart: true,
            restart_delay_s: 5,
            poll_interval_s: 2,
            restore_stock_on_stop: true,
            start_mining_on_login: false,
            notifications: true,
            tray_monochrome: true,
            language: Language::System,
            appearance: Appearance::System,
            simple_mode: true,
            temp_limit_c: 85,
            temp_resume_c: 65,
            watchdog_enabled: false,
            watchdog_threshold_pct: 80,
            watchdog_window_s: 300,
            watchdog_action: WatchdogAction::Notify,
            watchdog_max_restarts: 3,
        }
    }
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub active_preset: Option<String>,
    pub presets: BTreeMap<String, Preset>,
    /// Named per-card tuning presets. Independent of mining presets.
    pub gpu_presets: BTreeMap<String, GpuPreset>,
    pub miner_installs: BTreeMap<String, MinerInstall>,
    pub behavior: Behavior,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            // A fresh install has no preset: the dashboard empty state creates
            // the first one explicitly. There is no hidden default wallet,
            // pool or miner version.
            active_preset: None,
            presets: BTreeMap::new(),
            gpu_presets: BTreeMap::new(),
            miner_installs: BTreeMap::new(),
            behavior: Behavior::default(),
        }
    }
}

impl Config {
    pub fn load() -> anyhow::Result<Self> {
        let path = crate::paths::config_path();
        if !path.exists() {
            let legacy = crate::paths::legacy_config_path();
            if legacy.exists() {
                let text = std::fs::read_to_string(&legacy)?;
                let old: ConfigV1 = serde_json::from_str(&text)
                    .map_err(|e| anyhow::anyhow!("cannot parse {}: {e}", legacy.display()))?;
                let cfg = Self::from_v1(old);
                let _ = crate::paths::ensure_dirs();
                let _ = cfg.save();
                return Ok(cfg);
            }
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)?;
        let value: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("cannot parse {}: {e}", path.display()))?;
        let version = value
            .get("schema_version")
            .and_then(|v| v.as_u64())
            .unwrap_or(1);
        if version >= 4 {
            let cfg: Config = serde_json::from_value(value)
                .map_err(|e| anyhow::anyhow!("cannot parse {}: {e}", path.display()))?;
            Ok(cfg.normalize())
        } else if version >= 2 {
            let old: LegacyConfigV3 = serde_json::from_value(value)
                .map_err(|e| anyhow::anyhow!("cannot parse legacy {}: {e}", path.display()))?;
            let cfg = Self::from_v3(old).normalize();
            let _ = crate::paths::ensure_dirs();
            let _ = cfg.save();
            Ok(cfg)
        } else {
            let old: ConfigV1 = serde_json::from_value(value)
                .map_err(|e| anyhow::anyhow!("cannot parse legacy {}: {e}", path.display()))?;
            let cfg = Self::from_v1(old);
            let _ = crate::paths::ensure_dirs();
            let _ = cfg.save();
            Ok(cfg)
        }
    }

    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_default()
    }

    pub fn save(&self) -> anyhow::Result<()> {
        let path = crate::paths::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        write_atomic(&path, text.as_bytes())
    }

    /// Make sure indexes and defaults are coherent.
    ///
    /// A fresh install is allowed to have no presets; normalize never invents
    /// a wallet, pool or miner version.
    pub fn normalize(mut self) -> Self {
        if self.schema_version < SCHEMA_VERSION {
            self.schema_version = SCHEMA_VERSION;
        }
        let active_ok = self
            .active_preset
            .as_ref()
            .map(|id| self.presets.contains_key(id))
            .unwrap_or(false);
        if self.presets.is_empty() {
            self.active_preset = None;
        } else if !active_ok {
            self.active_preset = self.presets.keys().next().cloned();
        }
        for preset in self.presets.values_mut() {
            preset.normalize();
        }
        self.behavior.watchdog_threshold_pct = self.behavior.watchdog_threshold_pct.clamp(10, 100);
        self.behavior.watchdog_window_s = self.behavior.watchdog_window_s.clamp(60, 3600);
        self.behavior.watchdog_max_restarts = self.behavior.watchdog_max_restarts.clamp(1, 10);
        self
    }

    pub fn preset(&self) -> Option<&Preset> {
        self.active_preset
            .as_ref()
            .and_then(|id| self.presets.get(id))
    }

    pub fn preset_mut(&mut self) -> Option<&mut Preset> {
        self.active_preset
            .as_ref()
            .and_then(|id| self.presets.get_mut(id))
    }

    pub fn gpu_profile(&self) -> Option<&GpuProfile> {
        self.preset().map(|preset| &preset.gpu)
    }

    pub fn gpu_profile_mut(&mut self) -> Option<&mut GpuProfile> {
        self.preset_mut().map(|preset| &mut preset.gpu)
    }

    pub fn gpu_preset(&self, id: &str) -> Option<&GpuPreset> {
        self.gpu_presets.get(id)
    }

    pub fn install(&self, miner: &str, version: &str) -> Option<&MinerInstall> {
        self.miner_installs
            .get(&install_key(miner, version))
            .filter(|install| Path::new(&install.path).exists())
    }

    /// True when an install record exists, even if its binary disappeared.
    pub fn install_record_exists(&self, miner: &str, version: &str) -> bool {
        self.miner_installs
            .contains_key(&install_key(miner, version))
    }

    pub fn install_for(&self, preset: &Preset) -> Option<&MinerInstall> {
        self.install(&preset.miner, &preset.miner_version)
    }

    pub fn miner_path(&self, preset: &Preset) -> Option<PathBuf> {
        self.install_for(preset)
            .map(|install| PathBuf::from(&install.path))
    }

    pub fn add_install(&mut self, install: MinerInstall) {
        let key = install_key(&install.miner, &install.version);
        self.miner_installs.insert(key, install);
    }

    pub fn remove_install(&mut self, miner: &str, version: &str) -> bool {
        self.miner_installs
            .remove(&install_key(miner, version))
            .is_some()
    }

    pub fn install_users(&self, miner: &str, version: &str) -> Vec<String> {
        self.presets
            .values()
            .filter(|p| p.miner == miner && p.miner_version == version)
            .map(|p| p.name.clone())
            .collect()
    }

    pub fn total_fee_pct(&self) -> f64 {
        self.preset().map(Preset::total_fee_pct).unwrap_or(0.0)
    }

    /// Copy of the config with every preset wallet replaced by a placeholder.
    pub fn redacted(&self) -> Config {
        let mut copy = self.clone();
        for preset in copy.presets.values_mut() {
            if !preset.wallet.trim().is_empty() {
                preset.wallet = "<redacted>".into();
            }
        }
        copy
    }

    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Some(preset) = self.preset() else {
            out.push(tr("No mining preset selected", "Не выбран пресет майнинга"));
            return out;
        };
        if preset.wallet.trim().is_empty() {
            out.push(tr(
                "Wallet is not set; mining cannot start",
                "Кошелёк не задан — майнинг не запустится",
            ));
        } else {
            let prefix = preset.wallet_prefix();
            if !prefix.is_empty() && !preset.wallet.starts_with(prefix) {
                out.push(tr(
                    &format!(
                        "Wallet does not look like a {} address (expected {prefix}…)",
                        preset.symbol()
                    ),
                    &format!(
                        "Кошелёк не похож на адрес {} (ожидается {prefix}…)",
                        preset.symbol()
                    ),
                ));
            }
        }
        if !preset.pool_url.contains(':') {
            out.push(tr(
                "Pool address has no port (expected host:port)",
                "В адресе пула нет порта (ожидается host:port)",
            ));
        }
        if !preset.miner.trim().is_empty() && self.install_for(preset).is_none() {
            out.push(format!(
                "{} {} {}",
                tr("Miner is not installed:", "Майнер не установлен:"),
                preset.miner,
                preset.miner_version
            ));
        }
        if !preset.miner.trim().is_empty() && !preset.algo_supported() {
            out.push(tr(
                &format!(
                    "{} {} is not listed as supporting {}; mining may still work",
                    preset.miner, preset.miner_version, preset.algo
                ),
                &format!(
                    "{} {} не заявлен как поддерживающий {}; майнинг всё равно может работать",
                    preset.miner, preset.miner_version, preset.algo
                ),
            ));
        }
        let any_enabled =
            preset.gpu.default.enabled || preset.gpu.per_gpu.values().any(|tuning| tuning.enabled);
        if !any_enabled {
            out.push(tr(
                "No GPU is selected for mining",
                "Ни одна видеокарта не выбрана для майнинга",
            ));
        }
        if preset.gpu.has_tuning() && !self.behavior.restore_stock_on_stop {
            out.push(tr(
                "Restore stock on stop is off; Stop will leave power and clock changes applied",
                "Возврат стока при остановке выключен — «Остановить» оставит питание и частоты применёнными",
            ));
        }
        out
    }

    pub fn diff(&self, next: &Config) -> ConfigDiff {
        if self == next {
            return ConfigDiff::none();
        }
        let mut reasons = Vec::new();
        let mut class = ChangeClass::Live;

        if self.active_preset != next.active_preset {
            reasons.push(tr("Active preset changed", "Сменился активный пресет"));
            class = ChangeClass::RestartRequired;
        }
        match (self.preset(), next.preset()) {
            (Some(old), Some(new)) if !old.runtime_eq(new) => {
                reasons.push(tr(
                    "Preset settings changed",
                    "Изменились настройки пресета",
                ));
                class = ChangeClass::RestartRequired;
            }
            (Some(_), None) | (None, Some(_)) => {
                reasons.push(tr(
                    "Preset settings changed",
                    "Изменились настройки пресета",
                ));
                class = ChangeClass::RestartRequired;
            }
            _ => {}
        }
        if self.behavior.temp_limit_c != next.behavior.temp_limit_c
            || self.behavior.temp_resume_c != next.behavior.temp_resume_c
        {
            reasons.push(tr(
                "Temperature policy changed",
                "Изменилась температурная политика",
            ));
            class = ChangeClass::RestartRequired;
        }
        if self.behavior.language != next.behavior.language {
            reasons.push(tr(
                "Interface language changed",
                "Изменился язык интерфейса",
            ));
        }
        if self.behavior.notifications != next.behavior.notifications {
            reasons.push(tr("Notifications changed", "Изменились уведомления"));
        }
        if self.behavior.start_mining_on_login != next.behavior.start_mining_on_login {
            reasons.push(tr(
                "Start-at-login setting changed",
                "Изменилась настройка автозапуска",
            ));
        }
        if self.behavior.tray_monochrome != next.behavior.tray_monochrome {
            reasons.push(tr(
                "Tray icon style changed",
                "Изменился стиль иконки в трее",
            ));
        }
        if self.behavior.watchdog_enabled != next.behavior.watchdog_enabled
            || self.behavior.watchdog_threshold_pct != next.behavior.watchdog_threshold_pct
            || self.behavior.watchdog_window_s != next.behavior.watchdog_window_s
            || self.behavior.watchdog_action != next.behavior.watchdog_action
            || self.behavior.watchdog_max_restarts != next.behavior.watchdog_max_restarts
        {
            reasons.push(tr(
                "Hashrate watchdog settings changed",
                "Изменились настройки сторожа хешрейта",
            ));
        }
        ConfigDiff { class, reasons }
    }

    #[allow(clippy::field_reassign_with_default)]
    pub fn from_v1(old: ConfigV1) -> Self {
        let mut cfg = Config::default();
        cfg.active_preset = Some("pearl-imported".to_string());

        let mut preset = Preset::new_pearl();
        preset.id = "pearl-imported".to_string();
        preset.name = tr("Pearl (imported)", "Pearl (импортирован)");
        preset.wallet = old.wallet;
        preset.worker = old.worker;
        preset.pool_url = old.pool;
        preset.algo = old.algo;
        preset.miner = if old.miner_profile.is_empty() {
            "wildrig".into()
        } else {
            old.miner_profile
        };
        preset.miner_version = crate::catalog::miner(&preset.miner)
            .and_then(|miner| miner.latest_version())
            .map(|version| version.version.clone())
            .unwrap_or_default();
        preset.api_port = old.api_port;
        preset.expected_hashrate = if old.expected_hashrate > 0.0 {
            Some(old.expected_hashrate)
        } else {
            None
        };
        preset.pool_fee_pct = old.pool_fee_pct;
        preset.miner_fee_pct = crate::catalog::miner(&preset.miner)
            .map(|miner| miner.fee_for(&preset.algo).pct)
            .unwrap_or(0.0);
        preset.gpu.default = GpuTuning {
            power_limit_w: old.gpu.power_limit,
            core_clock_mhz: old.gpu.core_clock,
            memory_clock_mhz: old.gpu.memory_clock,
            ..Default::default()
        };
        if let Some(fan) = old.gpu.fan_speed {
            preset
                .miner_options
                .insert("fan_speed".into(), serde_json::json!(fan));
        }
        preset.updated_at = crate::util::now_unix();
        let install_version = preset.miner_version.clone();
        cfg.presets.insert(preset.id.clone(), preset);

        for (id, miner) in old.miners {
            if miner.path.trim().is_empty() {
                continue;
            }
            cfg.add_install(MinerInstall {
                miner: id,
                version: install_version.clone(),
                path: miner.path,
                sha256: String::new(),
                source_url: String::new(),
                verified: false,
                custom: true,
                installed_at: crate::util::now_unix(),
            });
        }

        cfg.behavior.auto_restart = old.auto_restart;
        cfg.behavior.restart_delay_s = old.restart_delay_s;
        cfg.behavior.poll_interval_s = old.poll_interval_s;
        cfg.behavior.notifications = old.notifications;
        cfg.behavior.start_mining_on_login = old.autostart_on_boot;
        cfg.behavior.temp_limit_c = old.gpu.temp_limit;
        cfg.behavior.temp_resume_c = old.gpu.temp_resume;
        cfg.normalize()
    }

    fn from_v3(old: LegacyConfigV3) -> Self {
        let infer_custom = old.schema_version < 3;
        let LegacyConfigV3 {
            active_preset,
            presets,
            gpu_profiles,
            miner_installs,
            mut behavior,
            ..
        } = old;

        // The old app kept one temperature policy in the profile default.
        if let Some(profile) = active_preset
            .as_ref()
            .and_then(|id| presets.get(id))
            .and_then(|preset| gpu_profiles.get(&preset.gpu_profile))
        {
            behavior.temp_limit_c = profile.default.temp_limit_c;
            behavior.temp_resume_c = profile.default.temp_resume_c;
        } else if let Some(profile) = gpu_profiles.values().next() {
            behavior.temp_limit_c = profile.default.temp_limit_c;
            behavior.temp_resume_c = profile.default.temp_resume_c;
        }

        let mut cfg = Config {
            schema_version: SCHEMA_VERSION,
            active_preset,
            presets: BTreeMap::new(),
            gpu_presets: BTreeMap::new(),
            miner_installs,
            behavior,
        };

        for (id, legacy) in presets {
            let profile = gpu_profiles
                .get(&legacy.gpu_profile)
                .cloned()
                .map(|profile| profile.into_profile(infer_custom))
                .unwrap_or_default();
            let mut preset = Preset {
                id: if legacy.id.is_empty() {
                    id.clone()
                } else {
                    legacy.id
                },
                name: legacy.name,
                coin: legacy.coin,
                coin_symbol: String::new(),
                coin_name: String::new(),
                wallet_prefix: String::new(),
                hashrate_unit: String::new(),
                algo: legacy.algo,
                miner: legacy.miner,
                miner_version: legacy.miner_version,
                pool_url: legacy.pool_url,
                pool_fee_pct: legacy.pool_fee_pct,
                pool_api_url: String::new(),
                wallet: legacy.wallet,
                worker: legacy.worker,
                gpu: profile,
                api_port: legacy.api_port,
                expected_hashrate: legacy.expected_hashrate,
                miner_options: legacy.miner_options,
                miner_fee_override_pct: legacy.miner_fee_override_pct,
                miner_fee_pct: legacy.miner_fee_pct,
                extra_args: legacy.extra_args,
                env: legacy.env,
                created_at: legacy.created_at,
                updated_at: legacy.updated_at,
            };
            preset.normalize();
            cfg.presets.insert(id, preset);
        }
        cfg.normalize()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeClass {
    None,
    Live,
    RestartRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigDiff {
    pub class: ChangeClass,
    pub reasons: Vec<String>,
}

impl ConfigDiff {
    pub fn none() -> Self {
        Self {
            class: ChangeClass::None,
            reasons: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyMode {
    Live,
    Restart,
    Stop,
}

impl ApplyMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "live" => Some(Self::Live),
            "restart" => Some(Self::Restart),
            "stop" => Some(Self::Stop),
            _ => None,
        }
    }
}

// ------------------------------------------------------------- migrations

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct ConfigV1 {
    pub wallet: String,
    pub worker: String,
    pub pool: String,
    pub algo: String,
    pub pool_fee_pct: f64,
    pub miner_profile: String,
    pub miners: BTreeMap<String, MinerCfgV1>,
    pub gpu: GpuCfgV1,
    pub api_port: u16,
    pub expected_hashrate: f64,
    pub auto_restart: bool,
    pub restart_delay_s: u64,
    pub poll_interval_s: u64,
    pub autostart_on_boot: bool,
    pub notifications: bool,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct MinerCfgV1 {
    pub path: String,
    pub dev_fee_pct: f64,
    pub extra_args: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GpuCfgV1 {
    pub power_limit: Option<u32>,
    pub core_clock: Option<u32>,
    pub memory_clock: Option<u32>,
    pub fan_speed: Option<u32>,
    pub temp_limit: u32,
    pub temp_resume: u32,
}

impl Default for GpuCfgV1 {
    fn default() -> Self {
        Self {
            // A v1 file without GPU fields never recorded tuning. Inventing
            // 300 W / 1620 / 810 and a 350 W stock limit applied clocks the
            // user never chose; missing means "auto" (no lock, no limit).
            power_limit: None,
            core_clock: None,
            memory_clock: None,
            fan_speed: None,
            temp_limit: 90,
            temp_resume: 65,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct LegacyConfigV3 {
    schema_version: u32,
    active_preset: Option<String>,
    presets: BTreeMap<String, LegacyPresetV3>,
    gpu_profiles: BTreeMap<String, LegacyGpuProfile>,
    miner_installs: BTreeMap<String, MinerInstall>,
    behavior: Behavior,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct LegacyPresetV3 {
    id: String,
    name: String,
    coin: String,
    algo: String,
    miner: String,
    miner_version: String,
    #[allow(dead_code)]
    pool: String,
    pool_url: String,
    wallet: String,
    worker: String,
    gpu_profile: String,
    api_port: u16,
    expected_hashrate: Option<f64>,
    miner_options: BTreeMap<String, serde_json::Value>,
    miner_fee_override_pct: Option<f64>,
    miner_fee_pct: f64,
    pool_fee_pct: f64,
    extra_args: String,
    env: BTreeMap<String, String>,
    created_at: u64,
    updated_at: u64,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct LegacyGpuProfile {
    #[allow(dead_code)]
    name: String,
    default: LegacyGpuTuning,
    per_gpu: BTreeMap<String, LegacyGpuTuning>,
}

#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
struct LegacyGpuTuning {
    enabled: bool,
    power_limit_w: Option<u32>,
    core_clock_mhz: Option<u32>,
    memory_clock_mhz: Option<u32>,
    #[allow(dead_code)]
    fan_pct: Option<u32>,
    temp_limit_c: u32,
    temp_resume_c: u32,
    custom: bool,
}

impl LegacyGpuTuning {
    fn has_tuning(&self) -> bool {
        self.power_limit_w.is_some()
            || self.core_clock_mhz.is_some()
            || self.memory_clock_mhz.is_some()
    }

    fn differs_from(&self, default: &LegacyGpuTuning) -> bool {
        self.power_limit_w != default.power_limit_w
            || self.core_clock_mhz != default.core_clock_mhz
            || self.memory_clock_mhz != default.memory_clock_mhz
    }

    fn into_tuning(self) -> GpuTuning {
        GpuTuning {
            enabled: self.enabled,
            power_limit_w: self.power_limit_w,
            core_clock_mhz: self.core_clock_mhz,
            memory_clock_mhz: self.memory_clock_mhz,
            custom: self.custom,
            gpu_preset: String::new(),
        }
    }
}

impl LegacyGpuProfile {
    fn into_profile(self, infer_custom: bool) -> GpuProfile {
        let default = self.default.clone();
        let mut per_gpu = BTreeMap::new();
        for (uuid, tuning) in self.per_gpu {
            let inferred = tuning.has_tuning() && tuning.differs_from(&default);
            let mut converted = tuning.into_tuning();
            if infer_custom {
                converted.custom = inferred;
            }
            per_gpu.insert(uuid, converted);
        }
        GpuProfile {
            default: default.into_tuning(),
            per_gpu,
        }
    }
}

/// Write bytes to `path` through a temporary file in the same directory.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".{}.tmp",
        path.file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "tmp".to_string())
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_have_no_preset() {
        let cfg = Config::default();
        assert!(cfg.presets.is_empty());
        assert!(cfg.active_preset.is_none());
        assert_eq!(cfg.schema_version, SCHEMA_VERSION);
        assert!(!cfg.warnings().is_empty());
    }

    #[test]
    fn appearance_is_optional_and_lowercase() {
        let cfg = serde_json::from_str::<Config>(r#"{"behavior":{"language":"en"}}"#).unwrap();
        assert_eq!(cfg.behavior.appearance, Appearance::System);
        let dark = Behavior {
            appearance: Appearance::Dark,
            ..Behavior::default()
        };
        let json = serde_json::to_string(&dark).unwrap();
        assert!(json.contains(r#""appearance":"dark""#));
    }

    #[test]
    fn restore_stock_is_on_by_default() {
        let cfg = serde_json::from_str::<Config>(r#"{"schema_version":4}"#).unwrap();
        assert!(cfg.behavior.restore_stock_on_stop);
        assert!(Behavior::default().restore_stock_on_stop);
    }

    #[test]
    fn tray_is_monochrome_by_default() {
        let cfg = serde_json::from_str::<Config>(r#"{"schema_version":4}"#).unwrap();
        assert!(cfg.behavior.tray_monochrome);
        assert!(Behavior::default().tray_monochrome);

        let colour = Behavior {
            tray_monochrome: false,
            ..Behavior::default()
        };
        let json = serde_json::to_string(&colour).unwrap();
        assert!(json.contains(r#""tray_monochrome":false"#));
    }

    #[test]
    fn tray_style_change_is_live() {
        let cfg = Config::default();
        let mut next = cfg.clone();
        next.behavior.tray_monochrome = false;
        let diff = cfg.diff(&next);
        assert_eq!(diff.class, ChangeClass::Live);
        assert!(!diff.reasons.is_empty());
    }

    #[test]
    fn round_trip() {
        let mut cfg = Config::default();
        let mut preset = Preset::new_pearl();
        preset.id = "p".into();
        preset.name = "P".into();
        cfg.presets.insert(preset.id.clone(), preset);
        cfg.active_preset = Some("p".into());
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn new_draft_is_data_driven() {
        let preset = Preset::new_draft();
        assert!(!preset.coin.is_empty());
        assert!(!preset.miner.is_empty());
        assert!(!preset.miner_version.is_empty());
        assert!(preset.gpu.default.power_limit_w.is_none());
    }

    #[test]
    fn v1_migration_keeps_wallet_without_inventing_gpu_tuning() {
        let old = ConfigV1 {
            wallet: "prl1testwallet".into(),
            worker: "rig42".into(),
            pool: "pool.pearlhash.xyz:9000".into(),
            algo: "pearlhash".into(),
            pool_fee_pct: 1.0,
            miner_profile: "wildrig".into(),
            api_port: 4028,
            expected_hashrate: 125e12,
            auto_restart: true,
            restart_delay_s: 5,
            poll_interval_s: 2,
            notifications: true,
            ..Default::default()
        };
        let cfg = Config::from_v1(old);
        let preset = cfg.preset().unwrap();
        assert_eq!(preset.wallet, "prl1testwallet");
        assert_eq!(preset.worker, "rig42");
        let tuning = &cfg.gpu_profile().unwrap().default;
        assert_eq!(tuning.power_limit_w, None);
        assert_eq!(tuning.core_clock_mhz, None);
        assert_eq!(tuning.memory_clock_mhz, None);
        assert_eq!(cfg.schema_version, SCHEMA_VERSION);
    }

    #[test]
    fn v1_migration_keeps_explicit_gpu_values_and_temperature() {
        let old: ConfigV1 = serde_json::from_value(serde_json::json!({
            "wallet": "prl1testwallet",
            "gpu": {
                "power_limit": 250,
                "core_clock": 1500,
                "memory_clock": 750,
                "fan_speed": 65,
                "temp_limit": 78,
                "temp_resume": 60
            }
        }))
        .unwrap();
        let cfg = Config::from_v1(old);
        let tuning = &cfg.gpu_profile().unwrap().default;
        assert_eq!(tuning.power_limit_w, Some(250));
        assert_eq!(tuning.core_clock_mhz, Some(1500));
        assert_eq!(tuning.memory_clock_mhz, Some(750));
        assert_eq!(cfg.behavior.temp_limit_c, 78);
        assert_eq!(cfg.behavior.temp_resume_c, 60);
    }

    #[test]
    fn v3_migration_embeds_gpu_and_moves_temperature() {
        let value = serde_json::json!({
            "schema_version": 3,
            "active_preset": "a",
            "presets": {
                "a": {
                    "id": "a", "name": "A", "coin": "pearl", "algo": "pearlhash",
                    "miner": "wildrig", "miner_version": "0.51.8",
                    "pool": "pearlhash", "pool_url": "pool.pearlhash.xyz:9000",
                    "wallet": "prl1test", "worker": "",
                    "gpu_profile": "balanced", "api_port": 4028,
                    "miner_options": {}, "miner_fee_override_pct": null,
                    "miner_fee_pct": 0.0, "pool_fee_pct": 1.0,
                    "extra_args": "", "env": {}, "created_at": 1, "updated_at": 2
                }
            },
            "gpu_profiles": {
                "balanced": {
                    "name": "Balanced",
                    "default": {
                        "enabled": true, "power_limit_w": 300,
                        "core_clock_mhz": 1620, "memory_clock_mhz": 810,
                        "temp_limit_c": 80, "temp_resume_c": 60
                    },
                    "per_gpu": {
                        "GPU-a": {"enabled": false, "custom": true, "power_limit_w": 250}
                    }
                }
            },
            "miner_installs": {},
            "behavior": {"language": "en"}
        });
        let cfg = Config::from_v3(serde_json::from_value(value).unwrap());
        assert_eq!(cfg.schema_version, 4);
        assert_eq!(cfg.behavior.temp_limit_c, 80);
        assert_eq!(cfg.behavior.temp_resume_c, 60);
        let preset = cfg.preset().unwrap();
        assert_eq!(preset.gpu.default.power_limit_w, Some(300));
        assert_eq!(preset.gpu.tuning_for("GPU-a").power_limit_w, Some(250));
        assert!(!preset.gpu.tuning_for("GPU-a").enabled);
        assert_eq!(preset.coin_symbol, "PRL");
        assert_eq!(preset.hashrate_unit, "TH/s");
    }

    #[test]
    fn v2_migration_infers_custom_only_for_real_differences() {
        let value = serde_json::json!({
            "schema_version": 2,
            "active_preset": "a",
            "presets": {
                "a": {
                    "id": "a", "name": "A", "coin": "pearl", "algo": "pearlhash",
                    "miner": "wildrig", "miner_version": "0.51.8",
                    "pool_url": "pool.pearlhash.xyz:9000",
                    "gpu_profile": "balanced"
                }
            },
            "gpu_profiles": {
                "balanced": {
                    "name": "Balanced",
                    "default": {"enabled": true, "power_limit_w": 300},
                    "per_gpu": {
                        "GPU-empty": {"enabled": true},
                        "GPU-custom": {"enabled": true, "power_limit_w": 250}
                    }
                }
            }
        });
        let cfg = Config::from_v3(serde_json::from_value(value).unwrap());
        let profile = cfg.gpu_profile().unwrap();
        assert!(!profile.is_custom("GPU-empty"));
        assert_eq!(profile.tuning_for("GPU-empty").power_limit_w, Some(300));
        assert!(profile.is_custom("GPU-custom"));
        assert_eq!(profile.tuning_for("GPU-custom").power_limit_w, Some(250));
    }

    #[test]
    fn default_profile_does_not_lock_clocks() {
        let tuning = GpuTuning::default();
        assert_eq!(tuning.power_limit_w, None);
        assert_eq!(tuning.core_clock_mhz, None);
        assert_eq!(tuning.memory_clock_mhz, None);
    }

    #[test]
    fn custom_flag_is_omitted_when_false() {
        let tuning = GpuTuning::default();
        let text = serde_json::to_string(&tuning).unwrap();
        assert!(!text.contains("custom"), "{text}");
        let custom = GpuTuning {
            custom: true,
            ..Default::default()
        };
        let text = serde_json::to_string(&custom).unwrap();
        assert!(text.contains("\"custom\":true"), "{text}");
    }

    #[test]
    fn gpu_preset_metadata_is_ignored_by_diff() {
        let mut cfg = Config::default();
        let mut preset = Preset::new_pearl();
        preset.id = "p".into();
        preset.name = "P".into();
        cfg.presets.insert(preset.id.clone(), preset);
        cfg.active_preset = Some("p".into());
        cfg.presets.get_mut("p").unwrap().gpu.per_gpu.insert(
            "GPU-a".into(),
            GpuTuning {
                custom: true,
                ..Default::default()
            },
        );
        let mut next = cfg.clone();
        next.presets
            .get_mut("p")
            .unwrap()
            .gpu
            .per_gpu
            .get_mut("GPU-a")
            .unwrap()
            .gpu_preset = "gpu-1".into();
        assert_eq!(cfg.diff(&next).class, ChangeClass::Live);
    }

    #[test]
    fn gpu_tuning_change_is_restart_required() {
        let mut cfg = Config::default();
        let mut preset = Preset::new_pearl();
        preset.id = "p".into();
        cfg.presets.insert(preset.id.clone(), preset);
        cfg.active_preset = Some("p".into());
        let mut next = cfg.clone();
        next.presets.get_mut("p").unwrap().gpu.default.power_limit_w = Some(250);
        assert_eq!(cfg.diff(&next).class, ChangeClass::RestartRequired);
    }

    #[test]
    fn normalize_keeps_verified_for_a_missing_binary() {
        let mut cfg = Config::default();
        cfg.add_install(MinerInstall {
            miner: "wildrig".into(),
            version: "0.51.8".into(),
            path: format!("/tmp/trommel-missing-{}/wildrig-multi", std::process::id()),
            sha256: "a".repeat(64),
            verified: true,
            ..Default::default()
        });
        let normalized = cfg.normalize();
        let install = normalized
            .miner_installs
            .get(&install_key("wildrig", "0.51.8"))
            .expect("install record");
        assert!(install.verified);
        let report = crate::versions::verify(&normalized, "wildrig", "0.51.8");
        assert!(!report.status.is_ok(), "{report:?}");
    }

    #[test]
    fn restore_off_with_tuning_warns() {
        let mut cfg = Config::default();
        let mut preset = Preset::new_pearl();
        preset.id = "p".into();
        preset.gpu.default.power_limit_w = Some(250);
        cfg.presets.insert(preset.id.clone(), preset);
        cfg.active_preset = Some("p".into());
        cfg.behavior.restore_stock_on_stop = false;
        let warnings = cfg.warnings();
        assert!(
            warnings
                .iter()
                .any(|warning| warning.contains("Restore stock")
                    || warning.contains("Возврат стока")),
            "{warnings:?}"
        );
    }

    #[test]
    fn fee_override_only_when_set() {
        let mut preset = Preset::new_pearl();
        preset.miner_fee_pct = 0.5;
        preset.pool_fee_pct = 1.0;
        assert_eq!(preset.total_fee_pct(), 1.5);
        preset.miner_fee_override_pct = Some(0.0);
        assert_eq!(preset.total_fee_pct(), 1.0);
    }

    #[test]
    fn diff_marks_preset_edits_as_restart() {
        let mut cfg = Config::default();
        let mut preset = Preset::new_pearl();
        preset.id = "p".into();
        cfg.presets.insert(preset.id.clone(), preset);
        cfg.active_preset = Some("p".into());
        let mut next = cfg.clone();
        next.preset_mut().unwrap().wallet = "prl1testwallet".into();
        assert_eq!(cfg.diff(&next).class, ChangeClass::RestartRequired);
    }

    #[test]
    fn redaction_hides_every_preset_wallet() {
        let mut cfg = Config::default();
        let mut active = Preset::new_pearl();
        active.id = "active".into();
        active.wallet = "prl1testactive".into();
        let mut other = Preset::new_pearl();
        other.id = "backup".into();
        other.wallet = "prl1testbackup".into();
        cfg.presets.insert(active.id.clone(), active);
        cfg.presets.insert(other.id.clone(), other);
        cfg.active_preset = Some("active".into());
        let redacted = cfg.redacted();
        for preset in redacted.presets.values() {
            assert_eq!(preset.wallet, "<redacted>");
        }
        assert_eq!(cfg.preset().unwrap().wallet, "prl1testactive");
        assert!(Config::default().redacted().presets.is_empty());
    }

    #[test]
    fn pool_host_and_name_are_derived() {
        let mut preset = Preset::new_pearl();
        assert_eq!(preset.pool_display_name(), "PearlHash");
        preset.pool_url = "example.org:3333".into();
        assert_eq!(preset.pool_display_name(), "example.org");
    }

    #[test]
    fn symbol_and_unit_fall_back_to_catalogue() {
        let mut preset = Preset::new_pearl();
        preset.coin_symbol.clear();
        preset.hashrate_unit.clear();
        assert_eq!(preset.symbol(), "PRL");
        assert_eq!(preset.hashrate_unit(), "TH/s");
        preset.coin = "custom-x".into();
        preset.coin_symbol = "CX".into();
        preset.hashrate_unit = "MH/s".into();
        assert_eq!(preset.symbol(), "CX");
        assert_eq!(preset.hashrate_unit(), "MH/s");
    }

    #[test]
    fn coin_is_filtered_by_miner_algorithms() {
        let miner = crate::catalog::miner("wildrig").unwrap();
        assert!(miner.supports_algo("pearlhash"));
    }

    #[test]
    fn watchdog_defaults_are_conservative() {
        let cfg = serde_json::from_str::<Config>(r#"{"schema_version":4,"behavior":{}}"#).unwrap();
        assert!(!cfg.behavior.watchdog_enabled);
        assert_eq!(cfg.behavior.watchdog_action, WatchdogAction::Notify);
        assert_eq!(cfg.behavior.watchdog_threshold_pct, 80);
        assert_eq!(cfg.behavior.watchdog_window_s, 300);
        assert_eq!(cfg.behavior.watchdog_max_restarts, 3);
    }

    #[test]
    fn watchdog_settings_are_clamped_and_a_v4_file_needs_no_migration() {
        let cfg = serde_json::from_str::<Config>(
            r#"{"schema_version":4,"behavior":{"watchdog_enabled":true,
                "watchdog_threshold_pct":0,"watchdog_window_s":5,
                "watchdog_max_restarts":0,"watchdog_action":"restart"}}"#,
        )
        .unwrap()
        .normalize();
        assert_eq!(cfg.schema_version, 4);
        assert!(cfg.behavior.watchdog_enabled);
        assert_eq!(cfg.behavior.watchdog_action, WatchdogAction::Restart);
        assert_eq!(cfg.behavior.watchdog_threshold_pct, 10);
        assert_eq!(cfg.behavior.watchdog_window_s, 60);
        assert_eq!(cfg.behavior.watchdog_max_restarts, 1);
    }

    #[test]
    fn watchdog_change_is_live_not_restart_required() {
        let a = Config::default();
        let mut b = a.clone();
        b.behavior.watchdog_enabled = true;
        let diff = a.diff(&b);
        assert_eq!(diff.class, ChangeClass::Live);
        assert_eq!(diff.reasons.len(), 1, "{:?}", diff.reasons);
    }
}
