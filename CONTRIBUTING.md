# Contributing to Trommel

Thanks for helping. Trommel is a small Rust/GTK project; the most useful
contributions are miner catalogue entries, packaging tests, GPU reports and
documentation fixes. Start with the [documentation index](docs/README.md), then
pick the matching guide:

- code and tests — [docs/development.md](docs/development.md) and
  [docs/architecture.md](docs/architecture.md#modules);
- screens and copy — [docs/design.md](docs/design.md);
- catalogue — [docs/miners.md](docs/miners.md);
- packaging — [docs/development.md](docs/development.md#packaging-and-releases).

## Setup

Rust 1.92+ is required (the declared MSRV, checked in CI with the `gui`
feature).

```bash
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
  gettext librsvg2-bin desktop-file-utils appstream \
  gir1.2-atspi-2.0 dbus xdg-utils at-spi2-core xvfb x11-utils
cargo build -p trommel --features gui
cargo build -p trommel          # CLI-only build must keep working
cargo build -p trommeld
cargo test --workspace
cargo test -p trommel --features gui
```

If the global Cargo cache is read-only, use a project-local one:

```bash
export CARGO_HOME="$PWD/.cargo-home"
```

## Run locally

```bash
./install.sh                 # user binaries + helper + service
trommel status
trommel gui
```

For UI work, use the isolated harnesses instead of a real rig:

```bash
./scripts/e2e.sh              # CLI lifecycle, fake GPU/helper/miner
./scripts/ui-smoke.sh         # AT-SPI click-through (desktop session)
xvfb-run -a ./scripts/ui-smoke.sh  # same, headless (what CI runs)
./scripts/first-run-smoke.sh  # pristine-install path
./scripts/screenshots.sh      # deterministic docs screenshots
```

They create their own XDG directories, a private D-Bus and a fake `nvidia-smi`;
see [docs/development.md](docs/development.md#test-harness-isolation). Do not
weaken that isolation.

## What to work on

- **Miner catalogue**: add versions, descriptions, fee data, option descriptors
  and checksums in `resources/catalog/miners.toml`. See
  [docs/miners.md](docs/miners.md).
- **Pools/coins**: add a pool entry and a parser adapter only when the API is
  stable and documented.
- **GPU/OC**: test on real hardware; report the card model, driver, stock limits
  and what the helper log showed.
- **Packaging**: `.deb` is the released target; RPM/AppImage/Flatpak need a
  working integration story first (see
  [docs/development.md](docs/development.md#formats)).
- **Docs**: if a document and the code disagree, fix the document in the same
  change. Run `./scripts/check-docs.sh`.

## Code rules

- Rust 2021, `cargo fmt`, no clippy warnings with
  `--features trommel/gui`.
- User-facing strings use `i18n::tr("English", "Русский")`; English is the
  source. Do not concatenate translated fragments.
- No shell interpolation when spawning miners; use `Command` with argument
  vectors. `shell-words` is only for parsing the user's extra arguments.
- No bundled miner binaries, no secrets, no hardcoded wallets.
- Keep the daemon as the single source of truth; the GUI edits a draft.
- Update tests, the matching document and, for visible changes, screenshots.
- Keep `trommel-core` free of GTK and of daemon-only policy.

## Commit and PR workflow

Use feature branches and Conventional Commits. Keep commits focused; do not mix
repository cleanup, refactors and behaviour changes. A PR should include:

- what changed and why;
- test commands/results (unit, e2e, MCP, UI smoke as applicable);
- hardware/desktop details for GPU changes;
- screenshots for visible UI changes;
- the documentation update that goes with it.

Before pushing:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --features trommel/gui -- -D warnings
cargo test --workspace
cargo test -p trommel --features gui
cargo build -p trommel
./scripts/check-no-secrets.sh
./scripts/check-docs.sh
```

By contributing you agree that your changes can be distributed under the
project's eventual license; the app license is still being decided.

## Reporting bugs

Include `trommel status --json`, `trommel diagnose --json`,
`systemctl --user status trommeld`, the daemon journal,
`nvidia-smi --query-gpu=name,driver_version,power.limit,power.default_limit
--format=csv` and the GNOME Shell version.
Never paste a seed phrase or private key; a wallet address is public but you may
redact it.
