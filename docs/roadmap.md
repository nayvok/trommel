# Roadmap

## v0.2 (current)

- Multi-GPU dashboard, presets, global miner installs, miner metadata/fees,
  dynamic miner options.
- Per-GPU stock snapshot/restore and apply/restart/stop flow.
- GNOME 50 extension with shared settings and Quick Settings toggle.
- Simple mode, a three-step quick-setup wizard, per-card tuning table and a
  safe "recommended" GPU profile.
- External agent surface: diagnostics, config diff/apply, preset export/import
  and a read-only MCP server (`trommel mcp`; `--allow-write` opt-in).
- `.deb`, developer install, EN/RU docs.

## v0.3

- Full download dialog in the GUI with progress/cancel (core downloader exists).
- Optional built-in assistant (user-provided OpenAI-compatible key) that returns
  a proposed profile for preview; never auto-applies.
- More coins/pools and a remote, signed catalogue.
- Per-GPU miner device selection for drivers that support it.
- RPM spec and container test; AppImage research.

## Later

- AMD/Intel backends.
- Portable tarball/AppImage with an explicit host-helper installer.
- Flatpak only with a proper host bridge/portal design.
- Translation platform and more locales.
