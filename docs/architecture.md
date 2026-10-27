# Architecture

```
GUI (draft Config) ──D-Bus──> trommeld (applied Config, Supervisor, State)
                                  ├── polkit helper → nvidia-smi (apply/restore)
                                  ├── exactly one miner child
                                  ├── miner HTTP API + log + nvidia-smi
                                  └── StatusChanged (JSON) ──> GUI / extension
App ── GSettings org.gnome.shell.extensions.trommel ──> extension (UI only)
Extension ── D-Bus ──> trommeld (status/start/stop); never OC
```

## Crates

- `trommel-core`: configuration v2, catalogue, download manager, NVIDIA query,
  OC snapshot/helper, supervisor, status contract, D-Bus engine.
- `trommeld`: serves `io.github.nayvok.Trommel.Daemon`, polls status, reaps the
  miner, schedules restarts and emits notifications.
- `trommel`: CLI and GTK4/libadwaita UI. The GUI is one `gui.rs` module for now;
  it keeps a draft and never runs `nvidia-smi` itself.

## Configuration

`~/.config/trommel/config.json`, `schema_version: 2`.

- `presets` — coin/algo/miner + version/pool/wallet/worker/GPU profile/options
  and fee data.
- `gpu_profiles` — `default` tuning plus per-GPU overrides keyed by UUID.
- `miner_installs` — **global** registry keyed `id@version`; presets reference
  a version, never a path.
- `behavior` — restart/poll/restore/logins/language.

Old flat Pearl Miner configs are migrated from `~/.config/pearl-miner` on load;
the old file is left untouched.

## Apply flow

`ApplyConfig(json, mode)` with `mode = live | restart | stop`.

- `live` is rejected if the running profile needs a restart.
- `restart` stops (restores stock), saves the new config, starts it again.
- `stop` stops and saves without starting.
- On start failure the old config is saved back and a best-effort stock restore
  is attempted.

## Status contract

`Status` carries `contract_version`, `state`, `preset`, `miner_info`, `gpus[]`,
`total`, `miner`, `pool`, `oc`, `warnings` and fees. The extension only reads
this document and sends Start/Stop/Toggle/OpenApp.

## Stock snapshot

Before applying a profile the daemon writes `oc-snapshot.json` with `boot_id`
and per-GPU power/clocks. On stop the helper restores `power.limit` (or
`power.default_limit` as fallback) and resets `-lgc/-lmc`. A snapshot from a
different boot is stale and removed. Pre-existing manual clock locks cannot be
read back through `nvidia-smi`; this limitation is shown in the UI.

## Security boundaries

- The root helper is `/usr/libexec/trommel/trommel-oc`; it accepts only a GPU
  UUID and numeric `--pl/--lgc/--lmc` values.
- The daemon runs as the user; polkit allows an active local user to call the
  exact helper.
- Miner processes are spawned directly with an argument vector; no shell.
- Downloads are HTTPS-only, size-limited, optionally SHA-256 verified, and
  extracted with path-traversal checks.

## Extension

`trommel@nayvok.github.io` has no `prefs.js`. Its UI settings live in
`org.gnome.shell.extensions.trommel`, shared with the app. The extension starts
the daemon through D-Bus activation and reconnects when the owner disappears.
