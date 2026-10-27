//! Configuration file (`~/.config/pearl-miner/config.json`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::paths;

/// Overclock / power settings applied through the polkit helper before start.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct GpuCfg {
    /// Mining power limit in watts (`nvidia-smi -pl`). `None` = do not touch.
    pub power_limit: Option<u32>,
    /// Stock power limit restored on stop ("играть" profile).
    pub stock_power_limit: u32,
    /// Locked core clock in MHz (`nvidia-smi -lgc N,N`). `None` = do not touch.
    pub core_clock: Option<u32>,
    /// Locked memory clock in MHz (`nvidia-smi -lmc N`). `None` = do not touch.
    pub memory_clock: Option<u32>,
    /// Fan speed in percent, passed to the miner (`--gpu-fan-speed`).
    /// `nvidia-smi` on consumer cards cannot set fan speed, so this is not
    /// applied through the polkit helper.
    pub fan_speed: Option<u32>,
    /// Pause mining above this temperature (miner-side).
    pub temp_limit: u32,
    /// Resume mining below this temperature (miner-side).
    pub temp_resume: u32,
}

impl Default for GpuCfg {
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

/// Per-miner settings (binary path, dev fee and free-form extra arguments).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct MinerCfg {
    pub path: String,
    pub dev_fee_pct: f64,
    pub extra_args: String,
}

impl Default for MinerCfg {
    fn default() -> Self {
        Self {
            path: String::new(),
            dev_fee_pct: 0.0,
            extra_args: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub wallet: String,
    pub worker: String,
    pub pool: String,
    pub algo: String,
    pub pool_fee_pct: f64,
    /// Active miner profile id: `wildrig`, `srbminer` or `onezero`.
    pub miner_profile: String,
    pub miners: BTreeMap<String, MinerCfg>,
    pub gpu: GpuCfg,
    /// Local HTTP statistics port of the miner.
    pub api_port: u16,
    /// Expected hash rate in H/s, used for the progress bars ("~130 TH/s").
    pub expected_hashrate: f64,
    pub auto_restart: bool,
    pub restart_delay_s: u64,
    pub poll_interval_s: u64,
    pub autostart_on_boot: bool,
    pub notifications: bool,
    pub show_temp_in_panel: bool,
}

impl Default for Config {
    fn default() -> Self {
        let mut miners = BTreeMap::new();
        miners.insert(
            "wildrig".to_string(),
            MinerCfg {
                path: paths::home()
                    .join("pearl")
                    .join("wildrig-multi")
                    .to_string_lossy()
                    .into_owned(),
                dev_fee_pct: 0.0,
                extra_args: String::new(),
            },
        );
        miners.insert(
            "srbminer".to_string(),
            MinerCfg {
                path: String::new(),
                dev_fee_pct: 0.85,
                extra_args: String::new(),
            },
        );
        miners.insert(
            "onezero".to_string(),
            MinerCfg {
                path: String::new(),
                dev_fee_pct: 1.0,
                extra_args: String::new(),
            },
        );

        Self {
            wallet: "prl1testwallet".to_string(),
            worker: "3080ti".to_string(),
            pool: "pool.pearlhash.xyz:9000".to_string(),
            algo: "pearlhash".to_string(),
            pool_fee_pct: 1.0,
            miner_profile: "wildrig".to_string(),
            miners,
            gpu: GpuCfg::default(),
            api_port: 4028,
            expected_hashrate: 125.0e12,
            auto_restart: true,
            restart_delay_s: 5,
            poll_interval_s: 2,
            autostart_on_boot: false,
            notifications: true,
            show_temp_in_panel: true,
        }
    }
}

impl Config {
    /// Load from disk, falling back to defaults when the file is missing.
    pub fn load() -> anyhow::Result<Self> {
        let path = paths::config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)?;
        let cfg: Config = serde_json::from_str(&text)
            .map_err(|e| anyhow::anyhow!("не удалось разобрать {}: {e}", path.display()))?;
        Ok(cfg)
    }

    /// Load without failing: on a broken file the defaults are returned.
    pub fn load_or_default() -> Self {
        Self::load().unwrap_or_default()
    }

    /// Atomic save (write to a temporary file, then rename).
    pub fn save(&self) -> anyhow::Result<()> {
        let path = paths::config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(self)?;
        write_atomic(&path, text.as_bytes())
    }

    /// Settings of the given miner profile, created on demand from defaults.
    pub fn miner(&self, id: &str) -> MinerCfg {
        self.miners.get(id).cloned().unwrap_or(MinerCfg {
            path: String::new(),
            dev_fee_pct: match id {
                "wildrig" => 0.0,
                "srbminer" => 0.85,
                "onezero" => 1.0,
                _ => 0.0,
            },
            extra_args: String::new(),
        })
    }

    /// Resolved binary path of the active miner profile.
    pub fn miner_path(&self) -> PathBuf {
        PathBuf::from(self.miner(self.miner_profile.trim()).path)
    }

    /// Combined dev fee of the miner and the pool, in percent.
    pub fn total_fee_pct(&self) -> f64 {
        self.miner(self.miner_profile.trim()).dev_fee_pct + self.pool_fee_pct
    }

    /// Non-fatal configuration problems shown in the UI.
    pub fn warnings(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !self.wallet.starts_with("prl1") {
            out.push("Кошелёк не похож на адрес Pearl (ожидается префикс prl1)".to_string());
        }
        if !self.pool.contains(':') {
            out.push("В адресе пула нет порта (пример: pool.pearlhash.xyz:9000)".to_string());
        }
        if self.api_port == 0 {
            out.push("Порт API майнера не задан — статистика будет читаться из лога".to_string());
        }
        if self.poll_interval_s < 1 {
            out.push("Интервал опроса меньше секунды — используется 1 с".to_string());
        }
        if let Some(pl) = self.gpu.power_limit {
            if pl < 50 || pl > 1000 {
                out.push(format!("Лимит питания {pl} W выглядит нереально"));
            }
        }
        if !self.miner_path().exists() {
            out.push(format!(
                "Бинарь майнера не найден: {}",
                self.miner_path().display()
            ));
        }
        out
    }
}

/// Write bytes to `path` through a temporary file in the same directory.
pub fn write_atomic(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
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
    fn defaults_match_the_existing_setup() {
        let cfg = Config::default();
        assert_eq!(cfg.pool, "pool.pearlhash.xyz:9000");
        assert_eq!(cfg.worker, "3080ti");
        assert_eq!(cfg.api_port, 4028);
        assert_eq!(cfg.gpu.power_limit, Some(300));
        assert_eq!(cfg.gpu.stock_power_limit, 350);
        assert_eq!(cfg.gpu.core_clock, Some(1620));
        assert_eq!(cfg.gpu.memory_clock, Some(810));
        assert_eq!(cfg.miner_profile, "wildrig");
        assert_eq!(cfg.miner("wildrig").dev_fee_pct, 0.0);
        assert_eq!(cfg.miner("srbminer").dev_fee_pct, 0.85);
        assert_eq!(cfg.total_fee_pct(), 1.0);
    }

    #[test]
    fn round_trip() {
        let cfg = Config::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, back);
    }

    #[test]
    fn partial_json_falls_back_to_defaults() {
        let cfg: Config = serde_json::from_str(r#"{"worker":"rig42"}"#).unwrap();
        assert_eq!(cfg.worker, "rig42");
        assert_eq!(cfg.pool, "pool.pearlhash.xyz:9000");
        assert_eq!(cfg.gpu.temp_limit, 90);
    }

    #[test]
    fn warnings_catch_broken_pool() {
        let mut cfg = Config::default();
        cfg.pool = "pool.pearlhash.xyz".to_string();
        cfg.wallet = "nope".to_string();
        let w = cfg.warnings();
        assert!(w.iter().any(|s| s.contains("пула")));
        assert!(w.iter().any(|s| s.contains("Кошелёк")));
    }
}
