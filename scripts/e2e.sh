#!/bin/bash
# End-to-end test on a private D-Bus with fake GPU, helper and miner.
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$ROOT/.e2e"
BIN="$ROOT/target/debug"
FAILS=0

pass() { printf '\033[1;32m  ✓ %s\033[0m\n' "$*"; }
fail() { printf '\033[1;31m  ✗ %s\033[0m\n' "$*"; FAILS=$((FAILS + 1)); }
step() { printf '\n\033[1;34m== %s ==\033[0m\n' "$*"; }

# A zombie is not a running process.
#
# On CI the daemon is killed while its miner runs; the orphan is reparented to
# a PID 1 that does not reap, so the miner stays visible as <defunct> for the
# rest of the run. `kill -0` still succeeds for it, which made every "the miner
# is gone" assertion fail. /proc reports the real state: Z (and X/x) is dead.
alive() {
    local pid="${1:-}" stat state
    [ -n "$pid" ] || return 1
    [ -r "/proc/$pid/stat" ] || return 1
    stat=$(<"/proc/$pid/stat")
    [ -n "$stat" ] || return 1
    state=${stat##*) }
    case "${state%% *}" in Z|X|x) return 1 ;; esac
    return 0
}

# Start the daemon in its own session and record the real daemon PID.
#
# `setsid` forks when the caller is a process-group leader (as it is on some CI
# runners), so `$!` is not necessarily the daemon. The wrapper writes its own
# PID before `exec`, which works whether setsid execs or forks.
start_daemon() {
    rm -f "$WORK/daemon.pid"
    setsid bash -c 'echo $$ > "$1"; shift; exec "$@"' _ "$WORK/daemon.pid" "$@" \
        > "$WORK/daemon.out" 2>&1 < /dev/null &
    for _ in $(seq 1 100); do
        [ -s "$WORK/daemon.pid" ] && break
        sleep 0.1
    done
    DAEMON_PID=$(cat "$WORK/daemon.pid" 2>/dev/null || true)
}

step "Build"
( cd "$ROOT" && cargo build -q -p trommeld -p trommel ) || { echo "build failed" >&2; exit 1; }

step "Prepare environment"
rm -rf "$WORK"
mkdir -p "$WORK/bin" "$WORK/config/trommel" "$WORK/state" "$WORK/data" "$WORK/cache"
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

cat > "$WORK/gpu-state.json" <<'JSON'
{
  "gpus": [
    {"index": 0, "uuid": "GPU-11111111-1111-1111-1111-111111111111", "name": "Fake RTX 3080 Ti",
     "temp": 60, "fan": 65, "draw": 300.0, "power": 350.0, "power_default": 350.0,
     "power_min": 100.0, "power_max": 400.0, "core": 1770, "core_default": 1770,
     "mem": 9501, "mem_default": 9501, "util": 100, "vram": 1000},
    {"index": 1, "uuid": "GPU-22222222-2222-2222-2222-222222222222", "name": "Fake RTX 3060",
     "temp": 55, "fan": 60, "draw": 130.0, "power": 170.0, "power_default": 170.0,
     "power_min": 60.0, "power_max": 200.0, "core": 1500, "core_default": 1500,
     "mem": 7000, "mem_default": 7000, "util": 99, "vram": 500}
  ]
}
JSON
: > "$WORK/fake-mode"
cp "$ROOT/scripts/fixtures/fake-nvidia-smi.py" "$WORK/bin/nvidia-smi"
cp "$ROOT/scripts/fixtures/fake-oc-helper.sh" "$WORK/bin/oc-helper"
chmod +x "$WORK/bin/nvidia-smi" "$WORK/bin/oc-helper"

cat > "$WORK/bin/wildrig-multi" <<SH
#!/bin/bash
printf '%s\n' "\$*" > "$WORK/miner-args.txt"
echo "[12:00:00] hashrate: 10s: 125.0 60s: 125.0 15m: 124.0 TH/s"
echo "[12:00:00] Shares: 512 accepted, 2 rejected"
while true; do sleep 2; done
SH
chmod +x "$WORK/bin/wildrig-multi"

python3 - "$WORK" <<'PY'
import json, pathlib, sys
work = pathlib.Path(sys.argv[1])
cfg = {
  "schema_version": 3, "active_preset": "pearl-wildrig",
  "presets": {"pearl-wildrig": {
    "id":"pearl-wildrig","name":"Pearl · WildRig","coin":"pearl","algo":"pearlhash",
    "miner":"wildrig","miner_version":"0.51.8","pool":"pearlhash",
    "pool_url":"pool.pearlhash.xyz:9000","wallet":"prl1testwallet","worker":"e2e",
    "gpu_profile":"balanced","api_port":0,"expected_hashrate":125e12,
    "miner_options":{"fan_speed":70},"miner_fee_override_pct":None,"miner_fee_pct":0.0,
    "pool_fee_pct":1.0,"extra_args":"","env":{},"created_at":0,"updated_at":0},
    "second": {
    "id":"second","name":"Second","coin":"pearl","algo":"pearlhash",
    "miner":"wildrig","miner_version":"0.51.8","pool":"pearlhash",
    "pool_url":"pool.pearlhash.xyz:9000","wallet":"prl1testwallet","worker":"e2e2",
    "gpu_profile":"balanced","api_port":0,"expected_hashrate":125e12,
    "miner_options":{},"miner_fee_override_pct":None,"miner_fee_pct":0.0,
    "pool_fee_pct":1.0,"extra_args":"","env":{},"created_at":0,"updated_at":0}},
  "gpu_profiles": {"balanced": {"name":"Balanced","default":{"enabled":True,"power_limit_w":300,
    "core_clock_mhz":1620,"memory_clock_mhz":810,"fan_pct":None,"temp_limit_c":90,"temp_resume_c":65},
    "per_gpu":{
      "GPU-11111111-1111-1111-1111-111111111111": {"enabled":True,"custom":False},
      "GPU-22222222-2222-2222-2222-222222222222": {"enabled":True,"custom":True,
        "power_limit_w":130,"core_clock_mhz":1500,"memory_clock_mhz":750}
    }}},
  "miner_installs": {"wildrig@0.51.8": {"miner":"wildrig","version":"0.51.8",
    "path":str(work/"bin"/"wildrig-multi"),"sha256":"","source_url":"","verified":False,"custom":True,"installed_at":0}},
  "custom_miners": {}, "pools": {},
  "behavior": {"auto_restart":True,"restart_delay_s":2,"poll_interval_s":1,
    "restore_stock_on_stop":True,"start_mining_on_login":False,"notifications":True,"language":"en"}
}
(work/"config"/"trommel"/"config.json").write_text(json.dumps(cfg, indent=2))
PY

export PATH="$WORK/bin:$ROOT/target/debug:$PATH"
export XDG_CONFIG_HOME="$WORK/config"
export XDG_STATE_HOME="$WORK/state"
export XDG_DATA_HOME="$WORK/data"
export XDG_CACHE_HOME="$WORK/cache"
export TROMMEL_OC_HELPER="$WORK/bin/oc-helper"
export TROMMEL_NO_EXTERNAL_SCAN=1
# Guard and session-bus tests opt in explicitly; the rest needs a predictable
# daemon lifetime.
export TROMMEL_NO_GUARD=1
export TROMMEL_OC_LOG="$WORK/oc.log"
export TROMMEL_FAKE_GPU_STATE="$WORK/gpu-state.json"
export TROMMEL_FAKE_MODE="$WORK/fake-mode"
export TROMMEL_OC_TIMEOUT_MS=1500
export TROMMEL_OC_VERIFY_MS=1000
BUS_OUT=$(dbus-daemon --config-file="$WORK/bus.conf" --print-address --print-pid --fork 2>/dev/null)
export DBUS_SESSION_BUS_ADDRESS="$(printf '%s\n' "$BUS_OUT" | sed -n 1p)"
BUS_PID="$(printf '%s\n' "$BUS_OUT" | sed -n 2p)"
cleanup() {
    pkill -f "$WORK/bin/wildrig-multi" 2>/dev/null || true
    pkill -f "$WORK/data/trommel/miners" 2>/dev/null || true
    [ -n "${NOTIFY_PID:-}" ] && kill "$NOTIFY_PID" 2>/dev/null || true
    [ -n "${DAEMON_PID:-}" ] && kill "$DAEMON_PID" 2>/dev/null || true
    [ -n "$BUS_PID" ] && kill "$BUS_PID" 2>/dev/null || true
}
trap cleanup EXIT

step "Notification service stub"
NOTIFY_LOG="$WORK/notify.log"
NOTIFY_READY="$WORK/notify.ready"
: > "$NOTIFY_LOG"
NOTIFY_STUB=0
if command -v python3 >/dev/null 2>&1 && python3 -c 'import gi' >/dev/null 2>&1; then
    python3 "$ROOT/scripts/fixtures/notify-stub.py" "$NOTIFY_LOG" "$NOTIFY_READY" \
        > "$WORK/notify-stub.out" 2>&1 &
    NOTIFY_PID=$!
    for _ in $(seq 1 50); do
        [ -f "$NOTIFY_READY" ] && break
        sleep 0.1
    done
    if [ -f "$NOTIFY_READY" ]; then
        NOTIFY_STUB=1
        pass "notification service stub owns org.freedesktop.Notifications"
    else
        fail "notification stub did not start"
        cat "$WORK/notify-stub.out"
    fi
else
    echo "  - python3-gi is unavailable; desktop notification delivery is skipped"
fi

step "Root helper"
if "$ROOT/scripts/test-oc-helper.sh" > "$WORK/helper-test.out" 2>&1; then
    pass "polkit helper verification tests"
else
    fail "polkit helper verification tests"
    tail -20 "$WORK/helper-test.out"
fi

step "Start daemon"
start_daemon "$BIN/trommeld"
sleep 2
if alive "$DAEMON_PID"; then pass "trommeld is running"; else fail "trommeld did not start"; cat "$WORK/daemon.out"; exit 1; fi

step "Status contract"
JSON=$("$BIN/trommel" status --json 2>/dev/null)
echo "$JSON" | python3 -c '
import json,sys
s=json.load(sys.stdin)
assert s["contract_version"]==3
assert len(s["gpus"])==2, s["gpus"]
assert s["miner_info"]["id"]=="wildrig"
assert s["miner_info"]["total_fee_pct"]==1.0
assert s["state"]["running"] is False
print("  contract ok, gpus:", len(s["gpus"]))
' && pass "status contains two GPUs and miner info" || fail "status contract"

step "Start and snapshot"
"$BIN/trommel" start 2>&1 | tee "$WORK/start.out"
sleep 2
if [ -f "$WORK/state/trommel/oc-snapshot.json" ]; then pass "snapshot written"; else fail "snapshot missing"; fi
if grep -q "apply --gpu GPU-11111111" "$WORK/oc.log" 2>/dev/null; then pass "per-GPU apply called"; else fail "per-GPU apply missing"; fi
if grep -q "apply --gpu GPU-11111111.*--pl 300" "$WORK/oc.log" 2>/dev/null; then pass "power limit passed to helper"; else fail "power limit missing"; fi
if grep -q "apply --gpu GPU-11111111.*--lgc 1620" "$WORK/oc.log" 2>/dev/null; then pass "core lock passed to helper"; else fail "core lock missing"; fi
if grep -q "apply --gpu GPU-11111111.*--lmc 810" "$WORK/oc.log" 2>/dev/null; then pass "optional memory lock passed to helper"; else fail "memory lock missing"; fi
if grep -q "apply --gpu GPU-22222222.*--pl 130" "$WORK/oc.log" 2>/dev/null; then pass "custom per-card override applied"; else fail "custom override missing"; fi
JSON=$("$BIN/trommel" status --json 2>/dev/null)
echo "$JSON" | python3 -c '
import json,sys
s=json.load(sys.stdin)
assert s["state"]["running"] is True, s["state"]
assert s["total"]["power_w"] == 430.0, s["total"]
print("  running:", s["state"]["running"], "power:", s["total"]["power_w"])
' && pass "miner running with aggregated power" || fail "running status"

step "Apply and restart while running"
"$BIN/trommel" preset use second --restart >/dev/null 2>&1
sleep 2
JSON=$("$BIN/trommel" status --json 2>/dev/null)
echo "$JSON" | python3 -c '
import json,sys
s=json.load(sys.stdin)
assert s["state"]["running"] is True, s["state"]
assert s["state"]["preset_id"]=="second", s["state"]
print("  active preset:", s["state"]["preset_id"])
' && pass "apply and restart kept mining" || fail "apply and restart"
grep -q "apply --gpu GPU-11111111" "$WORK/oc.log" && pass "GPU profile reapplied" || fail "GPU profile not reapplied"

step "Stop and restore"
"$BIN/trommel" stop >/dev/null 2>&1
sleep 1
if [ ! -f "$WORK/state/trommel/oc-snapshot.json" ]; then pass "snapshot cleared"; else fail "snapshot not cleared"; fi
if grep -q "reset --gpu GPU-11111111.*--pl 350" "$WORK/oc.log" 2>/dev/null; then pass "stock power limit restored"; else fail "stock restore missing"; tail -10 "$WORK/oc.log" 2>/dev/null; fi
if grep -q "reset --gpu GPU-22222222.*--pl 170" "$WORK/oc.log" 2>/dev/null; then pass "second card restored to its own snapshot"; else fail "second card restore missing"; fi

step "Per-GPU participation excludes a card from the miner"
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "config" / "trommel" / "config.json"
cfg = json.loads(path.read_text())
# The apply-and-restart test above activated the second preset; the
# participation checks below edit the main one.
cfg["active_preset"] = "pearl-wildrig"
cfg["gpu_profiles"]["balanced"]["per_gpu"]["GPU-22222222-2222-2222-2222-222222222222"]["enabled"] = False
path.write_text(json.dumps(cfg, indent=2))
PY
"$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1
: > "$WORK/miner-args.txt"
: > "$WORK/oc.log"
if "$BIN/trommel" start > "$WORK/participation-start.out" 2>&1; then
    pass "start with one excluded card"
else
    fail "start failed with one excluded card"; cat "$WORK/participation-start.out"
fi
sleep 2
if python3 - "$WORK" <<'PY'
import pathlib, sys
work = pathlib.Path(sys.argv[1])
args = (work / "miner-args.txt").read_text().split()
assert "--gpu-list" in args, args
index = args.index("--gpu-list")
assert args[index + 1] == "0", args[index:index + 2]
print("  miner args:", " ".join(args[index:index + 2]))
PY
then
    pass "the miner received only the participating card"
else
    fail "the miner device list is wrong"
fi
if grep -q "apply --gpu GPU-22222222" "$WORK/oc.log"; then
    fail "the excluded card was tuned"
else
    pass "the excluded card was not tuned"
fi
if "$BIN/trommel" status --json 2>/dev/null | python3 -c '
import json, sys
s = json.load(sys.stdin)
by_index = {card["index"]: card for card in s["participation"]}
assert by_index[0]["enabled"] and by_index[0]["mining"], by_index
assert not by_index[1]["enabled"] and not by_index[1]["mining"], by_index
assert s["total"]["gpus_enabled"] == 1, s["total"]
assert s["total"]["gpus_total"] == 2, s["total"]
assert any("Excluded" in w or "исключ" in w.lower() for w in s["warnings"]), s["warnings"]
print("  excluded card:", by_index[1])
'; then
    pass "status marks the excluded card and counts it out"
else
    fail "participation status is wrong"
fi
"$BIN/trommel" stop >/dev/null 2>&1 || true
if grep -q "reset --gpu GPU-22222222" "$WORK/oc.log"; then
    fail "stop tried to restore the never-tuned excluded card"
else
    pass "stop restored only the tuned card"
fi

step "Excluding a previously tuned card puts its stock back"
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "config" / "trommel" / "config.json"
cfg = json.loads(path.read_text())
cfg["behavior"]["restore_stock_on_stop"] = False
cfg["gpu_profiles"]["balanced"]["per_gpu"]["GPU-22222222-2222-2222-2222-222222222222"]["enabled"] = True
path.write_text(json.dumps(cfg, indent=2))
PY
"$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1
: > "$WORK/oc.log"
"$BIN/trommel" start >/dev/null 2>&1
sleep 2
if grep -q "apply --gpu GPU-22222222.*--pl 130" "$WORK/oc.log"; then
    pass "both cards were tuned before the exclusion"
else
    fail "the second card was not tuned before the exclusion"
fi
"$BIN/trommel" stop >/dev/null 2>&1 || true
[ -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "restore-off keeps the snapshot for the next session" \
    || fail "restore-off lost the snapshot"
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "config" / "trommel" / "config.json"
cfg = json.loads(path.read_text())
cfg["gpu_profiles"]["balanced"]["per_gpu"]["GPU-22222222-2222-2222-2222-222222222222"]["enabled"] = False
path.write_text(json.dumps(cfg, indent=2))
PY
"$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1
: > "$WORK/oc.log"
: > "$WORK/miner-args.txt"
"$BIN/trommel" start >/dev/null 2>&1
sleep 2
if grep -q "reset --gpu GPU-22222222.*--pl 170" "$WORK/oc.log"; then
    pass "the newly excluded card was returned to its stock limit"
else
    fail "the newly excluded card kept its tuning"
fi
if grep -q "apply --gpu GPU-22222222" "$WORK/oc.log"; then
    fail "the newly excluded card was tuned again"
else
    pass "the newly excluded card was not tuned"
fi
grep -q -- "--gpu-list 0" "$WORK/miner-args.txt" \
    && pass "the restart used the reduced device list" \
    || fail "the reduced device list is missing"
"$BIN/trommel" stop >/dev/null 2>&1 || true
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "config" / "trommel" / "config.json"
cfg = json.loads(path.read_text())
cfg["behavior"]["restore_stock_on_stop"] = True
cfg["gpu_profiles"]["balanced"]["per_gpu"]["GPU-22222222-2222-2222-2222-222222222222"]["enabled"] = True
path.write_text(json.dumps(cfg, indent=2))
PY
"$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1
"$BIN/trommel" oc restore --mode snapshot >/dev/null 2>&1 \
    && pass "explicit restore cleaned the restore-off session" \
    || fail "explicit restore failed after the restore-off session"
[ ! -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "no snapshot left after the explicit restore" \
    || fail "snapshot survived the explicit restore"

step "Emergency-stop restores only the participating card"
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "config" / "trommel" / "config.json"
cfg = json.loads(path.read_text())
cfg["gpu_profiles"]["balanced"]["per_gpu"]["GPU-22222222-2222-2222-2222-222222222222"]["enabled"] = False
path.write_text(json.dumps(cfg, indent=2))
PY
"$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1
: > "$WORK/oc.log"
"$BIN/trommel" start >/dev/null 2>&1
sleep 2
MINER_PID=$(pgrep -f "$WORK/bin/wildrig-multi" | head -1)
[ -n "$MINER_PID" ] && pass "miner is running with one excluded card" \
    || fail "miner did not start with one excluded card"
kill -9 "$DAEMON_PID" 2>/dev/null || true
sleep 1
"$BIN/trommel" emergency-stop --reason=manual > "$WORK/participation-emergency.out" 2>&1 \
    && pass "emergency-stop handled the reduced session" \
    || { fail "emergency-stop failed"; cat "$WORK/participation-emergency.out"; }
alive "$MINER_PID" \
    && fail "the excluded-session miner survived emergency-stop" \
    || pass "emergency-stop stopped the reduced-session miner"
grep -q "reset --gpu GPU-11111111.*--pl 350" "$WORK/oc.log" \
    && pass "emergency-stop restored the participating card" \
    || fail "emergency-stop did not restore the participating card"
if grep -q "reset --gpu GPU-22222222" "$WORK/oc.log"; then
    fail "emergency-stop touched the excluded card"
else
    pass "emergency-stop left the excluded card alone"
fi
[ ! -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "emergency-stop cleared the reduced snapshot" \
    || fail "reduced snapshot survived emergency-stop"
start_daemon "$BIN/trommeld"
sleep 2
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "config" / "trommel" / "config.json"
cfg = json.loads(path.read_text())
cfg["gpu_profiles"]["balanced"]["per_gpu"]["GPU-22222222-2222-2222-2222-222222222222"]["enabled"] = True
path.write_text(json.dumps(cfg, indent=2))
PY
"$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1

step "A miner without device selection refuses a partial rig"
"$BIN/trommel" miner import custom-b2 1.0.0 "$WORK/bin/wildrig-multi" >/dev/null 2>&1 \
    && pass "custom miner imported for the capability test" \
    || fail "custom miner import failed"
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "config" / "trommel" / "config.json"
cfg = json.loads(path.read_text())
preset = cfg["presets"]["pearl-wildrig"]
preset["miner"] = "custom-b2"
preset["miner_version"] = "1.0.0"
cfg["gpu_profiles"]["balanced"]["per_gpu"]["GPU-22222222-2222-2222-2222-222222222222"]["enabled"] = False
path.write_text(json.dumps(cfg, indent=2))
PY
"$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1
: > "$WORK/miner-args.txt"
if "$BIN/trommel" start > "$WORK/unsupported.out" 2>&1; then
    fail "start succeeded although the miner cannot select devices"
else
    pass "start is refused for a partial rig on an unsupported miner"
fi
grep -q "custom-b2" "$WORK/unsupported.out" \
    && pass "the error names the miner" \
    || { fail "the error does not name the miner"; cat "$WORK/unsupported.out"; }
[ ! -s "$WORK/miner-args.txt" ] \
    && pass "no miner process was spawned" \
    || fail "the unsupported miner was spawned anyway"
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "config" / "trommel" / "config.json"
cfg = json.loads(path.read_text())
preset = cfg["presets"]["pearl-wildrig"]
preset["miner"] = "wildrig"
preset["miner_version"] = "0.51.8"
cfg["gpu_profiles"]["balanced"]["per_gpu"]["GPU-22222222-2222-2222-2222-222222222222"]["enabled"] = True
cfg.get("miner_installs", {}).pop("custom-b2@1.0.0", None)
path.write_text(json.dumps(cfg, indent=2))
PY
"$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1

step "Preset commands"
"$BIN/trommel" preset list | grep -q "pearl-wildrig" && pass "preset list" || fail "preset list"
"$BIN/trommel" preset use pearl-wildrig --stop >/dev/null 2>&1 && pass "preset use --stop" || fail "preset use --stop"
"$BIN/trommel" miner list | grep -q "installed: 0.51.8" && pass "miner install state" || fail "miner list"

step "Miner version manager"
"$BIN/trommel" miner import wildrig 0.50.0 "$WORK/bin/wildrig-multi" >/dev/null 2>&1 \
    && pass "an older build is installed for rollback" || fail "older build import failed"
"$BIN/trommel" config get | python3 -c '
import json,sys
c=json.load(sys.stdin)
assert "wildrig@0.50.0" in c["miner_installs"]
assert all(p["miner_version"] == "0.51.8" for p in c["presets"].values())
' && pass "installing a version does not rewrite a preset" || fail "preset version was rewritten"
"$BIN/trommel" miner verify wildrig 0.50.0 --json > "$WORK/verify-old.json" 2>/dev/null \
    && python3 -c '
import hashlib, json, sys
report=json.load(open(sys.argv[1]))
assert report[0]["status"] == "ok", report
assert report[0]["sha256_verified"] is False, report
digest = report[0]["recorded_sha256"]
if digest is not None:
    actual = hashlib.sha256(open(report[0]["path"], "rb").read()).hexdigest()
    assert digest == actual, (digest, actual)
' "$WORK/verify-old.json" \
    && pass "verify reports the binary with only its real digest" || fail "verify reported a wrong state"
if "$BIN/trommel" miner remove wildrig 0.51.8 --yes > "$WORK/remove-used.out" 2>&1; then
    fail "a version pinned by a preset was removed"
else
    pass "removing a preset-pinned version is refused"
fi
grep -q "Pearl" "$WORK/remove-used.out" \
    && pass "the refusal names the affected preset" || fail "the refusal does not name the preset"
"$BIN/trommel" config get | python3 -c '
import json, os, sys
c=json.load(sys.stdin)
install=c["miner_installs"]["wildrig@0.51.8"]
assert os.path.exists(install["path"]), install
' && pass "the pinned binary was not deleted" || fail "the pinned binary disappeared"
"$BIN/trommel" miner remove wildrig 0.50.0 --yes >/dev/null 2>&1 \
    && pass "an unused version is removed with --yes" || fail "unused version removal failed"
[ ! -e "$WORK/data/trommel/miners/wildrig/0.50.0" ] \
    && pass "the unused version files were deleted" || fail "unused version files remain"
"$BIN/trommel" config get | python3 -c '
import json,sys
c=json.load(sys.stdin)
assert "wildrig@0.51.8" in c["miner_installs"], c["miner_installs"]
assert "wildrig@0.50.0" not in c["miner_installs"], c["miner_installs"]
' && pass "the install record is gone from the config" || fail "the install record survived"
"$BIN/trommel" diagnose --json > "$WORK/versions.json" 2>/dev/null \
    && python3 -c '
import json,sys
report=json.load(open(sys.argv[1]))
versions=report["miner_versions"]
assert [entry["version"] for entry in versions] == ["0.51.8"], versions
assert versions[0]["status"] == "ok", versions
' "$WORK/versions.json" \
    && pass "diagnostics report the installed version state" || fail "diagnostics version report"

step "Config and migration sanity"
"$BIN/trommel" config get | python3 -c 'import json,sys; c=json.load(sys.stdin); assert c["schema_version"]==3; assert c["active_preset"]=="pearl-wildrig"' && pass "config v3 is active" || fail "config v3"

step "Diagnostics redaction"
if "$BIN/trommel" diagnose --json > "$WORK/diagnostics.json" 2>/dev/null \
    && ! grep -q 'prl1testwallet' "$WORK/diagnostics.json" \
    && grep -q '<redacted>' "$WORK/diagnostics.json"; then
    pass "diagnose hides every preset wallet"
else
    fail "diagnose leaked a wallet or failed"; head -20 "$WORK/diagnostics.json" 2>/dev/null
fi

step "Stop failure keeps the snapshot"
"$BIN/trommel" start >/dev/null 2>&1
sleep 1
cat > "$WORK/bin/oc-helper" <<SH
#!/bin/bash
echo "\$*" >> "$WORK/oc.log"
case "\$1" in
  reset) exit 1 ;;
esac
exit 0
SH
chmod +x "$WORK/bin/oc-helper"
if "$BIN/trommel" stop >/dev/null 2>&1; then
    fail "stop reported success although reset failed"
else
    pass "stop returns an error when reset fails"
fi
if [ -f "$WORK/state/trommel/oc-snapshot.json" ]; then
    pass "snapshot kept for a retry"
else
    fail "snapshot was lost after a failed restore"
fi
cp "$ROOT/scripts/fixtures/fake-oc-helper.sh" "$WORK/bin/oc-helper"
chmod +x "$WORK/bin/oc-helper"
"$BIN/trommel" stop >/dev/null 2>&1 || true
if [ ! -f "$WORK/state/trommel/oc-snapshot.json" ]; then
    pass "retry restored stock and cleared the snapshot"
else
    fail "snapshot still present after a successful retry"
fi

step "OC report and restore modes"
REPORT=$("$BIN/trommel" oc report --json 2>/dev/null)
echo "$REPORT" | python3 -c '
import json,sys
r=json.load(sys.stdin)
assert r["needs_restore"] is False, r
assert r["snapshot_available"] is False, r
assert len(r["targets"]["defaults"]) == 2, r
print("  report ok, defaults:", [t["power_w"] for t in r["targets"]["defaults"]])
' && pass "OC report lists driver-default targets" || fail "OC report"
"$BIN/trommel" oc restore --mode snapshot >/dev/null 2>&1 \
    && pass "restore without a snapshot falls back to defaults" \
    || fail "restore without a snapshot failed"
if [ ! -f "$WORK/state/trommel/oc-snapshot.json" ]; then pass "no snapshot left after defaults restore"; else fail "snapshot left after defaults restore"; fi

step "Ignored power limit fails the start"
: > "$WORK/fake-mode"
echo ignore_pl > "$WORK/fake-mode"
if "$BIN/trommel" start > "$WORK/ignored.out" 2>&1; then
    fail "start succeeded although the power limit was ignored"
else
    pass "start fails when the driver ignores the power limit"
fi
"$BIN/trommel" status --json 2>/dev/null | python3 -c '
import json,sys
s=json.load(sys.stdin)
assert s["state"]["running"] is False, s["state"]
print("  miner stayed stopped")
' && pass "miner is not running after a failed apply" || fail "miner ran after a failed apply"
: > "$WORK/fake-mode"
"$BIN/trommel" oc restore --mode defaults >/dev/null 2>&1 \
    && pass "cleanup restore after the ignored power limit" \
    || fail "cleanup restore failed"

step "Ignored memory lock stops the miner after load"
: > "$WORK/fake-mode"
echo ignore_lmc > "$WORK/fake-mode"
if ! "$BIN/trommel" start > "$WORK/lmc.out" 2>&1; then
    fail "start failed before the runtime lock check"; tail -5 "$WORK/lmc.out"
else
    sleep 3
    "$BIN/trommel" status --json 2>/dev/null | python3 -c '
import json,sys
s=json.load(sys.stdin)
assert s["state"]["running"] is False, s["state"]
err=(s["state"].get("last_error") or "").lower()
assert "810" in err or "memory" in err or "памят" in err, err
print("  stopped with:", s["state"]["last_error"][:80])
' && pass "runtime verification stopped the miner and explained why" || fail "runtime lock verification"
fi
: > "$WORK/fake-mode"

step "Restore-on-stop off keeps the snapshot"
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "config" / "trommel" / "config.json"
cfg = json.loads(path.read_text())
cfg["behavior"]["restore_stock_on_stop"] = False
path.write_text(json.dumps(cfg, indent=2))
PY
"$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1 || true
BEFORE=$(grep -c '^reset ' "$WORK/oc.log" 2>/dev/null || true)
"$BIN/trommel" start >/dev/null 2>&1
"$BIN/trommel" stop >/dev/null 2>&1
AFTER=$(grep -c '^reset ' "$WORK/oc.log" 2>/dev/null || true)
if [ "$BEFORE" = "$AFTER" ]; then pass "stop did not call the helper when restore is off"; else fail "stop called reset although restore is off"; fi
if [ -f "$WORK/state/trommel/oc-snapshot.json" ]; then pass "snapshot kept for an explicit restore"; else fail "snapshot lost with restore off"; fi
"$BIN/trommel" status --json 2>/dev/null | python3 -c '
import json,sys
s=json.load(sys.stdin)
assert s["oc"]["needs_restore"] is True, s["oc"]
assert s["oc"]["restore_on_stop"] is False, s["oc"]
print("  needs_restore:", s["oc"]["needs_restore"])
' && pass "status reports that GPU settings are still applied" || fail "needs_restore status"
"$BIN/trommel" oc restore --mode snapshot >/dev/null 2>&1 \
    && pass "explicit restore works with restore-on-stop off" \
    || fail "explicit restore failed"
[ ! -f "$WORK/state/trommel/oc-snapshot.json" ] && pass "explicit restore cleared the snapshot" || fail "snapshot left after explicit restore"
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "config" / "trommel" / "config.json"
cfg = json.loads(path.read_text())
cfg["behavior"]["restore_stock_on_stop"] = True
path.write_text(json.dumps(cfg, indent=2))
PY
"$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1 || true

step "Helper timeout is reported, not hidden"
cat > "$WORK/bin/oc-helper" <<'SH'
#!/bin/bash
echo "$*" >> "$TROMMEL_OC_LOG"
sleep 3
exit 0
SH
chmod +x "$WORK/bin/oc-helper"
if "$BIN/trommel" start > "$WORK/timeout.out" 2>&1; then
    fail "start succeeded although the helper timed out"
else
    pass "start fails when the helper does not answer"
fi
"$BIN/trommel" status --json 2>/dev/null | python3 -c '
import json,sys
s=json.load(sys.stdin)
assert s["state"]["running"] is False, s["state"]
' && pass "no miner after a helper timeout" || fail "miner after a helper timeout"
cp "$ROOT/scripts/fixtures/fake-oc-helper.sh" "$WORK/bin/oc-helper"
chmod +x "$WORK/bin/oc-helper"
"$BIN/trommel" oc restore --mode defaults >/dev/null 2>&1 || true

step "Install sync while the daemon is running"
kill "$DAEMON_PID" 2>/dev/null || true
sleep 1
python3 - "$WORK" <<'PY'
import json, pathlib, sys
work = pathlib.Path(sys.argv[1])
p = work/"config"/"trommel"/"config.json"
cfg = json.loads(p.read_text())
cfg["miner_installs"] = {}
p.write_text(json.dumps(cfg, indent=2))
PY
start_daemon "$BIN/trommeld"
sleep 2
if "$BIN/trommel" start >/dev/null 2>&1; then
    fail "start succeeded without an installed miner"
else
    pass "start is blocked until the miner is installed"
fi
if "$BIN/trommel" miner import wildrig 0.51.8 "$WORK/bin/wildrig-multi" >/dev/null 2>&1; then
    pass "miner imported into the managed store"
else
    fail "miner import failed"
fi
sleep 1
if "$BIN/trommel" start >/dev/null 2>&1; then
    sleep 2
    if "$BIN/trommel" status --json 2>/dev/null | python3 -c 'import json,sys; s=json.load(sys.stdin); sys.exit(0 if s["state"]["running"] else 1)'; then
        pass "running daemon sees the newly imported miner"
    else
        fail "daemon did not start the newly imported miner"
    fi
else
    fail "start failed after import"
fi
COUNT=$(pgrep -fc "$WORK/data/trommel/miners/wildrig/0.51.8/wildrig-multi" 2>/dev/null || true)
[ "$COUNT" = "1" ] && pass "exactly one miner process" || fail "expected one miner, found ${COUNT:-0}"
"$BIN/trommel" stop >/dev/null 2>&1 || true

step "SIGKILL the daemon, then emergency-stop"
"$BIN/trommel" start >/dev/null 2>&1
sleep 2
MINER_PID=$(pgrep -f "$WORK/data/trommel/miners/wildrig/0.51.8/wildrig-multi" | head -1)
[ -n "$MINER_PID" ] && pass "miner is running before the SIGKILL" || fail "miner did not start"
kill -9 "$DAEMON_PID" 2>/dev/null || true
sleep 1
alive "$MINER_PID" \
    && pass "the miner survives a daemon crash (nothing kills it by accident)" \
    || fail "the miner died with the daemon"
if "$BIN/trommel" emergency-stop --reason=manual > "$WORK/emergency.out" 2>&1; then
    pass "emergency-stop exits successfully"
else
    fail "emergency-stop failed"; cat "$WORK/emergency.out"
fi
alive "$MINER_PID" \
    && fail "emergency-stop left the miner alive" \
    || pass "emergency-stop stopped the saved miner"
[ ! -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "emergency-stop cleared the snapshot" \
    || fail "snapshot survived emergency-stop"
grep -q "reset --gpu GPU-11111111.*--pl 350" "$WORK/oc.log" 2>/dev/null \
    && pass "emergency-stop restored the stock power limit" \
    || fail "emergency-stop did not restore stock"
[ ! -f "$WORK/state/trommel/state.json" ] \
    && pass "state cleared after the miner was stopped" \
    || fail "state file survived emergency-stop"
if "$BIN/trommel" emergency-stop --reason=manual > "$WORK/emergency2.out" 2>&1; then
    pass "emergency-stop is idempotent"
else
    fail "the second emergency-stop failed"; cat "$WORK/emergency2.out"
fi
start_daemon "$BIN/trommeld"
sleep 2

step "SIGKILL during an in-flight helper waits, then restores"
kill -9 "$DAEMON_PID" 2>/dev/null || true
sleep 1
start_daemon env TROMMEL_OC_TIMEOUT_MS=30000 "$BIN/trommeld"
sleep 2
cat > "$WORK/bin/oc-helper" <<SH
#!/bin/bash
echo "\$*" >> "\$TROMMEL_OC_LOG"
clean=()
lease_pid=""
lease_start=""
while [ \$# -gt 0 ]; do
    case "\$1" in
        --lease-pid) lease_pid="\$2"; shift 2 ;;
        --lease-start) lease_start="\$2"; shift 2 ;;
        *) clean+=("\$1"); shift ;;
    esac
done
if [ "\${clean[0]:-}" = apply ]; then
    [ -n "\$lease_pid" ] && [ -n "\$lease_start" ] || exit 3
    stat=\$(<"/proc/\$lease_pid/stat")
    rest=\${stat##*') '}
    now=\$(echo "\$rest" | awk '{print \$20}')
    [ "\$now" = "\$lease_start" ] || exit 3
    sleep "\${TROMMEL_FAKE_HELPER_SLEEP:-6}"
fi
exec "$ROOT/scripts/fixtures/fake-oc-helper.sh" "\${clean[@]}"
SH
chmod +x "$WORK/bin/oc-helper"
"$BIN/trommel" start > "$WORK/late-start.out" 2>&1 &
START_CLIENT=$!
sleep 2
kill -9 "$DAEMON_PID" 2>/dev/null || true
sleep 1
STARTED=$(date +%s)
if "$BIN/trommel" emergency-stop --reason=manual > "$WORK/late-emergency.out" 2>&1; then
    pass "emergency-stop waited for the in-flight helper"
else
    fail "emergency-stop failed during the helper"; cat "$WORK/late-emergency.out"
fi
ELAPSED=$(( $(date +%s) - STARTED ))
[ "$ELAPSED" -ge 3 ] \
    && pass "emergency-stop did not restore before the helper finished (${ELAPSED}s)" \
    || fail "emergency-stop restored too early (${ELAPSED}s)"
"$WORK/bin/nvidia-smi" --query-gpu=power.limit --format=csv,noheader,nounits -i "GPU-11111111-1111-1111-1111-111111111111" > "$WORK/late-pl.txt" 2>/dev/null || true
grep -q '^350' "$WORK/late-pl.txt" \
    && pass "the late helper did not overwrite the restore" \
    || fail "the late helper overwrote the restore (PL $(cat "$WORK/late-pl.txt" 2>/dev/null))"
[ ! -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "snapshot cleared after the late-helper restore" \
    || fail "snapshot survived the late-helper restore"
wait "$START_CLIENT" 2>/dev/null || true

step "SIGKILL during a stuck helper terminates it, then restores"
cat > "$WORK/bin/oc-helper" <<SH
#!/bin/bash
echo "\$*" >> "\$TROMMEL_OC_LOG"
clean=()
lease_pid=""
lease_start=""
while [ \$# -gt 0 ]; do
    case "\$1" in
        --lease-pid) lease_pid="\$2"; shift 2 ;;
        --lease-start) lease_start="\$2"; shift 2 ;;
        *) clean+=("\$1"); shift ;;
    esac
done
if [ "\${clean[0]:-}" = apply ]; then
    [ -n "\$lease_pid" ] && [ -n "\$lease_start" ] || exit 3
    stat=\$(<"/proc/\$lease_pid/stat")
    rest=\${stat##*') '}
    now=\$(echo "\$rest" | awk '{print \$20}')
    [ "\$now" = "\$lease_start" ] || exit 3
    sleep 14
fi
exec "$ROOT/scripts/fixtures/fake-oc-helper.sh" "\${clean[@]}"
SH
chmod +x "$WORK/bin/oc-helper"
start_daemon env TROMMEL_OC_TIMEOUT_MS=30000 "$BIN/trommeld"
sleep 2
"$BIN/trommel" start > "$WORK/stuck-start.out" 2>&1 &
START_CLIENT=$!
sleep 2
kill -9 "$DAEMON_PID" 2>/dev/null || true
sleep 1
STARTED=$(date +%s)
if "$BIN/trommel" emergency-stop --reason=manual > "$WORK/stuck-emergency.out" 2>&1; then
    pass "emergency-stop handled the stuck helper"
else
    fail "emergency-stop failed on the stuck helper"; cat "$WORK/stuck-emergency.out"
fi
ELAPSED=$(( $(date +%s) - STARTED ))
[ "$ELAPSED" -ge 9 ] \
    && pass "emergency-stop waited the lease timeout before killing (${ELAPSED}s)" \
    || fail "emergency-stop did not wait for the stuck helper (${ELAPSED}s)"
"$WORK/bin/nvidia-smi" --query-gpu=power.limit --format=csv,noheader,nounits -i "GPU-11111111-1111-1111-1111-111111111111" > "$WORK/stuck-pl.txt" 2>/dev/null || true
grep -q '^350' "$WORK/stuck-pl.txt" \
    && pass "the killed helper never applied its late tuning" \
    || fail "the killed helper changed the power limit (PL $(cat "$WORK/stuck-pl.txt" 2>/dev/null))"
sleep 5
"$WORK/bin/nvidia-smi" --query-gpu=power.limit --format=csv,noheader,nounits -i "GPU-11111111-1111-1111-1111-111111111111" > "$WORK/stuck-pl2.txt" 2>/dev/null || true
grep -q '^350' "$WORK/stuck-pl2.txt" \
    && pass "no delayed apply appeared after the kill" \
    || fail "a delayed apply changed the power limit after the kill"
wait "$START_CLIENT" 2>/dev/null || true
[ ! -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "snapshot cleared after the stuck-helper restore" \
    || fail "snapshot survived the stuck-helper restore"
cp "$ROOT/scripts/fixtures/fake-oc-helper.sh" "$WORK/bin/oc-helper"
chmod +x "$WORK/bin/oc-helper"
start_daemon "$BIN/trommeld"
sleep 2

step "Startup recovery stops the orphan and restores stock"
"$BIN/trommel" start >/dev/null 2>&1
sleep 2
MINER_PID=$(pgrep -f "$WORK/data/trommel/miners/wildrig/0.51.8/wildrig-multi" | head -1)
[ -n "$MINER_PID" ] && pass "miner is running before the crash" || fail "miner did not start"
kill -9 "$DAEMON_PID" 2>/dev/null || true
sleep 1
alive "$MINER_PID" \
    && pass "the orphaned miner is still alive when the new daemon starts" \
    || fail "the miner died before startup recovery"
start_daemon "$BIN/trommeld"
sleep 4
alive "$MINER_PID" \
    && fail "startup recovery left the orphaned miner running" \
    || pass "startup recovery stopped the orphaned miner"
[ ! -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "startup recovery cleared the snapshot" \
    || fail "snapshot survived startup recovery"
if "$BIN/trommel" status --json 2>/dev/null | python3 -c 'import json,sys; s=json.load(sys.stdin); sys.exit(0 if not s["state"]["running"] else 1)'; then
    pass "the restarted daemon reports a stopped session"
else
    fail "startup recovery resumed mining"
fi
if "$BIN/trommel" start >/dev/null 2>&1; then
    sleep 1
    "$BIN/trommel" stop >/dev/null 2>&1
    pass "start works again after startup recovery"
else
    fail "start failed after startup recovery"
fi

step "Stale external state is cleared"
python3 - "$WORK" <<'PY'
import json, pathlib, sys, time
work = pathlib.Path(sys.argv[1])
state = {"running": True, "miner_pid": 4000000, "started_at": int(time.time()) - 600,
         "restarts": 0, "last_error": None, "external": True, "orphan": False,
         "preset_id": "pearl-wildrig", "miner": "wildrig", "miner_version": "0.51.8"}
(work/"state"/"trommel"/"state.json").write_text(json.dumps(state, indent=2))
PY
kill "$DAEMON_PID" 2>/dev/null || true
sleep 1
start_daemon "$BIN/trommeld"
sleep 2
if "$BIN/trommel" status --json 2>/dev/null | python3 -c 'import json,sys; s=json.load(sys.stdin); sys.exit(0 if not s["state"]["running"] and not s["state"]["external"] else 1)'; then
    pass "phantom external state is gone"
else
    fail "phantom external state survived normalize"
fi
if "$BIN/trommel" start >/dev/null 2>&1; then
    sleep 1
    pass "start works after the stale state was cleared"
    "$BIN/trommel" stop >/dev/null 2>&1 || true
else
    fail "start still blocked by stale state"
fi

step "A custom-named miner is stopped by path"
cat > "$WORK/bin/my-custom-rig" <<'SH'
#!/bin/bash
echo "[12:00:00] hashrate: 10s: 90.0 60s: 90.0 15m: 90.0 TH/s"
echo "[12:00:00] Shares: 100 accepted, 0 rejected"
while true; do sleep 2; done
SH
chmod +x "$WORK/bin/my-custom-rig"
"$BIN/trommel" miner import wildrig 9.9.9 "$WORK/bin/my-custom-rig" >/dev/null 2>&1 \
    && pass "custom-named binary imported" || fail "custom binary import failed"
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "config" / "trommel" / "config.json"
cfg = json.loads(path.read_text())
cfg["presets"]["pearl-wildrig"]["miner_version"] = "9.9.9"
path.write_text(json.dumps(cfg, indent=2))
PY
"$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1
sleep 1
"$BIN/trommel" start >/dev/null 2>&1
sleep 2
CUSTOM_PID=$(pgrep -f "$WORK/data/trommel/miners/wildrig/9.9.9/my-custom-rig" | head -1)
[ -n "$CUSTOM_PID" ] && pass "the custom-named miner is running (PID $CUSTOM_PID)" \
    || fail "the custom-named miner did not start"
kill -9 "$DAEMON_PID" 2>/dev/null || true
sleep 1
"$BIN/trommel" emergency-stop --reason=manual >/dev/null 2>&1
alive "$CUSTOM_PID" \
    && fail "emergency-stop did not stop the custom-named miner" \
    || pass "emergency-stop stopped the custom-named miner by its saved path"
[ ! -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "custom binary restore cleared the snapshot" \
    || fail "snapshot survived the custom binary emergency stop"
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "config" / "trommel" / "config.json"
cfg = json.loads(path.read_text())
cfg["presets"]["pearl-wildrig"]["miner_version"] = "0.51.8"
path.write_text(json.dumps(cfg, indent=2))
PY
start_daemon "$BIN/trommeld"
sleep 2
"$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1

step "Partial apply rolls back and keeps a retryable snapshot"
cat > "$WORK/bin/oc-helper" <<SH
#!/bin/bash
echo "\$*" >> "\$TROMMEL_OC_LOG"
cmd="\${1:-}"
gpu=""
args=("\$@")
for ((i=0; i<\${#args[@]}; i++)); do
    [ "\${args[\$i]}" = "--gpu" ] && gpu="\${args[\$((i+1))]}"
done
if [ "\$cmd" = apply ] && [ "\$gpu" = "GPU-22222222-2222-2222-2222-222222222222" ]; then
    exit 1
fi
exec "$ROOT/scripts/fixtures/fake-oc-helper.sh" "\$@"
SH
chmod +x "$WORK/bin/oc-helper"
if "$BIN/trommel" start >/dev/null 2>&1; then
    fail "start succeeded although the second card failed"
else
    pass "a partial apply aborts the start"
fi
grep -q "reset --gpu GPU-11111111.*--pl 350" "$WORK/oc.log" 2>/dev/null \
    && pass "the first card was rolled back" \
    || fail "the first card was not rolled back"
[ ! -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "a successful rollback clears the snapshot" \
    || fail "snapshot left after a successful rollback"
# Now make the rollback itself fail: the snapshot must survive for a retry.
cat > "$WORK/bin/oc-helper" <<SH
#!/bin/bash
echo "\$*" >> "\$TROMMEL_OC_LOG"
cmd="\${1:-}"
gpu=""
args=("\$@")
for ((i=0; i<\${#args[@]}; i++)); do
    [ "\${args[\$i]}" = "--gpu" ] && gpu="\${args[\$((i+1))]}"
done
if [ "\$cmd" = apply ] && [ "\$gpu" = "GPU-22222222-2222-2222-2222-222222222222" ]; then
    exit 1
fi
if [ "\$cmd" = reset ] && [ "\$gpu" = "GPU-11111111-1111-1111-1111-111111111111" ]; then
    exit 1
fi
exec "$ROOT/scripts/fixtures/fake-oc-helper.sh" "\$@"
SH
chmod +x "$WORK/bin/oc-helper"
if "$BIN/trommel" start >/dev/null 2>&1; then
    fail "start succeeded although apply and rollback failed"
else
    pass "a failed rollback keeps the start aborted"
fi
if [ -f "$WORK/state/trommel/oc-snapshot.json" ]; then
    pass "the snapshot is kept when the rollback fails"
else
    fail "snapshot lost after a failed rollback"
fi
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "state" / "trommel" / "oc-snapshot.json"
snap = json.loads(path.read_text())
assert snap["applied"] is True, snap
assert snap.get("last_error"), snap
print("  retryable snapshot, last_error:", snap["last_error"][:60])
PY
[ $? -eq 0 ] && pass "the snapshot records the rollback error" || fail "no last_error on the snapshot"
cp "$ROOT/scripts/fixtures/fake-oc-helper.sh" "$WORK/bin/oc-helper"
chmod +x "$WORK/bin/oc-helper"
"$BIN/trommel" emergency-stop --reason=manual >/dev/null 2>&1
[ ! -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "emergency-stop retried the partial apply restore" \
    || fail "emergency-stop could not finish the partial apply"
"$BIN/trommel" oc restore --mode defaults >/dev/null 2>&1 || true

step "A failed restore is repeated by startup recovery"
"$BIN/trommel" start >/dev/null 2>&1
sleep 1
cat > "$WORK/bin/oc-helper" <<SH
#!/bin/bash
echo "\$*" >> "\$TROMMEL_OC_LOG"
if [ "\${1:-}" = reset ]; then exit 1; fi
exec "$ROOT/scripts/fixtures/fake-oc-helper.sh" "\$@"
SH
chmod +x "$WORK/bin/oc-helper"
if "$BIN/trommel" stop >/dev/null 2>&1; then
    fail "stop reported success although the restore failed"
else
    pass "stop reports a failed restore"
fi
[ -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "the failed restore keeps the snapshot" \
    || fail "snapshot lost after a failed restore"
kill -9 "$DAEMON_PID" 2>/dev/null || true
sleep 1
cp "$ROOT/scripts/fixtures/fake-oc-helper.sh" "$WORK/bin/oc-helper"
chmod +x "$WORK/bin/oc-helper"
BEFORE=$(grep -c '^reset ' "$WORK/oc.log" 2>/dev/null || true)
start_daemon "$BIN/trommeld"
sleep 4
AFTER=$(grep -c '^reset ' "$WORK/oc.log" 2>/dev/null || true)
[ "$AFTER" -gt "$BEFORE" ] \
    && pass "startup recovery retried the failed restore" \
    || fail "startup recovery did not retry the restore"
[ ! -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "startup recovery cleared the retried snapshot" \
    || fail "snapshot survived the successful retry"

step "An external miner is not stopped by emergency-stop"
kill -9 "$DAEMON_PID" 2>/dev/null || true
sleep 1
cat > "$WORK/bin/external-rig" <<'SH'
#!/bin/bash
while true; do sleep 2; done
SH
chmod +x "$WORK/bin/external-rig"
setsid "$WORK/bin/external-rig" --algo pearlhash >/dev/null 2>&1 < /dev/null &
EXT_PID=$!
sleep 1
python3 - "$WORK" "$EXT_PID" "$WORK/bin/external-rig" <<'PY'
import json, pathlib, sys, time
work = pathlib.Path(sys.argv[1])
state = {"running": True, "miner_pid": int(sys.argv[2]), "miner_path": sys.argv[3],
         "started_at": int(time.time()), "restarts": 0, "last_error": None,
         "external": True, "orphan": False, "preset_id": "pearl-wildrig",
         "miner": "wildrig", "miner_version": "0.51.8"}
(work/"state"/"trommel"/"state.json").write_text(json.dumps(state, indent=2))
PY
"$BIN/trommel" emergency-stop --reason=manual >/dev/null 2>&1
alive "$EXT_PID" \
    && pass "emergency-stop left the external miner alive" \
    || fail "emergency-stop killed an external miner"
[ ! -f "$WORK/state/trommel/state.json" ] \
    && pass "external state is not claimed by this installation" \
    || fail "external state survived emergency-stop"
kill "$EXT_PID" 2>/dev/null || true
start_daemon "$BIN/trommeld"
sleep 2

step "A hot second card triggers the overheat alert"
if [ "$NOTIFY_STUB" = 1 ]; then
    : > "$NOTIFY_LOG"
    python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "gpu-state.json"
state = json.loads(path.read_text())
state["gpus"][0]["temp"] = 60
state["gpus"][1]["temp"] = 95
path.write_text(json.dumps(state, indent=2))
PY
    sleep 4
    if grep -q 'card #1' "$NOTIFY_LOG" 2>/dev/null; then
        pass "the alert names the hot second card"
    else
        fail "overheat alert ignored the hot second card"
        cat "$NOTIFY_LOG" 2>/dev/null || true
    fi
    if "$BIN/trommel" status --json 2>/dev/null | python3 -c '
import json, sys
s = json.load(sys.stdin)
assert s["total"]["temp_max_c"] == 95, s["total"]
assert any("GPU #1" in w for w in s["warnings"]), s["warnings"]
'; then
        pass "the status note names the hot second card"
    else
        fail "status note or max temperature ignored the hot second card"
    fi
else
    echo "  - notification stub not running; overheat routing not exercised"
fi

step "Desktop notifications"

if [ "$NOTIFY_STUB" = 1 ]; then
    if grep -q 'Trommel' "$NOTIFY_LOG" 2>/dev/null; then
        pass "daemon delivers desktop notifications without an applet"
    else
        fail "no org.freedesktop.Notifications call reached the stub"
    fi
else
    echo "  - notification stub not running; desktop delivery not exercised"
fi

step "Benchmark refuses to run next to an external miner"
kill -9 "$DAEMON_PID" 2>/dev/null || true
sleep 1
cat > "$WORK/bin/wildrig-external" <<'SH'
#!/bin/bash
while true; do sleep 2; done
SH
chmod +x "$WORK/bin/wildrig-external"
setsid "$WORK/bin/wildrig-external" --algo pearlhash >/dev/null 2>&1 < /dev/null &
EXT_PID=$!
sleep 1
start_daemon env -u TROMMEL_NO_EXTERNAL_SCAN "$BIN/trommeld"
sleep 3
if "$BIN/trommel" bench --minutes 1 > "$WORK/bench-external.out" 2>&1; then
    fail "benchmark ran next to an external miner"
else
    pass "benchmark is refused while an external miner runs"
fi
alive "$EXT_PID" \
    && pass "the external miner is still alive after the refused benchmark" \
    || fail "the refused benchmark killed the external miner"
COUNT=$(pgrep -fc "$WORK/bin/wildrig-external" 2>/dev/null || true)
[ "$COUNT" = "1" ] && pass "no second benchmark process was spawned" \
    || fail "expected one external process, found ${COUNT:-0}"
kill -9 "$DAEMON_PID" 2>/dev/null || true
kill "$EXT_PID" 2>/dev/null || true
sleep 1
start_daemon "$BIN/trommeld"
sleep 2

step "Restart budget stops mining and restores"
cat > "$WORK/bin/flaky-miner" <<'SH'
#!/bin/bash
exit 1
SH
chmod +x "$WORK/bin/flaky-miner"
"$BIN/trommel" miner import wildrig 8.8.8 "$WORK/bin/flaky-miner" >/dev/null 2>&1 \
    && pass "flaky miner imported" || fail "flaky miner import failed"
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "config" / "trommel" / "config.json"
cfg = json.loads(path.read_text())
cfg["presets"]["pearl-wildrig"]["miner_version"] = "8.8.8"
path.write_text(json.dumps(cfg, indent=2))
PY
"$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1
kill -9 "$DAEMON_PID" 2>/dev/null || true
sleep 1
start_daemon env TROMMEL_MAX_RESTARTS=2 "$BIN/trommeld"
sleep 2
"$BIN/trommel" start >/dev/null 2>&1
sleep 12
JSON=$("$BIN/trommel" status --json 2>/dev/null)
echo "$JSON" | python3 -c '
import json,sys
s=json.load(sys.stdin)
assert s["state"]["running"] is False, s["state"]
assert s["state"]["restarts"] == 2, s["state"]
err=(s["state"].get("last_error") or "").lower()
assert "budget" in err or "лимит" in err, err
print("  stopped after", s["state"]["restarts"], "restarts:", s["state"]["last_error"][:60])
' && pass "bounded restart stopped after two restarts" || fail "restart budget"
[ ! -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "the exhausted restart budget restored stock" \
    || fail "snapshot left after the restart budget"
grep -q "reset --gpu GPU-11111111.*--pl 350" "$WORK/oc.log" 2>/dev/null \
    && pass "reset was called after the restart budget" \
    || fail "no reset after the restart budget"
python3 - "$WORK" <<'PY'
import json, pathlib, sys
path = pathlib.Path(sys.argv[1]) / "config" / "trommel" / "config.json"
cfg = json.loads(path.read_text())
cfg["presets"]["pearl-wildrig"]["miner_version"] = "0.51.8"
path.write_text(json.dumps(cfg, indent=2))
PY
kill -9 "$DAEMON_PID" 2>/dev/null || true
sleep 1
start_daemon "$BIN/trommeld"
sleep 2
"$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1

step "The guard restores after a SIGKILLed daemon"
kill -9 "$DAEMON_PID" 2>/dev/null || true
sleep 1
rm -f "$WORK/state/trommel/guard.pid"
start_daemon env -u TROMMEL_NO_GUARD "$BIN/trommeld"
sleep 3
"$BIN/trommel" start >/dev/null 2>&1
sleep 2
MINER_PID=$(pgrep -f "$WORK/data/trommel/miners/wildrig/0.51.8/wildrig-multi" | head -1)
GUARD_PID=$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["pid"])' "$WORK/state/trommel/guard.pid" 2>/dev/null || true)
[ -n "$MINER_PID" ] && pass "miner is running under a guarded daemon" || fail "miner did not start"
[ -n "$GUARD_PID" ] && pass "guard process is recorded (PID $GUARD_PID)" || fail "guard PID file missing"
kill -9 "$DAEMON_PID" 2>/dev/null || true
for _ in $(seq 1 40); do
    if ! alive "$MINER_PID" && [ ! -f "$WORK/state/trommel/oc-snapshot.json" ]; then
        break
    fi
    sleep 0.5
done
alive "$MINER_PID" \
    && fail "the guard did not stop the orphaned miner" \
    || pass "the guard stopped the orphaned miner"
[ ! -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "the guard cleared the snapshot" \
    || fail "snapshot survived the guard restore"
grep -q "emergency-stop reason=guard" "$WORK/state/trommel/daemon.log" 2>/dev/null \
    && pass "the guard logged its emergency reason" \
    || fail "no guard emergency-stop in the daemon log"
sleep 1
if [ -n "$GUARD_PID" ]; then
    alive "$GUARD_PID" \
        && fail "the guard is still running after the restore" \
        || pass "the guard exits after restoring"
fi
start_daemon "$BIN/trommeld"
sleep 2

step "A GUI crash does not stop mining"
"$BIN/trommel" start >/dev/null 2>&1
sleep 2
GUI_PID=""
if [ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]; then
    setsid "$BIN/trommel" gui > "$WORK/gui.out" 2>&1 < /dev/null &
    sleep 4
    GUI_PID=$(pgrep -f "$BIN/trommel gui" | head -1)
fi
if [ -n "$GUI_PID" ]; then
    BEFORE=$(grep -c '^reset ' "$WORK/oc.log" 2>/dev/null || true)
    kill -9 "$GUI_PID" 2>/dev/null || true
    sleep 2
    if "$BIN/trommel" status --json 2>/dev/null | python3 -c 'import json,sys; s=json.load(sys.stdin); sys.exit(0 if s["state"]["running"] else 1)'; then
        pass "mining continues after the GUI is SIGKILLed"
    else
        fail "a GUI crash stopped mining"
    fi
    AFTER=$(grep -c '^reset ' "$WORK/oc.log" 2>/dev/null || true)
    [ "$BEFORE" = "$AFTER" ] \
        && pass "a GUI crash does not trigger a restore" \
        || fail "the GUI crash triggered a restore"
else
    echo "  - no usable display; GUI crash is covered by ui-smoke/manual matrix"
fi
[ -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "the mining snapshot is still present after the GUI crash" \
    || fail "snapshot disappeared after the GUI crash"
"$BIN/trommel" stop >/dev/null 2>&1

step "A lost session bus stops and restores"
"$BIN/trommel" start >/dev/null 2>&1
sleep 2
MINER_PID=$(pgrep -f "$WORK/data/trommel/miners/wildrig/0.51.8/wildrig-multi" | head -1)
kill "$BUS_PID" 2>/dev/null || true
for _ in $(seq 1 40); do
    if ! alive "$DAEMON_PID" && ! alive "$MINER_PID"; then break; fi
    sleep 0.5
done
alive "$DAEMON_PID" \
    && fail "the daemon survived losing the session bus" \
    || pass "the daemon exits when the session bus closes"
alive "$MINER_PID" \
    && fail "the miner survived a lost session bus" \
    || pass "the miner is stopped after the session bus closes"
[ ! -f "$WORK/state/trommel/oc-snapshot.json" ] \
    && pass "the lost-bus shutdown restored stock" \
    || fail "snapshot survived a lost session bus"

echo
if [ "$FAILS" -eq 0 ]; then
    printf '\033[1;32mALL GREEN\033[0m\n'
    exit 0
else
    printf '\033[1;31mFAILURES: %s\033[0m\n' "$FAILS"
    exit 1
fi
