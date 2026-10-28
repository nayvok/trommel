#!/bin/bash
# Fake root helper for the Trommel e2e harness.
#
# It forwards the arguments to the stateful fake `nvidia-smi` and never needs
# root. The daemon uses it through TROMMEL_OC_HELPER. Keep the argument parsing
# in sync with polkit/trommel-oc.
set -euo pipefail

echo "$*" >> "${TROMMEL_OC_LOG:?TROMMEL_OC_LOG is required}"

cmd="${1:-}"
[ $# -gt 0 ] && shift
gpu=""
pl=""
lgc=""
lmc=""
while [ $# -gt 0 ]; do
    case "$1" in
        --gpu) gpu="$2"; shift 2 ;;
        --pl) pl="$2"; shift 2 ;;
        --lgc) lgc="$2"; shift 2 ;;
        --lmc) lmc="$2"; shift 2 ;;
        *) shift ;;
    esac
done
[ -n "$gpu" ] || { echo "fake helper: --gpu is required" >&2; exit 2; }

case "$cmd" in
    apply)
        [ -z "$pl" ] || nvidia-smi -i "$gpu" -pl "$pl"
        [ -z "$lgc" ] || nvidia-smi -i "$gpu" -lgc "$lgc"
        [ -z "$lmc" ] || nvidia-smi -i "$gpu" -lmc "$lmc"
        echo "apply ok pl=${pl:-none} lgc=${lgc:-none} lmc=${lmc:-none}"
        ;;
    reset)
        nvidia-smi -i "$gpu" -rgc
        nvidia-smi -i "$gpu" -rmc
        [ -z "$pl" ] || nvidia-smi -i "$gpu" -pl "$pl"
        echo "reset ok pl=${pl:-none}"
        ;;
    *)
        echo "fake helper: unknown command $cmd" >&2
        exit 2
        ;;
esac
