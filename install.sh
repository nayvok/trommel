#!/bin/bash
# Trommel developer install (per-user binaries + one root helper).
#
#   ./install.sh             # build, install and enable the user service
#   ./install.sh --no-build  # use target/release
#   ./install.sh --no-helper # skip the polkit helper (OC disabled)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
UNIT_DIR="$HOME/.config/systemd/user"
DBUS_DIR="$HOME/.local/share/dbus-1/services"
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
SYM_ICON_DIR="$HOME/.local/share/icons/hicolor/symbolic/apps"
METAINFO_DIR="$HOME/.local/share/metainfo"
HELPER_DIR="/usr/libexec/trommel"
APP_ID="io.github.nayvok.Trommel"

DO_BUILD=1
DO_HELPER=1
for arg in "$@"; do
    case $arg in
        --no-build) DO_BUILD=0 ;;
        --no-helper) DO_HELPER=0 ;;
        --no-extension)
            echo "  ! --no-extension is obsolete: the GNOME Shell applet was removed" >&2
            ;;
        -h|--help) sed -n '2,6p' "$0"; exit 0 ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

say() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[1;33m  ! %s\033[0m\n' "$*"; }
ok() { printf '\033[1;32m  ✓ %s\033[0m\n' "$*"; }

say "Checking build dependencies"
MISSING=()
command -v cargo >/dev/null || MISSING+=(cargo)
command -v pkg-config >/dev/null || MISSING+=(pkg-config)
pkg-config --exists gtk4 2>/dev/null || MISSING+=(libgtk-4-dev)
pkg-config --exists libadwaita-1 2>/dev/null || MISSING+=(libadwaita-1-dev)
if [ ${#MISSING[@]} -gt 0 ]; then
    warn "missing: ${MISSING[*]}"
    if command -v apt-get >/dev/null; then
        sudo apt-get update -qq
        sudo apt-get install -y --no-install-recommends \
            build-essential pkg-config libgtk-4-dev libadwaita-1-dev "${MISSING[@]}"
    else
        echo "Install manually: ${MISSING[*]}" >&2
        exit 1
    fi
fi
ok "dependencies present"

if [ "$DO_BUILD" = 1 ]; then
    say "Building release binaries"
    ( cd "$ROOT" && cargo build --release -p trommeld )
    ( cd "$ROOT" && cargo build --release -p trommel --features gui )
fi
for bin in trommeld trommel; do
    [ -x "$ROOT/target/release/$bin" ] || {
        echo "missing $ROOT/target/release/$bin; build without --no-build" >&2
        exit 1
    }
done

say "Installing binaries to $BIN_DIR"
mkdir -p "$BIN_DIR"
install -m755 "$ROOT/target/release/trommeld" "$BIN_DIR/trommeld"
install -m755 "$ROOT/target/release/trommel" "$BIN_DIR/trommel"
ok "trommel and trommeld installed"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *) warn "$BIN_DIR is not in PATH; add: export PATH=\"\$HOME/.local/bin:\$PATH\"" ;;
esac

if [ "$DO_HELPER" = 1 ]; then
    say "Installing the root GPU helper (sudo once)"
    sudo "$ROOT/scripts/install-polkit-rule.sh" "$USER"
    ok "power/clocks can be applied without a password for $USER"
fi

say "Installing the user service"
mkdir -p "$UNIT_DIR" "$DBUS_DIR"
sed 's#ExecStart=/usr/bin/trommeld#ExecStart=%h/.local/bin/trommeld#' \
    "$ROOT/systemd/trommeld.service" > "$UNIT_DIR/trommeld.service"
sed 's#/usr/bin/trommel#%h/.local/bin/trommel#g' \
    "$ROOT/systemd/trommel-autostart.service" > "$UNIT_DIR/trommel-autostart.service"
install -m644 "$ROOT/systemd/io.github.nayvok.Trommel.Daemon.service" \
    "$DBUS_DIR/io.github.nayvok.Trommel.Daemon.service"
systemctl --user daemon-reload
systemctl --user enable trommeld.service
systemctl --user restart trommeld.service
ok "trommeld is $([ "$(systemctl --user is-active trommeld.service)" = active ] && echo active || echo inactive)"

say "Installing desktop entry, icons and metadata"
mkdir -p "$APP_DIR" "$ICON_DIR" "$SYM_ICON_DIR" "$METAINFO_DIR"
install -m644 "$ROOT/data/$APP_ID.desktop" "$APP_DIR/$APP_ID.desktop"
install -m644 "$ROOT/data/$APP_ID.metainfo.xml" "$METAINFO_DIR/$APP_ID.metainfo.xml"
install -m644 "$ROOT/data/icons/$APP_ID.svg" "$ICON_DIR/$APP_ID.svg"
install -m644 "$ROOT/data/icons/$APP_ID-symbolic.svg" "$SYM_ICON_DIR/$APP_ID-symbolic.svg"
command -v update-desktop-database >/dev/null && update-desktop-database -q "$APP_DIR" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
ok "application menu entry installed"

say "Done"
cat <<EOF
Check:
  trommel status
  trommel status --json
  trommel start / stop / toggle

App: application menu "Trommel" (or trommel gui)
Config: ~/.config/trommel/config.json
Log: ~/.local/state/trommel/miner.log
Service: systemctl --user status trommeld

EOF
