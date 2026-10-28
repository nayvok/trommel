# Security model

Trommel runs a user service that applies GPU settings through a narrow root
helper and downloads third-party miner binaries. This document describes the
trust boundaries around that. Report vulnerabilities privately; see
[SECURITY.md](../SECURITY.md).

## Trust boundaries

| Component | Trust | Can do | Cannot do |
| --- | --- | --- | --- |
| `trommel` GUI/CLI | User session | Read/write its config draft, call D-Bus, download to the user store | Run root helpers directly, tune GPUs, spawn the supervisor's miner |
| `trommeld` | User session | Own the miner, call the helper through pkexec, read `nvidia-smi`, scan `/proc` | Run as root, write outside the user's dirs, grant polkit rights |
| `trommel-oc` | Root, invoked via pkexec | One `nvidia-smi` call per GPU with numeric arguments | Accept paths, shell text, arbitrary commands, other GPUs' UUID formats |
| Miner binary | User, third-party | Mine, bind a local API port, write its log | Be inspected or verified by Trommel beyond the executable/checksum checks |
| Pool API | Network | Return statistics to the daemon | Control the miner, trigger a config change, see the wallet in logs (the daemon does not log it) |
| GNOME extension | User session | Read status, Start/Stop/Toggle/OpenApp | Tune GPUs, install miners, write the config |

The D-Bus service is a **same-user** interface: any process running as the user
can call `GetConfig` and read the wallet. This is normal for a desktop service;
do not expose the bus name on the system bus or through a socket.

## The root helper

`/usr/libexec/trommel/trommel-oc` is the only privileged program.

- Accepts `apply --gpu GPU-… [--pl W] [--lgc MHz] [--lmc MHz]` and
  `reset --gpu GPU-… [--pl W]`, plus a read-only `status`.
- Validates the UUID shape (`^GPU-[0-9a-fA-F-]{8,64}$`) and ranges (power
  50–1000 W, clocks 100–4000 MHz) in both the Bash helper and
  `trommel-core::oc` before invoking it.
- Calls `nvidia-smi` directly with an argument vector; no paths, no shell
  interpolation, no environment-controlled commands.
- Waits briefly for the power limit to take effect and reports failures instead
  of claiming success.
- Has a 30 s timeout on the caller side; a missing polkit rule produces an
  explicit error, not a hung daemon.

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
  this for tracked files.
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
- A running miner is never silently restarted: GPU/preset changes require the
  explicit Apply and restart / Apply and stop dialog.
- Stop restores the snapshot; a failed restore is reported and the snapshot is
  kept for a retry.
- Benchmarks stop the real miner for their duration and restart it afterwards.

## Packaging and sandboxing

- The `.deb` installs the helper root-owned (0755) and does not enable the user
  service or the extension; D-Bus activation and the first launch handle that.
- Flatpak is not shipped: the sandbox blocks the host `nvidia-smi`, the polkit
  helper, the systemd user unit, the extension and runtime binary downloads.
  See [ADR 0003](adr/0003-packaging.md).
- The RPM spec is a template and is not released until it is tested.

## Limitations to keep in mind

- A pre-existing manual clock lock cannot be read back through `nvidia-smi`;
  Stop resets locks to the driver default. Status warns when the driver ignores
  a configured lock.
- Fan control and per-card temperature limits are not available through
  `nvidia-smi` on consumer cards; fan flags are passed to the miner.
- Miner binaries are third-party code. Verify the upstream source, checksum and
  license yourself; Trommel cannot audit them.
- A same-user process can always read the config and call the D-Bus API. This is
  a desktop trust model, not multi-user isolation.
