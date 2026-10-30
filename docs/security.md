# Security model

Trommel runs a user service that applies GPU settings through a narrow root
helper and downloads third-party miner binaries. This document describes the
trust boundaries around that. Report vulnerabilities privately; see
[SECURITY.md](../SECURITY.md).

## Trust boundaries

| Component | Trust | Can do | Cannot do |
| --- | --- | --- | --- |
| `trommel` GUI/CLI | User session | Read/write its config draft, call D-Bus, download to the user store | Run root helpers directly, tune GPUs, spawn the supervisor's miner |
| `trommeld` | User session | Own the miner, call the helper through pkexec, read `nvidia-smi`, scan `/proc` | Run as root, write outside the user's dirs, grant polkit rights, kill an external miner |
| `trommel guard` | User session | Watch the daemon PID/start time and run emergency-stop after a crash | Touch GPUs directly, run as root |
| `trommel-oc` | Root, invoked via pkexec | One `nvidia-smi` call per GPU with numeric arguments | Accept paths, shell text, arbitrary commands, other GPUs' UUID formats |
| Miner binary | User, third-party | Mine, bind a local API port, write its log | Be inspected or verified by Trommel beyond the executable/checksum checks |
| Pool API | Network | Return statistics to the daemon | Control the miner, trigger a config change, see the wallet in logs (the daemon does not log it) |

The D-Bus service is a **same-user** interface: any process running as the user
can call `GetConfig` and read the wallet. This is normal for a desktop service;
do not expose the bus name on the system bus or through a socket.

## The root helper

`/usr/libexec/trommel/trommel-oc` is the only privileged program.

- Accepts `apply --gpu GPU-… [--pl W] [--lgc MHz] [--lmc MHz]` and
  `reset --gpu GPU-… [--pl W]`, plus a read-only `status`. An optional
  `--lease-pid PID --lease-start START` pair names the daemon that queued the
  change.
- Validates the UUID shape (`^GPU-[0-9a-fA-F-]{8,64}$`) and ranges (power
  50–1000 W, clocks 100–4000 MHz) in both the Bash helper and
  `trommel-core::oc` before invoking it.
- Calls `nvidia-smi` directly with an argument vector; no paths, no shell
  interpolation, no environment-controlled commands.
- Waits for the power limit to read back and **fails** when the driver reports a
  different value; `reset` reports a failing `-rgc`/`-rmc` instead of swallowing
  it. Clock locks that a driver accepts but ignores are caught under load by the
  supervisor, which stops the miner and restores the snapshot.
- Has a 180 s caller-side timeout (`TROMMEL_OC_TIMEOUT_MS` in tests) so an
  interactive polkit password dialog is not killed while the user types. A
  missing rule costs one authentication, not a silent failure.
- Is called one GPU at a time; the daemon rejects a second concurrent OC
  operation while one is waiting for authorization.
- Validates the lease as strict digits and checks `/proc/<pid>/stat` field 22
  **before its first mutation**. A helper that sat behind a polkit dialog while
  the daemon died aborts instead of tuning a rig that is being restored.
- Is serialized with every other apply/restore by an advisory `flock` on
  `~/.local/state/trommel/oc.lock`, so a late helper cannot overwrite an
  emergency restore. Nested acquisition in one process is an error, not a
  deadlock.

## Polkit

- The action `io.github.nayvok.trommel.oc` is annotated with the helper path and
  `allow_gui=true`.
- The package `postinst` writes `/etc/polkit-1/rules.d/50-trommel.rules` for the
  installing `SUDO_USER`, so the per-user service can tune GPUs without a
  password prompt. The generic `/usr/share/polkit-1/rules.d` rule is only a local
  fallback.
- The developer installer writes the same per-user rule through
  `scripts/install-polkit-rule.sh`.
- Remove the rule to require authentication on every OC call. Removing the
  helper disables tuning; mining still starts, but the start is aborted when
  tuning was requested (Trommel does not mine with silently unapplied clocks).

## Miner downloads

- HTTPS only; the download URL must start with `https://`.
- 512 MiB limit, with connect/read/total timeouts.
- Optional SHA-256; if absent, the install is stored as `verified: false` and
  the UI says so. Never invent a checksum.
- `miner`, `version` and `binary` must each be a single safe path component
  (non-empty, not `.`/`..`, no `/`, `\` or NUL) before any network request or
  write. This blocks `../escape` and `a/b` before the archive extractor.
- Archives (`tar.gz`, `tgz`, `zip`, raw) reject absolute paths and `..`;
  extraction is staged in a temporary directory and moved into
  `~/.local/share/trommel/miners/<id>/<version>/`.
- A file must look like an ELF binary or a `#!` script before it is accepted.
- Imported local binaries are copied into the managed store; `verified` stays
  false.
- Trommel does not execute a miner as root and does not set setuid bits.

## Wallets, logs and diagnostics

- A wallet is public but personal. The status document and the daemon log do not
  include it.
- `Config::redacted()` replaces every preset wallet with `<redacted>`; it is used
  by `trommel diagnose --json`, `Copy diagnostics` and the MCP `trommel_config`,
  `trommel_preset_get` and `trommel_diagnose` tools.
- The config file contains the wallet in clear text (mode determined by the
  user's umask). Do not commit it or paste it into an issue; use diagnostics.
- Test fixtures may only use `prl1test…`; `scripts/check-no-secrets.sh` enforces
  this for tracked files and also rejects personal home paths (`/home/user` is
  the only allowed placeholder).
- Release packages are built with `--remap-path-prefix`, so panic locations and
  debug strings do not embed the builder's checkout or Cargo home.
  `scripts/test-deb.sh` rejects a package whose binaries still contain
  `/home/`.
- Never ask for a seed phrase or private key. Trommel has no wallet and cannot
  use one.

## Network behaviour

- The daemon calls the PearlHash account API
  (`https://pearlhash.xyz/api/account/<wallet>`) for the configured wallet,
  cached for 60 s with backoff; no other pool API is queried.
- The miner is expected to expose a local HTTP API on `127.0.0.1:<api_port>`;
  Trommel does not bind it to other interfaces.
- Downloads happen only when the user asks for a miner install.
- **No telemetry, no analytics, no update check, no account.** The app makes no
  other network calls.

## One miner, explicit actions

- The supervisor never spawns a second miner and never kills an external miner
  without user action; Start explains the conflict instead.
- Per-GPU participation never falls back to “all cards”. If one card is
  excluded and the selected miner has no documented device-selection flag,
  Start fails before any helper call; the flag comes from the catalogue, not
  from config or a prompt. The device list contains validated `nvidia-smi`
  indices only, and a benchmark entry for such a miner fails the same way.
  See [ADR 0007](adr/0007-per-gpu-participation.md).
- A PID is signalled only when `/proc/<pid>/exe`/`cmdline` matches the absolute
  `state.miner_path` recorded at spawn. The old name-only check is used only for
  legacy state files without a path: PIDs are reused and a custom binary has an
  arbitrary name, so killing by name could hit an unrelated process.
- External state is never claimed: Stop, shutdown, emergency-stop and the guard
  leave the process alone and only clear our state entry. A benchmark refuses
  to start while an external miner runs.
- A GUI crash is a no-op for mining. The GUI has no kill/restore path; only the
  daemon, systemd `ExecStopPost` and the guard own teardown. This is deliberate
  ([ADR 0006](adr/0006-failure-policy.md)), not an omission.
- Startup recovery stops an orphaned miner from a previous daemon and restores
  stock before the D-Bus name is served; it does not silently resume mining.
- A failed restore keeps `oc-snapshot.json` and `last_error`; the UI does not
  claim the stock state was restored. Do not delete the snapshot by hand.
- A running miner is never silently restarted: GPU/preset changes require the
  explicit Apply and restart / Apply and stop dialog.
- Stop restores the snapshot when `restore_stock_on_stop` is on; a failed
  restore is reported and the snapshot is kept for a retry. When the setting is
  off, Stop keeps the snapshot, leaves the GPU settings applied and reports
  `oc.needs_restore` instead of claiming the stock state was restored.
- Benchmarks stop only a miner this session owns, and only after confirming no
  external miner is running; they restart the previous miner afterwards.
- GPU mutations are serialized with `flock`; an in-flight helper is waited for
  or terminated before restore.

## Packaging and sandboxing

- The `.deb` installs the helper root-owned (0755) and does not enable the user
  service; D-Bus activation and the first launch handle that.
- `scripts/test-deb-install.sh` exercises `postinst`, `prerm` and `postrm` in a
  disposable root container: the generated per-user rule is written, kept on
  upgrade, removed on remove/purge, and the user config is never touched.
- Flatpak is not shipped: the sandbox blocks the host `nvidia-smi`, the polkit
  helper, the systemd user unit and runtime binary downloads. See
  [ADR 0003](adr/0003-packaging.md).
- The RPM spec is a template and is not released until it is tested.

## Limitations to keep in mind

- A pre-existing manual clock lock cannot be read back through `nvidia-smi`;
  Stop resets locks to the driver default. A snapshot that already had a
  non-default power limit is flagged (`was_tuned`) and the restore dialog offers
  the driver default as a second target.
- Device selection uses the miner's documented flag and the `nvidia-smi`
  index order. A heterogeneous or reordered rig can enumerate devices
  differently; verify the result with a benchmark. Trommel does not guess a
  mapping or silently switch to a different card.
- Clock locks are verified under load: memory is decisive (a mismatch stops the
  miner), core is a warning because the driver may fluctuate. An idle card can
  legitimately sit at the locked memory frequency, so the check is deferred
  until the miner has loaded the GPU.
- Fan control and per-card temperature limits are not available through
  `nvidia-smi` on consumer cards; fan flags are passed to the miner.
- Miner binaries are third-party code. Verify the upstream source, checksum and
  license yourself; Trommel cannot audit them.
- A same-user process can always read the config and call the D-Bus API. This is
  a desktop trust model, not multi-user isolation.
