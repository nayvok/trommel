#!/bin/bash
# Unit tests for polkit/trommel-oc.
#
# The installed helper must be root and uses /usr/bin/nvidia-smi, so the test
# runs a copy with those two lines adjusted to the fake GPU fixture. The copy
# is temporary and never installed.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
FAILS=0

pass() { printf '\033[1;32m  ✓ %s\033[0m\n' "$*"; }
fail() { printf '\033[1;31m  ✗ %s\033[0m\n' "$*"; FAILS=$((FAILS + 1)); }
cleanup() { rm -rf "$WORK"; }
trap cleanup EXIT

HELPER="$WORK/trommel-oc"
sed -e "s#^NVIDIA_SMI=.*#NVIDIA_SMI=$ROOT/scripts/fixtures/fake-nvidia-smi.py#" \
    -e '/\[ "$(id -u)" -eq 0 \]/d' \
    "$ROOT/polkit/trommel-oc" > "$HELPER"
chmod +x "$HELPER"

# One fake card; reset to this exact state before every case.
reset_state() {
    cat > "$WORK/state.json" <<'JSON'
{"gpus": [{"index": 0, "uuid": "GPU-12345678-1234-1234-1234-123456789abc",
  "name": "Fake RTX 3080 Ti", "temp": 60, "fan": 65, "draw": 300.0,
  "power": 350.0, "power_default": 350.0, "power_min": 100.0, "power_max": 400.0,
  "core": 1770, "core_default": 1770, "mem": 9501, "mem_default": 9501,
  "util": 100, "vram": 1000}]}
JSON
    : > "$WORK/mode"
    export TROMMEL_FAKE_GPU_STATE="$WORK/state.json"
    export TROMMEL_FAKE_MODE="$WORK/mode"
}

query() {
    "$ROOT/scripts/fixtures/fake-nvidia-smi.py" --query-gpu "$1" --format csv,noheader,nounits -i "$UUID"
}

UUID="GPU-12345678-1234-1234-1234-123456789abc"

reset_state
if "$HELPER" apply --gpu "$UUID" --pl 300 --lgc 1620 --lmc 810 >/dev/null 2>"$WORK/err"; then
    if [ "$(query power.limit)" = "300.00" ] && [ "$(query clocks.sm)" = "1620" ] && [ "$(query clocks.mem)" = "810" ]; then
        pass "apply writes power and clock locks"
    else
        fail "apply reported success but the fake GPU did not change"
    fi
else
    fail "apply failed: $(cat "$WORK/err")"
fi

reset_state
echo ignore_pl > "$WORK/mode"
if "$HELPER" apply --gpu "$UUID" --pl 300 >/dev/null 2>"$WORK/err"; then
    fail "apply accepted a power limit the driver ignored"
else
    grep -q "did not take effect" "$WORK/err" && pass "ignored power limit is rejected" \
        || fail "wrong error for an ignored power limit: $(cat "$WORK/err")"
fi

reset_state
echo ignore_lmc > "$WORK/mode"
if "$HELPER" apply --gpu "$UUID" --lmc 810 >/dev/null 2>"$WORK/err"; then
    pass "apply accepts a memory lock the driver may ignore (checked under load)"
else
    fail "apply rejected the command before the runtime check: $(cat "$WORK/err")"
fi

reset_state
if "$HELPER" reset --gpu "$UUID" --pl 350 >/dev/null 2>"$WORK/err"; then
    if [ "$(query power.limit)" = "350.00" ] && [ "$(query clocks.sm)" = "1770" ] && [ "$(query clocks.mem)" = "9501" ]; then
        pass "reset restores power and unlocks clocks"
    else
        fail "reset reported success but the fake GPU was not restored"
    fi
else
    fail "reset failed: $(cat "$WORK/err")"
fi

reset_state
echo fail_rgc > "$WORK/mode"
if "$HELPER" reset --gpu "$UUID" --pl 350 >/dev/null 2>"$WORK/err"; then
    fail "reset accepted a failing -rgc"
else
    grep -q "rgc" "$WORK/err" && pass "failed clock reset is reported" \
        || fail "wrong error for a failed clock reset: $(cat "$WORK/err")"
fi

reset_state
"$ROOT/scripts/fixtures/fake-nvidia-smi.py" -i "$UUID" -pl 300 >/dev/null
echo ignore_pl > "$WORK/mode"
if "$HELPER" reset --gpu "$UUID" --pl 350 >/dev/null 2>"$WORK/err"; then
    fail "reset accepted a power limit the driver ignored"
else
    grep -q "did not take effect" "$WORK/err" && pass "reset verifies the power limit" \
        || fail "wrong error for an ignored reset power limit: $(cat "$WORK/err")"
fi

reset_state
if "$HELPER" apply --gpu "$UUID" --pl 5 >/dev/null 2>"$WORK/err"; then
    fail "apply accepted an out-of-range power limit"
else
    pass "out-of-range power is rejected"
fi
if "$HELPER" apply --gpu "nope" --pl 300 >/dev/null 2>"$WORK/err"; then
    fail "apply accepted an invalid UUID"
else
    pass "invalid UUID is rejected"
fi
if "$HELPER" apply --gpu "$UUID" --lmc 12001 >/dev/null 2>"$WORK/err"; then
    fail "apply accepted a memory clock above the range"
else
    pass "memory clock range is enforced"
fi


# Lease validation: the helper must refuse to touch the GPU when the daemon
# that queued the change is gone or its PID was reused.
starttime() {
    local stat rest
    stat=$(<"/proc/$1/stat")
    rest=${stat##*') '}
    echo "$rest" | awk '{print $20}'
}

reset_state
sleep 30 &
LEASE_JOB=$!
LEASE_START=$(starttime "$LEASE_JOB")
if "$HELPER" apply --gpu "$UUID" --pl 300 --lease-pid "$LEASE_JOB" --lease-start "$LEASE_START" >/dev/null 2>"$WORK/err"; then
    [ "$(query power.limit)" = "300.00" ] && pass "a live lease allows the change" \
        || fail "live lease applied the wrong value"
else
    fail "live lease was rejected: $(cat "$WORK/err")"
fi

reset_state
if "$HELPER" apply --gpu "$UUID" --pl 300 --lease-pid "$LEASE_JOB" --lease-start "$((LEASE_START + 1))" >/dev/null 2>"$WORK/err"; then
    fail "apply accepted a replaced lease PID"
else
    grep -q "replaced" "$WORK/err" && pass "a replaced lease PID is rejected" \
        || fail "wrong error for a replaced lease PID: $(cat "$WORK/err")"
fi

reset_state
if "$HELPER" apply --gpu "$UUID" --pl 300 --lease-pid "$LEASE_JOB" >/dev/null 2>"$WORK/err"; then
    fail "apply accepted a lease without a start time"
else
    pass "an incomplete lease is rejected"
fi

reset_state
if "$HELPER" apply --gpu "$UUID" --pl 300 --lease-pid abc --lease-start 1 >/dev/null 2>"$WORK/err"; then
    fail "apply accepted a non-numeric lease PID"
else
    pass "a non-numeric lease PID is rejected"
fi

kill "$LEASE_JOB" 2>/dev/null || true
wait "$LEASE_JOB" 2>/dev/null || true

reset_state
if "$HELPER" reset --gpu "$UUID" --pl 350 --lease-pid 4000000 --lease-start 1 >/dev/null 2>"$WORK/err"; then
    fail "reset accepted a lease for a dead process"
else
    grep -q "gone" "$WORK/err" && pass "a dead lease process is rejected" \
        || fail "wrong error for a dead lease process: $(cat "$WORK/err")"
fi

exit "$FAILS"
