#!/bin/bash
# Build a Debian package in dist/ without extra packaging tools.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
APP_ID="io.github.nayvok.Trommel"
EXT_UUID="trommel@nayvok.github.io"
VERSION="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')"
ARCH="amd64"
STAGE="$ROOT/dist/${APP_ID}_${VERSION}_${ARCH}"
rm -rf "$STAGE"
mkdir -p "$STAGE/DEBIAN" \
    "$STAGE/usr/bin" \
    "$STAGE/usr/libexec/trommel" \
    "$STAGE/usr/share/applications" \
    "$STAGE/usr/share/metainfo" \
    "$STAGE/usr/share/icons/hicolor/scalable/apps" \
    "$STAGE/usr/share/icons/hicolor/symbolic/apps" \
    "$STAGE/usr/share/glib-2.0/schemas" \
    "$STAGE/usr/lib/systemd/user" \
    "$STAGE/usr/share/dbus-1/services" \
    "$STAGE/usr/share/polkit-1/actions" \
    "$STAGE/usr/share/polkit-1/rules.d" \
    "$STAGE/usr/share/trommel" \
    "$STAGE/usr/share/gnome-shell/extensions/$EXT_UUID"

cargo build --release -p trommeld
cargo build --release -p trommel --features gui

install -m755 target/release/trommel "$STAGE/usr/bin/trommel"
install -m755 target/release/trommeld "$STAGE/usr/bin/trommeld"
install -m755 polkit/trommel-oc "$STAGE/usr/libexec/trommel/trommel-oc"
install -m644 polkit/io.github.nayvok.trommel.policy "$STAGE/usr/share/polkit-1/actions/"
install -m644 packaging/deb/50-trommel.rules "$STAGE/usr/share/polkit-1/rules.d/50-trommel.rules"
install -m644 polkit/50-trommel.rules.in "$STAGE/usr/share/trommel/50-trommel.rules.in"
install -m644 systemd/trommeld.service "$STAGE/usr/lib/systemd/user/"
install -m644 systemd/trommel-autostart.service "$STAGE/usr/lib/systemd/user/"
install -m644 systemd/io.github.nayvok.Trommel.Daemon.service "$STAGE/usr/share/dbus-1/services/"
install -m644 data/$APP_ID.desktop "$STAGE/usr/share/applications/"
install -m644 data/$APP_ID.metainfo.xml "$STAGE/usr/share/metainfo/"
install -m644 data/icons/$APP_ID.svg "$STAGE/usr/share/icons/hicolor/scalable/apps/"
install -m644 data/icons/$APP_ID-symbolic.svg "$STAGE/usr/share/icons/hicolor/symbolic/apps/"
install -m644 data/schemas/org.gnome.shell.extensions.trommel.gschema.xml "$STAGE/usr/share/glib-2.0/schemas/"
install -m755 packaging/deb/postinst "$STAGE/DEBIAN/postinst"
install -m755 packaging/deb/prerm "$STAGE/DEBIAN/prerm"
install -m755 packaging/deb/postrm "$STAGE/DEBIAN/postrm"

TMP_EXT="$(mktemp -d)"
( cd "extension/$EXT_UUID" && gnome-extensions pack --force \
    --extra-source=icons --extra-source=LICENSE \
    --schema="$ROOT/data/schemas/org.gnome.shell.extensions.trommel.gschema.xml" \
    --out-dir "$TMP_EXT" . >/dev/null )
python3 -m zipfile -e "$TMP_EXT/$EXT_UUID.shell-extension.zip" \
    "$STAGE/usr/share/gnome-shell/extensions/$EXT_UUID"
rm -rf "$TMP_EXT"

cat > "$STAGE/DEBIAN/control" <<CONTROL
Package: trommel
Version: $VERSION
Section: utils
Priority: optional
Architecture: $ARCH
Maintainer: nayvok <yanyushkov@gmail.com>
Depends: libgtk-4-1, libadwaita-1-0, libglib2.0-0t64, libglib2.0-bin, pkexec | policykit-1, dbus-user-session, systemd
Recommends: nvidia-driver | nvidia-utils, gnome-shell
Suggests: gnome-shell-extension-manager
Description: NVIDIA mining rig manager with presets and miner catalogue
 Trommel is a GTK4/libadwaita control room for NVIDIA mining rigs. It shows
 live hashrate, temperature and power for every card, keeps reusable presets
 for different coins, pools and GPU profiles, downloads supported miners on
 demand, and restores the stock power limit and clocks when mining stops.
CONTROL

dpkg-deb --build --root-owner-group "$STAGE" "$ROOT/dist/trommel_${VERSION}_${ARCH}.deb"
(
    cd "$ROOT/dist"
    cp "trommel_${VERSION}_${ARCH}.deb" "trommel_${ARCH}.deb"
    sha256sum "trommel_${VERSION}_${ARCH}.deb" > "trommel_${VERSION}_${ARCH}.deb.sha256"
    sha256sum "trommel_${ARCH}.deb" > "trommel_${ARCH}.deb.sha256"
)
echo "built $ROOT/dist/trommel_${VERSION}_${ARCH}.deb"
echo "stable copy: $ROOT/dist/trommel_${ARCH}.deb"
