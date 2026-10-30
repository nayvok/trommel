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

cat > "$WORK/bin/nvidia-smi" <<'SH'
#!/bin/bash
# The values follow a slow sine of wall-clock time instead of constants so the
# dashboard sparklines have a curve to draw; the numbers stay plausible for a
# mining rig.
args="$*"
case "$args" in
  *index,uuid*)
    awk -v t="$(date +%s)" 'BEGIN{
      p0=301+20*sin(t/6.0)+6*sin(t/2.7);
      p1=128+10*sin(t/5.0)+4*sin(t/2.1);
      t0=61+3*sin(t/9.0);
      t1=55+2.4*sin(t/7.5+1.0);
      c0=1620+45*sin(t/5.0);
      c1=1500+35*sin(t/5.5+0.5);
      m0=810+26*sin(t/7.0+2.0);
      m1=750+20*sin(t/6.5+2.5);
      f0=68+5*sin(t/3.5);
      f1=62+4*sin(t/4.0+1.0);
      u0=97+3*sin(t/3.0);
      u1=95+4*sin(t/3.6+0.7);
      printf "0, GPU-11111111-1111-1111-1111-111111111111, NVIDIA GeForce RTX 3080 Ti, %.0f, %.0f, %.2f, 300.00, 350.00, 100.00, 400.00, %.0f, %.0f, %.0f, 8472\n", t0, f0, p0, c0, m0, u0;
      printf "1, GPU-22222222-2222-2222-2222-222222222222, NVIDIA GeForce RTX 3060, %.0f, %.0f, %.2f, 130.00, 170.00, 60.00, 200.00, %.0f, %.0f, %.0f, 4096\n", t1, f1, p1, c1, m1, u1
    }'
    ;;
  *power.limit*) echo "300.00" ;;
  *) exit 0 ;;
esac
SH
chmod +x "$WORK/bin/nvidia-smi"

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
"$WORK/wildrig-multi" --algo pearlhash >/dev/null 2>&1 &
MINER_PID=$!
for i in $(seq 1 80); do
  echo "[12:00:$((i%60))] hashrate: 10s: 125.$((i%9)) 60s: 125.0 15m: 124.9 TH/s" >> "$WORK/state/trommel/miner.log"
  echo "[12:00:$((i%60))] Shares: $((500+i)) accepted, 2 rejected" >> "$WORK/state/trommel/miner.log"
done
python3 - "$WORK" "$MINER_PID" "$DEMO_MINER" "$DEMO_MINER_OLD" <<'PY'
import json, pathlib, sys, time
work = pathlib.Path(sys.argv[1]); pid = int(sys.argv[2]); demo_miner = sys.argv[3]
demo_miner_old = sys.argv[4]
cfg = {
  "schema_version": 2, "active_preset": "pearl-wildrig",
  "presets": {"pearl-wildrig": {
    "id":"pearl-wildrig","name":"Pearl · WildRig","coin":"pearl","algo":"pearlhash",
    "miner":"wildrig","miner_version":"0.51.8","pool":"pearlhash",
    "pool_url":"pool.pearlhash.xyz:9000","wallet":"prl1testwallet","worker":"3080ti",
    "gpu_profile":"balanced","api_port":0,"expected_hashrate":125000000000000.0,
    "miner_options":{"fan_speed":70},"miner_fee_override_pct":None,"miner_fee_pct":0.0,
    "pool_fee_pct":1.0,"extra_args":"","env":{},"created_at":0,"updated_at":0},
   "pearl-backup": {
    "id":"pearl-backup","name":"Pearl backup","coin":"pearl","algo":"pearlhash",
    "miner":"wildrig","miner_version":"0.51.7","pool":"pearlhash",
    "pool_url":"pool.pearlhash.xyz:9000","wallet":"prl1testwallet","worker":"backup",
    "gpu_profile":"balanced","api_port":0,"expected_hashrate":125000000000000.0,
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
state = {"running":True,"miner_pid":pid,"started_at":int(time.time())-3600,"restarts":0,
         "last_error":None,"external":False,"orphan":True,"preset_id":"pearl-wildrig",
         "miner":"wildrig","miner_version":"0.51.8","profile":"wildrig"}
(work/"state"/"trommel"/"state.json").write_text(json.dumps(state, indent=2))
PY

cleanup() {
  [ -n "${DAEMON_PID:-}" ] && kill "$DAEMON_PID" 2>/dev/null || true
  [ -n "${MINER_PID:-}" ] && kill "$MINER_PID" 2>/dev/null || true
  [ -n "${BUS_PID:-}" ] && kill "$BUS_PID" 2>/dev/null || true
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

setsid "$BIN/trommeld" > "$WORK/daemon.out" 2>&1 < /dev/null &
DAEMON_PID=$!
sleep 2

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

wizard_shot() {
  local theme="$1" language="$2" target="$3" step="${4:-0}"
  wanted "$target" || return 0
  configure "$language" 1
  local attempt
  for attempt in 1 2; do
    rm -f "$target"
    TROMMEL_COLOR_SCHEME="$theme" TROMMEL_OPEN_WIZARD=1 TROMMEL_WIZARD_SCREENSHOT="$target" \
      TROMMEL_WINDOW_HEIGHT=820 \
      TROMMEL_WIZARD_STEP="$step" timeout 50 "$BIN/trommel" > "$WORK/gui-wizard-$theme-$language-$step.log" 2>&1 || true
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
wizard_shot dark en "$DOCS/setup-en-dark.png"
wizard_shot dark en "$DOCS/setup-wallet-en-dark.png" 2
shot dark ru dashboard "$DOCS/dashboard-ru-dark.png" 1 "" 960
shot dark ru settings "$DOCS/settings-ru-dark.png"
shot light en dashboard "$DOCS/dashboard-en-light.png" 1 "" 960
shot light ru dashboard "$DOCS/dashboard-ru-light.png" 1 "" 960
echo "done"
