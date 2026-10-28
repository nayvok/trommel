#!/bin/bash
# AT-SPI smoke test for the GTK app: fake two-GPU rig, private D-Bus, real
# Wayland display. It clicks the critical path (quick setup, start/stop,
# settings, preset editor, GPU tuning, miner install) and reports a matrix.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$ROOT/.uicheck"
BIN="$ROOT/target/debug"
FAILS=0
GUI_PID=""
DAEMON_PID=""
BUS_PID=""
GUI_STARTED=0

pass() { printf '\033[1;32m  ✓ %s\033[0m\n' "$*"; }
fail() { printf '\033[1;31m  ✗ %s\033[0m\n' "$*"; FAILS=$((FAILS + 1)); }
step() { printf '\n\033[1;34m== %s ==\033[0m\n' "$*"; }

cleanup() {
    [ -n "$GUI_PID" ] && kill "$GUI_PID" 2>/dev/null || true
    [ -n "$DAEMON_PID" ] && kill "$DAEMON_PID" 2>/dev/null || true
    [ -n "$BUS_PID" ] && kill "$BUS_PID" 2>/dev/null || true
    pkill -f "$WORK/bin/wildrig-multi" 2>/dev/null || true
}
trap cleanup EXIT

step "Build"
( cd "$ROOT" && cargo build -q -p trommel --features gui -p trommeld ) || {
    echo "build failed" >&2
    exit 1
}

step "Prepare private rig"
rm -rf "$WORK"
mkdir -p "$WORK/bin" "$WORK/config/trommel" "$WORK/state/trommel" "$WORK/data" "$WORK/cache"

cat > "$WORK/bin/nvidia-smi" <<'SH'
#!/bin/bash
args="$*"
case "$args" in
  *index,uuid*)
    echo "0, GPU-11111111-1111-1111-1111-111111111111, NVIDIA GeForce RTX 3080 Ti, 61, 68, 301.40, 300.00, 350.00, 100.00, 400.00, 1620, 810, 100, 8472"
    echo "1, GPU-22222222-2222-2222-2222-222222222222, NVIDIA GeForce RTX 3060, 55, 62, 128.20, 130.00, 170.00, 60.00, 200.00, 1500, 750, 99, 4096"
    ;;
  *power.limit*) echo "300.00" ;;
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

cat > "$WORK/bin/wildrig-multi" <<'SH'
#!/bin/bash
i=0
while true; do
  i=$((i+1))
  echo "[12:00:$((i%60))] hashrate: 10s: 125.$((i%9)) 60s: 125.0 15m: 124.9 TH/s"
  echo "[12:00:$((i%60))] Shares: $((500+i)) accepted, 2 rejected"
  sleep 2
done
SH
chmod +x "$WORK/bin/wildrig-multi"

python3 - "$WORK" <<'PY'
import json, pathlib, sys
work = pathlib.Path(sys.argv[1])
cfg = {
    "schema_version": 2,
    "active_preset": "pearl-wildrig",
    "presets": {
        "pearl-wildrig": {
            "id": "pearl-wildrig", "name": "Pearl WildRig", "coin": "pearl",
            "algo": "pearlhash", "miner": "wildrig", "miner_version": "0.51.8",
            "pool": "pearlhash", "pool_url": "pool.pearlhash.xyz:9000",
            "wallet": "", "worker": "smoke", "gpu_profile": "balanced",
            "api_port": 0, "expected_hashrate": None, "miner_options": {},
            "miner_fee_override_pct": None, "miner_fee_pct": 0.0,
            "pool_fee_pct": 1.0, "extra_args": "", "env": {},
            "created_at": 0, "updated_at": 0,
        },
        "second": {
            "id": "second", "name": "Second", "coin": "pearl",
            "algo": "pearlhash", "miner": "wildrig", "miner_version": "0.51.8",
            "pool": "pearlhash", "pool_url": "pool.pearlhash.xyz:9000",
            "wallet": "", "worker": "smoke2", "gpu_profile": "balanced",
            "api_port": 0, "expected_hashrate": None, "miner_options": {},
            "miner_fee_override_pct": None, "miner_fee_pct": 0.0,
            "pool_fee_pct": 1.0, "extra_args": "", "env": {},
            "created_at": 0, "updated_at": 0,
        }
    },
    "gpu_profiles": {
        "balanced": {
            "name": "Balanced",
            "default": {"enabled": True, "power_limit_w": None,
                        "core_clock_mhz": None, "memory_clock_mhz": None,
                        "fan_pct": None, "temp_limit_c": 85, "temp_resume_c": 65},
            "per_gpu": {},
        }
    },
    "miner_installs": {
        "wildrig@0.51.8": {
            "miner": "wildrig", "version": "0.51.8",
            "path": str(work / "bin" / "wildrig-multi"),
            "sha256": "", "source_url": "", "verified": False, "custom": True,
            "installed_at": 0,
        }
    },
    "custom_miners": {}, "pools": {},
    "behavior": {
        "auto_restart": True, "restart_delay_s": 2, "poll_interval_s": 1,
        "restore_stock_on_stop": True, "start_mining_on_login": False,
        "notifications": False, "language": "en", "appearance": "system",
        "simple_mode": False,
    },
}
(work / "config" / "trommel" / "config.json").write_text(json.dumps(cfg, indent=2))
PY

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
export AT_SPI_BUS_ADDRESS="${AT_SPI_BUS_ADDRESS:-unix:path=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/at-spi/bus}"
if [ ! -S "${AT_SPI_BUS_ADDRESS#unix:path=}" ]; then
    echo "AT-SPI bus not found at $AT_SPI_BUS_ADDRESS" >&2
    echo "run this script inside the graphical session" >&2
    exit 1
fi
pass "private rig and AT-SPI bus are ready"

step "Start daemon"
setsid "$BIN/trommeld" > "$WORK/daemon.out" 2>&1 < /dev/null &
DAEMON_PID=$!
sleep 2
if kill -0 "$DAEMON_PID" 2>/dev/null; then pass "trommeld is running"; else fail "trommeld did not start"; cat "$WORK/daemon.out"; exit 1; fi

start_gui() {
    local phase="$1"
    GUI_STARTED=1
    GUI_PID=""
    (
        unset TROMMEL_OPEN_PRESET TROMMEL_OPEN_WIZARD TROMMEL_OPEN_TUNE TROMMEL_OPEN_INSTALL TROMMEL_SCREENSHOT
        case "$phase" in
            editor) export TROMMEL_OPEN_PRESET=1 ;;
            tune) export TROMMEL_OPEN_TUNE=1 ;;
            install) export TROMMEL_OPEN_INSTALL=1 ;;
        esac
        exec setsid "$BIN/trommel"
    ) > "$WORK/gui-$phase.out" 2>&1 < /dev/null &
    GUI_PID=$!
}

run_phase() {
    local phase="$1"
    start_gui "$phase"
    sleep 1
    python3 "$ROOT/scripts/ui_smoke.py" --pid "$GUI_PID" --phase "$phase"
    local rc=$?
    if [ "$rc" -eq 0 ]; then pass "$phase phase"; else fail "$phase phase"; fi
    kill "$GUI_PID" 2>/dev/null || true
    wait "$GUI_PID" 2>/dev/null || true
    GUI_PID=""
    sleep 1
}

step "Main window: quick setup, start/stop, pages, settings"
run_phase main

step "Preset editor"
run_phase editor

step "GPU tuning while mining"
if ! "$BIN/trommel" start; then
    fail "could not start the miner before the tune phase"
fi
sleep 2
if ! "$BIN/trommel" status --json 2>/dev/null | python3 -c 'import json,sys; s=json.load(sys.stdin); sys.exit(0 if s["state"]["running"] else 1)'; then
    fail "miner is not running before the tune phase"
    cat "$WORK/daemon.out" 2>/dev/null | tail -5
fi
run_phase tune
"$BIN/trommel" stop >/dev/null 2>&1 || true

step "Miner install dialog"
run_phase install

step "Cleanup"
"$BIN/trommel" stop >/dev/null 2>&1 || true
kill "$DAEMON_PID" 2>/dev/null || true
kill "$BUS_PID" 2>/dev/null || true
DAEMON_PID=""
BUS_PID=""
sleep 1

echo
if [ "$FAILS" -eq 0 ]; then
    printf '\033[1;32mUI SMOKE: ALL GREEN\033[0m\n'
    exit 0
else
    printf '\033[1;31mUI SMOKE FAILURES: %s\033[0m\n' "$FAILS"
    echo "logs: $WORK"
    exit 1
fi
