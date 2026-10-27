#!/bin/bash
# Render hicolor PNGs from the source SVG when rsvg-convert is available.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$ROOT/data/icons/src/trommel.svg"
[ -f "$SRC" ] || { echo "missing $SRC" >&2; exit 1; }
if ! command -v rsvg-convert >/dev/null 2>&1; then
    echo "rsvg-convert is not installed; SVG-only install is still valid" >&2
    exit 0
fi
for size in 16 24 32 48 64 128 256 512; do
    dir="$ROOT/data/icons/hicolor/${size}x${size}/apps"
    mkdir -p "$dir"
    rsvg-convert -w "$size" -h "$size" "$SRC" -o "$dir/io.github.nayvok.Trommel.png"
done
echo "rendered hicolor PNGs"
