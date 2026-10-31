# Roadmap

Current release: **0.8.2** (30.10.2026). Shipped behaviour is documented in
[user-guide.md](user-guide.md) and [architecture.md](architecture.md); this page
only describes direction. Anything not shipped is explicitly in the future
sections — the README and guides do not describe it.

## Block plan

The 0.4–0.8 work is split into blocks. A block is one branch, one scope freeze
and one verified release; later blocks are not started until the previous one is
green. Status is updated in the same commit as the code.

| Block | Scope | Status |
| --- | --- | --- |
| **B0** | Documentation baseline: one active session, failure policy, block roadmap, schema v3 references | **Done in 0.4.4** |
| **B1** | Fault-tolerant session ownership: emergency-stop, OC file lock, helper lease, PID-by-path, startup recovery, systemd/bus shutdown (**B1a**) and guard, bounded restarts, benchmark guard (**B1b**) | **Done in 0.4.5** (B1a 0.4.4, B1b 0.4.5) |
| **B2** | Per-GPU participation: exclude a card from the active miner without a second process, catalogue capabilities for device selection | **Done in 0.5.0** |
| **B3** | Merge the Presets and Miners pages into one preset-centred flow (miner library dialog) | **Done in 0.6.0** |
| **B4** | Miner version manager: install/update/remove with the affected presets shown, rollback | **Done in 0.7.0** |
| **B5** | Release hardening after B1–B4: CI/test matrix, packaging install/upgrade/remove, release bundle verification, MSRV and documentation truth | **Done in 0.8.0** |
| **B6** | Preset-scoped GPU settings, named per-card GPU presets, coins/pools by miner capability, config schema v4, first-run without a wizard | **In progress** |
| **B7.0** | Dashboard truth and polish: pool balance, hero dedupe, power/temperature sparklines only, grouped preset editor, one GPU-presets entry, apply reasons | **In progress** |
| **B7.1** | Per-GPU miner telemetry, PCI-bus mapping, honest efficiency units, card hashrate anchor, status contract v5 | **In progress** |
| **B7.2** | Hashrate watchdog: notify by default, explicit restart opt-in, bounded restarts, ADR 0010 | **In progress** |
| **B7.3** | Daemon-owned local history, `GetHistory`, 24h average, History dialog and CSV | **In progress** |

B3 is complete in 0.6.0, B4 in 0.7.0 and B5 in 0.8.0. Multi-miner/multi-job
remains a separate product decision under
[ADR 0005](adr/0005-one-active-session.md).

## Shipped in 0.8.x — release hardening (B5)

Scope frozen on 30.10.2026 and shipped in 0.8.0. Every item is backed by a
concrete gap in the 0.7.0 tree; anything not listed here stayed out of B5.

Evidence for the freeze:

- CI built `trommeld` and the GUI but never the CLI-only `trommel` target,
  although the CLI-only build is a stated invariant; `cargo test --workspace`
  never enables the `gui` feature, so GUI unit tests cannot run; the release
  workflow skipped `./scripts/test-mcp.sh`; packaging was built only by the
  manual `package.yml` or at tag time.
- `scripts/test-deb.sh` asserted 9 of the 13 paths documented in
  [packaging.md](packaging.md) and never installed the package;
  `scripts/release-bundle.sh` wrote `SHA256SUMS` from `*.deb` only while the
  docs promised all assets, so `trommel-install.sh` had no checksum entry; a
  local build embedded `/home/<builder>` paths in the binaries; nothing checked
  the tag against the workspace version or the bundle before publishing.
- The e2e failure annotation showed 5 artifacts while the harness writes 19
  `.out` files plus `state/trommel/daemon.log` and `miner.log`;
  `scripts/atspi-bus.sh` returned success when the registry binary was missing;
  `check-no-secrets.sh` did not look for personal paths;
  `TROMMEL_LIBRARY_SCREENSHOT` was missing from the `NON_UNIQUE` list in
  `crates/cli/src/gui.rs`, so the library screenshot could attach to a normal
  running instance; the declared `rust-version = "1.80"` was lower than the
  locked dependencies (1.89) and the GUI dependencies (1.92).
- [README.md](README.md) of the docs said “0.6.x” and “CI does not check
  markdown links yet”, `development.md` said “The 0.5 roadmap”, and
  [verification.md](verification.md) implied that the release workflow ran the
  UI smoke tests.

The frozen scope, now shipped:

1. **Test matrix.** Build the CLI-only target; run `cargo test -p trommel
   --features gui`; add a pinned MSRV job (Rust 1.92); run
   `ui-smoke.sh` and `first-run-smoke.sh` headlessly under Xvfb with a private
   AT-SPI bus on pull requests and in the release workflow; run MCP and the
   packaging checks in the release workflow.
2. **Packaging.** Build and validate the `.deb` in CI on pull requests; assert
   the complete documented layout, file modes and maintainer scripts; add an
   install/upgrade/remove test; make the package build path-neutral.
3. **Release.** Add `scripts/check-version.sh` and
   `scripts/verify-release-bundle.sh`, include `trommel-install.sh` in
   `SHA256SUMS`, guard the tag/version in the release workflow, and add a
   concrete release checklist to [packaging.md](packaging.md).
4. **Harness and security.** One shared `scripts/e2e-diagnostics.sh` (all
   harness logs, uploaded as an artifact on failure); `atspi-bus.sh` fails
   closed when the registry is unavailable; `check-no-secrets.sh` rejects
   personal home paths; the library screenshot hook joins the private-instance
   list.
5. **Documentation.** Correct the stale version, roadmap, MSRV and CI claims;
   document the headless UI harness and the manual checks that still need a
   real rig.

Deliberately not in B5: the “Next” items (download progress, custom pool
adapters, gettext, RPM spec validation, signed catalogue), the “Later” items,
any config/schema change, a UI redesign or screenshot regeneration, new
product features, RPM/AppImage/Flatpak artifacts, multi-miner and any weakening
of an existing check to get a green run.

## Shipped in 0.7.x

- **Miner version manager (B4).** The miner library now lists every installed
  version per miner, the presets that pin it, its binary state and explicit
  **Verify** and **Remove** actions. Installing a newer build never rewrites a
  preset: after an install the affected presets are listed and the user chooses
  which ones (if any) move. A version pinned by a preset cannot be removed; an
  unused one needs confirmation. A **Preset versions** switch moves exactly one
  preset to any installed older version, so rollback is explicit and local.
  `trommel miner verify/remove` and the read-only MCP/`diagnose` bundle expose the
  same state; checksums are only echoed from the install record, never invented.
  See [ADR 0005](adr/0005-one-active-session.md).

## Shipped in 0.6.x

- **Preset-centred navigation (B3).** The Miners page is gone. Its catalogue,
  installed versions, preset usage and A/B benchmark now live in a modal miner
  library opened from the Presets page and the preset editor. A preset row whose
  pinned `miner_version` is missing says so and links straight to the library.
  Installed miners stay global in `miner_installs`; installing a build never
  rewrites a preset or its version. See
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

## Next (not scheduled)

Ordered by expected value, not commitment:

1. **Download dialog with progress and cancel.** The core downloader is
   synchronous and has no progress events; this needs a streaming API and a
   cancellable worker before it can be exposed.
2. **Custom pool adapters.** Pools beyond PearlHash currently show configured
   data only. Add an adapter only for a stable, documented API.
3. **Gettext migration** with `po/ru.po` while keeping English source strings
   ([i18n.md](i18n.md)).
4. **RPM spec validation** on Fedora/openSUSE; the spec template exists but is
   untested and not released.
5. **A signed remote catalogue** with a verification story (signature, key
   rotation, offline fallback).
6. **Tray follow-up.** The basic StatusNotifierItem floor is in the next
   release: status header, Start/Stop, Show window and Quit, with a monochrome
   default. The follow-up adds show/hide, start-minimised, a dynamic status
   icon/tooltip and Happ-style submenus.

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

Architecture decisions are recorded as ADRs:
[name](adr/0001-name.md), [extension packaging](adr/0002-extension-packaging.md),
[packaging](adr/0003-packaging.md),
[removing the extension](adr/0004-remove-extension.md),
[one active session](adr/0005-one-active-session.md),
[failure policy and emergency restore](adr/0006-failure-policy.md),
[per-GPU participation](adr/0007-per-gpu-participation.md) and
[desktop tray in the daemon](adr/0008-tray.md),
[preset-scoped GPU settings](adr/0009-preset-gpu-settings.md) and
[hashrate watchdog](adr/0010-hashrate-watchdog.md).

B1 implements ADR 0005 (single session, one snapshot) and ADR 0006 (daemon
ownership, bounded restart, GUI crash is a no-op, emergency restore, startup
recovery, visible restore failures). B2 implements point 4 of ADR 0005 and is
recorded in ADR 0007 (`enabled` means participation, documented device flags
only, no silent all-card fallback). B3 implements point 2 of ADR 0005 (the
Miners page becomes a miner library dialog; presets stay the primary object and
global installs are never rewritten). B4 implements point 3 (installed versions
are global and versioned; update, remove and rollback show the affected presets
and never rewrite them). A superseding decision gets its own ADR and an explicit
“supersedes” note; old ADRs are not rewritten.
