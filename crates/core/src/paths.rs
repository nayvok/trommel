//! Filesystem locations (XDG base directories, with sane fallbacks).

use std::path::PathBuf;

/// `$XDG_CONFIG_HOME/pearl-miner`, defaulting to `~/.config/pearl-miner`.
pub fn config_dir() -> PathBuf {
    match std::env::var_os("XDG_CONFIG_HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v).join("pearl-miner"),
        _ => home().join(".config").join("pearl-miner"),
    }
}

/// `$XDG_STATE_HOME/pearl-miner`, defaulting to `~/.local/state/pearl-miner`.
pub fn state_dir() -> PathBuf {
    match std::env::var_os("XDG_STATE_HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v).join("pearl-miner"),
        _ => home().join(".local").join("state").join("pearl-miner"),
    }
}

pub fn config_path() -> PathBuf {
    config_dir().join("config.json")
}

pub fn state_path() -> PathBuf {
    state_dir().join("state.json")
}

pub fn log_path() -> PathBuf {
    state_dir().join("miner.log")
}

pub fn supervisor_log_path() -> PathBuf {
    state_dir().join("daemon.log")
}

/// Directory with A/B benchmark results (`bench-<unix-ts>.json`).
pub fn bench_dir() -> PathBuf {
    state_dir().join("bench")
}

pub fn home() -> PathBuf {
    match std::env::var_os("HOME") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from("/tmp"),
    }
}

/// Best-effort creation of all directories we write into.
pub fn ensure_dirs() -> std::io::Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(state_dir())?;
    std::fs::create_dir_all(bench_dir())?;
    Ok(())
}
