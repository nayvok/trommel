# Участие в Trommel

[English](CONTRIBUTING.md) · **Русский**

Спасибо за помощь. Trommel — небольшой проект на Rust/GTK; самое полезное —
записи в каталоге майнеров, тесты упаковки, отчёты по GPU и правки
документации. Начните с [индекса документации](docs/README.md), затем откройте
нужный гид:

- код и тесты — [docs/development.md](docs/development.md),
  [docs/components.md](docs/components.md);
- экраны и тексты — [docs/design.ru.md](docs/design.ru.md);
- каталог — [docs/miners.md](docs/miners.md);
- упаковка — [docs/packaging.md](docs/packaging.md).

## Подготовка

```bash
sudo apt install build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
  gettext librsvg2-bin desktop-file-utils appstream \
  gir1.2-atspi-2.0 dbus xdg-utils
cargo build -p trommel --features gui
cargo build -p trommel          # сборка без GUI тоже должна работать
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

Для UI используйте изолированные стенды, а не реальный риг:

```bash
./scripts/e2e.sh              # CLI-жизненный цикл, фейковые GPU/хелпер/майнер
./scripts/ui-smoke.sh         # клики через AT-SPI (нужна desktop-сессия)
./scripts/first-run-smoke.sh  # путь чистой установки
./scripts/screenshots.sh      # детерминированные скриншоты docs
```

Они создают свои XDG-каталоги, приватный D-Bus и фейковый `nvidia-smi`; см.
[docs/development.md](docs/development.md#test-harness-isolation). Не ослабляйте
эту изоляцию.

## Что делать

- **Каталог майнеров**: версии, описания, комиссии, descriptor-ы опций и
  контрольные суммы в `resources/catalog/miners.toml`. См.
  [docs/miners.md](docs/miners.md).
- **Пулы/монеты**: добавлять парсер только для стабильного документированного
  API.
- **GPU/OC**: тестировать на реальном железе; указывать модель, драйвер,
  стоковые значения и что показал лог хелпера.
- **Расширение GNOME**: проверять GNOME 50, включение/выключение через
  Extension Manager и поведение после релогина.
- **Упаковка**: релизная цель — `.deb`; RPM/AppImage/Flatpak требуют истории
  интеграции (см. [docs/packaging.md](docs/packaging.md)).
- **Документация**: если документ и код расходятся, правьте документ в том же
  изменении. Запускайте `./scripts/check-docs.sh`.

## Правила кода

- Rust 2021, `cargo fmt`, без предупреждений clippy с
  `--features trommel/gui`.
- Пользовательские строки — через `i18n::tr("English", "Русский")`; исходный
  язык — английский. Не склеивайте переведённые фрагменты.
- Никакого shell при запуске майнеров; только `Command` с массивом аргументов.
  `shell-words` — только для разбора extra args.
- Не коммитить бинарники майнеров, секреты и личные кошельки.
- Демон — единственный источник правды; GUI редактирует draft.
- Обновлять тесты, соответствующий документ и, для видимых изменений,
  скриншоты.
- `trommel-core` не должен знать про GTK и содержать политику только для демона.

## Коммиты и PR

Feature-ветки и Conventional Commits. Не смешивать уборку репозитория,
рефакторинг и изменение поведения. В PR: что и зачем, команды/результаты
тестов (unit, e2e, MCP, UI smoke — по ситуации), модель GPU и версию GNOME для
соответствующих изменений, скриншоты для UI и обновление документации.

Перед пушем:

```bash
cargo fmt --check
cargo clippy --workspace --all-targets --features trommel/gui -- -D warnings
cargo test --workspace
./scripts/check-no-secrets.sh
./scripts/check-docs.sh
```

Отправляя изменения, вы соглашаетесь, что они могут распространяться под
будущей лицензией проекта. Расширение — GPL-3.0-or-later; лицензия приложения
пока не выбрана.

## Баг-репорт

Приложите `trommel status --json`, `trommel diagnose --json`,
`systemctl --user status trommeld`, журнал демона,
`nvidia-smi --query-gpu=name,driver_version,power.limit,power.default_limit
--format=csv`, версию GNOME Shell и статус расширения. Никогда не присылайте
seed-фразу или приватный ключ; адрес кошелька публичный, но его можно скрыть.
