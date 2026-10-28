# Trommel — план следующей goal-сессии

**Статус:** v0.2 собран и закоммичен; рабочее дерево чистое.
**Цель следующей сессии:** довести UX обычного пользователя до конца
(quick-setup wizard, advanced-настройки, документация/скриншоты, smoke-тесты
MCP), не сломав уже сделанные опциональные локи и текущий e2e.

> **Обновление после UX-итерации:** основная часть раздела 2 уже сделана:
> карточки GPU, per-GPU список в настройках, опциональные core/memory locks,
> safe defaults без локов, copy-to-similar/all, simple mode скрывает локи,
> e2e проверяет `--lmc`, тесты 66. Следующий goal должен начинать с
> оставшихся пунктов: quick-setup wizard, advanced-скриншот, MCP smoke test,
> финальная упаковка/документация.

---

## 0. Как продолжить после компакта

1. Прочитать `AGENTS.md` (инварианты) и этот файл.
2. Проверить состояние:

```bash
cd /home/nayvok/pearl
git status --short
git log --oneline -8
CARGO_HOME=$PWD/.cargo-home cargo test --workspace
CARGO_HOME=$PWD/.cargo-home ./scripts/e2e.sh
```

3. Работать feature-ветками и Conventional Commits, как в `AGENTS.md`.
4. Не коммитить `local/`, `dist/`, `.shots/`, бинарники и личные кошельки.

### Goal prompt (можно вставить в goal-режим)

> Работай над проектом Trommel в `/home/nayvok/pearl`. Сначала прочитай
> `AGENTS.md` и `PLAN.md` — опциональные локи, карточки GPU и safe defaults
> уже сделаны. Доведи UX: quick-setup wizard для 1–3 карт, advanced-режим
> с локами/таблицей настроек, копирование профиля на одинаковые/все карты,
> понятные пустые состояния и документацию. Добавь MCP smoke test, обнови
> скриншоты/docs/CHANGELOG, пересобери `.deb`, прогони unit/e2e/clippy/fmt.
> Не ломай существующие e2e/тесты и не применяй настройки без подтверждения.

---

## 1. Что уже сделано

- Приложение переименовано в **Trommel**, новый icon set, desktop/metainfo,
  GSettings, systemd/D-Bus/polkit, расширение GNOME 50 без `prefs.js`.
- Config v2: пресеты, GPU-профили (default + per-GPU по UUID), глобальные
  `miner_installs`, `miner_options`, fee breakdown.
- Мульти-GPU: `nvidia-smi` по всем картам, карточки/строки, per-GPU настройки.
- Снимок стока до старта, восстановление при stop, stale boot cleanup,
  fallback на `power.default_limit`, helper `apply/reset --gpu <uuid>`.
- Apply-flow: live / restart / stop, диалог, rollback, e2e.
- Каталог майнеров, download/import, gzip/zip с защитой путей, SHA-256,
  глобальное состояние установки, fee fixed/configurable, динамические опции.
- Simple mode (по умолчанию), quick setup entry point, recommended GPU profile,
  copy diagnostics.
- Agent surface: `trommel diagnose`, `config validate/export/diff/apply`,
  `preset export/import`, MCP server `trommel mcp` (read-only по умолчанию).
- Debian package, dev install/uninstall, CI, EN/RU docs, ADR.
- Тесты: 63 unit, e2e зелёный, package layout проходит, clippy/fmt зелёные.

Текущие коммиты (верхушка):

```
1a6cd6c feat(ui): add simple mode, quick setup and agent diagnostics
f0b6a32 feat(agents): add diagnostics, config diff/apply and MCP server
f32f933 feat(config): add simple-mode preference
6b64d5f docs: record naming, extension and packaging decisions
...
```

---

## 2. Опциональные локи частот (выполнено, оставлено как спецификация)

### 2.1 Текущее состояние

`GpuTuning` уже содержит `core_clock_mhz: Option<u32>` и
`memory_clock_mhz: Option<u32>`. `None` означает «не трогать», `Some(N)` —
`-lgc/-lmc` через helper. Но:

- `GpuTuning::default()` сейчас содержит `Some(1620)` и `Some(810)`, то есть
  **новый профиль по умолчанию уже с локами**.
- В Settings и в per-GPU диалоге есть поля частот, но нет явного
  «включить лок»; 0 трактуется как «не трогать», что неочевидно.
- В Simple mode поля частот всё равно доступны в диалоге тюнинга.
- Пользовательский кейс: на некоторых монетах **низкий лок памяти** снижает
  потребление и повышает хешрейт; на других — вредит. Нужно «по желанию,
  если шаришь», с понятным пояснением.

### 2.2 Решение

- `Option<u32>` остаётся источником правды: `None` = auto, `Some` = lock.
- **Новые профили по умолчанию без локов**: `core_clock_mhz: None`,
  `memory_clock_mhz: None`. Power limit по умолчанию тоже `None` (не менять),
  temp limit 85/65. «Recommended» выставляет безопасный стартовый профиль:
  80% от `power.default_limit`, локи выключены, temp 85.
- **Миграция v1**: старый `core_clock`/`memory_clock` сохраняются как `Some`
  (у пользователя был 1620/810 — он это увидит и сможет выключить).
- **UI**:
  - В per-GPU “Tune” диалоге: switch **“Lock core clock”** + spin (доступен
    только при switch on), switch **“Lock memory clock”** + spin; подпись
    «Off = driver auto. Some coins benefit from a lower memory lock; test it.»
  - В Settings → GPU defaults — то же для профиля по умолчанию.
  - В Simple mode: показывать только power/temp; локи — под “Advanced”.
    Если пользователь включил Advanced, switches появляются.
  - «Recommended» всегда оставляет локи выключенными.
  - Значение 0/пусто = off; не отправлять `--lgc/--lmc`.
- **Применение**:
  - `miners::build_args` / `oc::apply_args` уже добавляют флаги только для
    `Some`; проверить, что `0`/`None` не попадают.
  - Stop: `-rgc -rmc` уже сбрасывает локи; при `restore_stock_on_stop=false`
    снимок сохраняется, локи остаются до следующего reset.
  - В статусе/логе показывать, что лок применён: `oc.applied` + per-GPU
    `tuning_applied`; при желании добавить `core_locked`/`memory_locked`.
- **Валидация**: core 100–4000 MHz, memory 100–12000 MHz; предупреждать, если
  значение вне диапазона текущей карты/драйвера; не silently clamp.
- **Зависимость от монеты**: локи живут в GPU-профиле, профиль выбирается
  пресетом; разные монеты/пулы могут иметь разные профили. Явно написать это
  в docs.

### 2.3 Файлы

- `crates/core/src/config.rs` — defaults, migration, tests.
- `crates/core/src/oc.rs` — args/validation/tests.
- `crates/core/src/miners.rs` — если опции влияют на аргументы.
- `crates/core/src/supervisor.rs` — применение/сброс, e2e.
- `crates/core/src/status.rs` — при необходимости поля lock state.
- `crates/cli/src/gui.rs` — switches/spins, simple/advanced, recommended.
- `scripts/e2e.sh` — fake helper assertions для `--lgc/--lmc` и reset.
- `docs/user-guide.md/.ru.md`, `README.md/.ru.md`, `CHANGELOG.md`.
- `docs/images/*` — обновить скриншоты, если UI изменился.

### 2.4 Тесты и приёмка (выполнено)

- Unit: default без локов; `Some` добавляет `--lgc/--lmc`; `None`/0 не
  добавляет; миграция v1 сохраняет 1620/810; ranges.
- E2E: start с memory lock → helper log содержит `--lmc 810`; stop → reset
  `-rmc`; start без lock → нет `--lmc`; apply restart.
- UI: switch off → спиннер disabled/скрыт; on → значение; Recommended
  выключает локи; Simple mode не показывает их без Advanced.
- Ручное на железе: 3080 Ti, 300 W + mem 810 (текущий кейс) и без лока;
  сравнить хешрейт/потребление; stop возвращает сток.
- Acceptance: новый конфиг не меняет частоты; старый сохраняет; пользователь
  может включить/выключить лок на конкретной карте; docs объясняют риски.

---

## 3. Доработки UX для «обычного пользователя»

- Довести Quick setup до мастера из 3 шагов (GPU → майнер → кошелёк/старт),
  а не только кнопки, открывающей редактор.
- В Simple mode скрыть fee override, extra args, miner options; оставить
  power/temp/Start.
- Проверить, что Recommended не выглядит как «оптимальные настройки»:
  это безопасный старт, а не обещание максимума.
- Добавить понятное пустое состояние «майнер не установлен» с Download/Import
  прямо в пресете (частично есть).
- Локализовать оставшиеся технические строки в UI (gettext — позже, сейчас
  `tr(en, ru)`).

## 4. Agent/MCP hardening

- MCP уже есть; добавить тест-скрипт `scripts/test-mcp.sh` с initialize/
  tools/list/tools/call и проверкой, что write-инструменты недоступны без
  `--allow-write`.
- `trommel config apply` и MCP apply должны требовать явного режима и
  возвращать diff; не auto-apply.
- Диагностика не должна содержать wallet/seed; проверить redaction.
- Документировать Codex config и ограничения.
- Встроенный AI-ассистент не делать в v1; внешний агент — достаточно.

## 5. Упаковка и проверки

- Пересобрать `.deb` после изменений, `scripts/test-deb.sh`, установка в
  Ubuntu 26.04 container.
- Проверить `appstreamcli`, `desktop-file-validate`,
  `glib-compile-schemas --strict`, `gnome-extensions pack`.
- Расширение вручную: install/enable, Extension Manager, relogin, app state.
- RPM/AppImage/Flatpak — только docs; не релизить.

## 6. Известные ограничения / риски

- В текущей sandbox нет `/dev/nvidia*`; реальные GPU-тесты только в сессии
  пользователя. E2E использует fake nvidia-smi/helper.
- Расширение нельзя полностью проверить в sandbox (writes в `~/.cache`/user
  extension path); нужен ручной прогон.
- Точное восстановление pre-existing core/mem lock невозможно; stop делает
  `-rgc/-rmc` (драйверный default), power limit — по снимку. Это описано.
- Лицензия приложения ещё не выбрана; AppStream `LicenseRef-proprietary`;
  extension GPL-3.0-or-later.
- Нет gettext-каталогов; сейчас runtime `tr(en, ru)`. При переходе на gettext
  сохранить английский source и добавить `po/ru.po`.
- Старый `pearl-minerd`/extension могут остаться в системе; новый install не
  удаляет их автоматически (миграция/cleanup — отдельно).

## 7. Порядок работ (обновлён)

1. ~~`feat(config): make GPU clock locks opt-in`~~ — сделано.
2. ~~`feat(ui): add explicit core/memory lock switches`~~ — сделано.
3. ~~`test(e2e): cover optional locks and reset`~~ — сделано.
4. ~~`docs: explain memory lock trade-offs`~~ — базово сделано, расширить.
5. `feat(ui): finish quick-setup wizard` — 3 шага, 1–3 карты.
6. `feat(ui): add advanced GPU table / advanced screenshot` — таблица карт,
   locks, копирование на similar/all уже есть в диалоге.
7. `test(agents): add MCP smoke test` + docs.
8. Rebuild deb, screenshots, final checks, tag `v0.2.1` (или `v0.3.0`).

## 8. Definition of Done

- Новый профиль не содержит локов; старый конфиг сохраняет 810/1620.
- На каждой карте можно включить/выключить core/memory lock отдельно.
- Simple mode не показывает локи без Advanced.
- `--lgc/--lmc` уходят helper только при `Some`; stop делает reset.
- Тесты/e2e/clippy/fmt зелёные; `.deb` собирается; docs обновлены.
- В `git` нет секретов/бинарников; коммиты по фичам.
