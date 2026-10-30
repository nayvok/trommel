# ADR 0005: One active mining session, presets as the centre

Status: accepted, 2026-10-29.

## Context

Trommel has four top-level pages (Dashboard, Presets, Miners, Settings). The
Miners page duplicates the miner selection in the preset editor and forces a
second mental model: a miner is global, but a preset is what the user actually
runs. At the same time, a multi-GPU rig may have cards that cannot mine the
same coin, which suggests running several miners at once.

The current status contract is built around one running miner: one hashrate,
one pool, one fee, one OC snapshot and one overheat alert. Multi-miner would
require per-job status, API ports, logs, OC snapshots, notifications and a
different dashboard.

## Decision

1. **One active mining session.** At most one miner process and one coin are
   mining at a time, on a disjoint set of GPUs. Presets may describe different
   miners and coins, but only one preset is active.
2. **Presets are the primary object.** A preset owns coin, algorithm, pool,
   wallet, miner id and miner version. The Miners page is replaced by a miner
   library dialog opened from the presets page and the preset editor.
3. **Miner versions stay global and versioned.** `miner_installs` remains keyed
   by `miner@version`; a preset pins one version. Several versions may be
   installed for rollback. Global update/remove operations must show the
   affected presets and must never rewrite them silently.
4. **Per-GPU participation is the supported multi-GPU feature.** A card that
   cannot mine the active coin is excluded from the mining process, not
   silently left mining. Multi-miner/multi-job is not implemented.
5. Revisiting multi-miner requires a new ADR. It is not a permanent non-goal,
   but it is out of scope until the single-session path, fault tolerance and
   per-GPU participation are shipped and verified.

## Consequences

- The dashboard keeps one coherent set of metrics and alerts.
- The miner library must own install/update/remove/verify and show preset
  impact; the preset editor links to it.
- Fault tolerance stays simple: one snapshot and one supervisor per session.
- Heterogeneous cards are handled by excluding a card, choosing a multi-algo
  miner, or accepting an idle card. A future multi-session mode is a separate
  product decision.
