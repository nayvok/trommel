# Miner catalogue and options

Built-in data lives in `resources/catalog/miners.toml`,
`resources/catalog/coins.toml` and `resources/catalog/pools.toml`. No miner
binaries are bundled; the catalogue links to upstream releases.

The catalogue is parsed once by `trommel_core::catalog` (`OnceLock`) and drives
the miner library dialog, the preset editor, the option widgets, the install
dialog and the fee display. The argument builder is
`trommel_core::miners::build_args` / `build_args_with_devices`.

## Miner entry

```toml
[[miners]]
id = "wildrig"
name = "WildRig Multi"
short_description_en = "…"
short_description_ru = "…"
homepage = "https://github.com/andru-kun/wildrig-multi"
license = "Freeware"
source_url = "https://github.com/andru-kun/wildrig-multi/releases"
supported_algos = ["pearlhash", "kawpow", "nexapow"]
fee_pct = 0.0
fee_configurable = false
fee_flag = ""
per_gpu_stats = true
per_gpu_devices = true
device_flag = "--gpu-list"
driver = "wildrig"
api_default_port = 4028
api_port_flag = "--api-port"
api_worker_flag = "--api-worker-id"
default_options = { fan_speed = "0" }

[[miners.fees]]
algo = "pearlhash"
pct = 0.0
```

| Field | Meaning |
| --- | --- |
| `id` | Stable key used by presets and installs |
| `name` | Display name |
| `short_description_en/ru` | One-line description shown in the catalogue/editor |
| `homepage`, `source_url`, `license` | Upstream metadata; `source_url` is the link in the install dialog, the rest is available to the status JSON and external tools |
| `supported_coins` | Optional explicit coin whitelist. Empty means “algorithm-compatible”: the UI filters coins by `supported_algos` |
| `supported_algos` | Algorithms the miner advertises; the GUI uses this (or an explicit probe) to filter coins |
| `fee_pct` | Fallback developer fee when `fees` has no entry for the selected algorithm |
| `fees` | Per-algorithm `{ algo, pct, configurable, flag }` table |
| `fee_configurable` | Only `true` when the miner really accepts a fee flag |
| `fee_flag` | Flag used when a fee override is applied (e.g. `--fee`) |
| `per_gpu_stats` | The miner documents per-GPU statistics. WildRig, SRBMiner and OneZero are parsed; `false` means the dashboard shows `—` per card and never splits the total |
| `per_gpu_devices` | The miner can be told which cards to use. Must be paired with `device_flag` |
| `device_flag` | The documented flag that selects devices (for example `--gpu-id`); empty when the miner cannot choose |
| `driver` | Core argument shape and API parser: `wildrig`, `generic` (custom builds), `srbminer`, `onezero` |
| `api_default_port`, `api_port_flag`, `api_enable_flag`, `api_worker_flag` | Documented local API shape; no Rust branch needed |
| `algorithm_probe_arg` | Optional `--list-algorithms` / `--help` used by **Check capabilities** |
| `default_options` | Defaults the editor and new preset start from; each descriptor's `default` is the fallback |

## Version entry

```toml
[[miners.versions]]
version = "0.51.8"
channel = "stable"                 # stable | older | …
url = "https://…/wildrig-multi-linux-0.51.8.tar.gz"
sha256 = "…"                       # empty = unverified
archive = "tar.gz"                 # binary | tar.gz | tgz | zip
binary = "wildrig-multi"
verified = true
released_at = "2026-10-24"
notes = "Official Linux tarball; 0% fee on PearlHash."
```

- A version URL must be HTTPS. Some catalogue entries intentionally point at the
  upstream releases page with `sha256 = ""` and `verified = false`; the user
  then supplies a direct URL in the install dialog.
- `archive` selects extraction; `binary` is the file name to find and install.
- `verified = true` means a real download was checked against `sha256`. Do not
  set it by hand otherwise.
- The GUI lists installed versions from `miner_installs`, not only from the
  catalogue, so a user can install a version newer than the catalogue.

## Option descriptor

```toml
[[miners.options]]
key = "fan_speed"
label_en = "Fan speed"
label_ru = "Вентилятор"
help_en = "Passed to the miner; N/A on cards without fan control."
help_ru = "Передаётся майнеру; не работает на картах без управления."
kind = "int"          # int | bool | enum | string
default = "0"
min = 0
max = 100
values = []           # enum choices
flag = "--gpu-fan-speed"
scope = "gpu"         # global | gpu
advanced = false
```

Rendering rules in `append_options`:

- `bool` — emit `flag` only when true;
- `int` — emit `flag value` when the value parses and is within `min..=max`;
  `fan_speed = 0` is treated as “auto” and skipped;
- `enum`/`string` — emit `flag text` when the text is non-empty;
- a configurable fee adds `fee_flag value` only when `miner_fee_override_pct` is
  set; a fixed fee is never passed to the miner;
- `extra_args` is parsed with `shell-words` and appended as separate arguments.

The GUI builds widgets from `kind` (`AdwSpinRow`, `AdwSwitchRow`, `AdwEntryRow`,
`AdwComboRow`) and stores values in `preset.miner_options`. `advanced` and
`scope` are parsed for future per-row filtering; the current UI hides the whole
options group in simple mode. Switching the miner rebuilds the rows, and keys
the new miner does not define are dropped when the preset is saved.

## Driver branches

`build_args_with_devices` has three core shapes; the optional device flag (see
below) is added for every branch:

| Driver | Selected by | Core flags |
| --- | --- | --- |
| WildRig / generic | `driver = "wildrig"` or `"generic"` | `--algo`, `--url`, `--user`, optional `--worker`, `--log-file`, `--no-color`, `--print-time 10`, `--gpu-temp-limit`, `--gpu-temp-resume`, optional `--gpu-list` |
| SRBMiner | `driver = "srbminer"` | `--algorithm`, `--pool`, `--wallet wallet.worker`, `--password x`, `--log-file`, optional `--gpu-id` |
| OneZero | `driver = "onezero"` | `-a`, `-o`, `-u`, optional `-w`, optional `--devices` |

API flags are appended from the `api_*` catalogue fields for every driver.
`preset.api_port = 0` means “use `api_default_port`”; an explicit port overrides
it. A custom/imported miner with no catalogue entry keeps the generic
`--api-port` shape when the preset sets a port.

Add a branch only when the core flags genuinely differ; otherwise a catalogue
entry is enough. The generic branch is also the custom/imported-miner driver.
Multi-GPU support is inherent in all three miners and is expressed by
`per_gpu_devices` + `device_flag`; one process covers every participating card.
The temperature limit and resume value come from the active GPU profile's
`default` tuning and are global, not per card.

## Device selection (per-GPU participation)

A card with `GpuTuning.enabled = false` is excluded from the active mining
session: it is not passed to the miner and it is not tuned. The device list is
rendered by `build_args_with_devices`; `device_plan` decides whether an explicit
list is needed and whether the selected miner can honour it.

Only a flag documented upstream is acceptable. The built-in catalogue currently
ships:

| Miner | `device_flag` | Documented meaning |
| --- | --- | --- |
| WildRig Multi | `--gpu-list` | list of GPUs to use; `--print-devices` lists them |
| SRBMiner-MULTI | `--gpu-id` | GPU ids printed by `--list-devices`, comma-separated |
| OneZeroMiner | `--devices` | comma-separated list of devices to use |

Rules:

- The list is comma-separated and contains the `nvidia-smi` index of every
  participating card, in query order (`0`, `1,2`).
- When every detected card participates, no device flag is passed: the miner's
  default (“all devices”) is already correct and existing command lines do not
  change.
- When at least one card is excluded and the miner has no documented flag,
  Start fails with an explicit error. Trommel never silently runs the miner on
  the excluded card. The same rule applies to each benchmark entry.
- The device list is only emitted for a catalogue miner with both
  `per_gpu_devices = true` and a non-empty `device_flag`. A custom/imported miner
  id has no capability and therefore cannot take part in a partial rig.
- A device flag in the preset's `extra_args` is rejected when Trommel supplies
  the list: two contradictory selections are not a supported configuration.
- The indices follow the `nvidia-smi` order shown on the dashboard. When an
  explicit list is passed, the supervisor sets `CUDA_DEVICE_ORDER=PCI_BUS_ID`
  for the miner process unless `preset.env` overrides it, so CUDA enumeration
  matches the `nvidia-smi` indices. The variable is inert for OpenCL miners such
  as WildRig, whose device order is not guaranteed to match PCI order; a
  heterogeneous or reordered rig is verified best with a real benchmark. When
  every card participates no list is passed and the variable is not set.
- A card excluded now but tuned by an earlier restore-off session is returned
  to its snapshot before the new session starts. Snapshot, Stop, shutdown and
  emergency-stop cover only the participating, tuned cards.

## Installs and state

- `miner_installs` in the config is global and keyed `id@version`.
- Binaries live in `~/.local/share/trommel/miners/<id>/<version>/`.
- `catalog::install_status` classifies a preset as `Installed`, `OtherVersion`
  or `NotInstalled`; the editor, the preset list and the miner library derive the
  same state from the shared installs.
- A missing version blocks Start. A record whose binary disappeared is still
  listed with the `missing` status and can be removed, but it cannot be pinned.
- The miner library lists one row per installed version with the presets that
  pin it, its binary status and **Verify**/**Remove** actions. Removal is refused
  while any preset pins the version and asks for confirmation otherwise; it
  deletes only the managed `<id>/<version>/` directory and never edits a preset.
  Installing another version leaves every pin alone; the affected presets are
  then offered as an explicit, default-off selection.
- `versions::pin_preset` is the only rollback path: it moves one preset to an
  installed, executable version and reports the change; other presets keep
  their pins.
- `trommel miner verify` re-checks every recorded binary and exits non-zero on a
  broken one; `trommel miner remove <id> <version> --yes` mirrors the library
  rules (never a pinned version). The read-only MCP `trommel_miners` tool and
  `trommel diagnose --json` expose the same state.
- Downloads and imports update the running daemon immediately (the CLI applies
  the new config after saving).
- `miner`, `version` and the archive `binary` name are validated as one path
  component each before any network request or write: non-empty, not `.`/`..`,
  and no `/`, `\` or NUL. A version such as `1.0.0-rc1` is valid; `../escape`
  and `a/b` are rejected.
- A local import is copied into the managed store, so the preset keeps working
  after the original download is moved. The installed file may have any name —
  `my-custom-rig`, not just `wildrig-multi` — and the supervisor records its
  absolute path as `state.miner_path`.
- Stop, shutdown and `trommel emergency-stop` signal the miner only when
  `/proc/<pid>/exe` (or the script argument in `cmdline`) still matches that
  saved path. Custom binaries are therefore stopped correctly without a
  name-based guess.

## Coins and pools

`coins.toml` entries carry `id`, `symbol`, `name`, `wallet_prefix`, `algos`,
`hashrate_unit`, `verified` and localised descriptions. The GUI filters coins by
the selected miner's `supported_coins` whitelist or, when it is empty, by
`coin.algos` ∩ the miner's `supported_algos`. A detected capability probe can
widen or correct the catalogue set; a custom coin remains available for manual
metadata.

`pools.toml` is a known-pool hint directory: `id`, `name`, `url`, `fee_pct`,
`api_kind`, an optional `api_url` template with `{wallet}`, `min_payout`,
`verified` and localised notes. The GUI has **no pool dropdown**: the preset's
`pool_url` is authoritative. A matching host can suggest a fee and supply the
documented account API; any other pool can set `pool_api_url` explicitly.

Only documented JSON APIs are used (`pool.rs`). There is no HTML scraping: a pool
without an API shows configured data and a clear “balance unavailable” note.
Adding a parser requires a stable documented API and a test fixture.

## Fees

The display is always `miner + pool = total`:

```
miner 0.00% + pool 1.00% = total 1.00%
```

`Preset::miner_fee_pct` is the catalogue fee cached in the preset;
`miner_fee_override_pct` is used only when the catalogue sets
`fee_configurable = true`. `Status` reports both plus `fee_source`
(`catalog`/`override`). The effective hashrate in a benchmark subtracts the
total.

## Adding a miner

1. Add a `[[miners]]` entry to `miners.toml`: description in both languages,
   homepage, license, coins/algos, fee, versions and option descriptors.
2. Add `per_gpu_devices = true` and `device_flag = "--…"` only when the upstream
   documentation names a device-selection flag. Never guess one.
3. Set `driver` and the `api_*`/`algorithm_probe_arg` fields from documented
   upstream behavior. Add a new driver branch only if the core arguments differ
   from the existing `generic`/`srbminer`/`onezero` shapes.
4. Add per-algorithm `fees`; do not apply one algorithm's fee to another.
4. Add unit tests for argument rendering, fee handling and (when applicable)
   the device flag in `crates/core/src/miners.rs`.
5. Add a version with a real HTTPS URL. Do not invent a SHA-256 or mark
   `verified = true` until a download was checked; otherwise leave `sha256 = ""`
   and `verified = false`.
6. If the miner exposes a local API, check whether the tolerant parser in
   `minerapi.rs` picks up its hashrate/shares; add a fixture if it needs a key
   mapping.
7. Run `cargo test --workspace`, `./scripts/e2e.sh` and a real benchmark before
   announcing support.
