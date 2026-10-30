# Roadmap

Current release: **0.5.0** (29.10.2026). Shipped behaviour is documented in
[user-guide.md](user-guide.md) and [architecture.md](architecture.md); this page
only describes direction. Anything not shipped is explicitly in the future
sections — the README and guides do not describe it.

## Block plan

The 0.5 work is split into blocks. A block is one branch, one scope freeze and
one verified release; later blocks are not started until the previous one is
green. Status is updated in the same commit as the code.

| Block | Scope | Status |
| --- | --- | --- |
| **B0** | Documentation baseline: one active session, failure policy, block roadmap, schema v3 references | **Done in 0.4.4** |
| **B1** | Fault-tolerant session ownership: emergency-stop, OC file lock, helper lease, PID-by-path, startup recovery, systemd/bus shutdown (**B1a**) and guard, bounded restarts, benchmark guard (**B1b**) | **Done in 0.4.5** (B1a 0.4.4, B1b 0.4.5) |
| **B2** | Per-GPU participation: exclude a card from the active miner without a second process, catalogue capabilities for device selection | **Done in 0.5.0** |
| **B3** | Merge the Presets and Miners pages into one preset-centred flow (miner library dialog) | Not started |
| **B4** | Miner version manager: install/update/remove with the affected presets shown, rollback | Not started |
| **B5** | Release hardening after B1–B4 (packaging/test matrix review; scope defined when B4 closes) | Not started |

B3, B4 and B5 are not started; B2 is complete in 0.5.0. Multi-miner/multi-job
remains a separate product decision under
[ADR 0005](adr/0005-one-active-session.md).

## Shipped in 0.5.x

- **Per-GPU participation (B2).** `GpuTuning.enabled` now excludes a card from
  the active miner, not only from tuning. Only a documented miner flag is used
  (WildRig `--gpu-list`, SRBMiner `--gpu-id`, OneZero `--devices`); a miner
  without one refuses a partial rig instead of mining on every card. The status
  exposes `participation[]`, the dashboard marks each card mining/excluded and
  Totals counts them. Stop/shutdown/emergency-stop and the snapshot cover only
  the participating cards. See [ADR 0007](adr/0007-per-gpu-participation.md).

## Shipped in 0.4.x

- Multi-GPU dashboard, presets, global miner installs, miner metadata and fee
  breakdown, catalogue-driven miner options.
- Per-GPU stock snapshot and restore with an explicit apply/restart/stop flow,
  rollback on failure, external-miner detection and crash-safe session
  ownership.
- Fault-tolerant ownership (B1): `trommel emergency-stop`, a cross-process OC
  lock, helper PID/start-time leases, `state.miner_path` and PID-by-path
  matching, startup recovery, systemd `ExecStopPost`, session-bus monitoring,
  the guard process, bounded restart with exponential backoff and the benchmark
  external-miner guard. See [ADR 0006](adr/0006-failure-policy.md).
- Simple mode by default, three-step Quick setup, per-card tuning table,
  recommended 80 % profile, optional clock locks and copy-to-similar/all.
- Modal preset editor, one shared miner install dialog (catalogue, custom
  version, HTTPS download, local import), adaptive 400 px window.
- GNOME Shell 50 extension with shared GSettings, live popup and Quick Settings
  toggle; no `prefs.js`. It has since been removed from the tree — desktop
  notifications now come from the daemon, see
  [ADR 0004](adr/0004-remove-extension.md).
- Agent surface: `diagnose`, config validate/export/diff/apply, preset
  export/import, read-only-by-default MCP server.
- GitHub Releases (`deb` + checksum + installer), release workflow, e2e, UI
  smoke, first-run smoke, MCP smoke, screenshots.
- EN/RU user docs, ADRs, this documentation set.

## Next (0.6 candidates, not scheduled)

Ordered by expected value, not commitment:

1. **B4 — miner version manager.** Install/update/remove several versions with
   a clear preset-impact view and no silent preset rewrite.
2. **B3 — preset-centred navigation.** Replace the Miners page with a miner
   library dialog opened from the presets flow.
3. **Download dialog with progress and cancel.** The core downloader is
   synchronous and has no progress events; this needs a streaming API and a
   cancellable worker before it can be exposed.
4. **Custom pool adapters.** Pools beyond PearlHash currently show configured
   data only. Add an adapter only for a stable, documented API.
5. **Gettext migration** with `po/ru.po` while keeping English source strings
   ([i18n.md](i18n.md)).
6. **RPM spec validation** on Fedora/openSUSE; the spec template exists but is
   untested and not released.
7. **A signed remote catalogue** with a verification story (signature, key
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
- Running `trommeld` itself as root. Independent supervision is systemd plus the
  unprivileged guard process, never a root daemon.

## Decision log

Architecture decisions are recorded as ADRs:
[name](adr/0001-name.md), [extension packaging](adr/0002-extension-packaging.md),
[packaging](adr/0003-packaging.md),
[removing the extension](adr/0004-remove-extension.md),
[one active session](adr/0005-one-active-session.md),
[failure policy and emergency restore](adr/0006-failure-policy.md) and
[per-GPU participation](adr/0007-per-gpu-participation.md).

B1 implements ADR 0005 (single session, one snapshot) and ADR 0006 (daemon
ownership, bounded restart, GUI crash is a no-op, emergency restore, startup
recovery, visible restore failures). B2 implements point 4 of ADR 0005 and is
recorded in ADR 0007 (`enabled` means participation, documented device flags
only, no silent all-card fallback). A superseding decision gets its own ADR and
an explicit “supersedes” note; old ADRs are not rewritten.
