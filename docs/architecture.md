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
| `trommeld` | desktop user | Owns config, miner child, snapshot, status; serves D-Bus | Runs as root, edits the helper, downloads without a user action |
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
D-Bus activated on demand. It does **not** keep mining across its own restart:
on start-up it stops an orphaned miner from the previous session, restores an
applied snapshot and leaves mining stopped (startup recovery). Adoption no
longer resumes mining; see [ADR 0006](adr/0006-failure-policy.md).

Three independent failure nets cover the daemon: systemd (`KillMode=control-group`,
`TimeoutStopSec=30`, `ExecStopPost=-/usr/bin/trommel emergency-stop
--reason=service-stop`) for packaged runs, the `trommel guard` child for manual
runs, and startup recovery on the next daemon start. They all call the same
idempotent emergency path. The GUI is not part of this chain: killing the window
is a no-op for mining.

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

Per-module responsibilities and key functions are listed in
[components.md](components.md).

## Configuration model (schema v3)

`~/.config/trommel/config.json` is the single source of truth for intent. It is
owned by the daemon once running; the GUI keeps a **draft** copy and only sends
it through `ApplyConfig`.

```
Config
├── schema_version: 3
├── active_preset: Option<String>
├── presets: { id -> Preset }
│     coin, algo, miner, miner_version, pool, pool_url, wallet, worker,
│     gpu_profile, api_port, expected_hashrate, miner_options, fee overrides,
│     extra_args, env, timestamps
├── gpu_profiles: { id -> GpuProfile }
│     name, default: GpuTuning, per_gpu: { gpu-uuid -> GpuTuning }
├── miner_installs: { "id@version" -> MinerInstall }
│     path, source_url, verified, custom, installed_at
├── custom_miners: { id -> CustomMiner }      (reserved; not used by the GUI)
├── pools: { id -> CustomPool }               (reserved for future endpoints)
└── behavior: auto_restart, restart_delay_s, poll_interval_s,
              restore_stock_on_stop, start_mining_on_login, notifications,
              language, appearance, simple_mode
```

`GpuTuning` uses `Option` values as the source of truth: `None` means “do not
touch”, `Some` means “apply”. `enabled` decides whether the card participates in
the profile at all. A fresh profile has no clock locks; power limit `None` means
driver auto, and `0` in the UI maps to `None`.

Presets reference a miner **id and version**, never a path. `miner_installs` is
global: one installation is visible to every preset, and a preset whose version
is missing blocks Start until the user installs it or switches versions.

`Config::normalize()` repairs the common broken states on load (dangling active
preset, missing default profile, missing install path, stale fee override).
`Config::from_v1`
migrates the former flat configuration once, from the legacy
`~/.config/pearl-miner/config.json`; the old file is left untouched. That is the
only user-visible legacy configuration path; the state file keeps one legacy
`profile` field so old state files still parse.

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
3. Resolve the effective tuning per card. A per-GPU entry with `custom = false`
   inherits power, clocks and temperature from the profile default and only
   keeps its own enable switch. Cards whose effective tuning has no power or
   clock values are not touched at all.
4. Query all GPUs. For a profile with tuning, reuse the current-boot snapshot or
   capture one; write it with `applied = true`, **all** target UUIDs and the
   exact applied tuning *before* the first helper call.
5. Apply the profile per enabled card through the helper. Every target is
   already registered in the snapshot and in the rollback list, so a partial
   failure restores every card that may have been touched. The power limit is
   read back (±1 W); a mismatch is a failure.
6. Spawn the miner with an explicit argument vector (no shell), record the
   absolute `state.miner_path`, write state, schedule a one-shot clock check and
   clear the pool cache.

The whole snapshot/apply/rollback block holds the cross-process OC file lock
(§ *OC serialization and helper lease*). Any failure in 5–6 restores the
snapshot before returning the error. If the rollback also fails, the snapshot is
kept with `last_error`, so the UI can offer a retry and emergency-stop can
repeat it. A profile with no tuning creates no snapshot and calls no helper.
Start never mines with a silently unapplied power limit.

### Stop

Terminate the child (or a PID after a daemon restart) only after confirming
that `/proc/<pid>/exe`/`cmdline` matches `state.miner_path`. An external state
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
explicit Start.

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
state files without `miner_path` fall back to the legacy name heuristic for one
migration window.

### Startup recovery and emergency-stop

`trommel emergency-stop --reason=…` is the same idempotent teardown used by
systemd, the guard, startup recovery and manual runs:

1. take the OC file lock (bounded wait) and drop a snapshot from another boot;
2. wait up to 10 s for an in-flight helper, then terminate helpers carrying the
   dead daemon's lease before touching the cards;
3. stop only the miner recorded in state when its PID still matches
   `miner_path`; an external miner is left alive and unclaimed;
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

On SIGTERM/SIGINT the daemon disarms the guard, calls
`Supervisor::shutdown(&cfg)` (force restore, 20 s helper timeout) and exits.
`systemd` runs `ExecStopPost` afterwards, so a failed restore is retried outside
the process; `KillMode=control-group` also removes the guard. The daemon selects
on `connection.closed()`: losing the session bus is a shutdown reason, not a
reason to keep mining unattended.

For manual runs the daemon spawns `trommel guard` in its own session (`setsid`).
The guard watches the daemon PID **and** its `/proc` start time. If the daemon
disappears without writing the matching `guard.disarm` token, the guard runs
`emergency-stop --reason=guard` and exits. A graceful shutdown disarms the guard
before the potentially long restore, so a normal stop cannot trigger a second
restore. A new daemon terminates a stale guard left by the previous session
after startup recovery.

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

`Status` (contract version 3) is serialized to JSON and pushed over
`StatusChanged`; `Status()`/`Refresh()` return the same document. It contains:

- `state` — running, uptime, restarts, last error, external flag, preset
  id/name;
- `miner_info` — catalogue description, version, license, install/verified
  state and the fee breakdown;
- `gpus[]` and `gpu` — one `GpuStats` per card from `nvidia-smi`;
- `total` — aggregate hashrate, power, efficiency (J/TH), max temperature and
  enabled/total GPU counts. Power is `power.draw` summed over every detected
  card (an idle or profile-disabled card still draws, and a card whose reading
  is `N/A` is skipped), the temperature is the hottest readable card and the
  efficiency is that power over the hashrate;
- `miner` — API/log statistics (`hashrate_10s/60s/15m`, accepted, rejected,
  stale, source);
- `pool` — cached pool statistics when mining;
- `oc` — `applied` (the snapshot was applied), `needs_restore` (applied while
  stopped), snapshot availability, stale-boot flag, restore-on-stop setting,
  last restore error and a first-card summary of the restore/default targets;
- `warnings` — config warnings plus “lock not honoured” warnings and one
  “card #N is at T °C (limit L °C)” note while the hottest card is at or above
  the profile limit;
- fees, expected hashrate, temperature limit, timestamp (`ts`, `ts_iso`) and
  app version.

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
- `custom = false` per-GPU entries inherit the active profile default; only
  `custom = true` entries (written by the tuning dialog) replace it. This keeps
  the setup wizard and the enable toggle from shadowing the default.
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
  fails, the daemon parses the tail of the miner log. The parser is tolerant:
  it walks the JSON document, matches key names and prefers top-level summaries
  over per-GPU entries.

## Concurrency

- `Engine` holds `Inner { cfg, sup, status, last_report }` behind an async
  `Mutex`. Every D-Bus method takes it; blocking work (`nvidia-smi`, helper,
  spawn) runs in `spawn_blocking`/`block_in_place`.
- GPU mutations are additionally serialized across processes by the OC
  `flock`; within a process a nested acquisition is an error. A shutdown tries
  the engine lock for 5 s and otherwise leaves the teardown to `ExecStopPost` /
  the guard instead of waiting for a polkit dialog.
- The daemon runs two async loops: the status refresher (poll interval, or an
  on-demand refresh after supervisor events) and the 1 s supervisor tick. A
  third select arm watches SIGTERM/SIGINT and `connection.closed()`.
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

## Design constraints and trade-offs

- **The manager is not a miner.** No bundled binaries, no wallet, no seed
  phrases; the catalogue links to upstream releases.
- **One miner at a time.** Never spawn a second child, never silently restart a
  running miner, never kill an external process without explicit user action.
- **Stock restore is mandatory by default.** No hardcoded power limit; capture
  before touching power/clocks and restore on stop.
- **The daemon is the source of truth.** Editing `config.json` directly is not
  enough while the daemon runs — use Apply/CLI so the daemon validates and
  applies the change.
- **No `nvidia-smi` in the GUI.** Any future client can be added without
  privileged access.
- **No Flatpak.** The sandbox conflicts with the host helper, systemd
  unit and runtime binary downloads; see
  [ADR 0003](adr/0003-packaging.md).
- **Debian first.** One tested distribution beats four untested ones; see
  [packaging.md](packaging.md).

## Files and locations

| Path | Contents |
| --- | --- |
| `~/.config/trommel/config.json` | Applied config (schema v3) |
| `~/.local/state/trommel/state.json` | Running state, PID, `miner_path`, preset, restart count |
| `~/.local/state/trommel/oc.lock` | Cross-process OC `flock` |
| `~/.local/state/trommel/oc-lease.json` | Daemon/helper PID + start-time lease |
| `~/.local/state/trommel/guard.pid` | Guard PID/start time and the daemon it watches |
| `~/.local/state/trommel/guard.disarm` | Disarm token written by a graceful daemon |
| `~/.local/state/trommel/miner.log` | Miner stdout/stderr |
| `~/.local/state/trommel/daemon.log` | Daemon event log |
| `~/.local/state/trommel/oc-snapshot.json` | Per-boot stock snapshot |
| `~/.local/state/trommel/bench/bench-<ts>.json` | A/B benchmark reports |
| `~/.local/share/trommel/miners/<id>/<version>/` | Installed miner binaries |
| `/usr/libexec/trommel/trommel-oc` | Root helper |
| `/usr/share/polkit-1/actions/io.github.nayvok.trommel.policy` | Polkit action |

XDG variables are honoured; `paths.rs` centralises every location and uses a
temporary root under `cargo test` so unit tests never touch real user state.
