# Changelog

All notable changes to Trommel are documented here. Format follows Keep a
Changelog; versions use SemVer.

## [Unreleased]

## [0.2.0] - 2026-10-28

### Added

- Multi-GPU dashboard with per-card temperature, power, fan and clocks.
- Presets, global miner installs, miner metadata/fee breakdown and dynamic
  miner option descriptors.
- Per-GPU stock snapshot/restore and Apply & Restart / Apply & Stop flow.
- GNOME Shell 50 extension with shared GSettings, live popup and Quick Settings
  toggle.
- Debian packaging, developer installer, bilingual docs.

### Changed

- Renamed the app from Pearl Miner to Trommel; removed the hardcoded wallet and
  the hardcoded 350 W stock limit.
- Extension no longer has its own prefs window; settings live in the app.

### Fixed

- Extension enable failure is no longer swallowed; install verifies state.
- External miner detection no longer confuses shell command lines with miners.
