# Verification

What is proven automatically, what still needs a desktop session and what still
needs a real rig. Run the commands from the repository root; see
[development.md](development.md) for prerequisites and harness isolation.

## Automated

| Command | Covers |
| --- | --- |
| `cargo test --workspace` | Config defaults/migration/diff/warnings, miner argument rendering, fee handling, catalogue parsing, `nvidia-smi` parsing, OC argument validation, supervisor/state/status helpers, D-Bus helpers, hashrate formatting, download/path safety |
| `cargo clippy --workspace --all-targets --features trommel/gui -- -D warnings` | Lints in all crates including GUI code |
| `cargo fmt --check` | Formatting |
| `./scripts/e2e.sh` | CLI lifecycle on a private bus: start/stop/toggle, config v3, per-card inheritance vs custom overrides, apply modes, power read-back, ignored power/clock locks, restore snapshot/defaults, restore-on-stop off, helper timeout, snapshot/restore, stale external state and orphan adoption |
| `./scripts/test-oc-helper.sh` | The real Bash helper against a stateful fake `nvidia-smi`: apply/reset, ignored power rejection, failing `-rgc`, range/UUID validation |
| `./scripts/test-mcp.sh` | MCP initialize/ping/tools/list/tools/call, read-only default, `--allow-write` gating, rejected arguments, wallet redaction |
| `./scripts/ui-smoke.sh` | AT-SPI click-through with a fake two-GPU rig: quick setup, start/stop, settings, preset editor, GPU tuning, miner install, alerts |
| `./scripts/first-run-smoke.sh` | Pristine install: empty XDG dirs, no config, no wallet, no installed miner, one fake GPU; dashboard placeholders, wizard, single-GPU tuning |
| `./scripts/check-polkit.sh` | Polkit action id and helper path match the code |
| `./scripts/check-no-secrets.sh` | No wallet-like strings or secret markers in tracked files |
| `./scripts/check-docs.sh` | Relative markdown links resolve; removed docs are not referenced; docs are reachable from the index |
| `./scripts/package-deb.sh` + `./scripts/test-deb.sh` | Package builds; expected binaries, helper, units, desktop/metainfo, schema and extension are present |
| Asset validation | `desktop-file-validate`, `appstreamcli validate`, `glib-compile-schemas --strict`; the package build also runs `gnome-extensions pack` |

CI (`.github/workflows/ci.yml`) runs unit tests, clippy, fmt, e2e, MCP, secrets,
docs links and asset validation on Ubuntu 26.04. UI smoke, first-run smoke,
packaging and the release bundle are run manually or by the release workflow.

## Interactive matrix

The table records what was last checked manually on the reference RTX 3080 Ti +
RTX 3060 rig, GNOME 50 / Ubuntu 26.04 / Wayland. It is a regression checklist,
not a claim that CI covers it.

| Surface | Action | Expected |
| --- | --- | --- |
| Dashboard | Refresh, Start/Stop, GPU include switch, Tune | Live status; Cancel in the alert restores the switch and the dirty banner |
| Dashboard layout | one GPU, then two GPUs | One card spans the row with a 3×2 bubble grid; two cards return to compact 240 px tiles |
| Dashboard | stopped state | `0 TH/s`, numeric pool balance, configured endpoint named, no lone dashes |
| Quick setup | Cancel, Next, Back, Apply 80 %, Use auto, Start mining | Three steps, validation, per-card values, start only after confirmation |
| First run | Start with no wallet/miner | Opens Quick setup; miner marked not installed; single-GPU tune has no “Apply to” |
| Presets | row, chevron, New preset, Use | Editor opens; chevron is a real button; Use applies the selection |
| Preset editor | Save, Cancel, Duplicate, Delete…, Install… | Apply, confirmation, shared install dialog |
| Miners | row, chevron, benchmark, install dialog | Installed state, cancel, sequential benchmark with a winner |
| Settings | switches, power 0=auto, Save and apply, Copy diagnostics, Restore stock | Aligned pill buttons, live apply, redaction, restore dialog with snapshot/defaults targets, stop-first guard |
| GPU tune | Now row, **⋯** menu (Recommended / Profile default), Save, locks on/off, Similar/All GPUs | Value rows hidden while off; Apply-to only with >1 GPU; restart/stop dialog |
| Alerts | Apply and restart / Apply and stop / Cancel, delete confirmation | Explicit restart/stop; Cancel reverts optimistic changes |
| Menu | Open log, About, Quit | Opens the log/about, quits with `Ctrl+Q` |
| Window | resize to ~400 px | No clipping; GPU value bubbles and action buttons readable |
| GNOME extension | panel icon, popup, Start/Stop, Quick Settings, panel modes, notifications | Mirrors the daemon; menu comes from the panel button; no prefs window; works after re-login |
| Installer | `install.sh`, `.deb` postinst, release installer | Binaries, helper, rule, units, extension, no password prompt for the installing user |
| Real rig | polkit prompt-free, 300 W/1620/810, start/stop, restore dialog | OC applied, ~125 TH/s, power read back, stock restore; an ignored memory lock stops the miner with a named error |
| Error paths | missing helper, missing miner, external miner, failed restore, cancelled auth | Start aborts or explains; snapshot kept on failed restore; no silent success; a 180 s auth dialog is not killed |

## Manual checks that cannot run in CI

- **Real GPU snapshot/restore.** The harness fakes `nvidia-smi`; only a real
  card proves that `--pl`, `--lgc`, `--lmc` and the reset path work with the
  installed driver.
- **Polkit without a password.** Requires the rule installed for the desktop
  user and, typically, an active graphical session.
- **GNOME extension load.** GNOME Shell 50 caches extensions at login; installing
  or enabling requires a log-out/log-in on Wayland.
- **Wayland rendering, fonts and theme.** Screenshots from another machine or
  font stack are not comparable; regenerate them locally.
- **Real pool API.** The pool is external and rate-limited; the cache/backoff
  path is unit-tested, the live JSON is not.

## Known harness limits

- `AdwSpinRow` spinners are not exposed as AT-SPI values, so the UI smoke forces
  a profile change through the core-lock switch instead of typing a number.
- Native file choosers and the About dialog are manual.
- A destroyed modal frame can linger in the AT-SPI tree after Cancel; the same
  click closes the window in the manual check.
- The screenshot test needs a display; it fails with a clear error rather than
  writing a blank PNG.
- There is no `/dev/nvidia*` in the sandbox used for development; e2e always uses
  a fake GPU.

## Reporting a regression

Include `trommel status --json`, `trommel diagnose --json`, the daemon journal,
`nvidia-smi --query-gpu=name,driver_version,power.limit,power.default_limit
--format=csv`, the desktop session (GNOME/Wayland/X11) and which command from
the table failed. Redact wallets; never include a seed phrase or private key.
