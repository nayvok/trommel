# Components

This is the reference for **where a behaviour lives**. It lists every component,
its public surface and the invariants a change must preserve. For how the parts
fit together, read [architecture.md](architecture.md) first.

## Component map

| Component | Path | Language | Role |
| --- | --- | --- | --- |
| `trommel-core` | `crates/core/src` | Rust | Shared model and logic; no UI, no daemon main |
| `trommeld` | `crates/daemon/src/main.rs` | Rust | User D-Bus service, status refresher, supervisor loop |
| `trommel` CLI | `crates/cli/src/main.rs` | Rust | Command-line client, daemon activation, JSON output |
| `trommel` GUI | `crates/cli/src/gui.rs` | Rust (GTK4) | libadwaita application behind `--features gui` |
| MCP server | `crates/cli/src/mcp.rs` | Rust | stdio Model Context Protocol interface |
| Catalogue | `resources/catalog/*.toml` | TOML | Built-in miners, coins and pools |
| Root helper | `polkit/trommel-oc` | Bash | Narrow `nvidia-smi` wrapper run through pkexec |
| Polkit assets | `polkit/` | XML / conf | Action definition and per-user rule template |
| systemd units | `systemd/` | INI | D-Bus activation, user service, start-at-login |
| Desktop assets | `data/` | XML / desktop / SVG | Desktop entry, metainfo, icons |
| Installer | `install.sh`, `uninstall.sh` | Bash | Developer install/removal |
| Packaging | `packaging/`, `scripts/*deb*` | Bash / spec | Debian package, GitHub release bundle, RPM template |
| Test harness | `scripts/e2e.sh`, `ui-smoke.sh`, `first-run-smoke.sh`, `test-mcp.sh` | Bash / Python | Automated verification |

---

## `trommel-core`

`crates/core/src/lib.rs` declares the modules and the shared constants:

| Constant | Value | Used by |
| --- | --- | --- |
| `BUS_NAME` | `io.github.nayvok.Trommel.Daemon` | daemon, GUI, CLI |
| `OBJECT_PATH` | `/io/github/nayvok/Trommel` | daemon, GUI, CLI |
| `APP_ID` | `io.github.nayvok.Trommel` | GUI, desktop entry, metainfo |
| `VERSION` | `CARGO_PKG_VERSION` | status, CLI, About |

### `config` — model, migration, diff, validation

The largest module and the shape of everything else.

| Item | Contract |
| --- | --- |
| `SCHEMA_VERSION` | `3`; `Config::load` migrates v1 configs and infers the v2→v3 `custom` flag |
| `Config` | `active_preset`, `presets`, `gpu_profiles`, `miner_installs`, `custom_miners`/`pools` (reserved), `behavior` |
| `Preset` | Coin/algo/miner/version/pool/wallet/worker/profile, `miner_options`, fee fields, `extra_args`, `env` |
| `GpuTuning` | `enabled` (participation, not just tuning), optional `power_limit_w`, `core_clock_mhz`, `memory_clock_mhz`, `fan_pct`, global `temp_limit_c`/`temp_resume_c`, and `custom` (per-GPU entries only) |
| `GpuProfile` | `default` tuning plus `per_gpu` entries keyed by GPU UUID; `tuning_for(uuid)` inherits the default unless the entry is `custom`; a `false` `enabled` excludes the card from the miner and from tuning |
| `Behavior` | Restart/poll/restore/login/notifications, `language`, `appearance`, `simple_mode` |
| `MinerInstall` | Global install record: `miner`, `version`, `path`, `source_url`, `verified`, `custom`, `installed_at` |
| `ApplyMode` | `Live`, `Restart`, `Stop`; `parse("live")`, `parse("restart")`, `parse("stop")` |

Key functions:

- `Config::load()` / `load_or_default()` / `save()` — `save` writes atomically
  (`write_atomic`, used by the state file too). `load` normalises, migrates v1
  if needed, and returns an error for unparsable JSON rather than silently
  discarding it.
- `Config::normalize()` — repairs a dangling `active_preset`, a missing default
  GPU profile or empty preset map, a missing install path (`verified = false`)
  and stale fee overrides for miners that do not allow them. For a v2 file it
  also infers `custom` on every per-GPU entry: an entry with no power/clock
  values inherits the default; an entry with values is custom only when they
  differ from the default.
- `Config::preset()` / `preset_mut()` / `ensure_preset()` — active preset access;
  `ensure_preset` clones the default when the map is empty.
- `Config::gpu_profile()` / `gpu_profile_mut()` — active profile access.
- `Config::install(miner, version)` / `install_for(preset)` / `miner_path(preset)`
  — global install lookup; `install_for` uses `install_key`.
- `Config::add_install()` / `remove_install()` / `install_users()` — install
  registry maintenance. No current UI/CLI calls `remove_install()`; the app only
  adds installs.
- `Config::warnings()` — user-facing validation: no preset, empty/suspicious
  wallet, pool address without a port, missing miner install, no enabled GPU,
  and “restore on stop is off” when the active profile has tuning.
- `Config::diff(next) -> ConfigDiff` — `ChangeClass::{None, Live, RestartRequired}`
  plus human-readable reasons; this drives the GUI alert and MCP `config_diff`.
- `Config::redacted()` — replaces every preset wallet with `<redacted>`; used by
  `diagnose` and the MCP tools, never by the daemon's own state.
- `Config::from_v1(ConfigV1)` — one-time migration of the former flat config;
  builds a `pearl-imported` preset and an `imported` profile.

### `catalog` — TOML catalogue and install status

- Types: `MinerDef`, `MinerVersion`, `MinerOption`, `OptionKind`
  (`int|bool|enum|string`), `OptionScope` (`global|gpu`), `CoinDef`, `PoolDef`.
- `MinerDef::selects_devices()` is true only when `per_gpu_devices` **and** a
  non-empty `device_flag` are present; the flag is documented upstream, never
  guessed. Built-in flags: WildRig `--gpu-list`, SRBMiner `--gpu-id`, OneZero
  `--devices`.
- `catalog()` parses `resources/catalog/{miners,coins,pools}.toml` once
  (`OnceLock`), and exposes `miners()`, `miner(id)`, `coins()`, `coin(id)`,
  `pools()`, `pool(id)`, `miner_fee_pct(id)`, `pool_fee_pct(id)`.
- `installed_versions(cfg, miner_id)` — versions that have a record **and** an
  existing binary on disk.
- `install_status(cfg, preset)` — `Installed { version, verified, custom }`,
  `OtherVersion { installed }` or `NotInstalled`. It is used by tests and is the
  reference classifier; the GUI derives the visible state with `install()` and
  `installed_versions()`.
- Localised helpers: `MinerDef::short_description()`, `MinerOption::label()`,
  `help()`, `CoinDef::description()`, `PoolDef::note()` choose `_en`/`_ru`.
- `MinerOption::default_value()` converts the TOML string default to a typed
  JSON value.

Adding or changing fields is documented in [miners.md](miners.md).

### `paths` — XDG locations

| Function | Default |
| --- | --- |
| `config_dir()` / `config_path()` | `$XDG_CONFIG_HOME/trommel/config.json` |
| `data_dir()` | `$XDG_DATA_HOME/trommel` (miner store lives here) |
| `state_dir()` / `state_path()` | `$XDG_STATE_HOME/trommel` (state, logs, snapshot, bench) |
| `log_path()` | `state_dir()/miner.log` |
| `supervisor_log_path()` | `state_dir()/daemon.log` |
| `bench_dir()` | `state_dir()/bench` |
| `legacy_config_path()` | `$XDG_CONFIG_HOME/pearl-miner/config.json` (read-only migration) |
| `ensure_dirs()` | Best-effort creation of config/state/bench directories |

Under `cargo test` all paths resolve below a per-process temporary directory, so
unit tests cannot read or clobber real rig state. The e2e scripts point
`XDG_CONFIG_HOME`/`XDG_STATE_HOME` at their own fixtures.

### `nvidia` — GPU query

- `GpuStats` — index, UUID, name, temperature, fan, power draw/limit/default/min/max,
  core/memory clock, utilisation, VRAM used.
- `query_all()` runs one `nvidia-smi --query-gpu=… --format=csv,noheader,nounits`
  and parses each non-empty line; `query()` returns the first card;
  `parse_line()` is unit-tested independently.
- The GUI never calls this module; only the daemon and CLI `gpus`/`diagnose` do.

### `oc` — snapshot, helper protocol, restore

- `HELPER_PATH = /usr/libexec/trommel/trommel-oc`; `helper_path()` honours
  `TROMMEL_OC_HELPER` for tests, `helper_installed()` checks the file exists.
- `OcSnapshot { boot_id, created_at, applied, applied_gpus, applied_tuning,
  last_error, gpus }` and `GpuSnapshot` (`was_tuned` records a pre-existing
  non-default limit). `restore_power_w(mode)` targets the captured limit
  (`Snapshot`) or the driver default (`Defaults`).
- `snapshot()`, `driver_default_snapshot()` (`fallback_snapshot` is an alias),
  `save_snapshot()`, `load_snapshot()`, `clear_snapshot()`, `record_error()`,
  `cleanup_stale_snapshot()`, `current_boot_snapshot()`, `current_boot_id()`.
- `validate_uuid()` and `validate_apply_args()`/`validate_reset_args()` — the
  in-process mirror of the helper's checks (`GPU-…`, numeric ranges, at least
  one tuning parameter, optional `--lease-pid/--lease-start` pair). `--lmc`
  accepts the real 100–12000 MHz range. A partial or non-numeric lease is
  rejected.
- `apply_tuning(gpu, tuning)` — acquires `OcLock`, then performs the no-op check,
  validation, helper call and power read-back (±1 W). `apply_tuning_locked` is
  the variant for callers that already hold the lock.
- `restore_gpu(snapshot, mode)` / `restore_snapshot(snapshot, mode)` — acquire
  the lock, restore every applied card, verify the power limit, clear the
  snapshot only when every restore succeeded. `restore_gpu_locked` and
  `restore_snapshot_locked` do not acquire; `restore_snapshot_with_timeout` and
  `restore_snapshot_locked_with_timeout` accept a short helper timeout for
  shutdown/emergency.
- `OcLock` — advisory `flock` on `state_dir/oc.lock`; `acquire_lock()` uses the
  15 s standard timeout and a process-global flag rejects nested acquisition
  instead of deadlocking.
- `HelperLease`, `load_lease()`, `wait_for_lease(timeout)`, `kill_lease_processes()`,
  `helper_lease_args()` — the daemon writes the spawned helper/`pkexec` PID and
  its `/proc` start time; emergency-stop waits, then terminates only processes
  carrying the exact lease marker. The helper receives the daemon PID/start and
  aborts before mutating when it no longer matches.
- `verify_power()`, `runtime_lock_issues()` — synchronous power check and the
  under-load clock comparison used by the supervisor.
- `run_helper(args)` invokes `pkexec /usr/libexec/trommel/trommel-oc …` or
  `$TROMMEL_OC_HELPER` (directly when already root), with a 180 s default
  timeout (`TROMMEL_OC_TIMEOUT_MS` for tests) and actionable errors (cancelled
  pkexec, auth timeout, helper failure). It appends the caller's lease flags,
  writes `oc-lease.json` for the spawned helper and clears it on exit.

Helper argument contract (also enforced by `polkit/trommel-oc`):

```
trommel-oc apply --gpu GPU-xxxx [--pl W] [--lgc MHz] [--lmc MHz] \
                 [--lease-pid PID --lease-start STARTTIME]
trommel-oc reset --gpu GPU-xxxx [--pl W] [--lease-pid … --lease-start …]
trommel-oc status --gpu GPU-xxxx
```

Ranges: `--pl` 50–1000 W, `--lgc` 100–4000 MHz, `--lmc` 100–12000 MHz;
`--lease-pid` 1–4194304 and `--lease-start` a positive `/proc` field-22 value.
No paths, no shell. The helper verifies the lease before its first `nvidia-smi`
mutation. `apply` waits for the power limit to read back; `reset` reports
failing `-rgc`/`-rmc` instead of swallowing them.

### `miners` — argument construction and device selection

`build_args(cfg, preset, log_file) -> Vec<String>` is the "all detected cards"
form; `build_args_with_devices(cfg, preset, log_file, Option<&[u32]>)` adds the
documented device flag. `device_plan(cfg, preset, gpus)` decides whether an
explicit list is needed (`None` when every card participates), rejects an empty
rig, and returns the explicit unsupported-miner error for a partial rig. The
argument builder is the only place that knows miner CLI shapes. Core flags:

| Driver | Match | Shape |
| --- | --- | --- |
| WildRig / generic | any unknown `miner` id | `--algo`, `--url`, `--user`, optional `--worker`, `--api-port`, `--api-worker-id`, `--log-file`, `--no-color`, `--print-time 10`, `--gpu-temp-limit`, `--gpu-temp-resume` |
| SRBMiner | `"srbminer"` | `--algorithm`, `--pool`, `--wallet wallet.worker`, `--password x`, optional API flags/log |
| OneZero | `"onezero"` | `-a`, `-o`, `-u`, optional `-w` |

The device flag is emitted before catalogue options and contains the
comma-separated `nvidia-smi` indices of the participating cards. It is only
emitted for a catalogue miner with `selects_devices()`; a device flag inside
`extra_args` is rejected to avoid two contradictory selections.

Catalogue options are appended by `append_options`: bool options emit the flag
when true, int options emit `flag value` within `min`/`max` (and skip
`fan_speed = 0` as “auto”), enum/string options emit non-empty text. A
configurable fee emits `fee_flag value` only when `miner_fee_override_pct` is
set. `extra_args` is parsed with `shell-words` and appended verbatim as separate
arguments — no shell.

### `supervisor` — miner process lifecycle

- `Event::{Started, Crashed, Stopped, Error, Warning, UserStopped}`; `message()`
  produces localised notification text.
- `Supervisor::new()` is the test/tool constructor; `Supervisor::with_config(cfg)`
  is the daemon entry point. It backfills `state.miner_path` from the active
  preset's install before normalising, so an old state file still resolves its
  binary. A live miner from a previous daemon is marked `orphan` until startup
  recovery stops it; adoption never resumes mining.
- `start(cfg)` — the Start algorithm from [architecture.md](architecture.md#start):
  external-miner guard, preset validation, participation + device-capability
  check, effective tuning, restoration of a now-excluded card that still had
  restore-off tuning, snapshot intent, per-GPU apply + power verification with
  rollback, spawn, state update. It holds the OC lock across the whole block and
  records the absolute `state.miner_path`. The chosen device list is kept for
  crash restarts. A profile with no tuning creates no snapshot and calls no
  helper.
- `stop(cfg) / shutdown(cfg)` — terminate only our child/PID (path-checked),
  skip and clear external state, restore the applied snapshot (unless disabled),
  clear state; propagates restore errors. `shutdown` forces the restore with a
  20 s helper timeout. Both hold the OC lock.
- `owns_miner()` — `running && !external` and a live child or path-matching PID;
  the benchmark uses it before stopping anything.
- `tick(cfg) -> Vec<Event>` — one-second supervisor step: reap, bounded restart
  with backoff (with the same device list as the session), detect adoption
  loss, emit events, and run the one-shot clock check after the GPU reaches
  load.
- `handle_unexpected_exit` — the bounded restart policy: up to five consecutive
  restarts (2/4/8/… s, cap 60), then stop + restore + error; `started_at` resets
  on every spawn.
- `terminate_pid`, `log_line` — process helper and daemon log writer.

### `state` — runtime state file

`State { running, miner_pid, miner_path, started_at, restarts, last_error,
external, orphan, preset_id, miner, miner_version, profile }` (the last is a
legacy-parse field; `miner_path` is additive with a serde default).

- `load()`, `save()`, `clear_file()`, `uptime_s()`, `miner_alive()`.
- `normalize()` — a running flag without a matching process becomes stopped,
  including the phantom `external` state that used to make Start a no-op.
- `proc_start_time(pid)` / `pid_alive(pid)` — `/proc/<pid>/stat` field 22 and a
  liveness check; the start time catches PID reuse.
- `pid_matches_binary(pid, path)` — `/proc/<pid>/exe` plus the interpreter
  argument for `#!` scripts. A mismatch is never treated as ours.
- `pid_matches_miner(pid)` — legacy name heuristic used only for old state files
  without `miner_path`.
- `find_external_miner(cfg, exclude_pid)` — `/proc` scan using the catalogue
  names plus every installed miner's directory/binary basename; skips our own
  and `exclude_pid`. `TROMMEL_NO_EXTERNAL_SCAN=1` disables it for test isolation.

### `emergency` — idempotent teardown

- `Reason::{ServiceStop, Guard, Startup, Manual}` with `parse()`/`as_str()`.
- `run(reason)` — take the OC lock, drop a stale-boot snapshot, wait up to 10 s
  for a helper lease and terminate it if needed, stop **our** miner only
  (path-checked; external state is left alone), clear state after the stop and
  restore a current-boot applied snapshot with a 20 s timeout. Restore failure
  keeps the snapshot + `last_error` and returns an error. `Outcome` records the
  stopped PID, restored card count and whether an external miner was skipped.
- `startup_recovery()` — `run(Reason::Startup)`; called by `trommeld` before it
  owns the bus name. It never resumes mining.

### `guard` — independent supervisor for manual runs

- `spawn_guard()` — start `trommel guard --daemon-pid … --daemon-start …` via
  `setsid` (skipped with `TROMMEL_NO_GUARD`), terminate a stale guard from a
  previous session first.
- `run(daemon_pid, daemon_start)` — write `guard.pid`, poll the daemon's PID and
  `/proc` start time; a matching `guard.disarm` token exits cleanly, otherwise a
  disappeared daemon leads to `emergency::run(Reason::Guard)`.
- `disarm_and_stop()` — write the token for this exact daemon, signal the guard
  PID (start-time checked) and remove `guard.pid`. Called before the graceful
  restore so a normal shutdown cannot race a second restore.

### `status` — the status document

- `CONTRACT_VERSION = 3`.
- `Status` composition and collection are described in
  [architecture.md](architecture.md#status-contract).
- `Status::collect(cfg, state)` is blocking (nvidia-smi, HTTP pool, log tail) and
  is always called from a blocking context. It also builds `participation[]`
  from the applied profile and the running state, and adds the “excluded from
  mining” / “miner cannot choose GPUs” warnings.
- `Status::stopped(cfg)` is the cheap snapshot used when stopping.
- `OcView` — `applied`, `needs_restore`, `snapshot_available`, `stale`,
  `restore_on_stop`, `last_error`, snapshot age/boot and a first-card target
  summary. `needs_restore` drives the dashboard banner.
- `lock_warnings()` — memory lock compared under load, core lock only when the
  card is under load; the tolerance absorbs rounding.
- `read_tail(path, max_bytes)` — log tail reader used by the miner parser.
- `acceptance_pct()` — accepted/(accepted+rejected) for display.

### `dbus` — service and proxy

`Engine` is the service:

| Method | Behaviour |
| --- | --- |
| `Start()` / `Stop()` / `Toggle()` | Delegate to the supervisor; return errors as D-Bus failures |
| `Status()` | Last collected status JSON (no refresh) |
| `Refresh()` | Recollect status now and return it |
| `GetConfig()` | Applied config JSON (not redacted; local clients only) |
| `ApplyConfig(json, mode)` | Validate, diff, stop/restart/save with rollback; returns applied/restarted/stopped/changes JSON |
| `SetConfig(json)` | Compatibility wrapper: `ApplyConfig(json, "restart")` |
| `RunBenchmark(request)` | `{"profiles": [], "seconds": 180}`; refuses while an external or unowned miner runs; returns a report |
| `OCReport()` | Read-only JSON: snapshot, current limits, per-card targets for `snapshot` and `defaults` |
| `RestoreOC(mode)` | Restore `snapshot` or `defaults`; returns a JSON report; refuses while mining |
| `ResetOC()` / `RestoreStock()` | Compatibility wrapper: `RestoreOC("snapshot")` (falls back to defaults) |
| `OpenApp()` / `Quit()` | Launch the GUI / terminate the daemon |

`Engine` also holds an `oc_busy` flag: a second OC operation is rejected while
one is waiting for authorization. The status refresher uses `try_snapshot()` so
a 180 s auth dialog does not freeze status collection. `Engine::new()` uses
`Supervisor::with_config`; `Engine::shutdown()` is the bounded graceful stop
used by the daemon signal/bus handler.

Properties: `Running`, `Version`, `ContractVersion`. Signals:
`StatusChanged(json)` and `Notification(summary, body)`.

`MinerProxy` (generated by `zbus::proxy`) is the client trait used by the GUI,
CLI and MCP. `poll_interval(cfg)` clamps the configured interval to 1–60 s;
`events_to_notifications(events)` maps supervisor events to notification
strings; `state_snapshot()` reads the state file for tools that cannot ask the
daemon.

### `pool` — pool statistics

- `PoolStats` — fee, balance (mined, unpaid), immature, paid, pool hashrate,
  workers online, last payout, minimum payout.
- `account_url(wallet)`; `query(cfg)` speaks the PearlHash account API, only for
  the `pearlhash` pool. Other pools show configured data only.
- `query_cached(cfg)` — non-blocking facade with a 60 s TTL and 45 s failure
  backoff; the first call returns the previous value and spawns the fetch.
- `parse_json()` also handles generic pool JSON by key matching.
- `invalidate_cache()` — called after start/stop/apply.

### `minerapi` — hashrate and share parsing

- `MinerStats { hashrate_10s, hashrate_60s, hashrate_15m, accepted, rejected, stale, source }`.
- `query_api(port)` — `http://127.0.0.1:<port>/` with a 1.2 s timeout.
- `parse_json(value)` — walks numbers, matches key names, prefers top-level
  summaries; tolerant by design because miners disagree on JSON shape.
- `parse_log(text)` — fallback parser for miner output, including ANSI stripping
  and localised decimal separators.
- `primary()` picks 10 s → 60 s → 15 m; `is_empty()` drives the “no data” state.

### `bench` — A/B benchmark

- `BenchEntry` — profile, name, version, raw/effective hashrate, fees, samples,
  accepted shares, pool hashrate, error.
- `BenchReport::best()` — highest effective hashrate.
- `run(cfg, profiles, seconds)` — sequential runs, warm-up (seconds/3 clamped to
  10–45 s), sampling every 2 s, effective = raw × (1 − fees), 5 s gap between
  miners. The caller stops/restarts the real miner (daemon `RunBenchmark` does).
- `save(report)` — `state/bench/bench-<timestamp>.json`.
- `run` assumes the OC profile is already applied and never applies tuning
  itself.

### `download` — install and import

- `DownloadRequest { miner, version, url, sha256, archive, binary, source_url }`.
- `validate_component(value, field)` — non-empty, not `.`/`..`, no `/`, `\` or
  NUL, exactly one path component. `install`, `import_binary` and
  `install_from_bytes` call it before the network/write.
- `install(req, root)` — HTTPS only, 512 MiB cap, optional SHA-256,
  `tar.gz`/`tgz`/`zip`/raw; rejects absolute paths and `..`; requires an
  executable (`ELF` or `#!`), moves atomically into the store, returns a
  `MinerInstall`.
- `import_binary(miner, version, source, root)` — copies a local executable into
  the store (so the preset does not depend on the user's download directory),
  validates the executable format.
- `miner_root()` → `data_dir()/miners`; `install_dir()` →
  `…/<miner>/<version>`; `install_key_for()` → `id@version`.
- `install_from_bytes()` is a test helper and must not be used in production
  code paths.

### `diagnostics`, `i18n`, `util`

- `diagnostics::report(cfg, status)` — JSON bundle for agents: app/version,
  contract version, paths, **redacted** config, status, warnings. The CLI adds
  GPU data.
- `i18n` — `Language::{System, En, Ru}`, `set_language`, `current`, `is_ru`,
  `tr(en, ru)`, `tr_ref(en, ru)`. Details in [i18n.md](i18n.md).
- `util` — hashrate formatting (`format_hashrate`, compact variant), tolerant
  hashrate parsing, Unix time, ISO-8601 UTC, human durations. No dependencies;
  keep it that way.

---

## `trommeld`

`crates/daemon/src/main.rs` is intentionally small:

1. `paths::ensure_dirs()`, `Engine::new()`, `zbus::connection::Builder` serves
   `Engine` at `OBJECT_PATH` on `BUS_NAME`.
2. **Status refresher** — every `poll_interval` (or when a supervisor event
   requests it) collects `Status`, emits `StatusChanged` only when the JSON
   changed, updates `Engine`, and sends one overheat desktop notification per
   episode when the hottest card (`Status::hottest_gpu`) reaches the temperature
   limit; the body names that card.
3. **Supervisor loop** — a 1 s interval calling `Supervisor::tick`, logging
   events, sending one desktop notification (and the `Notification` signal) per
   event, requesting a refresh.
4. **Startup recovery** — before the bus name is served, stop an orphaned miner
   from the previous daemon, restore an applied snapshot and stay stopped.
5. **Shutdown** — SIGTERM/SIGINT or a closed session bus disarm the guard,
   stop only our miner, force a short-timeout restore and exit. systemd's
   `ExecStopPost` and the guard cover a SIGKILL; both call the same
   `trommel emergency-stop`.

`trommeld` is single-purpose: do not put policy here that belongs in
`trommel-core` (it would be untestable without D-Bus).

---

## `trommel` CLI

`crates/cli/src/main.rs` defines the clap command tree. Global behaviour:

- `connect()` resolves the session bus and starts `trommeld` through D-Bus
  activation (falling back to `trommeld` on `PATH`) when no one owns the name,
  then waits up to ~4 s.
- The `gui` feature is optional; `gui`/no-subcommand fails with a build hint in
  a CLI-only build.
- MCP runs on a plain thread because tool handlers create their own runtimes.

| Command | Behaviour |
| --- | --- |
| `status [--json]` | Human summary or the Status JSON contract |
| `start` / `stop` / `toggle` | Supervisor actions; Stop restores stock unless disabled |
| `oc report [--json]` / `oc restore --mode snapshot\|defaults` | Inspect or restore GPU state explicitly |
| `gpus [--json]` | `nvidia-smi` view without the daemon |
| `bench [--minutes N] [--miners a,b]` | Daemon benchmark, human table |
| `preset list/show/use/export/import` | Preset management; `use --restart/--stop` |
| `miner list/import/install` | Catalogue and miner store; applies the install to a running daemon |
| `config path/get/reset/validate/export/diff/apply` | Config lifecycle; `apply --restart/--stop` |
| `diagnose [--json]` | Redacted bundle (config, status, GPU data) |
| `mcp [--allow-write]` | MCP server over stdio |
| `daemon` | Runs `trommeld` in the foreground |
| `gui` / no subcommand | Opens the GTK app |

`config apply` and `preset use` save the file first and then ask the daemon to
apply it; if no daemon is running they report “config saved; daemon is not
running” instead of failing. `miner install/import` behaves the same way so a
running daemon sees the new install immediately.

Human output is currently Russian; JSON output is the stable machine contract.
See [agents.md](agents.md) for the agent-facing subset.

---

## GTK GUI

`crates/cli/src/gui.rs` is a single module (~5k lines). Keep the following
structure when changing it.

### State model

`App` holds:

- `config` — the **draft** (`RefCell<Config>`), edited by every widget;
- `applied` — the last config confirmed by the daemon;
- `dirty` — whether the draft differs and the “unsaved changes” banner shows;
- `status` — the last `Status` document;
- widget handles for the header, hero card, GPU flow, settings rows, the preset
  group and the optional miner-library group;
- `tx: UnboundedSender<Cmd>` — the only way to reach D-Bus.

`Cmd::{Refresh, Toggle, Apply{json,mode,quiet}, ApplyStart{json}, ResetOc,
Bench{minutes}}` goes to the worker thread;
`Ev::{Status, Config, ServiceDown, Error, Applied{message,quiet}, Bench}` comes
back. GTK widgets are only touched from `handle_event`/`apply_status` on the
main thread.

### Worker thread

`spawn_worker` starts a named thread with a current-thread tokio runtime. The
loop polls `Status()` every second while connected, reconnects on demand, and
executes commands. It never blocks the GTK main loop.

### Pages and dialogs

| Function | Surface |
| --- | --- |
| `build_ui` | Window, header view switcher, dashboard hero action, three pages, banners, `AdwDialog` host |
| dashboard block in `build_ui` | Hero card, GPU `AdwWrapBox`, totals, shares, pool, warnings |
| `build_presets_page` / `refresh_preset_list` | Preset list, missing-version hint and Install entry, miner library row; rows open the editor |
| `miner_library_dialog` / `refresh_miners` | Modal miner library (catalogue, installed versions, preset usage, benchmark) and its refresh while open |
| `build_settings_page` / `load_settings` / `save_settings` | Behaviour (incl. the global temperature policy), appearance, actions |
| `preset_editor_dialog` | Preset editor `AdwDialog` (save/cancel/duplicate/delete, opens the library) |
| `miner_install_dialog` | The only miner install UI (catalogue, custom version, download, import) |
| `gpu_tune_dialog` | Per-card tuning `AdwDialog`, “Now” row, copy to similar/all GPUs |
| `quick_setup_wizard` | Three-step first-run flow (cards → miner → wallet) |
| `dialog_footer` / `dialog_body` | Shared dialog chrome: header keeps the title, actions go in a footer below the content |
| `save_and_apply_with` | Diff + restart/stop alert; `on_cancel` restores optimistic changes |
| `apply_status` | Renders a `Status` into every widget, including empty states and participation; records sparkline samples |
| `History` / `sparkline` / `update_sparklines` | Per-GPU five-minute ring buffer and the Cairo sparklines drawn on the GPU bubble floor |
| `copy_diagnostics` | Redacted bundle to the clipboard |

Rules that are easy to break:

- The dashboard hero **Start** applies the draft first (`Cmd::ApplyStart`),
  then starts. Starting with the daemon's old config silently ignored GPU edits;
  the button lives in the hero card, not in the header, so the header switcher
  stays single-line at narrower widths.
- A dirty draft survives `Refresh`; only an explicit apply clears it.
- Toggling a GPU or editing an appearance/simple-mode switch updates the draft
  immediately, and **Cancel** in the restart/stop alert must undo it.
- The GPU participation switch and the tuning dialog share one profile; there is
  no second source of truth in the UI. The switch edits the draft; the card
  chip (`mining` / `excluded`) and the Totals row describe the applied session,
  so a switch change never pretends the miner already restarted.
- Dialogs never put actions in the header: build them with `dialog_footer()` so
  the title stays clean, actions never scroll away and the primary button sits
  on the trailing edge.
- Every user-visible string goes through `tr(en, ru)` (except the screenshot and
  test hooks).

### Screenshot and test hooks

Environment variables read by the GUI are a supported test surface; keep them
documented in [development.md](development.md#ui-hooks) when adding one:
`TROMMEL_COLOR_SCHEME`, `TROMMEL_WINDOW_WIDTH`, `TROMMEL_WINDOW_HEIGHT`,
`TROMMEL_SCREENSHOT`, `TROMMEL_SCREENSHOT_PAGE`, `TROMMEL_SCREENSHOT_DELAY`,
`TROMMEL_SCREENSHOT_SCROLL`, `TROMMEL_OPEN_WIZARD`, `TROMMEL_OPEN_PRESET`,
`TROMMEL_OPEN_LIBRARY`, `TROMMEL_OPEN_INSTALL`, `TROMMEL_OPEN_TUNE`,
`TROMMEL_WIZARD_STEP`, `TROMMEL_WIZARD_SCREENSHOT`, `TROMMEL_TUNE_SCREENSHOT`,
`TROMMEL_PRESET_SCREENSHOT`, `TROMMEL_LIBRARY_SCREENSHOT`,
`TROMMEL_INSTALL_SCREENSHOT`.
`scripts/screenshots.sh` is the canonical driver.

---

## Notifications

`behavior.notifications` gates two consumers of the same event stream:

- `dbus::notify_desktop(connection, summary, body)` calls
  `org.freedesktop.Notifications.Notify` (app name `Trommel`, app icon, 8 s
  timeout) inside a 2 s bound; a missing service or a failure is ignored so the
  daemon never blocks. The status refresher sends one overheat alert per
  episode for the hottest card and names it in the body; the supervisor loop
  sends one per `Event` through `events_to_notifications`.
- The `Notification(summary, body)` D-Bus signal stays available for external
  integrations. Both paths use the same `i18n::tr` strings.

There is no shell extension and no GSettings schema; the GUI only edits the
`behavior.notifications` switch.

---

## Assets and packaging

| Asset | Purpose |
| --- | --- |
| `data/io.github.nayvok.Trommel.desktop` | Application menu entry |
| `data/io.github.nayvok.Trommel.metainfo.xml` | AppStream metadata and release notes |
| `data/icons/*.svg`, `data/icons/src/trommel.svg` | Installed icons; source artwork |
| `systemd/trommeld.service` | User service (`Type=dbus`) |
| `systemd/trommel-autostart.service` | “Start mining at login” one-shot |
| `systemd/io.github.nayvok.Trommel.Daemon.service` | D-Bus activation file |
| `polkit/io.github.nayvok.trommel.policy` | Annotated action for the helper |
| `polkit/50-trommel.rules.in` | Rule template for the installing user |
| `packaging/deb/{postinst,prerm,postrm}` | Package maintainer scripts |
| `packaging/github/trommel-install.sh` | User-facing release installer |
| `packaging/rpm/trommel.spec.in` | Unbuilt RPM template |
| `packaging/appimage/README.md`, `packaging/flatpak/README.md` | Deferred-format rationale |

## Scripts

| Script | Use |
| --- | --- |
| `scripts/e2e.sh` | Private D-Bus, fake GPU/helper/miner end-to-end CLI checks |
| `scripts/ui-smoke.sh` + `ui_smoke.py` | AT-SPI click-through of the GTK app |
| `scripts/first-run-smoke.sh` | Pristine-install path (no config, no wallet, no miner) |
| `scripts/test-mcp.sh` | MCP protocol, read-only default, redaction |
| `scripts/screenshots.sh` | Deterministic docs screenshots from a fake rig |
| `scripts/package-deb.sh` / `test-deb.sh` | Build and validate the `.deb` |
| `scripts/release-bundle.sh` | Copy release assets and stable aliases |
| `scripts/check-polkit.sh` | Action id/helper path consistency |
| `scripts/check-no-secrets.sh` | Tracked-file secret/wallet scan |
| `scripts/install-polkit-rule.sh` | Install helper, policy and per-user rule |
| `scripts/check-docs.sh` | Relative markdown link and stale-name check |
| `scripts/render-icons.sh` | Optional PNG icon rendering |

## Where to change what

| Change | Touch |
| --- | --- |
| Add a miner/version/option | `resources/catalog/miners.toml`, tests in `crates/core/src/miners.rs`, [miners.md](miners.md) |
| Add a pool or coin | `resources/catalog/*.toml`; a parser adapter only with a stable API |
| Change miner arguments | `crates/core/src/miners.rs` (`build_args` / `append_options`) and its tests |
| Change GPU apply/restore | `crates/core/src/oc.rs`, `polkit/trommel-oc`, `scripts/e2e.sh` |
| Change per-GPU participation | `crates/core/src/miners.rs` (`device_plan`, `build_args_with_devices`), catalogue `device_flag`, `crates/core/src/supervisor.rs`, `status.rs`, GUI cards/totals, [ADR 0007](adr/0007-per-gpu-participation.md) |
| Change the config model | `crates/core/src/config.rs`, `docs/schema/*.json`, migration/normalise tests |
| Add a D-Bus method | `crates/core/src/dbus.rs` (trait + impl), CLI/MCP callers, [architecture.md](architecture.md) |
| Add a status field | `crates/core/src/status.rs`, GUI rendering, CLI/MCP output |
| Add a GUI page/dialog | `crates/cli/src/gui.rs`, [design.md](design.md), screenshots, UI smoke |
| Add a CLI command | `crates/cli/src/main.rs`, [agents.md](agents.md), [user-guide.md](user-guide.md) |
| Add an MCP tool | `crates/cli/src/mcp.rs`, `scripts/test-mcp.sh`, [agents.md](agents.md) |
| Change packaging | `packaging/`, `scripts/package-deb.sh`, `.github/workflows/`, [packaging.md](packaging.md) |
