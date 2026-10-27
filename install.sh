#!/bin/bash
# Trommel developer install (per-user binaries + one root helper).
#
#   ./install.sh                 # build, install, enable service + extension
#   ./install.sh --no-build      # use target/release
#   ./install.sh --no-extension  # skip the GNOME Shell extension
#   ./install.sh --no-helper     # skip the polkit helper (OC disabled)
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
BIN_DIR="$HOME/.local/bin"
UNIT_DIR="$HOME/.config/systemd/user"
DBUS_DIR="$HOME/.local/share/dbus-1/services"
APP_DIR="$HOME/.local/share/applications"
ICON_DIR="$HOME/.local/share/icons/hicolor/scalable/apps"
SYM_ICON_DIR="$HOME/.local/share/icons/hicolor/symbolic/apps"
METAINFO_DIR="$HOME/.local/share/metainfo"
SCHEMA_DIR="$HOME/.local/share/glib-2.0/schemas"
HELPER_DIR="/usr/libexec/trommel"
APP_ID="io.github.nayvok.Trommel"
EXT_UUID="trommel@nayvok.github.io"

DO_BUILD=1
DO_EXTENSION=1
DO_HELPER=1
for arg in "$@"; do
    case $arg in
        --no-build) DO_BUILD=0 ;;
        --no-extension) DO_EXTENSION=0 ;;
        --no-helper) DO_HELPER=0 ;;
        -h|--help) sed -n '2,10p' "$0"; exit 0 ;;
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
    sudo install -d -m755 "$HELPER_DIR"
    sudo install -m755 -o root -g root "$ROOT/polkit/trommel-oc" "$HELPER_DIR/trommel-oc"
    sudo install -m644 -o root -g root \
        "$ROOT/polkit/io.github.nayvok.trommel.policy" \
        /usr/share/polkit-1/actions/io.github.nayvok.trommel.policy
    TMP_RULE="$(mktemp)"
    sed "s/@USER@/$USER/g" "$ROOT/polkit/50-trommel.rules.in" > "$TMP_RULE"
    sudo install -m644 -o root -g root "$TMP_RULE" /etc/polkit-1/rules.d/50-trommel.rules
    rm -f "$TMP_RULE"
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
systemctl --user enable --now trommeld.service
ok "trommeld is $([ "$(systemctl --user is-active trommeld.service)" = active ] && echo active || echo inactive)"

say "Installing desktop entry, icons and metadata"
mkdir -p "$APP_DIR" "$ICON_DIR" "$SYM_ICON_DIR" "$METAINFO_DIR" "$SCHEMA_DIR"
install -m644 "$ROOT/data/$APP_ID.desktop" "$APP_DIR/$APP_ID.desktop"
install -m644 "$ROOT/data/$APP_ID.metainfo.xml" "$METAINFO_DIR/$APP_ID.metainfo.xml"
install -m644 "$ROOT/data/icons/$APP_ID.svg" "$ICON_DIR/$APP_ID.svg"
install -m644 "$ROOT/data/icons/$APP_ID-symbolic.svg" "$SYM_ICON_DIR/$APP_ID-symbolic.svg"
install -m644 "$ROOT/data/schemas/org.gnome.shell.extensions.trommel.gschema.xml" "$SCHEMA_DIR/"
glib-compile-schemas --strict "$SCHEMA_DIR"
command -v update-desktop-database >/dev/null && update-desktop-database -q "$APP_DIR" || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true
ok "application menu entry installed"

if [ "$DO_EXTENSION" = 1 ]; then
    say "Installing the GNOME Shell extension"
    if ! command -v gnome-extensions >/dev/null; then
        warn "gnome-extensions is not available; skipping"
    else
        EXT_SRC="$ROOT/extension/$EXT_UUID"
        TMP_DIR="$(mktemp -d)"
        ( cd "$EXT_SRC" && gnome-extensions pack --force \
            --extra-source=icons --extra-source=LICENSE \
            --schema="$ROOT/data/schemas/org.gnome.shell.extensions.trommel.gschema.xml" \
            --out-dir "$TMP_DIR" . >/dev/null )
        gnome-extensions install --force "$TMP_DIR/$EXT_UUID.shell-extension.zip" >/dev/null
        rm -rf "$TMP_DIR"
        if gnome-extensions enable "$EXT_UUID"; then
            ok "extension enabled"
        else
            warn "gnome-extensions enable returned an error; see 'gnome-extensions info $EXT_UUID'"
        fi
        if gnome-extensions list --enabled | grep -qx "$EXT_UUID"; then
            ok "extension is enabled"
        else
            warn "extension is installed but not enabled; log out and back in, then check"
            gnome-extensions info "$EXT_UUID" || true
        fi
    fi
fi

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
