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
        "oc": crate::oc::load_snapshot(),
        // Installed versions with their binary state and preset usage. The
        // recorded SHA-256 is echoed verbatim (or null); nothing is computed.
        "miner_versions": crate::versions::all_installed(cfg),
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

    #[test]
    fn miner_versions_are_reported_without_inventing_a_checksum() {
        let mut cfg = Config::default();
        cfg.add_install(crate::config::MinerInstall {
            miner: "wildrig".into(),
            version: "0.51.7".into(),
            path: "/tmp/trommel-missing-miner".into(),
            sha256: String::new(),
            source_url: String::new(),
            verified: false,
            custom: true,
            installed_at: 0,
        });
        let report = report(&cfg, None);
        let versions = report["miner_versions"].as_array().unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(versions[0]["status"], "missing");
        assert!(versions[0]["recorded_sha256"].is_null());
        assert_eq!(versions[0]["sha256_verified"], false);
    }
}
