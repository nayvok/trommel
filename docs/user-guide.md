# Trommel user guide

Trommel controls a local NVIDIA rig: it stores presets, installs miners, applies
power/clocks, starts exactly one miner and restores the stock GPU state when
mining stops. The daemon keeps working after the window is closed.

Russian: [user-guide.ru.md](user-guide.ru.md).

## Install

Download the latest `.deb` from GitHub Releases
(<https://github.com/nayvok/trommel/releases/latest>) and install it with
`sudo apt install ./trommel_amd64.deb`, or run the release
`trommel-install.sh`, which verifies the SHA-256 first. There is no apt
repository or PPA; upgrades are new release downloads.

`./install.sh` is the developer path. A working NVIDIA driver and `nvidia-smi`
are required. The app talks to the daemon over D-Bus; the daemon runs as your
user, and the narrow GPU helper runs through polkit — without a password for the
user who installed the package. The background service and tray start at login
by default; turn off **Settings → Behavior → Start Trommel at login** if you
prefer to start it on demand.

## First run: create a preset

Simple mode is on by default: the preset editor hides raw miner arguments and
per-miner options. Turn it off in **Settings → Behavior → Simple mode** if you
need them.

A fresh install has **no preset**. The dashboard is the first page: it shows the
detected cards, and the hero says that a preset is required. Press **Create
preset** (or **Start**, which explains the same thing) to open the preset editor.

A preset stores:

1. **Mining** — name, coin, algorithm, wallet and worker. The coin list is
   filtered by the selected miner's advertised algorithms; a coin the miner does
   not advertise can be added as a **Custom** coin with its symbol, name,
   algorithm, wallet prefix and hashrate unit.
2. **Miner** — miner and version. **Install…** opens the shared install dialog:
   download a catalogue version, type a version newer than the catalogue, or
   import a local binary. One installation serves every preset; only one miner
   runs at a time.
3. **Pool** — the pool address is entered by hand (`host:port`); there is no
   pool dropdown. The pool fee is used for the effective-hashrate calculation.
   When the host matches a known pool, Trommel offers its suggested fee and uses
   its documented JSON account API; otherwise an optional **Pool API URL** can
   point at a documented JSON endpoint. Without one, the dashboard says that
   balance data is unavailable instead of inventing it.
4. **GPU settings** — each mining preset owns its own per-card participation,
   power limit and clock locks. Select **Tune** on a dashboard card to edit
   them. A new preset starts from stock; the last values are stored with the
   preset, so switching presets restores the matching GPU setup.

Save the preset, then press **Start**. Before the miner starts, Trommel
snapshots each enabled card's power limit and clocks; Stop restores the snapshot.
If restore fails, the snapshot is kept and the error is shown.

## Dashboard

- The hero card shows total hashrate, the state pill (`mining`, `external`, `stopped`,
  `offline`), and the coin, pool and fee breakdown. The preset name lives once,
  in the **Active preset** row.
- Every GPU is a card with its own hashrate anchor (the miner's per-card value
  and a five-minute sparkline, or `—` when the miner does not report it), live
  power, temperature, core/memory clocks, fan and utilisation in value bubbles,
  plus a switch that includes the card in the mining session. A single-GPU rig uses one full-width card; with two or more
  cards each keeps the compact tile. Only the power and temperature bubbles
  have a sparkline of the last five minutes (hover it for the observed minimum
  and maximum); core, memory, fan and load are values, not trends. Click
  **Tune** to edit its power limit and clock locks. A card chip says **mining**
  or **excluded**; the Active preset row shows the miner/version, card count,
  stock/tuned state and the rig power and temperature.
- The hero metadata line compares the expected hashrate from the preset with
  the actual value and shows efficiency in the coin's unit (`J/TH` for Pearl,
  `J/MH` for Ravencoin/Zilliqa). Efficiency uses only the participating cards;
  the rig power in the Active preset row still covers every detected card.
- The **Pool and shares** group shows the pool name, endpoint, fee and API
  status, accepted/rejected/stale shares with the acceptance percentage, and the
  pending/paid/immature balance. Without a pool API, or before the first answer,
  the balance is **—** with an explanation in the tooltip; it is never shown as
  `0.000`. The group is hidden when no preset exists.
- A running miner that has not reported yet shows **—**, not `0`; a real zero
  reported by the miner or pool is still shown as `0`. When stopped, the pool
  row still names the configured endpoint.
- **Start** in the dashboard hero card applies your edited draft before
  starting. Without a preset it explains how to create one; with a preset it
  names a missing wallet, pool or miner version and offers **Open preset**
  instead of failing with a toast. It is disabled while every detected GPU is
  excluded from mining; the warning row explains why. While mining, the same
  button reads **Stop**.
- Action errors show a **Details** button. The dialog keeps the full message
  selectable and offers **Copy**, so a long daemon error does not disappear in a
  truncated toast.

### Excluding a GPU

Turn a card's switch off to exclude it from the miner. An excluded card is
**not** passed to the miner, gets no power or clock tuning, and is returned to
its stock settings if an earlier restore-off session left tuning on it. The card
stays visible with its live temperature, power and fan readings and is marked
**excluded**; the Active preset row shows how many of the detected cards are
mining.

The exclusion is handed to the miner through a flag documented by that miner:

| Miner | Flag |
| --- | --- |
| WildRig Multi | `--gpu-list` |
| SRBMiner-MULTI | `--gpu-id` |
| OneZeroMiner | `--devices` |

If the selected miner has no documented device-selection flag, Start refuses
when a card is excluded instead of quietly mining on every card. Enable all
cards or switch to a miner that supports device selection. A custom/imported
miner id has no catalogue capability, so it can only run with every card
enabled.

The device numbers are the `nvidia-smi` indices shown as `GPU #N` on the cards.
On a rig whose miner enumerates devices in a different order, verify the result
with a benchmark. Excluding or re-including a card while mining uses the normal
Apply and restart / Apply and stop dialog.

## Presets and installed miners

A preset bundles coin/algorithm (Pearl/pearlhash in the current catalogue),
pool, wallet, worker, miner version and miner options. GPU settings are edited
on each card's **Tune** dialog, not in the preset editor. A preset references a miner **version**,
not a path. Installing a version once makes every preset that references it see
the version as installed. If a preset references a missing version, its row says
so, Start is blocked, and **Install…** opens the miner library focused on that
preset. Installed versions are managed in the library: you can verify, update
and remove them there, and move one preset to an older version. Nothing changes
a preset silently — see [Miner versions](#miner-versions).

![Presets](images/presets-en-dark.png)

The **Miner library** row on the Presets page opens the library dialog. It lists
the catalogue miners, every installed version with the presets that pin it, a
per-preset version switch, and the A/B benchmark:

![Miner library](images/miner-library-en-dark.png)

Presets open in a modal editor with a summary at the top, then **Coin and
pool**, **Wallet**, **Miner**, **GPU** and **Advanced** groups, with
**Cancel**/**Save** in the footer and **Duplicate**/**Delete…** at the end of
the page. The algorithm of a known coin is read-only (it is set automatically);
a custom coin keeps it editable. **Advanced** holds the miner options, pool API
URL, API port, fee override and extra arguments, and Simple mode hides it.
Saving applies immediately: a stopped miner is saved at once; a running miner
gets the restart/stop dialog. Deleting asks for confirmation, names the preset,
and explains that the daemon keeps mining with the previously applied preset
until another one is applied.

![Preset editor](images/preset-editor-en-dark.png)

## Miner installation

The install dialog is the only place that installs miners, and it is shared by
the miner library and the preset editor:

![Miner installation](images/miner-install-en-dark.png)

- pick a catalogue version or type your own version name;
- press **Install** to download it over HTTPS (open **Download source** to
  change the URL, add a SHA-256, or pick `tar.gz`, `zip` or a raw binary);
- or import a local executable.

Imported builds are copied into Trommel's managed store, so moving or deleting
the original download does not break the preset. Downloads are size-limited and
archive paths are validated. An install without a SHA-256 is recorded as
unverified; supply one whenever the release publishes it.

The catalogue is a convenience, not a gate: if a miner released a new build
before the catalogue was updated, type the version and paste the upstream URL.

### Miner versions

Installed builds are global and several versions of one miner can stay
installed. The library shows one row per version under its miner:

- the row names the presets that pin that version and their versions;
- it reports whether the binary still exists and is executable, and whether a
  SHA-256 was recorded — Trommel never invents a checksum;
- **Verify** (the check icon) re-checks the binary, reports the result in a
  toast and marks the row **verified** while the library stays open. **Remove**
  (the trash icon) is the explicit uninstall.

**Update** is installing another version with the shared install dialog. The
build is added next to the existing ones and **no preset is rewritten**. After
the install, Trommel lists the presets that pin a different version and asks
which of them to move; nothing is selected and **Keep preset versions** is the
default, so presets stay where they were unless you choose otherwise.

**Remove** never happens silently:

- while any preset pins the version, the action is refused with the list of
  those presets. Switch them to another installed version first;
- an unused version asks for confirmation and explains that the managed binary
  is deleted (or only the install record, when the binary lives outside the
  managed store), every preset stays as it is and other versions are kept.

**Rollback** is the version switch in the **Preset versions** group: pick a
preset, choose any other installed version and confirm. Only that preset moves;
all other presets keep their pins. If the edited preset is the one the running
miner uses, the change goes through the usual **Apply and restart / Apply and
stop** dialog. A build whose binary is missing is listed but cannot be
selected.

## Settings

![Settings](images/settings-en-dark.png)

- **Behavior** — restart on crash, restore stock on stop (on by default),
  notifications, start Trommel at login (on by default; service and tray, not
  mining), monochrome tray icon (on by default), temperature limit and resume
  temperature (global: they drive the overheat warning and are passed to miners
  that support a limit), start mining at login, simple mode, poll interval,
  language (needs an app restart) and appearance.
- **Reliability** — the optional hashrate watchdog. It is off by default.
  When enabled it only notifies; **Restart the miner** is a separate explicit
  choice. Set the minimum percentage of the preset's expected hashrate, the
  window in minutes and, for restart, the limit. A false alarm can come from a
  thermal throttle, a driver glitch or a stalled miner API; the watchdog never
  reboots the rig and never touches an external miner.
- **GPU** — global temperature limit/resume and **GPU presets**, the single
  place to rename or delete saved per-card tuning templates.
- **Actions** — **Save and apply**, **Copy diagnostics** (redacted), **Restore
  stock**.

The per-card **Tune** dialog on the dashboard is the only place that edits a
single card's override; Settings no longer repeats the card list.

## Clock locks (advanced)

Core and memory clock locks are opt-in and off in a fresh profile. Turn off
**Simple mode** to see them in each card's **Tune** dialog.

![Settings actions](images/settings-advanced-en-dark.png)

The tuning dialog starts with a **Now** row so you can compare the target with
the live state. A clock value row is visible only while its lock switch is on.
The footer has **Cancel**, a **GPU presets** menu and **Save**. The menu offers
**Stock (default)** (driver-auto power and clocks), saved per-card GPU presets
and **Save as GPU preset…**. Renaming or deleting saved templates lives in
**Settings → GPU → Manage…**, not in the Tune menu. Selecting a preset only
changes the dialog fields; nothing reaches the daemon until **Save**. **Cancel**
and closing the dialog discard every local change and never open the Apply
alert. On a multi-GPU machine **Apply to** copies the values to similar cards or
all cards; copied cards become custom rather than claiming the other card's GPU
preset.

![GPU tuning dialog](images/gpu-tune-en-dark.png)

A lower memory clock can reduce power/heat and sometimes raise hashrate on
PearlHash-like coins, but it can hurt other algorithms; test with the A/B
benchmark. Stop resets the locks to the driver default.

## Fees

The miner developer fee comes from the catalogue and is fixed unless the
catalogue marks it configurable. The pool fee is used for the effective hashrate
calculation. The total is shown in the dashboard and the preset editor:

```
miner 0.00% + pool 1.00% = total 1.00%
```

The UI never pretends a fixed fee can be changed.

## Miner options

The options section is built from the selected miner's descriptors: types,
ranges, defaults and CLI flags come from the catalogue for the selected miner.
Changing the miner rebuilds the rows; values whose keys the new miner does not
define are dropped when the preset is saved. Extra arguments are appended
verbatim as an argument vector (no shell). In simple mode both sections are
hidden.

## Apply while mining

If the miner is running and you change a GPU/profile setting, **Apply** asks:

- **Apply and restart** — stop, restore stock, save, start again.
- **Apply and stop** — stop, restore stock, save the new config, stay stopped.
- **Cancel** — keep the old applied config and your draft; optimistic switch
  changes are undone.

A live-only change (language, notifications, autostart) is applied without a
restart. A failed start saves the previous config back and attempts a
best-effort stock restore.

## Stock restore and limitations

- **Restore stock** (Settings → Actions) asks first: it can restore the snapshot
  captured before mining (`350 W`, clocks unlocked) or reset to the driver
  default when the snapshot itself already had a reduced limit. The dialog shows
  the exact target and warns when the two differ.
- The power limit is restored from the snapshot captured before mining and read
  back; a mismatch is an error, not a silent “applied”. The snapshot is kept
  when the restore fails, so you can retry.
- `-lgc`/`-lmc` are reset to the driver default. A pre-existing manual clock
  lock cannot be read back through `nvidia-smi`; the UI says so, and a warning
  appears when the driver does not honour a configured lock.
- Memory/core locks are checked once the miner has loaded the GPU. If the
  driver ignores the memory lock, the miner is stopped and the error names the
  requested/actual frequency; remove the lock from the profile or update the
  driver.
- **Restore stock on stop** can be turned off. In that case Stop keeps the
  snapshot and leaves the power/clocks applied; the dashboard shows a banner
  with **Restore stock** and the status warning explains why. Turning it off is
  never silent.
- The daemon owns the session, not the window: closing or crashing the GUI never
  stops mining. If the daemon itself is killed, the systemd unit or the guard
  runs an emergency stop, restores the snapshot and does **not** resume mining;
  the next start begins stopped. If the restore fails, the banner and the last
  error stay until a retry succeeds.
- A profile with no power/clock values touches nothing: Start needs no snapshot,
  no helper and no password, so a pure stock profile can keep restore-on-stop
  enabled.
- Per-card settings inherit the preset's GPU defaults (driver-auto power, no
  clock locks) unless the card has a custom override. The participation switch
  does not create an override; **Tune → Save** does. Each dashboard card shows
  **stock**, **tuned** or **GPU preset**.
- Fan control on consumer cards is not available through `nvidia-smi`; fan flags
  are passed to the miner when supported and disappear with the process.
- On reboot the snapshot is marked stale and is not used; Start captures a fresh
  one, and the driver resets GPU settings anyway.
- Trommel does not kill a miner it did not start. Stop that process yourself
  before pressing Start.

## Notifications

**Behavior → Notifications** turns crash, error and overheat events into regular
desktop notifications. The daemon sends them through
`org.freedesktop.Notifications`, so they work on any freedesktop desktop and do
not need a shell extension. The same events are also emitted as the
`Notification` D-Bus signal for custom integrations.

## Tray

The background service `trommeld` owns the tray icon, so it is available even
when the main window is closed. The menu is rendered by the panel and contains a
status line (`Mining stopped` or `Mining · <preset> · <hashrate>`), a separator,
**Start mining** / **Stop mining**, **Show window** and **Quit**.

- **Start/Stop** uses the same supervisor and busy guard as the dashboard; it
  never starts a second miner.
- **Show window** launches Trommel or brings the existing window to the front.
- **Quit** stops only Trommel's miner, restores the captured power limit and
  clocks, asks an open window to close and shuts the daemon down. After Quit no
  Trommel process is left; start the app again to get the tray back.

KDE Plasma supports the tray natively. GNOME Shell needs the AppIndicator
extension — Ubuntu ships it as `ubuntu-appindicators`; on other GNOME installs,
enable “AppIndicator and KStatusNotifierItem Support”. Without a
StatusNotifierItem host the icon is simply absent and every other feature keeps
working. Windows is not supported.

The related switches live in **Settings → Behavior**:

- **Use monochrome tray icon** (on by default) draws the same drum in a single
  colour; turn it off for the colour icon. The change applies on the next status
  refresh.
- **Start Trommel at login** (on by default) starts the background service and
  tray at login. It does **not** start mining; **Start mining at login** is a
  separate switch.

## Benchmarks

**Presets → Miner library → A/B benchmark** (or `trommel bench --minutes 3`)
stops mining, runs each selected miner sequentially, records raw and fee-adjusted
effective hashrate, restarts the previous miner if it was running, and prints the
winner. Benchmarking moves a real rig: the miner stops for the duration of the
test.

## Asking an external agent

Use **Settings → Actions → Copy diagnostics** or `trommel diagnose --json` to
create a redacted bundle, then give it to Codex/ChatGPT together with the config
schema. `trommel config diff proposed.json` shows whether a change needs a
restart; `trommel config apply proposed.json --restart` applies it after you
review the diff. For MCP-aware agents, run `trommel mcp` (read-only) or
`trommel mcp --allow-write`. See [agents.md](agents.md).

## Troubleshooting

- **Service is not running** — `systemctl --user start trommeld` or just reopen
  the app; D-Bus activation should start it. Check
  `journalctl --user -b -u trommeld`.
- **GPU unavailable** — check `nvidia-smi` as your user; a container or Flatpak
  sandbox may hide the device nodes.
- **OC not applied** — check the polkit helper/policy. The app aborts the start
  when tuning was requested and failed, instead of mining with unknown clocks.
  Run `./scripts/check-polkit.sh` and reinstall the rule.
- **Start says the miner is missing** — install the referenced version or switch
  the preset to an installed one; installs are global, not per preset.
- **Start refuses because the miner cannot choose GPUs** — a card is excluded
  but the selected miner has no documented device flag. Enable every card or
  choose a miner that supports device selection; Trommel does not fall back to
  running on all cards.
- **No pool balance** — the pool API is cached for a minute and backs off after
  failures; only PearlHash has an account API adapter.
