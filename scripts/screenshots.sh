#!/bin/bash
# Перегенерирует скриншоты для docs/ в тёмной и светлой теме.
#
# Поднимает приватную шину D-Bus, демон с «подставными» данными (фейковый
# nvidia-smi + лог майнера) и снимает окно приложения. Настоящая видеокарта,
# майнер и polkit не используются.
#
#   ./scripts/screenshots.sh            # тёмная и светлая тема
#   ./scripts/screenshots.sh --dark     # только тёмная
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$ROOT/.shots"
BIN="$ROOT/target/debug"
DOCS="$ROOT/docs"
DELAY="${SCREENSHOT_DELAY:-42}"

DO_DARK=1
DO_LIGHT=1
[ "${1:-}" = "--dark" ] && DO_LIGHT=0
[ "${1:-}" = "--light" ] && DO_DARK=0

[ -x "$BIN/pearl-miner" ] || { echo "нет $BIN/pearl-miner — собери сначала" >&2; exit 1; }
[ -x "$BIN/pearl-minerd" ] || { echo "нет $BIN/pearl-minerd — собери сначала" >&2; exit 1; }

rm -rf "$WORK"
mkdir -p "$WORK/config/pearl-miner" "$WORK/state/pearl-miner" "$WORK/bin" "$DOCS"

# --- подставной nvidia-smi (RTX 3080 Ti в майнинге на 300 W) ----------------
cat > "$WORK/bin/nvidia-smi" <<'SH'
#!/bin/bash
args="$*"
case "$args" in
    *power.min_limit*) echo "100.00, 400.00" ;;
    *temperature.gpu*)
        temp=$((60 + RANDOM % 4))
        fan=$((68 + RANDOM % 5))
        power=$((290 + RANDOM % 12))
        core=$((1600 + RANDOM % 40))
        echo "NVIDIA GeForce RTX 3080 Ti, $temp, $fan, $power.40, 300.00, $core, 810, 100, 8472"
        ;;
    *) exit 0 ;;
esac
SH
chmod +x "$WORK/bin/nvidia-smi"

# --- фейковый майнер: пишет строки в лог, чтобы шёл «живой» хешрейт --------
cat > "$WORK/feed-log.sh" <<'SH'
#!/bin/bash
LOG="$1"
i=0
while true; do
    hr=$(LC_ALL=C awk -v i="$i" 'BEGIN { printf "%.2f", 125.0 + 2.4*sin(i/3.0) + 0.6*sin(i/1.1) }')
    h60=$(LC_ALL=C awk -v i="$i" 'BEGIN { printf "%.2f", 124.6 + 0.9*sin(i/7.0) }')
    ts=$(printf '12:%02d:%02d' $((i / 60 % 60)) $((i % 60)))
    echo "[$ts] hashrate: 10s: $hr 60s: $h60 15m: 123.9 TH/s" >> "$LOG"
    echo "[$ts] Shares: $((512 + i / 3)) accepted, 2 rejected" >> "$LOG"
    i=$((i+1))
    sleep 1
done
SH
chmod +x "$WORK/feed-log.sh"

# --- конфиг ----------------------------------------------------------------
python3 - "$WORK" <<'PY'
import json, pathlib, sys
work = pathlib.Path(sys.argv[1])
cfg = {
    "wallet": "prl1testwallet",
    "worker": "3080ti",
    "pool": "pool.pearlhash.xyz:9000",
    "miner_profile": "wildrig",
    "pool_fee_pct": 0.0,
    "miners": {
        # Путь в скриншоте — «настоящий», как у пользователя, а процесс для
        # state.json запускается отдельно (см. ниже).
        "wildrig": {"path": f"{pathlib.Path.home()}/pearl/wildrig-multi",
                    "dev_fee_pct": 0.0, "extra_args": ""},
        "srbminer": {"path": "", "dev_fee_pct": 0.85, "extra_args": ""},
        "onezero": {"path": "", "dev_fee_pct": 1.0, "extra_args": ""},
    },
    "api_port": 0,
    "poll_interval_s": 1,
    "auto_restart": True,
    "restart_delay_s": 2,
    "notifications": False,
    "expected_hashrate": 125.0e12,
    "gpu": {"power_limit": 300, "stock_power_limit": 350, "core_clock": 1620,
            "memory_clock": 810, "fan_speed": 0, "temp_limit": 90, "temp_resume": 65},
}
(work / "config" / "pearl-miner" / "config.json").write_text(json.dumps(cfg, indent=2))
(work / "wildrig-multi").write_text("#!/bin/bash\nsleep 9999\n")
(work / "wildrig-multi").chmod(0o755)
PY

cleanup() {
    pkill -f "$WORK/feed-log.sh" 2>/dev/null
    [ -n "${MINER_PID:-}" ] && kill "$MINER_PID" 2>/dev/null
    [ -n "${DAEMON_PID:-}" ] && kill "$DAEMON_PID" 2>/dev/null
}
trap cleanup EXIT

export PATH="$WORK/bin:$PATH"
export XDG_CONFIG_HOME="$WORK/config"
export XDG_STATE_HOME="$WORK/state"
export XDG_CACHE_HOME="$WORK/cache"
# Шина — настоящая сессионная: только на ней доступны портал и тема GNOME.
# Демон с тем же именем на ней не запущен (продукт не установлен), поэтому
# имена свободны. Конфиг и состояние при этом изолированы в $WORK.

# Демон «считает», что майнер уже запущен (state.json), поэтому статус живой.
"$WORK/wildrig-multi" >/dev/null 2>&1 &
MINER_PID=$!
"$WORK/feed-log.sh" "$WORK/state/pearl-miner/miner.log" &
sleep 2
python3 - "$WORK" "$MINER_PID" <<'PY'
import json, pathlib, sys, time
work, pid = pathlib.Path(sys.argv[1]), int(sys.argv[2])
state = {"running": True, "miner_pid": pid, "started_at": int(time.time()) - 4530,
         "restarts": 0, "last_error": None, "external": False, "profile": "wildrig"}
(work / "state" / "pearl-miner" / "state.json").write_text(json.dumps(state, indent=2))
PY
"$BIN/pearl-minerd" > "$WORK/daemon.out" 2>&1 &
DAEMON_PID=$!
sleep 2

shot() {
    local theme="$1" page="$2" target="$3"
    shift 3
    rm -f "$target"   # иначе старый файл даст ложное «✓»
    PEARL_MINER_SCREENSHOT="$target" \
    PEARL_MINER_SCREENSHOT_PAGE="$page" \
    PEARL_MINER_SCREENSHOT_DELAY="$DELAY" \
    timeout $((DELAY + 25)) "$BIN/pearl-miner" gui > "$WORK/gui-$theme-$page.log" 2>&1
    if [ -f "$target" ]; then
        echo "  ✓ docs/$(basename "$target")"
    else
        echo "  ✗ не снялось: $page ($theme)" >&2
        tail -3 "$WORK/gui-$theme-$page.log" >&2
    fi
}

if [ "$DO_DARK" = 1 ]; then
    echo "Тёмная тема"
    export PEARL_MINER_COLOR_SCHEME=dark
    shot dark dashboard "$DOCS/dashboard.png"
    shot dark settings "$DOCS/settings.png"
    shot dark miner "$DOCS/miner.png"
fi

if [ "$DO_LIGHT" = 1 ]; then
    echo "Светлая тема"
    export PEARL_MINER_COLOR_SCHEME=light
    shot light dashboard "$DOCS/light-dashboard.png"
    shot light settings "$DOCS/light-settings.png"
    shot light miner "$DOCS/light-miner.png"
fi

echo "Готово: $(ls "$DOCS"/*.png 2>/dev/null | wc -l) файлов в docs/"
