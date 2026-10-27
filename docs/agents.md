# Using Trommel from an external agent

Trommel is deliberately file/CLI-first, so Codex or another coding agent can
inspect and change it without a special API:

- config: `~/.config/trommel/config.json` (schema v2)
- state: `~/.local/state/trommel/state.json`
- miner log: `~/.local/state/trommel/miner.log`
- miner installs: `~/.local/share/trommel/miners/`
- schemas: [`config.schema.json`](schema/config.schema.json),
  [`preset.schema.json`](schema/preset.schema.json)

## Safe workflow

```bash
trommel diagnose --json > /tmp/trommel-diagnostics.json
trommel config export /tmp/trommel-config.json
# let the agent propose /tmp/trommel-proposed.json
trommel config validate /tmp/trommel-proposed.json
trommel config diff /tmp/trommel-proposed.json
trommel config apply /tmp/trommel-proposed.json --restart
```

`config diff` prints `Live` or `RestartRequired` and the reasons. `config apply`
saves and sends the config to the daemon; `--restart`/`--stop` choose the same
modes as the GUI. The agent should never apply a config without the user seeing
the diff.

## CLI JSON

```bash
trommel status --json
trommel gpus --json
trommel preset list
trommel preset show
trommel preset export pearl-wildrig /tmp/preset.json
trommel preset import /tmp/preset.json
trommel miner list
```

## MCP server

`trommel mcp` speaks the Model Context Protocol over stdio. It is read-only by
default. Add `--allow-write` only when you explicitly want the agent to
start/stop the miner or apply a proposed config.

Codex (or another MCP client) can launch it as a stdio server:

```toml
[mcp_servers.trommel]
command = "trommel"
args = ["mcp"]            # add "--allow-write" to enable write tools
```

Read-only tools include `trommel_status`, `trommel_gpus`, `trommel_presets`,
`trommel_preset_get`, `trommel_config`, `trommel_config_validate`,
`trommel_config_diff`, `trommel_miners` and `trommel_diagnose`. With
`--allow-write`, `trommel_start`, `trommel_stop` and `trommel_apply_config` are
also exposed.

## Safety rules for agents

- Never ask for or include a seed phrase/private key. Wallet addresses are
  public but redacted by `trommel diagnose` and the MCP config tool.
- Prefer diagnostics/JSON over screenshots; if a screenshot is needed, let the
  user take it.
- Validate ranges and preview the diff before applying.
- Do not bypass the Apply and restart / Apply and stop decision for a running
  miner.
- Do not edit files under `~/.local/share/trommel/miners/` by hand; use
  `trommel miner import/install`.
- The daemon is the single source of truth; editing config while it runs is not
  enough — use `trommel config apply`.
