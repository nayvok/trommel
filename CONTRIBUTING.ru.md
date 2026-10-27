# Участие в Trommel

[English](CONTRIBUTING.md) · **Русский**

Спасибо за помощь. Trommel — небольшой проект на Rust/GTK; самое полезное —
записи в каталоге майнеров, тесты упаковки и отчёты по GPU.

## Подготовка

```bash
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
  gettext librsvg2-bin desktop-file-utils appstream
cargo build -p trommel --features gui
cargo build -p trommeld
cargo test --workspace
```

Если глобальный кэш Cargo только для чтения:

```bash
export CARGO_HOME="$PWD/.cargo-home"
```

## Локальный запуск

```bash
./install.sh --no-extension   # бинарники, хелпер, сервис
trommel status
trommel gui
```

Для UI можно сгенерировать детерминированные скриншоты с фейковым ригом на две
карты:

```bash
./scripts/screenshots.sh
```

## Что делать

- **Каталог майнеров**: версии, описания, комиссии, descriptor-ы опций и
  контрольные суммы в `resources/catalog/miners.toml`. См. `docs/miners.md`.
- **Пулы/монеты**: добавлять парсер только для стабильного документированного
  API.
- **GPU/OC**: тестировать на реальном железе; указывать модель, драйвер и
  стоковые значения.
- **Расширение GNOME**: проверять GNOME 50, включение/выключение через
  Extension Manager и поведение после релогина.
- **Упаковка**: цель v1 — `.deb`; RPM/AppImage/Flatpak требуют истории
  интеграции (см. `docs/packaging.md`).

## Правила кода

- Rust 2021, `cargo fmt`, без предупреждений clippy.
- Пользовательские строки — через `i18n::tr("English", "Русский")`; исходный
  язык — английский.
- Никакого shell при запуске майнеров; только `Command` с массивом аргументов.
  `shell-words` — только для разбора extra args.
- Не коммитить бинарники майнеров, секреты и личные кошельки.
- Демон — единственный источник правды; GUI редактирует draft.
- Обновлять тесты и документацию вместе с поведением.

## Коммиты и PR

Feature-ветки и Conventional Commits. Не смешивать уборку репозитория,
рефакторинг и изменение поведения. В PR: что и зачем, команды/результаты
тестов, модель GPU и версию GNOME для соответствующих изменений, скриншоты для
UI.

Отправляя изменения, вы соглашаетесь, что они могут распространяться под
будущей лицензией проекта. Расширение — GPL-3.0-or-later; лицензия приложения
пока не выбрана.

## Баг-репорт

Приложите `trommel status --json`, `systemctl --user status trommeld`,
`nvidia-smi --query-gpu=name,driver_version,power.limit,power.default_limit
--format=csv`, версию GNOME Shell и статус расширения. Никогда не присылайте
seed-фразу или приватный ключ; адрес кошелька публичный, но его можно скрыть.
