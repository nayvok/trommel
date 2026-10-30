#!/bin/bash
# Verify that the workspace version is consistent across the release files and,
# when a tag is supplied (or the build runs from a tag ref), that the annotated
# tag names it and points at HEAD.
#
# Usage: ./scripts/check-version.sh [vX.Y.Z]
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
TAG="${1:-}"
# GITHUB_REF_NAME is the branch name on a push and "123/merge" on a pull
# request; only a tag build may be checked against a tag.
if [ -z "$TAG" ] && [ -n "${GITHUB_REF:-}" ]; then
    case "$GITHUB_REF" in
        refs/tags/*)
            TAG="${GITHUB_REF_NAME:?GITHUB_REF_NAME is required on a tag build}"
            ;;
    esac
fi
VERSION="$(cargo metadata --no-deps --format-version 1 --locked | python3 -c 'import json,sys; print(json.load(sys.stdin)["packages"][0]["version"])')"
FAILS=0
fail() { echo "error: $*" >&2; FAILS=$((FAILS + 1)); }

[ -n "$VERSION" ] || { echo "error: cannot read the workspace version" >&2; exit 1; }

grep -q "^version = \"$VERSION\"$" Cargo.toml \
    || fail "Cargo.toml workspace version is not $VERSION"

python3 - "$VERSION" <<'PY' || fail "Cargo.lock workspace versions do not match $VERSION"
import re
import sys

version = sys.argv[1]
text = open("Cargo.lock", encoding="utf-8").read()
for name in ("trommel-core", "trommeld", "trommel"):
    match = re.search(
        r'\[\[package\]\]\nname = "%s"\nversion = "([^"]+)"' % re.escape(name),
        text,
    )
    if match is None or match.group(1) != version:
        found = match.group(1) if match else "missing"
        print(f"Cargo.lock: {name} is {found}, expected {version}", file=sys.stderr)
        sys.exit(1)
PY

grep -q "^## \[$VERSION\] - [0-9]\{4\}-[0-9]\{2\}-[0-9]\{2\}$" CHANGELOG.md \
    || fail "CHANGELOG.md has no dated '## [$VERSION]' heading"

python3 - "$VERSION" <<'PY' || fail "metainfo.xml does not start with a $VERSION release"
import re
import sys

version = sys.argv[1]
text = open("data/io.github.nayvok.Trommel.metainfo.xml", encoding="utf-8").read()
match = re.search(r'<release version="([^"]+)"', text)
if match is None or match.group(1) != version:
    found = match.group(1) if match else "missing"
    print(f"metainfo.xml: newest release is {found}, expected {version}", file=sys.stderr)
    sys.exit(1)
PY

if [ -n "$TAG" ]; then
    [ "$TAG" = "v$VERSION" ] || fail "tag '$TAG' does not name v$VERSION"
    if git rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
        # A lightweight tag has no release notes and no tagger; the release
        # checklist requires an annotated tag.
        [ "$(git cat-file -t "refs/tags/$TAG")" = "tag" ] \
            || fail "tag $TAG is not annotated (use git tag -a)"
        [ "$(git rev-parse "$TAG^{commit}")" = "$(git rev-parse HEAD)" ] \
            || fail "tag $TAG is not on the current HEAD"
    else
        echo "note: tag $TAG is not available locally; skipping the HEAD comparison"
    fi
fi

if [ "$FAILS" -ne 0 ]; then
    echo "version check failed" >&2
    exit 1
fi
echo "version check ok: $VERSION${TAG:+ (tag $TAG)}"
