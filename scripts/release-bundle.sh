#!/bin/bash
# Build and verify the GitHub release assets locally:
#   dist/release/trommel_X.Y.Z_amd64.deb   versioned package
#   dist/release/trommel_amd64.deb         stable latest URL
#   dist/release/*.sha256                  sidecars
#   dist/release/trommel-install.sh        installer
#   dist/release/SHA256SUMS                the two debs + the installer
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
VERSION="$(cargo metadata --no-deps --format-version 1 --locked | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')"
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
    # The installer is executed by users too, so it belongs in the sums even
    # though it cannot contain its own digest.
    sha256sum *.deb trommel-install.sh > SHA256SUMS
    ls -l
)
./scripts/verify-release-bundle.sh "$ROOT/dist/release"
echo "release bundle: $ROOT/dist/release"
