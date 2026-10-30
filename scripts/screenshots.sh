#!/bin/bash
# Generate documentation screenshots with a fake two-GPU rig.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$ROOT/.shots"
BIN="$ROOT/target/debug"
DOCS="$ROOT/docs/images"
mkdir -p "$DOCS"
if [ "${TROMMEL_SKIP_BUILD:-0}" != 1 ]; then
    ( cd "$ROOT" && cargo build -q -p trommel --features gui -p trommeld )
fi
[ -x "$BIN/trommel" ] || { echo "build first: cargo build -p trommel --features gui -p trommeld" >&2; exit 1; }
[ -x "$BIN/trommeld" ] || { echo "build first" >&2; exit 1; }

rm -rf "$WORK"
mkdir -p "$WORK/bin" "$WORK/config/trommel" "$WORK/state/trommel" "$WORK/cache"

# Stateful fake rig shared with the e2e/ui-smoke harness: the daemon must be
# able to snapshot stock, apply the profile, read the power back and verify the
# clock locks. A static CSV cannot pass that verification.
cp "$ROOT/scripts/fixtures/fake-nvidia-smi.py" "$WORK/bin/nvidia-smi"
cp "$ROOT/scripts/fixtures/fake-oc-helper.sh" "$WORK/bin/oc-helper"
chmod +x "$WORK/bin/nvidia-smi" "$WORK/bin/oc-helper"
: > "$WORK/fake-mode"
cat > "$WORK/gpu-state.json" <<'JSON'
{
  "gpus": [
    {"index": 0, "uuid": "GPU-11111111-1111-1111-1111-111111111111", "pci_bus_id": "00000000:01:00.0", "name": "NVIDIA GeForce RTX 3080 Ti",
     "temp": 61, "fan": 68, "draw": 301.4, "power": 350.0, "power_default": 350.0,
     "power_min": 100.0, "power_max": 400.0, "core": 1770, "core_default": 1770,
     "mem": 9501, "mem_default": 9501, "util": 100, "vram": 8472},
    {"index": 1, "uuid": "GPU-22222222-2222-2222-2222-222222222222", "pci_bus_id": "00000000:02:00.0", "name": "NVIDIA GeForce RTX 3060",
     "temp": 55, "fan": 62, "draw": 128.2, "power": 170.0, "power_default": 170.0,
     "power_min": 60.0, "power_max": 200.0, "core": 1500, "core_default": 1500,
     "mem": 7000, "mem_default": 7000, "util": 99, "vram": 4096}
  ]
}
JSON

cat > "$WORK/wildrig-multi" <<'SH'
#!/bin/bash
while true; do sleep 2; done
SH
chmod +x "$WORK/wildrig-multi"
# The preset editor shows the install path. Keep the demo rig out of the
# repository path so the tracked screenshot never contains a personal home
# directory.
DEMO_MINER="/tmp/trommel-screenshot-wildrig-multi"
cp "$WORK/wildrig-multi" "$DEMO_MINER"
chmod +x "$DEMO_MINER"
# A second installed build keeps the version manager visible: the backup preset
# pins this older version, so the library shows usage, verify/remove and the
# per-preset version switch.
DEMO_MINER_OLD="/tmp/trommel-screenshot-wildrig-multi-0.51.7"
cp "$WORK/wildrig-multi" "$DEMO_MINER_OLD"
chmod +x "$DEMO_MINER_OLD"
for i in $(seq 1 80); do
  echo "[12:00:$((i%60))] hashrate: 10s: 125.$((i%9)) 60s: 125.0 15m: 124.9 TH/s" >> "$WORK/state/trommel/miner.log"
  echo "[12:00:$((i%60))] Shares: $((500+i)) accepted, 2 rejected" >> "$WORK/state/trommel/miner.log"
done
python3 - "$WORK" "$DEMO_MINER" "$DEMO_MINER_OLD" <<'PY'
import json, pathlib, sys
work = pathlib.Path(sys.argv[1]); demo_miner = sys.argv[2]
demo_miner_old = sys.argv[3]
cfg = {
  "schema_version": 2, "active_preset": "pearl-wildrig",
  "presets": {"pearl-wildrig": {
    "id":"pearl-wildrig","name":"Pearl · WildRig","coin":"pearl","algo":"pearlhash",
    "miner":"wildrig","miner_version":"0.51.8","pool":"pearlhash",
    "pool_url":"pool.pearlhash.xyz:9000","wallet":"prl1testwallet","worker":"3080ti",
    "gpu_profile":"balanced","api_port":14029,"expected_hashrate":125000000000000.0,
    "miner_options":{"fan_speed":70},"miner_fee_override_pct":None,"miner_fee_pct":0.0,
    "pool_fee_pct":1.0,"extra_args":"","env":{},"created_at":0,"updated_at":0},
   "pearl-backup": {
    "id":"pearl-backup","name":"Pearl backup","coin":"pearl","algo":"pearlhash",
    "miner":"wildrig","miner_version":"0.51.7","pool":"pearlhash",
    "pool_url":"pool.pearlhash.xyz:9000","wallet":"prl1testwallet","worker":"backup",
    "gpu_profile":"balanced","api_port":14029,"expected_hashrate":125000000000000.0,
    "miner_options":{},"miner_fee_override_pct":None,"miner_fee_pct":0.0,
    "pool_fee_pct":1.0,"extra_args":"","env":{},"created_at":0,"updated_at":0}},
  "gpu_profiles": {"balanced": {"name":"Balanced",
    "default":{"enabled":True,"power_limit_w":None,"core_clock_mhz":None,
               "memory_clock_mhz":None,"fan_pct":None,"temp_limit_c":90,"temp_resume_c":65},
    "per_gpu":{
      "GPU-11111111-1111-1111-1111-111111111111":{"enabled":True,"power_limit_w":300,
        "core_clock_mhz":1620,"memory_clock_mhz":810,"fan_pct":None,"temp_limit_c":90,"temp_resume_c":65},
      "GPU-22222222-2222-2222-2222-222222222222":{"enabled":True,"power_limit_w":130,
        "core_clock_mhz":1500,"memory_clock_mhz":750,"fan_pct":None,"temp_limit_c":90,"temp_resume_c":65}}}},
  "miner_installs": {
    "wildrig@0.51.8": {"miner":"wildrig","version":"0.51.8",
      "path":demo_miner,"sha256":"","source_url":"","verified":False,"custom":True,"installed_at":0},
    "wildrig@0.51.7": {"miner":"wildrig","version":"0.51.7",
      "path":demo_miner_old,"sha256":"","source_url":"","verified":False,"custom":True,"installed_at":0}},
  "custom_miners": {}, "pools": {},
  "behavior": {"auto_restart":True,"restart_delay_s":5,"poll_interval_s":2,
    "restore_stock_on_stop":True,"start_mining_on_login":False,"notifications":False,"language":"en"}
}
(work/"config"/"trommel"/"config.json").write_text(json.dumps(cfg, indent=2))
PY

cleanup() {
  [ -n "${JITTER_PID:-}" ] && kill "$JITTER_PID" 2>/dev/null || true
  [ -n "${API_PID:-}" ] && kill "$API_PID" 2>/dev/null || true
  [ -n "${DAEMON_PID:-}" ] && kill "$DAEMON_PID" 2>/dev/null || true
  [ -n "${BUS_PID:-}" ] && kill "$BUS_PID" 2>/dev/null || true
  pkill -f "${DEMO_MINER:-/tmp/trommel-screenshot-wildrig-multi}" 2>/dev/null || true
  rm -f "${DEMO_MINER:-/tmp/trommel-screenshot-wildrig-multi}"
  rm -f "${DEMO_MINER_OLD:-/tmp/trommel-screenshot-wildrig-multi-0.51.7}"
}
trap cleanup EXIT

# A private session bus keeps an already running Trommel (or a stale test
# daemon) from answering our D-Bus calls and polluting the screenshots.
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
BUS_OUT="$(dbus-daemon --config-file="$WORK/bus.conf" --fork --print-address=1 --print-pid=1)"
export DBUS_SESSION_BUS_ADDRESS="$(printf '%s\n' "$BUS_OUT" | sed -n 1p)"
BUS_PID="$(printf '%s\n' "$BUS_OUT" | sed -n 2p)" 

export PATH="$WORK/bin:$PATH"
export XDG_CONFIG_HOME="$WORK/config"
export XDG_STATE_HOME="$WORK/state"
export XDG_CACHE_HOME="$WORK/cache"
# The demo rig is the only miner: do not adopt a real one from the host, and
# do not leave a guard behind when the screenshot daemon is killed.
export TROMMEL_NO_EXTERNAL_SCAN=1
export TROMMEL_NO_TRAY=1
export TROMMEL_AUTOSTART_STATE=on
export TROMMEL_NO_GUARD=1
export TROMMEL_OC_HELPER="$WORK/bin/oc-helper"
export TROMMEL_OC_LOG="$WORK/oc.log"
export TROMMEL_FAKE_GPU_STATE="$WORK/gpu-state.json"
export TROMMEL_FAKE_MODE="$WORK/fake-mode"
export TROMMEL_OC_TIMEOUT_MS=1500
# Verify the clock locks well before the first dashboard capture.
export TROMMEL_OC_VERIFY_MS=3000

python3 "$ROOT/scripts/fixtures/fake-miner-api.py" 14029 "$WORK/gpu-state.json" \
    > "$WORK/miner-api.out" 2>&1 &
API_PID=$!
sleep 1

setsid "$BIN/trommeld" > "$WORK/daemon.out" 2>&1 < /dev/null &
DAEMON_PID=$!
sleep 2

# Let the daemon own the demo miner: it spawns the recorded DEMO_MINER path,
# snapshots stock, applies the profile and reports an owned session. Starting
# the miner by hand made the daemon classify it as external ("2 of 2 · external
# miner") and left its GPU state untuned.
if ! "$BIN/trommel" start > "$WORK/start.out" 2>&1; then
    echo "could not start the demo miner:" >&2
    cat "$WORK/start.out" >&2
    cat "$WORK/state/trommel/daemon.log" >&2 || true
    exit 1
fi
sleep 2

mkdir -p "$WORK/state/trommel/history"
python3 - "$WORK" <<'PY'
import json, math, pathlib, sys, time
work = pathlib.Path(sys.argv[1])
now = int(time.time())
start = now - 23 * 3600
files = {}
for index in range(24 * 60):
    ts = start + index * 60
    phase = index / 37.0
    total = 125.0e12 + 1.6e12 * math.sin(phase / 3.0)
    gpu0 = 100.0e12 + 1.2e12 * math.sin(phase / 4.0)
    gpu1 = total - gpu0
    sample = {
        "v": 1, "ts": ts, "running": True, "external": False,
        "preset": "pearl-wildrig", "miner": "wildrig",
        "total_hashrate": total, "power_w": 425.0, "mining_power_w": 425.0,
        "accepted": 1000 + index // 3, "rejected": 3, "stale": 0, "source": "api",
        "gpus": [
            {"uuid": "GPU-11111111-1111-1111-1111-111111111111", "index": 0, "bus_id": 1,
             "hashrate": gpu0, "power_w": 300.0, "temp_c": 60, "fan_pct": 65},
            {"uuid": "GPU-22222222-2222-2222-2222-222222222222", "index": 1, "bus_id": 2,
             "hashrate": gpu1, "power_w": 125.0, "temp_c": 55, "fan_pct": 60},
        ],
    }
    day = time.strftime("%Y%m%d", time.gmtime(ts))
    files.setdefault(day, []).append(json.dumps(sample))
for day, lines in files.items():
    (work / "state" / "trommel" / "history" / f"history-{day}.jsonl").write_text("\n".join(lines) + "\n")
PY

# The fake rig is stateful but static; vary the live measurements so the
# dashboard sparklines have a curve. Power/clock values stay as applied.
python3 - "$WORK/gpu-state.json" <<'PY' &
import json, math, os, sys, time
path = sys.argv[1]
start = time.time()
bases = [{"draw": 301.0, "temp": 61.0, "fan": 68.0, "util": 100.0},
         {"draw": 128.0, "temp": 55.0, "fan": 62.0, "util": 99.0}]
while True:
    try:
        with open(path, encoding="utf-8") as handle:
            state = json.load(handle)
        t = time.time() - start
        for index, gpu in enumerate(state["gpus"]):
            if index >= len(bases):
                break
            base, phase = bases[index], index * 1.7
            gpu["draw"] = round(base["draw"] + 6 * math.sin(t / 5 + phase), 2)
            gpu["temp"] = round(base["temp"] + 2.5 * math.sin(t / 9 + phase))
            gpu["fan"] = round(base["fan"] + 4 * math.sin(t / 3.5 + phase))
            gpu["util"] = min(100, round(base["util"] + 2 * math.sin(t / 3 + phase)))
        tmp = path + ".tmp"
        with open(tmp, "w", encoding="utf-8") as handle:
            json.dump(state, handle)
        os.replace(tmp, path)
    except Exception:
        pass
    time.sleep(2)
PY
JITTER_PID=$!

# Set ONLY=substring to regenerate just the matching images while iterating;
# a full run leaves it empty.
ONLY="${ONLY:-}"
wanted() { [ -z "$ONLY" ] || case "$(basename "$1")" in *"$ONLY"*) true ;; *) false ;; esac; }

configure() {
  local language="$1" simple="$2"
  python3 - "$WORK" "$language" "$simple" <<'PY'
import json, pathlib, sys
work, language, simple = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3] == "1"
p = work/"config"/"trommel"/"config.json"
cfg = json.loads(p.read_text())
cfg["behavior"]["language"] = language
cfg["behavior"]["simple_mode"] = simple
p.write_text(json.dumps(cfg, indent=2))
PY
  # The daemon owns the applied config: send the edit there too, otherwise the
  # GUI refresh would restore the old simple-mode/language values.
  "$BIN/trommel" config apply "$WORK/config/trommel/config.json" >/dev/null 2>&1 || true
}

shot() {
  local theme="$1" language="$2" page="$3" target="$4" simple="${5:-1}" scroll="${6:-}" height="${7:-}"
  wanted "$target" || return 0
  configure "$language" "$simple"
  if [ -n "$scroll" ]; then export TROMMEL_SCREENSHOT_SCROLL="$scroll"; else unset TROMMEL_SCREENSHOT_SCROLL; fi
  if [ -n "$height" ]; then export TROMMEL_WINDOW_HEIGHT="$height"; else unset TROMMEL_WINDOW_HEIGHT; fi
  # The dashboard sparklines need a handful of samples; other pages do not.
  local delay=6
  [ "$page" = dashboard ] && delay=14
  local attempt
  for attempt in 1 2; do
    rm -f "$target"
    TROMMEL_COLOR_SCHEME="$theme" TROMMEL_SCREENSHOT="$target" TROMMEL_SCREENSHOT_PAGE="$page" \
      TROMMEL_SCREENSHOT_DELAY="$delay" timeout 55 "$BIN/trommel" > "$WORK/gui-$theme-$language-$page.log" 2>&1 || true
    [ -f "$target" ] && break
    echo "  … retrying $(basename "$target")" >&2
  done
  [ -f "$target" ] && echo "  ✓ $(basename "$target")" || echo "  ✗ $target" >&2
}

tune_shot() {
  local theme="$1" language="$2" target="$3"
  wanted "$target" || return 0
  configure "$language" 0
  local attempt
  for attempt in 1 2; do
    rm -f "$target"
    TROMMEL_COLOR_SCHEME="$theme" TROMMEL_OPEN_TUNE=1 TROMMEL_TUNE_SCREENSHOT="$target" \
      TROMMEL_WINDOW_HEIGHT=820 timeout 50 "$BIN/trommel" > "$WORK/gui-tune-$theme-$language.log" 2>&1 || true
    [ -f "$target" ] && break
    echo "  … retrying $(basename "$target")" >&2
  done
  [ -f "$target" ] && echo "  ✓ $(basename "$target")" || echo "  ✗ $target" >&2
}

preset_shot() {
  local theme="$1" language="$2" target="$3"
  wanted "$target" || return 0
  configure "$language" 1
  local attempt
  for attempt in 1 2; do
    rm -f "$target"
    TROMMEL_COLOR_SCHEME="$theme" TROMMEL_OPEN_PRESET=1 TROMMEL_PRESET_SCREENSHOT="$target" \
      TROMMEL_WINDOW_HEIGHT=820 timeout 50 "$BIN/trommel" > "$WORK/gui-preset-$theme-$language.log" 2>&1 || true
    [ -f "$target" ] && break
    echo "  … retrying $(basename "$target")" >&2
  done
  [ -f "$target" ] && echo "  ✓ $(basename "$target")" || echo "  ✗ $target" >&2
}

install_shot() {
  local theme="$1" language="$2" target="$3"
  wanted "$target" || return 0
  configure "$language" 1
  local attempt
  for attempt in 1 2; do
    rm -f "$target"
    TROMMEL_COLOR_SCHEME="$theme" TROMMEL_OPEN_INSTALL=1 TROMMEL_INSTALL_SCREENSHOT="$target" \
      TROMMEL_WINDOW_HEIGHT=820 timeout 50 "$BIN/trommel" > "$WORK/gui-install-$theme-$language.log" 2>&1 || true
    [ -f "$target" ] && break
    echo "  … retrying $(basename "$target")" >&2
  done
  [ -f "$target" ] && echo "  ✓ $(basename "$target")" || echo "  ✗ $target" >&2
}

library_shot() {
  local theme="$1" language="$2" target="$3"
  wanted "$target" || return 0
  configure "$language" 1
  local attempt
  for attempt in 1 2; do
    rm -f "$target"
    TROMMEL_COLOR_SCHEME="$theme" TROMMEL_OPEN_LIBRARY=1 TROMMEL_LIBRARY_SCREENSHOT="$target" \
      TROMMEL_WINDOW_HEIGHT=820 timeout 50 "$BIN/trommel" > "$WORK/gui-library-$theme-$language.log" 2>&1 || true
    [ -f "$target" ] && break
    echo "  … retrying $(basename "$target")" >&2
  done
  [ -f "$target" ] && echo "  ✓ $(basename "$target")" || echo "  ✗ $target" >&2
}

history_shot() {
  local theme="$1" language="$2" target="$3"
  wanted "$target" || return 0
  configure "$language" 1
  local attempt
  for attempt in 1 2; do
    rm -f "$target"
    TROMMEL_COLOR_SCHEME="$theme" TROMMEL_OPEN_HISTORY=1 TROMMEL_HISTORY_SCREENSHOT="$target" \
      TROMMEL_WINDOW_HEIGHT=820 timeout 50 "$BIN/trommel" > "$WORK/gui-history-$theme-$language.log" 2>&1 || true
    [ -f "$target" ] && break
    echo "  … retrying $(basename "$target")" >&2
  done
  [ -f "$target" ] && echo "  ✓ $(basename "$target")" || echo "  ✗ $target" >&2
}

shot dark en dashboard "$DOCS/dashboard-en-dark.png" 1 "" 960
shot dark en presets "$DOCS/presets-en-dark.png"
shot dark en settings "$DOCS/settings-en-dark.png"
shot dark en settings "$DOCS/settings-advanced-en-dark.png" 0 bottom
preset_shot dark en "$DOCS/preset-editor-en-dark.png"
library_shot dark en "$DOCS/miner-library-en-dark.png"
install_shot dark en "$DOCS/miner-install-en-dark.png"
tune_shot dark en "$DOCS/gpu-tune-en-dark.png"
history_shot dark en "$DOCS/history-en-dark.png"
shot dark ru dashboard "$DOCS/dashboard-ru-dark.png" 1 "" 960
shot dark ru settings "$DOCS/settings-ru-dark.png"
shot light en dashboard "$DOCS/dashboard-en-light.png" 1 "" 960
shot light ru dashboard "$DOCS/dashboard-ru-light.png" 1 "" 960
echo "done"
