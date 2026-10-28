#!/bin/bash
# MCP smoke test for `trommel mcp`.
#
# Exercises the JSON-RPC surface an external agent (Codex, Claude, …) sees:
# initialize, ping, tools/list and tools/call. Checks that the server is
# read-only by default, that write tools need --allow-write, and that no wallet
# address leaks through config/preset/diagnose responses.
#
# The test never calls a write tool for real: it only uses --allow-write to
# assert the tool is listed and that argument validation fails before anything
# is applied.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BIN="$ROOT/target/debug/trommel"
if [ "${TROMMEL_SKIP_BUILD:-0}" != 1 ]; then
    ( cd "$ROOT" && cargo build -q -p trommel )
fi
[ -x "$BIN" ] || { echo "build first: cargo build -p trommel" >&2; exit 1; }

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
mkdir -p "$WORK/config/trommel" "$WORK/state/trommel" "$WORK/cache" "$WORK/bin"
export XDG_CONFIG_HOME="$WORK/config"
export XDG_STATE_HOME="$WORK/state"
export XDG_CACHE_HOME="$WORK/cache"

# Test fixture: two presets with distinct wallets so we can prove every preset
# is redacted, not only the active one.
WALLET_ACTIVE="prl1testactive0000000000000000000000000000"
WALLET_BACKUP="prl1testbackup0000000000000000000000000000"
cat > "$WORK/config/trommel/config.json" <<JSON
{
  "schema_version": 2,
  "active_preset": "pearl-wildrig",
  "presets": {
    "pearl-wildrig": {
      "id": "pearl-wildrig",
      "name": "Pearl · WildRig",
      "coin": "pearl",
      "algo": "pearlhash",
      "miner": "wildrig",
      "miner_version": "0.51.8",
      "pool": "pearlhash",
      "pool_url": "pool.pearlhash.xyz:9000",
      "wallet": "$WALLET_ACTIVE",
      "worker": "rig1",
      "pool_fee_pct": 1.0
    },
    "backup": {
      "id": "backup",
      "name": "Backup",
      "coin": "pearl",
      "algo": "pearlhash",
      "miner": "wildrig",
      "miner_version": "0.51.8",
      "pool": "pearlhash",
      "pool_url": "pool.pearlhash.xyz:9000",
      "wallet": "$WALLET_BACKUP",
      "worker": "rig1",
      "pool_fee_pct": 1.0
    }
  },
  "behavior": {"simple_mode": true}
}
JSON

cat > "$WORK/requests-read.jsonl" <<'JSON'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"smoke","version":"1"}}}
{"jsonrpc":"2.0","method":"notifications/initialized"}
{"jsonrpc":"2.0","id":2,"method":"ping"}
{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"trommel_config","arguments":{}}}
{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"trommel_preset_get","arguments":{"id":"backup"}}}
{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"trommel_presets","arguments":{}}}
{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"trommel_config_validate","arguments":{}}}
{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"trommel_start","arguments":{}}}
{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"trommel_apply_config","arguments":{"config":{}}}}
{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"trommel_no_such_tool","arguments":{}}}
not valid json
JSON

cat > "$WORK/requests-write.jsonl" <<'JSON'
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}
{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}
{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"trommel_apply_config","arguments":{"config":{},"mode":"bogus"}}}
{"jsonrpc":"2.0","id":4,"method":"tools/list","params":{}}
JSON

echo "· read-only server"
timeout 120 "$BIN" mcp < "$WORK/requests-read.jsonl" > "$WORK/out-read.jsonl" 2> "$WORK/err-read.log" || {
    echo "mcp (read-only) exited with an error" >&2
    tail -20 "$WORK/err-read.log" >&2
    exit 1
}
echo "· --allow-write server"
timeout 120 "$BIN" mcp --allow-write < "$WORK/requests-write.jsonl" > "$WORK/out-write.jsonl" 2> "$WORK/err-write.log" || {
    echo "mcp --allow-write exited with an error" >&2
    tail -20 "$WORK/err-write.log" >&2
    exit 1
}

echo "· trommel diagnose redaction"
PATH="$WORK/bin:/usr/bin:/bin" timeout 60 "$BIN" diagnose --json > "$WORK/diagnose.json" 2> "$WORK/diagnose.err" || true

python3 - "$WORK" "$WALLET_ACTIVE" "$WALLET_BACKUP" <<'PY'
import json, pathlib, re, sys

work = pathlib.Path(sys.argv[1])
wallet_active, wallet_backup = sys.argv[2], sys.argv[3]
failures = []


def check(condition, message):
    if condition:
        print(f"  ✓ {message}")
    else:
        print(f"  ✗ {message}")
        failures.append(message)


def read_responses(name):
    responses = {}
    raw = (work / name).read_text()
    for line in raw.splitlines():
        line = line.strip()
        if not line:
            continue
        message = json.loads(line)
        responses[message.get("id")] = message
    return responses, raw


def text_of(message):
    """Return the parsed text payload of a tools/call result."""
    return json.loads(message["result"]["content"][0]["text"])


read, read_raw = read_responses("out-read.jsonl")
write, write_raw = read_responses("out-write.jsonl")
diagnose_raw = (work / "diagnose.json").read_text()
combined = read_raw + write_raw + diagnose_raw

check(read[1]["result"]["protocolVersion"] == "2024-11-05", "initialize announces protocol 2024-11-05")
check(read[1]["result"]["serverInfo"]["name"] == "trommel", "initialize names the trommel server")
check(read[2]["result"] == {}, "ping answers with an empty result")

tools = read[3]["result"]["tools"]
names = {tool["name"] for tool in tools}
check("trommel_status" in names, "tools/list exposes the read tools")
check(
    not ({"trommel_start", "trommel_stop", "trommel_apply_config"} & names),
    "read-only server hides write tools",
)
check(all("inputSchema" in tool for tool in tools), "every tool has an inputSchema")

cfg = text_of(read[4])
wallets = [preset.get("wallet") for preset in cfg["presets"].values()]
check(
    wallets and all(wallet == "<redacted>" for wallet in wallets),
    "trommel_config redacts every preset wallet",
)
check(text_of(read[5])["wallet"] == "<redacted>", "trommel_preset_get redacts the wallet")
presets = text_of(read[6])
check(
    {preset["id"] for preset in presets} == {"pearl-wildrig", "backup"},
    "trommel_presets lists both presets",
)
check(
    next(preset for preset in presets if preset["id"] == "pearl-wildrig")["active"],
    "trommel_presets marks the active preset",
)
check(isinstance(text_of(read[7])["warnings"], list), "trommel_config_validate returns warnings")

check("write tool disabled" in read[8]["error"]["message"], "trommel_start is refused without --allow-write")
check(
    "write tool disabled" in read[9]["error"]["message"],
    "trommel_apply_config is refused without --allow-write",
)
check("unknown tool" in read[10]["error"]["message"], "unknown tool is a JSON-RPC error")
check(read[None]["error"]["code"] == -32700, "invalid JSON yields a parse error")

write_tools = {tool["name"] for tool in write[2]["result"]["tools"]}
check(
    {"trommel_start", "trommel_stop", "trommel_apply_config"} <= write_tools,
    "--allow-write exposes the write tools",
)
check("mode must be live|restart|stop" in write[3]["error"]["message"], "bad apply mode is rejected")
check("tools" in write[4]["result"], "server stays alive after a rejected tool call")

check(wallet_active not in combined, "active wallet never appears in agent output")
check(wallet_backup not in combined, "backup wallet never appears in agent output")
check(
    not re.search(r"prl1[0-9a-z]{20,}", combined),
    "no wallet-shaped string appears in agent output",
)

diagnose = json.loads(diagnose_raw) if diagnose_raw.strip().startswith("{") else {}
if diagnose:
    diagnose_wallets = [
        preset.get("wallet") for preset in diagnose.get("config", {}).get("presets", {}).values()
    ]
    check(
        all(wallet in ("", "<redacted>") for wallet in diagnose_wallets),
        "trommel diagnose --json redacts wallets",
    )

if failures:
    print(f"\n{len(failures)} MCP check(s) failed", file=sys.stderr)
    sys.exit(1)
print("\nMCP smoke test passed")
PY
