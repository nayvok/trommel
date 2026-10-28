#!/bin/bash
# Build the GitHub release assets locally:
#   dist/release/trommel_0.4.1_amd64.deb
#   dist/release/trommel_amd64.deb          (stable latest URL)
#   dist/release/*.sha256                   (basenames)
#   dist/release/trommel-install.sh
#   dist/release/SHA256SUMS
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
VERSION="$(cargo metadata --no-deps --format-version 1 | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')"
ARCH=amd64

./scripts/package-deb.sh >/dev/null
rm -rf dist/release
mkdir -p dist/release
cp "dist/trommel_${VERSION}_${ARCH}.deb" "dist/release/"
cp "dist/trommel_${ARCH}.deb" "dist/release/"
cp "dist/trommel_${VERSION}_${ARCH}.deb.sha256" "dist/release/"
cp "dist/trommel_${ARCH}.deb.sha256" "dist/release/"
cp packaging/github/trommel-install.sh "dist/release/"
(
    cd dist/release
    sha256sum *.deb > SHA256SUMS
    ls -l
)
echo "release bundle: $ROOT/dist/release"
