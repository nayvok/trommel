# ADR 0009: Preset-scoped GPU settings and named GPU presets

- Status: accepted for the v4 configuration
- Date: 2026-10-30
- Supersedes: nothing; extends ADR 0005 and ADR 0007

## Context

The v3 configuration kept GPU power/clocks in a map of shared `GpuProfile`
objects. The GUI only ever used the `balanced` profile, every new preset cloned
the same `gpu_profile` string, and the tuning dialog's “Profile default” action
wrote to the draft and applied immediately. As a result:

- cancelling the tuning dialog could still leave an applied/dirty GPU change;
- switching mining presets did not switch the GPU setup;
- a new preset inherited another preset's overrides instead of starting stock;
- the dashboard gave no clear signal that a card switch belongs to the preset.

At the same time, users asked for reusable tuning presets for a card, not only a
single “recommended” or “profile default” choice.

## Decision

1. **GPU settings belong to the mining preset.** `Preset::gpu` is a
   `GpuProfile` with a fallback `default` and per-UUID `per_gpu` entries. The
   old `Config::gpu_profiles` map and `Preset::gpu_profile` reference are
   removed. A new preset starts with `GpuProfile::default()` (stock).
2. **Named GPU presets are per-card templates.** `Config::gpu_presets` stores
   `GpuPreset { name, gpu_uuid, gpu_name, tuning }`. Applying one copies its
   power/clocks into the active preset's card entry and records its id in
   `GpuTuning::gpu_preset` (metadata only). `gpu_preset` is ignored by
   `Config::diff`.
3. **Stock is explicit.** “Stock (default)” clears power/clocks, keeps the
   participation switch, and sets `custom = true` with empty values so it does
   not inherit a migrated fallback. The “Recommended 80 %” action is removed.
4. **Cancel is a no-op.** The tuning dialog only mutates the draft on Save.
   Selecting Stock or a saved GPU preset changes dialog widgets; Cancel/Esc
   closes without a dirty banner, Apply alert or toast.
5. **Deleting a mining preset deletes its private GPU values; named GPU presets
   remain.** Deleting a named preset clears links but never changes stored power
   or clocks.
6. **Temperature moves to `Behavior`.** One miner process receives one
   temperature flag, so `temp_limit_c`/`temp_resume_c` are global, not
   per-preset.
7. **One active miner remains.** All supported miners can drive several GPUs;
   a single process is therefore the right unit for multi-GPU mining. Separate
   miners per card would require per-card supervision and snapshots and is a
   separate product decision.

## Consequences

- Migration copies the referenced v3 profile into each preset. Profiles that no
  preset references are dropped; the changelog records this.
- Preset export/import is self-contained: it includes GPU settings and coin/pool
  display metadata.
- The GUI can show “Active preset: X” on the dashboard and explain that a card
  switch belongs to that preset.
- Future per-card or multi-session work can build on the UUID-keyed entries
  without changing the v4 ownership model.
