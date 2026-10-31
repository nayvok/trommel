# Changelog

All notable changes to Trommel are documented here. Format follows Keep a
Changelog; versions use SemVer.

## [Unreleased]

### Added — daemon-owned local history

- **One sample per minute, kept for about a week.** `trommeld` appends a JSONL
  sample (per-GPU hashrate/power/temperature/fan, total hashrate, shares,
  preset id, source) to `~/.local/state/trommel/history/history-YYYYMMDD.jsonl`.
  Daily UTC files keep retention a simple prune; corrupt lines are skipped,
  duplicate timestamps are deduplicated on read and a clock that steps
  backwards never overwrites a newer sample. No wallet, worker, pool URL or
  network telemetry is stored.
- **`GetHistory(since, until)`.** The daemon reads the local range (≤ 8 days) in
  a blocking task and returns a sorted JSON array, so `Status` does not grow on
  every poll.
- **History dialog and 24h average.** A calm **History** button in the hero
  footer opens an `AdwDialog` with 1h/24h/7d ranges, an optional per-card
  series, the existing sparkline language and a one-line summary; CSV export
  writes the raw long format. The hero meta line can append `24h avg` from the
  same local samples. The former five-minute GUI ring buffer is only the
  sparkline window and is now named accordingly.

### Added — hashrate watchdog (notification by default)

- **A live but degraded miner is no longer invisible.** `trommeld` watches the
  last status in the supervisor loop and detects sustained low total hashrate,
  0 H/s, a participating card that disappeared from the per-card data and stale
  miner/status samples. It only acts on a session Trommel owns; an external
  miner, a benchmark and an in-flight crash restart are never touched.
- **Notify by default; restart is an explicit opt-in.** The watchdog is off by
  default. `behavior.watchdog_action = "notify"` emits the existing
  `Notification` signal/desktop notification; `"restart"` waits the existing
  `restart_delay_s` backoff and calls the normal supervisor Stop/Start
  (snapshot, restore, device plan, external scan). After
  `watchdog_max_restarts` failed restarts it stops and notifies; it never
  reboots the rig. See [ADR 0010](docs/decisions.md#adr-0010).
- **Settings → Reliability.** One calm group: enable, minimum percentage of the
  preset expected hashrate, window in minutes, Notify only / Restart the miner
  and the restart limit. Dependent rows are hidden while the watchdog is off,
  and a note explains when the preset has no expected hashrate.
- **Schema v4 stays additive.** The new `behavior` keys have defaults and are
  clamped by `normalize()`; no data migration and no `schema_version` bump. A
  v4 file without the keys loads with the safe defaults.

### Added — per-GPU telemetry and honest efficiency (status contract v5)

- **Per-card miner statistics.** The status `miner.gpus[]` now carries the
  card's own hashrate windows (and accepted/rejected shares where the API
  documents them). WildRig (`hashrate.threads` + `hwmon.busID`), SRBMiner
  (`algorithms[].hashrate.gpu.gpuN` + `gpu_devices[].bus_id`) and OneZero
  (`devices[].bus_id` + parallel `algos[].hashrates[]`) have documented
  parsers; a miner that does not report per-card data leaves the list empty and
  the dashboard shows `—`. The total is never divided between cards.
- **PCI bus mapping.** `nvidia-smi` now records `pci.bus_id`; a miner record is
  attached to a card only when exactly one card has that bus. Collisions and
  unknown buses stay unmapped with one calm warning.
- **Efficiency has a unit.** `total.mining_power_w` covers participating cards
  only; `total.efficiency` + `total.efficiency_unit` use the coin's display
  unit (`J/TH`, `J/MH`, …), and the legacy `efficiency_j_per_th` is null
  outside TH/s. Rig `power_w` still covers every detected card.
- **Hashrate is the card's anchor.** The GPU card shows the per-card hashrate
  and its five-minute sparkline above the six value bubbles; power and
  temperature keep their trends, core/memory/fan/load stay plain values.
- **Hero expected vs actual.** One metadata line compares the preset's expected
  hashrate with the actual value and shows efficiency. The preset name stays in
  the Active preset row.
- **Benchmark parser fixed.** The A/B benchmark now uses the preset's effective
  API port and the same driver-aware parser, so a default-port WildRig API is
  no longer reported as “no hash rate”.

### Fixed — dashboard truth before B7 telemetry

- **A missing pool API no longer looks like a zero balance.** The Pool balance
  row shows `—` with a calm explanation when the pool has no documented account
  API, has not answered yet, or was never configured; only a real API value can
  render `0.000`. The pool/shares group is hidden when no preset exists.
- **The hero no longer repeats the preset.** It now carries coin, pool and fee;
  the preset name lives once in the Active preset row. That row also carries the
  rig power, hottest temperature and stock/tuned state with per-card tooltips —
  the totals widgets existed but were never added to the layout.
- **Unknown hashrate and shares are `—`, not fake zeros.** A running miner that
  has not reported yet no longer shows `0 TH/s`; a real zero from the miner or
  pool is still shown as `0`.
- **Only power and temperature keep sparklines.** Core, memory, fan and load
  remain live values, but four extra graphs per card were noise without a
  trend; the six-bubble grid keeps its shape and height.
- **The preset editor is grouped.** Summary, Coin and pool, Wallet, Miner, GPU
  and Advanced replace one flat form; the algorithm of a known coin is
  read-only, and Simple mode hides the whole Advanced group.
- **GPU preset management has one entry.** Rename/delete lives in
  Settings → GPU; the Tune menu only selects a saved preset or offers Save as.
- **The apply alert lists the reasons.** It shows the already localised
  `ConfigDiff` class/reasons and explains exactly what restarts; it never
  invents old→new values the diff does not carry.

### Added — preset-scoped GPU settings and schema v4

- **Each mining preset now owns its GPU settings.** The shared `gpu_profiles`
  map is gone; a preset carries its per-card participation, power limit and
  clock locks. Switching the active preset switches the GPU setup, and a new
  preset starts from stock.
- **Named GPU presets.** A card's Tune dialog can save, apply, rename and delete
  reusable per-card tuning templates. Deleting a mining preset keeps them.
- **First run without Quick setup.** A fresh config has no preset. The dashboard
  offers **Create preset**; Start explains the same thing and opens the preset
  editor. The old wizard no longer exists.
- **Coins and manual pools.** The preset editor chooses a coin filtered by the
  selected miner's advertised algorithms (with a custom-coin path), enters the
  pool address by hand and can point at a documented JSON pool API. The pool
  dropdown is gone; a known host only supplies a fee hint and API template.
- **Per-coin hashrate units.** The hero and CLI use the coin's preferred unit
  (`TH/s`, `MH/s`, …) with automatic scaling when the value is far away.
- **Miner capabilities.** Catalogue drivers, API flags, per-algorithm fees and
  optional `--list-algorithms`/`--help` probing replace per-miner Rust branches.
  Arbitrary newer miner versions can be installed without an app update.
- **Report issue.** Error details and Settings can open a prefilled GitHub issue
  with redacted diagnostics.

### Changed

- **Cancel in the GPU tuning dialog is a real cancel.** Stock and GPU-preset
  choices only change the dialog; Save is the only action that writes to the
  draft. “Recommended” and the immediate “Profile default” action are removed.
- **Temperature limit and resume are global** in `behavior`, not per GPU preset.
- **One active miner remains.** All supported miners can use several GPUs; the
  per-card data is ready for a future multi-session design.
- The dashboard replaces the Totals group with an Active preset row and a single
  Pool and shares group.

### Migration

- v1/v2/v3 configs are migrated on first load: each preset embeds the GPU
  profile it referenced; unreferenced profiles are dropped. The changelog note
  is intentional: named v3 profiles were never selectable from the GUI.

### Added

- **New Adwaita app icon and tray artwork.** The main icon is installed as a
  scalable SVG plus 32–512 px PNGs; the tray ships a monochrome symbolic icon and
  a colour icon at 16–32 px.
- **Background tray in `trommeld`.** A StatusNotifierItem menu with a status
  header, Start/Stop mining, Show window and Quit. It works on KDE Plasma
  natively and on GNOME through the AppIndicator extension, survives a closed
  GUI and is disabled by `TROMMEL_NO_TRAY=1` in tests.
- **Tray quit and show-window paths.** Quit stops only Trommel's miner, restores
  the snapshot, asks an open window to close through `QuitRequested` and exits;
  Show window launches or presents the GTK app.
- **Tray settings.** `behavior.tray_monochrome` (default `true`, live) selects
  the symbolic or colour icon, and **Start Trommel at login** (default on)
  enables `trommeld.service` through systemd. The Debian package enables it for
  the installing user on first install; upgrades keep a later opt-out.

### Changed

- **The monochrome tray glyph matches the colour drum.** It keeps the same
  circular screen, feet and nugget, but paints them in one light ink. The old
  `#2e3436` fill was loaded as an ordinary pixmap by some tray hosts and never
  recoloured, so it disappeared into a dark panel; the light fill is deliberate.
- **The Adwaita artwork is 15% larger inside its canvas.** The app icon now has
  the same optical weight as its dock neighbours without redrawing the drum.

### Fixed

- **Uptime ticks every second.** The dashboard advances the uptime from the last
  status sample once per second instead of showing it only on the 2–10 s poll.
  A cached daemon snapshot no longer rewinds the stopwatch between refreshes.
- **`install.sh` restarts `trommeld` after installing icons** and rewrites the
  per-user unit's `ExecStopPost` path, so the tray resolves the new artwork and
  the emergency-stop hook still runs.

## [0.8.2] - 2026-10-30

### Fixed

- **`SetConfig` no longer restarts a running miner.** The compatibility wrapper
  applies live and returns the restart-required error for a change that needs a
  restart; a client must pass an explicit `restart` or `stop` mode to
  `ApplyConfig`.
- **Installed versions sort numerically.** `0.51.10` is listed after `0.51.9`,
  and `update_available` is true only when the catalogue latest is numerically
  newer than the target and not installed yet.
- **An unreadable binary is reported as `not_readable`** instead of being folded
  into `not_executable`, both in the JSON diagnostics and in the GUI.
- **A downloaded release bundle verifies from any checkout.**
  `verify-release-bundle.sh` reads the version from the bundle filenames instead
  of the workspace version.
- **The Debian build remaps paths through `CARGO_ENCODED_RUSTFLAGS`.** A
  checkout path containing spaces no longer corrupts the remap flags, and flags
  set by the caller are preserved.
- **`check-no-secrets.sh` inspects every match.** A line that names both the
  `/home/user` placeholder and a real account fails instead of passing.
- **CI checks the version on every run and requires an annotated tag.**
  `GITHUB_REF_NAME` is treated as a tag only for `refs/tags/*`, and the release
  workflow's write token is limited to the publishing job.
- **The e2e harness records the real detached worker PIDs**, so the external
  miner checks and the final cleanup address the process the test started.
- **Docs and the RPM template.** One canonical release checklist, a precise
  `CUDA_DEVICE_ORDER` rule, a security contact and the untagged 0.4.1-0.4.4
  note; the template version matches 0.8.2.

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

- **Miner version manager (B4).** The miner library installs, updates, verifies
  and removes global `miner@version` installs, shows the presets that pin a
  version, refuses to remove a pinned version, and keeps older versions for
  rollback. Installing a new version never rewrites a preset.
- **Preset rollback.** A **Preset versions** group switches exactly one preset
  to another installed version; cancelling restores the previous choice.
- **`trommel miner verify` and `trommel miner remove`.** Verify reports the real
  binary and a recorded digest only; remove deletes an unused version and its
  managed files.

## [0.6.0] - 2026-10-29

### Added

- **Miner library dialog (B3).** The catalogue, installed versions, preset
  usage, missing-version hints and the benchmark entry moved into one dialog
  opened from Presets and the preset editor.
- **Preset-centred navigation.** Top-level pages are Dashboard, Presets and
  Settings; the separate Miners page and its benchmark group were removed.

### Removed

- **The GNOME Shell extension.** Desktop notifications moved into the daemon;
  the `Notification` D-Bus signal remains. See
  [ADR 0004](docs/decisions.md#adr-0004).

## [0.5.0] - 2026-10-29

### Added

- **Per-GPU participation (B2).** A card switched off in the GPU profile is
  excluded from the miner instead of silently hashing. WildRig, SRBMiner and
  OneZero use documented device flags; a miner without one refuses a partial
  rig. See [ADR 0007](docs/decisions.md#adr-0007).
- **Documented device-selection flags** in the catalogue and the dashboard
  efficiency row (J/TH).

## [0.4.5] - 2026-10-29

### Added

- **Bounded restart with backoff (B1b).** A crashing miner is restarted at most
  N times, then stop and restore the snapshot.
- **A guard process for manual runs (B1b).** The daemon starts a hidden guard
  that runs emergency-stop if the daemon disappears.
- **Benchmark safety (B1b).** A benchmark refuses to start while an external
  miner runs and restarts only the miner this session owns.

## [0.4.4] - 2026-10-29

### Added

- **Fault-tolerant session ownership (B1a).** Idempotent `trommel
  emergency-stop`, startup recovery of an orphaned miner and an applied
  snapshot, an OC file lock and helper lease, and restore on a lost session bus
  or stopped systemd unit.
- **Schema v3** with the applied config, presets and global miner installs; the
  v1 migration is one-time and explicit.

## [0.4.3] – [0.4.1] — 2026-10-28

Packaging, tray and GPU-tuning fixes and small additions on the way to the first
tagged release line; see the commit history for the individual changes.

## [0.4.0] — 2026-10-28

First functional release: dashboard, presets, miner catalogue, power/clock
tuning with stock restore, D-Bus daemon, CLI and `.deb` packaging.

## [0.3.0] and [0.2.0] — 2026-10-28

Early previews of the config model, catalogue and GTK shell. The history is kept
in git; no tagged artifacts were published for these versions.
