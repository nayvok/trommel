# Architecture decisions

This is the append-only decision log. Each entry records a decision that is
still relevant; the original, longer ADRs remain in git history. Add a new
entry at the end with the next number and do not rewrite an accepted decision.
A decision that replaces another gets its own entry and an explicit
“supersedes” note.

<a id="adr-0001"></a>
## ADR 0001 — Name “Trommel”

- Status: accepted.
- **Decision:** the product is named Trommel. All identifiers derive from it:
  `io.github.nayvok.Trommel`, `trommel`/`trommeld`, `~/.config/trommel`.
- **Why:** a trommel is a real mining machine that screens and washes ore. It is
  one short noun, directly related to the domain, easy to pronounce and not a
  trademark of another mining app.
- **Consequences:** the name pairs with a simple drum-and-gold icon and does not
  tie the product to Pearl.

<a id="adr-0002"></a>
## ADR 0002 — Bundle the GNOME Shell extension

- Status: superseded by [ADR 0004](#adr-0004).
- **Decision:** ship the applet in the same package, visible in Extension
  Manager, with no `prefs.js`; the first app launch offers an Enable action.
- **Why:** a Debian post-install script cannot safely enable an extension for a
  specific user.
- **Consequences:** the applet was removed in 0.6.0; this entry is kept only to
  explain the older packaging decision.

<a id="adr-0003"></a>
## ADR 0003 — Debian first, no Flatpak in v1

- Status: accepted for v1.
- **Decision:** `.deb` is the released target. RPM, AppImage and Flatpak have no
  release artifacts until a tested integration exists.
- **Why:** the product needs the host `nvidia-smi`, a root helper through
  polkit, a systemd user unit and runtime miner downloads. Flatpak and AppImage
  cannot provide that combination without a broad sandbox or a read-only mount;
  RPM needs a Fedora/openSUSE install/upgrade/removal test.
- **Consequences:** GitHub Releases is the only distribution channel; upgrades
  are new release downloads. A portable tarball plus an explicit host-helper
  installer is the preferred fallback if another format is revisited.

<a id="adr-0004"></a>
## ADR 0004 — Remove the GNOME Shell extension

- Status: accepted; supersedes [ADR 0002](#adr-0002).
- **Decision:** remove the applet, its GSettings schema, the
  **Settings → GNOME integration** group and the extension step of the package
  and `install.sh`. Desktop notifications move into the daemon through
  `org.freedesktop.Notifications`; the `Notification` D-Bus signal remains as an
  optional integration surface, gated by `behavior.notifications`.
- **Why:** the applet duplicated the status contract already rendered by the app
  and CLI, coupled the repository to one Shell version and added a second
  settings surface. Users on other desktops could not use it at all.
- **Consequences:** notifications work on any freedesktop desktop. An upgrade
  from a release that shipped the applet can leave its UUID in GNOME's
  `enabled-extensions`; the files are removed by the package manager and the
  stale entry is removed once in Extensions or with `gsettings`.
  `install.sh --no-extension` remains a deprecated no-op, and `uninstall.sh`
  still cleans up old developer installs.

<a id="adr-0005"></a>
## ADR 0005 — One active mining session, presets as the centre

- Status: accepted, 2026-10-29.
- **Decision:**
  1. At most one miner process and one coin mine at a time, on a disjoint set of
     GPUs. Only one preset is active.
  2. A preset owns coin, algorithm, pool, wallet, miner id and miner version. The
     Miners page becomes a miner library dialog opened from Presets and the
     preset editor.
  3. Miner installs stay global and versioned (`miner@version`); a preset pins
     one version. Update/remove shows the affected presets and never rewrites
     them.
  4. Per-GPU participation is the supported multi-GPU feature: a card that
     cannot mine the active coin is excluded from the process, not left mining.
  5. Multi-miner/multi-job requires a new decision; it is not a permanent
     non-goal, only out of scope until the single-session path is verified.
- **Why:** one session keeps one status contract, one snapshot, one supervisor
  and one set of alerts coherent. The Miners page duplicated the preset editor's
  miner choice.
- **Consequences:** the dashboard keeps one set of metrics; the miner library
  owns install/update/remove/verify and shows preset impact; heterogeneous cards
  are handled by exclusion, a multi-algo miner or an idle card.

<a id="adr-0006"></a>
## ADR 0006 — Failure policy and emergency restore

- Status: accepted, 2026-10-29.
- **Decision:**
  1. `trommeld` owns the session; GUI and CLI only send commands.
  2. Write the OC snapshot with `applied = true` before the first helper call, so
     a crash mid-apply always leaves a restorable record.
  3. A miner crash is not a session crash: restart with a bounded budget and
     backoff when `auto_restart` is on, otherwise stop and restore.
  4. A GUI crash is a no-op; mining continues.
  5. Loss of the control plane (SIGTERM, closed session bus, stopped unit,
     SIGKILLed daemon) runs `trommel emergency-stop`: stop only our miner,
     restore the snapshot, keep it on failure.
  6. systemd `ExecStopPost`, the guard process and startup recovery are
     independent nets; all call the same idempotent emergency path.
  7. Restore failures keep the snapshot and `last_error`; the UI never claims the
     stock state was restored.
- **Why:** `Drop` and signal handlers cannot cover SIGKILL or a dead session, and
  the GUI is not the supervisor.
- **Consequences:** OC calls are serialized with a file lock; a daemon restart
  stops the miner and restores stock instead of adopting it; a reboot safely
  invalidates the snapshot.

<a id="adr-0007"></a>
## ADR 0007 — Per-GPU participation, not just tuning

- Status: accepted, 2026-10-29; implements point 4 of
  [ADR 0005](#adr-0005).
- **Decision:**
  1. `GpuTuning.enabled` means participation. A disabled card is not passed to
     the miner, is not tuned, is returned to stock if an earlier session left
     tuning on it, and is shown as excluded.
  2. Only documented miner flags are used: WildRig `--gpu-list`, SRBMiner
     `--gpu-id`, OneZero `--devices`. `per_gpu_devices` alone is not enough.
  3. No silent fallback: if a card is excluded and the miner has no documented
     device flag, Start (and a benchmark entry) fails instead of running on every
     card.
  4. Device lists are `nvidia-smi` indices. When an explicit list is passed, the
     supervisor sets `CUDA_DEVICE_ORDER=PCI_BUS_ID` unless the preset overrides
     it.
  5. The snapshot covers only participating, tuned cards (`applied_gpus`), and
     the status carries per-card `participation`.
- **Why:** the old `enabled` flag only skipped tuning; a card switched off in the
  UI kept hashing. Guessing a device flag can silently exclude the wrong card.
- **Consequences:** excluded cards remain monitored for temperature/power/fan;
  rig power and hottest temperature still cover all detected cards; changing
  participation restarts a running miner through the normal Apply dialog;
  heterogeneous enumeration still needs a real benchmark.

<a id="adr-0008"></a>
## ADR 0008 — Desktop tray in the daemon

- Status: accepted for the basic tray.
- **Decision:** host a freedesktop StatusNotifierItem in `trommeld` with `ksni`.
  The menu has a status header, Start/Stop, Show window and Quit. Quit emits the
  additive `QuitRequested` signal, then stops our miner and restores stock. The
  symbolic icon is the default; colour is an opt-out. `TROMMEL_NO_TRAY=1`
  disables the tray for tests.
- **Why:** the tray must exist without a window, survive a GUI crash and be able
  to stop mining. A GTK-hosted tray would make the GUI part of the mining
  lifecycle.
- **Consequences:** the tray exists exactly while the daemon runs. KDE Plasma
  supports SNI natively; GNOME needs the AppIndicator extension. Without a host
  the tray is absent and the app keeps working. Show/hide, start-minimised,
  dynamic icons and submenus are deliberate follow-ups.

<a id="adr-0009"></a>
## ADR 0009 — Preset-scoped GPU settings and named GPU presets

- Status: accepted for the v4 configuration, 2026-10-30; extends
  [ADR 0005](#adr-0005) and [ADR 0007](#adr-0007).
- **Decision:**
  1. GPU settings live in the mining preset (`Preset::gpu`, a `GpuProfile` with a
     default and per-UUID entries). The shared `gpu_profiles` map and the preset's
     `gpu_profile` reference are removed; a new preset starts stock.
  2. `Config::gpu_presets` stores named per-card templates. Applying one copies
     power/clocks into the preset's card entry; `gpu_preset` is metadata only and
     is ignored by `Config::diff`.
  3. “Stock (default)” clears power/clocks, keeps participation and sets
     `custom = true` with empty values. The “Recommended 80 %” action is removed.
  4. The tuning dialog mutates only the draft on Save; Cancel/Esc is a no-op.
  5. Deleting a mining preset deletes its private GPU values; named GPU presets
     remain.
  6. Temperature limits stay global in `Behavior` because one miner process
     receives one temperature flag.
- **Why:** the v3 shared profile made switching presets leave the previous GPU
  setup in place, and Cancel could still leave an applied change. Users also
  asked for reusable per-card tuning templates.
- **Consequences:** migration copies the referenced profile into each preset and
  drops unreferenced profiles; preset export/import is self-contained; the
  dashboard can show the active preset and explain that a card switch belongs to
  it.

<a id="adr-0010"></a>
## ADR 0010 — Hashrate watchdog, notify by default

- Status: accepted for B7.2, 2026-10-31; extends [ADR 0006](#adr-0006) and
  [ADR 0007](#adr-0007).
- **Decision:**
  1. The daemon checks the last status document in the supervisor loop; it does
     not call `nvidia-smi`, start a second process or touch an external miner.
  2. The watchdog is off by default. When enabled, `watchdog_action = "notify"`
     only emits the existing notification; `"restart"` is an explicit opt-in.
  3. A restart goes through `Supervisor::stop`/`start` after the normal backoff
     and uses the existing restart budget.
  4. Detection ignores the first `min(window, 120)` seconds and requires a
     low/zero/card-down condition for `watchdog_window_s` (default 300 s); one
     healthy sample clears it.
  5. Stale or missing data produces a notification, not a restart. Without
     `expected_hashrate` the watchdog still detects 0 H/s, a disappeared card and
     stale miner data.
  6. After `watchdog_max_restarts` failures it stops, restores stock through the
     normal Stop path and notifies. It never reboots the rig or retries after the
     limit until an explicit Start or a config change.
- **Why:** a live miner can degrade without crashing, while restarting on every
  noisy hashrate dip would be worse than the problem.
- **Consequences:** the default behaviour is unchanged; an operator must tune
  `expected_hashrate`, threshold and window for the rig/coin; every action has a
  log line and the `Notification` signal (the desktop notification stays gated
  by `behavior.notifications`).
