# Trommel user guide

## Install

Use the `.deb` for Ubuntu 26.04 or `./install.sh` for development. A working
NVIDIA driver and `nvidia-smi` are required. The app talks to the daemon over
D-Bus; the daemon runs as your user, the narrow GPU helper runs through polkit.

## First run

Simple mode is on by default: the preset editor hides raw miner arguments and
per-miner options. Turn it off in **Settings → Behavior → Simple mode** if you
need them.

1. Open Trommel. If the wallet or miner is missing, use **Quick setup** on the
   dashboard (it opens the active preset editor).
2. Create a preset. Choose a pool, a miner version, wallet/worker and a GPU
   profile. If the miner is not installed, import an existing binary or use
   `trommel miner install` with a direct HTTPS URL.
3. Save and apply.
4. Start. Before the miner starts, Trommel snapshots the current power limit
   and clocks for every enabled GPU.
5. Stop restores the snapshot. If restore fails, the snapshot is kept and the
   error is shown.

## Presets and installed miners

A preset references a miner **version**, not a path. Installing a version once
makes it visible as `Installed` in every preset. If a preset references a
missing version, Start is disabled until you import/download it or switch to an
installed version. Removing a miner that is used by a preset is refused.

## Fees

The miner developer fee comes from the catalogue and is fixed unless the
catalogue says it is configurable. The pool fee is used for the effective
hashrate calculation; for a custom pool it is the real fee. The total is shown
in the dashboard, preset editor, miner catalogue and extension popup.

## Miner options

The options section is built from the selected miner's descriptors. Defaults
come from the selected version; changing miner keeps compatible values, warns
about incompatible ones and offers a reset. Extra arguments are appended
verbatim as an argument vector (no shell).

## Clock locks (advanced)

Core and memory clock locks are opt-in and off in a fresh profile. Turn off
**Simple mode** to see them in **Settings → GPU defaults** and in each card's
**Tune** dialog. A lower memory clock can reduce power/heat and sometimes raise
hashrate on PearlHash-like coins, but it can hurt other algorithms; test with
the A/B benchmark. `Stop` resets `-lgc/-lmc` via the helper. Use **Similar
GPUs** or **All GPUs** in the dialog to copy one card's tuning.

## Apply while mining

If the miner is running and you change a GPU/profile setting, the Apply button
asks:

- **Apply and restart** — stop, restore stock, save, start again.
- **Apply and stop** — stop, restore stock, save the new config, leave it
  stopped.
- **Cancel** — keep the old applied config and your draft.

If a live-only change is made (for example notifications), it is applied without
a restart.

## Asking an external agent

Use **Settings → Actions → Copy diagnostics** or `trommel diagnose --json`
to create a redacted diagnostics file, then give it to Codex/ChatGPT together with the config schema.
`trommel config diff proposed.json` shows whether a change needs a restart;
`trommel config apply proposed.json --restart` applies it only after you review
the diff. For MCP-aware agents, run `trommel mcp` (read-only) or
`trommel mcp --allow-write`. See [agents.md](agents.md).

## GNOME integration

The extension is shipped with the package. Enable it in Settings or Extension
Manager; if it does not appear, log out and back in. The popup shows total
hashrate, temperature, power, shares, pool balance, active preset and miner/fee.
Settings for the panel label, live activity and Quick Settings toggle are in the
app, not in a separate extension preferences window.

## Troubleshooting

- **Service is not running** — `systemctl --user start trommeld` or just reopen
  the app; D-Bus activation should start it.
- **GPU unavailable** — check `nvidia-smi` as your user; a container/Flatpak
  sandbox may hide the device nodes.
- **OC not applied** — check the polkit helper/policy; the app still starts the
  miner with a visible warning only when no GPU tuning was required. If tuning
  was requested and failed, the start is aborted.
- **Extension installed but disabled** — `gnome-extensions info
  trommel@nayvok.github.io`; enable it in Extension Manager or the app; re-login
  on Wayland if needed.
