# ADR 0007: Per-GPU participation, not just tuning

Status: accepted, 2026-10-29. Implements point 4 of
[ADR 0005](0005-one-active-session.md); it does not supersede that decision.

## Context

Before this decision `GpuTuning.enabled` only decided whether Trommel applied a
card's power limit and clock locks. The miner was still started without any
device flag, so a card switched off in the profile kept mining: the UI said
“disabled” while the card was hashing. ADR 0005 requires the opposite — a card
that cannot mine the active coin is **excluded from the mining process**, not
silently left in it.

The miner catalogue already had a `per_gpu_devices` hint, but no documented
flag and no code used it. Guessing a flag would be worse than refusing: a wrong
flag can silently move the exclusion to another physical card.

## Decision

1. **`GpuTuning.enabled` means participation.** `false` excludes the card from
   the active mining session. An excluded card is
   - not passed to the miner;
   - not tuned (no power limit, core or memory lock);
   - returned to its previous settings if an earlier session on this boot left
     tuning on it;
   - shown in the status and UI as excluded, not as mining.
2. **One miner, one coin, one snapshot, one session.** Exclusion changes the
   device set of the single miner. Multi-job and a second process remain out of
   scope.
3. **Only documented miner flags are used.** The catalogue records the flag in
   `device_flag`; a flag is set only when the upstream documentation names it:

   | Miner | Flag | Upstream meaning |
   | --- | --- | --- |
   | WildRig Multi | `--gpu-list` | list of GPUs to use (all by default) |
   | SRBMiner-MULTI | `--gpu-id` | ids from `--list-devices`, comma-separated |
   | OneZeroMiner | `--devices` | comma-separated list of devices to use |

   `per_gpu_devices` alone is not enough: a miner must also carry a non-empty
   `device_flag`.
4. **No silent fallback.** When at least one detected card is excluded and the
   selected miner has no documented way to select devices, Start fails with an
   explicit error. Trommel never falls back to “run on every card”. The same
   refusal applies to a benchmark entry for such a miner.
5. **Device identity.** Device lists are `nvidia-smi` indices, shown on
   the dashboard as `GPU #N`. When an explicit list is passed, the supervisor
   sets `CUDA_DEVICE_ORDER=PCI_BUS_ID` for CUDA miners so their enumeration
   matches `nvidia-smi`; an explicit `preset.env` value still wins. WildRig uses
   OpenCL, whose device order is PCI as well.
6. **Snapshots cover the cards the session actually touches.** `applied_gpus`
   contains only participating, tuned cards. Stop, shutdown and emergency-stop
   restore exactly those cards. An excluded card is never part of the new
   snapshot’s applied set.
7. **Exclusion is visible.** The status document carries a per-card
   `participation` list (`enabled`, `mining`), `total.gpus_enabled` counts
   participating cards, and the dashboard labels each card as mining/excluded
   and shows “N of M” in Totals.

## Consequences

- An excluded card can still be monitored (temperature, power, fan); it simply
  is not part of the miner.
- Aggregate rig power and the hottest temperature keep covering every detected
  card, including excluded ones, because they are physical rig readings. The
  tooltips say so; hashrate comes from the miner and therefore only from
  participating cards.
- A partial participation requires restarting a running miner, like any other
  GPU or preset change: the existing Apply and restart / Apply and stop dialog
  is the only path.
- Miner enumeration can differ from `nvidia-smi` on a heterogeneous or
  reordered rig. The mapping is documented; a real benchmark remains the
  verification step, and a future driver-level probe can replace the index
  assumption without changing the model.
- Adding a new miner requires a real upstream flag before it can take part in a
  partial rig; do not add a guessed flag to make the UI accept it.
