# Trommel documentation

Trommel is a GTK4/libadwaita control room for a local NVIDIA mining rig. The
daemon `trommeld` owns the applied configuration, the miner process and the GPU
snapshot; the GTK app and the CLI are clients.

This directory documents the current `main`. The latest published release is
**0.8.2**; `main` also contains the unreleased B6/B7 work described in the
changelog and the roadmap. If a document and the source disagree, the source is
right — fix the document in the same change that fixes the code.

## Start here

| You are | Read, in this order |
| --- | --- |
| A new user | [README](../README.md) → [User guide](user-guide.md) |
| A contributor | [CONTRIBUTING](../CONTRIBUTING.md) → [Development](development.md) → [Architecture](architecture.md) |
| Building an integration or an agent | [Agents](agents.md) → [Architecture § Status contract](architecture.md#status-contract) → [Config schema](schema/config.schema.json) and the [status schema](schema/status.schema.json) |
| Reviewing UI or visual work | [Design](design.md) → [User guide](user-guide.md) |
| Packaging, releasing or testing | [Development § Packaging and releases](development.md#packaging-and-releases) → [Development § Test matrix](development.md#test-matrix) |
| Reviewing security | [SECURITY](../SECURITY.md) → [Architecture § Security model](architecture.md#security-model) |
| Looking for direction | [Roadmap](roadmap.md) → [Decisions](decisions.md) |

Russian is available for the README only: [README.ru](../README.ru.md). All
other documents, including this index, are English.

## Document map

| Document | Answers | Update when |
| --- | --- | --- |
| [architecture.md](architecture.md) | Processes, modules, D-Bus, lifecycle, config model, status contract, security model and files | A process boundary, module, method, lifecycle, state machine or trust boundary changes |
| [development.md](development.md) | Build, test, verification, screenshots, packaging, release and i18n | Tooling, the test matrix, packaging or release automation changes |
| [user-guide.md](user-guide.md) | Everyday operation from install to troubleshooting | User-visible behaviour changes |
| [design.md](design.md) | Every visible surface, navigation, layout, colour, copy and empty states | A screen, dialog, interaction or visual rule changes |
| [miners.md](miners.md) | Catalogue fields, option descriptors, driver branches, device flags and fees | The catalogue or argument builder changes |
| [agents.md](agents.md) | The CLI/JSON/MCP surface and the safety rules for external agents | A command, JSON field or MCP tool changes |
| [roadmap.md](roadmap.md) | What shipped, what is implemented on `main` and what is next | A release ships or a block lands |
| [decisions.md](decisions.md) | Why an architectural decision was made | Never rewritten; append a new ADR |
| [schema/](schema/) | JSON schemas for config, presets and status | A schema changes |

Historical documents: [CHANGELOG](../CHANGELOG.md) and
[decisions.md](decisions.md). Both are append-only; add an entry instead of
rewriting the past.

## Rules for keeping docs honest

1. **No aspirational present tense.** A feature that is not merged is described
   in [roadmap.md](roadmap.md), not in the user guide or the README.
2. **One home per fact.** The D-Bus contract and module map live in
   [architecture.md](architecture.md); the screen inventory in
   [design.md](design.md); the test matrix and release process in
   [development.md](development.md). Other documents link instead of copying.
3. **Only the README is bilingual.** `README.md` and `README.ru.md` mirror each
   other; every other document is English.
4. **Version numbers only where they are data**: catalogue entries, the
   changelog, release assets and the verification matrix. The README and guides
   describe behaviour, not a version.
5. **Screenshots are generated.** `scripts/screenshots.sh` writes
   `docs/images/*.png` from a fake two-GPU rig; do not hand-edit screenshots and
   do not reference an image the script does not produce.
6. **Legacy paths are transitional.** The only code that may mention the former
   product name is the one-time config migration
   (`paths::legacy_config_path`) and the historical changelog.
7. **Every relative link must resolve.** `scripts/check-docs.sh` runs in CI on
   every pull request and release; run it before a documentation commit.
