#!/bin/bash
# Render hicolor PNGs from the source SVGs when rsvg-convert is available.
#
# The checked-in PNGs are the released artwork. This script is a regeneration
# helper for contributors who change data/icons/src/trommel.svg or the colour
# tray icon; it writes into the same hicolor tree that install.sh and the
# Debian package install.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/data/icons/src/trommel.svg"
TRAY="$ROOT/data/icons/io.github.nayvok.Trommel-tray.svg"
[ -f "$SRC" ] || { echo "missing $SRC" >&2; exit 1; }
[ -f "$TRAY" ] || { echo "missing $TRAY" >&2; exit 1; }
if ! command -v rsvg-convert >/dev/null 2>&1; then
    echo "rsvg-convert is not installed; SVG-only install is still valid" >&2
    exit 0
fi

render() {
    local src="$1" size="$2" name="$3"
    local dir="$ROOT/data/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$dir"
    rsvg-convert -w "$size" -h "$size" "$src" -o "$dir/$name"
}

for size in 32 48 64 128 256 512; do
    render "$SRC" "$size" io.github.nayvok.Trommel.png
done
for size in 16 22 24 32; do
    render "$TRAY" "$size" io.github.nayvok.Trommel-tray.png
done
echo "rendered hicolor PNGs"
