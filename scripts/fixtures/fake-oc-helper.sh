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
lease_pid=""
lease_start=""
while [ $# -gt 0 ]; do
    case "$1" in
        --gpu) gpu="$2"; shift 2 ;;
        --pl) pl="$2"; shift 2 ;;
        --lgc) lgc="$2"; shift 2 ;;
        --lmc) lmc="$2"; shift 2 ;;
        --lease-pid) lease_pid="$2"; shift 2 ;;
        --lease-start) lease_start="$2"; shift 2 ;;
        *) shift ;;
    esac
done
[ -n "$gpu" ] || { echo "fake helper: --gpu is required" >&2; exit 2; }

# Mirror the real helper's lease check: refuse to tune a rig whose daemon is
# gone. The e2e harness relies on this to prove a late apply can not overwrite
# an emergency restore.
check_lease() {
    [ -z "$lease_pid" ] && [ -z "$lease_start" ] && return 0
    [ -n "$lease_pid" ] && [ -n "$lease_start" ] \
        || { echo "fake helper: lease flags must be passed together" >&2; exit 3; }
    [[ "$lease_pid" =~ ^[0-9]{1,7}$ ]] \
        || { echo "fake helper: invalid --lease-pid" >&2; exit 3; }
    [[ "$lease_start" =~ ^[0-9]{1,18}$ ]] \
        || { echo "fake helper: invalid --lease-start" >&2; exit 3; }
    [ -r "/proc/$lease_pid/stat" ] \
        || { echo "fake helper: lease process is gone" >&2; exit 3; }
    local stat rest start
    stat=$(<"/proc/$lease_pid/stat")
    rest=${stat##*') '}
    start=$(echo "$rest" | awk '{print $20}')
    [ "$start" = "$lease_start" ] \
        || { echo "fake helper: lease process was replaced" >&2; exit 3; }
}
check_lease

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
