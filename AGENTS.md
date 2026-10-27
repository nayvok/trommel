# AGENTS.md — working on Trommel

Trommel is a GTK4/libadwaita manager for NVIDIA mining rigs. The daemon
(`trommeld`) owns the applied configuration, the miner process and the GPU
snapshot. The GUI keeps a draft; the extension is a thin D-Bus client.

## Repository layout

- `crates/core` — config v2, catalogue, GPU/OC, supervisor, status, D-Bus.
- `crates/daemon` — `trommeld` binary and polling/supervisor loops.
- `crates/cli` — `trommel` CLI and `gui/` (currently one `gui.rs` module).
- `resources/catalog` — built-in miners/coins/pools (TOML, no binaries).
- `data` — desktop, metainfo, icons, GSettings schema.
- `extension/` — GNOME Shell 50 extension (no `prefs.js`).
- `packaging/`, `scripts/`, `docs/`, `po/` (future gettext catalogue).

## Commands

```bash
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo build -p trommel --features gui
cargo build -p trommeld
./scripts/package-deb.sh
./scripts/screenshots.sh
./scripts/check-no-secrets.sh
```

Set `CARGO_HOME=$PWD/.cargo-home` when the global cargo cache is read-only.

## Invariants

- One miner at a time. Never spawn a second process; never kill an external
  miner without explicit user action.
- Snapshot stock GPU state **before** touching power/clocks, and restore it on
  stop. Never hardcode a stock power limit (the old `350 W` bug).
- A running miner must not be silently restarted. Edits require the explicit
  Apply and restart / Apply and stop dialog.
- Miner binaries are never committed or bundled. Downloads require HTTPS and a
  SHA-256 when the catalogue provides one; archive extraction must reject
  absolute paths and `..`.
- No personal wallets, seed phrases or private paths in tracked files. Test
  fixtures may use only `prl1test…`.
- The GUI never runs `nvidia-smi`; only the daemon/helper does.
- Miner fee is editable only when `fee_configurable=true`. Do not fake it.
- Installed miners are global (`miner_installs`), not per-preset. A preset only
  references `miner` + `miner_version`.
- The extension has no `prefs.js`; settings are shared with the app via
  `org.gnome.shell.extensions.trommel`.
- Keep English as the source string; Russian is a translation. Use
  `trommel_core::i18n::tr(en, ru)` in Rust and the `tr()` helper in GJS.

## External agents

`trommel diagnose --json`, `config diff/apply`, preset export/import and
`trommel mcp` are the intended integration surface. The MCP server is read-only
unless started with `--allow-write`; never auto-apply a proposed profile. See
`docs/agents.md`.

## Adding a miner

1. Add a `[[miners]]` entry to `resources/catalog/miners.toml`: description,
   homepage, license, supported coins/algos, fee, versions, options.
2. Add a driver branch only if core arguments differ from the generic/WildRig
   shape; otherwise catalogue options are enough.
3. Add tests for argument rendering and fee handling.
4. Do not invent URLs or checksums; leave `sha256 = ""` and `verified = false`
   until a real download was checked.

## Commits

Conventional Commits, one feature per branch, merge with `--no-ff`:

```
feat(miners): add metadata, fee model and option schemas
feat(oc): restore pre-mining power limits per GPU
feat(ui): add miner info, fee breakdown and dynamic settings
fix(extension): verify enable state after install
```

Before committing: `cargo fmt`, `cargo test`, `scripts/check-no-secrets.sh`,
and review `git diff --cached`. Never commit `local/`, `dist/`, `target/`,
miner binaries, `.mo` files or screenshots scratch directories.

## Definition of done

- Builds with and without the `gui` feature.
- Unit/e2e tests cover the changed behavior; fake `nvidia-smi` for GPU paths.
- `appstreamcli validate`, `desktop-file-validate`, `glib-compile-schemas
  --strict` pass for touched assets.
- Docs/README updated if user-visible behavior changed.
- No new warnings from `cargo clippy -D warnings`.
