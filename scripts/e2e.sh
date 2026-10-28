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

step "Build"
( cd "$ROOT" && cargo build -q -p trommeld -p trommel ) || { echo "build failed" >&2; exit 1; }

step "Extension source"
EXT_JS="$ROOT/extension/trommel@nayvok.github.io/extension.js"
if grep -q 'this\.menu' "$EXT_JS"; then
    fail "extension uses Extension.menu instead of the panel button menu"
else
    pass "extension menu comes from the panel button"
fi
if grep -q 'this\._indicator\.menu' "$EXT_JS"; then
    pass "panel menu is wired through this._indicator.menu"
else
    fail "panel menu wiring is missing"
fi

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

cat > "$WORK/bin/wildrig-multi" <<'SH'
#!/bin/bash
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
    "restore_stock_on_stop":True,"start_mining_on_login":False,"notifications":False,"language":"en"}
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
    [ -n "${DAEMON_PID:-}" ] && kill "$DAEMON_PID" 2>/dev/null || true
    [ -n "$BUS_PID" ] && kill "$BUS_PID" 2>/dev/null || true
}
trap cleanup EXIT

step "Root helper"
if "$ROOT/scripts/test-oc-helper.sh" > "$WORK/helper-test.out" 2>&1; then
    pass "polkit helper verification tests"
else
    fail "polkit helper verification tests"
    tail -20 "$WORK/helper-test.out"
fi

step "Start daemon"
setsid "$BIN/trommeld" > "$WORK/daemon.out" 2>&1 < /dev/null &
DAEMON_PID=$!
sleep 2
if kill -0 "$DAEMON_PID" 2>/dev/null; then pass "trommeld is running"; else fail "trommeld did not start"; cat "$WORK/daemon.out"; exit 1; fi

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

step "Preset commands"
"$BIN/trommel" preset list | grep -q "pearl-wildrig" && pass "preset list" || fail "preset list"
"$BIN/trommel" preset use pearl-wildrig --stop >/dev/null 2>&1 && pass "preset use --stop" || fail "preset use --stop"
"$BIN/trommel" miner list | grep -q "installed: 0.51.8" && pass "miner install state" || fail "miner list"

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
setsid "$BIN/trommeld" > "$WORK/daemon.out" 2>&1 < /dev/null &
DAEMON_PID=$!
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

step "Daemon restart adopts the miner and auto-restarts"
"$BIN/trommel" start >/dev/null 2>&1
sleep 2
MINER_PID=$(pgrep -f "$WORK/data/trommel/miners/wildrig/0.51.8/wildrig-multi" | head -1)
[ -n "$MINER_PID" ] && pass "miner is running before the daemon restart" || fail "miner did not start"
kill "$DAEMON_PID" 2>/dev/null || true
sleep 1
setsid "$BIN/trommeld" > "$WORK/daemon.out" 2>&1 < /dev/null &
DAEMON_PID=$!
sleep 2
if "$BIN/trommel" status --json 2>/dev/null | python3 -c 'import json,sys; s=json.load(sys.stdin); sys.exit(0 if s["state"]["running"] else 1)'; then
    pass "restarted daemon still reports the miner as running"
else
    fail "restarted daemon lost the running miner"
fi
kill "$MINER_PID" 2>/dev/null || true
sleep 5
NEW_PID=$(pgrep -f "$WORK/data/trommel/miners/wildrig/0.51.8/wildrig-multi" | head -1)
if [ -n "$NEW_PID" ] && [ "$NEW_PID" != "$MINER_PID" ]; then
    pass "orphan was auto-restarted with a new PID"
else
    fail "orphan was not auto-restarted (old=$MINER_PID new=${NEW_PID:-none})"
fi
"$BIN/trommel" stop >/dev/null 2>&1 || true

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
setsid "$BIN/trommeld" > "$WORK/daemon.out" 2>&1 < /dev/null &
DAEMON_PID=$!
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

echo
if [ "$FAILS" -eq 0 ]; then
    printf '\033[1;32mALL GREEN\033[0m\n'
    exit 0
else
    printf '\033[1;31mFAILURES: %s\033[0m\n' "$FAILS"
    exit 1
fi
