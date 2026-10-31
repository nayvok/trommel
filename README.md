# Trommel

**English** · [Русский](README.ru.md)

Trommel is a GTK4/libadwaita control room for NVIDIA mining rigs. It manages
miners, pools, coins, GPU profiles and reusable presets without turning the
desktop into a pile of shell scripts.

<picture>
  <source media="(prefers-color-scheme: dark)" srcset="docs/images/dashboard-en-dark.png">
  <source media="(prefers-color-scheme: light)" srcset="docs/images/dashboard-en-light.png">
  <img src="docs/images/dashboard-en-dark.png" alt="Trommel dashboard with two NVIDIA GPUs and the rig totals" width="960">
</picture>

<sub>Dashboard with two GPUs. The image follows the system colour scheme; the
dark theme is used by default.</sub>

> Trommel is a manager, not a miner. Miner binaries are downloaded separately,
> only one miner runs at a time, and the stock power limit and clocks are
> restored when mining stops.

**Who it is for:** a home PC with one to three NVIDIA cards. It is a local,
simpler alternative to a full mining OS — not a remote farm dashboard, not an OS
replacement and not a wallet. Start with **Simple mode**; advanced miner
arguments stay hidden until you turn them on.

## Key features

- **Multi-GPU dashboard** — per-card temperature, power, fan, clocks, load and
  hashrate, with efficiency when the miner reports it; rig power, the hottest
  card and each card's series in one calm view.
- **Presets own the rig** — a preset stores the coin/algorithm, pool, wallet,
  worker, miner version, miner options and its own per-card participation,
  power and clock settings. Switching presets switches the whole GPU setup; a
  new preset starts from stock. Named **GPU presets** save and reuse per-card
  tuning templates.
- **Per-GPU participation** — switch a card off to exclude it from the active
  miner. It is not passed to the miner and is not tuned; WildRig, SRBMiner and
  OneZero use their documented device flags, and a miner without one refuses a
  partial rig instead of silently mining on every card.
- **Miner library and version manager** — short description, every installed
  version with the presets that pin it, verify/remove actions and an upstream
  link. Installs are global and versioned: a new build never rewrites a preset,
  older versions stay for rollback, and a version pinned by a preset cannot be
  removed. The developer fee is shown as a breakdown (miner + pool = total).
- **Safe by default** — snapshot each card's stock power and clocks before
  tuning and restore them on Stop. A running miner is never silently restarted:
  edits require **Apply and restart** or **Apply and stop**. Emergency-stop,
  the guard and startup recovery cover crashes and a lost session bus.
- **Hashrate watchdog** — off by default; when enabled it notifies on a
  sustained low hashrate, and an explicit opt-in can restart the miner through
  the normal bounded-restart path.
- **Local history** — the daemon keeps one sample per minute for about eight
  days and exposes it through the History dialog (1h/24h/7d), a 24h average and
  CSV export. No wallet, pool URL or telemetry is stored.
- **Tray and notifications** — a StatusNotifierItem menu with live status,
  Start/Stop, Show window and Quit, plus desktop notifications for crash, error
  and overheat events. The daemon keeps mining when the window closes.
- **Agent-ready** — JSON status and diagnostics, `config diff/apply`, preset
  export/import and an optional read-only-by-default MCP server. The agent
  proposes; you see the diff and approve.
- **Simple mode and bilingual UI** — English and Russian, light and dark colour
  schemes, and the desktop GTK theme. Simple mode hides miner arguments and
  advanced clock rows.

## Supported setup

- Linux with GNOME Shell 50 (Ubuntu 26.04 is the tested target), Wayland. The
  tray also works on KDE Plasma 6; GNOME needs the AppIndicator extension
  (Ubuntu ships it).
- An NVIDIA GPU with a working driver and `nvidia-smi`.
- For development: Rust 1.92+, `libgtk-4-dev`, `libadwaita-1-dev`, `pkg-config`.

AMD/Intel GPUs, X11-specific paths and other desktops are not supported.

## Install

Trommel is distributed from **GitHub Releases**; there is no apt repository or
PPA. Download the latest `.deb` from
<https://github.com/nayvok/trommel/releases/latest> and install it:

```bash
sudo apt install ./trommel_amd64.deb
```

Or use the installer, which downloads the latest `.deb`, verifies its SHA-256
and installs it:

```bash
curl -fsSLO https://github.com/nayvok/trommel/releases/latest/download/trommel-install.sh
bash trommel-install.sh
```

The package contains the app, `trommeld`, the polkit helper, the desktop entry,
icons and metainfo. The daemon starts on demand over D-Bus; GPU tuning works
without a password for the user who installed the package. Upgrade by installing
the new `.deb`; remove with `sudo apt remove trommel`.

The developer path builds and installs into the user session:

```bash
./install.sh             # build, install and enable the user service
./install.sh --no-helper # without the polkit helper (no OC)
```

RPM, AppImage and Flatpak are **not released**; the reasons and current status
are in [docs/development.md](docs/development.md#formats).

## First run

A fresh install has **no preset**. The dashboard shows the detected cards and
explains that a preset is required. **Create preset** opens the editor: choose
the coin, enter the pool endpoint, wallet and worker, install a miner version
and tune the cards. **Start** snapshots each enabled card, applies power/clocks
and launches exactly one miner. Stop restores the snapshot.

The full walkthrough, including pools, fees, clock locks and troubleshooting, is
in the [user guide](docs/user-guide.md).

## CLI

```bash
trommel status [--json]        # human summary or the status contract
trommel start / stop / toggle
trommel preset list / use <id>
trommel miner list / install / import / verify / remove
trommel bench --minutes 3
trommel diagnose --json > diagnostics.json
trommel config diff proposed.json
trommel config apply proposed.json --restart
trommel mcp                    # read-only MCP server over stdio
```

The daemon is the single source of truth. The GUI can be closed; the miner keeps
working.

## Documentation

- [User guide](docs/user-guide.md) — everyday operation.
- [Architecture](docs/architecture.md) — processes, modules, D-Bus, lifecycle,
  config, status contract and security model.
- [Development](docs/development.md) — build, tests, verification, screenshots,
  packaging, release and i18n.
- [Design](docs/design.md) — screens, layout, copy and visual rules.
- [Miner catalogue](docs/miners.md) — fields, options, driver branches and fees.
- [External agents](docs/agents.md) — CLI/JSON/MCP surface and safety rules.
- [Roadmap](docs/roadmap.md) — shipped, implemented on `main`, and next.
- [Decisions](docs/decisions.md) — architectural decision log.
- [Changelog](CHANGELOG.md) · [Contributing](CONTRIBUTING.md) ·
  [Security](SECURITY.md) · [Third-party components](THIRD_PARTY.md)

## License

**License is not chosen yet.** The old MIT claim was removed; the AppStream
metadata currently uses `LicenseRef-proprietary`. Miner binaries are not
distributed by this project; their licenses are listed on their upstream pages.

## Disclaimer

Mining consumes power and generates heat. You are responsible for your hardware,
electricity, pool fees, wallet security and local laws. Start with conservative
power limits and monitor the first run.
