# Third-party components

Trommel does not bundle miner binaries. The built-in catalogue links to upstream
releases; their licenses and developer fees are set by their authors.

| Component | Role | License/notes |
|---|---|---|
| WildRig Multi | GPU miner (catalogue) | Freeware; 0% on PearlHash per its docs |
| SRBMiner-MULTI | GPU miner (catalogue) | Freeware; 0.85% on PearlHash |
| OneZeroMiner | GPU miner (catalogue) | Freeware; 1% on PearlHash |
| GTK4 / libadwaita | UI | LGPL |
| zbus, tokio, serde, ureq, toml, sha2, tar, flate2, zip | Rust dependencies | See Cargo.lock and crates.io |

PearlHash and the pools listed in `resources/catalog/pools.toml` are external
services. Their fees and APIs can change; verify them before mining.
