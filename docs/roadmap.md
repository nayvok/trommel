# Roadmap

Current release: **0.4.3** (28.10.2026). Shipped behaviour is documented in
[user-guide.md](user-guide.md) and [architecture.md](architecture.md); this page
only describes direction. Anything not shipped is explicitly in the future
sections — the README and guides do not describe it.

## Shipped in 0.4.x

- Multi-GPU dashboard, presets, global miner installs, miner metadata and fee
  breakdown, catalogue-driven miner options.
- Per-GPU stock snapshot and restore with an explicit apply/restart/stop flow,
  rollback on failure, external-miner detection and daemon-restart adoption.
- Simple mode by default, three-step Quick setup, per-card tuning table,
  recommended 80 % profile, optional clock locks and copy-to-similar/all.
- Modal preset editor, one shared miner install dialog (catalogue, custom
  version, HTTPS download, local import), adaptive 400 px window.
- GNOME Shell 50 extension with shared GSettings, live popup and Quick Settings
  toggle; no `prefs.js`.
- Agent surface: `diagnose`, config validate/export/diff/apply, preset
  export/import, read-only-by-default MCP server.
- GitHub Releases (`deb` + checksum + installer), release workflow, e2e, UI
  smoke, first-run smoke, MCP smoke, screenshots.
- EN/RU user docs, ADRs, this documentation set.

## Next (0.5 candidates, not scheduled)

Ordered by expected value, not commitment:

1. **Download dialog with progress and cancel** in the GUI. The core downloader
   is synchronous and has no progress events; this needs a streaming API and a
   cancellable worker before it can be exposed.
2. **Custom pool adapters.** Pools beyond PearlHash currently show configured
   data only. Add an adapter only for a stable, documented API.
3. **Per-GPU miner device selection** for miners/drivers that support it,
   including catalogue capabilities without pretending unsupported flags work.
4. **Gettext migration** with `po/ru.po` while keeping English source strings
   ([i18n.md](i18n.md)).
5. **RPM spec validation** on Fedora/openSUSE; the spec template exists but is
   untested and not released.
6. **A signed remote catalogue** with a verification story (signature, key
   rotation, offline fallback).

## Later

- AMD/Intel backends. They need their own query and OC abstraction; the current
  supervisor and helper are NVIDIA-specific by design.
- A portable tarball with an explicit host-helper installer (preferred over an
  AppImage for this architecture).
- Flatpak only with a proper host bridge/portal design; a GUI-only Flatpak would
  be a different product.
- Translation platform and additional locales.

## Non-goals

- Remote farm orchestration, multi-rig dashboards, telemetry.
- Bundling miner binaries, wallets, seed phrases or a built-in wallet.
- Applying a profile without explicit user confirmation.
- A second miner process or “tuning without restart” for a running miner.
- Shipping an RPM/AppImage/Flatpak artifact without a tested integration story.

## Decision log

Architecture decisions are recorded as ADRs:
[name](adr/0001-name.md), [extension packaging](adr/0002-extension-packaging.md),
[packaging](adr/0003-packaging.md). Add a new ADR rather than rewriting an old
one; a superseding decision gets its own number and an explicit “supersedes”
note.
