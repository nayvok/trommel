# Changelog

All notable changes to Trommel are documented here. Format follows Keep a
Changelog; versions use SemVer.

## [0.4.0] - 2026-10-28

### Added

- Presets are edited in a modal window instead of an in-page stack with a Back
  button: the list opens an editor with Save/Cancel, duplicate, and a
  confirmation before deleting.
- One miner install dialog for the Miners page, the preset editor and the
  quick-setup wizard. It lists catalogue versions plus imported builds, accepts
  a version name that is newer than the catalogue, downloads over HTTPS with an
  optional SHA-256 and imports local binaries.
- Appearance setting (System, Light, Dark) applied immediately.
- Colour for informational values: hashrate, mining/stopped/offline pill,
  accepted/rejected shares, pool balance, install state and warnings. Rounded
  16 px GPU cards and a hero card for the hashrate.
- UX map in `docs/ux.md` / `docs/ux.ru.md` describing every window and rule.
- Adaptive layout: the window opens at 680×760 and can shrink to 380 px,
  navigation is a single bottom bar (never duplicated in the header and never
  missing on narrow windows), GPU cards wrap into up to three columns, and page
  switches animate.

### Changed

- The Miners page uses compact activatable rows with an installed-versions
  label and a chevron instead of two large pill buttons per row.
- Quick setup starts each card from its own override (or driver auto) instead
  of the default profile, so a 300 W default is not silently applied to a
  165 W card.
- "Fill" is now **Apply 80 %**: it fills ticked cards, expands them and writes
  the applied watts into each row, so the action has a visible result.
- The rounded action buttons were made compact and consistent, and the old
  paired pill buttons are gone.

### Fixed

- Deleting a preset asks for confirmation and reports which preset the daemon
  keeps using.
- Config schema gains optional `appearance` and `theme` fields; older configs
  load with safe defaults.

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
- `trommel mcp` no longer panicked with "cannot start a runtime from within a
  runtime" when a tool needed D-Bus (`trommel_status`, `trommel_diagnose`,
  start/stop); the stdio loop now runs outside the tokio runtime.

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
