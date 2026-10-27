#!/bin/bash
# Установка Pearl Miner (демон + приложение + расширение GNOME Shell 50).
#
#   ./install.sh                # всё: зависимости, сборка, polkit, systemd, расширение
#   ./install.sh --no-build     # не пересобирать Rust (использовать target/release)
#   ./install.sh --no-extension # без расширения GNOME Shell
#   ./install.sh --with-hotkey  # добавить Ctrl+Alt+M на старт/стоп
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
UNIT_DIR="$HOME/.config/systemd/user"
DBUS_DIR="$HOME/.local/share/dbus-1/services"
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
SYM_ICON_DIR="$HOME/.local/share/icons/hicolor/symbolic/apps"
METAINFO_DIR="$HOME/.local/share/metainfo"

DO_BUILD=1
DO_EXTENSION=1
DO_HOTKEY=0
for arg in "$@"; do
    case $arg in
        --no-build) DO_BUILD=0 ;;
        --no-extension) DO_EXTENSION=0 ;;
        --with-hotkey) DO_HOTKEY=1 ;;
        -h|--help) sed -n '2,9p' "$0"; exit 0 ;;
        *) echo "Неизвестный аргумент: $arg" >&2; exit 2 ;;
    esac
done

say() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m  ! %s\033[0m\n' "$*"; }
ok() { printf '\033[1;32m  ✓ %s\033[0m\n' "$*"; }

# ---------------------------------------------------------------- зависимости
say "Проверяю зависимости сборки"
MISSING_PKGS=()
command -v cargo >/dev/null || MISSING_PKGS+=(cargo)
command -v pkg-config >/dev/null || MISSING_PKGS+=(pkg-config)
pkg-config --exists gtk4 2>/dev/null || MISSING_PKGS+=(libgtk-4-dev)
pkg-config --exists libadwaita-1 2>/dev/null || MISSING_PKGS+=(libadwaita-1-dev)

if [ ${#MISSING_PKGS[@]} -gt 0 ]; then
    warn "не хватает: ${MISSING_PKGS[*]}"
    if command -v apt-get >/dev/null; then
        echo "Устанавливаю (нужен пароль sudo)..."
        sudo apt-get update -qq
        sudo apt-get install -y --no-install-recommends \
            build-essential pkg-config libgtk-4-dev libadwaita-1-dev \
            ${MISSING_PKGS[@]/#/}
    else
        echo "Установи вручную: ${MISSING_PKGS[*]}" >&2
        exit 1
    fi
fi
ok "зависимости на месте"

# -------------------------------------------------------------------- сборка
if [ "$DO_BUILD" = 1 ]; then
    say "Собираю Rust (release)"
    ( cd "$ROOT" && cargo build --release -p pearl-minerd )
    ( cd "$ROOT" && cargo build --release -p pearl-miner --features gui )
fi

for bin in pearl-minerd pearl-miner; do
    [ -x "$ROOT/target/release/$bin" ] || { echo "Нет $ROOT/target/release/$bin — собери без --no-build" >&2; exit 1; }
done

say "Ставлю бинарники в $BIN_DIR"
mkdir -p "$BIN_DIR"
install -m755 "$ROOT/target/release/pearl-minerd" "$BIN_DIR/pearl-minerd"
install -m755 "$ROOT/target/release/pearl-miner" "$BIN_DIR/pearl-miner"
ok "pearl-miner и pearl-minerd установлены"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) warn "$BIN_DIR нет в PATH — добавь: export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac

# --------------------------------------------------------------------- polkit
say "Ставлю polkit-хелпер (нужен пароль sudo один раз)"
sudo install -m755 -o root -g root "$ROOT/polkit/pearl-oc" /usr/local/sbin/pearl-oc
sudo install -m644 -o root -g root "$ROOT/polkit/org.pearl.miner.policy" \
    /usr/share/polkit-1/actions/org.pearl.miner.policy
TMP_RULE="$(mktemp)"
sed "s/@USER@/$USER/g" "$ROOT/polkit/50-pearl-miner.rules.in" > "$TMP_RULE"
sudo install -m644 -o root -g root "$TMP_RULE" /etc/polkit-1/rules.d/50-pearl-miner.rules
rm -f "$TMP_RULE"
ok "разгон/питание теперь применяются без пароля (для $USER)"

# -------------------------------------------------------------------- systemd
say "Настраиваю фоновый сервис"
mkdir -p "$UNIT_DIR" "$DBUS_DIR"
install -m644 "$ROOT/systemd/pearl-minerd.service" "$UNIT_DIR/pearl-minerd.service"
install -m644 "$ROOT/systemd/pearl-miner-autostart.service" "$UNIT_DIR/pearl-miner-autostart.service"
install -m644 "$ROOT/systemd/org.pearl.Miner.Daemon.service" "$DBUS_DIR/org.pearl.Miner.Daemon.service"
systemctl --user daemon-reload
systemctl --user enable --now pearl-minerd.service
ok "pearl-minerd запущен: $(systemctl --user is-active pearl-minerd.service)"

# ------------------------------------------------------------- ярлык и иконка
say "Ставлю ярлык и иконку"
mkdir -p "$APP_DIR" "$ICON_DIR" "$SYM_ICON_DIR" "$METAINFO_DIR"
install -m644 "$ROOT/data/org.pearl.Miner.desktop" "$APP_DIR/org.pearl.Miner.desktop"
install -m644 "$ROOT/data/org.pearl.Miner.metainfo.xml" "$METAINFO_DIR/org.pearl.Miner.metainfo.xml"
install -m644 "$ROOT/data/icons/org.pearl.Miner.svg" "$ICON_DIR/org.pearl.Miner.svg"
# символические иконки живут в отдельном каталоге hicolor — иначе GTK/Shell их не находит
install -m644 "$ROOT/data/icons/org.pearl.Miner-symbolic.svg" \
    "$SYM_ICON_DIR/org.pearl.Miner-symbolic.svg"
command -v update-desktop-database >/dev/null && update-desktop-database -q "$APP_DIR" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
ok "приложение появилось в меню"

# ----------------------------------------------------------------- расширение
if [ "$DO_EXTENSION" = 1 ]; then
    say "Устанавливаю расширение GNOME Shell"
    EXT_SRC="$ROOT/extension/pearl-miner@pearl"
    TMP_DIR="$(mktemp -d)"
    ( cd "$EXT_SRC" && gnome-extensions pack --force --extra-source=icons \
        --out-dir "$TMP_DIR" . >/dev/null )
    gnome-extensions install --force "$TMP_DIR/pearl-miner@pearl.shell-extension.zip" >/dev/null
    rm -rf "$TMP_DIR"
    gnome-extensions enable "pearl-miner@pearl" 2>/dev/null || true
    ok "расширение установлено"
    warn "на Wayland расширение появится в панели после перелогина"
fi

# --------------------------------------------------------------------- хоткей
if [ "$DO_HOTKEY" = 1 ]; then
    say "Добавляю горячую клавишу Ctrl+Alt+M"
    KEY_PATH="/org/gnome/settings-daemon/plugins/media-keys/custom-keybindings/pearl-miner/"
    CURRENT=$(gsettings get org.gnome.settings-daemon.plugins.media-keys custom-keybindings)
    case "$CURRENT" in
        *"$KEY_PATH"*) ;;
        *)
            if [ "$CURRENT" = "@as []" ] || [ "$CURRENT" = "[]" ]; then
                NEW="['$KEY_PATH']"
            else
                NEW="${CURRENT%]*}, '$KEY_PATH']"
            fi
            gsettings set org.gnome.settings-daemon.plugins.media-keys custom-keybindings "$NEW"
            ;;
    esac
    SCHEMA="org.gnome.settings-daemon.plugins.media-keys.custom-keybinding:$KEY_PATH"
    gsettings set "$SCHEMA" name 'Pearl Miner: старт/стоп'
    gsettings set "$SCHEMA" command "$BIN_DIR/pearl-miner toggle"
    gsettings set "$SCHEMA" binding '<Control><Alt>m'
    ok "Ctrl+Alt+M переключает майнер"
fi

say "Готово"
cat <<EOF
Проверка:
  pearl-miner status          # текстовый статус
  pearl-miner status --json   # то, что читает панель
  pearl-miner toggle          # старт/стоп

Приложение: меню «Pearl Miner» (или pearl-miner gui)
Панель: перелогинься (Wayland), затем кнопка с молнией в верхней панели.

Настройки: ~/.config/pearl-miner/config.json
Лог майнера: ~/.local/state/pearl-miner/miner.log
Сервис: systemctl --user status pearl-minerd
EOF
