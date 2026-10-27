//! Trommel core library.
//!
//! Shared logic for the D-Bus daemon (`trommeld`), the CLI/GUI binary
//! (`trommel`) and tests: configuration, catalog, GPU metrics, miners,
//! the supervisor and the status contract.

pub mod bench;
pub mod catalog;
pub mod config;
pub mod dbus;
pub mod download;
pub mod i18n;
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
pub const BUS_NAME: &str = "io.github.nayvok.Trommel.Daemon";
/// D-Bus object path of the daemon.
pub const OBJECT_PATH: &str = "/io/github/nayvok/Trommel";
/// Application id used by the GTK application and the desktop entry.
pub const APP_ID: &str = "io.github.nayvok.Trommel";
/// Version reported over D-Bus.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
