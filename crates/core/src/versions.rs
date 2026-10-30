//! Miner version manager.
//!
//! Installed miners are global (`miner_installs`, keyed `miner@version`); a
//! preset only pins one version. This module answers the questions the miner
//! library, the CLI and the diagnostics bundle ask: which versions are
//! installed, which presets use each one, what an update would touch, whether
//! a binary still exists, and whether a version may be removed.
//!
//! Nothing here rewrites a preset implicitly. [`pin_preset`] and
//! [`pin_presets`] change exactly the presets the caller names; an install or an
//! update never edits `preset.miner_version`. Checksums are never computed or
//! invented: a report only echoes the SHA-256 recorded at install time, and
//! `None` means the install was recorded without one.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::catalog;
use crate::config::{install_key, Config, MinerInstall};
use crate::download;
use crate::i18n::tr;
use crate::util;

/// What an install record's binary looks like on disk right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BinaryStatus {
    /// The recorded path exists, is a regular file and has an execute bit.
    Ok,
    /// There is no install record for the miner/version.
    NotRecorded,
    /// Nothing exists at the recorded path.
    Missing,
    /// The path exists but is not a regular file.
    NotAFile,
    /// The file exists but no execute bit is set.
    NotExecutable,
}

impl BinaryStatus {
    /// True when the recorded binary can be executed.
    pub fn is_ok(self) -> bool {
        self == Self::Ok
    }

    /// Stable machine-readable name for diagnostics and scripts.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NotRecorded => "not_recorded",
            Self::Missing => "missing",
            Self::NotAFile => "not_a_file",
            Self::NotExecutable => "not_executable",
        }
    }
}

/// A preset that references a miner version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PresetUsage {
    pub id: String,
    pub name: String,
    pub miner: String,
    /// The version this preset pins (equal to the queried version in a version
    /// row, but shown explicitly so the UI can state the pin).
    pub miner_version: String,
    pub active: bool,
}

/// One installed version with disk state and preset usage.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InstalledVersion {
    pub miner: String,
    pub version: String,
    pub key: String,
    pub path: String,
    pub custom: bool,
    /// True when a SHA-256 was recorded and verified at install time.
    pub sha256_verified: bool,
    /// The recorded digest, verbatim. `None` when the install has none; this
    /// module never derives one.
    pub recorded_sha256: Option<String>,
    pub installed_at: u64,
    pub status: BinaryStatus,
    pub used_by: Vec<PresetUsage>,
}

/// Result of checking one installed binary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct VerifyReport {
    pub miner: String,
    pub version: String,
    /// Recorded path; empty when there is no install record.
    pub path: String,
    pub status: BinaryStatus,
    pub exists: bool,
    pub is_file: bool,
    pub executable: bool,
    pub readable: bool,
    /// The digest recorded at install time, echoed verbatim.
    pub recorded_sha256: Option<String>,
    pub sha256_verified: bool,
    /// Localized one-line result for the UI and human CLI output.
    pub message: String,
}

/// What installing/updating `target_version` would leave unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UpdatePlan {
    pub miner: String,
    pub target_version: String,
    /// The target binary is present and executable.
    pub target_installed: bool,
    /// Catalogue latest version, when the miner is in the catalogue.
    pub latest: Option<String>,
    /// A newer catalogue version is not installed yet.
    pub update_available: bool,
    /// Every installed version for this miner, version-sorted.
    pub installed: Vec<String>,
    /// Presets that pin this miner at another version. Installing the target
    /// does not touch them; moving one is an explicit choice.
    pub affected: Vec<PresetUsage>,
}

/// A version the user may remove, or why removal is blocked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemovePlan {
    pub miner: String,
    pub version: String,
    /// True only when no preset references the version.
    pub allowed: bool,
    pub used_by: Vec<PresetUsage>,
    /// Managed directory that would be deleted. `None` when the recorded
    /// binary lives outside the store and is left untouched.
    pub delete_dir: Option<String>,
    pub recorded_path: String,
}

/// Summary of a completed removal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RemovalReport {
    pub miner: String,
    pub version: String,
    /// True when files under the managed store were deleted.
    pub removed_files: bool,
    /// Directory removed, if any.
    pub delete_dir: Option<String>,
    /// The path that was recorded before the record disappeared.
    pub recorded_path: String,
}

/// One preset whose pin changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PresetChange {
    pub id: String,
    pub name: String,
    pub from: String,
    pub to: String,
}

fn recorded_sha256(install: &MinerInstall) -> Option<String> {
    let value = install.sha256.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// Inspect a recorded path without following it into a fabricated success.
fn inspect(path: &str) -> (BinaryStatus, bool, bool, bool, bool) {
    if path.trim().is_empty() {
        return (BinaryStatus::Missing, false, false, false, false);
    }
    let path = Path::new(path);
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(_) => return (BinaryStatus::Missing, false, false, false, false),
    };
    if !metadata.is_file() {
        return (BinaryStatus::NotAFile, true, false, false, false);
    }
    #[cfg(unix)]
    let executable = {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    };
    #[cfg(not(unix))]
    let executable = true;
    let readable = std::fs::File::open(path).is_ok();
    let status = if !executable {
        BinaryStatus::NotExecutable
    } else if !readable {
        // A file we cannot open is not usable even with the execute bit.
        BinaryStatus::NotExecutable
    } else {
        BinaryStatus::Ok
    };
    (status, true, true, executable, readable)
}

fn verify_install(install: &MinerInstall) -> VerifyReport {
    let (status, exists, is_file, executable, readable) = inspect(&install.path);
    let recorded = recorded_sha256(install);
    let message = match status {
        BinaryStatus::Ok => {
            if recorded.is_some() && install.verified {
                tr(
                    "The binary exists and is executable; the recorded SHA-256 was verified at install time",
                    "Бинарник существует и исполняем; записанный SHA-256 был проверен при установке",
                )
            } else if recorded.is_some() {
                tr(
                    "The binary exists and is executable; a SHA-256 is recorded but was not verified at install time",
                    "Бинарник существует и исполняем; SHA-256 записан, но не был проверен при установке",
                )
            } else {
                tr(
                    "The binary exists and is executable; no SHA-256 was recorded",
                    "Бинарник существует и исполняем; SHA-256 не записан",
                )
            }
        }
        BinaryStatus::Missing => format!(
            "{} {}",
            tr("The binary is missing at", "Бинарник отсутствует:"),
            install.path
        ),
        BinaryStatus::NotAFile => format!(
            "{} {}",
            tr(
                "The recorded path is not a regular file:",
                "Записанный путь не является файлом:"
            ),
            install.path
        ),
        BinaryStatus::NotExecutable => format!(
            "{} {}",
            tr(
                "The binary is not executable or not readable:",
                "Бинарник не исполняем или недоступен для чтения:"
            ),
            install.path
        ),
        BinaryStatus::NotRecorded => tr(
            "No install record for this miner version",
            "Для этой версии майнера нет записи об установке",
        ),
    };
    VerifyReport {
        miner: install.miner.clone(),
        version: install.version.clone(),
        path: install.path.clone(),
        status,
        exists,
        is_file,
        executable,
        readable,
        sha256_verified: install.verified && recorded.is_some(),
        recorded_sha256: recorded,
        message,
    }
}

/// Check that a recorded install still has an executable binary.
pub fn verify(cfg: &Config, miner: &str, version: &str) -> VerifyReport {
    let key = install_key(miner, version);
    match cfg.miner_installs.get(&key) {
        Some(install) => verify_install(install),
        None => VerifyReport {
            miner: miner.to_string(),
            version: version.to_string(),
            path: String::new(),
            status: BinaryStatus::NotRecorded,
            exists: false,
            is_file: false,
            executable: false,
            readable: false,
            recorded_sha256: None,
            sha256_verified: false,
            message: tr(
                "No install record for this miner version",
                "Для этой версии майнера нет записи об установке",
            ),
        },
    }
}

/// Verify every recorded install, sorted by miner and version. Versions whose
/// binary disappeared are included so the diagnostics bundle shows them.
pub fn verify_all(cfg: &Config) -> Vec<VerifyReport> {
    let mut reports: Vec<VerifyReport> = cfg.miner_installs.values().map(verify_install).collect();
    reports.sort_by(|a, b| (&a.miner, &a.version).cmp(&(&b.miner, &b.version)));
    reports
}

/// Presets that pin exactly `miner@version`, sorted by name.
pub fn usage(cfg: &Config, miner: &str, version: &str) -> Vec<PresetUsage> {
    let mut users: Vec<PresetUsage> = cfg
        .presets
        .values()
        .filter(|preset| preset.miner == miner && preset.miner_version == version)
        .map(|preset| PresetUsage {
            id: preset.id.clone(),
            name: preset.name.clone(),
            miner: preset.miner.clone(),
            miner_version: preset.miner_version.clone(),
            active: cfg.active_preset.as_deref() == Some(preset.id.as_str()),
        })
        .collect();
    users.sort_by(|a, b| a.name.cmp(&b.name));
    users
}

/// Presets that use this miner at any version, sorted by name.
pub fn presets_for_miner(cfg: &Config, miner: &str) -> Vec<PresetUsage> {
    let mut users: Vec<PresetUsage> = cfg
        .presets
        .values()
        .filter(|preset| preset.miner == miner)
        .map(|preset| PresetUsage {
            id: preset.id.clone(),
            name: preset.name.clone(),
            miner: preset.miner.clone(),
            miner_version: preset.miner_version.clone(),
            active: cfg.active_preset.as_deref() == Some(preset.id.as_str()),
        })
        .collect();
    users.sort_by(|a, b| a.name.cmp(&b.name));
    users
}

/// Every recorded version for one miner, including a missing binary.
pub fn installed(cfg: &Config, miner: &str) -> Vec<InstalledVersion> {
    let mut versions: Vec<InstalledVersion> = cfg
        .miner_installs
        .values()
        .filter(|install| install.miner == miner)
        .map(|install| {
            let (status, ..) = inspect(&install.path);
            InstalledVersion {
                miner: install.miner.clone(),
                version: install.version.clone(),
                key: install_key(&install.miner, &install.version),
                path: install.path.clone(),
                custom: install.custom,
                sha256_verified: install.verified && recorded_sha256(install).is_some(),
                recorded_sha256: recorded_sha256(install),
                installed_at: install.installed_at,
                status,
                used_by: usage(cfg, &install.miner, &install.version),
            }
        })
        .collect();
    versions.sort_by(|a, b| a.version.cmp(&b.version));
    versions
}

/// Every recorded install for every miner, sorted by miner and version.
pub fn all_installed(cfg: &Config) -> Vec<InstalledVersion> {
    let mut ids: Vec<String> = cfg
        .miner_installs
        .values()
        .map(|install| install.miner.clone())
        .collect();
    ids.sort();
    ids.dedup();
    let mut versions: Vec<InstalledVersion> = ids
        .into_iter()
        .flat_map(|miner| installed(cfg, &miner))
        .collect();
    versions.sort_by(|a, b| (&a.miner, &a.version).cmp(&(&b.miner, &b.version)));
    versions
}

/// Miner ids the library must show: the catalogue plus stored custom builds.
pub fn miner_ids(cfg: &Config) -> Vec<String> {
    let mut ids: Vec<String> = catalog::miners()
        .iter()
        .map(|miner| miner.id.clone())
        .collect();
    ids.extend(
        cfg.miner_installs
            .values()
            .map(|install| install.miner.clone()),
    );
    ids.sort();
    ids.dedup();
    ids
}

/// Catalogue latest version, when the miner is catalogued.
pub fn latest_version(miner: &str) -> Option<String> {
    catalog::miner(miner)
        .and_then(|def| def.latest_version())
        .map(|item| item.version.clone())
}

/// Describe the impact of installing `target_version` without changing it.
pub fn update_plan(cfg: &Config, miner: &str, target_version: &str) -> UpdatePlan {
    let installed_versions: Vec<String> = installed(cfg, miner)
        .into_iter()
        .map(|item| item.version)
        .collect();
    let target_installed = verify(cfg, miner, target_version).status.is_ok();
    let latest = latest_version(miner);
    let offered = latest
        .as_ref()
        .map(|version| version != target_version && !installed_versions.contains(version))
        .unwrap_or(false);
    let mut affected: Vec<PresetUsage> = cfg
        .presets
        .values()
        .filter(|preset| preset.miner == miner && preset.miner_version != target_version)
        .map(|preset| PresetUsage {
            id: preset.id.clone(),
            name: preset.name.clone(),
            miner: preset.miner.clone(),
            miner_version: preset.miner_version.clone(),
            active: cfg.active_preset.as_deref() == Some(preset.id.as_str()),
        })
        .collect();
    affected.sort_by(|a, b| a.name.cmp(&b.name));
    UpdatePlan {
        miner: miner.to_string(),
        target_version: target_version.to_string(),
        target_installed,
        latest,
        update_available: offered,
        installed: installed_versions,
        affected,
    }
}

fn managed_dir(root: &Path, miner: &str, version: &str, install: &MinerInstall) -> Option<PathBuf> {
    let dir = root.join(miner).join(version);
    // Only delete a directory this install owns. A hand-edited or legacy
    // record pointing somewhere else just loses its record.
    if Path::new(&install.path).starts_with(&dir) {
        Some(dir)
    } else {
        None
    }
}

/// Decide whether a version may be removed. This never mutates anything.
pub fn remove_plan(cfg: &Config, miner: &str, version: &str) -> RemovePlan {
    remove_plan_in(cfg, miner, version, &download::miner_root())
}

/// [`remove_plan`] with an injectable miner store root (tests, scripts).
pub fn remove_plan_in(cfg: &Config, miner: &str, version: &str, root: &Path) -> RemovePlan {
    let key = install_key(miner, version);
    let used_by = usage(cfg, miner, version);
    let install = cfg.miner_installs.get(&key);
    let allowed = used_by.is_empty() && install.is_some();
    RemovePlan {
        miner: miner.to_string(),
        version: version.to_string(),
        allowed,
        used_by,
        // Nothing is deleted while a preset still uses the version.
        delete_dir: if allowed {
            install
                .and_then(|install| managed_dir(root, miner, version, install))
                .map(|dir| dir.to_string_lossy().into_owned())
        } else {
            None
        },
        recorded_path: install
            .map(|install| install.path.clone())
            .unwrap_or_default(),
    }
}

/// Delete an unused version from the config and the managed store.
///
/// Refuses a version used by any preset (the caller must ask the user to
/// re-pin first). Only a directory the record owns under `root` is deleted;
/// a binary outside the store keeps its file and only loses the record.
pub fn delete_install(
    cfg: &mut Config,
    miner: &str,
    version: &str,
    root: &Path,
) -> anyhow::Result<RemovalReport> {
    download::validate_component(miner, "miner")?;
    download::validate_component(version, "version")?;
    let key = install_key(miner, version);
    let Some(install) = cfg.miner_installs.get(&key).cloned() else {
        anyhow::bail!(
            "{}",
            tr(
                "No install record for this miner version",
                "Для этой версии майнера нет записи об установке"
            )
        );
    };
    let users = usage(cfg, miner, version);
    if !users.is_empty() {
        let names = users
            .iter()
            .map(|preset| preset.name.clone())
            .collect::<Vec<_>>()
            .join(", ");
        anyhow::bail!(
            "{}",
            tr(
                &format!(
                    "Version {version} is still used by: {names}. Switch those presets to another installed version first."
                ),
                &format!(
                    "Версия {version} всё ещё используется: {names}. Сначала переведите эти пресеты на другую установленную версию."
                )
            )
        );
    }
    let dir = managed_dir(root, miner, version, &install);
    let mut removed_files = false;
    if let Some(dir) = &dir {
        match std::fs::symlink_metadata(dir) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                std::fs::remove_file(dir)?;
                removed_files = true;
            }
            Ok(metadata) if metadata.is_dir() => {
                std::fs::remove_dir_all(dir)?;
                removed_files = true;
            }
            Ok(_) => {
                std::fs::remove_file(dir)?;
                removed_files = true;
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }
    cfg.miner_installs.remove(&key);
    Ok(RemovalReport {
        miner: miner.to_string(),
        version: version.to_string(),
        removed_files,
        delete_dir: dir.map(|dir| dir.to_string_lossy().into_owned()),
        recorded_path: install.path,
    })
}

/// Remove a version from one config snapshot and persist the result.
///
/// The GUI used to plan against the draft while deleting from the applied
/// config, then ignore the `save()` error and show a success toast anyway. This
/// helper makes the snapshot explicit: the operation runs on a clone, the
/// caller's config is untouched unless `save` succeeds, and the persisted
/// config is returned so both the draft and the applied copy can be updated
/// from the same value.
pub fn remove_and_persist(
    snapshot: &Config,
    miner: &str,
    version: &str,
    root: &Path,
    save: impl FnOnce(&Config) -> anyhow::Result<()>,
) -> anyhow::Result<(Config, RemovalReport)> {
    let mut next = snapshot.clone();
    let report = delete_install(&mut next, miner, version, root)?;
    save(&next)?;
    Ok((next, report))
}

/// Move one preset to another **installed** version of its miner.
///
/// Returns `None` when the preset already pins that version. No other preset is
/// touched; a version whose binary is missing is refused.
pub fn pin_preset(
    cfg: &mut Config,
    preset_id: &str,
    version: &str,
) -> anyhow::Result<Option<PresetChange>> {
    download::validate_component(version, "version")?;
    let (miner, name, current) = {
        let preset = cfg
            .presets
            .get(preset_id)
            .ok_or_else(|| anyhow::anyhow!("unknown preset {preset_id}"))?;
        (
            preset.miner.clone(),
            preset.name.clone(),
            preset.miner_version.clone(),
        )
    };
    if current == version {
        return Ok(None);
    }
    let report = verify(cfg, &miner, version);
    if !report.status.is_ok() {
        anyhow::bail!("{}", report.message);
    }
    if let Some(preset) = cfg.presets.get_mut(preset_id) {
        preset.miner_version = version.to_string();
        preset.updated_at = util::now_unix();
    }
    Ok(Some(PresetChange {
        id: preset_id.to_string(),
        name,
        from: current,
        to: version.to_string(),
    }))
}

/// Move only the named presets to an installed version.
///
/// The target is checked once, then each id is pinned. Ids that do not exist
/// and presets for another miner are skipped, so the caller can pass a UI
/// selection without a second validation pass.
pub fn pin_presets(
    cfg: &mut Config,
    preset_ids: &[String],
    miner: &str,
    version: &str,
) -> anyhow::Result<Vec<PresetChange>> {
    let report = verify(cfg, miner, version);
    if !report.status.is_ok() {
        anyhow::bail!("{}", report.message);
    }
    let mut changes = Vec::new();
    for id in preset_ids {
        let matches = cfg
            .presets
            .get(id)
            .map(|preset| preset.miner == miner && preset.miner_version != version)
            .unwrap_or(false);
        if !matches {
            continue;
        }
        if let Some(preset) = cfg.presets.get_mut(id) {
            let from = std::mem::replace(&mut preset.miner_version, version.to_string());
            preset.updated_at = util::now_unix();
            changes.push(PresetChange {
                id: id.clone(),
                name: preset.name.clone(),
                from,
                to: version.to_string(),
            });
        }
    }
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{Config, MinerInstall, Preset};
    use std::os::unix::fs::PermissionsExt;

    fn temp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("trommel-versions-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn binary(root: &Path, miner: &str, version: &str, mode: u32) -> MinerInstall {
        let dir = root.join(miner).join(version);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("miner");
        std::fs::write(&path, b"#!/bin/sh\ntrue\n").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
        MinerInstall {
            miner: miner.into(),
            version: version.into(),
            path: path.to_string_lossy().into_owned(),
            sha256: String::new(),
            source_url: String::new(),
            verified: false,
            custom: true,
            installed_at: 0,
        }
    }

    fn with_second_preset(cfg: &mut Config, name: &str, version: &str) -> String {
        let mut preset = cfg.preset().cloned().unwrap_or_else(Preset::new_pearl);
        preset.id = format!("preset-{}", name.to_lowercase().replace(' ', "-"));
        preset.name = name.into();
        preset.miner_version = version.into();
        let id = preset.id.clone();
        cfg.presets.insert(id.clone(), preset);
        id
    }

    #[test]
    fn installed_lists_every_record_and_marks_a_missing_binary() {
        let root = temp_root("installed");
        let mut cfg = Config::default();
        cfg.add_install(binary(&root, "wildrig", "0.51.8", 0o755));
        let mut gone = binary(&root, "wildrig", "0.51.7", 0o755);
        std::fs::remove_file(&gone.path).unwrap();
        gone.path = root
            .join("wildrig")
            .join("0.51.7")
            .join("miner")
            .to_string_lossy()
            .into_owned();
        cfg.add_install(gone);

        let versions = installed(&cfg, "wildrig");
        assert_eq!(versions.len(), 2);
        assert_eq!(versions[0].version, "0.51.7");
        assert_eq!(versions[0].status, BinaryStatus::Missing);
        assert_eq!(versions[1].status, BinaryStatus::Ok);
        assert_eq!(
            versions[1].used_by.len(),
            1,
            "the default preset pins 0.51.8"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn usage_names_the_presets_and_marks_the_active_one() {
        let root = temp_root("usage");
        let mut cfg = Config::default();
        cfg.add_install(binary(&root, "wildrig", "0.51.8", 0o755));
        with_second_preset(&mut cfg, "Backup", "0.51.8");
        let users = usage(&cfg, "wildrig", "0.51.8");
        assert_eq!(users.len(), 2);
        assert!(users.iter().any(|user| user.active));
        assert!(users.iter().all(|user| user.miner_version == "0.51.8"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn update_plan_shows_affected_presets_and_never_rewrites() {
        let root = temp_root("plan");
        let mut cfg = Config::default();
        cfg.add_install(binary(&root, "wildrig", "0.51.8", 0o755));
        with_second_preset(&mut cfg, "Backup", "0.51.7");
        let before = cfg.clone();
        let plan = update_plan(&cfg, "wildrig", "0.51.9");
        assert!(!plan.target_installed);
        assert_eq!(plan.affected.len(), 2);
        assert_eq!(plan.installed, vec!["0.51.8".to_string()]);
        assert_eq!(cfg, before, "the plan must not touch presets");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn remove_plan_blocks_a_version_used_by_a_preset() {
        let root = temp_root("blocked");
        let mut cfg = Config::default();
        cfg.add_install(binary(&root, "wildrig", "0.51.8", 0o755));
        let plan = remove_plan_in(&cfg, "wildrig", "0.51.8", &root);
        assert!(!plan.allowed);
        assert_eq!(plan.used_by.len(), 1);
        assert_eq!(plan.delete_dir, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn remove_plan_allows_an_unused_version() {
        let root = temp_root("allowed");
        let mut cfg = Config::default();
        cfg.add_install(binary(&root, "wildrig", "0.51.7", 0o755));
        let plan = remove_plan_in(&cfg, "wildrig", "0.51.7", &root);
        assert!(plan.allowed);
        assert!(plan.delete_dir.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_install_refuses_a_used_version() {
        let root = temp_root("refuse");
        let mut cfg = Config::default();
        cfg.add_install(binary(&root, "wildrig", "0.51.8", 0o755));
        let err = delete_install(&mut cfg, "wildrig", "0.51.8", &root).unwrap_err();
        assert!(err.to_string().contains("Pearl"), "{err}");
        assert!(cfg.install_record_exists("wildrig", "0.51.8"));
        assert!(root.join("wildrig").join("0.51.8").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_install_removes_only_the_unused_version_directory() {
        let root = temp_root("delete");
        let mut cfg = Config::default();
        cfg.add_install(binary(&root, "wildrig", "0.51.8", 0o755));
        cfg.add_install(binary(&root, "wildrig", "0.51.7", 0o755));
        let report = delete_install(&mut cfg, "wildrig", "0.51.7", &root).unwrap();
        assert!(report.removed_files);
        assert!(!cfg.install_record_exists("wildrig", "0.51.7"));
        assert!(!root.join("wildrig").join("0.51.7").exists());
        assert!(root.join("wildrig").join("0.51.8").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn delete_install_keeps_a_binary_outside_the_managed_store() {
        let root = temp_root("outside-store");
        let outside = temp_root("outside-binary");
        let mut install = binary(&outside, "wildrig", "0.51.7", 0o755);
        install.path = outside
            .join("wildrig")
            .join("0.51.7")
            .join("miner")
            .to_string_lossy()
            .into_owned();
        let mut cfg = Config::default();
        cfg.add_install(install.clone());
        let report = delete_install(&mut cfg, "wildrig", "0.51.7", &root).unwrap();
        assert!(!report.removed_files);
        assert!(Path::new(&install.path).exists());
        assert!(!cfg.install_record_exists("wildrig", "0.51.7"));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn remove_and_persist_keeps_the_snapshot_when_save_fails() {
        let root = temp_root("persist-fail");
        let mut cfg = Config::default();
        cfg.add_install(binary(&root, "wildrig", "0.51.7", 0o755));
        let snapshot = cfg.clone();
        let err = remove_and_persist(&cfg, "wildrig", "0.51.7", &root, |_| {
            anyhow::bail!("disk is read-only")
        })
        .unwrap_err();
        assert!(err.to_string().contains("read-only"), "{err}");
        assert_eq!(cfg, snapshot, "the caller's snapshot must not change");
        assert!(cfg.install_record_exists("wildrig", "0.51.7"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn remove_and_persist_saves_the_same_snapshot_it_deleted_from() {
        let root = temp_root("persist-ok");
        let outside = temp_root("persist-ok-outside");
        let mut cfg = Config::default();
        // The install lives outside the managed store, so only the record is
        // removed; the returned config and the saved one must be identical.
        let mut install = binary(&outside, "wildrig", "0.51.7", 0o755);
        install.path = outside
            .join("wildrig")
            .join("0.51.7")
            .join("miner")
            .to_string_lossy()
            .into_owned();
        cfg.add_install(install);
        let mut saved = None;
        let (next, report) = remove_and_persist(&cfg, "wildrig", "0.51.7", &root, |config| {
            saved = Some(config.clone());
            Ok(())
        })
        .unwrap();
        assert!(!report.removed_files);
        let saved = saved.expect("save must be called");
        assert_eq!(saved, next, "the saved config must be the removed one");
        assert!(!next.install_record_exists("wildrig", "0.51.7"));
        assert!(cfg.install_record_exists("wildrig", "0.51.7"));
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&outside);
    }

    #[test]
    fn pin_preset_changes_only_the_chosen_preset() {
        let root = temp_root("pin");
        let mut cfg = Config::default();
        cfg.add_install(binary(&root, "wildrig", "0.51.7", 0o755));
        cfg.add_install(binary(&root, "wildrig", "0.51.8", 0o755));
        let other = with_second_preset(&mut cfg, "Backup", "0.51.8");
        let change = pin_preset(&mut cfg, "pearl-wildrig", "0.51.7")
            .unwrap()
            .expect("a change");
        assert_eq!(change.from, "0.51.8");
        assert_eq!(change.to, "0.51.7");
        assert_eq!(cfg.presets["pearl-wildrig"].miner_version, "0.51.7");
        assert_eq!(
            cfg.presets[&other].miner_version, "0.51.8",
            "other presets keep their pin"
        );
        assert!(pin_preset(&mut cfg, "pearl-wildrig", "0.51.7")
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pin_preset_requires_an_installed_executable() {
        let root = temp_root("pin-missing");
        let mut cfg = Config::default();
        cfg.add_install(binary(&root, "wildrig", "0.51.7", 0o644));
        assert_eq!(
            verify(&cfg, "wildrig", "0.51.7").status,
            BinaryStatus::NotExecutable
        );
        let err = pin_preset(&mut cfg, "pearl-wildrig", "0.51.7").unwrap_err();
        assert!(!err.to_string().is_empty());
        assert_eq!(cfg.presets["pearl-wildrig"].miner_version, "0.51.8");
        assert!(pin_preset(&mut cfg, "no-such-preset", "0.51.7").is_err());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn pin_presets_moves_only_the_named_ids() {
        let root = temp_root("pin-many");
        let mut cfg = Config::default();
        cfg.add_install(binary(&root, "wildrig", "0.51.7", 0o755));
        let other = with_second_preset(&mut cfg, "Backup", "0.51.8");
        let changes = pin_presets(
            &mut cfg,
            &["pearl-wildrig".to_string(), "missing".to_string()],
            "wildrig",
            "0.51.7",
        )
        .unwrap();
        assert_eq!(changes.len(), 1);
        assert_eq!(cfg.presets["pearl-wildrig"].miner_version, "0.51.7");
        assert_eq!(cfg.presets[&other].miner_version, "0.51.8");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn verify_never_invents_a_checksum() {
        let root = temp_root("verify");
        let mut cfg = Config::default();
        let install = binary(&root, "wildrig", "0.51.8", 0o755);
        cfg.add_install(install.clone());
        let report = verify(&cfg, "wildrig", "0.51.8");
        assert_eq!(report.status, BinaryStatus::Ok);
        assert_eq!(report.recorded_sha256, None);
        assert!(!report.sha256_verified);

        let mut with_sum = install;
        with_sum.sha256 = "abc123".into();
        with_sum.verified = true;
        cfg.add_install(with_sum);
        let report = verify(&cfg, "wildrig", "0.51.8");
        assert_eq!(report.recorded_sha256.as_deref(), Some("abc123"));
        assert!(report.sha256_verified);

        let gone = verify(&cfg, "wildrig", "9.9.9");
        assert_eq!(gone.status, BinaryStatus::NotRecorded);
        assert_eq!(gone.recorded_sha256, None);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn miner_ids_include_custom_installs() {
        let root = temp_root("ids");
        let mut cfg = Config::default();
        cfg.add_install(binary(&root, "private-build", "1.0.0", 0o755));
        let ids = miner_ids(&cfg);
        assert!(ids.contains(&"wildrig".to_string()));
        assert!(ids.contains(&"private-build".to_string()));
        let _ = std::fs::remove_dir_all(&root);
    }
}
