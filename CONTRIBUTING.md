# Contributing to Trommel

**English** · [Русский](CONTRIBUTING.ru.md)

Thanks for helping. Trommel is a small Rust/GTK project; the most useful
contributions are miner catalogue entries, packaging tests and GPU reports.

## Setup

```bash
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
  gettext librsvg2-bin desktop-file-utils appstream
cargo build -p trommel --features gui
cargo build -p trommeld
cargo test --workspace
```

If the global Cargo cache is read-only, use a project-local one:

```bash
export CARGO_HOME="$PWD/.cargo-home"
```

## Run locally

```bash
./install.sh --no-extension   # user binaries + helper + service
trommel status
trommel gui
```

For UI work, generate deterministic screenshots with a fake two-GPU rig:

```bash
./scripts/screenshots.sh
```

## What to work on

- **Miner catalogue**: add versions, descriptions, fee data, option descriptors
  and checksums in `resources/catalog/miners.toml`. See `docs/miners.md`.
- **Pools/coins**: add a pool entry and a parser adapter only when the API is
  stable and documented.
- **GPU/OC**: test on real hardware; report card model, driver and defaults.
- **GNOME extension**: test on GNOME 50, including Extension Manager enable/
  disable and re-login behavior.
- **Packaging**: `.deb` is the v1 target; RPM/AppImage/Flatpak need a working
  integration story first (see `docs/packaging.md`).

## Code rules

- Rust 2021, `cargo fmt`, no clippy warnings.
- User-facing strings use `i18n::tr("English", "Русский")`; source is English.
- No shell interpolation when spawning miners; use `Command` with argument
  vectors. `shell-words` is only for parsing the user's extra arguments.
- No bundled miner binaries, no secrets, no hardcoded wallets.
- Keep the daemon as the single source of truth; the GUI edits a draft.
- Update tests and docs with behavior changes.

## Commit and PR workflow

Use feature branches and Conventional Commits. Keep commits focused; do not mix
repository cleanup, refactors and behavior changes. A PR should include:

- what changed and why;
- test commands/results;
- hardware/desktop details for GPU or extension changes;
- screenshots for visible UI changes.

By contributing you agree that your changes can be distributed under the
project's eventual license. The extension is GPL-3.0-or-later; the app license
is still being decided.

## Reporting bugs

Include `trommel status --json`, `systemctl --user status trommeld`,
`nvidia-smi --query-gpu=name,driver_version,power.limit,power.default_limit
--format=csv`, GNOME Shell version and whether the extension is enabled.
Never paste a seed phrase or private key; a wallet address is public but you may
redact it.
