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
    let mut headers = reqwest::header::HeaderMap::new();
    headers.insert(
        reqwest::header::AUTHORIZATION,
        format!("Bearer {}", cw_core::control::control_token(true)?).parse()?,
    );
    let client = reqwest::Client::builder()
        .default_headers(headers)
        .redirect(reqwest::redirect::Policy::none())
        .build()?;

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
                    "session_id": { "type": "string", "description": "Optional session ID (defaults to CW_SESSION_ID or the unique session matching the current workspace; pass explicitly when several sessions share the workspace)" }
                }
            }
        },
        {
            "name": "cw_shadow_dedupe",
            "description": "Shadow-deduplicates all stale or duplicate file reads in the context window so only the freshest read remains.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Optional session ID (defaults to unique current-workspace match)" },
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
                    "session_id": { "type": "string", "description": "Optional session ID (defaults to unique current-workspace match)" },
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
                    "session_id": { "type": "string", "description": "Optional session ID (defaults to unique current-workspace match)" },
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
                    "session_id": { "type": "string", "description": "Optional session ID (defaults to unique current-workspace match)" },
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
                    "session_id": { "type": "string", "description": "Optional session ID (defaults to unique current-workspace match)" },
                    "label": { "type": "string" },
                    "content": { "type": "string" }
                },
                "required": ["label", "content"]
            }
        },
        {
            "name": "cw_unpin_context",
            "description": "Removes pinned `<cw_pinned_context>` items from the session (all pins by default, or filtered by `label`), or clears all active session mutations when `clear_all_mutations` is true.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Optional session ID (defaults to unique current-workspace match)" },
                    "label": { "type": "string", "description": "Optional label substring to unpin only matching items. Omit to clear all pins." },
                    "clear_all_mutations": { "type": "boolean", "description": "If true, clears all active mutations (pins, prunes, folds) for the session." }
                }
            }
        },
        {
            "name": "cw_gate_tools",
            "description": "Disables unused MCP tool servers or specific tools for the remainder of the session to save tokens and sharpen tool selection accuracy.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Optional session ID (defaults to unique current-workspace match)" },
                    "disable_servers": { "type": "array", "items": { "type": "string" } },
                    "disable_tools": { "type": "array", "items": { "type": "string" } }
                }
            }
        },
        {
            "name": "cw_optimize_strategy",
            "description": "Applies a context optimization intensity level (`min`, `mid`, `max`) or named strategy (`cache_safe_boost`, `clean_code_state`, `surgical_log_diet`, `max_token_diet`) as a session overlay, and optionally exports a separate compacted `.jsonl` copy (leaving the source harness transcript unchanged).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "session_id": { "type": "string", "description": "Optional session ID (defaults to unique current-workspace match)" },
                    "strategy_id": {
                        "type": "string",
                        "description": "Intensity level (`min` | `mid` | `max`) or Strategy ID (`cache_safe_boost` | `clean_code_state` | `surgical_log_diet` | `max_token_diet`). Use `cw_fold_turns` for explicit turn folding."
                    },
                    "export_compacted": {
                        "type": "boolean",
                        "description": "If true, exports a separate compacted `.jsonl` copy to cw's export directory while leaving the source transcript untouched."
                    },
                    "sync_to_disk": {
                        "type": "boolean",
                        "description": "Legacy alias for `export_compacted`: exports a separate compacted `.jsonl` copy without modifying the source transcript in-place."
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
    if !matches!(
        tool_name,
        "cw_context_status"
            | "cw_shadow_dedupe"
            | "cw_evict_stale_reads"
            | "cw_fold_turns"
            | "cw_prune_tool_output"
            | "cw_pin_context"
            | "cw_unpin_context"
            | "cw_gate_tools"
            | "cw_optimize_strategy"
    ) {
        anyhow::bail!("Unknown tool: {tool_name}");
    }

    let base = base_url.trim_end_matches('/');
    let session_id = resolve_mcp_session_id(client, base, args).await?;

    let graph: Value = client
        .get(format!("{base}/api/v1/sessions/{session_id}/graph"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let revision = graph.get("revision").and_then(Value::as_str).unwrap_or("");
    match tool_name {
        "cw_context_status" => {
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
            let start_turn = args.get("start_turn").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
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
        "cw_unpin_context" => {
            let clear_all = args
                .get("clear_all_mutations")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let only_pins = !clear_all;
            let mut req = client
                .delete(format!("{base}/api/v1/sessions/{session_id}/mutations"))
                .header(reqwest::header::IF_MATCH, revision)
                .query(&[("only_pins", only_pins.to_string())]);
            if let Some(label) = args.get("label").and_then(|v| v.as_str())
                && !label.trim().is_empty()
            {
                req = req.query(&[("label", label.trim().to_string())]);
            }
            let resp: Value = req.send().await?.error_for_status()?.json().await?;
            Ok(serde_json::to_string_pretty(&resp)?)
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
            let export_compacted = args
                .get("export_compacted")
                .or_else(|| args.get("sync_to_disk"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            let opt_resp: Value = client
                .post(format!("{base}/api/v1/sessions/{session_id}/optimize"))
                .header(reqwest::header::IF_MATCH, revision)
                .json(&json!({
                    "strategy_id": strategy_id,
                    "status": "committed"
                }))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;

            if export_compacted {
                let sync_resp: Value = client
                    .post(format!("{base}/api/v1/sessions/{session_id}/sync-back"))
                    .send()
                    .await?
                    .error_for_status()?
                    .json()
                    .await?;
                Ok(serde_json::to_string_pretty(&json!({
                    "optimize": opt_resp,
                    "export": sync_resp
                }))?)
            } else {
                Ok(serde_json::to_string_pretty(&opt_resp)?)
            }
        }
        other => anyhow::bail!("Unknown tool: {other}"),
    }
}

async fn resolve_mcp_session_id(
    client: &reqwest::Client,
    base: &str,
    args: &Value,
) -> Result<String> {
    if let Some(s) = args
        .get("session_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Ok(s.to_string());
    }
    if let Ok(env_id) = std::env::var("CW_SESSION_ID") {
        let trimmed = env_id.trim();
        if !trimmed.is_empty() {
            return Ok(trimmed.to_string());
        }
    }
    let sessions: Vec<Value> = client
        .get(format!("{base}/api/v1/sessions"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let cwd = std::env::current_dir().ok();
    resolve_session_from_list(&sessions, cwd.as_deref())
}

pub fn resolve_session_from_list(
    sessions: &[Value],
    cwd: Option<&std::path::Path>,
) -> Result<String> {
    if let Some(cwd_path) = cwd {
        let matching: Vec<&str> = sessions
            .iter()
            .filter(|s| {
                s.get("workspace_root")
                    .and_then(|v| v.as_str())
                    .is_some_and(|root| std::path::Path::new(root) == cwd_path)
            })
            .filter_map(|s| s.get("id").and_then(|v| v.as_str()))
            .collect();
        match matching.as_slice() {
            [single] => return Ok((*single).to_string()),
            many if many.len() > 1 => {
                anyhow::bail!(
                    "Multiple sessions match workspace '{}': [{}]. Pass session_id explicitly.",
                    cwd_path.display(),
                    many.join(", ")
                );
            }
            _ => {}
        }
    }
    let available: Vec<&str> = sessions
        .iter()
        .filter_map(|s| s.get("id").and_then(|v| v.as_str()))
        .take(10)
        .collect();
    if available.is_empty() {
        anyhow::bail!("No active sessions recorded yet; pass session_id explicitly.");
    }
    anyhow::bail!(
        "No unique session matches the current workspace; pass session_id explicitly (candidates: [{}]).",
        available.join(", ")
    )
}

async fn post_mutation(
    client: &reqwest::Client,
    base: &str,
    session_id: &str,
    op: MutationOp,
) -> Result<String> {
    let graph: Value = client
        .get(format!("{base}/api/v1/sessions/{session_id}/graph"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let revision = graph.get("revision").and_then(Value::as_str).unwrap_or("");
    let resp: Value = client
        .post(format!("{base}/api/v1/sessions/{session_id}/mutations"))
        .header(reqwest::header::IF_MATCH, revision)
        .json(&json!({
            "op": op,
            "status": MutationStatus::Committed
        }))
        .send()
        .await?
        .error_for_status()?
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
        assert_eq!(tools.len(), 9);
        assert!(tools.iter().any(|t| t["name"] == "cw_shadow_dedupe"));
        assert!(tools.iter().any(|t| t["name"] == "cw_evict_stale_reads"));
        assert!(tools.iter().any(|t| t["name"] == "cw_unpin_context"));
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
        assert!(
            call_unknown["result"]["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("Unknown tool")
        );
    }

    #[test]
    fn test_resolve_session_from_list_workspace_matching() {
        let sessions = vec![
            json!({
                "id": "sess_alpha",
                "workspace_root": "/tmp/project-alpha"
            }),
            json!({
                "id": "sess_beta_1",
                "workspace_root": "/tmp/project-beta"
            }),
            json!({
                "id": "sess_beta_2",
                "workspace_root": "/tmp/project-beta"
            }),
        ];

        // 1. Unique workspace match resolves unambiguously
        let resolved =
            resolve_session_from_list(&sessions, Some(std::path::Path::new("/tmp/project-alpha")))
                .unwrap();
        assert_eq!(resolved, "sess_alpha");

        // 2. Multiple sessions in the same workspace return an error listing the candidate IDs
        let err_multi =
            resolve_session_from_list(&sessions, Some(std::path::Path::new("/tmp/project-beta")))
                .unwrap_err()
                .to_string();
        assert!(err_multi.contains("sess_beta_1"));
        assert!(err_multi.contains("sess_beta_2"));

        // 3. Unmatched workspace returns an error listing available candidates
        let err_none =
            resolve_session_from_list(&sessions, Some(std::path::Path::new("/tmp/project-gamma")))
                .unwrap_err()
                .to_string();
        assert!(err_none.contains("sess_alpha"));
    }
}
