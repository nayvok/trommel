# Using Trommel from an external agent

Trommel is designed to be inspected and changed by an agent without a private
API. The daemon is the single source of truth; everything an agent needs is
reachable through JSON, files, the CLI, D-Bus or the MCP server.

This document is the contract. [AGENTS.md](../AGENTS.md) contains the
repository-level working rules; [architecture.md](architecture.md) explains the
runtime.

## Surface

| Surface | Access | Read/write | Notes |
| --- | --- | --- | --- |
| Config | `~/.config/trommel/config.json` | read; write only through Apply | Schema v3; see [schemas](schema/config.schema.json) |
| Runtime state | `~/.local/state/trommel/state.json` | read | PID, `miner_path`, running, restarts, last error, external/orphan |
| Miner log | `~/.local/state/trommel/miner.log` | read | Tail for hashrate/share fallback |
| Daemon log | `~/.local/state/trommel/daemon.log` | read | Supervisor events |
| GPU snapshot | `~/.local/state/trommel/oc-snapshot.json` | read | Per-boot stock power/clocks, `applied`, `last_error` |
| OC lock/lease | `~/.local/state/trommel/oc.lock`, `oc-lease.json` | read-only | Cross-process serialization and the in-flight helper |
| Guard | `~/.local/state/trommel/guard.pid`, `guard.disarm` | read-only | Watch state for the manual-run supervisor |
| Benchmarks | `~/.local/state/trommel/bench/*.json` | read | A/B reports |
| Miner store | `~/.local/share/trommel/miners/<id>/<version>/` | read | Never hand-edit; use `trommel miner import/install/remove` |
| Status JSON | `trommel status --json` | read | The GUI/CLI contract |
| Diagnostics | `trommel diagnose --json` | read | Redacted bundle for a model |
| Config diff/apply | `trommel config diff/apply` | read/apply | Explicit mode, explicit consent |
| D-Bus | `io.github.nayvok.Trommel.Daemon` | read + control | Same user session; local trust |
| MCP | `trommel mcp` | read-only by default | stdio JSON-RPC |

Paths honour XDG variables. `trommel diagnose --json` prints the effective
`config_path`/`state_path` in its report, so an agent should read them from there
instead of hardcoding `$HOME`.

## Safe workflow

```bash
# 1. Observe
trommel diagnose --json > /tmp/trommel-diagnostics.json
trommel status --json > /tmp/trommel-status.json
trommel config export /tmp/trommel-config.json

# 2. Propose a change to /tmp/trommel-proposed.json
#    Keep ids, schema_version and unknown fields intact.

# 3. Validate and classify
trommel config validate /tmp/trommel-proposed.json
trommel config diff /tmp/trommel-proposed.json

# 4. Show the diff to the user; apply only after approval
trommel config apply /tmp/trommel-proposed.json --restart
```

- `config validate` prints warnings and **exits 0 even with warnings**. Treat a
  non-empty warning list as a failure for the proposed config.
- `config diff` prints `<class>: <reasons>`, where class is `None`, `Live` or
  `RestartRequired`. A running miner with `RestartRequired` must go through
  `--restart` or `--stop`; a plain apply is rejected by the daemon.
- `GpuTuning.enabled = false` excludes a card from the mining session, not just
  from tuning: the miner is started with the participating `nvidia-smi` indices
  only and the excluded card is not tuned. Changing participation is a restart
  change. A partial rig on a miner without a documented device flag is refused
  with an explicit error; do not “fix” it by removing the exclusion silently.
- `config apply` saves the file first and then asks the daemon. If no daemon is
  running it prints “config saved; daemon is not running”; the change takes
  effect on the next start, not silently now.
- `trommel stop` reports whether the stock snapshot was restored. If restore on
  stop is disabled, the snapshot is kept and `oc.needs_restore` is true; use
  `trommel oc restore` (an explicit action) to put the GPU back.
- `trommel emergency-stop --reason=manual` is a hidden, idempotent teardown for
  a daemon that is already gone or wedged. It waits for the OC lock and for an
  in-flight helper, stops only the miner whose PID still matches
  `state.miner_path`, restores the snapshot and keeps it on failure. Do not use
  it as a substitute for `trommel stop` while the daemon is healthy, and never
  delete the snapshot to “fix” a failed restore.
- `trommel oc restore` refuses while a miner is running. Modes: `snapshot`
  (captured pre-mining limit) and `defaults` (driver default, clocks unlocked).
  The returned JSON includes per-card targets and a human message.
- Never apply without showing the user the diff. Never choose `--restart` on a
  running miner without explicit approval.
- `trommel diagnose` and the MCP config/preset tools replace **every** preset
  wallet with `<redacted>`, including inactive presets. Do not ask for a wallet
  to “complete” a diagnostic bundle; ask the user to enter it in the app.

## CLI JSON

```bash
trommel status --json        # full Status document, contract_version = 3
trommel gpus --json          # nvidia-smi view, works without the daemon
trommel diagnose --json      # app/version, paths, redacted config, status, GPUs, warnings
trommel preset list          # human table with total fees
trommel preset show          # active preset JSON
trommel preset export <id> /tmp/preset.json
trommel preset import /tmp/preset.json
trommel miner list           # catalogue + installed versions, their presets and binary state
trommel miner verify         # check every installed binary; exits non-zero on a broken one
trommel miner verify wildrig 0.51.8 --json
trommel miner remove wildrig 0.51.7 --yes   # unused versions only; pinned versions are refused
trommel oc report --json     # snapshot, current limits and restore targets
trommel oc restore --mode snapshot|defaults
```

`miner verify --json` returns one object per install: `miner`, `version`, `path`,
`status` (`ok`, `missing`, `not_a_file`, `not_executable`, `not_readable` or
`not_recorded`),
`exists`, `is_file`, `executable`, `readable`, `recorded_sha256`,
`sha256_verified` and a localized `message`. `recorded_sha256` is the digest
stored at install time or `null`; the verifier never hashes a file to invent one.

`miner remove` refuses a version any preset pins, naming those presets, and
requires `--yes` even for an unused one. It deletes only files under the managed
store (a record pointing outside it loses the record and keeps the binary) and it
never rewrites a preset. Installing another version is the update path: the new
version is added, presets keep their pins, and only an explicit config change
moves one.

The current human-readable output is Russian. Parse JSON; do not scrape the
human text. `status` and `start/stop/toggle` connect through D-Bus and start
`trommeld` on demand; `gpus`/`diagnose` work offline.

### Status fields an agent usually needs

| Field | Meaning |
| --- | --- |
| `contract_version` | Contract revision (3); compare this, not `version` |
| `state.running` | A miner is running (including `external`) |
| `state.external` | The process was started outside Trommel and is never ours to kill |
| `state.last_error` | Last supervisor/config error, including a failed restore |
| `state.uptime_s`, `state.restarts` | Current process uptime and consecutive restart count; the budget is five |
| `miner_info` | Miner id/name/version, install/verified flags, fee breakdown |
| `gpus[]` | Per-card live values and static limits |
| `participation[]` | One entry per detected card: `uuid`, `index`, `name`, `enabled` (applied profile switch) and `mining` (part of the running session Trommel owns). The dashboard chip is this list |
| `total` | Aggregate hashrate/power/efficiency/temperature; power sums every detected card, temperature is the hottest card; `gpus_enabled` counts participating cards and `gpus_total` counts detected cards |
| `miner` | Hashrate windows, accepted/rejected/stale, `source` (`api`/`log`) |
| `pool` | Cached pool statistics when mining |
| `oc` | Snapshot availability, `applied`/`needs_restore`, stale flag, restore-on-stop, last OC error and target summary |
| `warnings` | User-facing configuration and lock warnings |

The document deliberately contains no wallet and no seed material.

## MCP server

`trommel mcp` speaks JSON-RPC 2.0 over stdio (protocol version `2024-11-05`).
It supports `initialize`, `ping`, `tools/list`, `tools/call` and `shutdown`;
notifications without an `id` are ignored.

```toml
[mcp_servers.trommel]
command = "trommel"
args = ["mcp"]            # add "--allow-write" to enable write tools
```

| Tool | Input | Returns |
| --- | --- | --- |
| `trommel_status` | — | Status JSON |
| `trommel_gpus` | — | `nvidia-smi` GPU list |
| `trommel_presets` | — | Preset summaries with fee totals |
| `trommel_preset_get` | `id` | One preset, wallet redacted |
| `trommel_config` | — | Full config, all wallets redacted |
| `trommel_config_validate` | — | `{"warnings": [...]}` for the current config |
| `trommel_config_diff` | `config` object | `{"class": "...", "reasons": [...]}` |
| `trommel_miners` | — | Catalogue entries, installed versions with `status`, `recorded_sha256` (or `null`), `sha256_verified` and `used_by` presets |
| `trommel_diagnose` | — | Redacted diagnostics bundle |
| `trommel_oc` | — | Snapshot, current limits and restore targets |
| `trommel_restore_oc` | `mode` = `snapshot`\|`defaults` | Write tool (`--allow-write`); returns a restore report |
| `trommel_start` | — | Starts mining (write-gated) |
| `trommel_stop` | — | Stops and restores stock (write-gated) |
| `trommel_apply_config` | `config`, `mode` | Applies a config (write-gated) |

Without `--allow-write`, a write tool returns a JSON-RPC error
`write tool disabled; start the MCP server with --allow-write` and the server
keeps running. `--allow-write` is a guardrail, not an authentication boundary:
MCP runs as the same user and can do anything the CLI can.

Validation rules visible to the agent: `mode` must be `live|restart|stop`;
`config` must deserialize as a complete `Config`; unknown tools return
`unknown tool: …`; malformed JSON returns a parse error. The server never
auto-applies a config from a resource or prompt.

Smoke test:

```bash
./scripts/test-mcp.sh
```

It covers the protocol, read-only defaults, write gating, bad arguments and
wallet redaction; it never executes a real write tool.

## D-Bus

For a non-MCP agent, the session bus is the richest surface:

```bash
gdbus introspect --session --dest io.github.nayvok.Trommel.Daemon \
  --object-path /io/github/nayvok/Trommel
gdbus call --session --dest io.github.nayvok.Trommel.Daemon \
  --object-path /io/github/nayvok/Trommel --method io.github.nayvok.Trommel.Status
```

Methods, properties and signals are listed in
[architecture.md](architecture.md#status-contract) and
[components.md](components.md). Always pass an explicit `ApplyConfig` mode.
`SetConfig` is a compatibility wrapper that applies live and refuses a change
that needs a restart, so a running miner is never restarted silently.
`Refresh()` is safe to poll; `Status()` returns the last snapshot.

## Things an agent must not do

- Ask for a seed phrase or private key. A wallet address is public but personal;
  it is redacted for a reason.
- Start a second miner, kill an external miner, or manually edit
  `~/.local/share/trommel/miners/`. Use Start/Stop and
  `trommel miner import/install`.
- Kill a process by miner name or by a PID read from `state.json` without
  checking `/proc/<pid>/exe` against `state.miner_path`; PIDs are reused.
- Run a benchmark while an external miner is active: the daemon refuses, and
  an agent must not work around that refusal.
- Edit a preset's wallet in an exported config and apply it without the user
  seeing the diff.
- Bypass the **Apply and restart / Apply and stop** decision for a running
  miner.
- “Fix” a failed stock restore by clearing `oc-snapshot.json`; the snapshot is
  needed for a retry.
- Treat a config file write as an applied config. The daemon owns the live
  config; use Apply.
- Add a bundled miner binary or a checksum/URL that was not actually verified.
- Remove an installed version while a preset pins it, or rewrite
  `preset.miner_version` for several presets to work around the refusal. Ask the
  user to switch the preset versions first; the GUI switch changes one preset at
  a time.
- Treat `trommel_miners.installs[].recorded_sha256` as a verified checksum when
  `sha256_verified` is `false`, or compute a digest and present it as the
  recorded one.
- Set a card's `enabled` to `false` on a miner that cannot select devices, or
  add `--gpu-list`/`--gpu-id`/`--devices` to `extra_args` to work around the
  refusal. Start and the agent must report the unsupported combination instead.

## A good agent loop

1. Read [AGENTS.md](../AGENTS.md) and this file.
2. `trommel diagnose --json`; check `warnings`, `state.last_error`, `oc.stale`.
3. Reproduce with `cargo test --workspace` and the relevant script; use the fake
   rig from [development.md](development.md#test-harness-isolation) rather than
   a real GPU.
4. Propose the smallest change, with tests and the related doc update.
5. Show `trommel config diff` or the code diff; wait for approval before
   applying anything to a rig.
6. Verify with `status --json` after an approved apply, and confirm that the
   stock restore was reported correctly. If `oc.needs_restore` is true, report
   it; do not claim the stock state came back until a restore succeeds.
