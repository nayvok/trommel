# Development

How to build, test, debug and release Trommel. Commands assume the repository
root; if your global cargo cache is read-only, prefix with
`CARGO_HOME="$PWD/.cargo-home"`.

## Prerequisites

```bash
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
  desktop-file-utils appstream gettext librsvg2-bin python3 \
  gir1.2-atspi-2.0 dbus xdg-utils
```

- Rust 1.80+ (edition 2021). GTK 4.22 / libadwaita 1.9 are required by the
  dependencies; Ubuntu 26.04 is the tested distribution.
- Python 3 is used by the test scripts and by packaging/release helpers.
- The UI smoke test needs an accessible desktop session (AT-SPI). It prefers
  the X11 backend through XWayland because the nested Wayland accessibility
  bridge exposes AdwDialog children late; set `TROMMEL_UI_WAYLAND=1` to run it
  on native Wayland. The screenshot test needs a display too. Both fail closed
  with a clear message rather than silently passing.

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
| `cargo clippy --workspace --all-targets --features trommel/gui -- -D warnings` | No new lints, including GUI code | GTK dev libs |
| `cargo fmt --check` | Formatting | Rust only |
| `./scripts/e2e.sh` | Full CLI lifecycle on a private bus with fake GPU, helper and miner, including per-GPU participation (device arguments, excluded card untouched, restore, refusal), SIGKILL/emergency-stop, startup recovery, helper lease, partial apply, restore retry, custom binary, benchmark external guard, restart budget, guard and lost session bus | Python 3, dbus |
| `./scripts/test-mcp.sh` | MCP protocol, read-only default, write gating, redaction | Python 3 |
| `./scripts/ui-smoke.sh` | Critical GUI path through AT-SPI (quick setup, start/stop, settings, editor, tuning, install) | desktop session, AT-SPI; XWayland preferred |
| `./scripts/first-run-smoke.sh` | Pristine install: no config, no wallet, no miner, one fake GPU | desktop session, AT-SPI; XWayland preferred |
| `./scripts/check-no-secrets.sh` | No wallet-like string or secret marker in tracked files | git |
| `./scripts/check-polkit.sh` | Action id and helper path consistency | none |
| `./scripts/check-docs.sh` | Relative markdown links resolve; no references to removed docs | none |
| `./scripts/package-deb.sh` + `./scripts/test-deb.sh` | Package builds and has the expected layout | dpkg-dev |
| Asset validation | `desktop-file-validate`, `appstreamcli validate` | those tools |

CI (`.github/workflows/ci.yml`) runs secrets, format, clippy with `gui`, tests,
e2e, MCP, docs links and asset validation on Ubuntu 26.04. The release workflow
additionally runs `check-polkit.sh` and builds the release bundle. UI smoke and
first-run smoke need a desktop session and are run manually.

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
| `TROMMEL_OPEN_WIZARD` | Open Quick setup on start |
| `TROMMEL_WIZARD_STEP` | Open a specific wizard step (0–2) |
| `TROMMEL_OPEN_PRESET` | Open the preset editor on start |
| `TROMMEL_OPEN_LIBRARY` | Open the miner library dialog on start |
| `TROMMEL_OPEN_INSTALL` | Open the miner install dialog on start |
| `TROMMEL_OPEN_TUNE` | Open the GPU tuning dialog on start |
| `TROMMEL_WIZARD_SCREENSHOT`, `TROMMEL_TUNE_SCREENSHOT`, `TROMMEL_PRESET_SCREENSHOT`, `TROMMEL_LIBRARY_SCREENSHOT`, `TROMMEL_INSTALL_SCREENSHOT` | Capture a transient window |

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
| `TROMMEL_MAX_RESTARTS` | Override the consecutive restart budget (default 5) |
| `TROMMEL_GUARD_BIN` | Path to the `trommel` binary the daemon uses for its guard |
| `TROMMEL_SKIP_BUILD` | Test scripts reuse an existing build |
| `TROMMEL_VERSION` | Override the version expected by `test-deb.sh` |

## Screenshots

```bash
./scripts/screenshots.sh
```

Builds debug binaries, creates a fake two-GPU rig with a running fake miner,
starts a private daemon and renders every surface into `docs/images/`. The
script uses the hooks above, so a UI change should be visible there without
manual work. Regenerate screenshots whenever the user guide references a changed
surface; never commit screenshots from another machine or a different font
stack.

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

The 0.5 roadmap is split into B0–B5 ([roadmap.md](roadmap.md#block-plan)). A
block is the unit of review and release:

1. **Branch.** `feat/<block>-<topic>` from the current mainline. Do not mix the
   next block into the branch, even when it is tempting.
2. **Scope freeze.** Write down the block's acceptance criteria (the roadmap row
   and the ADRs it implements). Anything else is a follow-up, not a drive-by.
3. **Design first.** Update or check the ADR before code; accepted ADRs are not
   drafts. B1 is governed by [ADR 0005](adr/0005-one-active-session.md) and
   [ADR 0006](adr/0006-failure-policy.md).
4. **Code + tests.** Core logic in `trommel-core` with unit tests; lifecycle
   behaviour in `scripts/e2e.sh` with the fake rig. Crash paths (SIGKILL,
   partial apply, failed restore) need an automated test, not a comment.
5. **Docs in the same commit.** Architecture, components, security, agents,
   verification, changelog and the roadmap status. A feature is not done while
   one of those still describes the old behaviour.
6. **Screenshots.** Only regenerate them when the UI actually changed; do not
   touch them for daemon/CLI blocks.
7. **Package and tag.** Bump the workspace version, run
   `./scripts/package-deb.sh && ./scripts/test-deb.sh`, then follow the release
   checklist below. Tag `vX.Y.Z` only after the whole matrix is green.

If the session runs out of time, stop after the last green sub-block, record the
unfinished part in the roadmap and do not start the next block.

## Release checklist

1. `cargo fmt --check`, `cargo clippy … -D warnings`, `cargo test --workspace`.
2. `./scripts/e2e.sh`, `./scripts/test-mcp.sh`, `./scripts/check-polkit.sh`,
   `./scripts/check-no-secrets.sh`, `./scripts/check-docs.sh`.
3. `./scripts/ui-smoke.sh` and `./scripts/first-run-smoke.sh` on a desktop
   session; regenerate screenshots if the UI changed.
4. Bump `version` in the workspace `Cargo.toml`; add a `CHANGELOG.md` entry and
   a metainfo `<release>` entry.
5. `./scripts/package-deb.sh`, `./scripts/test-deb.sh`, optional Docker install
   on Ubuntu 26.04.
6. Commit, tag `vX.Y.Z`, push the tag. The release workflow publishes the
   versioned and stable `.deb` aliases, checksums and `trommel-install.sh` from
   `scripts/release-bundle.sh`.
7. Verify the published assets and the `releases/latest/download` URLs; install
   once from a clean machine.

There is no apt repository and no PPA. Do not publish RPM/AppImage/Flatpak
artifacts without a tested integration story ([packaging.md](packaging.md)).
