#!/bin/bash
# First-run smoke test: no config, no miner, no wallet, one fake GPU.
#
# This is deliberately separate from ui-smoke.sh, which starts from a populated
# config. It answers the "what does a fresh download look like?" question:
# dashboard placeholders, Quick setup, Start opening the wizard, Miners showing
# "not installed" and the single-GPU tune dialog.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$ROOT/.first-run-check"
BIN="$ROOT/target/debug"
FAILS=0
GUI_PID=""
DAEMON_PID=""
BUS_PID=""

pass() { printf '\033[1;32m  ✓ %s\033[0m\n' "$*"; }
fail() { printf '\033[1;31m  ✗ %s\033[0m\n' "$*"; FAILS=$((FAILS + 1)); }
step() { printf '\n\033[1;34m== %s ==\033[0m\n' "$*"; }

cleanup() {
    [ -n "$GUI_PID" ] && kill "$GUI_PID" 2>/dev/null || true
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    [ -n "$BUS_PID" ] && kill "$BUS_PID" 2>/dev/null || true
}
trap cleanup EXIT

step "Build"
( cd "$ROOT" && cargo build -q -p trommel --features gui -p trommeld ) || {
    echo "build failed" >&2
    exit 1
}

step "Prepare pristine environment"
rm -rf "$WORK"
mkdir -p "$WORK/bin" "$WORK/config" "$WORK/state" "$WORK/data" "$WORK/cache"

cat > "$WORK/bin/nvidia-smi" <<'SH'
#!/bin/bash
args="$*"
case "$args" in
  *index,uuid*)
    echo "0, GPU-33333333-3333-3333-3333-333333333333, NVIDIA GeForce RTX 3080 Ti, 46, 30, 49.00, 350.00, 350.00, 100.00, 400.00, 210, 405, 0, 512"
    ;;
  *power.limit*) echo "350.00" ;;
  *) exit 0 ;;
esac
SH
chmod +x "$WORK/bin/nvidia-smi"

cat > "$WORK/bin/oc-helper" <<SH
#!/bin/bash
echo "\$*" >> "$WORK/oc.log"
exit 0
SH
chmod +x "$WORK/bin/oc-helper"

cat > "$WORK/bus.conf" <<XML
<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:dir=$WORK</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
  <apparmor mode="disabled"/>
</busconfig>
XML
BUS_OUT="$(dbus-daemon --config-file="$WORK/bus.conf" --print-address --print-pid --fork)"
export DBUS_SESSION_BUS_ADDRESS="$(printf '%s\n' "$BUS_OUT" | sed -n 1p)"
BUS_PID="$(printf '%s\n' "$BUS_OUT" | sed -n 2p)"

export PATH="$WORK/bin:$PATH"
export XDG_CONFIG_HOME="$WORK/config"
export XDG_STATE_HOME="$WORK/state"
export XDG_DATA_HOME="$WORK/data"
export XDG_CACHE_HOME="$WORK/cache"
export TROMMEL_OC_HELPER="$WORK/bin/oc-helper"
export TROMMEL_NO_EXTERNAL_SCAN=1
export TROMMEL_OC_LOG="$WORK/oc.log"
export GTK_A11Y=atspi
export LANG=C.UTF-8
export LANGUAGE=en
export AT_SPI_BUS_ADDRESS="${AT_SPI_BUS_ADDRESS:-unix:path=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/at-spi/bus}"
if [ ! -S "${AT_SPI_BUS_ADDRESS#unix:path=}" ]; then
    echo "AT-SPI bus not found at $AT_SPI_BUS_ADDRESS" >&2
    echo "run this script inside the graphical session" >&2
    exit 1
fi
pass "pristine XDG dirs and AT-SPI bus are ready"

step "Start daemon and GUI"
setsid "$BIN/trommeld" > "$WORK/daemon.out" 2>&1 < /dev/null &
DAEMON_PID=$!
sleep 2
if kill -0 "$DAEMON_PID" 2>/dev/null; then
    pass "trommeld starts without a config"
else
    fail "trommeld did not start"
    cat "$WORK/daemon.out"
    exit 1
fi

(
    unset TROMMEL_OPEN_PRESET TROMMEL_OPEN_WIZARD TROMMEL_OPEN_TUNE TROMMEL_OPEN_INSTALL TROMMEL_SCREENSHOT
    exec setsid "$BIN/trommel"
) > "$WORK/gui.out" 2>&1 < /dev/null &
GUI_PID=$!
sleep 2

step "First-run UI"
python3 "$ROOT/scripts/ui_smoke.py" --pid "$GUI_PID" --phase first-run
RC=$?
if [ "$RC" -eq 0 ]; then pass "first-run UI phase"; else fail "first-run UI phase"; fi

step "Logs"
if grep -q -E "panic|RUST_BACKTRACE" "$WORK/daemon.out" "$WORK/gui.out" 2>/dev/null; then
    fail "panic in first-run logs"
    grep -n -E "panic|RUST_BACKTRACE" "$WORK/daemon.out" "$WORK/gui.out" 2>/dev/null | tail -10
else
    pass "no panic in first-run logs"
fi

echo
if [ "$FAILS" -eq 0 ]; then
    printf '\033[1;32mFIRST RUN: ALL GREEN\033[0m\n'
    exit 0
else
    printf '\033[1;31mFIRST RUN FAILURES: %s\033[0m\n' "$FAILS"
    echo "logs: $WORK"
    exit 1
fi
