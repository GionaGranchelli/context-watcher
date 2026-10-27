use anyhow::Result;
use cw_core::ir::{MutationOp, MutationStatus};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Runs the JSON-RPC 2.0 MCP Server over `stdio`, bridging tool calls to the local `cw` daemon
/// (or directly against `CwStore` when embedded).
pub async fn run_mcp_stdio_server(daemon_api_url: &str) -> Result<()> {
    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin).lines();
    let client = reqwest::Client::new();

    while let Some(line) = reader.next_line().await? {
        if line.trim().is_empty() {
            continue;
        }
        let Ok(req) = serde_json::from_str::<Value>(&line) else {
            continue;
        };

        let id = req.get("id").cloned();
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(Value::Null);

        if let Some(resp) = handle_mcp_request(&client, daemon_api_url, id, method, &params).await {
            let out_line = format!("{}\n", serde_json::to_string(&resp)?);
            stdout.write_all(out_line.as_bytes()).await?;
            stdout.flush().await?;
        }
    }
    Ok(())
}

pub async fn handle_mcp_request(
    client: &reqwest::Client,
    daemon_api_url: &str,
    id: Option<Value>,
    method: &str,
    params: &Value,
) -> Option<Value> {
    let req_id = id?; // Notifications without `id` do not expect a response

    let (result, error) = match method {
        "initialize" => (
            Some(json!({
                "protocolVersion": "2024-11-05",
                "capabilities": {
                    "tools": {}
                },
                "serverInfo": {
                    "name": "contextWatcher-mcp",
                    "version": "0.1.0"
                }
            })),
            None,
        ),
        "tools/list" => (
            Some(json!({
                "tools": mcp_tool_definitions()
            })),
            None,
        ),
        "tools/call" => {
            let tool_name = params.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let args = params
                .get("arguments")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            match execute_mcp_tool(client, daemon_api_url, tool_name, &args).await {
                Ok(text) => (
                    Some(json!({
                        "content": [{ "type": "text", "text": text }],
                        "isError": false
                    })),
                    None,
                ),
                Err(e) => (
                    Some(json!({
                        "content": [{ "type": "text", "text": format!("Error: {e}") }],
                        "isError": true
                    })),
                    None,
                ),
            }
        }
        _ => (
            None,
            Some(json!({
                "code": -32601,
                "message": format!("Method not found: {method}")
            })),
        ),
    };

    if let Some(err) = error {
        Some(json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "error": err
        }))
    } else {
        Some(json!({
            "jsonrpc": "2.0",
            "id": req_id,
            "result": result.unwrap_or(json!({}))
        }))
    }
}

pub fn mcp_tool_definitions() -> Value {
    json!([
        {
            "name": "cw_context_status",
            "description": "Returns current context window fullness, KV cache hit ratio, category token breakdown, and active lint warnings (stale files, duplicate reads, tool bloat).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Optional session ID (defaults to latest active session)" }
                }
            }
        },
        {
            "name": "cw_shadow_dedupe",
            "description": "Shadow-deduplicates all stale or duplicate file reads in the context window so only the freshest read remains.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "file_path": { "type": "string", "description": "Optional specific file path to deduplicate" }
                }
            }
        },
        {
            "name": "cw_evict_stale_reads",
            "description": "Alias for cw_shadow_dedupe: shadow-deduplicates all stale or duplicate file reads in context.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "file_path": { "type": "string", "description": "Optional specific file path to deduplicate" }
                }
            }
        },
        {
            "name": "cw_fold_turns",
            "description": "Compresses a range of turns (e.g., a completed or failed debugging sub-trajectory) into a single concise outcome summary node.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "start_turn": { "type": "integer" },
                    "end_turn": { "type": "integer" },
                    "outcome_summary": { "type": "string" }
                },
                "required": ["start_turn", "end_turn", "outcome_summary"]
            }
        },
        {
            "name": "cw_prune_tool_output",
            "description": "Replaces a large tool output from an earlier turn with a compact summary stub.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "call_id": { "type": "string" },
                    "summary": { "type": "string" }
                },
                "required": ["call_id", "summary"]
            }
        },
        {
            "name": "cw_pin_context",
            "description": "Pins a critical architectural constraint or code snippet to the high-attention tail of the prompt without breaking the KV cache prefix.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "label": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["label", "content"]
            }
        },
        {
            "name": "cw_gate_tools",
            "description": "Disables unused MCP tool servers or specific tools for the remainder of the session to save tokens and sharpen tool selection accuracy.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "disable_servers": { "type": "array", "items": { "type": "string" } },
                    "disable_tools": { "type": "array", "items": { "type": "string" } }
                }
            }
        },
        {
            "name": "cw_optimize_strategy",
            "description": "Applies a context optimization intensity level (`min`, `mid`, `max`) or named strategy (`cache_safe_boost`, `clean_code_state`, `surgical_log_diet`, `deep_trajectory_fold`, `max_token_diet`) and optionally syncs the compacted context back to the harness's disk transcript.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string" },
                    "strategy_id": {
                        "type": "string",
                        "description": "Intensity level (`min` | `mid` | `max`) or Strategy ID (`cache_safe_boost` | `clean_code_state` | `surgical_log_diet` | `deep_trajectory_fold` | `max_token_diet`)"
                    },
                    "sync_to_disk": {
                        "type": "boolean",
                        "description": "If true, writes compacted steps back to the session's .jsonl transcript on disk (creating a .bak backup first)."
                    }
                },
                "required": ["strategy_id"]
            }
        }
    ])
}

async fn execute_mcp_tool(
    client: &reqwest::Client,
    base_url: &str,
    tool_name: &str,
    args: &Value,
) -> Result<String> {
    let base = base_url.trim_end_matches('/');
    let session_id = match args.get("session_id").and_then(|v| v.as_str()) {
        Some(s) if !s.is_empty() => s.to_string(),
        _ => {
            let sessions: Vec<Value> = client
                .get(format!("{base}/api/v1/sessions"))
                .send()
                .await?
                .json()
                .await?;
            sessions
                .first()
                .and_then(|s| s.get("id"))
                .and_then(|v| v.as_str())
                .unwrap_or("default")
                .to_string()
        }
    };

    match tool_name {
        "cw_context_status" => {
            let graph: Value = client
                .get(format!("{base}/api/v1/sessions/{session_id}/graph"))
                .send()
                .await?
                .json()
                .await?;
            let Some(turn) = graph.get("latest_turn").filter(|t| !t.is_null()) else {
                return Ok(format!("Session '{session_id}' has no recorded turns yet."));
            };
            Ok(serde_json::to_string_pretty(&json!({
                "session_id": session_id,
                "health_score": turn.get("health_score"),
                "token_usage": turn.get("token_usage"),
                "cost_and_perf": graph.get("cost_and_perf"),
                "local_model_analysis": graph.get("local_model_analysis"),
                "kv_cache_report": turn.get("kv_cache_report"),
                "category_breakdown": turn.get("category_breakdown"),
                "lint_findings": turn.get("lint_findings"),
                "file_states": graph.get("file_states")
            }))?)
        }
        "cw_shadow_dedupe" | "cw_evict_stale_reads" => {
            let file_path = args
                .get("file_path")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let op = MutationOp::ShadowDedupe { file_path };
            post_mutation(client, base, &session_id, op).await
        }
        "cw_fold_turns" => {
            let start_turn = args
                .get("start_turn")
                .and_then(|v| v.as_u64())
                .unwrap_or(1) as u32;
            let end_turn = args
                .get("end_turn")
                .and_then(|v| v.as_u64())
                .unwrap_or(start_turn as u64) as u32;
            let summary_text = args
                .get("outcome_summary")
                .and_then(|v| v.as_str())
                .unwrap_or("Folded by agent")
                .to_string();
            let op = MutationOp::FoldSubtree {
                start_turn,
                end_turn,
                summary_text,
            };
            post_mutation(client, base, &session_id, op).await
        }
        "cw_prune_tool_output" => {
            let call_id = args
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let summary = args
                .get("summary")
                .and_then(|v| v.as_str())
                .unwrap_or("Pruned by agent")
                .to_string();
            let op = MutationOp::PruneOrStub {
                target_fingerprint: None,
                target_call_id: Some(call_id),
                target_json_pointer: None,
                stub_text: format!("[cw:pruned summary=\"{summary}\"]"),
            };
            post_mutation(client, base, &session_id, op).await
        }
        "cw_pin_context" => {
            let label = args
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("Note")
                .to_string();
            let content = args
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let op = MutationOp::PinContext {
                label,
                content,
                source_node_id: None,
            };
            post_mutation(client, base, &session_id, op).await
        }
        "cw_gate_tools" => {
            let disabled_mcp_servers: Vec<String> = args
                .get("disable_servers")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let disabled_tools: Vec<String> = args
                .get("disable_tools")
                .and_then(|v| v.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|x| x.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default();
            let op = MutationOp::GateToolsAndSkills {
                disabled_tools,
                disabled_mcp_servers,
                disabled_skills: vec![],
            };
            post_mutation(client, base, &session_id, op).await
        }
        "cw_optimize_strategy" => {
            let strategy_id = args
                .get("strategy_id")
                .and_then(|v| v.as_str())
                .unwrap_or("max_token_diet");
            let sync_to_disk = args
                .get("sync_to_disk")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let opt_resp: Value = client
                .post(format!("{base}/api/v1/sessions/{session_id}/optimize"))
                .json(&json!({
                    "strategy_id": strategy_id,
                    "status": "committed"
                }))
                .send()
                .await?
                .json()
                .await?;

            if sync_to_disk {
                let sync_resp: Value = client
                    .post(format!("{base}/api/v1/sessions/{session_id}/sync-back"))
                    .send()
                    .await?
                    .json()
                    .await?;
                Ok(serde_json::to_string_pretty(&json!({
                    "optimize": opt_resp,
                    "disk_sync": sync_resp
                }))?)
            } else {
                Ok(serde_json::to_string_pretty(&opt_resp)?)
            }
        }
        other => anyhow::bail!("Unknown tool: {other}"),
    }
}

async fn post_mutation(
    client: &reqwest::Client,
    base: &str,
    session_id: &str,
    op: MutationOp,
) -> Result<String> {
    let resp: Value = client
        .post(format!("{base}/api/v1/sessions/{session_id}/mutations"))
        .json(&json!({
            "op": op,
            "status": MutationStatus::Committed
        }))
        .send()
        .await?
        .json()
        .await?;
    Ok(serde_json::to_string_pretty(&resp)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mcp_initialize_and_tools_list() {
        let client = reqwest::Client::new();
        let init_resp = handle_mcp_request(
            &client,
            "http://127.0.0.1:4400",
            Some(json!(1)),
            "initialize",
            &json!({}),
        )
        .await
        .unwrap();
        assert_eq!(
            init_resp["result"]["serverInfo"]["name"],
            "contextWatcher-mcp"
        );

        let list_resp = handle_mcp_request(
            &client,
            "http://127.0.0.1:4400",
            Some(json!(2)),
            "tools/list",
            &json!({}),
        )
        .await
        .unwrap();
        let tools = list_resp["result"]["tools"].as_array().unwrap();
        assert_eq!(tools.len(), 8);
        assert!(tools.iter().any(|t| t["name"] == "cw_shadow_dedupe"));
        assert!(tools.iter().any(|t| t["name"] == "cw_evict_stale_reads"));
    }

    #[tokio::test]
    async fn test_mcp_unknown_tool_and_unsupported_method() {
        let client = reqwest::Client::new();
        let resp = handle_mcp_request(
            &client,
            "http://127.0.0.1:4400",
            Some(json!(42)),
            "unknown_method",
            &json!({}),
        )
        .await
        .unwrap();
        assert_eq!(resp["error"]["code"], -32601);

        let call_unknown = handle_mcp_request(
            &client,
            "http://127.0.0.1:4400",
            Some(json!(43)),
            "tools/call",
            &json!({
                "name": "non_existent_tool",
                "arguments": {}
            }),
        )
        .await
        .unwrap();
        assert_eq!(call_unknown["result"]["isError"], true);
        assert!(call_unknown["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("Unknown tool"));
    }
}
