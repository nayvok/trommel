#!/bin/bash
# Fail if tracked files look like they contain a real wallet or secret.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
status=0

# Real-looking Pearl addresses: prl1 + at least 50 base32-ish chars.
if git grep -n -E 'prl1[0-9a-z]{50,}' -- ':!local' ':!*.md' >/tmp/trommel-wallets 2>/dev/null; then
    if grep -v 'prl1test' /tmp/trommel-wallets; then
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

# Binaries/miners must never be tracked.
if git ls-files | grep -E '(^|/)(wildrig-multi|\.deb|\.rpm|\.AppImage|target/)' ; then
    echo "error: binary/build artifact is tracked" >&2
    status=1
fi

[ "$status" -eq 0 ] && echo "no secrets or bundled binaries found"
exit "$status"
