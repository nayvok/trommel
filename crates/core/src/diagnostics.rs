//! Redacted diagnostics shared by the GUI and `trommel diagnose`.
//!
//! Wallets are public but personal: every preset is redacted, not only the
//! active one, because an agent bundle must never leak an inactive preset.

use serde_json::{json, Value};

use crate::config::Config;
use crate::paths;
use crate::status::Status;

pub fn report(cfg: &Config, status: Option<&Status>) -> Value {
    json!({
        "app": "Trommel",
        "version": crate::VERSION,
        "contract_version": crate::status::CONTRACT_VERSION,
        "config_path": paths::config_path(),
        "state_path": paths::state_path(),
        "config": cfg.redacted(),
        "status": status,
        "warnings": cfg.warnings(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Preset;

    #[test]
    fn every_preset_wallet_is_redacted() {
        let mut cfg = Config::default();
        let mut active = cfg.preset().cloned().unwrap_or_else(Preset::new_pearl);
        active.id = "active".into();
        active.name = "Active".into();
        active.wallet = "prl1testactivewallet".into();
        let mut other = active.clone();
        other.id = "other".into();
        other.name = "Other".into();
        other.wallet = "prl1testinactivewallet".into();
        cfg.presets.clear();
        cfg.presets.insert(active.id.clone(), active);
        cfg.presets.insert(other.id.clone(), other);
        cfg.active_preset = Some("active".into());

        let text = report(&cfg, None).to_string();
        assert!(!text.contains("prl1testactivewallet"), "{text}");
        assert!(!text.contains("prl1testinactivewallet"), "{text}");
        assert!(text.contains("<redacted>"));
    }
}
