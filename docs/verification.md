# Verification

What is proven automatically, what still needs a desktop session and what still
needs a real rig. Run the commands from the repository root; see
[development.md](development.md) for prerequisites and harness isolation.

## Automated

| Command | Covers |
| --- | --- |
| `cargo test --workspace` | Config defaults/migration/diff/warnings, miner argument rendering, documented device flags and per-driver device lists, device-capability refusal, participation status/warnings, fee handling, catalogue parsing, `nvidia-smi` parsing, OC argument validation, supervisor/state/status helpers, D-Bus helpers, hashrate formatting, download/path safety, miner version manager (usage, update plan, blocked removal, managed-file deletion, single-preset pin, verify without a fabricated checksum) |
| `cargo test -p trommel --features gui` | GUI-feature unit tests; the screenshot-hook test proves every `TROMMEL_*_SCREENSHOT` capture runs as a private instance |
| `cargo clippy --workspace --all-targets --features trommel/gui -- -D warnings` | Lints in all crates including GUI code |
| `cargo fmt --check` | Formatting |
| `cargo build -p trommel` | The CLI-only build still links, so no GTK type leaked into `trommel-core`/the daemon |
| `./scripts/e2e.sh` | CLI lifecycle on a private bus: start/stop/toggle, config v3, per-card inheritance vs custom overrides, apply modes, power read-back, ignored power/clock locks, restore snapshot/defaults, restore-on-stop off, helper timeout, snapshot/restore, stale external state. Fault-tolerance: SIGKILL + emergency-stop, startup recovery of an orphan, helper lease (live/replaced/dead), partial apply with and without a failed rollback, failed restore retried by startup recovery, custom-named binary stopped by path, external miner left alive, benchmark refusal, restart budget, guard, and a lost session bus. Per-GPU participation: one excluded card means the miner receives only `--gpu-list 0`, the excluded card is neither tuned nor restored, a previously tuned excluded card is returned to stock, emergency-stop touches only the participating card, and a miner without a documented device flag refuses the partial rig without spawning. Version manager: installing an older build does not rewrite a preset, `miner verify` reports the real binary and only a real digest, a pinned version cannot be removed, an unused version is deleted with `--yes`, and diagnostics report the remaining install. A stub `org.freedesktop.Notifications` service (Python/Gio; the step is skipped when `python3-gi` is missing) proves the daemon sends desktop notifications without a shell applet. A failure prints every harness artifact, `daemon.log`, `miner.log` and the `✗` lines through `scripts/e2e-diagnostics.sh` |
| `./scripts/test-oc-helper.sh` | The real Bash helper against a stateful fake `nvidia-smi`: apply/reset, ignored power rejection, failing `-rgc`, range/UUID validation |
| `./scripts/test-mcp.sh` | MCP initialize/ping/tools/list/tools/call, read-only default, `--allow-write` gating, rejected arguments, wallet redaction, installed-version status and absence of invented checksums |
| `./scripts/ui-smoke.sh` | AT-SPI click-through with a fake two-GPU rig: quick setup, start/stop, GPU participation switch (excluded chip and Totals), settings, preset editor, GPU tuning, miner install, version manager (installed row, verify chip, blocked removal alert, preset version switches), alerts, and SIGKILLing the GUI while mining continues. CI runs it headlessly with `xvfb-run` and a private AT-SPI bus |
| `./scripts/first-run-smoke.sh` | Pristine install: empty XDG dirs, no config, no wallet, no installed miner, one fake GPU; dashboard placeholders, wizard, single-GPU tuning. Also headless in CI |
| `./scripts/check-polkit.sh` | Polkit action id and helper path match the code |
| `./scripts/check-no-secrets.sh` | No wallet-like strings, secret markers or personal home paths in tracked files (`/home/user` is the only placeholder) |
| `./scripts/check-docs.sh` | Relative markdown links resolve; removed docs are not referenced; docs are reachable from the index |
| `./scripts/package-deb.sh` + `./scripts/test-deb.sh` | Package builds; every path documented in [packaging.md](packaging.md) exists with the right mode, the maintainer scripts and polkit annotation are present, and the binaries contain no builder home path |
| `./scripts/test-deb-install.sh` | Install/upgrade/remove in a disposable root container: `postinst` writes the per-user rule, an upgrade keeps it, remove/purge deletes it, and the user config survives all three |
| `./scripts/check-version.sh` | Workspace version matches `Cargo.toml`, `Cargo.lock`, `CHANGELOG.md` and the metainfo release, and a supplied tag names it and sits on HEAD |
| `./scripts/verify-release-bundle.sh` | The bundle has exactly the documented assets, both sidecars verify, `SHA256SUMS` covers the debs and `trommel-install.sh`, and the stable alias equals the versioned package |
| Asset validation | `desktop-file-validate`, `appstreamcli validate` |

CI (`.github/workflows/ci.yml`) runs secrets, docs, fmt, clippy with `gui`,
workspace tests, GUI-feature tests, the CLI-only build, the root helper, e2e,
MCP, asset validation, the headless UI smokes (`.github/workflows/ui.yml`), the
MSRV check and a packaging job (layout + install/upgrade/remove) on Ubuntu
26.04. The release workflow runs the same matrix, verifies the tag/version and
the bundle, and publishes. The real desktop session is still needed for the
interactive matrix below and for screenshots; the headless job proves the
widget tree and the click path, not compositor behaviour.

## Interactive matrix

The table records what was last checked manually on the reference RTX 3080 Ti +
RTX 3060 rig, GNOME 50 / Ubuntu 26.04 / Wayland. It is a regression checklist,
not a claim that CI covers it.

| Surface | Action | Expected |
| --- | --- | --- |
| Dashboard | Refresh, Start/Stop, GPU participation switch, Tune | Live status; Cancel in the alert restores the switch and the dirty banner; Start is disabled with every GPU excluded or when the miner cannot select devices; the card chip and Totals show mining/excluded |
| Dashboard layout | one GPU, then two GPUs | One card spans the row with a 3×2 bubble grid; two cards split the row evenly; three or more wrap |
| Dashboard | stopped state | `0 TH/s`, numeric pool balance, configured endpoint named, no lone dashes |
| Quick setup | Cancel, Next, Back, Apply 80 %, Use auto, Start mining | Three steps, validation, per-card values, start only after confirmation |
| First run | Start with no wallet/miner | Opens Quick setup; miner marked not installed; single-GPU tune has no “Apply to” |
| Presets | row, chevron, New preset, Use, missing-version **Install…** | Editor opens; chevron is a real button; Use applies the selection; Install… opens the library focused on the preset |
| Preset editor | Save, Cancel, Duplicate, Delete…, Miner library… | Apply, confirmation, shared library and install dialog |
| Miner library | Presets page and editor entry, miner rows, version rows, Verify, Remove, preset switches, benchmark, Close | Catalogue, preset usage, installed and missing binaries, recorded checksum only, verify chip, blocked removal alert listing presets, unused removal confirmation, one-preset switch with cancel restoring the combo, sequential benchmark with a winner |
| Settings | switches, temperature limit/resume, Save and apply, Copy diagnostics, Restore stock | Aligned pill buttons, live apply, redaction, restore dialog with snapshot/defaults targets, stop-first guard |
| GPU tune | Now row, **⋯** menu (Recommended / Profile default), Save, locks on/off, Similar/All GPUs | Value rows hidden while off; Apply-to only with >1 GPU; restart/stop dialog |
| Alerts | Apply and restart / Apply and stop / Cancel, delete confirmation | Explicit restart/stop; Cancel reverts optimistic changes |
| Errors | force a long daemon error | Toast shows **Details**; the dialog text is selectable and **Copy** puts it on the clipboard |
| Menu | Open log, About, Quit | Opens the log/about, quits with `Ctrl+Q` |
| Window | resize to ~400 px | No clipping; GPU value bubbles and action buttons readable; the header switcher falls back to the title and the bar at the bottom |
| Notifications | A crash/error or an overheat event with **Behavior → Notifications** on | A desktop notification appears from `trommeld`; no shell extension is installed |
| Installer | `install.sh`, `.deb` postinst, release installer | Binaries, helper, rule, units, no password prompt for the installing user; the `.deb` install/upgrade/remove path is automated by `test-deb-install.sh` |
| Real rig | polkit prompt-free, 300 W/1620/810, start/stop, restore dialog | OC applied, ~125 TH/s, power read back, stock restore; an ignored memory lock stops the miner with a named error |
| Error paths | missing helper, missing miner, external miner, failed restore, cancelled auth | Start aborts or explains; snapshot kept on failed restore; no silent success; a 180 s auth dialog is not killed |
| Crash — daemon | `kill -9 trommeld` while mining (service and manual run) | systemd `ExecStopPost` or the guard restores PL/clocks; snapshot cleared; a later start does not resume mining |
| Crash — GUI | `kill -9 trommel` while mining | Mining and uptime continue; no restore, snapshot kept |
| Shutdown | `systemctl --user restart trommeld` while mining | SIGTERM path stops only our miner, restores stock; the restarted unit reports stopped |
| Logout/reboot | log out of GNOME or reboot without stopping | Unit stop restores stock; after reboot the boot-id mismatch drops the snapshot and no false restore runs |
| Bus loss | kill the session `dbus-daemon` in the e2e harness | Daemon restores and exits; the miner is gone |

## Fault-tolerance drill on a real rig

Run once per release on the reference rig; a fake GPU cannot prove the driver
accepted the restore.

1. `trommel start`; note `nvidia-smi --query-gpu=power.limit,clocks.mem
   --format=csv`. Set a non-default limit (e.g. 320 W) so the restore is visible.
2. `systemctl --user restart trommeld`: the unit stops/restores; the restarted
   daemon reports a stopped session and `nvidia-smi` shows the stock limit.
   Start again.
3. `kill -9 $(pgrep -f target/release/trommeld)` (manual run or a unit without
   `ExecStopPost`): the guard runs emergency-stop; wait a few seconds and check
   that the miner is gone, the snapshot is absent and the stock limit is back.
4. Kill only the GUI (`kill -9 $(pgrep -f 'trommel$')`): `trommel status --json`
   still reports running, the miner PID is unchanged and no new `oc reset`
   appears in `daemon.log`.
5. Log out (or reboot) while mining: the unit stop restores the snapshot; after
   login the dashboard is stopped. After a reboot an old snapshot is dropped by
   the boot-id check, never replayed.
6. Break the restore (temporarily install a helper that fails `reset`): Stop and
   emergency-stop return non-zero, `oc-snapshot.json` + `last_error` remain, and
   the UI banner stays. Restore the helper, run `trommel emergency-stop`, and
   confirm the snapshot is cleared.

## Manual checks that cannot run in CI

- **Real GPU snapshot/restore.** The harness fakes `nvidia-smi`; only a real
  card proves that `--pl`, `--lgc`, `--lmc` and the reset path work with the
  installed driver.
- **Polkit without a password.** Requires the rule installed for the desktop
  user and, typically, an active graphical session.
- **Wayland rendering, fonts and theme.** Screenshots from another machine or
  font stack are not comparable; regenerate them locally.
- **Real pool API.** The pool is external and rate-limited; the cache/backoff
  path is unit-tested, the live JSON is not.
- **Real SIGKILL/restart during a polkit prompt.** The helper lease is covered
  with the fake helper; a real `pkexec` dialog + daemon SIGKILL needs a desktop
  session and is checked manually with `nvidia-smi` before/after.
- **systemd/logout/reboot.** The unit file and `ExecStopPost` are packaged and
  reviewed, but the actual user-manager stop and session logout need a desktop
  session; the e2e harness has no systemd.
- **Published release download.** `verify-release-bundle.sh` checks the bundle
  before upload; downloading the published assets, running `sha256sum -c
  SHA256SUMS` and installing once from a clean machine is a manual release
  step ([packaging.md](packaging.md#release-checklist)).

## Known harness limits

- `AdwSpinRow` spinners are not exposed as AT-SPI values, so the UI smoke forces
  a profile change through the core-lock switch instead of typing a number.
- Some GTK/AT-SPI builds expose `GtkMenuButton` (“Presets” in the tuning dialog)
  without an action, so the smoke skips opening the Recommended menu and still
  exercises the profile change through the core-lock switch.
- `AdwExpanderRow` content (URL/SHA-256 in the install dialog) is not exposed by
  every AT-SPI build; the smoke opens the expander when possible and reports the
  fields as an accessibility limitation instead of a failure.
- Device selection is verified against a fake miner that records its arguments;
  a real miner's enumeration order (especially on a heterogeneous rig) still
  needs a real benchmark.
- Native file choosers and the About dialog are manual.
- A closed dialog can linger in the AT-SPI tree after Cancel; the same click
  dismisses it in the manual check.
- The headless UI job uses Xvfb and a private AT-SPI bus; it proves the widget
  tree and the click path, not the Wayland/XWayland compositor behaviour. The
  interactive matrix above and the screenshot test still need a real display.
- The screenshot test needs a display; it fails with a clear error rather than
  writing a blank PNG.
- There is no `/dev/nvidia*` in the sandbox used for development; e2e always uses
  a fake GPU.

## Reporting a regression

Include `trommel status --json`, `trommel diagnose --json`, the daemon journal,
`nvidia-smi --query-gpu=name,driver_version,power.limit,power.default_limit
--format=csv`, the desktop session (GNOME/Wayland/X11) and which command from
the table failed. Redact wallets; never include a seed phrase or private key.
