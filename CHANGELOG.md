# Changelog

All notable changes to Trommel are documented here. Format follows Keep a
Changelog; versions use SemVer.

## [0.3.0] - 2026-10-28

### Added

- Three-step quick-setup wizard (cards → miner → wallet) that saves the preset
  and starts the miner in one confirmed action; a running miner still gets the
  Apply and restart / Apply and stop dialog.
- Per-card tuning table in **Settings → Detected GPUs**; simple mode shows
  power and temperature, advanced mode adds clock locks and fan, with an
  **Advanced settings** shortcut.
- `scripts/test-mcp.sh` MCP smoke test: initialize, tools/list, tools/call,
  read-only defaults, `--allow-write` gating, rejected arguments and wallet
  redaction.

### Changed

- Wallet redaction now covers every preset (`Config::redacted`), so
  `trommel diagnose --json` and the MCP config/preset tools cannot leak a
  non-active preset's wallet.

### Fixed

- Quick-setup and GPU-tuning dialogs no longer show a duplicated window title
  bar; their header bars act as the window decoration.

## [0.2.0] - 2026-10-28

### Added

- Multi-GPU dashboard with per-card temperature, power, fan and clocks.
- Presets, global miner installs, miner metadata/fee breakdown and dynamic
  miner option descriptors.
- Per-GPU stock snapshot/restore and Apply & Restart / Apply & Stop flow.
- GNOME Shell 50 extension with shared GSettings, live popup and Quick Settings
  toggle.
- Debian packaging, developer installer, bilingual docs.
- Simple mode, quick setup entry point and a safe "recommended" GPU profile.
- Multi-GPU cards on the dashboard, per-card settings, optional core/memory
  clock locks (off by default) and copy-to-similar/all.
- Agent surface: `diagnose`, config diff/validate/apply, preset export/import
  and a read-only-by-default MCP server (`trommel mcp`).

### Changed

- Renamed the app from Pearl Miner to Trommel; removed the hardcoded wallet and
  the hardcoded 350 W stock limit.
- Extension no longer has its own prefs window; settings live in the app.

### Fixed

- Extension enable failure is no longer swallowed; install verifies state.
- External miner detection no longer confuses shell command lines with miners.
