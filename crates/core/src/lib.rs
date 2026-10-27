//! Pearl Miner core library.
//!
//! Shared logic for the D-Bus daemon (`pearl-minerd`), the CLI/GUI binary
//! (`pearl-miner`) and tests: configuration, state, miner profiles, GPU
//! metrics, miner HTTP API clients and the status contract.

pub mod bench;
pub mod config;
pub mod dbus;
pub mod minerapi;
pub mod miners;
pub mod nvidia;
pub mod oc;
pub mod paths;
pub mod pool;
pub mod state;
pub mod status;
pub mod supervisor;
pub mod util;

/// D-Bus name used by the daemon (also the systemd/dbus-activation name).
pub const BUS_NAME: &str = "org.pearl.Miner.Daemon";
/// D-Bus object path of the daemon.
pub const OBJECT_PATH: &str = "/org/pearl/Miner";
/// Application id used by the GTK application and the desktop entry.
pub const APP_ID: &str = "org.pearl.Miner";
/// Version reported over D-Bus.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
