#!/bin/bash
# Compact failure report for a failed e2e run. It is safe to run locally:
#
#   ./scripts/e2e-diagnostics.sh [.e2e-run.log]
#
# The full report is written to .e2e/diagnostics.txt and printed. On GitHub
# Actions it is also flattened into one ::error:: annotation (the artifact
# upload keeps the complete logs).
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$ROOT/.e2e"
LOG="${1:-$ROOT/.e2e-run.log}"
REPORT="$WORK/diagnostics.txt"
mkdir -p "$WORK" 2>/dev/null || true

section() { printf '\n== %s\n' "$*"; }
tail_file() {
    local file="$1" lines="${2:-40}"
    [ -f "$file" ] || return 0
    section "$file"
    tail -"$lines" "$file"
}

{
    section "environment"
    date -u '+%Y-%m-%dT%H:%M:%SZ'
    uname -a
    echo "cargo: $(cargo --version 2>/dev/null || echo missing)"
    echo "rustc: $(rustc --version 2>/dev/null || echo missing)"

    section "processes"
    pgrep -af 'trommeld|/trommel($| )' 2>/dev/null || echo "no trommel process"
    pgrep -af 'wildrig-multi|my-custom-rig|external-rig|flaky-miner' 2>/dev/null \
        || echo "no miner process"

    section "harness artifacts"
    shopt -s nullglob
    for file in "$WORK"/*.out; do
        tail_file "$file" 40
    done
    shopt -u nullglob
    tail_file "$WORK/state/trommel/daemon.log" 80
    tail_file "$WORK/state/trommel/miner.log" 40
    tail_file "$WORK/oc.log" 40
    for file in "$WORK/state/trommel/state.json" "$WORK/state/trommel/oc-snapshot.json" \
        "$WORK/diagnostics.json" "$WORK/miner-args.txt"; do
        tail_file "$file" 60
    done

    section "failures"
    if [ -f "$LOG" ]; then
        grep -a '✗' "$LOG" || echo "no ✗ line in $LOG"
    else
        echo "missing $LOG"
    fi
    section "run log tail"
    tail_file "$LOG" 60
} > "$REPORT"

cat "$REPORT"
if [ "${GITHUB_ACTIONS:-}" = "true" ]; then
    echo "::error::$(tr '\n' ' ' < "$REPORT" | cut -c1-12000)"
fi
exit 0
