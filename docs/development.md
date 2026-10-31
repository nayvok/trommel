# Development

How to build, test, debug and release Trommel. Commands assume the repository
root; if your global cargo cache is read-only, prefix with
`CARGO_HOME="$PWD/.cargo-home"`.

## Prerequisites

```bash
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
  desktop-file-utils appstream gettext librsvg2-bin python3 \
  gir1.2-atspi-2.0 dbus xdg-utils at-spi2-core xvfb x11-utils
```

- Rust 1.92+ (edition 2021); this is the declared MSRV and CI checks it with
  the `gui` feature. GTK 4.22 / libadwaita 1.9 are required by the
  dependencies; Ubuntu 26.04 is the tested distribution.
- Python 3 is used by the test scripts and by packaging/release helpers.
- The UI smoke test needs an accessible desktop session (AT-SPI). It prefers
  the X11 backend through XWayland because the nested Wayland accessibility
  bridge exposes AdwDialog children late; set `TROMMEL_UI_WAYLAND=1` to run it
  on native Wayland. `scripts/atspi-bus.sh` first verifies that the session
  accessibility bus answers a real D-Bus call; a stale socket from a crashed
  launcher no longer fails the run, because the scripts then start a private
  accessibility bus and registry under their work directory. `AT_SPI_BUS_ADDRESS`
  still overrides both. CI runs both smokes headlessly with `xvfb-run`; the
  registry comes from `at-spi2-core`, and the private fallback fails with a
  named error when it is missing. The screenshot test needs a display too. Both
  fail closed with a clear message rather than silently passing.

## Build

```bash
cargo build -p trommel --features gui   # GUI + CLI
cargo build -p trommel                  # CLI only (no GTK)
cargo build -p trommeld                 # daemon
cargo build --workspace                 # everything, default features
```

The `gui` feature is additive and optional. A CLI-only build must keep working;
do not move GTK types into `trommel-core` or the daemon.

## Test matrix

| Command | What it proves | Needs |
| --- | --- | --- |
| `cargo test --workspace` | Unit tests: config/migration, args, parsers, OC validation, status, D-Bus helpers | Rust only |
| `cargo test -p trommel --features gui` | GUI-feature unit tests, including the screenshot-hook/private-instance pairing | GTK dev libs |
| `cargo clippy --workspace --all-targets --features trommel/gui -- -D warnings` | No new lints, including GUI code | GTK dev libs |
| `cargo fmt --check` | Formatting | Rust only |
| `cargo build -p trommel` | The CLI-only build keeps working (no GTK types leak into core/daemon) | Rust only |
| `./scripts/e2e.sh` | Full CLI lifecycle on a private bus with fake GPU, helper and miner, including per-GPU participation (device arguments, excluded card untouched, restore, refusal), SIGKILL/emergency-stop, startup recovery, helper lease, partial apply, restore retry, custom binary, benchmark external guard, restart budget, watchdog restart drill, guard, lost session bus and the refused `SetConfig` restart | Python 3, dbus |
| `./scripts/test-oc-helper.sh` | The real Bash helper against the stateful fake `nvidia-smi` | bash |
| `./scripts/test-mcp.sh` | MCP protocol, read-only default, write gating, redaction | Python 3 |
| `./scripts/ui-smoke.sh` | Critical GUI path through AT-SPI (first-run preset creation, start/stop, settings, editor, tuning, install, version manager) | desktop session or Xvfb, `at-spi2-core`; XWayland preferred |
| `./scripts/first-run-smoke.sh` | Pristine install: no config, no wallet, no miner, one fake GPU | desktop session or Xvfb, `at-spi2-core` |
| `./scripts/check-no-secrets.sh` | No wallet-like string, secret marker or personal home path in tracked files | git |
| `./scripts/check-polkit.sh` | Action id and helper path consistency | none |
| `./scripts/check-docs.sh` | Relative markdown links resolve; no references to removed docs | none |
| `./scripts/package-deb.sh` + `./scripts/test-deb.sh` | Package builds; every documented path, mode, maintainer script, polkit annotation and a path-neutral binary are present | dpkg-dev |
| `./scripts/test-deb-install.sh` | Install, upgrade and remove: per-user rule written/kept/removed, user config preserved | root in a disposable container |
| `./scripts/check-version.sh` | Workspace version matches `Cargo.lock`, `CHANGELOG.md` and metainfo; a tag is annotated and sits on HEAD | git, python3 |
| `./scripts/verify-release-bundle.sh` | Bundle has the expected assets; `SHA256SUMS` covers the debs and installer; sidecars verify | built bundle |
| Asset validation | `desktop-file-validate`, `appstreamcli validate` | those tools |

CI (`.github/workflows/ci.yml`) runs secrets, docs, format, clippy with `gui`,
workspace tests, GUI-feature tests, the CLI-only build, the root helper, e2e,
MCP, asset validation, the headless UI smokes (`.github/workflows/ui.yml`), the
MSRV check and a packaging job (layout + install/upgrade/remove) on Ubuntu
26.04. The release workflow runs the same matrix, verifies the tag/version and
the release bundle, and then publishes. The manual matrix and the real-rig
checks below remain manual because they need a desktop session and hardware.

### Manual and real-rig checks

CI proves the headless widget tree, the click path and the fake-rig lifecycle.
These checks still need a desktop session, a real driver or a real pool. The
reference rig is an RTX 3080 Ti + RTX 3060 on GNOME 50 / Ubuntu 26.04 / Wayland.

| Surface | Check |
| --- | --- |
| Dashboard | Refresh, Start/Stop, participation switch, Tune; Cancel restores the switch and the dirty banner; Start is disabled with every card excluded or without a device flag; the chip and Rig summary show mining/excluded |
| Layout | One card spans the row; two split it; three or more wrap; stopped hashrate/balance/shares are `—` unless a real zero was reported |
| Telemetry | Per-card hashrate and `J/TH`/`J/MH` efficiency; an unmapped record is `—`, the total is not divided |
| Reliability | Watchdog off: one switch, dependent rows hidden; notify: one notification and no PID change; restart: one bounded restart through the normal path; limit stops mining; external miner untouched |
| History | 1h/24h/7d ranges, card selector, CSV; samples survive a daemon restart; empty state before the first minute; no wallet/pool data |
| First run | Empty dashboard, Create preset, Start prompt, Tuning unavailable; Cancel an unsaved new preset leaves no artifact |
| Presets/editor | Row, chevron, New/Use, missing-version Install…; Save/Cancel/Duplicate/Delete; shared library opens from both entries |
| Miner library | Catalogue, preset usage, installed/missing binaries, recorded checksum only, verify chip, blocked removal listing presets, one-preset version switch with cancel, sequential benchmark |
| Settings | Switches, temperature limit/resume, Save and apply, Copy diagnostics, Restore stock targets; live apply; redaction |
| GPU tune | Now row, Cancel, GPU presets (Stock/saved/Save as/Manage), locks, Similar/All, restart/stop alert; Cancel leaves no dirty state |
| Alerts/errors | Apply and restart/stop/Cancel; delete confirmation; a long daemon error shows Details, selectable text and Copy |
| Window | ~400 px: no clipping, readable bubbles/actions, header switcher falls back to the title and bottom bar |
| Tray | GNOME (AppIndicator) and KDE Plasma: status header, Start/Stop, Show window, Quit; mono/colour switch; no host means no tray and no crash; login autostart matches the Settings switch |
| Installer | `install.sh`, `.deb` postinst, release installer: binaries, helper, rule, units, no password prompt for the installing user |
| Real rig | polkit prompt-free, e.g. 300 W/1620/810, start/stop, restore; an ignored memory lock stops the miner with a named error |
| Error paths | Missing helper/miner, external miner, failed restore, cancelled auth: Start aborts or explains, snapshot kept on failed restore, no silent success |
| Crash/shutdown | `kill -9 trommeld` restores via systemd `ExecStopPost` or the guard; `kill -9 trommel` leaves mining and uptime running; unit restart/logout restores; boot-id mismatch drops a stale snapshot |
| Bus loss | Kill the session `dbus-daemon`: the daemon restores and exits, the miner is gone |

Fault-tolerance drill on a real rig, once per release:

1. `trommel start`; note `nvidia-smi --query-gpu=power.limit,clocks.mem
   --format=csv`. Set a non-default limit (e.g. 320 W) so the restore is visible.
2. `systemctl --user restart trommeld`: the unit stops/restores, the restarted
   daemon reports stopped, `nvidia-smi` shows the stock limit; start again.
3. `kill -9 $(pgrep -f target/release/trommeld)` (manual run or a unit without
   `ExecStopPost`): the guard runs emergency-stop; the miner is gone, the
   snapshot absent, the stock limit back.
4. Kill only the GUI (`kill -9 $(pgrep -f 'trommel$')`): `trommel status --json`
   still reports running, the miner PID is unchanged, no new `oc reset` in
   `daemon.log`.
5. Log out or reboot while mining: the unit stop restores; after login the
   dashboard is stopped. After a reboot the boot-id check drops the snapshot.
6. Break the restore (a helper that fails `reset`): Stop and emergency-stop
   return non-zero, `oc-snapshot.json` and `last_error` remain, the UI banner
   stays. Restore the helper and confirm `trommel emergency-stop` clears it.

Checks that cannot run in CI:

- real GPU snapshot/restore (`--pl`, `--lgc`, `--lmc`, reset) and the driver's
  read-back;
- polkit without a password in an active graphical session;
- Wayland rendering, fonts, theme and screenshots;
- the live pool API (unit-tested cache/backoff only);
- StatusNotifierItem rendering and recolouring on GNOME and KDE;
- a real SIGKILL/restart during a polkit prompt;
- systemd user-manager stop, logout and reboot;
- downloading and installing the published release from a clean machine.

Known harness limits:

- `AdwSpinRow` values and some `AdwExpanderRow` fields are not exposed by every
  AT-SPI build, so the UI smoke changes a profile through the core-lock switch
  and reports those widgets as an accessibility limitation.
- `GtkMenuButton` (“Presets” in the tuning dialog) is not always exposed as an
  action; the smoke uses Cancel/local changes instead of the popover.
- Device selection is checked against a fake miner; a real heterogeneous rig
  still needs a benchmark.
- Native file choosers, the About dialog and a lingering closed dialog are
  manual.
- Xvfb proves the widget tree and click path, not the compositor; screenshots
  need a real display.

Report a regression with `trommel status --json`, `trommel diagnose --json`,
the daemon journal, `nvidia-smi --query-gpu=name,driver_version,power.limit,power.default_limit
--format=csv`, the desktop session and the failing command. Redact wallets;
never include a seed phrase or private key.

### Test harness isolation

The scripts never touch a real rig:

- each script creates its own working directory (`/.e2e`, `/.uicheck`,
  `/.first-run-check`, `/.shots`) and sets `XDG_CONFIG_HOME`,
  `XDG_STATE_HOME`, `XDG_CACHE_HOME` and `XDG_DATA_HOME` inside it;
- a private `dbus-daemon` prevents an existing Trommel session from answering;
- a stateful fake `nvidia-smi` (`scripts/fixtures/fake-nvidia-smi.py`) is
  prepended to `PATH` and models a deterministic two-GPU rig, including
  `-pl/-lgc/-lmc/-rgc/-rmc`, so the daemon can verify read-backs;
- `TROMMEL_OC_HELPER` points at `scripts/fixtures/fake-oc-helper.sh`, which logs
  its arguments and forwards them to the fake `nvidia-smi`;
- `TROMMEL_NO_EXTERNAL_SCAN=1` stops the supervisor from adopting a real miner
  running on the same user;
- `TROMMEL_NO_GUARD=1` keeps daemon lifetimes predictable; guard and session-bus
  tests opt back in with `env -u`;
- unit tests get a per-process temporary `paths` root.

Never weaken this isolation to make a test pass.

### UI hooks

The GUI reads these environment variables for tests and screenshots:

| Variable | Effect |
| --- | --- |
| `TROMMEL_COLOR_SCHEME` | `dark`, `light` or anything else = System |
| `TROMMEL_WINDOW_WIDTH`, `TROMMEL_WINDOW_HEIGHT` | Override the initial size |
| `TROMMEL_SCREENSHOT` | Save a PNG of the main window, then quit |
| `TROMMEL_SCREENSHOT_PAGE` | Page name to switch to before capturing |
| `TROMMEL_SCREENSHOT_DELAY` | Milliseconds to wait before capturing |
| `TROMMEL_SCREENSHOT_SCROLL` | `bottom` scrolls the page before capture |
| `TROMMEL_OPEN_PRESET` | Open the preset editor on start |
| `TROMMEL_OPEN_LIBRARY` | Open the miner library dialog on start |
| `TROMMEL_OPEN_INSTALL` | Open the miner install dialog on start |
| `TROMMEL_OPEN_TUNE` | Open the GPU tuning dialog on start |
| `TROMMEL_OPEN_HISTORY` | Open the History dialog on start |
| `TROMMEL_AUTOSTART_STATE` | `on`/`off` forces the Settings login-autostart switch (deterministic screenshots) |
| `TROMMEL_TUNE_SCREENSHOT`, `TROMMEL_PRESET_SCREENSHOT`, `TROMMEL_LIBRARY_SCREENSHOT`, `TROMMEL_INSTALL_SCREENSHOT`, `TROMMEL_HISTORY_SCREENSHOT` | Capture a transient window |

Runtime/test variables used outside the GUI:

| Variable | Effect |
| --- | --- |
| `TROMMEL_OC_HELPER` | Path to an alternative helper (fake in tests) |
| `TROMMEL_OC_LOG` | Where the fake helper writes arguments (test scripts) |
| `TROMMEL_OC_TIMEOUT_MS` | Override the 180 s helper/auth timeout (tests) |
| `TROMMEL_OC_VERIFY_MS` | Delay before the under-load clock check (tests) |
| `TROMMEL_FAKE_GPU_STATE` | JSON state file for the fake `nvidia-smi` |
| `TROMMEL_FAKE_MODE` | `ignore_pl`, `ignore_lmc`, `fail_rgc`, … for the fake |
| `TROMMEL_NO_EXTERNAL_SCAN` | Disable the `/proc` external-miner scan |
| `TROMMEL_NO_GUARD` | Do not start the guard (deterministic test daemon lifetimes) |
| `TROMMEL_NO_TRAY` | Do not create the StatusNotifierItem tray (tests and screenshots) |
| `TROMMEL_MAX_RESTARTS` | Override the consecutive restart budget (default 5) |
| `TROMMEL_WATCHDOG_WINDOW_S` | Override the watchdog window for e2e (default from config) |
| `TROMMEL_HISTORY_INTERVAL_S` | History sampling interval for e2e (default 60) |
| `TROMMEL_GUARD_BIN` | Path to the `trommel` binary the daemon uses for its guard |
| `TROMMEL_SKIP_BUILD` | Test scripts reuse an existing build |
| `TROMMEL_VERSION` | Override the version expected by `test-deb.sh` |

## Screenshots

```bash
./scripts/screenshots.sh
```

Builds debug binaries, creates a stateful fake two-GPU rig, starts a private
daemon and lets **the daemon start the recorded demo miner itself** (`trommel
start`), so the captures show an owned mining session rather than an adopted
external process; the fake OC helper is the same one the e2e harness uses and
the dashboard measurements are animated from the fake rig state. Every surface
is rendered into `docs/images/`. The script uses the hooks above, so a UI change
should be visible there without manual work. Regenerate screenshots whenever the
user guide references a changed surface; never commit screenshots from another
machine or a different font stack.

## Debugging

```bash
# Is the daemon alive and what does it report?
systemctl --user status trommeld
journalctl --user -b -u trommeld --no-pager
trommel status --json | python3 -m json.tool
trommel diagnose --json > /tmp/trommel-diag.json

# Inspect the D-Bus interface
gdbus introspect --session --dest io.github.nayvok.Trommel.Daemon \
  --object-path /io/github/nayvok/Trommel
gdbus call --session --dest io.github.nayvok.Trommel.Daemon \
  --object-path /io/github/nayvok/Trommel --method io.github.nayvok.Trommel.Status

# Logs and state
tail -f ~/.local/state/trommel/miner.log
tail -f ~/.local/state/trommel/daemon.log
cat ~/.local/state/trommel/state.json

# GPU and helper
nvidia-smi --query-gpu=uuid,name,power.limit,power.default_limit --format=csv
sudo pkexec /usr/libexec/trommel/trommel-oc status --gpu GPU-…
```

Common causes:

- **Start does nothing** — a stale `state.json` or an external miner. Check
  `trommel status --json` (`state.external`, `state.last_error`) and the daemon
  log; the GUI should explain both.
- **OC prompts or hangs** — the polkit rule does not match the annotated action.
  Run `./scripts/check-polkit.sh` and reinstall the rule with
  `sudo ./scripts/install-polkit-rule.sh "$USER"`.
- **Miner installed but Start says missing** — the daemon did not receive the
  install. `trommel miner list` then `trommel config apply` (or restart the
  service); the CLI should apply automatically.
- **Pool balance never appears** — the API is cached for 60 s and backed off for
  45 s after a failure; a non-PearlHash pool shows configured data only.
- **No GPU in a container** — `/dev/nvidia*` and `nvidia-smi` are host devices;
  the GUI/daemon cannot work inside a sandbox without them.
- **Snapshot kept after a crash/stop** — the restore failed or
  `restore_stock_on_stop` is off. Read `oc.last_error`, fix the helper/auth
  issue and run `trommel emergency-stop` (idempotent); do not delete
  `oc-snapshot.json` by hand.
- **State says running but nothing mines** — a PID was reused or the binary was
  replaced. `State::miner_alive` compares `state.miner_path`, so Stop and
  emergency-stop refuse to signal a mismatched PID; restart the daemon to run
  startup recovery.

## Adding a feature

1. Put model/logic in `trommel-core` with unit tests; keep the daemon and GUI
   thin.
2. If the config shape changes, update `docs/schema/config.schema.json`,
   `normalize()` and add a migration/default test.
3. If the D-Bus surface changes, update `MinerProxy`, the CLI/MCP callers and
   [architecture.md](architecture.md#status-contract).
4. If a status field is added, render it in the GUI and the CLI, or explicitly
   leave it unrendered on purpose.
5. Add the user-visible strings in both languages (`tr(en, ru)`).
6. Extend `e2e.sh` for lifecycle behaviour and `ui-smoke.sh`/`first-run-smoke.sh`
   for visible interaction.
7. Update the relevant document ([docs/README.md](README.md) says which) in the
   same commit.

## Working in blocks

The roadmap is split into B0–B7 ([roadmap.md](roadmap.md#block-plan)). A block
is the unit of review and release:

1. **Branch.** `feat/<block>-<topic>` from the current mainline. Do not mix the
   next block into the branch, even when it is tempting.
2. **Scope freeze.** Write down the block's acceptance criteria (the roadmap row
   and the ADRs it implements). Anything else is a follow-up, not a drive-by.
3. **Design first.** Update or check the ADR before code; accepted ADRs are not
   drafts. B1 is governed by [ADR 0005](decisions.md#adr-0005) and
   [ADR 0006](decisions.md#adr-0006).
4. **Code + tests.** Core logic in `trommel-core` with unit tests; lifecycle
   behaviour in `scripts/e2e.sh` with the fake rig. Crash paths (SIGKILL,
   partial apply, failed restore) need an automated test, not a comment.
5. **Docs in the same commit.** Architecture/development/decisions, agents,
   changelog and the roadmap status. A feature is not done while one of those
   still describes the old behaviour.
6. **Screenshots.** Only regenerate them when the UI actually changed; do not
   touch them for daemon/CLI blocks.
7. **Package and tag.** Bump the workspace version, run
   `./scripts/check-version.sh`, `./scripts/package-deb.sh`,
   `./scripts/test-deb.sh` and `./scripts/test-deb-install.sh`, then follow the
   [release checklist](#release-checklist). Tag `vX.Y.Z` only after
   the whole matrix is green.

If the session runs out of time, stop after the last green sub-block, record the
unfinished part in the roadmap and do not start the next block.

## Packaging and releases

Trommel is distributed only through GitHub Releases as a Debian package. There
is no apt repository and no PPA; upgrades are new release downloads.

### Formats

| Format | Status | Why |
| --- | --- | --- |
| `.deb` | **Released** on GitHub | Needs host `nvidia-smi`, a polkit helper and a systemd user unit |
| `.rpm` | Template only (`packaging/rpm/trommel.spec.in`) | Needs a Fedora/openSUSE install/upgrade/removal test |
| AppImage | Deferred | A read-only mount cannot install the daemon, helper or user unit; a portable tarball plus an explicit host-helper installer is the fallback |
| Flatpak | Deferred | The sandbox lacks `/dev/nvidia*` and `/proc/driver/nvidia`, cannot use the host `nvidia-smi`, the polkit helper or the user unit, and downloads miners at runtime; a GUI-only Flatpak would be a different product |

Do not publish an RPM/AppImage/Flatpak artifact until its integration story is
tested. The reasoning is in [ADR 0003](decisions.md#adr-0003).

### Release assets

A tag `vX.Y.Z` triggers `.github/workflows/release.yml`. It runs the full CI
matrix (including the headless UI smokes and MSRV), verifies the tag against the
workspace version, builds and verifies the bundle, then publishes:

```
trommel_X.Y.Z_amd64.deb      versioned package
trommel_amd64.deb            stable alias for releases/latest/download
trommel_X.Y.Z_amd64.deb.sha256
trommel_amd64.deb.sha256
trommel-install.sh           verifies the SHA-256, then apt installs
SHA256SUMS                   both debs and trommel-install.sh
```

Locally:

```bash
./scripts/package-deb.sh            # dist/trommel_X.Y.Z_amd64.deb + alias + .sha256
./scripts/release-bundle.sh         # dist/release/*, then verifies it
./scripts/test-deb.sh               # full layout validation
./scripts/test-deb-install.sh       # install/upgrade/remove (root, disposable container)
./scripts/verify-release-bundle.sh  # assets, sidecars and SHA256SUMS
```

Users install with `sudo apt install ./trommel_amd64.deb`, or run the release
`trommel-install.sh`, which verifies the checksum first. `apt remove trommel`
keeps the user config.

### Debian package layout

```
/usr/bin/trommel
/usr/bin/trommeld
/usr/libexec/trommel/trommel-oc                  root helper (0755)
/usr/share/polkit-1/actions/io.github.nayvok.trommel.policy
/usr/share/polkit-1/rules.d/50-trommel.rules     generic fallback
/usr/share/trommel/50-trommel.rules.in           per-user template
/usr/lib/systemd/user/trommeld.service
/usr/lib/systemd/user/trommel-autostart.service
/usr/share/dbus-1/services/io.github.nayvok.Trommel.Daemon.service
/usr/share/applications/io.github.nayvok.Trommel.desktop
/usr/share/metainfo/io.github.nayvok.Trommel.metainfo.xml
/usr/share/icons/hicolor/{scalable,symbolic}/apps/io.github.nayvok.Trommel*.svg
/usr/share/icons/hicolor/<size>/apps/io.github.nayvok.Trommel*.png
```

Dependencies: GTK4, libadwaita, glib, pkexec/polkit, user D-Bus and systemd;
the NVIDIA driver is recommended. The tray uses `ksni` in-process and adds no
GNOME dependency. `postinst` refreshes desktop/icon caches and writes the
per-user polkit rule for `SUDO_USER`, so GPU tuning does not prompt. On the
first install it enables `trommeld.service` through a
`graphical-session.target.wants` symlink; upgrades preserve a user's choice.
`postrm` removes the rule and the symlink; `prerm` does not stop the miner.
`scripts/test-deb.sh` asserts every path and mode, the polkit annotation and a
path-neutral binary; `scripts/test-deb-install.sh` covers
install/upgrade/remove and config preservation in a disposable root container.

### Developer install

```bash
./install.sh             # build, install user binaries, helper and service
./install.sh --no-build  # use target/release
./install.sh --no-helper # skip the polkit helper (no OC)
```

`install.sh` installs to `~/.local/bin`, `~/.config/systemd/user`,
`~/.local/share/{applications,icons,metainfo,dbus-1/services}`, puts the helper
at `/usr/libexec/trommel/trommel-oc` and installs the per-user polkit rule
through `scripts/install-polkit-rule.sh`. It restarts `trommeld` so an upgrade
does not leave the previous process running. Remove with `./uninstall.sh`
(keeps config) or `./uninstall.sh --purge` (removes config, state and
benchmarks); it refuses to remove the helper while a miner is running.

### CI workflows

| Workflow | Trigger | Checks |
| --- | --- | --- |
| `ci.yml` | push to `main`, pull request | secrets, docs links, fmt, clippy with `gui`, tests, GUI-feature tests, CLI-only build, e2e, MCP, release build, desktop/metainfo validation, package layout + install/upgrade/remove, MSRV |
| `ui.yml` | called by `ci.yml` and `release.yml` | headless `ui-smoke.sh` and `first-run-smoke.sh` under Xvfb with a private AT-SPI bus |
| `package.yml` | manual | builds the `.deb` and validates its layout |
| `release.yml` | tag `v*` | the CI matrix plus polkit check, tag/version check, bundle verification, install/upgrade/remove, GitHub Release upload |

### Versioning

One version for the whole workspace (`Cargo.toml` `[workspace.package]
version`). `./scripts/check-version.sh [vX.Y.Z]` checks it against `Cargo.lock`,
`CHANGELOG.md`, the metainfo `<release>` and, for a tag build, that the
annotated tag names it and points at `HEAD`. Update the changelog and metainfo
entry with every release.

### Release checklist

Run from a clean tree.

1. **Version.** Bump `version` in `Cargo.toml`; add a dated `CHANGELOG.md` entry
   and a metainfo `<release>`; bump `Version:` in
   `packaging/rpm/trommel.spec.in`. Build once so `Cargo.lock` picks up the
   version, then `./scripts/check-version.sh` must pass.
2. **Matrix.** `cargo fmt --all -- --check`, clippy with `gui`, workspace tests,
   GUI-feature tests, the CLI-only and GUI builds, `cargo build -p trommeld`,
   `./scripts/test-oc-helper.sh`, `./scripts/e2e.sh`, `./scripts/test-mcp.sh`,
   both UI smokes (real desktop for a release), `./scripts/check-docs.sh`,
   `./scripts/check-no-secrets.sh`, `./scripts/check-polkit.sh`,
   `desktop-file-validate` and `appstreamcli validate`. Regenerate screenshots
   only if the UI changed.
3. **Package.** `./scripts/package-deb.sh`, `./scripts/test-deb.sh`,
   `./scripts/test-deb-install.sh` (root, disposable container).
4. **Bundle.** `./scripts/release-bundle.sh` builds `dist/release` and verifies
   it; `(cd dist/release && sha256sum -c SHA256SUMS)` must pass.
5. **Commit and tag.** Commit everything, tag the last commit with an annotated
   `vX.Y.Z`, push the branch and tag. Do not tag an intermediate commit. If the
   workflow fails, fix on the branch, move the tag and push again; delete a
   remote tag only if nothing was published and that is agreed.
6. **Watch the workflow** re-run the matrix, verify the tag/version and bundle,
   and publish only if they pass.
7. **Verify the published release.** Download every asset and run
   `./scripts/verify-release-bundle.sh <dir>`; check the `.deb`, both `.sha256`
   sidecars, `trommel-install.sh` and `SHA256SUMS`, and that
   `releases/latest/download/trommel_amd64.deb` resolves. Install once from a
   clean machine.

## Internationalization

English is the source string; Russian is a translation kept in the same call
site. There is no gettext catalogue yet.

```rust
use trommel_core::i18n::{tr, tr_ref, set_language, Language};

let text = tr("Start", "Запустить");
let static_label = tr_ref("System", "Системная");
set_language(Language::Ru);
```

- `Language::{System, En, Ru}`; `System` reads `LANGUAGE`, `LANG` or `LC_ALL`
  and falls back to English unless the value starts with `ru`.
- `set_language` updates a process-global `AtomicU8`; the GUI calls it at start
  and the daemon when an applied config changes the language.
- `tr(en, ru)` returns `String` (safe for `format!`); `tr_ref(en, ru)` borrows a
  `'static` translation. Never concatenate translated fragments.
- `behavior.language` takes effect after an app restart (the Settings row says
  so). The daemon applies it in `ApplyConfig` so notifications/status follow.
- Catalogue `*_en`/`*_ru` fields are both required; accessors on `MinerDef`,
  `MinerOption`, `CoinDef` and `PoolDef` choose the language.
- The GUI, desktop notifications and daemon messages are bilingual. Human CLI
  output is Russian-only; `--json` is language-neutral and is the stable
  interface. Some daemon log lines are Russian by design.
- The gettext migration (planned, not shipped): add `gettext-rs` with
  `gettext-system`, wrap strings, extract with `xtr`, compile `po/ru.po` to
  `/usr/share/locale/ru/LC_MESSAGES/trommel.mo`, add `translation
  type="gettext"` to metainfo, never commit `.mo` files. Until then every new
  user-visible string is added in both languages.
