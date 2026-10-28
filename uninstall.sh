#!/bin/bash
# Remove a Trommel developer install. Config is kept unless --purge.
set -euo pipefail

PURGE=0
[ "${1:-}" = "--purge" ] && PURGE=1
APP_ID="io.github.nayvok.Trommel"
EXT_UUID="trommel@nayvok.github.io"

say() { printf '\n\033[1;34m==> %s\033[0m\n' "$*"; }

say "Stopping mining"
if command -v trommel >/dev/null; then
    if ! trommel stop; then
        echo "trommel stop failed (stock restore?); refusing to remove the helper." >&2
        echo "Fix the GPU state first, then rerun uninstall." >&2
        exit 1
    fi
    if ! trommel status --json | python3 -c 'import json,sys; s=json.load(sys.stdin); sys.exit(0 if not s["state"]["running"] else 1)'; then
        echo "the miner is still running; refusing to remove the helper." >&2
        exit 1
    fi
else
    echo "trommel is not in PATH; skipping the automatic stop"
fi

say "Removing user service"
systemctl --user disable --now trommel-autostart.service 2>/dev/null || true
systemctl --user disable --now trommeld.service 2>/dev/null || true
rm -f "$HOME/.config/systemd/user/trommeld.service" \
      "$HOME/.config/systemd/user/trommel-autostart.service" \
      "$HOME/.local/share/dbus-1/services/io.github.nayvok.Trommel.Daemon.service"
systemctl --user daemon-reload 2>/dev/null || true

say "Removing the legacy GNOME Shell applet (if installed)"
gnome-extensions disable "$EXT_UUID" 2>/dev/null || true
gnome-extensions uninstall "$EXT_UUID" 2>/dev/null || true

say "Removing menu entry, icons and metadata"
rm -f "$HOME/.local/share/applications/$APP_ID.desktop" \
      "$HOME/.local/share/metainfo/$APP_ID.metainfo.xml" \
      "$HOME/.local/share/icons/hicolor/scalable/apps/$APP_ID.svg" \
      "$HOME/.local/share/icons/hicolor/symbolic/apps/$APP_ID-symbolic.svg" \
      "$HOME/.local/share/glib-2.0/schemas/org.gnome.shell.extensions.trommel.gschema.xml"
glib-compile-schemas "$HOME/.local/share/glib-2.0/schemas" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q -t "$HOME/.local/share/icons/hicolor" 2>/dev/null || true

say "Removing binaries"
rm -f "$HOME/.local/bin/trommel" "$HOME/.local/bin/trommeld"

say "Removing root helper and polkit files (sudo)"
sudo rm -f /usr/libexec/trommel/trommel-oc \
           /usr/share/polkit-1/actions/io.github.nayvok.trommel.policy \
           /etc/polkit-1/rules.d/50-trommel.rules
sudo rmdir /usr/libexec/trommel 2>/dev/null || true

if [ "$PURGE" = 1 ]; then
    say "Removing config, state and benchmarks"
    rm -rf "$HOME/.config/trommel" "$HOME/.local/state/trommel" "$HOME/.cache/trommel"
fi

say "Done"
[ "$PURGE" = 0 ] && echo "Config kept: ~/.config/trommel/config.json"
