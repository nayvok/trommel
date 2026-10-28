//! Safe miner downloads and archive extraction.
//!
//! Binaries are not bundled with Trommel. A download is accepted only over
//! HTTPS, optionally verified with SHA-256, extracted without path traversal,
//! and moved into the per-user miner store atomically.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::config::{install_key, MinerInstall};

const MAX_BYTES: u64 = 512 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct DownloadRequest {
    pub miner: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub archive: String,
    pub binary: String,
    pub source_url: String,
}

pub fn miner_root() -> PathBuf {
    crate::paths::data_dir().join("miners")
}

pub fn install_dir(miner: &str, version: &str) -> PathBuf {
    miner_root().join(miner).join(version)
}

/// Download and install a miner. `root` is the miners directory (injectable for
/// tests); pass `miner_root()` for normal use.
pub fn install(req: &DownloadRequest, root: &Path) -> anyhow::Result<MinerInstall> {
    if !req.url.starts_with("https://") {
        anyhow::bail!("only HTTPS download URLs are accepted");
    }
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(std::time::Duration::from_secs(10))
        .timeout_read(std::time::Duration::from_secs(120))
        .timeout(std::time::Duration::from_secs(180))
        .build();
    let response = agent
        .get(&req.url)
        .set("Accept", "application/octet-stream")
        .call()
        .map_err(|e| anyhow::anyhow!("download failed: {e}"))?;
    let mut bytes = Vec::new();
    response
        .into_reader()
        .take(MAX_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| anyhow::anyhow!("cannot read download: {e}"))?;
    if bytes.len() as u64 > MAX_BYTES {
        anyhow::bail!("download is larger than 512 MiB");
    }
    if !req.sha256.trim().is_empty() {
        let actual = hex_digest(&bytes);
        if !actual.eq_ignore_ascii_case(req.sha256.trim()) {
            anyhow::bail!("SHA-256 mismatch: expected {}, got {actual}", req.sha256);
        }
    }

    let staging = std::env::temp_dir().join(format!(
        "trommel-{}-{}-{}",
        req.miner,
        req.version.replace(['/', '\\'], "_"),
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let result = (|| -> anyhow::Result<PathBuf> {
        let binary = match req.archive.as_str() {
            "tar.gz" | "tgz" => extract_tar_gz(&bytes, &staging, &req.binary)?,
            "zip" => extract_zip(&bytes, &staging, &req.binary)?,
            _ => {
                let path = staging.join(if req.binary.is_empty() {
                    "miner"
                } else {
                    &req.binary
                });
                std::fs::write(&path, &bytes)?;
                path
            }
        };
        check_executable(&binary)?;
        let final_dir = root.join(&req.miner).join(&req.version);
        std::fs::create_dir_all(&final_dir)?;
        let final_path = final_dir.join(binary.file_name().unwrap_or_default());
        move_file(&binary, &final_path)?;
        make_executable(&final_path)?;
        Ok(final_path)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    let path = result?;
    Ok(MinerInstall {
        miner: req.miner.clone(),
        version: req.version.clone(),
        path: path.to_string_lossy().into_owned(),
        sha256: hex_digest(&bytes),
        source_url: if req.source_url.is_empty() {
            req.url.clone()
        } else {
            req.source_url.clone()
        },
        verified: !req.sha256.trim().is_empty(),
        custom: false,
        installed_at: crate::util::now_unix(),
    })
}

pub fn install_key_for(install: &MinerInstall) -> String {
    install_key(&install.miner, &install.version)
}

fn hex_digest(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Linux executables we accept: ELF, or an interpreter script with `#!`.
/// This rejects the HTML page of a GitHub release URL, which used to be
/// installed as a "binary" and only failed later at spawn time.
fn executable_format(head: &[u8]) -> bool {
    head.starts_with(b"\x7fELF") || head.starts_with(b"#!")
}

fn check_executable(path: &Path) -> anyhow::Result<()> {
    let mut file = std::fs::File::open(path)?;
    let mut head = [0u8; 4];
    let read = file.read(&mut head)?;
    if !executable_format(&head[..read]) {
        anyhow::bail!(
            "downloaded file is not a Linux executable ({}); is the URL a release page instead of a file?",
            path.file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("binary")
        );
    }
    Ok(())
}

/// Copy a local binary into the managed miner store.
///
/// Imported builds become independent of the original path: the user can move
/// or delete the download, and the config still points at a stable file.
pub fn import_binary(
    miner: &str,
    version: &str,
    source: &Path,
    root: &Path,
) -> anyhow::Result<MinerInstall> {
    let source = std::fs::canonicalize(source)
        .map_err(|e| anyhow::anyhow!("cannot resolve {}: {e}", source.display()))?;
    if !source.is_file() {
        anyhow::bail!("not a file: {}", source.display());
    }
    let bytes = std::fs::read(&source)?;
    if bytes.len() as u64 > MAX_BYTES {
        anyhow::bail!("miner binary is larger than 512 MiB");
    }
    if !executable_format(&bytes[..bytes.len().min(4)]) {
        anyhow::bail!("{} is not a Linux executable or script", source.display());
    }
    let name = source
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("miner")
        .to_string();
    let final_dir = root.join(miner).join(version);
    std::fs::create_dir_all(&final_dir)?;
    let final_path = final_dir.join(&name);
    if final_path != source {
        let staging =
            std::env::temp_dir().join(format!("trommel-import-{}-{}", miner, std::process::id()));
        let _ = std::fs::remove_dir_all(&staging);
        std::fs::create_dir_all(&staging)?;
        let temp = staging.join(&name);
        std::fs::write(&temp, &bytes)?;
        make_executable(&temp)?;
        move_file(&temp, &final_path)?;
        let _ = std::fs::remove_dir_all(&staging);
    } else {
        make_executable(&final_path)?;
    }
    Ok(MinerInstall {
        miner: miner.to_string(),
        version: version.to_string(),
        path: final_path.to_string_lossy().into_owned(),
        sha256: hex_digest(&bytes),
        source_url: String::new(),
        verified: false,
        custom: true,
        installed_at: crate::util::now_unix(),
    })
}

fn safe_name(name: &str) -> bool {
    let path = Path::new(name);
    !path.is_absolute()
        && !name.contains('\0')
        && path
            .components()
            .all(|component| !matches!(component, std::path::Component::ParentDir))
}

fn extract_tar_gz(bytes: &[u8], dest: &Path, binary: &str) -> anyhow::Result<PathBuf> {
    let decoder = flate2::read::GzDecoder::new(bytes);
    let mut archive = tar::Archive::new(decoder);
    let mut found: Option<PathBuf> = None;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.to_string_lossy().into_owned();
        if !safe_name(&path) {
            anyhow::bail!("unsafe archive path: {path}");
        }
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let out = dest.join(&path);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut file)?;
        if !binary.is_empty()
            && Path::new(&path)
                .file_name()
                .map(|name| name == binary)
                .unwrap_or(false)
        {
            found = Some(out);
        }
    }
    found.ok_or_else(|| anyhow::anyhow!("binary {binary:?} not found in archive"))
}

fn extract_zip(bytes: &[u8], dest: &Path, binary: &str) -> anyhow::Result<PathBuf> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive = zip::ZipArchive::new(cursor)?;
    let mut found: Option<PathBuf> = None;
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index)?;
        let name = entry.name().to_string();
        if !safe_name(&name) {
            anyhow::bail!("unsafe archive path: {name}");
        }
        if entry.is_dir() {
            continue;
        }
        let out = dest.join(&name);
        if let Some(parent) = out.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut file = std::fs::File::create(&out)?;
        std::io::copy(&mut entry, &mut file)?;
        if !binary.is_empty()
            && Path::new(&name)
                .file_name()
                .map(|n| n == binary)
                .unwrap_or(false)
        {
            found = Some(out);
        }
    }
    found.ok_or_else(|| anyhow::anyhow!("binary {binary:?} not found in archive"))
}

/// Move a staged file into the miner store. `/tmp` is often tmpfs, so a plain
/// rename fails with EXDEV across filesystems; fall back to copy+remove.
fn move_file(from: &Path, to: &Path) -> anyhow::Result<()> {
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    std::fs::copy(from, to)?;
    std::fs::remove_file(from)?;
    Ok(())
}

fn make_executable(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = std::fs::metadata(path)?.permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(path, permissions)?;
    Ok(())
}

/// Test helper: write a binary without network access.
pub fn install_from_bytes(
    req: &DownloadRequest,
    bytes: &[u8],
    root: &Path,
) -> anyhow::Result<MinerInstall> {
    let staging =
        std::env::temp_dir().join(format!("trommel-test-{}-{}", req.miner, std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)?;
    let path = staging.join(&req.binary);
    std::fs::write(&path, bytes)?;
    make_executable(&path)?;
    let final_dir = root.join(&req.miner).join(&req.version);
    std::fs::create_dir_all(&final_dir)?;
    let final_path = final_dir.join(&req.binary);
    move_file(&path, &final_path)?;
    let _ = std::fs::remove_dir_all(&staging);
    Ok(MinerInstall {
        miner: req.miner.clone(),
        version: req.version.clone(),
        path: final_path.to_string_lossy().into_owned(),
        sha256: hex_digest(bytes),
        source_url: req.url.clone(),
        verified: !req.sha256.trim().is_empty(),
        custom: false,
        installed_at: crate::util::now_unix(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_plain_http() {
        let req = DownloadRequest {
            miner: "x".into(),
            version: "1".into(),
            url: "http://example.com/x".into(),
            sha256: String::new(),
            archive: "binary".into(),
            binary: "x".into(),
            source_url: String::new(),
        };
        assert!(install(&req, Path::new("/tmp/trommel-test")).is_err());
    }

    #[test]
    fn rejects_html_binaries() {
        let root = std::env::temp_dir().join(format!("trommel-html-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("page");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&src, b"<!doctype html><html>release</html>").unwrap();
        let err = import_binary("wildrig", "1", &src, &root).unwrap_err();
        assert!(err.to_string().contains("not a Linux executable"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn imports_elf_and_script_and_copies() {
        let root = std::env::temp_dir().join(format!("trommel-import-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let src = root.join("wildrig-multi");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(&src, b"\x7fELFfake").unwrap();
        let install = import_binary("wildrig", "0.51.8", &src, &root).unwrap();
        assert!(Path::new(&install.path).exists());
        assert_ne!(install.path, src.to_string_lossy());
        assert!(install.custom);
        assert!(executable_format(b"#!/bin/bash\n"));

        let script = root.join("fake-miner");
        std::fs::write(&script, b"#!/bin/sh\nexec sleep 1\n").unwrap();
        assert!(import_binary("wildrig", "custom", &script, &root).is_ok());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn installs_binary_and_hashes() {
        let root = std::env::temp_dir().join(format!("trommel-dl-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let req = DownloadRequest {
            miner: "test".into(),
            version: "1.0".into(),
            url: "https://example.com/test".into(),
            sha256: hex_digest(b"hello"),
            archive: "binary".into(),
            binary: "test-miner".into(),
            source_url: String::new(),
        };
        let install = install_from_bytes(&req, b"hello", &root).unwrap();
        assert!(Path::new(&install.path).exists());
        assert!(install.verified);
        assert_eq!(install.sha256, hex_digest(b"hello"));
        let _ = std::fs::remove_dir_all(&root);
    }
}
