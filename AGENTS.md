# AGENTS.md — working on Trommel

Trommel is a GTK4/libadwaita manager for NVIDIA mining rigs. The daemon
(`trommeld`) owns the applied configuration, the miner process and the GPU
snapshot. The GUI keeps a draft; the CLI and the daemon's D-Bus surface are the
other clients.

**Start with [docs/README.md](docs/README.md).** It maps every document by
audience. The most useful references while coding:

- [docs/architecture.md](docs/architecture.md) — processes, D-Bus, lifecycle,
  config model and invariants.
- [docs/components.md](docs/components.md) — module-by-module key functions and
  “where to change X”.
- [docs/agents.md](docs/agents.md) — the JSON/CLI/MCP surface and the safety
  rules for an integrating agent.
- [docs/development.md](docs/development.md) — build/test/debug/screenshot
  workflow and the fake-rig harness.

## Repository layout

- `crates/core` — config v3, catalogue, GPU/OC, emergency-stop, guard,
  supervisor, state, status, D-Bus.
- `crates/daemon` — `trommeld` binary and polling/supervisor loops.
- `crates/cli` — `trommel` CLI, `mcp.rs`, and `gui.rs` (the GTK app behind the
  `gui` feature).
- `resources/catalog` — built-in miners/coins/pools (TOML, no binaries).
- `data` — desktop entry, metainfo, icons.
- `packaging/`, `polkit/`, `systemd/`, `scripts/`, `docs/`; no `po/` yet (the
  future gettext catalogue will live there).

## Commands

```bash
cargo test --workspace
cargo clippy --workspace --all-targets --features trommel/gui -- -D warnings
cargo fmt --check
cargo build -p trommel --features gui
cargo build -p trommel            # CLI-only build must keep working
cargo build -p trommeld
./scripts/e2e.sh
./scripts/test-mcp.sh
./scripts/ui-smoke.sh
./scripts/first-run-smoke.sh
./scripts/screenshots.sh
./scripts/package-deb.sh
./scripts/check-no-secrets.sh
./scripts/check-docs.sh
```

Set `CARGO_HOME=$PWD/.cargo-home` when the global cargo cache is read-only.
UI/screenshot scripts need a desktop session; the e2e/MCP tests do not.

## Invariants

- One miner at a time. Never spawn a second process; never kill an external
  miner without explicit user action.
- A PID is ours only when `/proc/<pid>/exe` (or the script argument in
  `cmdline`) matches `state.miner_path`. Never signal a PID by miner name
  alone: PIDs are reused and custom builds have arbitrary names.
- A GUI crash is a no-op. The daemon owns the session; killing the window must
  not stop mining or restore the snapshot.
- External miners are never killed by Stop, shutdown, emergency-stop, the guard
  or a benchmark. Start and benchmark refuse instead.
- Snapshot stock GPU state **before** touching power/clocks, and restore it on
  stop. Never hardcode a stock power limit.
- A running miner must not be silently restarted. Edits require the explicit
  Apply and restart / Apply and stop dialog.
- Miner binaries are never committed or bundled. Downloads require HTTPS and a
  SHA-256 when the catalogue provides one; archive extraction must reject
  absolute paths and `..`.
- No personal wallets, seed phrases or private paths in tracked files. Test
  fixtures may use only `prl1test…`.
- The GUI never runs `nvidia-smi`; only the daemon/helper does.
- Miner fee is editable only when `fee_configurable=true`. Do not fake it.
- Installed miners are global (`miner_installs`), not per-preset. A preset only
  references `miner` + `miner_version`.
- Desktop notifications are sent by the daemon through
  `org.freedesktop.Notifications`, gated by `behavior.notifications`; the same
  events also go out as the `Notification` D-Bus signal.
- Keep English as the source string; Russian is a translation. Use
  `trommel_core::i18n::tr(en, ru)` in Rust.
- Schema changes require `docs/schema/config.schema.json`, a `normalize()`
  default and a migration test.
- Update the matching document in the same commit; never add a feature only to
  the README and leave [docs/](docs/README.md) stale.

## Working in blocks

The roadmap is split into blocks (B0–B5). Work on one block per branch
(`feat/<block>-<topic>`), keep the scope frozen to that block and do not start
the next one in the same branch. Each block lands with its code, unit/e2e tests,
docs and changelog in the same commit series; a release tag follows the
repository checklist. Details are in [docs/development.md](docs/development.md#working-in-blocks)
and the status table in [docs/roadmap.md](docs/roadmap.md#block-plan).

## Documentation rules

- [docs/README.md](docs/README.md) is the index and owns the “one home per fact”
  rule. Architecture, components, design and agents docs are the canonical
  homes; link instead of copying.
- No aspirational present tense in the README or user guide. Future work goes to
  [docs/roadmap.md](docs/roadmap.md).
- Historical documents ([CHANGELOG.md](CHANGELOG.md), ADRs) are frozen; add an
  entry/ADR instead of rewriting them.
- `./scripts/check-docs.sh` must pass: relative markdown links resolve, removed
  docs are not referenced, and every docs file is reachable from the index.
- Screenshots are generated by `./scripts/screenshots.sh`; do not hand-edit them
  or reference an image the script does not produce.

## External agents

`trommel diagnose --json`, `config diff/apply`, preset export/import and
`trommel mcp` are the intended integration surface. The MCP server is read-only
unless started with `--allow-write`; never auto-apply a proposed profile. See
[docs/agents.md](docs/agents.md).

## Adding a miner

1. Add a `[[miners]]` entry to `resources/catalog/miners.toml`: description,
   homepage, license, supported coins/algos, fee, versions, options.
2. Add a driver branch only if core arguments differ from the generic/WildRig
   shape; otherwise catalogue options are enough.
3. Add tests for argument rendering and fee handling.
4. Do not invent URLs or checksums; leave `sha256 = ""` and `verified = false`
   until a real download was checked.
5. Update [docs/miners.md](docs/miners.md) if the schema or a branch changed.

## Commits

Conventional Commits, one feature per branch, merge with `--no-ff`:

```
feat(miners): add metadata, fee model and option schemas
feat(oc): restore pre-mining power limits per GPU
feat(ui): add miner info, fee breakdown and dynamic settings
feat(notifications): send desktop alerts from the daemon
docs: restructure documentation by audience
```

Before committing: `cargo fmt`, `cargo test`, `scripts/check-no-secrets.sh`,
`scripts/check-docs.sh`, and review `git diff --cached`. Never commit `local/`,
`dist/`, `target/`, miner binaries, `.mo` files or screenshot scratch
directories.

## Definition of done

- Builds with and without the `gui` feature.
- Unit/e2e tests cover the changed behavior; fake `nvidia-smi` for GPU paths.
- `appstreamcli validate` and `desktop-file-validate` pass for touched assets.
- Docs/README updated if user-visible behavior changed; `check-docs.sh` passes.
- No new warnings from `cargo clippy -D warnings`.
