#!/bin/bash
# Fail if tracked files look like they contain a real wallet, a secret or a
# developer's personal home path.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
status=0

WALLETS="$(mktemp)"
HOME_HITS="$(mktemp)"
trap 'rm -f "$WALLETS" "$HOME_HITS"' EXIT

# Real-looking Pearl addresses: prl1 + at least 50 base32-ish chars.
if git grep -n -E 'prl1[0-9a-z]{50,}' -- ':!local' >"$WALLETS" 2>/dev/null; then
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
# Filter every match, not every line: one line can name both the placeholder
# and a real account, and the real one must still fail the check.
if git grep -n -E '/home/[A-Za-z0-9._-]+' \
    -- ':!local' ':!scripts/check-no-secrets.sh' >"$HOME_HITS" 2>/dev/null; then
    personal=0
    while IFS= read -r hit; do
        if printf '%s\n' "$hit" \
            | grep -oE '/home/[A-Za-z0-9._-]+' \
            | grep -qvx '/home/user'; then
            printf '%s\n' "$hit" >&2
            personal=1
        fi
    done < "$HOME_HITS"
    if [ "$personal" -ne 0 ]; then
        echo "error: personal home path in tracked files (use /home/user)" >&2
        status=1
    fi
fi

# Binaries/miners must never be tracked.
if git ls-files | grep -E '(^|/)(wildrig-multi|target/|[^/]*\.(deb|rpm|AppImage)$)' ; then
    echo "error: binary/build artifact is tracked" >&2
    status=1
fi

[ "$status" -eq 0 ] && echo "no secrets, personal paths or bundled binaries found"
exit "$status"
