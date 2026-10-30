# ADR 0010: Hashrate watchdog, notify by default

- Status: accepted for B7.2
- Date: 2026-10-31
- Extends: [ADR 0006](0006-failure-policy.md) (bounded restart) and
  [ADR 0007](0007-per-gpu-participation.md) (per-card session)

## Context

ADR 0006 says a crash of the miner is handled by a bounded restart, but a
**live** miner can also degrade: one card stops hashing, the whole rig drops
below the expected rate, the API stops answering or the miner reports 0 H/s.
Until B7 the supervisor only reacted to `try_wait`, so this state was invisible
on the dashboard and never recovered.

At the same time, “a running miner must not be silently restarted” is a core
invariant. A hashrate reading can be noisy because of pool difficulty, thermal
throttling, a driver glitch or a slow miner API. Automatically restarting on
every dip would be worse than the problem.

## Decision

1. **The daemon owns the check.** The watchdog runs in the supervisor loop on
   the last status document; it does not call `nvidia-smi`, does not start a
   second process and never touches an external miner.
2. **Notify by default, restart is an explicit opt-in.** `behavior.watchdog`
   is off by default. When enabled, `watchdog_action = "notify"` only emits the
   existing `Notification` signal / desktop notification. `"restart"` must be
   selected deliberately.
3. **Restart goes through the existing session path.** The watchdog calls
   `Supervisor::stop` and `Supervisor::start` (snapshot, stock restore, device
   plan, external scan, busy guard) after the existing `restart_delay_s`
   backoff. It does not duplicate process supervision or create a second
   restart counter.
4. **Detection needs a warm-up and a window.** A fresh start is ignored for
   `min(window, 120)` seconds. A low/zero/card-down condition must persist for
   `watchdog_window_s` (default 300 s). One healthy sample clears it. This is
   the hysteresis that keeps pool variance from causing a restart.
5. **Stale data never restarts.** A late status sample, `source = "no data"`
   longer than the window, or a log fallback that stopped updating produces a
   notification, not a restart: the control plane cannot prove that the miner
   is the problem.
6. **No expected hashrate means no percentage check.** Without
   `preset.expected_hashrate` the watchdog still detects 0 H/s, a card that
   disappeared from the per-card data and stale miner data; it says so in
   Settings and in the status warnings.
7. **A card is named, not guessed.** Per-card records are used only when B7.1
   mapped them by a unique PCI bus id. An unmapped record never blames a card
   and never splits the total.
8. **Bounded restarts, then stop.** After `watchdog_max_restarts` consecutive
   failed restarts the watchdog stops the miner, restores stock through the
   normal Stop path and notifies. It never reboots the rig, never kills an
   external miner and does not retry after the limit until an explicit Start or
   a changed configuration.
9. **Every action has a notification and a log line.** The D-Bus
   `Notification` signal is emitted regardless of the desktop-notification
   setting; the desktop call stays gated by `behavior.notifications`.
10. **Pool difficulty is not a health signal.** The watchdog uses the miner's
    local hashrate, which pool difficulty does not change. Accepted/rejected
    shares are not used as a threshold because a difficulty change would create
    false positives.

## Consequences

- The default behaviour is unchanged: a degraded miner keeps running until the
  user opts into a restart. The dashboard/status already show the raw values.
- A restart is visible and slow (stop, restore, apply, start), and can fail; the
  watchdog counts a failed restart and eventually stops instead of looping.
- An operator must tune `expected_hashrate`, threshold and window for the
  rig/coin; the settings copy explains the false-positive trade-off.
- A future health check or benchmark-based expected value can replace the
  preset field without changing this ADR.
