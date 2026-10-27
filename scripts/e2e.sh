#!/bin/bash
# Сквозной тест: демон + CLI на приватной шине D-Bus с фейковым майнером.
#
# Не трогает настоящий майнер, nvidia-smi и polkit-хелпер: всё в ./.e2e/
# (временный каталог внутри проекта, потому что /tmp в песочнице отдельный).
#
#   ./scripts/e2e.sh
set -uo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORK="$ROOT/.e2e"
BIN="$ROOT/target/debug"
FAILS=0

pass() { printf '\033[1;32m  ✓ %s\033[0m\n' "$*"; }
fail() { printf '\033[1;31m  ✗ %s\033[0m\n' "$*"; FAILS=$((FAILS + 1)); }
step() { printf '\n\033[1;34m== %s ==\033[0m\n' "$*"; }

command -v dbus-daemon >/dev/null || { echo "нужен dbus-daemon" >&2; exit 1; }

step "Сборка"
( cd "$ROOT" && cargo build -q -p pearl-minerd -p pearl-miner ) || { echo "сборка не удалась" >&2; exit 1; }

step "Подготовка окружения"
rm -rf "$WORK"
mkdir -p "$WORK/config/pearl-miner" "$WORK/state"
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

cat > "$WORK/fake-miner.sh" <<'SH'
#!/bin/bash
echo "[fake] WildRig Multi (test double)"
while true; do
  echo "[00:00:00] hashrate: 10s: 125.1 60s: 125.0 15m: 124.2 TH/s"
  echo "[00:00:00] Shares: 512 accepted, 2 rejected"
  sleep 2
done
SH
chmod +x "$WORK/fake-miner.sh"

python3 - "$WORK" <<'PY'
import json, pathlib, sys
work = pathlib.Path(sys.argv[1])
cfg = {
  "wallet": "prl1testwallet",
  "worker": "e2e",
  "pool": "pool.pearlhash.xyz:9000",
  "miner_profile": "wildrig",
  "miners": {"wildrig": {"path": str(work / "fake-miner.sh"), "dev_fee_pct": 0.0, "extra_args": ""}},
  "api_port": 4099,
  "poll_interval_s": 1,
  "auto_restart": True,
  "restart_delay_s": 2,
  "notifications": False,
  "gpu": {"power_limit": 300, "stock_power_limit": 350, "core_clock": 1620,
          "memory_clock": 810, "temp_limit": 90, "temp_resume": 65},
}
(work / "config" / "pearl-miner" / "config.json").write_text(json.dumps(cfg, indent=2))
PY

export XDG_CONFIG_HOME="$WORK/config"
export XDG_STATE_HOME="$WORK/state"
BUS_OUT=$(dbus-daemon --config-file="$WORK/bus.conf" --print-address --print-pid --fork 2>/dev/null)
export DBUS_SESSION_BUS_ADDRESS="$(printf '%s\n' "$BUS_OUT" | sed -n 1p)"
BUS_PID="$(printf '%s\n' "$BUS_OUT" | sed -n 2p)"

cleanup() {
    pkill -f "$WORK/fake-miner.sh" 2>/dev/null
    [ -n "${DAEMON_PID:-}" ] && kill "$DAEMON_PID" 2>/dev/null
    [ -n "$BUS_PID" ] && kill "$BUS_PID" 2>/dev/null
}
trap cleanup EXIT

step "Старт демона"
"$BIN/pearl-minerd" > "$WORK/daemon.out" 2>&1 &
DAEMON_PID=$!
sleep 2
if kill -0 "$DAEMON_PID" 2>/dev/null; then
    pass "pearl-minerd работает (pid $DAEMON_PID)"
else
    fail "pearl-minerd не запустился"; cat "$WORK/daemon.out"; exit 1
fi

step "start"
if "$BIN/pearl-miner" start 2>&1 | grep -q "запущен"; then
    pass "CLI start"
else
    fail "CLI start"
fi
sleep 3

step "status --json"
JSON=$("$BIN/pearl-miner" status --json 2>/dev/null)
echo "$JSON" | python3 -c '
import json, sys
s = json.load(sys.stdin)
assert s["state"]["running"] is True, "running"
assert s["miner"]["hashrate_10s"] == 125.1e12, s["miner"]["hashrate_10s"]
assert s["miner"]["accepted"] >= 512, s["miner"]["accepted"]
assert s["ts_iso"].endswith("Z"), s["ts_iso"]
assert "gpu" in s and "pool" in s
print("  статус разобран:", s["miner"]["source"], s["ts_iso"])
' && pass "статус соответствует контракту" || fail "контракт статуса"

step "автоперезапуск"
pkill -f "$WORK/fake-miner.sh"
sleep 6
RESTARTS=$("$BIN/pearl-miner" status --json | python3 -c 'import json,sys; print(json.load(sys.stdin)["state"]["restarts"])')
if [ "${RESTARTS:-0}" -ge 1 ]; then
    pass "майнер перезапущен (restarts=$RESTARTS)"
else
    fail "автоперезапуск не сработал"
fi

step "toggle (стоп)"
"$BIN/pearl-miner" toggle >/dev/null 2>&1
sleep 1
if [ -f "$XDG_STATE_HOME/pearl-miner/state.json" ]; then
    fail "state.json не удалён после остановки"
else
    pass "state.json очищен"
fi
"$BIN/pearl-miner" status --json | python3 -c '
import json, sys
s = json.load(sys.stdin)
assert s["state"]["running"] is False, "всё ещё running"
assert s["miner"]["hashrate_10s"] is None, "хешрейт после стопа"
' && pass "статус после стопа корректен" || fail "статус после стопа"

step "конфиг"
if "$BIN/pearl-miner" config get | python3 -c 'import json,sys; json.load(sys.stdin)' 2>/dev/null; then
    pass "config get — валидный JSON"
else
    fail "config get"
fi

echo
if [ "$FAILS" -eq 0 ]; then
    printf '\033[1;32mВСЁ ЗЕЛЁНОЕ\033[0m\n'
    exit 0
else
    printf '\033[1;31mПРОВАЛОВ: %s\033[0m\n' "$FAILS"
    exit 1
fi
