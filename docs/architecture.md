# Architecture

Trommel separates a **privileged, long-lived owner** of the rig state
(`trommeld`) from **short-lived clients** (the GTK app and the CLI). This is the
central rule; every other design decision follows from it.

## Runtime topology

```
┌────────────────────────────┐        ┌─────────────────────────────┐
│ trommel (GTK4/libadwaita)  │        │ trommel CLI                 │
│ draft Config, no GPU calls │        │ read-only JSON + commands   │
└─────────────┬──────────────┘        └──────────────┬──────────────┘
              │ D-Bus session bus                    │ D-Bus session bus
              │ io.github.nayvok.Trommel             │ (same interface)
              ▼                                      ▼
┌──────────────────────────────────────────────────────────────────┐
│ trommeld (user service, Type=dbus)                               │
│  Engine: applied Config · Supervisor · last Status               │
│  loops: status refresher · 1 s supervisor tick                   │
│  tray: StatusNotifierItem on the session bus                     │
└───────┬──────────────────────┬───────────────────────┬───────────┘
        │ spawn (args vector)  │ pkexec                 │ HTTPS
        ▼                      ▼                       ▼
  miner child            trommel-oc (root)        pool account API
  (exactly one)           nvidia-smi --pl/-lgc/-lmc
        │
        ├── local HTTP API 127.0.0.1:<api_port>
        └── log file ~/.local/state/trommel/miner.log
```

The CLI (`trommel`) is the same client as the GUI: it connects to
`io.github.nayvok.Trommel.Daemon` and, if nothing owns the name, asks D-Bus
activation (or `trommeld` from `PATH`) to start it. `trommel` without a
subcommand opens the GUI when built with `--features gui`.

## Process and privilege model

| Process | User | Responsibility | Never does |
| --- | --- | --- | --- |
| `trommeld` | desktop user | Owns config, miner child, snapshot, status; serves D-Bus; hosts the SNI tray | Runs as root, edits the helper, downloads without a user action |
| `trommel` (GUI/CLI) | desktop user | Draft editing, display, command dispatch | Calls `nvidia-smi`, spawns/kills the miner, writes state |
| `trommel guard` | desktop user | Watches the daemon PID/start time, runs emergency-stop after a crash | Touches GPUs directly, runs as root, survives a graceful disarm |
| `trommel emergency-stop` | desktop user | Idempotent teardown: stop **our** miner, restore the snapshot | Kills an external miner, restores a stale-boot snapshot |
| `trommel-oc` | root via `pkexec` | Applies/restores `--pl/--lgc/--lmc` for one GPU UUID | Accepts paths, shell fragments or arbitrary commands |
| miner child | desktop user | Mining | Is managed by anything except the supervisor |

Only `trommel-oc` is privileged. It accepts either `apply --gpu GPU-… --pl W
--lgc MHz --lmc MHz` or `reset --gpu GPU-… --pl W`; values are range-checked by
both `trommel-core::oc` and the script. A per-user polkit rule for the annotated
action `io.github.nayvok.trommel.oc` removes the password prompt for the user
who installed the package; without the rule pkexec falls back to
authentication. The helper call waits up to 180 s so an interactive password
dialog is not killed; the per-user rule removes the prompt entirely. A missing
rule therefore costs one authentication, not a silent failure.

The daemon runs in the user session (`PartOf=graphical-session.target`) and is
both enabled at login (so the tray is present) and D-Bus activated on demand.
The tray does not imply mining: it starts in the stopped state. It does **not**
keep mining across its own restart:
on start-up it stops an orphaned miner from the previous session, restores an
applied snapshot and leaves mining stopped (startup recovery). Adoption no
longer resumes mining; see [ADR 0006](decisions.md#adr-0006).

Three independent failure nets cover the daemon: systemd (`KillMode=control-group`,
`TimeoutStopSec=30`, `ExecStopPost=-/usr/bin/trommel emergency-stop
--reason=service-stop`) for packaged runs, the `trommel guard` child for manual
runs, and startup recovery on the next daemon start. They all call the same
idempotent emergency path. The GUI is not part of this chain: killing the window
is a no-op for mining.

## Security model

Only `trommel-oc` is privileged. Everything else — the GUI, the CLI, the daemon
and the guard — runs in the user session. Report vulnerabilities privately; the
policy is in [SECURITY.md](../SECURITY.md).

| Component | Can do | Cannot do |
| --- | --- | --- |
| `trommel` GUI/CLI | Edit its draft, call D-Bus, download into the user store | Tune GPUs directly, spawn the supervisor's miner |
| `trommeld` | Own the miner, call the helper through pkexec, read `nvidia-smi`, scan `/proc` | Run as root, write outside the user's dirs, grant polkit rights, kill an external miner |
| `trommel guard` | Watch the daemon PID/start time and run emergency-stop after a crash | Touch GPUs directly, run as root |
| `trommel-oc` (root, via pkexec) | One `nvidia-smi` call per GPU with numeric arguments | Accept paths, shell text, arbitrary commands or another GPU's UUID |
| Miner binary | Mine, bind a local API port, write its log | Be audited by Trommel beyond the executable/checksum checks |
| Pool API | Return statistics to the daemon | Control the miner, trigger a config change or see the wallet |

The D-Bus service is a **same-user** interface: any process running as the user
can call `GetConfig` and read the wallet. This is a normal desktop trust model;
do not expose the bus name on the system bus or through a socket.

**Root helper.** `trommel-oc` accepts `apply`/`reset`/`status`, validates the
UUID shape and numeric ranges (50–1000 W, 100–4000 MHz) in both the script and
`trommel-core::oc`, and invokes `nvidia-smi` with an argument vector — no paths,
no shell interpolation. It waits for the power limit to read back and fails when
the driver reports a different value. A 180 s timeout protects an interactive
polkit prompt. The optional `--lease-pid`/`--lease-start` pair is checked before
the first mutation, so a helper queued behind a polkit dialog aborts after the
daemon dies. An advisory `flock` on `oc.lock` serializes apply/restore across
processes. The package writes a per-user polkit rule for `SUDO_USER`; without
it, each OC call asks for authentication.

**Downloads.** HTTPS only, 512 MiB limit, optional SHA-256 (a missing digest is
`verified: false`, never invented), safe single-component `miner`/`version`/
`binary` names, archive extraction that rejects absolute paths and `..`, staging
in a temporary directory, and an ELF/`#!` check before the binary is accepted.
Miner binaries are never run as root and are never committed.

**Wallets, logs and diagnostics.** The config file contains the wallet in clear
text; the status document and daemon log do not. `Config::redacted()` replaces
wallets with `<redacted>` for `trommel diagnose --json`, **Copy diagnostics** and
the MCP read tools. Local history stores timestamps, preset/miner ids, GPU UUIDs,
power, temperature and shares — no wallet, worker, pool URL or IP — and is never
uploaded. Test fixtures may only use `prl1test…`; `scripts/check-no-secrets.sh`
rejects real-looking wallets and personal home paths.

**Network.** The daemon calls the PearlHash account API for the configured
wallet (60 s cache with backoff); the miner exposes a local HTTP API on
`127.0.0.1:<api_port>`. Downloads happen only on an explicit install. There is
no telemetry, analytics, update check or account.

**Limits.** A pre-existing manual clock lock cannot be read back through
`nvidia-smi`, so Stop resets locks to the driver default. Fan control and
per-card temperature limits are not available on consumer cards; fan flags are
passed to the miner. Device selection follows the miner's documented flag and
the `nvidia-smi` index order; verify a heterogeneous rig with a benchmark.
Same-user processes can always read the config and call D-Bus — this is not
multi-user isolation.

## Crates and dependency direction

```
trommel (bin) ─────────────┐
  ├── cli/src/main.rs      │
  ├── cli/src/gui.rs       ├──> trommel-core
  └── cli/src/mcp.rs       │      (config, catalog, dbus, oc, supervisor, …)
trommeld (bin) ────────────┘
```

- `crates/core` — library shared by both binaries and tests. It has no UI code
  and no process-global state except caches and the i18n language.
- `crates/daemon` — `trommeld` only: D-Bus service wiring plus two tokio loops.
- `crates/cli` — `trommel` CLI, the GTK GUI behind the `gui` feature and the
  stdio MCP server.

The GUI feature is optional: `cargo build -p trommel` produces a CLI-only
binary, `cargo build -p trommel --features gui` adds GTK4/libadwaita. The
release packages always build with `gui`.

The module map and the “where to change what” table are below.

## Modules

The component map:

| Component | Path | Role |
| --- | --- | --- |
| `trommel-core` | `crates/core/src` | Shared model and logic; no UI, no daemon main |
| `trommeld` | `crates/daemon/src/main.rs`, `tray.rs` | User D-Bus service, status refresher, supervisor loop, SNI tray |
| `trommel` CLI | `crates/cli/src/main.rs` | Command-line client, daemon activation, JSON output |
| `trommel` GUI | `crates/cli/src/gui.rs` | libadwaita application behind `--features gui` |
| MCP server | `crates/cli/src/mcp.rs` | stdio Model Context Protocol interface |
| Catalogue | `resources/catalog/*.toml` | Built-in miners, coins and pools |
| Root helper | `polkit/trommel-oc` | Narrow `nvidia-smi` wrapper run through pkexec |
| Polkit assets | `polkit/` | Action definition and per-user rule template |
| systemd units | `systemd/` | D-Bus activation, user service, start-at-login |
| Desktop assets | `data/` | Desktop entry, metainfo, icons |
| Installer and packaging | `install.sh`, `uninstall.sh`, `packaging/`, `scripts/*deb*` | Developer install and Debian package |
| Test harness | `scripts/e2e.sh`, `ui-smoke.sh`, `first-run-smoke.sh`, `test-mcp.sh` | Automated verification |

`crates/core/src/lib.rs` declares the shared constants (`BUS_NAME`,
`OBJECT_PATH`, `APP_ID`, `VERSION`) and these modules:

| Module | Responsibility |
| --- | --- |
| `config` | `Config`/`Preset`/`GpuProfile`/`MinerInstall`, schema v4 migration, `normalize`, `diff`, warnings, redaction |
| `catalog` | Parse `resources/catalog/*.toml` once; miner/coin/pool definitions, install status, localized labels |
| `paths` | XDG config/data/state paths; legacy config migration; test-root override |
| `nvidia` | One `nvidia-smi` query, parsing, PCI bus ids |
| `oc` | OC snapshot, helper protocol, apply/restore, lease and file lock |
| `miners` | `build_args`, option rendering, `device_plan`, per-GPU participation |
| `supervisor` | Spawn/tick/stop/restart, crash detection, external-miner scan |
| `watchdog` | Hashrate health state machine (notify/restart/stop) |
| `history` | One JSONL sample per minute, retention, range load, CSV |
| `state` | Atomic `state.json`, PID ownership by `miner_path` |
| `emergency` | Idempotent teardown: stop our miner, restore the snapshot |
| `guard` | Independent supervisor for manual runs |
| `status` | The status document (contract v5) |
| `dbus` | Daemon service and client proxy |
| `pool` | PearlHash account API, cache and backoff |
| `minerapi` | Local miner API parsers, per-card hashrate and PCI mapping |
| `bench` | A/B benchmark and report |
| `download` | HTTPS install/import, path and archive safety |
| `versions` | Install registry, verify, usage checks, delete |
| `diagnostics`, `i18n`, `util` | Redacted bundle, `tr(en, ru)` helper, formatting/time helpers |

`trommeld` wires `Engine` to D-Bus, runs the status refresher and the 1 s
supervisor loop, appends history, sends notifications and hosts the tray.
`trommel` is the clap CLI; the GUI feature is optional and additive. The GTK app
keeps a draft and never calls `nvidia-smi`; the daemon owns the applied config,
the miner, the snapshot and the status.

### Where to change what

| Change | Touch |
| --- | --- |
| Add a miner/version/option | `resources/catalog/miners.toml`, tests in `crates/core/src/miners.rs`, [miners.md](miners.md) |
| Change miner arguments | `crates/core/src/miners.rs` (`build_args`/`append_options`) and its tests |
| Change GPU apply/restore | `crates/core/src/oc.rs`, `polkit/trommel-oc`, `scripts/e2e.sh` |
| Change per-GPU participation | `crates/core/src/miners.rs` (`device_plan`), catalogue `device_flag`, `supervisor.rs`, `status.rs`, GUI cards, [ADR 0007](decisions.md#adr-0007) |
| Change the config model | `crates/core/src/config.rs`, `docs/schema/*.json`, migration/normalize tests |
| Add a D-Bus method | `crates/core/src/dbus.rs`, CLI/MCP callers, this document |
| Add a status field | `crates/core/src/status.rs`, GUI rendering, CLI/MCP output |
| Add a GUI page/dialog | `crates/cli/src/gui.rs`, [design.md](design.md), screenshots, UI smoke |
| Add a CLI command | `crates/cli/src/main.rs`, [agents.md](agents.md), [user-guide.md](user-guide.md) |
| Add an MCP tool | `crates/cli/src/mcp.rs`, `scripts/test-mcp.sh`, [agents.md](agents.md) |
| Change packaging | `packaging/`, `scripts/package-deb.sh`, `.github/workflows/`, [development.md](development.md#packaging-and-releases) |

## Configuration model (schema v4)

`~/.config/trommel/config.json` is the single source of truth for intent. It is
owned by the daemon once running; the GUI keeps a **draft** copy and only sends
it through `ApplyConfig`.

```
Config
├── schema_version: 4
├── active_preset: Option<String>
├── presets: { id -> Preset }
│     coin, coin_symbol/name/wallet_prefix/hashrate_unit, algo,
│     miner, miner_version, pool_url, pool_fee_pct, pool_api_url,
│     wallet, worker, gpu: GpuProfile, api_port, expected_hashrate,
│     miner_options, fee overrides, extra_args, env, timestamps
├── gpu_presets: { id -> GpuPreset }       // named per-card tuning templates
│     name, gpu_uuid, gpu_name, tuning, timestamps
├── miner_installs: { "id@version" -> MinerInstall }
│     path, sha256 (recorded, may be empty), source_url, verified, custom,
│     installed_at
└── behavior: auto_restart, restart_delay_s, poll_interval_s,
              restore_stock_on_stop, start_mining_on_login, notifications,
              tray_monochrome, language, appearance, simple_mode,
              temp_limit_c, temp_resume_c,
              watchdog_enabled, watchdog_threshold_pct, watchdog_window_s,
              watchdog_action, watchdog_max_restarts
```

GPU power, clocks and participation live **inside the mining preset**
(`Preset::gpu`). Switching the active preset therefore switches the whole GPU
setup; a new preset starts from stock. A named `GpuPreset` is a reusable
per-card template and is independent of mining presets: deleting a mining preset
deletes only its own values, never the saved templates.

`GpuTuning` uses `Option` values as the source of truth: `None` means “do not
touch”, `Some` means “apply”. `enabled` decides whether the card participates in
the **mining session**: `false` excludes it from the miner, skips its tuning and
returns any tuning left over from an earlier restore-off session (see
[ADR 0007](decisions.md#adr-0007)). A fresh preset has no clock
locks; power limit `None` means driver auto, and `0` in the UI maps to `None`.
`gpu_preset` is metadata only: it names the template a card follows, never
changes what is applied, and is ignored by `Config::diff`.

The temperature policy is global in `behavior.temp_limit_c`/`temp_resume_c`
because one miner process receives one temperature flag. The pool is user input:
there is no catalogue pool id in the preset; the URL is authoritative and a known
host only supplies a hint and a documented JSON API template.

Presets reference a miner **id and version**, never a path. `miner_installs` is
global and keyed `id@version`, so several versions of the same miner can stay
installed for rollback; one installation is visible to every preset, and a
preset whose version is missing blocks Start until the user installs it or
switches versions. Updates, removals and rollbacks are explicit: installing a
version never edits a preset, a version pinned by any preset cannot be removed,
and moving a preset to another installed version changes only that preset.
`versions::verify` checks that a recorded binary exists, is a regular file, is
executable and is readable; it echoes a recorded SHA-256 or `null` and never
hashes a file to invent one. `versions::probe_algorithms` is the explicit
capability probe (`--list-algorithms` or `--help`) and runs only when asked.
See [Modules § versions](#modules).

`Config::normalize()` repairs the common broken states on load (dangling active
preset, missing coin display fields, stale fee override). It deliberately does
**not** invent a preset or a miner version: a fresh install has no presets.
`Config::from_v1`/`from_v3` migrate the former flat config and the shared
`gpu_profiles` layout once. The v1 legacy path is
`~/.config/pearl-miner/config.json`; the old file is left untouched. The state
file keeps one legacy `profile` field so old state files still parse.

The JSON schemas in [`schema/`](schema/) are the machine-readable form for
external tools. They are descriptive, not enforced by the daemon; `Preset`
(`additionalProperties: true`) tolerates forward-compatible fields.

## Lifecycle

### Start

1. Reject if the supervisor already owns a live child, or if an **external**
   miner is detected in `/proc` (the user must stop it first; Trommel never
   kills a process it did not start).
2. Validate the active preset: wallet present, pool address contains `:`,
   miner binary installed and executable.
3. Resolve participation and the effective tuning per card. A per-GPU entry
   with `custom = false` inherits power and clocks from the preset's GPU default
   and only keeps its own participation switch; temperature is global. A card with
   `enabled = false` is excluded from the miner and is never tuned; a
   participating card whose effective tuning has no power or clock values is
   not touched at all. A partial participation is validated against the
   selected miner's documented device flag **before** any helper call.
4. Query all GPUs. For a profile with tuning, reuse the current-boot snapshot or
   capture one; write it with `applied = true`, **all** target UUIDs and the
   exact applied tuning *before* the first helper call.
5. Apply the profile per enabled card through the helper. Every target is
   already registered in the snapshot and in the rollback list, so a partial
   failure restores every card that may have been touched. The power limit is
   read back (±1 W); a mismatch is a failure.
6. Spawn the miner with an explicit argument vector (no shell). When a card is
   excluded, the argument vector carries the miner's documented device flag
   (`--gpu-list`, `--gpu-id` or `--devices`) with the participating
   `nvidia-smi` indices; a miner without such a flag aborts the start instead of
   quietly mining on every card. Record the absolute `state.miner_path`, write
   state, schedule a one-shot clock check and clear the pool cache.

The snapshot captures every detected card, but `applied_gpus` holds
the participating, tuned UUIDs, so Stop, shutdown and emergency-stop never
restore a card that was not tuned. A card excluded now but still carrying
tuning from an earlier restore-off session on this boot is restored to its
snapshot before the new session starts. The whole snapshot/apply/rollback block
holds the cross-process OC file lock (§ *OC serialization and helper lease*).
Any failure in 5–6 restores the snapshot before returning the error. If the rollback also fails, the snapshot is
kept with `last_error`, so the UI can offer a retry and emergency-stop can
repeat it. A profile with no tuning creates no snapshot and calls no helper.
Start never mines with a silently unapplied power limit.

### Stop

Terminate the child (or a PID after a daemon restart) only after confirming
that `/proc/<pid>/exe`/`cmdline` matches `state.miner_path`, and re-check the
kernel start time immediately before each signal so a recycled PID is never
killed. A legacy state without `miner_path` cannot prove ownership: its PID is
not signalled at all, the stale record is dropped with a warning and the user
stops that process explicitly. An external state
(`state.external`) is left untouched, and the state entry is cleared without a
restore because its tuning was never ours. If
`behavior.restore_stock_on_stop` is set and a snapshot was applied, restore the
captured power limit, unlock the clocks, read the power back and clear the
snapshot. On failure the snapshot is kept with `last_error` and the error is
returned. If restore is disabled, Stop keeps the snapshot, leaves the settings
on the cards and reports `oc.needs_restore`; it does not pretend the stock state
was restored. State and pool cache are cleared either way, and the stop path
holds the OC lock across kill + restore.

### Clock verification

A power limit can be read back immediately. A clock lock cannot be verified on
an idle GPU: the card may already sit at the locked frequency (or below it).
The supervisor therefore waits for the miner to load the GPU and then compares
`clocks.mem` and `clocks.sm` with `snapshot.applied_tuning`. A memory mismatch
is decisive: Stop with a forced restore, persist the error and notify the user.
A core mismatch is a warning because the driver may fluctuate. This catches a
driver that accepts `-lmc` but ignores it, which was previously reported as
“applied”.

### Crash and restart

`Supervisor::tick()` runs once per second. When the child exits unexpectedly and
`auto_restart` is on, it schedules a restart with exponential backoff from
`restart_delay_s` (2, 4, 8, … capped at 60 s), increments `state.restarts` and
emits a `Crashed` event. The budget is five consecutive restarts
(`TROMMEL_MAX_RESTARTS` overrides it for tests); when it is exhausted the
session stops, the snapshot is restored, and an error event/notification says
so. With `auto_restart` off the first crash stops and restores immediately.
Every spawn resets `started_at`, so `uptime_s` is the uptime of the current
process, not of the whole crash streak. Restart reuses the same config and does
not re-capture the stock snapshot; the restart counter resets only on an
explicit Start. Before the scheduled spawn, the supervisor rescans `/proc` for
an external miner: if the user started one during the backoff, the restart is
cancelled, the snapshot is restored and an `Error` names the PID instead of
leaving two miners on the rig.

### Hashrate watchdog

`trommeld` also runs a watchdog in the supervisor loop (ADR 0010). It is off by
default. When `behavior.watchdog_enabled` is on, it inspects the last `Status`
every second and reacts to a running miner it owns:

- `Low` — total hashrate below `watchdog_threshold_pct` of
  `preset.expected_hashrate` for `watchdog_window_s` (default 80 %, 300 s);
- `Zero` — the miner reports 0 H/s for the window;
- `CardDown` — a participating card mapped by B7.1 reports 0 H/s or is missing
  from the per-card data while other cards report;
- `Stale` — the status sample is older than `max(3 × poll, 90 s)`, or the miner
  reports no data for the window; this is notification-only.

A warm-up of `min(window_s, 120)` seconds after every start and restart prevents
false positives while the miner loads. One healthy sample clears a low episode.
With `watchdog_action = "notify"` (default) the daemon only emits the existing
`Notification` signal/desktop notification. With `"restart"` it waits the
existing exponential `restart_delay_s`, then calls `Supervisor::stop` +
`Supervisor::start` (snapshot/restore, device plan, external scan, busy guard)
and notifies. After `watchdog_max_restarts` failed restarts it stops, restores
stock and notifies; it never reboots the rig. An external miner, a benchmark or
a crash restart in flight is never touched. Pool difficulty is not a signal: the
watchdog uses the miner's local hashrate. See
[ADR 0010](decisions.md#adr-0010).

### Local history

The daemon writes one JSONL sample per minute to
`~/.local/state/trommel/history/history-YYYYMMDD.jsonl` (UTC daily files). A
sample keeps `ts`, running/external flags, preset id, miner id, total/mining
power, hashrate, shares, source and the mapped per-GPU hashrate/power/temp/fan.
It never contains a wallet, worker, pool URL, IP or any network telemetry.
Daily files make retention a prune of files older than eight days; append is one
short line, corruption is skipped per line, duplicate timestamps are
deduplicated on read, and a clock that steps backwards does not overwrite a
newer sample. The writer is a `history::Writer` owned by the status loop; the
GUI never writes state.

`GetHistory(since, until)` reads the files in a blocking task and returns the
sorted, deduplicated JSON array (range ≤ 8 days). The GUI uses it for the
History dialog and a cached 24-hour average in the hero; the CLI/status
document is not enlarged on every poll. History is local-only and is not
uploaded anywhere.

### Apply while mining

`ApplyConfig(json, mode)` with `mode`:

| Mode | Running miner | Stopped miner |
| --- | --- | --- |
| `live` | Rejected when the diff class is `RestartRequired` | Saved |
| `restart` | Stop (restores stock), start with the new config, save on success | Save, start |
| `stop` | Stop, save, stay stopped | Save |

`Config::diff` classifies changes: active preset, GPU profiles and the active
preset's settings require a restart; language, notifications and the autostart
setting are live changes. On a failed start the previous config is saved back
and a best-effort stock restore is attempted. The GUI never sends `restart` or
`stop` without the explicit alert dialog.

### External miner and PID ownership

`state::find_external_miner(cfg, exclude_pid)` scans `/proc` for the catalogue
names and for the directory/binary basenames of every entry in
`miner_installs`, plus shell scripts whose first argument has one of those
names. It ignores the command line as a whole because config JSON can contain
miner names, and it always skips the caller's PID and `exclude_pid`.
`TROMMEL_NO_EXTERNAL_SCAN=1` is a test isolation hook.

`state.miner_path` is the absolute binary path recorded by the supervisor when
it spawns the miner. `State::miner_alive()` and `pid_matches_binary(pid, path)`
compare `/proc/<pid>/exe` (and the interpreter argument for a `#!` script). A
PID whose executable does not match the saved path is **not** signalled: PID
reuse and custom binary names make the old “known miner name” check unsafe. Old
state files without `miner_path` are never signalled by name either: the name
heuristic only feeds liveness/external detection, so the stale record is
cleared with a warning and the process is left to the user. `proc_liveness`
keeps “gone” (the kernel returned `ENOENT`) apart from “unknown” (`/proc` could
not be read): an unknown answer is treated as possibly alive, never as dead.

### Startup recovery and emergency-stop

`trommel emergency-stop --reason=…` is the same idempotent teardown used by
systemd, the guard, startup recovery and manual runs:

1. take the OC file lock (bounded wait) and drop a snapshot from another boot;
2. wait up to 10 s for an in-flight helper, then terminate helpers carrying the
   dead daemon's lease before touching the cards;
3. stop only the miner recorded in state when its PID still matches
   `miner_path`; a legacy record without a path is cleared without a signal,
   and an external miner is left alive and unclaimed;
4. clear state only after the miner is gone; restore a current-boot snapshot
   with `applied = true` through `RestoreMode::Snapshot` with a 20 s helper
   timeout; clear the snapshot only after every card restored.

A failed restore keeps the snapshot + `last_error`, logs the reason and exits
non-zero. A second call is safe. Startup recovery is the same routine with
`--reason=startup`, run by `trommeld` before it owns the D-Bus name: it stops an
orphaned miner from the previous session, restores stock and leaves mining
stopped. It never resumes mining. `trommel emergency-stop` is hidden from the
CLI help on purpose: it is an operator/automation command, not a normal Stop.

### Shutdown, lost session bus and guard

On SIGTERM/SIGINT the daemon calls `Supervisor::shutdown(&cfg)` (force restore,
20 s helper timeout), disarms the guard and exits. The tray's **Quit** joins the
same path after emitting `QuitRequested`; it first asks an open GUI to close,
then stops only our miner, restores stock and exits with status 0. Restore comes first and the
guard stays armed during the shutdown window: if the daemon is SIGKILLed
mid-restore, the guard runs the same idempotent emergency-stop and the OC flock
serializes the two. Disarming first would leave a killed daemon with applied
clocks and no one to put them back.
`systemd` runs `ExecStopPost` afterwards, so a failed restore is retried outside
the process; `KillMode=control-group` also removes the guard. Under systemd
(`INVOCATION_ID` or `JOURNAL_STREAM` set) the daemon does not spawn a guard at
all: `ExecStopPost` is the net. The daemon selects
on `connection.closed()`: losing the session bus is a shutdown reason, not a
reason to keep mining unattended.

For manual runs the daemon spawns `trommel guard` in its own session (`setsid`).
The binary is only `TROMMEL_GUARD_BIN` or the `trommel` sibling of
`current_exe`; the old `$PATH` fallback could start an unrelated build. The
guard writes `guard.pid` before it starts watching; the daemon waits up to two
seconds for that ready marker and reports `guard started` only when the file
names this daemon and the guard PID is alive. The guard watches the daemon PID
**and** its `/proc` start time. If the daemon disappears without writing the
matching `guard.disarm` token, the guard runs
`emergency-stop --reason=guard` and exits. A graceful shutdown restores stock,
then disarms, so a normal stop cannot trigger a second restore. A new daemon
terminates a stale guard left by the previous session after startup recovery.

### OC serialization and helper lease

Every apply/restore/emergency block holds an advisory `flock` on
`~/.local/state/trommel/oc.lock`. The lock is per process; a nested acquisition
is rejected instead of deadlocking, and `trommel`, `trommeld` and emergency-stop
serialize across processes. This prevents a stale apply from landing after an
emergency restore.

The helper also receives `--lease-pid <daemon-pid> --lease-start
<starttime>` (field 22 of `/proc/<pid>/stat`). Both numbers are validated by the
Rust side and by the Bash helper; the helper checks the lease **before its first
`nvidia-smi` mutation** and aborts when the process is gone or its start time
changed. The daemon writes `oc-lease.json` with the spawned `pkexec`/helper PID
so emergency-stop can wait for or terminate it. A queued pkexec authorization
therefore cannot tune a card that is being restored.

### Benchmark

`RunBenchmark` first refuses when an external miner is running or when the
running miner is not owned by this session; it never calls `Supervisor::stop`
to take a process it does not own. It then stops our miner if needed, runs each
requested miner for the given duration (30–3600 s), records raw and effective
hashrate with fees, saves the report under `~/.local/state/trommel/bench/`,
restarts the previous miner if it was running and returns the JSON report.
Benchmarks are sequential by design: one miner at a time is an invariant.

## Status contract

`Status` (contract version 5) is serialized to JSON and pushed over
`StatusChanged`; `Status()`/`Refresh()` return the same document. It contains:

- `state` — running, uptime, restarts, last error, external flag, preset
  id/name;
- `miner_info` — catalogue description, version, license, install/verified
  state and the fee breakdown;
- `gpus[]` and `gpu` — one `GpuStats` per card from `nvidia-smi`, including the
  static `pci_bus_id` used to map miner per-card records;
- `participation[]` — one entry per detected card with `index`, `uuid`, `name`,
  `enabled` (the applied profile switch) and `mining` (this card is part of the
  running session Trommel owns). It is the machine-readable form of the
  dashboard chip;
- `miner` — API/log statistics (`hashrate_10s/60s/15m`, accepted, rejected,
  stale, source) and `gpus[]`: per-card records with `index`/`uuid` (mapped by
  unique PCI bus id; `null` when the mapping is ambiguous), the miner-side
  `device`, `bus_id`, hashrate windows and optional accepted/rejected shares.
  An empty array means the miner does not report per-card data; the total is
  never divided between cards;
- `total` — aggregate hashrate, whole-rig `power_w`, `mining_power_w`,
  `efficiency` + `efficiency_unit`, max temperature and participating/total GPU
  counts (`gpus_enabled` / `gpus_total`). `power_w` is `power.draw` summed over
  every detected card (an idle or excluded card still draws, and a card whose
  reading is `N/A` is skipped). `mining_power_w` uses participating cards only.
  `efficiency` is that power over the hashrate in the coin's display unit
  (`J/TH`, `J/MH`, …), or `null` for an external miner. The legacy
  `efficiency_j_per_th` is filled only when the unit really is J/TH;
- `pool` — cached pool statistics when mining; `pool_error` keeps the last
  failed fetch;
- `oc` — `applied` (the snapshot was applied), `needs_restore` (applied while
  stopped), snapshot availability, stale-boot flag, restore-on-stop setting,
  last restore error and a first-card summary of the restore/default targets;
- `warnings` — config warnings plus “lock not honoured”, unmatched per-card
  mapping and one “card #N is at T °C (limit L °C)” note while the hottest card
  is at or above the profile limit;
- fees, expected hashrate, temperature limit, timestamp (`ts`, `ts_iso`) and
  app version.

The machine-readable description is
[status.schema.json](schema/status.schema.json). New fields are additive and
`serde(default)` so a new client can read an older daemon; external clients
should compare `contract_version`, not the app version.

The document is display-oriented: it is not an RPC request format and has no
wallet. Clients and external agents should treat unknown fields as optional and
compare `contract_version` rather than the app version.

## GPU tuning and stock restore

- `oc::snapshot()` reads per-card power limits (`power.limit`,
  `power.default_limit`, min/max), clocks and identity, plus the kernel boot id.
- `OcSnapshot` also records `applied`, `applied_gpus`, `applied_tuning` and
  `last_error`. A snapshot is written with `applied = true` and **every** target
  UUID before the first helper call, so a crash halfway still leaves a record
  that restores all registered cards, including a partially applied one.
- The snapshot is stored at `~/.local/state/trommel/oc-snapshot.json`. A
  snapshot from another boot is deleted when the daemon starts and is never
  used by Stop/Restore; Start captures a fresh one.
- `custom = false` per-GPU entries inherit the active preset's GPU default; only
  `custom = true` entries (written by the tuning dialog) replace it. This keeps
  the participation switch from shadowing the default.
- Apply passes only values that are `Some`; a profile with no power/clock
  values produces no snapshot and no helper call.
- Restore has two targets: `snapshot` (the captured limit, clocks unlocked) and
  `defaults` (the driver's `power.default_limit`, clocks unlocked). The UI shows
  both when they differ and warns when the snapshot itself already had a
  non-default limit.
- The helper verifies `power.limit` and fails when the driver does not honour
  it; the supervisor verifies memory/core locks under load.
- A pre-existing manual clock lock cannot be read back through `nvidia-smi`;
  Stop resets locks to the driver default and the UI says so. This is a
  documented limitation, not a bug.
- `Status` compares configured locks with the driver values and adds a warning
  when a lock is not honoured (memory under load; core under load, because an
  idle GPU legitimately downclocks), plus a warning when applied settings are
  still on the cards while the miner is stopped.

## Miner installs and downloads

Miner binaries are never committed or bundled. Two paths install them:
`download::install` (HTTPS, ≤512 MiB, optional SHA-256, `tar.gz`/`zip`/raw
extraction with absolute-path and `..` rejection, executable check, atomic move
into `~/.local/share/trommel/miners/<miner>/<version>/`) and
`download::import_binary` (copy a local executable into the same store). Before
any network request or write, `miner`, `version` and `binary` must each be one
safe path component: non-empty, not `.`/`..`, and free of `/`, `\` and NUL. A
running daemon receives the updated config immediately after either command, so
a newly installed version is usable without a restart. Installs are global; a
preset only references `miner@version`. The absolute installed path becomes
`state.miner_path` when the supervisor starts it, so custom file names are
stopped correctly.

## Pool and miner statistics

- Pool statistics come from the PearlHash account API
  (`https://pearlhash.xyz/api/account/<wallet>`). The call is cached for 60 s and
  runs on a background thread with retry/backoff, because a slow API used to
  block `Status::collect` for tens of seconds. Other pools show configured data
  only.
- Miner statistics prefer the local HTTP API on `127.0.0.1:<api_port>`; if that
  fails, the daemon parses the tail of the miner log. The parser is
  driver-aware: WildRig (`hashrate.total[]`, `hashrate.threads[][]`,
  `hwmon.busID[]`), SRBMiner (`algorithms[].hashrate.gpu.gpuN` +
  `gpu_devices[].bus_id`) and OneZero (`devices[].bus_id` + parallel
  `algos[].hashrates[]`) have documented shapes; a custom miner falls back to
  the tolerant generic parser with total values only. Per-card records are
  attached to an `nvidia-smi` card only when exactly one card has the same PCI
  bus number; a collision, a missing bus id or an unknown bus stays unmapped
  and the dashboard shows `—` with one status warning. The total is never split
  evenly between cards.

## Concurrency

- `Engine` holds `Inner { cfg, sup, status, last_report }` behind an async
  `Mutex`. Every D-Bus method takes it; blocking work (`nvidia-smi`, helper,
  spawn) runs in `spawn_blocking`/`block_in_place`.
- GPU mutations are additionally serialized across processes by the OC
  `flock`; within a process a nested acquisition is an error. A shutdown tries
  the engine lock for 5 s and otherwise leaves the teardown to `ExecStopPost` /
  the guard instead of waiting for a polkit dialog.
- The daemon runs two async loops: the status refresher (poll interval, or an
  on-demand refresh after supervisor events) and the 1 s supervisor tick; the
  same tick evaluates the hashrate watchdog against the last status and may run
  its opt-in Stop/Start through the supervisor. A third select arm watches
  SIGTERM/SIGINT and `connection.closed()`.
- The guard is a separate process, not a thread: it must survive a `SIGKILL` of
  the daemon. It is started with `setsid` and writes `guard.pid`; shutdown writes
  `guard.disarm` with the exact daemon PID/start time before restoring, so a
  new daemon is never mistaken for the one the guard watched.
- The GUI owns a separate thread with a small tokio runtime and a
  command/event channel (`Cmd`/`Ev`). GTK state is only touched on the main
  thread; D-Bus results arrive as `Ev::Status/Config/Error/...`.
- The i18n language is process-global (`AtomicU8`); the daemon updates it when
  a config with a new language is applied.
- The GUI is single-instance: a second launch presents the existing window
  (`APP_SLOT`).

## Notifications

`behavior.notifications` gates two paths for the same events (miner started,
stopped, crashed, warnings and an overheat episode):

- the daemon calls `org.freedesktop.Notifications.Notify` on the session bus,
  which turns the event into a regular desktop notification. The call is
  bounded by a short timeout and failures are ignored, so a desktop without a
  notification service (or a test bus) never blocks the daemon;
- the daemon emits the `Notification(summary, body)` D-Bus signal for custom
  integrations that want to render their own alert.

The overheat alert follows the hottest card, not card 0, and names it in the
body (`… 95 °C (card #1, limit 90 °C)`), so a two-GPU rig alerts on whichever
card is actually hot. Both paths use the same localised summary and body. The
daemon is the only
long-lived process, so it owns this path; there is no shell extension.

## Tray

`trommeld` hosts a freedesktop **StatusNotifierItem** tray (see
[ADR 0008](decisions.md#adr-0008)). It lives in the
daemon on purpose: the icon exists without an open GTK window, survives a GUI
crash and can start/stop the miner directly. The GTK app remains a short-lived
client.

The menu is built by `ksni` over `org.kde.StatusNotifierItem` and
`com.canonical.dbusmenu`; the panel renders it. It contains a disabled status
header (`Mining stopped` / `Mining · <preset> · <hashrate>`), a separator,
**Start/Stop mining**, **Show window** and **Quit**:

- **Start/Stop** calls `Engine::tray_toggle`, the same supervisor path and busy
  guard as the D-Bus `Toggle()` method.
- **Show window** runs `gtk-launch io.github.nayvok.Trommel` and falls back to
  `trommel gui`; the single-instance GApplication presents an existing window.
- **Quit** is a full shutdown: the daemon emits `QuitRequested` so an open GUI
  can close, then runs the normal graceful stop (only our miner, snapshot
  restore, guard disarm) and exits. `ExecStopPost`/the guard remain as the
  crash net.

KDE Plasma supports SNI natively. GNOME Shell needs the AppIndicator extension
(Ubuntu ships `ubuntu-appindicators`); without a watcher the tray simply does not
appear and every other feature keeps working. Windows would need a different
platform backend and is not supported. `TROMMEL_NO_TRAY=1` disables the tray for
tests. The next tray iteration (show/hide, start minimised, dynamic icon) is
listed in [roadmap.md](roadmap.md#next-not-scheduled).

`behavior.tray_monochrome` (default `true`) selects the monochrome version of
the tray drum; `false` selects the colour icon. Both are installed in hicolor.
The change is live and does not restart the miner.

## Design constraints and trade-offs

- **The manager is not a miner.** No bundled binaries, no wallet, no seed
  phrases; the catalogue links to upstream releases.
- **One miner at a time.** Never spawn a second child, never silently restart a
  running miner, never kill an external process without explicit user action.
  Per-GPU participation reduces the device set of that one miner; it is not a
  second job, a second process or multi-job mining.
- **Stock restore is mandatory by default.** No hardcoded power limit; capture
  before touching power/clocks and restore on stop.
- **The daemon is the source of truth.** Editing `config.json` directly is not
  enough while the daemon runs — use Apply/CLI so the daemon validates and
  applies the change.
- **No `nvidia-smi` in the GUI.** Any future client can be added without
  privileged access.
- **No Flatpak.** The sandbox conflicts with the host helper, systemd
  unit and runtime binary downloads; see
  [ADR 0003](decisions.md#adr-0003).
- **Debian first.** One tested distribution beats four untested ones; see
  [Development § packaging and releases](development.md#packaging-and-releases).

## Files and locations

| Path | Contents |
| --- | --- |
| `~/.config/trommel/config.json` | Applied config (schema v4) |
| `~/.local/state/trommel/state.json` | Running state, PID, `miner_path`, preset, restart count |
| `~/.local/state/trommel/oc.lock` | Cross-process OC `flock` |
| `~/.local/state/trommel/oc-lease.json` | Daemon/helper PID + start-time lease |
| `~/.local/state/trommel/guard.pid` | Guard PID/start time and the daemon it watches |
| `~/.local/state/trommel/guard.disarm` | Disarm token written by a graceful daemon |
| `~/.local/state/trommel/miner.log` | Miner stdout/stderr |
| `~/.local/state/trommel/daemon.log` | Daemon event log |
| `~/.local/state/trommel/oc-snapshot.json` | Per-boot stock snapshot |
| `~/.local/state/trommel/bench/bench-<ts>.json` | A/B benchmark reports |
| `~/.local/state/trommel/history/history-YYYYMMDD.jsonl` | One daemon-owned local sample per minute; 8-day retention |
| `~/.local/share/trommel/miners/<id>/<version>/` | Installed miner binaries |
| `/usr/libexec/trommel/trommel-oc` | Root helper |
| `/usr/share/polkit-1/actions/io.github.nayvok.trommel.policy` | Polkit action |

XDG variables are honoured; `paths.rs` centralises every location and uses a
temporary root under `cargo test` so unit tests never touch real user state.
