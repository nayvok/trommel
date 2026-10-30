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

cat > "$WORK/gpu-state.json" <<'JSON'
{
  "gpus": [
    {"index": 0, "uuid": "GPU-11111111-1111-1111-1111-111111111111", "name": "NVIDIA GeForce RTX 3080 Ti",
     "temp": 61, "fan": 68, "draw": 301.4, "power": 350.0, "power_default": 350.0,
     "power_min": 100.0, "power_max": 400.0, "core": 1770, "core_default": 1770,
     "mem": 9501, "mem_default": 9501, "util": 100, "vram": 8472},
    {"index": 1, "uuid": "GPU-22222222-2222-2222-2222-222222222222", "name": "NVIDIA GeForce RTX 3060",
     "temp": 55, "fan": 62, "draw": 128.2, "power": 170.0, "power_default": 170.0,
     "power_min": 60.0, "power_max": 200.0, "core": 1500, "core_default": 1500,
     "mem": 7000, "mem_default": 7000, "util": 99, "vram": 4096}
  ]
}
JSON
: > "$WORK/fake-mode"
cp "$ROOT/scripts/fixtures/fake-nvidia-smi.py" "$WORK/bin/nvidia-smi"
cp "$ROOT/scripts/fixtures/fake-oc-helper.sh" "$WORK/bin/oc-helper"
chmod +x "$WORK/bin/nvidia-smi" "$WORK/bin/oc-helper"

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
export TROMMEL_FAKE_GPU_STATE="$WORK/gpu-state.json"
export TROMMEL_FAKE_MODE="$WORK/fake-mode"
export TROMMEL_OC_TIMEOUT_MS=1500
export TROMMEL_OC_VERIFY_MS=1000
export GTK_A11Y=atspi
export AT_SPI_BUS_ADDRESS="${AT_SPI_BUS_ADDRESS:-unix:path=${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/at-spi/bus}"
if [ ! -S "${AT_SPI_BUS_ADDRESS#unix:path=}" ]; then
    echo "AT-SPI bus not found at $AT_SPI_BUS_ADDRESS" >&2
    echo "run this script inside the graphical session" >&2
    exit 1
fi
# AdwDialog children are exposed late on the nested Wayland accessibility
# bridge; XWayland gives AT-SPI a complete tree. The app itself is the same GTK
# build, and TROMMEL_UI_WAYLAND=1 still runs the suite on native Wayland.
if [ -n "${DISPLAY:-}" ] && [ "${TROMMEL_UI_WAYLAND:-0}" != 1 ]; then
    export GDK_BACKEND=x11
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
    rm -f "$WORK/gui.pid"
    (
        unset TROMMEL_OPEN_PRESET TROMMEL_OPEN_WIZARD TROMMEL_OPEN_TUNE TROMMEL_OPEN_INSTALL TROMMEL_SCREENSHOT
        case "$phase" in
            editor) export TROMMEL_OPEN_PRESET=1 ;;
            tune) export TROMMEL_OPEN_TUNE=1 ;;
            install) export TROMMEL_OPEN_INSTALL=1 ;;
        esac
        # setsid may fork on some runners, so the child writes its own PID
        # instead of trusting the shell's `$!`.
        exec setsid sh -c 'echo $$ > "$1/gui.pid"; exec "$2"' sh "$WORK" "$BIN/trommel"
    ) > "$WORK/gui-$phase.out" 2>&1 < /dev/null &
    for _ in $(seq 1 50); do
        [ -s "$WORK/gui.pid" ] && break
        sleep 0.1
    done
    GUI_PID="$(cat "$WORK/gui.pid" 2>/dev/null || true)"
}

stop_gui() {
    [ -n "$GUI_PID" ] || return 0
    kill "$GUI_PID" 2>/dev/null || true
    # A zombie still satisfies kill -0; read /proc so a dead GUI is not
    # mistaken for a live one and left to capture the next launch.
    for _ in $(seq 1 30); do
        local state
        state="$(awk '{print $3}' "/proc/$GUI_PID/stat" 2>/dev/null || true)"
        if [ -z "$state" ] || [ "$state" = Z ] || [ "$state" = X ]; then
            break
        fi
        sleep 0.1
    done
    kill -9 "$GUI_PID" 2>/dev/null || true
    GUI_PID=""
}

run_phase() {
    local phase="$1" attempt rc=1 attempts=2
    # The main phase changes daemon state (wallet, mining), so it cannot be
    # replayed against the same rig; the remaining phases are read-mostly.
    [ "$phase" = main ] && attempts=1
    # AT-SPI exposes an AdwDialog before its page is mapped; on a loaded VM the
    # first attempt can see only the header. A fresh GUI makes the phase
    # deterministic without weakening any of its checks.
    for attempt in $(seq 1 "$attempts"); do
        start_gui "$phase"
        sleep 2
        python3 "$ROOT/scripts/ui_smoke.py" --pid "$GUI_PID" --phase "$phase"
        rc=$?
        stop_gui
        [ "$rc" -eq 0 ] && break
        [ "$attempt" -lt "$attempts" ] && echo "  … retrying the $phase phase (AT-SPI timing)"
        sleep 1
    done
    if [ "$rc" -eq 0 ]; then pass "$phase phase"; else fail "$phase phase"; fi
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

step "A GUI crash leaves mining running (ADR 0006)"
if "$BIN/trommel" start >/dev/null 2>&1; then
    sleep 2
    BEFORE=$(grep -c '^reset ' "$WORK/oc.log" 2>/dev/null || true)
    start_gui crash
    sleep 3
    if [ -n "$GUI_PID" ] && kill -0 "$GUI_PID" 2>/dev/null; then
        kill -9 "$GUI_PID" 2>/dev/null || true
        wait "$GUI_PID" 2>/dev/null || true
        GUI_PID=""
        sleep 2
        if "$BIN/trommel" status --json 2>/dev/null | python3 -c 'import json,sys; s=json.load(sys.stdin); sys.exit(0 if s["state"]["running"] else 1)'; then
            pass "mining continues after SIGKILLing the GUI"
        else
            fail "the GUI crash stopped mining"
        fi
        AFTER=$(grep -c '^reset ' "$WORK/oc.log" 2>/dev/null || true)
        if [ "$BEFORE" = "$AFTER" ]; then
            pass "the GUI crash did not trigger a stock restore"
        else
            fail "the GUI crash triggered a restore"
        fi
    else
        fail "the GUI did not stay up long enough to kill it"
    fi
    "$BIN/trommel" stop >/dev/null 2>&1 || true
else
    fail "could not start mining for the GUI crash check"
fi

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
