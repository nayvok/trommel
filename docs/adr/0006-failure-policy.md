# ADR 0006: Failure policy and emergency restore

Status: accepted, 2026-10-29.

## Context

Trommel applies power limits and clock locks before spawning the miner. A
process crash, a SIGKILL or a lost control session can leave those settings on
the cards. Relying on `Drop` or signal handlers is not enough: `SIGKILL` cannot
be handled, and a signal handler may not run when the desktop session dies.

The GUI is not the supervisor. It is a short-lived client of `trommeld`; a
window crash must not stop mining. The control plane is the daemon, the session
bus and the systemd user manager.

## Decision

1. **The daemon owns the session.** The GUI and CLI only send commands.
2. **Snapshot before the first mutation.** The OC snapshot is written with
   `applied = true` and the target UUIDs before the first helper call. A crash
   mid-apply therefore always leaves a restorable record.
3. **A crash of the miner is not a crash of the session.** With
   `auto_restart = true` the supervisor restarts with a bounded budget and
   backoff; when the budget is exhausted it stops and restores. With
   `auto_restart = false` the first crash stops and restores.
4. **A crash of the GUI is a no-op.** Mining continues. Closing the window is
   not an emergency. An opt-in "stop when the app quits" behaviour would be a
   separate feature, not the default.
5. **Loss of the control plane stops and restores.** SIGTERM, a closed session
   bus, a stopped systemd unit and a `SIGKILL`ed daemon all lead to
   `trommel emergency-stop`: stop only the miner this installation owns,
   restore the snapshot, keep the snapshot on failure.
6. **Independent supervisors.** The packaged service uses systemd
   `ExecStopPost` plus `KillMode=control-group`; a lightweight guard process
   covers manual runs. Startup recovery is the final net: an orphaned miner is
   stopped, an applied snapshot is restored, and mining is not silently
   resumed.
7. **Restore failures are visible.** The snapshot and `last_error` are kept so
   a later start/guard can retry; the UI must not claim the stock state was
   restored.

## Consequences

- `trommel emergency-stop` must be idempotent and safe to call from systemd,
  the guard and startup recovery.
- OC calls are serialized with a file lock across processes; an in-flight
  helper is waited for or terminated before restore.
- A daemon restart after a crash no longer adopts the miner and continues
  mining. It stops it, restores the stock state and leaves mining stopped.
- A reboot is safe without action: the boot id invalidates the snapshot and the
  driver has already reset the settings.
