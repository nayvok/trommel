//! Redacted diagnostics shared by the GUI and `trommel diagnose`.
//!
//! Wallets are public but personal: every preset is redacted, not only the
//! active one, because an agent bundle must never leak an inactive preset.

use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use serde_json::{json, Value};

use crate::config::Config;
use crate::paths;
use crate::status::Status;

pub fn report(cfg: &Config, status: Option<&Status>) -> Value {
    let mut value = json!({
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
    });
    redact_home(&mut value);
    value
}

fn redact_home(value: &mut Value) {
    let Ok(home) = std::env::var("HOME") else {
        return;
    };
    if home.is_empty() {
        return;
    }
    fn walk(value: &mut Value, home: &str) {
        match value {
            Value::String(text) => {
                if text.contains(home) {
                    *text = text.replace(home, "~");
                }
            }
            Value::Array(items) => {
                for item in items {
                    walk(item, home);
                }
            }
            Value::Object(map) => {
                for item in map.values_mut() {
                    walk(item, home);
                }
            }
            _ => {}
        }
    }
    walk(value, &home);
}

/// A prefilled GitHub issue URL with redacted diagnostics.
///
/// The body is deliberately capped: GitHub rejects very large query strings.
/// When the full report is longer, the caller should copy it to the clipboard
/// and tell the user to paste it.
pub fn issue_url(cfg: &Config, status: Option<&Status>, error: &str) -> String {
    let report = report(cfg, status);
    let mut body = serde_json::to_string_pretty(&report).unwrap_or_default();
    const LIMIT: usize = 6000;
    if body.len() > LIMIT {
        body.truncate(LIMIT);
        body.push_str("\n… (truncated; copy diagnostics from Settings for the full bundle)");
    }
    let first_line = error.lines().next().unwrap_or("").trim();
    let title = if first_line.is_empty() {
        format!("Trommel {}: bug report", crate::VERSION)
    } else {
        format!("Trommel {}: {}", crate::VERSION, first_line)
    };
    let body = format!(
        "**Describe what happened**\n\n\n**Steps to reproduce**\n1. \n2. \n3. \n\n         **Error**\n```\n{error}\n```\n\n**Diagnostics (wallets redacted)**\n```json\n{body}\n```\n"
    );
    format!(
        "https://github.com/nayvok/trommel/issues/new?title={}&body={}",
        utf8_percent_encode(&title, NON_ALPHANUMERIC),
        utf8_percent_encode(&body, NON_ALPHANUMERIC)
    )
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
    fn issue_url_redacts_wallets_and_home_paths() {
        let mut cfg = Config::default();
        let mut preset = Preset::new_pearl();
        preset.id = "active".into();
        preset.wallet = "prl1testsecretwallet".into();
        cfg.presets.insert(preset.id.clone(), preset);
        cfg.active_preset = Some("active".into());
        let url = issue_url(&cfg, None, "miner API changed");
        assert!(url.starts_with("https://github.com/nayvok/trommel/issues/new?"));
        assert!(!url.contains("prl1testsecretwallet"), "{url}");
        assert!(
            !url.contains(&std::env::var("HOME").unwrap_or_default()),
            "{url}"
        );
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
