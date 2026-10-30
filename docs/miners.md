# Miner catalogue and options

Built-in data lives in `resources/catalog/miners.toml`,
`resources/catalog/coins.toml` and `resources/catalog/pools.toml`. No miner
binaries are bundled; the catalogue links to upstream releases.

The catalogue is parsed once by `trommel_core::catalog` (`OnceLock`) and drives
the Miners page, the preset editor, the option widgets, the install dialog and
the fee display. The argument builder is `trommel_core::miners::build_args` /
`build_args_with_devices`.

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
supported_coins = ["pearl"]
supported_algos = ["pearlhash"]
fee_pct = 0.0
fee_configurable = false
fee_flag = ""
per_gpu_stats = false
per_gpu_devices = true
device_flag = "--gpu-list"
default_options = { fan_speed = "0" }
```

| Field | Meaning |
| --- | --- |
| `id` | Stable key used by presets and installs; also the driver match |
| `name` | Display name |
| `short_description_en/ru` | One-line description shown in the catalogue/editor |
| `homepage`, `source_url`, `license` | Upstream metadata; `source_url` is the link in the install dialog, the rest is available to the status JSON and external tools |
| `supported_coins`, `supported_algos` | Catalogue capability labels; they do not hard-block a combination and are not displayed by the current GUI |
| `fee_pct` | Developer fee (percent) on the catalogue algorithm |
| `fee_configurable` | Only `true` when the miner really accepts a fee flag |
| `fee_flag` | Flag used when a fee override is applied (e.g. `--fee`) |
| `per_gpu_stats` | The miner reports per-GPU statistics; parsed, not used for device selection |
| `per_gpu_devices` | The miner can be told which cards to use. Must be paired with `device_flag` |
| `device_flag` | The documented flag that selects devices (for example `--gpu-id`); empty when the miner cannot choose |
| `default_options` | Defaults applied by quick setup when it switches miner; the editor falls back to each descriptor's `default` when a value is absent |

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
| Generic/WildRig | any id without a branch | `--algo`, `--url`, `--user`, optional `--worker`, `--api-port`, `--api-worker-id`, `--log-file`, `--no-color`, `--print-time 10`, `--gpu-temp-limit`, `--gpu-temp-resume`, optional `--gpu-list` |
| SRBMiner | `"srbminer"` | `--algorithm`, `--pool`, `--wallet wallet.worker`, `--password x`, optional `--api-enable/--api-port/--api-rig-name`, `--log-file`, optional `--gpu-id` |
| OneZero | `"onezero"` | `-a`, `-o`, `-u`, optional `-w`, optional `--devices` |

Add a branch only when the core flags genuinely differ; otherwise a catalogue
entry is enough. The generic branch is also the custom/imported-miner driver.
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
- The indices follow the `nvidia-smi` order shown on the dashboard. For an
  explicit CUDA list the supervisor sets `CUDA_DEVICE_ORDER=PCI_BUS_ID` unless
  `preset.env` overrides it, so CUDA enumeration matches `nvidia-smi`. WildRig
  uses OpenCL, which enumerates in PCI order. A heterogeneous/reordered rig is
  still verified best with a real benchmark.
- A card excluded now but tuned by an earlier restore-off session is returned
  to its snapshot before the new session starts. Snapshot, Stop, shutdown and
  emergency-stop cover only the participating, tuned cards.

## Installs and state

- `miner_installs` in the config is global and keyed `id@version`.
- Binaries live in `~/.local/share/trommel/miners/<id>/<version>/`.
- `catalog::install_status` classifies a preset as `Installed`, `OtherVersion`
  or `NotInstalled`; the editor and Miners page derive the same state from the
  shared installs.
- A missing version blocks Start. There is no uninstall flow yet; deleting a
  binary by hand leaves the install record and Start reports the missing binary.
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

`coins.toml` entries carry `id`, `symbol`, `name`, `wallet_prefix`, `algos` and
localised descriptions. `pools.toml` entries carry `id`, `name`, `url`, `fee_pct`,
`api_kind`, `min_payout`, `verified` and localised notes.

Only PearlHash has an account API adapter (`pool.rs`). A generic pool may be used
for mining; the dashboard then shows configured data without a live balance.
Adding a parser requires a stable documented API and a test fixture — do not
scrape HTML.

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
3. Add a driver branch only if the core arguments differ from the generic/WildRig
   shape.
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
