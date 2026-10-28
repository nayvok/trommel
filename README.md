# Trommel

**English** · [Русский](README.ru.md)

Trommel is a GTK4/libadwaita control room for NVIDIA mining rigs. It manages
miners, pools, coins, GPU profiles and reusable presets — without turning your
desktop into a pile of shell scripts.

![Dashboard](docs/images/dashboard-en-dark.png)

> Trommel is a manager, not a miner. Miner binaries are downloaded separately,
> only one miner runs at a time, and the app restores the stock power limit and
> clocks when mining stops.

**Who it is for:** a home PC with one to three NVIDIA cards. It is a local,
simpler alternative to a full mining OS — not a remote farm dashboard, not an
OS replacement, and not a wallet. Start with **Simple mode**; advanced miner
arguments are hidden until you turn them on.

## Features

- **Multi-GPU dashboard** — per-card temperature, power, fan, clocks and
  utilization; total power, temperature and efficiency for the whole rig.
- **Presets** — bundle coin, pool, wallet, worker, miner version, GPU profile
  and miner-specific options; switch without re-entering everything.
- **Miner catalogue** — short description, version, source, license, developer
  fee and supported algorithms. The fee is shown as a breakdown
  (miner + pool = total) everywhere.
- **Miner-dependent settings** — option descriptors come from the catalogue:
  types, ranges, defaults and CLI flags are chosen for the selected miner
  version. "Reset to miner defaults" is always available.
- **One install, all presets** — a single install dialog owns the version
  list (catalogue plus your own imported builds), downloads over HTTPS and
  imports local binaries. Install a version once; every preset that references
  it shows `Installed`, and missing versions block Start until resolved.
- **Preset editor as a dialog** — rows open a modal editor with Save/Cancel,
  duplicate and a confirmation before deleting; no in-page Back navigation.
- **Appearance** — System, Light or Dark colour scheme applied immediately and
  remembered across restarts. Trommel uses libadwaita's own styling and the
  system GTK theme, so it matches other GNOME apps on the same desktop.
- **Stock snapshot** — before applying power/clocks, Trommel captures each
  GPU's current power limit and restores it on stop. No hardcoded `-pl 350`.
- **Optional clock locks** — core/memory locks are off by default; enable them
  per card, copy to identical cards or all GPUs. A lower memory lock can save
  power on some coins and hurt others, so it is an advanced choice.
- **Per-card tuning table** — **Settings → Detected GPUs** lists power, clock
  locks, fan and temperature for every card; simple mode shows only power and
  temperature, and **Advanced settings** reveals the rest.
- **Safe apply flow** — if the miner is running and you edit the profile, you
  choose **Apply and restart** or **Apply and stop**; failures roll back.
- **GNOME Shell integration** — a panel indicator, live popup, Quick Settings
  toggle and optional labels. The app can enable it and reflects the state from
  Extension Manager.
- **Simple mode by default** — ordinary users see power, temperature and Start;
  advanced miner options and raw arguments are one switch away.
- **Agent-friendly** — JSON config, CLI diagnostics, diff/apply commands and an
  optional MCP server for Codex/other agents. The agent proposes; you see the
  diff and approve.
- **English and Russian UI**, the desktop GTK theme (Yaru, Adwaita), light and dark colour schemes, GNOME HIG-style widgets.

## Requirements (v1)

- Linux with GNOME Shell 50 (Ubuntu 26.04 is the tested target), Wayland.
- NVIDIA GPU + working `nvidia-smi` and driver.
- For development: Rust 1.80+, `libgtk-4-dev`, `libadwaita-1-dev`, `pkg-config`.

AMD/Intel, X11-specific paths and non-GNOME desktops are out of scope for v1.

## Install

### GitHub release (recommended)

Trommel is distributed from **GitHub Releases**; there is no apt repository or
PPA. Download the latest release from
<https://github.com/nayvok/trommel/releases/latest> and install the `.deb`:

```bash
sudo apt install ./trommel_amd64.deb
```

Or use the installer, which downloads the latest `.deb`, verifies its SHA-256
and installs it:

```bash
curl -fsSLO https://github.com/nayvok/trommel/releases/latest/download/trommel-install.sh
bash trommel-install.sh
```

The package contains the app, `trommeld`, the polkit helper, desktop entry,
icons, metainfo, GSettings schema and the GNOME extension. The daemon starts on
demand over D-Bus; GPU tuning works without a password for the user who ran the
install. The extension is enabled from **Settings → GNOME integration** (or
Extension Manager) after a re-login on Wayland.

Upgrade by installing the new release's `.deb`; remove with
`sudo apt remove trommel`. The dev install below is for contributors.

### Developer install

```bash
./install.sh                 # build, install, enable service + extension
./install.sh --no-extension  # without the panel applet
./install.sh --no-helper     # without the polkit helper (no OC)
```

The script installs binaries to `~/.local/bin`, the helper to
`/usr/libexec/trommel/`, and the extension to the user extension directory. It
asks for `sudo` once for the helper and polkit rule.

### Other formats

RPM, AppImage and Flatpak are **not released in v1**. Reasons and the current
status are documented in [docs/packaging.md](docs/packaging.md). Flatpak cannot
provide the host `nvidia-smi`/polkit/systemd/extension integration this app
needs without a broad sandbox; a GUI-only Flatpak would be a different product.

## First run

A **Quick setup** button appears on the dashboard until a wallet and an installed
miner are ready. It opens a three-step wizard for a 1–3 card rig:

1. **Cards** — tick the cards to mine on and give each one a power limit
   (**Apply 80 %** fills the ticked cards from their stock limit, expands them
   and shows the applied watts; **Use auto** = no limit).
2. **Miner** — choose a miner and version, then **Install…**: the dialog can
   download a catalogue version, take a version name that is newer than the
   catalogue, or import a local binary. One installation serves every preset.
3. **Wallet** — preset name, wallet, worker, pool and pool address. Nothing is
   sent to the daemon or started until you press **Start mining**; a running
   miner gets the usual Apply and restart / Apply and stop dialog instead of a
   silent restart.

![Quick setup](docs/images/setup-en-dark.png)

Presets open in a modal editor, and the same install dialog serves the Miners
page, the editor and the wizard:

![Preset editor](docs/images/preset-editor-en-dark.png)

![Miner installation](docs/images/miner-install-en-dark.png)

The classic path still works:

1. Open **Trommel** from the application menu.
2. Open a preset row: the editor is a modal window with coin → pool →
   miner/version (install or import a binary) → wallet/worker → GPU profile →
   miner options.
3. Choose **Save** (it saves and applies). If the miner is stopped, it is saved immediately;
   if it is running, you get the restart/stop dialog.
4. Press **Start**. Trommel snapshots the stock power limit and applies the
   selected profile before spawning the miner.
5. Press **Stop** to terminate the miner and restore the snapshot.

The app never ships a wallet or a miner binary. Existing Pearl Miner configs are
imported automatically from `~/.config/pearl-miner/config.json` on first load.

## CLI

```bash
trommel status               # human-readable
trommel status --json        # contract used by the GUI/extension
trommel start / stop / toggle
trommel preset list          # show presets and their total fee
trommel preset use <id> --restart
trommel miner list           # catalogue + installed versions
trommel miner import wildrig 0.51.8 /path/to/wildrig-multi
trommel miner install wildrig 0.51.8 https://example.org/miner.tar.gz \
  --sha256 <hex> --archive tar.gz --binary wildrig-multi
trommel bench --minutes 3
trommel diagnose --json > diagnostics.json
trommel config diff proposed.json
trommel config apply proposed.json --restart
trommel mcp                  # read-only MCP server over stdio
```

The daemon is the single source of truth. The GUI can be closed; the panel
indicator and miner keep working.

## Fees and miner info

Each miner page shows a short description, version, source, license, developer
fee, supported coins/algorithms and installed versions. The preset editor shows
the same data and a fee breakdown:

```
miner 0.00% + pool 1.00% = total 1.00%
```

A miner fee is editable **only** when the catalogue marks it configurable; the
UI never pretends a fixed fee can be changed. The pool fee is used for effective
hashrate calculations and is editable for custom pools.

## Stock restore and limitations

- Power limit is restored from the snapshot captured before mining.
- `-lgc`/`-lmc` are reset to the driver default. A pre-existing manual clock
  lock cannot be read back through `nvidia-smi`; this is documented in the UI.
- Fan control on consumer cards is not available through `nvidia-smi`; fan
  flags are passed to the miner when supported and disappear with the process.
- On reboot the snapshot is considered stale and cleared; the driver resets
  GPU settings anyway.

## GNOME extension

The extension is shipped with the package and is listed in Extension Manager.
It has no separate preferences window: panel label mode, live activity and the
Quick Settings toggle are configured in **Trommel → Settings → GNOME
integration**, using a shared GSettings schema. If Extension Manager disables
it, the app shows it as disabled. The daemon works without the extension.

## Documentation

- [User guide](docs/user-guide.md) · [Русский](docs/user-guide.ru.md)
- [Architecture](docs/architecture.md)
- [Miner catalogue and options](docs/miners.md)
- [Using Trommel from an external agent](docs/agents.md)
- [Packaging status](docs/packaging.md)
- [GNOME extension](docs/extension.md)
- [Security model](docs/security.md)
- [Roadmap](docs/roadmap.md)

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) / [Русский](CONTRIBUTING.ru.md) and
[AGENTS.md](AGENTS.md). Feature branches and Conventional Commits are expected.

## License

**License is not chosen yet.** The old MIT claim was removed; the AppStream
metadata currently uses `LicenseRef-proprietary`. The GNOME Shell extension is
`GPL-3.0-or-later` because it derives from GNOME Shell. Mining binaries are not
distributed by this project; their licenses are listed on their upstream pages.

## Disclaimer

Mining consumes power and generates heat. You are responsible for your hardware,
electricity, pool fees, wallet security and local laws. Start with conservative
power limits and monitor the first run.
