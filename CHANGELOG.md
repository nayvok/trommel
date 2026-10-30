# Changelog

All notable changes to Trommel are documented here. Format follows Keep a
Changelog; versions use SemVer.

## [Unreleased]

## [0.8.1] - 2026-10-30

### Fixed

- **A scheduled restart no longer races a manual miner.** If an external miner
  appears during the crash backoff, Trommel restores stock, cancels the restart
  and names the PID instead of spawning a second miner.
- **A legacy state file without `miner_path` is never killed by name.** The
  stale record is cleared with a warning. Stop, emergency-stop and startup
  recovery only signal a PID whose `/proc/<pid>/exe` (or script argument) still
  matches the recorded binary, and every signal re-checks the kernel start time,
  so a recycled PID is left alone. An unreadable `/proc` is no longer treated as
  “dead”, and an in-flight helper is re-proved against its lease before each
  signal.
- **The guard restore survives a SIGKILL during shutdown.** The daemon restores
  stock first and disarms the guard second. The guard starts only from
  `TROMMEL_GUARD_BIN` or the `trommel` sibling (never `$PATH`), is skipped under
  systemd, and is reported as started only after its ready marker names this
  daemon.
- **Removing a miner version uses one config snapshot.** Plan, deletion and
  `save()` describe the same record; a failed save is an error toast and never a
  success.
- **`verified` is no longer rewritten when the binary disappears.**
  `normalize()` keeps the install-time fact; the verify report still says the
  binary is missing.
- **A v1 config without GPU fields does not acquire 300 W / 1620 / 810 tuning.**
  Missing values stay auto and the old stock power limit is not carried.
- **The Debian package no longer depends on GNOME.** `gnome-shell` is not
  recommended, `libglib2.0-bin` is not required and the postinst does not
  recompile GLib schemas.
- **Metainfo links point at the repository** (`homepage`, `vcs-browser`) and its
  issue tracker.
- **Documentation screenshots show an owned mining session.** The demo daemon
  starts the recorded miner itself on the shared stateful fake rig, so the
  dashboard and README hero no longer show “external miner”.

## [0.8.0] - 2026-10-30

### Added

- **Headless UI coverage in CI.** `ui-smoke.sh` and `first-run-smoke.sh` run
  under Xvfb with a private AT-SPI bus on pull requests and on the release tag,
  so the AT-SPI click path is no longer a manual-only gate.
- **MSRV job.** The workspace declares Rust 1.92, the real requirement of the
  locked GUI dependencies, and CI checks it with `cargo check --all-targets
  --features trommel/gui`.
- **Packaging checks on every pull request.** `test-deb.sh` now asserts every
  documented path, file mode, maintainer script and the polkit annotation, and
  the new `test-deb-install.sh` installs, upgrades and removes the package in a
  disposable root container.
- **Release verification.** `check-version.sh` and `verify-release-bundle.sh`
  check the version/tag, the asset set and every checksum before publishing;
  the release workflow gates the upload on the UI and MSRV jobs.
- **`SHA256SUMS` covers `trommel-install.sh`** as well as both `.deb` files.

### Changed

- CI and the release workflow build the CLI-only `trommel` target and run the
  GUI-feature unit tests; the release workflow also runs the MCP smoke test.
- The package build remaps the checkout and Cargo home, so release binaries no
  longer contain a builder home path.
- A failed e2e run prints one report with every harness artifact, `daemon.log`,
  `miner.log` and the `✗` lines, and uploads the logs as an artifact.
- `check-no-secrets.sh` also rejects personal home paths (`/home/user` is the
  only allowed placeholder).
- Documentation no longer claims the 0.6.x tree, the 0.5 roadmap, a Rust 1.80
  MSRV or that CI skips markdown links.

### Fixed

- `TROMMEL_LIBRARY_SCREENSHOT` now runs as a private instance, so the miner
  library capture cannot attach to a normal running window; a GUI-feature test
  keeps every screenshot hook paired with the private-instance list.
- `scripts/atspi-bus.sh` verifies a bus with a registered client (the old
  `dbus-send --address` call never sent `Hello`), and fails with a named error
  when `at-spi2-core` or the registry is missing.

No user-facing feature, config/schema change or screenshot change.

## [0.7.0] - 2026-10-29

### Added

- **Miner version manager (B4).** The miner library is now a version manager:
  every installed version is a row that names the presets pinning it, its
  binary state and the recorded SHA-256 (or none), with explicit **Verify** and
  **Remove** actions. Installing another version is an update; the library then
  lists the presets that still pin an older version and lets the user select
  which ones move. Presets are never rewritten by installing a build, and the
  default is to keep every pin.
- **Preset rollback.** A **Preset versions** group switches exactly one preset
  to any other installed (and executable) version. Other presets keep their
  versions; a running miner goes through the normal apply/restart/stop dialog.
- **Guarded removal.** Removing a version that any preset pins is refused with
  the list of presets and an explanation; removing an unused version asks for
  confirmation. Only files under the managed miner store are deleted; a record
  pointing elsewhere loses its record and keeps the binary.
- **`trommel miner verify` and `trommel miner remove`.** Verify checks that
  each recorded binary exists, is a regular file, is executable and readable,
  and reports `ok`/`missing`/`not_executable` with only the recorded SHA-256
  (never a fabricated one); it exits non-zero when a check fails. Remove refuses
  a pinned version and requires `--yes` for an unused one.
- **Version state in the agent surfaces.** `trommel diagnose --json` and the
  read-only MCP `trommel_miners` tool now include installed versions with their
  binary status, recorded checksum and the presets that pin them.

### Changed

- `trommel miner list` lists one line per installed version with its binary
  status and the presets using it, including custom builds outside the
  catalogue.
- The miner library groups installed versions separately from the catalogue row
  and adds the per-preset version switch. The install dialog remains the only
  installer.

## [0.6.0] - 2026-10-29

### Added

- **Miner library dialog (B3).** The catalogue, installed versions, preset
  usage and the A/B benchmark moved from the removed Miners page into a modal
  library opened from the Presets page and the preset editor. From a preset the
  library shows that preset first, with its pinned `miner_version` and a direct
  **Install…** action.
- **Missing-version hints on preset rows.** A preset that pins an uninstalled
  version now says so in its subtitle and offers **Install…**, which opens the
  library focused on that preset. Start was already blocked by the missing
  install; the row now explains why and where to fix it.

### Changed

- **Preset-centred navigation.** The top-level navigation is now Dashboard,
  Presets and Settings. Miner installation remains global in `miner_installs`,
  and installing a build never rewrites a preset or its version.
- The preset editor's install action is now **Miner library…**; the library
  keeps the preset context and links to the unchanged install dialog.
- The miner library lists which presets and versions reference each miner, so
  the effect of installing a shared build is visible before acting.

### Removed

- **The Miners top-level page** and its benchmark group. Installing remains one
  dialog shared by the library, the editor and the wizard.

## [0.5.0] - 2026-10-29

### Added

- **Per-GPU participation (B2).** A card switched off in the GPU profile is now
  excluded from the active mining session, not just from tuning: the miner is
  started with the participating `nvidia-smi` indices only, the excluded card
  receives no power or clock changes, and any tuning left by a previous
  restore-off session is returned to stock before the new session starts. The
  status document exposes `participation[]` (`enabled`, `mining`), the dashboard
  marks each card `mining`/`excluded` and Totals counts `N of M`.

- **Documented device-selection flags.** The catalogue now carries
  `device_flag` next to `per_gpu_devices`: WildRig `--gpu-list`, SRBMiner
  `--gpu-id`, OneZero `--devices`. A miner without a documented flag refuses a
  partial rig with an explicit error instead of silently mining on every card;
  a device flag in `extra_args` is rejected while Trommel supplies the list.
  Benchmarks follow the same rule. See
  [ADR 0007](docs/adr/0007-per-gpu-participation.md).

- **Exclusion coverage in tests.** Unit tests cover every driver branch,
  comma-separated indices, empty/one/many cards and the unsupported-miner
  refusal. The e2e run starts with one excluded card and checks the miner
  arguments, the untouched excluded card, stock restoration for a previously
  tuned excluded card, a reduced emergency-stop restore and the explicit
  refusal without spawning.

### Changed

- `GpuTuning.enabled` now means “participates in the mining session”. An enabled
  card with no tuning still mines; an excluded card is not passed to the miner.
  Changing participation is a restart change and uses the normal apply dialog.
- The snapshot's `applied_gpus` covers only the participating, tuned cards;
  Stop, shutdown and emergency-stop restore exactly that set. Aggregate rig
  power and the hottest-card temperature still include excluded cards and say so
  in their tooltips.
- The GUI switch is labelled for mining participation (not “profile enable”);
  the card chip and Totals describe the applied session, so a draft switch never
  claims the miner already restarted.

### Removed

- **The dashboard Efficiency row.** The J/TH value is still computed and
  published in the status JSON for agents and diagnostics, but the Totals group
  no longer shows it.

## [0.4.5] - 2026-10-29

### Added

- **Bounded restart with backoff (B1b).** A crashing miner is restarted at most
  five times in a row, with an exponential delay from `restart_delay_s` capped
  at 60 s. When the budget is exhausted the session stops, the stock snapshot is
  restored and an error notification explains why. `started_at` is reset on
  every spawn, so uptime is the uptime of the current process.

- **A guard process for manual runs (B1b).** `trommeld` starts a hidden
  `trommel guard` child in its own session. The guard watches the daemon PID and
  `/proc` start time; if the daemon is SIGKILLed without disarming, the guard
  runs the idempotent `emergency-stop`. A graceful shutdown disarms it before
  the restore, and a new daemon terminates a stale guard after startup
  recovery.

- **Benchmark safety (B1b).** A benchmark refuses to start while an external
  miner is running or while the live miner is not owned by this session. It no
  longer stops a process it cannot prove is its own.

### Changed

- `state.miner_path` is now written on spawn; a custom-named miner is stopped by
  its absolute path rather than by a name heuristic.
- `TROMMEL_MAX_RESTARTS`, `TROMMEL_NO_GUARD` and `TROMMEL_GUARD_BIN` are test/
  debugging hooks, documented in [development.md](docs/development.md).
- GUI crash test added to the UI smoke and the e2e matrix.

## [0.4.4] - 2026-10-29

### Added

- **Fault-tolerant session ownership (B1a).** `trommel emergency-stop` is an
  idempotent teardown for a daemon that crashed or was SIGKILLed: it waits for
  an in-flight GPU helper (or terminates it), stops only the miner recorded in
  `state.json` when its PID still matches the saved binary path, restores the
  snapshot with a short timeout, keeps the snapshot and `last_error` on failure
  and logs the reason (`service-stop`, `guard`, `startup`, `manual`).

- **Startup recovery.** A restarted daemon stops an orphaned miner from the
  previous session, restores an applied snapshot and leaves mining stopped. It
  no longer adopts the miner and continues mining.

- **OC file lock and helper lease.** Apply, restore and emergency-stop are
  serialized with an advisory `flock`; nested acquisition is rejected. The
  helper receives `--lease-pid/--lease-start` (strict numeric values) and
  aborts before its first mutation when the daemon PID/start time no longer
  matches, so a queued polkit request cannot tune a rig that is being restored.

- **Graceful shutdown and lost session bus.** SIGTERM/SIGINT and
  `connection.closed()` stop only our miner, force a snapshot restore with a
  short timeout and exit. systemd runs
  `ExecStopPost=-/usr/bin/trommel emergency-stop --reason=service-stop` with
  `KillMode=control-group` and `TimeoutStopSec=30`.

- **Path-safe miner installs.** `miner`, `version` and `binary` are validated as
  one safe path component before any network request or write; `../escape`,
  `a/b`, empty and `.`/`..` are rejected.

### Changed

- `State` gained the additive `miner_path` field (old files parse unchanged).
  `State::miner_alive`, Stop and emergency-stop compare `/proc/<pid>/exe` and
  the script argument in `cmdline`; an external miner is never killed.
- `find_external_miner(cfg, exclude_pid)` uses the directory/binary basenames of
  every installed miner in addition to the catalogue names.
- `Supervisor::with_config` resolves the active preset's binary; startup
  recovery runs before the daemon serves D-Bus.
- The failure policy is recorded in
  [ADR 0005](docs/adr/0005-one-active-session.md) and
  [ADR 0006](docs/adr/0006-failure-policy.md).


### Added

- **Desktop notifications without the panel applet.** The daemon sends crash,
  error and overheat alerts through `org.freedesktop.Notifications`, so the
  **Behavior → Notifications** switch keeps working after the GNOME Shell
  extension was removed. The `Notification` D-Bus signal is still emitted for
  integrations that want to render their own alert.

- **Totals now explain themselves.** The **Totals** group gained an
  **Efficiency** row (J/TH, already in the status JSON but never shown), and the
  power and temperature rows carry a tooltip with one line per card. Power is
  `power.draw` summed over every detected card, including the ones switched off
  in the profile; the temperature is the hottest card's reading, not an average.

- **A note when a card is at the temperature limit.** The dashboard **Notes**
  row now names the hot card and both temperatures (`card #1: 95 °C (limit
  90 °C)`) instead of staying silent while the miner throttles itself.

- **Sparklines in the GPU cards.** Every metric bubble — power, core and memory
  clock, temperature, fan and load — ends with a five-minute sparkline on the
  bubble floor: a thin accent line over a soft wash, tinted like the value when
  it runs warm or hot. The history is GUI-side (it starts empty and fills at the
  poll rate), so the daemon status contract and the CLI/MCP JSON are unchanged.

### Changed

- **Default window narrowed to 880 × 760.** The one-line view switcher holds in
  Russian down to about 850 px, so the window opens a hundred pixels narrower
  without falling back to the stacked labels or the bottom bar.
- **Miner install: two groups and a shorter primary action.** Version and
  installation are separate groups; **Import binary** is a row instead of a
  third footer button, and the URL, checksum, archive and binary fields sit in a
  **Download source** expander whose subtitle shows the URL. The footer keeps
  **Cancel**, the status line and a plain **Install** button, so it fits a
  400 px window; long status messages ellipsize and keep the full text in a
  tooltip. The dialog no longer needs to scroll to reach its own footer.
- **GPU card charts share one floor.** A grid row gives its leftover height to
  the tile's text block instead of the chart holder, so a chart stays 24 px tall
  and ends on the bubble floor even when a neighbour in the same row carries a
  hint line. The temperature chart was the visible case: it stopped a line
  short of the bottom edge.
- **Sparklines reach the bubble edges.** The wash is over-drawn to the last
  pixel and the plot keeps only a 2 px bottom margin, so the graph sits on the
  bubble floor instead of floating above it or being clipped before the right
  corner.
- **Navigation moved into the header bar.** The four sections are an
  `AdwViewSwitcherTitle`, so the switcher sits next to the window actions
  instead of in a separate bar at the bottom. When the window is too narrow for
  labels, libadwaita swaps the switcher back to the title and reveals the
  `AdwViewSwitcherBar` — one navigation surface at a time. The GPU cards now
  share the row (`AdwWrapBox`, fill) instead of hugging the left edge.
- **Start/Stop moved from the header into the dashboard hero.** The header now
  holds navigation and window chrome only (refresh, switcher, menu), so the
  one-line switcher fits down to roughly 880 px and the default window shrank
  back to 960 × 760. The primary action sits in the hero footer with a
  play/stop icon, right aligned with the uptime line, and keeps its
  suggested/destructive colouring.
- **Dialog actions moved into footers.** The preset editor, GPU tuning, miner
  install, quick setup and error details keep their headers for the title and
  the window controls only; **Cancel**/**Save**, **Next** and the tuning presets
  menu now sit in a footer below the content, with the primary action on the
  right. The tuning dialog's **⋯** menu became a labelled **Presets** menu next
  to **Save**, and the miner install dialog's release-page icon became a
  **Release page** row in the version group.
- **Modal windows became `AdwDialog`s.** The preset editor, miner install, GPU
  tuning, quick setup and error details are presented inside the main window.
  They inherit its style and use libadwaita's spring presentation (the same
  subtle bounce as the alert dialogs) instead of the compositor's window-open
  scale. Dialog headers drop the automatic close button when they already carry
  an explicit Cancel/Close.
- **Settings no longer carries a separate “GPU defaults” group.** Power limits
  and clock locks are GPU state, so they are edited per card in **Tune**; the
  profile default is only the built-in base (driver-auto power, no clock locks)
  that non-custom cards inherit. The global **Temperature limit** and
  **Resume temperature** rows moved to **Behavior** — they drive the overheat
  warning and are passed to miners that support a temperature limit.

### Removed

- **The GNOME Shell extension and its shared GSettings schema.** The applet,
  `data/schemas/org.gnome.shell.extensions.trommel.gschema.xml`, the
  **Settings → GNOME integration** group and `install.sh --no-extension` are
  gone; the flag is accepted as a no-op so old commands do not break.
  `uninstall.sh` still removes an applet left by an older install. Upgrading
  from a release that shipped the applet leaves its UUID in
  `org.gnome.shell enabled-extensions`; the package manager removes the files,
  and the stale entry is removed once in Extensions (or with `gsettings`). See
  [ADR 0004](docs/adr/0004-remove-extension.md).

### Fixed

- **The overheat notification followed only the first card.** The daemon read
  `status.gpu`, so on a two-GPU rig a hot second card never produced an alert
  even though the **Totals** row showed its temperature. The refresher now uses
  `Status::hottest_gpu`, names the card in the body (`… 95 °C (card #1, limit
  90 °C)`) and re-arms on the same episode rule as before; the e2e suite drives
  the second card over the limit to cover it.
- **GPU settings are now verified, not assumed.** `trommel-oc` reads the power
  limit back after `-pl` and fails instead of claiming success; `reset` reports
  a failing `-rgc`/`-rmc`; the supervisor checks memory/core locks once the
  miner has loaded the GPU. An ignored memory lock stops the miner with the
  requested and actual frequencies.
- **The 30 s helper timeout no longer kills the polkit password dialog.** The
  call waits up to 180 s (`TROMMEL_OC_TIMEOUT_MS` for tests), and the status
  loop keeps serving the last document while authorization is pending.
- **A per-GPU entry no longer shadows the default profile.** Schema v3 adds
  `custom`: entries created by the setup wizard or the GPU enable switch inherit
  power/clocks/temperature from the profile default; only the tuning
  dialog creates a custom override. v2 configs are migrated automatically.
- **Stop and Restore now tell the truth.** With `restore_stock_on_stop=false`
  Stop keeps the snapshot and reports `oc.needs_restore`; the dashboard shows a
  banner and the status warning explains it. A failed restore keeps the snapshot
  and its error. The Restore action offers the pre-mining snapshot or the driver
  default when they differ, and never reports success for a no-op.
- **Snapshot hygiene:** a snapshot from another boot is deleted at daemon start
  and never used; a profile with no tuning creates no snapshot and needs no
  root; a stock Start restores any lingering applied snapshot first.
- **Settings switches now stick without the Save button.** Toggling *Restore
  stock on stop* updated only the widget; pressing the global Apply banner saved
  the old value and the dashboard warning stayed. Behaviour switches and the
  poll interval now write into the draft as they change.
- The Dashboard navigation icon used `utilities-system-monitor-symbolic`, which
  is missing from the Adwaita icon theme, so some themes showed a broken-image
  placeholder. It now uses `view-grid-symbolic`, available in Adwaita and Yaru.
- The Rust `--lmc` validator accepted only up to 4000 MHz while the schema, GUI
  and helper allow real memory clocks up to 12000 MHz.
- **All cards off no longer produces a clipped Start error.** Header **Start**
  is disabled while every detected GPU is off, the status warning covers that
  case even when the profile default would enable a missing card, and action
  errors now open a selectable, copyable **Details** dialog.

### Changed

- The status contract is now **3**: `oc` gains `needs_restore`,
  `restore_on_stop`, `last_error`, snapshot metadata and target summaries.
- Dashboard GPU cards show live power, clocks, temperature, fan and load in
  value bubbles with the configured target/lock as a hint. A single-GPU rig
  uses one full-width card (3×2 bubbles); 2+ cards keep the compact grid. The
  card itself is not clickable, so its hover tint is gone; **Tune** and the
  include switch remain the controls. The duplicate *Detected GPUs* group left
  Settings; per-card edits live in **Tune**. The tune dialog's **Recommended** /
  **Profile default** actions moved into a **⋯** menu.
- The dashboard hero is accent-tinted with a soft shadow, a small HASHRATE
  caption, a larger value, a separator and a quieter uptime/restarts row.
- New D-Bus `OCReport()` / `RestoreOC(mode)`, CLI `trommel oc report|restore`,
  MCP `trommel_oc` / `trommel_restore_oc`, and the under-load clock check.
- Documentation was restructured by audience: a new index, an architecture and
  component reference, a development guide with the test harness, and a design
  specification. The UX map was folded into `docs/design.md` /
  `docs/design.ru.md`; the obsolete session plan was removed.
- `scripts/check-docs.sh` validates relative markdown links and flags removed
  documents.
- The config schema now documents `behavior.appearance`.

## [0.4.3] - 2026-10-28

### Fixed

- The **Settings → Actions** buttons were stretched to the height of their
  `AdwActionRow`; the pill buttons are now vertically centred and have a
  consistent height at both ~400 px and the default width.
- Detected GPUs in Settings are individual rounded cards with an icon, name,
  three labelled configured metrics (power/core/memory) and live
  temperature/fan/load chips. The old `auto · auto / 810 MHz …` string did not
  say which value was which.
- Toggling a GPU off while mining and then choosing **Cancel** left the switch
  off and the draft dirty. The switch, the per-card override and the dirty
  banner are now restored; Apply-and-restart / Apply-and-stop still work.
- The GPU tuning dialog no longer shows a stale 1620/810 value next to an
  enabled-looking lock. The value row appears only when the lock is on, the
  dialog shows the current clocks, and a single-GPU machine no longer gets an
  empty “Apply to” section.
- The dashboard no longer renders lone dashes when the miner is stopped:
  hashrate shows `0 TH/s`, pool balance shows a numeric placeholder, and the
  pool row still names the configured endpoint. Core and memory clocks are
  labelled separately on the GPU card.
- Status now warns when a configured core/memory lock is not what the driver
  reports. This makes a real 810-vs-9501 mismatch visible instead of hiding it
  behind an ambiguous “1770 / 810 MHz” label.
- On a first start with no wallet or no miner binary, **Start** opens Quick
  setup instead of failing with a toast. A new `scripts/first-run-smoke.sh`
  covers the pristine-install path.

### Added

- `scripts/first-run-smoke.sh` and the `first-run` AT-SPI phase: clean XDG
  dirs, no config, no installed miner, one fake GPU, placeholders, wizard and
  single-GPU tune dialog.

## [0.4.2] - 2026-10-28

### Fixed

- Header **Start** now applies the GUI draft before starting. Previously it
  asked the daemon to start with its already-saved config, so a GPU profile
  edited in the preset/GPU dialog could be ignored until Apply was pressed.
- The chevrons in **Presets** and **Miners** are real buttons again. They open
  the preset editor / miner installer on click instead of being decorative
  images that did nothing.
- Preset and miner rows are added directly to their `AdwPreferencesGroup`
  instead of an intermediate box, restoring normal row activation.
- The GNOME extension crashed on enable with `TypeError: can't access property
  "box", this.menu is undefined`; the menu now comes from the panel button.
- The main window can shrink from ~698 px to ~400 px. The per-card settings
  table is now one compact row per card instead of six columns, so the
  Settings page no longer sets the minimum window width.

## [0.4.1] - 2026-10-28

### Added

- GitHub Releases distribution: versioned and stable `.deb` assets, SHA-256
  files, `trommel-install.sh` and a tag-driven release workflow. No apt
  repository or PPA is used; upgrades are a new release download.
- `scripts/release-bundle.sh`, `scripts/install-polkit-rule.sh`,
  `scripts/check-polkit.sh` and an AT-SPI `scripts/ui-smoke.sh` UI smoke test.

### Changed

- The Debian postinst writes a per-user polkit rule for `SUDO_USER`, so GPU
  tuning works without a password prompt from the user service. The generic
  `/usr/share` rule is only a local fallback.
- WildRig catalogue versions 0.51.8/0.51.5 now point at the official Linux
  tarballs with verified SHA-256 checksums and `tar.gz` extraction.
- `miner import` copies the binary into the managed miner store instead of
  referencing an external path; downloads reject HTML/non-executable files.
- `Settings -> Power limit` accepts 0 = driver auto (it used to clamp 0 to
  50 W and could save a dangerous 50 W limit).
- The per-card table shows live temperature/fan; the per-card dialog no longer
  pretends to tune fan/temperature, which are global.
- GPU tuning and GPU enable switches apply immediately (with the usual
  restart/stop dialog while mining), and preset "Use" applies the selection.
- `install.sh` restarts `trommeld`, so an upgrade does not keep the old 0.3.0
  process.

### Fixed

- Polkit rule matched `org.freedesktop.policykit.exec` while pkexec checks the
  annotated `io.github.nayvok.trommel.oc` action; every OC call prompted or
  hung. The helper now has a timeout with a clear error.
- Miner installs were saved to disk but not sent to the running daemon, so
  Start claimed the miner was missing. The GUI/CLI now apply the install to
  the daemon (and the GUI no longer drops a dirty draft on Refresh).
- Stale `external` state made Start a silent no-op; a daemon restart lost the
  miner's process handle. State normalization, adoption by PID and auto-restart
  now handle both.
- `stop` swallowed stock-restore errors and a partially applied `--pl` could
  survive a failed start. Errors are surfaced, the snapshot is kept for a
  retry, and the current card is restored too.
- The pool API could block status collection for up to 40 s; it now runs in a
  background worker with a cache/backoff.
- `Copy diagnostics` redacted only the active preset; all presets are now
  redacted through `trommel_core::diagnostics`.
- A cross-filesystem `/tmp` made installs fail with EXDEV; staged files are
  moved with a copy fallback.
- A second launch created a second window and D-Bus worker; it now presents the
  existing window.

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
