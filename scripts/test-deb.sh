#!/bin/bash
# Basic layout validation for the built Debian package.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="${TROMMEL_VERSION:-$(cd "$ROOT" && cargo metadata --no-deps --format-version 1 2>/dev/null | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])' 2>/dev/null || echo 0.0.0)}"
DEB="${1:-$ROOT/dist/trommel_${VERSION}_amd64.deb}"
[ -f "$DEB" ] || { echo "missing $DEB; run scripts/package-deb.sh" >&2; exit 1; }
LISTING="$(dpkg-deb -c "$DEB")"
for path in \
  ./usr/bin/trommel \
  ./usr/bin/trommeld \
  ./usr/libexec/trommel/trommel-oc \
  ./usr/share/applications/io.github.nayvok.Trommel.desktop \
  ./usr/share/metainfo/io.github.nayvok.Trommel.metainfo.xml \
  ./usr/share/glib-2.0/schemas/org.gnome.shell.extensions.trommel.gschema.xml \
  ./usr/share/gnome-shell/extensions/trommel@nayvok.github.io/metadata.json \
  ./usr/share/polkit-1/actions/io.github.nayvok.trommel.policy \
  ./usr/lib/systemd/user/trommeld.service \
  ./usr/share/dbus-1/services/io.github.nayvok.Trommel.Daemon.service
do
  grep -q "${path#./}" <<< "$LISTING" || {
    echo "missing in package: $path" >&2
    exit 1
  }
done
dpkg-deb -I "$DEB" | grep -q '^ Package: trommel$'
echo "package layout ok: $DEB"
