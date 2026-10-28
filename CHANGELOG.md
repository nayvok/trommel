# Changelog

All notable changes to Trommel are documented here. Format follows Keep a
Changelog; versions use SemVer.

## [Unreleased]

### Added

- **Desktop notifications without the panel applet.** The daemon sends crash,
  error and overheat alerts through `org.freedesktop.Notifications`, so the
  **Behavior → Notifications** switch keeps working after the GNOME Shell
  extension was removed. The `Notification` D-Bus signal is still emitted for
  integrations that want to render their own alert.

### Changed

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
