#!/bin/bash
# First-run smoke test: no config, no miner, no wallet, one fake GPU.
#
# This is deliberately separate from ui-smoke.sh, which starts from a populated
# config. It answers the "what does a fresh download look like?" question:
# empty dashboard, Create preset, Start explaining the missing preset, the
# miner library showing "not installed" and the disabled tune dialog.
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
    [ -n "${ATSPI_REGISTRY_PID:-}" ] && kill "$ATSPI_REGISTRY_PID" 2>/dev/null || true
    [ -n "${ATSPI_BUS_PID:-}" ] && kill "$ATSPI_BUS_PID" 2>/dev/null || true
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

cat > "$WORK/gpu-state.json" <<'JSON'
{
  "gpus": [
    {"index": 0, "uuid": "GPU-33333333-3333-3333-3333-333333333333", "pci_bus_id": "00000000:01:00.0", "name": "NVIDIA GeForce RTX 3080 Ti",
     "temp": 46, "fan": 30, "draw": 49.0, "power": 350.0, "power_default": 350.0,
     "power_min": 100.0, "power_max": 400.0, "core": 210, "core_default": 1770,
     "mem": 405, "mem_default": 9501, "util": 0, "vram": 512}
  ]
}
JSON
: > "$WORK/fake-mode"
cp "$ROOT/scripts/fixtures/fake-nvidia-smi.py" "$WORK/bin/nvidia-smi"
cp "$ROOT/scripts/fixtures/fake-oc-helper.sh" "$WORK/bin/oc-helper"
chmod +x "$WORK/bin/nvidia-smi" "$WORK/bin/oc-helper"

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
export TROMMEL_NO_TRAY=1
export TROMMEL_AUTOSTART_STATE=on
export TROMMEL_OC_LOG="$WORK/oc.log"
export TROMMEL_FAKE_GPU_STATE="$WORK/gpu-state.json"
export TROMMEL_FAKE_MODE="$WORK/fake-mode"
export TROMMEL_OC_TIMEOUT_MS=1500
export TROMMEL_OC_VERIFY_MS=1000
export GTK_A11Y=atspi
export LANG=C.UTF-8
export LANGUAGE=en
# shellcheck source=scripts/atspi-bus.sh
. "$ROOT/scripts/atspi-bus.sh"
ensure_atspi_bus "$WORK" || {
    echo "cannot provide an AT-SPI bus; install at-spi2-core or run this inside a graphical session" >&2
    exit 1
}
# See ui-smoke.sh: XWayland exposes the complete AdwDialog tree to AT-SPI.
if [ -n "${DISPLAY:-}" ] && [ "${TROMMEL_UI_WAYLAND:-0}" != 1 ]; then
    export GDK_BACKEND=x11
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
    unset TROMMEL_OPEN_PRESET TROMMEL_OPEN_TUNE TROMMEL_OPEN_INSTALL TROMMEL_SCREENSHOT
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
