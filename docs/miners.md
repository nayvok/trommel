# Miner catalogue and options

Built-in data lives in `resources/catalog/miners.toml`,
`resources/catalog/coins.toml` and `resources/catalog/pools.toml`. No miner
binaries are bundled; the catalogue links to upstream releases.

The catalogue is parsed once by `trommel_core::catalog` (`OnceLock`) and drives
the Miners page, the preset editor, the option widgets, the install dialog and
the fee display. The argument builder is `trommel_core::miners::build_args`.

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
per_gpu_devices = false
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
| `per_gpu_stats`, `per_gpu_devices` | Parsed capability flags, currently unused; reserved for future per-device selection |
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

`build_args` has three shapes:

| Driver | Selected by | Core flags |
| --- | --- | --- |
| Generic/WildRig | any id without a branch | `--algo`, `--url`, `--user`, optional `--worker`, `--api-port`, `--api-worker-id`, `--log-file`, `--no-color`, `--print-time 10`, `--gpu-temp-limit`, `--gpu-temp-resume` |
| SRBMiner | `"srbminer"` | `--algorithm`, `--pool`, `--wallet wallet.worker`, `--password x`, optional `--api-enable/--api-port/--api-rig-name`, `--log-file` |
| OneZero | `"onezero"` | `-a`, `-o`, `-u`, optional `-w` |

Add a branch only when the core flags genuinely differ; otherwise a catalogue
entry is enough. The generic branch is also the custom/imported-miner driver.
The temperature limit and resume value come from the active GPU profile's
`default` tuning and are global, not per card.

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
2. Add a driver branch only if the core arguments differ from the generic/WildRig
   shape.
3. Add unit tests for argument rendering and fee handling in
   `crates/core/src/miners.rs`.
4. Add a version with a real HTTPS URL. Do not invent a SHA-256 or mark
   `verified = true` until a download was checked; otherwise leave `sha256 = ""`
   and `verified = false`.
5. If the miner exposes a local API, check whether the tolerant parser in
   `minerapi.rs` picks up its hashrate/shares; add a fixture if it needs a key
   mapping.
6. Run `cargo test --workspace`, `./scripts/e2e.sh` and a real benchmark before
   announcing support.
