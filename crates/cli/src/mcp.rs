//! Minimal Model Context Protocol server over stdio.
//!
//! Codex/other MCP clients can read Trommel state and (with --allow-write)
//! start/stop the miner or apply a proposed config. Read-only by default.

use std::io::{self, BufRead, Write};

use serde_json::{json, Value};
use trommel_core::config::{ApplyMode, Config};
use trommel_core::dbus::MinerProxy;

pub fn run(allow_write: bool) -> anyhow::Result<()> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request: Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(err) => {
                let response = error_response(Value::Null, -32700, &format!("parse error: {err}"));
                writeln!(stdout, "{response}")?;
                stdout.flush()?;
                continue;
            }
        };
        if request.get("id").is_none() {
            // Notification such as notifications/initialized.
            continue;
        }
        let id = request.get("id").cloned().unwrap_or(Value::Null);
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").cloned().unwrap_or(Value::Null);
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "trommel", "version": trommel_core::VERSION}
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tool_definitions(allow_write)})),
            "tools/call" => call_tool(&params, allow_write),
            "shutdown" => Ok(json!({})),
            other => Err(format!("unknown method: {other}")),
        };
        let response = match result {
            Ok(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}),
            Err(message) => error_response(id, -32601, &message),
        };
        writeln!(stdout, "{response}")?;
        stdout.flush()?;
    }
    Ok(())
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn text_result(value: Value) -> Value {
    json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&value).unwrap_or_default()}]})
}

fn tool_definitions(allow_write: bool) -> Vec<Value> {
    let mut tools = vec![
        json!({
            "name": "trommel_status",
            "description": "Current Trommel status: state, GPUs, miner info, pool, fees and warnings.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "trommel_gpus",
            "description": "Raw NVIDIA GPU data reported by nvidia-smi.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "trommel_presets",
            "description": "List mining presets.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "trommel_preset_get",
            "description": "Get one preset by id.",
            "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}, "required": ["id"]}
        }),
        json!({
            "name": "trommel_config",
            "description": "Current config with the wallet redacted.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "trommel_config_validate",
            "description": "Validate the current config and return warnings.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "trommel_config_diff",
            "description": "Compare the current config with a proposed config object.",
            "inputSchema": {"type": "object", "properties": {"config": {"type": "object"}}, "required": ["config"]}
        }),
        json!({
            "name": "trommel_miners",
            "description": "Miner catalogue and installed versions.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "trommel_diagnose",
            "description": "Build a redacted diagnostics bundle suitable for an external assistant.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
        json!({
            "name": "trommel_oc",
            "description": "GPU stock snapshot, current limits and the targets a restore would write.",
            "inputSchema": {"type": "object", "properties": {}}
        }),
    ];
    if allow_write {
        tools.extend([
            json!({
                "name": "trommel_start",
                "description": "Start mining. Requires --allow-write on the MCP server.",
                "inputSchema": {"type": "object", "properties": {}}
            }),
            json!({
                "name": "trommel_stop",
                "description": "Stop mining and restore stock. Requires --allow-write.",
                "inputSchema": {"type": "object", "properties": {}}
            }),
            json!({
                "name": "trommel_apply_config",
                "description": "Apply a proposed config. Requires --allow-write; mode is live|restart|stop.",
                "inputSchema": {"type": "object", "properties": {
                    "config": {"type": "object"},
                    "mode": {"type": "string", "enum": ["live", "restart", "stop"]}
                }, "required": ["config"]}
            }),
            json!({
                "name": "trommel_restore_oc",
                "description": "Restore GPU power/clocks. Requires --allow-write; mode is snapshot|defaults.",
                "inputSchema": {"type": "object", "properties": {
                    "mode": {"type": "string", "enum": ["snapshot", "defaults"]}
                }}
            }),
        ]);
    }
    tools
}

fn call_tool(params: &Value, allow_write: bool) -> Result<Value, String> {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    match name {
        "trommel_status" => {
            let status = block_on_status()?;
            Ok(text_result(status))
        }
        "trommel_gpus" => {
            let gpus = trommel_core::nvidia::query_all().map_err(|e| e.to_string())?;
            Ok(text_result(json!(gpus)))
        }
        "trommel_presets" => {
            let cfg = Config::load_or_default();
            let presets: Vec<Value> = cfg
                .presets
                .values()
                .map(|preset| {
                    json!({
                        "id": preset.id,
                        "name": preset.name,
                        "active": cfg.active_preset.as_deref() == Some(preset.id.as_str()),
                        "coin": preset.coin,
                        "algo": preset.algo,
                        "miner": preset.miner,
                        "version": preset.miner_version,
                        "total_fee_pct": preset.total_fee_pct(),
                    })
                })
                .collect();
            Ok(text_result(json!(presets)))
        }
        "trommel_preset_get" => {
            let id = args.get("id").and_then(Value::as_str).unwrap_or("");
            let cfg = Config::load_or_default().redacted();
            let preset = cfg
                .presets
                .get(id)
                .ok_or_else(|| format!("unknown preset {id}"))?;
            Ok(text_result(json!(preset)))
        }
        "trommel_config" => {
            let cfg = Config::load_or_default().redacted();
            Ok(text_result(json!(cfg)))
        }
        "trommel_config_validate" => {
            let cfg = Config::load_or_default();
            Ok(text_result(json!({"warnings": cfg.warnings()})))
        }
        "trommel_config_diff" => {
            let current = Config::load_or_default();
            let proposed: Config =
                serde_json::from_value(args.get("config").cloned().ok_or("config is required")?)
                    .map_err(|e| format!("config is invalid: {e}"))?;
            let diff = current.diff(&proposed);
            Ok(text_result(json!({
                "class": format!("{:?}", diff.class),
                "reasons": diff.reasons
            })))
        }
        "trommel_miners" => {
            let cfg = Config::load_or_default();
            let miners: Vec<Value> = trommel_core::catalog::miners()
                .iter()
                .map(|miner| {
                    json!({
                        "id": miner.id,
                        "name": miner.name,
                        "description": miner.short_description(),
                        "fee_pct": miner.fee_pct,
                        "fee_configurable": miner.fee_configurable,
                        "installed": trommel_core::catalog::installed_versions(&cfg, &miner.id),
                        "versions": miner.versions.iter().map(|v| &v.version).collect::<Vec<_>>(),
                    })
                })
                .collect();
            Ok(text_result(json!(miners)))
        }
        "trommel_diagnose" => {
            let cfg = Config::load_or_default();
            let status = block_on_status().ok();
            Ok(text_result(json!({
                "app": "Trommel",
                "version": trommel_core::VERSION,
                "config_path": trommel_core::paths::config_path(),
                "status": status,
                "gpus": trommel_core::nvidia::query_all().ok(),
                "warnings": cfg.warnings(),
            })))
        }
        "trommel_oc" => {
            let raw =
                block_on(
                    |proxy| async move { proxy.oc_report().await.map_err(|e| e.to_string()) },
                )?;
            let value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
            Ok(text_result(value))
        }
        "trommel_start" if allow_write => {
            block_on(|proxy| async move { proxy.start().await.map_err(|e| e.to_string()) })?;
            Ok(text_result(json!({"ok": true})))
        }
        "trommel_stop" if allow_write => {
            block_on(|proxy| async move { proxy.stop().await.map_err(|e| e.to_string()) })?;
            Ok(text_result(json!({"ok": true})))
        }
        "trommel_restore_oc" if allow_write => {
            let mode = args
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("snapshot")
                .to_string();
            let raw = block_on(move |proxy| async move {
                proxy.restore_oc(&mode).await.map_err(|e| e.to_string())
            })?;
            let value: Value = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
            Ok(text_result(json!({"ok": true, "result": value})))
        }
        "trommel_apply_config" if allow_write => {
            let config = args.get("config").cloned().ok_or("config is required")?;
            let mode = args.get("mode").and_then(Value::as_str).unwrap_or("live");
            if ApplyMode::parse(mode).is_none() {
                return Err("mode must be live|restart|stop".into());
            }
            let text = serde_json::to_string(&config).map_err(|e| e.to_string())?;
            let mode = mode.to_string();
            let result = block_on(move |proxy| async move {
                proxy
                    .apply_config(&text, &mode)
                    .await
                    .map_err(|e| e.to_string())
            })?;
            Ok(text_result(json!({"ok": true, "result": result})))
        }
        "trommel_start" | "trommel_stop" | "trommel_apply_config" | "trommel_restore_oc" => {
            Err("write tool disabled; start the MCP server with --allow-write".into())
        }
        other => Err(format!("unknown tool: {other}")),
    }
}

fn block_on_status() -> Result<Value, String> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let connection = zbus::Connection::session()
            .await
            .map_err(|e| e.to_string())?;
        let proxy = MinerProxy::new(&connection)
            .await
            .map_err(|e| e.to_string())?;
        let text = proxy.refresh().await.map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    })
}

fn block_on<F, Fut, T>(f: F) -> Result<T, String>
where
    F: FnOnce(MinerProxy<'static>) -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())?;
    runtime.block_on(async {
        let connection = zbus::Connection::session()
            .await
            .map_err(|e| e.to_string())?;
        let proxy = MinerProxy::new(&connection)
            .await
            .map_err(|e| e.to_string())?;
        f(proxy).await
    })
}
