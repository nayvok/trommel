#!/bin/bash
# Install the root GPU helper, its policy and the per-user polkit rule.
#
# Run through sudo as the desktop user:
#   sudo ./scripts/install-polkit-rule.sh
# or let install.sh call it:
#   sudo ./scripts/install-polkit-rule.sh "$USER"
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET="${SUDO_USER:-${1:-}}"

if [ "$(id -u)" -ne 0 ]; then
    echo "run through sudo: sudo $0 [user]" >&2
    exit 1
fi
case "$TARGET" in
    ""|root|*[!a-zA-Z0-9._-]*)
        echo "usage: sudo $0 <desktop-user>" >&2
        echo "could not determine a normal user from SUDO_USER or the first argument" >&2
        exit 2
        ;;
esac
if ! getent passwd "$TARGET" >/dev/null 2>&1; then
    echo "unknown user: $TARGET" >&2
    exit 2
fi

install -d -m755 /usr/libexec/trommel
install -m755 -o root -g root "$ROOT/polkit/trommel-oc" /usr/libexec/trommel/trommel-oc
install -m644 -o root -g root \
    "$ROOT/polkit/io.github.nayvok.trommel.policy" \
    /usr/share/polkit-1/actions/io.github.nayvok.trommel.policy

install -d -m750 -o root -g polkitd /etc/polkit-1/rules.d 2>/dev/null || true
TMP_RULE="$(mktemp)"
sed "s/@USER@/$TARGET/g" "$ROOT/polkit/50-trommel.rules.in" > "$TMP_RULE"
install -m644 -o root -g root "$TMP_RULE" /etc/polkit-1/rules.d/50-trommel.rules
rm -f "$TMP_RULE"

grep -q "io.github.nayvok.trommel.oc" /etc/polkit-1/rules.d/50-trommel.rules
echo "installed the GPU helper and polkit rule for user $TARGET"
