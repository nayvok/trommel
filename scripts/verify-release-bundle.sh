#!/bin/bash
# Verify a local or downloaded release bundle.
#
# Expected files (the same names the release workflow publishes):
#   trommel_X.Y.Z_amd64.deb        versioned package
#   trommel_amd64.deb              stable alias
#   trommel_X.Y.Z_amd64.deb.sha256 sidecar
#   trommel_amd64.deb.sha256       sidecar
#   trommel-install.sh             release installer
#   SHA256SUMS                     checksums for the two debs and the installer
#
# Usage: ./scripts/verify-release-bundle.sh [dist/release]
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
DIR="${1:-$ROOT/dist/release}"
VERSION="$(cargo metadata --no-deps --format-version 1 --locked | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')"
FAILS=0
fail() { echo "error: $*" >&2; FAILS=$((FAILS + 1)); }

[ -d "$DIR" ] || { echo "missing bundle directory: $DIR" >&2; exit 1; }

VERSIONED="trommel_${VERSION}_amd64.deb"
ALIAS="trommel_amd64.deb"
INSTALLER="trommel-install.sh"
EXPECTED=("$VERSIONED" "$ALIAS" "${VERSIONED}.sha256" "${ALIAS}.sha256" "$INSTALLER" "SHA256SUMS")

for name in "${EXPECTED[@]}"; do
    [ -f "$DIR/$name" ] || fail "missing asset: $name"
done
if [ "$FAILS" -ne 0 ]; then
    echo "release bundle check failed: $DIR" >&2
    exit 1
fi

# Nothing else may be published by accident.
shopt -s nullglob
for path in "$DIR"/*; do
    name="$(basename "$path")"
    found=0
    for expected in "${EXPECTED[@]}"; do
        [ "$name" = "$expected" ] && found=1
    done
    [ "$found" -eq 1 ] || fail "unexpected asset in the bundle: $name"
done

( cd "$DIR" && sha256sum -c SHA256SUMS ) || fail "SHA256SUMS does not verify"
( cd "$DIR" && sha256sum -c "${VERSIONED}.sha256" ) || fail "${VERSIONED}.sha256 does not verify"
( cd "$DIR" && sha256sum -c "${ALIAS}.sha256" ) || fail "${ALIAS}.sha256 does not verify"
cmp -s "$DIR/$VERSIONED" "$DIR/$ALIAS" || fail "$ALIAS is not the versioned package"

python3 - "$DIR/SHA256SUMS" "$VERSION" <<'PY' || fail "SHA256SUMS does not cover exactly the debs and the installer"
import sys

version = sys.argv[2]
listed = set()
for line in open(sys.argv[1], encoding="utf-8"):
    line = line.strip()
    if not line:
        continue
    parts = line.split(None, 1)
    if len(parts) != 2 or len(parts[0]) != 64:
        print(f"SHA256SUMS: malformed line: {line}", file=sys.stderr)
        sys.exit(1)
    listed.add(parts[1].lstrip("*"))
expected = {f"trommel_{version}_amd64.deb", "trommel_amd64.deb", "trommel-install.sh"}
if listed != expected:
    print(f"SHA256SUMS lists {sorted(listed)}, expected {sorted(expected)}", file=sys.stderr)
    sys.exit(1)
PY

if [ "$FAILS" -ne 0 ]; then
    echo "release bundle check failed: $DIR" >&2
    exit 1
fi
echo "release bundle ok: $DIR ($VERSION)"
