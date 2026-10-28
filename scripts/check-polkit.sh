#!/bin/bash
# Static check for the two things that break prompt-free GPU tuning:
# the polkit action id and the helper path.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$ROOT"
FAIL=0
fail() { echo "error: $*" >&2; FAIL=1; }
ok() { printf '  \033[1;32m✓\033[0m %s\n' "$*"; }

ACTION="io.github.nayvok.trommel.oc"
HELPER="/usr/libexec/trommel/trommel-oc"

grep -q "id=\"$ACTION\"" polkit/io.github.nayvok.trommel.policy \
    && ok "policy action id is $ACTION" \
    || fail "policy action id is not $ACTION"

grep -q "$ACTION" polkit/50-trommel.rules.in \
    && ok "dev rule matches the annotated action" \
    || fail "dev rule does not match $ACTION"

grep -q "$ACTION" packaging/deb/50-trommel.rules \
    && ok "deb rule matches the annotated action" \
    || fail "deb rule does not match $ACTION"

for rule in polkit/50-trommel.rules.in packaging/deb/50-trommel.rules; do
    if grep -q 'org.freedesktop.policykit.exec' "$rule"; then
        fail "$rule still matches org.freedesktop.policykit.exec (pkexec uses the annotated action)"
    fi
done

grep -q "$HELPER" polkit/io.github.nayvok.trommel.policy \
    && ok "policy points at $HELPER" \
    || fail "policy does not point at $HELPER"

[ -x polkit/trommel-oc ] && ok "helper is executable" || fail "helper is not executable"

exit "$FAIL"
