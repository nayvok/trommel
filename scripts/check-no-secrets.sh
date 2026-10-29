#!/bin/bash
# Fail if tracked files look like they contain a real wallet, a secret or a
# developer's personal home path.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
status=0

WALLETS="$(mktemp)"
trap 'rm -f "$WALLETS"' EXIT

# Real-looking Pearl addresses: prl1 + at least 50 base32-ish chars.
if git grep -n -E 'prl1[0-9a-z]{50,}' -- ':!local' ':!*.md' >"$WALLETS" 2>/dev/null; then
    if grep -v 'prl1test' "$WALLETS"; then
        echo "error: real-looking wallet address in tracked files" >&2
        status=1
    fi
fi

# Obvious secret markers.
if git grep -n -i -E '(seed phrase|mnemonic|private key|BEGIN (OPENSSH|RSA|EC) PRIVATE KEY)' \
    -- ':!local' ':!*.md' ':!docs' ':!scripts/check-no-secrets.sh' ; then
    echo "error: secret-like text in tracked files" >&2
    status=1
fi

# Personal home directories leak the developer's account into sources, docs and
# built binaries. /home/user is the neutral placeholder used by test fixtures.
if git grep -n -E '/home/[A-Za-z0-9._-]+' \
    -- ':!local' ':!scripts/check-no-secrets.sh' \
    | grep -vE '/home/user([^A-Za-z0-9._-]|$)'; then
    echo "error: personal home path in tracked files (use /home/user)" >&2
    status=1
fi

# Binaries/miners must never be tracked.
if git ls-files | grep -E '(^|/)(wildrig-multi|\.deb|\.rpm|\.AppImage|target/)' ; then
    echo "error: binary/build artifact is tracked" >&2
    status=1
fi

[ "$status" -eq 0 ] && echo "no secrets, personal paths or bundled binaries found"
exit "$status"
