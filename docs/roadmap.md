# Roadmap

Current published release: **0.8.2** (30.10.2026). `main` also contains the
unreleased B6/B7 work. Shipped behaviour is documented in
[user-guide.md](user-guide.md) and [architecture.md](architecture.md); this page
only describes direction and block status. The README and guides do not
describe future work as if it existed.

## Block plan

A block is one branch, one scope freeze and one verified release; the next block
does not start until the previous one is green. Status is updated in the same
commit as the code.

| Block | Scope | Status |
| --- | --- | --- |
| **B0** | Documentation baseline: one active session, failure policy, block roadmap, schema v3 references | **Done in 0.4.4** |
| **B1** | Fault-tolerant session ownership: emergency-stop, OC file lock, helper lease, PID-by-path, startup recovery, systemd/bus shutdown (**B1a**) and guard, bounded restarts, benchmark guard (**B1b**) | **Done in 0.4.5** (B1a 0.4.4, B1b 0.4.5) |
| **B2** | Per-GPU participation: exclude a card from the active miner without a second process; catalogue capabilities for device selection | **Done in 0.5.0** |
| **B3** | Merge the Presets and Miners pages into one preset-centred flow (miner library dialog) | **Done in 0.6.0** |
| **B4** | Miner version manager: install/update/remove with the affected presets shown, rollback | **Done in 0.7.0** |
| **B5** | Release hardening after B1–B4: CI/test matrix, packaging install/upgrade/remove, release bundle verification, MSRV and documentation truth | **Done in 0.8.0** |
| **B6** | Preset-scoped GPU settings, named per-card GPU presets, coins/pools by miner capability, config schema v4, first-run without a wizard | **Implemented on `main`; unreleased** |
| **B7.0** | Dashboard truth and polish: pool balance, hero dedupe, power/temperature sparklines only, grouped preset editor, one GPU-presets entry, apply reasons | **Implemented on `main`; unreleased** |
| **B7.1** | Per-GPU miner telemetry, PCI-bus mapping, honest efficiency units, card hashrate anchor, status contract v5 | **Implemented on `main`; unreleased** |
| **B7.2** | Hashrate watchdog: notify by default, explicit restart opt-in, bounded restarts | **Implemented on `main`; unreleased** |
| **B7.3** | Daemon-owned local history, `GetHistory`, 24h average, History dialog and CSV | **Implemented on `main`; unreleased** |

B3 is complete in 0.6.0, B4 in 0.7.0 and B5 in 0.8.0. Multi-miner/multi-job
remains a separate product decision under [ADR 0005](decisions.md#adr-0005).

## Shipped releases

The detailed release notes for 0.4.0–0.8.2 live in
[CHANGELOG.md](../CHANGELOG.md); the tags and GitHub Releases are the source of
truth. B5 closed the 0.8.0 release-hardening block: CI matrix, headless UI
smokes, package install/upgrade/remove, release-bundle verification, MSRV and a
truthful documentation pass.

## Next (not scheduled)

Ordered by expected value, not commitment:

1. **Download dialog with progress and cancel.** The core downloader is
   synchronous and has no progress events; this needs a streaming API and a
   cancellable worker.
2. **Custom pool adapters.** Pools beyond PearlHash currently show configured
   data only. Add an adapter only for a stable, documented API.
3. **Gettext migration** with `po/ru.po` while keeping English source strings;
   see [development.md](development.md#internationalization).
4. **RPM spec validation** on Fedora/openSUSE; the template exists but is
   untested and not released.
5. **A signed remote catalogue** with a verification story (signature, key
   rotation, offline fallback).
6. **Tray follow-up.** The basic StatusNotifierItem floor is shipped: status
   header, Start/Stop, Show window and Quit, with a monochrome default. The
   follow-up adds show/hide, start-minimised, a dynamic status icon/tooltip and
   submenus.

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

## Release history note

The `CHANGELOG.md` entries for 0.4.1–0.4.4 were written before tags and GitHub
Releases were used; those versions were never tagged or published. `v0.4.5` is
the first tag of that line. The history is not rewritten and old commits are not
retagged: the existing tags and published releases are the source of truth.

## Decision log

Architecture decisions are recorded in [decisions.md](decisions.md):
[name](decisions.md#adr-0001), [extension packaging](decisions.md#adr-0002),
[packaging](decisions.md#adr-0003),
[removing the extension](decisions.md#adr-0004),
[one active session](decisions.md#adr-0005),
[failure policy and emergency restore](decisions.md#adr-0006),
[per-GPU participation](decisions.md#adr-0007),
[desktop tray in the daemon](decisions.md#adr-0008),
[preset-scoped GPU settings](decisions.md#adr-0009) and
[hashrate watchdog](decisions.md#adr-0010).

B1 implements ADR 0005 (single session, one snapshot) and ADR 0006 (daemon
ownership, bounded restart, GUI crash is a no-op, emergency restore, startup
recovery, visible restore failures). B2 implements point 4 of ADR 0005 and is
recorded in ADR 0007 (`enabled` means participation, documented device flags
only, no silent all-card fallback). B3 implements point 2 of ADR 0005 (the
Miners page becomes a miner library dialog; presets stay the primary object and
global installs are never rewritten). B4 implements point 3 (installed versions
are global and versioned; update, remove and rollback show the affected presets
and never rewrite them). A superseding decision gets its own entry and an
explicit “supersedes” note; old entries are not rewritten.
