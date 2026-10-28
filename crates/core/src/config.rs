//! Configuration v2: presets, GPU profiles, miner installs and behavior.
//!
//! The on-disk file is `$XDG_CONFIG_HOME/trommel/config.json`. Version 1
//! (the old Pearl Miner flat config) is migrated automatically on load.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::i18n::{tr, Language};

pub const SCHEMA_VERSION: u32 = 2;
pub const DEFAULT_PRESET_ID: &str = "pearl-wildrig";
pub const DEFAULT_GPU_PROFILE_ID: &str = "balanced";
pub const DEFAULT_POOL_URL: &str = "pool.pearlhash.xyz:9000";

/// Build the stable key used in `miner_installs`.
pub fn install_key(miner: &str, version: &str) -> String {
    format!("{miner}@{version}")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GpuTuning {
    pub enabled: bool,
    pub power_limit_w: Option<u32>,
    pub core_clock_mhz: Option<u32>,
    pub memory_clock_mhz: Option<u32>,
    pub fan_pct: Option<u32>,
    pub temp_limit_c: u32,
    pub temp_resume_c: u32,
}

impl Default for GpuTuning {
    fn default() -> Self {
        Self {
            enabled: true,
            // Safe defaults: do not change clocks or power until the user
            // explicitly enables a lock or a recommended profile.
            power_limit_w: None,
            core_clock_mhz: None,
            memory_clock_mhz: None,
            fan_pct: None,
            temp_limit_c: 85,
            temp_resume_c: 65,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GpuProfile {
    pub name: String,
    pub default: GpuTuning,
    /// Per-GPU overrides keyed by NVIDIA UUID (stable across reordering).
    pub per_gpu: BTreeMap<String, GpuTuning>,
}

impl GpuProfile {
    pub fn tuning_for(&self, uuid: &str) -> GpuTuning {
        self.per_gpu
            .get(uuid)
            .cloned()
            .unwrap_or_else(|| self.default.clone())
    }
}

impl Default for GpuProfile {
    fn default() -> Self {
        Self {
            name: "Balanced".to_string(),
            default: GpuTuning::default(),
            per_gpu: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub coin: String,
    pub algo: String,
    pub miner: String,
    pub miner_version: String,
    pub pool: String,
    pub pool_url: String,
    pub wallet: String,
    pub worker: String,
    pub gpu_profile: String,
    pub api_port: u16,
    pub expected_hashrate: Option<f64>,
    /// Values of miner-specific options (see the catalog descriptors).
    pub miner_options: BTreeMap<String, serde_json::Value>,
    /// Only used when the selected miner allows a fee override.
    pub miner_fee_override_pct: Option<f64>,
    /// Cached catalog fee for this preset (miner/algorithm).
    pub miner_fee_pct: f64,
    pub pool_fee_pct: f64,
    pub extra_args: String,
    pub env: BTreeMap<String, String>,
    pub created_at: u64,
    pub updated_at: u64,
}

impl Preset {
    pub fn new_pearl() -> Self {
        Self {
            id: DEFAULT_PRESET_ID.to_string(),
            name: tr("Pearl · WildRig", "Pearl · WildRig"),
            coin: "pearl".into(),
            algo: "pearlhash".into(),
            miner: "wildrig".into(),
            miner_version: "0.51.8".into(),
            pool: "pearlhash".into(),
            pool_url: DEFAULT_POOL_URL.into(),
            wallet: String::new(),
            worker: String::new(),
            gpu_profile: DEFAULT_GPU_PROFILE_ID.into(),
            api_port: 4028,
            expected_hashrate: None,
            miner_options: BTreeMap::new(),
            miner_fee_override_pct: None,
            miner_fee_pct: 0.0,
            pool_fee_pct: 1.0,
            extra_args: String::new(),
            env: BTreeMap::new(),
            created_at: 0,
            updated_at: 0,
        }
    }

    pub fn effective_miner_fee_pct(&self) -> f64 {
        self.miner_fee_override_pct.unwrap_or(self.miner_fee_pct)
    }

    pub fn total_fee_pct(&self) -> f64 {
        self.effective_miner_fee_pct() + self.pool_fee_pct
    }
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

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct CustomMiner {
    pub id: String,
    pub name: String,
    pub description: String,
    pub path: String,
    pub fee_pct: f64,
    pub verified: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct CustomPool {
    pub id: String,
    pub name: String,
    pub url: String,
    pub fee_pct: f64,
    pub api_kind: String,
    pub api_url: String,
    pub min_payout: f64,
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
    pub language: Language,
    /// Hide advanced miner options and extra arguments in the UI.
    pub simple_mode: bool,
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
            language: Language::System,
            simple_mode: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub schema_version: u32,
    pub active_preset: Option<String>,
    pub presets: BTreeMap<String, Preset>,
    pub gpu_profiles: BTreeMap<String, GpuProfile>,
    pub miner_installs: BTreeMap<String, MinerInstall>,
    pub custom_miners: BTreeMap<String, CustomMiner>,
    pub pools: BTreeMap<String, CustomPool>,
    pub behavior: Behavior,
}

impl Default for Config {
    fn default() -> Self {
        let mut presets = BTreeMap::new();
        presets.insert(DEFAULT_PRESET_ID.to_string(), Preset::new_pearl());
        let mut gpu_profiles = BTreeMap::new();
        gpu_profiles.insert(DEFAULT_GPU_PROFILE_ID.to_string(), GpuProfile::default());
        Self {
            schema_version: SCHEMA_VERSION,
            active_preset: Some(DEFAULT_PRESET_ID.to_string()),
            presets,
            gpu_profiles,
            miner_installs: BTreeMap::new(),
            custom_miners: BTreeMap::new(),
            pools: BTreeMap::new(),
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
        if version >= 2 {
            let cfg: Config = serde_json::from_value(value)
                .map_err(|e| anyhow::anyhow!("cannot parse {}: {e}", path.display()))?;
            Ok(cfg.normalize())
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
    pub fn normalize(mut self) -> Self {
        if self.schema_version == 0 {
            self.schema_version = SCHEMA_VERSION;
        }
        if self.gpu_profiles.is_empty() {
            self.gpu_profiles
                .insert(DEFAULT_GPU_PROFILE_ID.to_string(), GpuProfile::default());
        }
        if self.presets.is_empty() {
            self.presets
                .insert(DEFAULT_PRESET_ID.to_string(), Preset::new_pearl());
        }
        let active_ok = self
            .active_preset
            .as_ref()
            .map(|id| self.presets.contains_key(id))
            .unwrap_or(false);
        if !active_ok {
            self.active_preset = self.presets.keys().next().cloned();
        }
        for install in self.miner_installs.values_mut() {
            if !install.path.is_empty() && !Path::new(&install.path).exists() {
                install.verified = false;
            }
        }
        for preset in self.presets.values_mut() {
            if let Some(def) = crate::catalog::miner(&preset.miner) {
                preset.miner_fee_pct = def.fee_pct;
                if !def.fee_configurable {
                    preset.miner_fee_override_pct = None;
                }
            }
        }
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

    pub fn ensure_preset(&mut self) -> &mut Preset {
        if self.active_preset.is_none() {
            let id = DEFAULT_PRESET_ID.to_string();
            self.presets
                .entry(id.clone())
                .or_insert_with(Preset::new_pearl);
            self.active_preset = Some(id);
        }
        let id = self.active_preset.clone().unwrap_or_default();
        self.presets
            .entry(id.clone())
            .or_insert_with(Preset::new_pearl);
        self.presets.get_mut(&id).expect("preset inserted above")
    }

    pub fn gpu_profile(&self) -> Option<&GpuProfile> {
        let id = self
            .preset()
            .map(|p| p.gpu_profile.as_str())
            .unwrap_or(DEFAULT_GPU_PROFILE_ID);
        self.gpu_profiles.get(id)
    }

    pub fn gpu_profile_mut(&mut self) -> &mut GpuProfile {
        let id = self
            .preset()
            .map(|p| p.gpu_profile.clone())
            .unwrap_or_else(|| DEFAULT_GPU_PROFILE_ID.to_string());
        self.gpu_profiles.entry(id.clone()).or_default();
        self.gpu_profiles
            .get_mut(&id)
            .expect("profile inserted above")
    }

    pub fn install(&self, miner: &str, version: &str) -> Option<&MinerInstall> {
        self.miner_installs
            .get(&install_key(miner, version))
            .filter(|install| Path::new(&install.path).exists())
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
        } else if !preset.wallet.starts_with("prl1") {
            out.push(tr(
                "Wallet does not look like a Pearl address (prl1…)",
                "Кошелёк не похож на адрес Pearl (ожидается prl1…)",
            ));
        }
        if !preset.pool_url.contains(':') {
            out.push(tr(
                "Pool address has no port (example: pool.pearlhash.xyz:9000)",
                "В адресе пула нет порта (пример: pool.pearlhash.xyz:9000)",
            ));
        }
        if self.install_for(preset).is_none() {
            out.push(format!(
                "{} {} {}",
                tr("Miner is not installed:", "Майнер не установлен:"),
                preset.miner,
                preset.miner_version
            ));
        }
        let profile = self.gpu_profile();
        let any_enabled = profile
            .map(|p| p.default.enabled || p.per_gpu.values().any(|t| t.enabled))
            .unwrap_or(true);
        if !any_enabled {
            out.push(tr(
                "No GPU is enabled in the active profile",
                "В активном профиле не включено ни одной видеокарты",
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
        if self.gpu_profiles != next.gpu_profiles {
            reasons.push(tr("GPU profile changed", "Изменился профиль GPU"));
            class = ChangeClass::RestartRequired;
        }
        if self.preset() != next.preset() {
            reasons.push(tr(
                "Preset settings changed",
                "Изменились настройки пресета",
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
        ConfigDiff { class, reasons }
    }

    pub fn from_v1(old: ConfigV1) -> Self {
        let mut cfg = Config::default();
        cfg.presets.clear();
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
        preset.miner_version = "0.51.8".into();
        preset.api_port = old.api_port;
        preset.expected_hashrate = if old.expected_hashrate > 0.0 {
            Some(old.expected_hashrate)
        } else {
            None
        };
        preset.pool_fee_pct = old.pool_fee_pct;
        preset.miner_fee_pct = old
            .miners
            .get(&preset.miner)
            .map(|m| m.dev_fee_pct)
            .unwrap_or(0.0);
        preset.gpu_profile = "imported".to_string();
        if let Some(fan) = old.gpu.fan_speed {
            preset
                .miner_options
                .insert("fan_speed".into(), serde_json::json!(fan));
        }
        preset.updated_at = crate::util::now_unix();
        cfg.presets.insert(preset.id.clone(), preset.clone());

        let profile = GpuProfile {
            name: tr("Imported", "Импортирован"),
            default: GpuTuning {
                power_limit_w: old.gpu.power_limit,
                core_clock_mhz: old.gpu.core_clock,
                memory_clock_mhz: old.gpu.memory_clock,
                fan_pct: old.gpu.fan_speed,
                temp_limit_c: old.gpu.temp_limit,
                temp_resume_c: old.gpu.temp_resume,
                ..Default::default()
            },
            per_gpu: BTreeMap::new(),
        };
        cfg.gpu_profiles.clear();
        cfg.gpu_profiles.insert("imported".into(), profile);

        for (id, miner) in old.miners {
            if miner.path.trim().is_empty() {
                continue;
            }
            cfg.add_install(MinerInstall {
                miner: id.clone(),
                version: "0.51.8".to_string(),
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
        cfg
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

/// Old flat configuration (v1), kept only for migration.
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
    pub stock_power_limit: u32,
    pub core_clock: Option<u32>,
    pub memory_clock: Option<u32>,
    pub fan_speed: Option<u32>,
    pub temp_limit: u32,
    pub temp_resume: u32,
}

impl Default for GpuCfgV1 {
    fn default() -> Self {
        Self {
            power_limit: Some(300),
            stock_power_limit: 350,
            core_clock: Some(1620),
            memory_clock: Some(810),
            fan_speed: None,
            temp_limit: 90,
            temp_resume: 65,
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
    fn defaults_have_a_safe_empty_preset() {
        let cfg = Config::default();
        let preset = cfg.preset().unwrap();
        assert_eq!(preset.pool_url, DEFAULT_POOL_URL);
        assert!(preset.wallet.is_empty());
        assert_eq!(cfg.total_fee_pct(), 1.0);
        assert!(!cfg.warnings().is_empty());
    }

    #[test]
    fn round_trip() {
        let cfg = Config::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn v1_migration_keeps_wallet_and_gpu() {
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
        assert_eq!(cfg.gpu_profile().unwrap().default.power_limit_w, Some(300));
        assert_eq!(
            cfg.gpu_profile().unwrap().default.core_clock_mhz,
            Some(1620)
        );
        assert_eq!(
            cfg.gpu_profile().unwrap().default.memory_clock_mhz,
            Some(810)
        );
        assert_eq!(preset.total_fee_pct(), 1.0);
    }

    #[test]
    fn default_profile_does_not_lock_clocks() {
        let tuning = GpuTuning::default();
        assert_eq!(tuning.power_limit_w, None);
        assert_eq!(tuning.core_clock_mhz, None);
        assert_eq!(tuning.memory_clock_mhz, None);
        assert_eq!(tuning.temp_limit_c, 85);
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
        let cfg = Config::default();
        let mut next = cfg.clone();
        next.preset_mut().unwrap().wallet = "prl1testwallet".into();
        assert_eq!(cfg.diff(&next).class, ChangeClass::RestartRequired);
    }
}
