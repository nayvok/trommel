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

cat > "$WORK/wildrig-multi" <<'SH'
#!/bin/bash
while true; do sleep 2; done
SH
chmod +x "$WORK/wildrig-multi"
"$WORK/wildrig-multi" --algo pearlhash >/dev/null 2>&1 &
MINER_PID=$!
for i in $(seq 1 80); do
  echo "[12:00:$((i%60))] hashrate: 10s: 125.$((i%9)) 60s: 125.0 15m: 124.9 TH/s" >> "$WORK/state/trommel/miner.log"
  echo "[12:00:$((i%60))] Shares: $((500+i)) accepted, 2 rejected" >> "$WORK/state/trommel/miner.log"
done
python3 - "$WORK" "$MINER_PID" <<'PY'
import json, pathlib, sys, time
work = pathlib.Path(sys.argv[1]); pid = int(sys.argv[2])
cfg = {
  "schema_version": 2, "active_preset": "pearl-wildrig",
  "presets": {"pearl-wildrig": {
    "id":"pearl-wildrig","name":"Pearl · WildRig","coin":"pearl","algo":"pearlhash",
    "miner":"wildrig","miner_version":"0.51.8","pool":"pearlhash",
    "pool_url":"pool.pearlhash.xyz:9000","wallet":"prl1testwallet","worker":"3080ti",
    "gpu_profile":"balanced","api_port":0,"expected_hashrate":125000000000000.0,
    "miner_options":{"fan_speed":70},"miner_fee_override_pct":None,"miner_fee_pct":0.0,
    "pool_fee_pct":1.0,"extra_args":"","env":{},"created_at":0,"updated_at":0}},
  "gpu_profiles": {"balanced": {"name":"Balanced","default":{"enabled":True,"power_limit_w":300,
    "core_clock_mhz":1620,"memory_clock_mhz":810,"fan_pct":None,"temp_limit_c":90,"temp_resume_c":65},
    "per_gpu":{}}},
  "miner_installs": {"wildrig@0.51.8": {"miner":"wildrig","version":"0.51.8",
    "path":str(work/"wildrig-multi"),"sha256":"","source_url":"","verified":False,"custom":True,"installed_at":0}},
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
}
trap cleanup EXIT

export PATH="$WORK/bin:$PATH"
export XDG_CONFIG_HOME="$WORK/config"
export XDG_STATE_HOME="$WORK/state"
export XDG_CACHE_HOME="$WORK/cache"

setsid "$BIN/trommeld" > "$WORK/daemon.out" 2>&1 < /dev/null &
DAEMON_PID=$!
sleep 2

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
  local theme="$1" language="$2" page="$3" target="$4" simple="${5:-1}" scroll="${6:-}"
  configure "$language" "$simple"
  if [ -n "$scroll" ]; then export TROMMEL_SCREENSHOT_SCROLL="$scroll"; else unset TROMMEL_SCREENSHOT_SCROLL; fi
  local attempt
  for attempt in 1 2; do
    rm -f "$target"
    TROMMEL_COLOR_SCHEME="$theme" TROMMEL_SCREENSHOT="$target" TROMMEL_SCREENSHOT_PAGE="$page" \
      TROMMEL_SCREENSHOT_DELAY=6 timeout 55 "$BIN/trommel" > "$WORK/gui-$theme-$language-$page.log" 2>&1 || true
    [ -f "$target" ] && break
    echo "  … retrying $(basename "$target")" >&2
  done
  [ -f "$target" ] && echo "  ✓ $(basename "$target")" || echo "  ✗ $target" >&2
}

tune_shot() {
  local theme="$1" language="$2" target="$3"
  configure "$language" 0
  local attempt
  for attempt in 1 2; do
    rm -f "$target"
    TROMMEL_COLOR_SCHEME="$theme" TROMMEL_OPEN_TUNE=1 TROMMEL_TUNE_SCREENSHOT="$target" \
      timeout 50 "$BIN/trommel" > "$WORK/gui-tune-$theme-$language.log" 2>&1 || true
    [ -f "$target" ] && break
    echo "  … retrying $(basename "$target")" >&2
  done
  [ -f "$target" ] && echo "  ✓ $(basename "$target")" || echo "  ✗ $target" >&2
}

wizard_shot() {
  local theme="$1" language="$2" target="$3" step="${4:-0}"
  configure "$language" 1
  local attempt
  for attempt in 1 2; do
    rm -f "$target"
    TROMMEL_COLOR_SCHEME="$theme" TROMMEL_OPEN_WIZARD=1 TROMMEL_WIZARD_SCREENSHOT="$target" \
      TROMMEL_WIZARD_STEP="$step" timeout 50 "$BIN/trommel" > "$WORK/gui-wizard-$theme-$language-$step.log" 2>&1 || true
    [ -f "$target" ] && break
    echo "  … retrying $(basename "$target")" >&2
  done
  [ -f "$target" ] && echo "  ✓ $(basename "$target")" || echo "  ✗ $target" >&2
}

shot dark en dashboard "$DOCS/dashboard-en-dark.png"
shot dark en miners "$DOCS/miners-en-dark.png"
shot dark en settings "$DOCS/settings-en-dark.png"
shot dark en settings "$DOCS/settings-advanced-en-dark.png" 0 bottom
tune_shot dark en "$DOCS/gpu-tune-en-dark.png"
wizard_shot dark en "$DOCS/setup-en-dark.png"
wizard_shot dark en "$DOCS/setup-wallet-en-dark.png" 2
shot dark ru dashboard "$DOCS/dashboard-ru-dark.png"
shot dark ru settings "$DOCS/settings-ru-dark.png"
shot light en dashboard "$DOCS/dashboard-en-light.png"
echo "done"
