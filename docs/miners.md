# Miner catalogue and options

Built-in data lives in `resources/catalog/miners.toml`. No binaries are bundled.

## Entry fields

- `id`, `name`, `short_description_en/ru`, `homepage`, `license`, `source_url`.
- `supported_coins`, `supported_algos`.
- `fee_pct`, `fee_configurable`, `fee_flag`. Set `fee_configurable = true` only
  when the miner really accepts a fee flag; otherwise the UI is read-only.
- `per_gpu_stats`, `per_gpu_devices` (capabilities).
- `versions[]`: `version`, `channel`, `url`, `sha256`, `archive`
  (`binary|tar.gz|zip`), `binary`, `verified`, `released_at`, `notes`.
- `default_options` and `options[]`.

## Option descriptor

```toml
[[miners.options]]
key = "fan_speed"
label_en = "Fan speed"
label_ru = "Вентилятор"
help_en = "Passed to the miner"
help_ru = "Передаётся майнеру"
kind = "int"          # int | bool | enum | string
default = "0"
min = 0
max = 100
flag = "--gpu-fan-speed"
scope = "gpu"         # global | gpu
advanced = false
```

The GUI builds rows from these descriptors; values are stored in
`preset.miner_options`. The command builder appends `flag value` for typed
values, skips `fan_speed = 0` (auto), and validates ranges. `extra_args` is
parsed with `shell-words` and appended; no shell is used.

## Install state

`miner_installs` is global (`id@version`). The app scans
`~/.local/share/trommel/miners/` and shows the same installed state in every
preset. A missing version blocks Start and offers import/download. Removing an
installed miner is refused while any preset references it.

## Adding a miner

1. Add the TOML entry and a version. Do not invent a SHA-256; leave it empty
   and `verified = false` until a real download is checked.
2. If the miner's core flags differ from WildRig/SRBMiner/OneZero, add a driver
   branch in `crates/core/src/miners.rs` and tests.
3. Add a short description for both languages and a fee note.
4. Test argument rendering, API/log parsing and a real benchmark before marking
   it verified.
