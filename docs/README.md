# Trommel documentation

Trommel is a GTK4/libadwaita control room for a local NVIDIA mining rig. The
daemon `trommeld` owns the applied configuration, the miner process and the GPU
snapshot; the GTK app and the GNOME Shell extension are clients.

This directory documents the shipped 0.4.x code. If a document and the source
disagree, the source is right — fix the document in the same change that fixes
the code.

## Start here

| You are | Read, in this order |
| --- | --- |
| A new user | [README](../README.md) → [User guide](user-guide.md) |
| A contributor | [CONTRIBUTING](../CONTRIBUTING.md) → [Development](development.md) → [Architecture](architecture.md) → [Components](components.md) |
| Building an integration or an agent | [Agents](agents.md) → [Architecture § Status contract](architecture.md#status-contract) → [Config schemas](schema/config.schema.json) |
| Reviewing UI or visual work | [Design](design.md) → [User guide](user-guide.md) |
| Packaging or releasing | [Packaging](packaging.md) → [Verification](verification.md) |
| Reviewing security | [Security](security.md) → [ADR 0003](adr/0003-packaging.md) |
| Looking for direction | [Roadmap](roadmap.md) |

Russian mirrors exist for the user-facing documents: [README.ru](../README.ru.md),
[user-guide.ru](user-guide.ru.md), [design.ru](design.ru.md),
[CONTRIBUTING.ru](../CONTRIBUTING.ru.md). Technical reference documents are kept
in English only, matching the source strings and the catalogue fields.

## Document map

| Document | Answers | Update when |
| --- | --- | --- |
| [architecture.md](architecture.md) | How the processes, data flow and safety rules fit together | A process boundary, D-Bus method, lifecycle or state machine changes |
| [components.md](components.md) | Which module does what, key types and functions, where to change X | A module, public function or responsibility changes |
| [design.md](design.md) | Every visible surface, navigation, layout, colour, copy and empty states | A screen, dialog, interaction or visual rule changes |
| [development.md](development.md) | Build, test, debug, screenshot and release workflow | Tooling, test harness or commands change |
| [verification.md](verification.md) | What is automated, what needs a real rig | A test surface or known limitation changes |
| [agents.md](agents.md) | The CLI/JSON/MCP surface and the safety rules for external agents | A command, JSON field or MCP tool changes |
| [user-guide.md](user-guide.md) | Everyday operation from install to troubleshooting | User-visible behaviour changes |
| [miners.md](miners.md) | Catalogue fields, option descriptors, driver branches | The catalogue or argument builder changes |
| [extension.md](extension.md) | The GNOME Shell applet and its shared settings | The extension or its schema changes |
| [packaging.md](packaging.md) | Release formats, deb layout, CI workflows | Packaging or release automation changes |
| [security.md](security.md) | Trust boundaries, helper, downloads, redaction | A privileged path or a security-relevant behaviour changes |
| [i18n.md](i18n.md) | The runtime translation helper and the gettext plan | Localisation mechanics change |
| [roadmap.md](roadmap.md) | What shipped and what is next | A release ships or a plan is adopted |
| [adr/](adr/) | Why a decision was made | Never — add a new ADR instead |

Historical, intentionally frozen documents: [CHANGELOG](../CHANGELOG.md) and the
ADRs. Do not rewrite them to match the present; add a new entry or ADR.

## Rules for keeping docs honest

1. **No aspirational present tense.** A feature that is not merged is described
   in [roadmap.md](roadmap.md), not in the user guide or the README.
2. **One home per fact.** The D-Bus contract lives in
   [architecture.md](architecture.md); the module list in
   [components.md](components.md); the screen inventory in
   [design.md](design.md). Other documents link instead of copying.
3. **Version numbers only where they are data**: catalogue entries, the
   changelog, release assets and the verification matrix. The README and guides
   describe behaviour, not a version.
4. **Screenshots are generated.** `scripts/screenshots.sh` writes
   `docs/images/*.png` from a fake two-GPU rig; do not hand-edit screenshots and
   do not reference images that the script does not produce.
5. **Legacy paths are transitional.** The only code that may mention the former
   product name is the one-time config migration (`paths::legacy_config_path`)
   and the historical changelog.
6. **Every relative link must resolve.** CI does not check markdown links yet;
   `scripts/check-docs.sh` does (run it before a documentation commit).
