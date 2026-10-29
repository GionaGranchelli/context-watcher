use crate::ir::{
    CacheControlBreakpoint, ContextNode, KvNodeStatus, NodeFingerprint, NodeKind, ProviderKind,
    Role, SkillEntry, TokenSpan, ToolSemanticMeta,
};
use crate::tokenizer::assign_token_spans_and_salience;
use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::LazyLock;

static XML_RULE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<RULE\[([^\]]*)\]>(.*?)</RULE\[[^\]]*\]>").unwrap());
static XML_SKILLS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)<skills>(.*?)</skills>").unwrap());
static SKILL_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^-\s+([a-zA-Z0-9_-]+)\s+\(([^)]+)\):\s*(.+)$").unwrap());

#[derive(Debug, thiserror::Error)]
pub enum NormalizeError {
    #[error("Invalid JSON payload: {0}")]
    InvalidJson(#[from] serde_json::Error),
    #[error("Payload root must be a JSON object")]
    NotAnObject,
}

/// Auto-detects provider from request path or payload structure.
pub fn detect_provider(path: &str, payload: &Value) -> ProviderKind {
    if path.contains("anthropic") || path.ends_with("/v1/messages") {
        return ProviderKind::Anthropic;
    }
    if path.contains("gemini")
        || path.contains("generateContent")
        || payload.get("contents").is_some()
    {
        return ProviderKind::Gemini;
    }
    if payload.get("system").is_some()
        && payload.get("messages").and_then(|m| m.as_array()).is_some()
        && payload.get("max_tokens").is_some()
    {
        return ProviderKind::Anthropic;
    }
    ProviderKind::OpenAI
}

/// Normalizes any supported LLM request payload into an ordered list of `ContextNode`s
/// with RFC 6901 JSON Pointers (`raw_json_pointer`) and computed `TokenSpan`s.
pub fn parse_request_to_nodes(
    payload: &Value,
    provider: ProviderKind,
    turn_seq: u32,
) -> Result<Vec<ContextNode>, NormalizeError> {
    if !payload.is_object() {
        return Err(NormalizeError::NotAnObject);
    }

    let mut nodes = Vec::new();
    match provider {
        ProviderKind::Anthropic => parse_anthropic(payload, turn_seq, &mut nodes),
        ProviderKind::Gemini => parse_gemini(payload, turn_seq, &mut nodes),
        ProviderKind::OpenAI | ProviderKind::OpenAICompatible => {
            parse_openai(payload, turn_seq, &mut nodes)
        }
    }

    assign_token_spans_and_salience(&mut nodes, provider);
    Ok(nodes)
}

fn parse_anthropic(payload: &Value, turn_seq: u32, nodes: &mut Vec<ContextNode>) {
    // 1. Tools (`/tools/i`)
    if let Some(tools) = payload.get("tools").and_then(|v| v.as_array()) {
        for (i, tool) in tools.iter().enumerate() {
            let name = tool
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_tool")
                .to_string();
            let desc = tool
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let schema = tool
                .get("input_schema")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            let cache_ctrl = extract_cache_control(tool);
            let (server_name, short_name) = split_mcp_tool_name(&name);
            let pointer = format!("/tools/{i}");
            push_node(
                nodes,
                1,
                &pointer,
                Role::System,
                NodeKind::McpToolSchema {
                    server_name,
                    tool_name: short_name,
                    description: desc,
                    schema_json: schema,
                },
                cache_ctrl,
            );
        }
    }

    // 2. System (`/system`)
    if let Some(sys) = payload.get("system") {
        match sys {
            Value::String(s) => {
                deconstruct_system_text(s, "/system", 1, None, nodes);
            }
            Value::Array(arr) => {
                for (i, block) in arr.iter().enumerate() {
                    let pointer = format!("/system/{i}");
                    let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    let cache_ctrl = extract_cache_control(block);
                    deconstruct_system_text(text, &pointer, 1, cache_ctrl, nodes);
                }
            }
            _ => {}
        }
    }

    // 3. Messages (`/messages/i`)
    let mut call_meta_by_id: HashMap<String, (String, Option<ToolSemanticMeta>)> = HashMap::new();

    if let Some(messages) = payload.get("messages").and_then(|v| v.as_array()) {
        for (msg_idx, msg) in messages.iter().enumerate() {
            let role_str = msg.get("role").and_then(|v| v.as_str()).unwrap_or("user");
            let is_tool_msg = role_str == "tool" || role_str == "toolResult";
            let role = if role_str == "assistant" {
                Role::Assistant
            } else if is_tool_msg {
                Role::Tool
            } else {
                Role::User
            };
            let msg_pointer = format!("/messages/{msg_idx}");
            let msg_cache = extract_cache_control(msg);
            let approx_turn = msg
                .get("cw_turn_seq")
                .and_then(|v| v.as_u64())
                .map(|n| n as u32)
                .unwrap_or((msg_idx as u32 / 2) + 1);
            let msg_ts = msg
                .get("cw_timestamp")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());
            let before_len = nodes.len();

            if is_tool_msg {
                let call_id = msg
                    .get("toolCallId")
                    .or_else(|| msg.get("tool_call_id"))
                    .or_else(|| msg.get("tool_use_id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let is_error = msg
                    .get("isError")
                    .or_else(|| msg.get("is_error"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let content = extract_tool_result_text(msg.get("content"));
                let (tool_name, call_sem) =
                    if let Some(n) = msg.get("toolName").and_then(|v| v.as_str()) {
                        let sem = call_meta_by_id.get(&call_id).and_then(|(_, s)| s.clone());
                        (n.to_string(), sem)
                    } else {
                        call_meta_by_id
                            .get(&call_id)
                            .cloned()
                            .unwrap_or_else(|| ("tool_result".to_string(), None))
                    };
                let sem = enrich_result_semantic_meta(call_sem, &content);
                push_node(
                    nodes,
                    approx_turn,
                    &format!("{msg_pointer}/content"),
                    Role::Tool,
                    NodeKind::ToolResult {
                        call_id,
                        tool_name,
                        is_error,
                        content,
                        semantic_meta: sem,
                    },
                    msg_cache,
                );
                if let Some(ref ts) = msg_ts {
                    for node in &mut nodes[before_len..] {
                        node.timestamp = Some(ts.clone());
                    }
                }
                continue;
            }

            match msg.get("content") {
                Some(Value::String(text)) => {
                    let kind = if role == Role::Assistant {
                        NodeKind::AssistantText { text: text.clone() }
                    } else {
                        NodeKind::UserMessage { text: text.clone() }
                    };
                    push_node(
                        nodes,
                        approx_turn,
                        &format!("{msg_pointer}/content"),
                        role,
                        kind,
                        msg_cache,
                    );
                }
                Some(Value::Array(blocks)) => {
                    for (blk_idx, blk) in blocks.iter().enumerate() {
                        let blk_ptr = format!("{msg_pointer}/content/{blk_idx}");
                        let blk_cache = extract_cache_control(blk).or_else(|| msg_cache.clone());
                        let blk_type =
                            blk.get("type").and_then(|v| v.as_str()).unwrap_or_else(|| {
                                if blk.get("thinking").is_some() {
                                    "thinking"
                                } else {
                                    "text"
                                }
                            });

                        match blk_type {
                            "thinking" => {
                                let thought = blk
                                    .get("thinking")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let sig = blk
                                    .get("signature")
                                    .or_else(|| blk.get("thinkingSignature"))
                                    .and_then(|v| v.as_str())
                                    .map(|s| s.to_string());
                                push_node(
                                    nodes,
                                    approx_turn,
                                    &blk_ptr,
                                    Role::Assistant,
                                    NodeKind::AssistantThinking {
                                        thought,
                                        signature: sig,
                                    },
                                    blk_cache,
                                );
                            }
                            "tool_use" | "tool_call" | "toolCall" => {
                                let call_id = blk
                                    .get("id")
                                    .or_else(|| blk.get("call_id"))
                                    .or_else(|| blk.get("toolCallId"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let tool_name = blk
                                    .get("name")
                                    .or_else(|| blk.get("tool_name"))
                                    .or_else(|| blk.get("toolName"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("tool")
                                    .to_string();
                                let args = blk
                                    .get("input")
                                    .or_else(|| blk.get("arguments"))
                                    .cloned()
                                    .unwrap_or(Value::Null);
                                let sem = infer_call_semantic_meta(&tool_name, &args);
                                call_meta_by_id
                                    .insert(call_id.clone(), (tool_name.clone(), sem.clone()));
                                push_node(
                                    nodes,
                                    approx_turn,
                                    &blk_ptr,
                                    Role::Assistant,
                                    NodeKind::ToolCall {
                                        call_id,
                                        tool_name,
                                        arguments: args,
                                        semantic_meta: sem,
                                    },
                                    blk_cache,
                                );
                            }
                            "tool_result" | "toolResult" => {
                                let call_id = blk
                                    .get("tool_use_id")
                                    .or_else(|| blk.get("tool_call_id"))
                                    .or_else(|| blk.get("toolCallId"))
                                    .or_else(|| blk.get("id"))
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let is_error = blk
                                    .get("is_error")
                                    .or_else(|| blk.get("isError"))
                                    .and_then(|v| v.as_bool())
                                    .unwrap_or(false);
                                let content = extract_tool_result_text(blk.get("content"));
                                let (tool_name, call_sem) =
                                    blk.get("tool_name")
                                        .or_else(|| blk.get("toolName"))
                                        .and_then(|v| v.as_str())
                                        .map(|s| (s.to_string(), None))
                                        .unwrap_or_else(|| {
                                            call_meta_by_id.get(&call_id).cloned().unwrap_or_else(
                                                || ("tool_result".to_string(), None),
                                            )
                                        });
                                let sem = enrich_result_semantic_meta(call_sem, &content);
                                push_node(
                                    nodes,
                                    approx_turn,
                                    &blk_ptr,
                                    Role::Tool,
                                    NodeKind::ToolResult {
                                        call_id,
                                        tool_name,
                                        is_error,
                                        content,
                                        semantic_meta: sem,
                                    },
                                    blk_cache,
                                );
                            }
                            _ => {
                                let text = blk
                                    .get("text")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("")
                                    .to_string();
                                let kind = if role == Role::Assistant {
                                    NodeKind::AssistantText { text }
                                } else {
                                    NodeKind::UserMessage { text }
                                };
                                push_node(nodes, approx_turn, &blk_ptr, role, kind, blk_cache);
                            }
                        }
                    }
                }
                _ => {}
            }
            if let Some(ref ts) = msg_ts {
                for node in &mut nodes[before_len..] {
                    node.timestamp = Some(ts.clone());
                }
            }
        }
    }
    let _ = turn_seq;
}

fn parse_openai(payload: &Value, turn_seq: u32, nodes: &mut Vec<ContextNode>) {
    // 1. Tools (`/tools/i`)
    if let Some(tools) = payload.get("tools").and_then(|v| v.as_array()) {
        for (i, tool) in tools.iter().enumerate() {
            let func = tool.get("function").unwrap_or(tool);
            let name = func
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown_tool")
                .to_string();
            let desc = func
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let schema = func
                .get("parameters")
                .cloned()
                .unwrap_or(Value::Object(Default::default()));
            let (server_name, short_name) = split_mcp_tool_name(&name);
            push_node(
                nodes,
                1,
                &format!("/tools/{i}"),
                Role::System,
                NodeKind::McpToolSchema {
                    server_name,
                    tool_name: short_name,
                    description: desc,
                    schema_json: schema,
                },
                None,
            );
        }
    }
    let _ = turn_seq;

    // 2. Top-level instructions (OpenAI Responses API `/instructions`)
    if let Some(instr) = payload.get("instructions").and_then(|v| v.as_str()) {
        deconstruct_system_text(instr, "/instructions", 1, None, nodes);
    }

    if let Some(text) = payload.get("input").and_then(Value::as_str) {
        push_node(
            nodes,
            1,
            "/input",
            Role::User,
            NodeKind::UserMessage {
                text: text.to_owned(),
            },
            None,
        );
    }
    // 3. Messages or Input (`/messages` or `/input`)
    let (arr_key, messages_opt) =
        if let Some(m) = payload.get("messages").and_then(|v| v.as_array()) {
            ("messages", Some(m))
        } else if let Some(inp) = payload.get("input").and_then(|v| v.as_array()) {
            ("input", Some(inp))
        } else {
            ("messages", None)
        };

    let mut call_meta_by_id: HashMap<String, (String, Option<ToolSemanticMeta>)> = HashMap::new();

    if let Some(messages) = messages_opt {
        for (idx, msg) in messages.iter().enumerate() {
            let ptr = format!("/{arr_key}/{idx}");
            let role_str = msg
                .get("role")
                .or_else(|| msg.get("type"))
                .and_then(|v| v.as_str())
                .unwrap_or("user");
            let approx_turn = (idx as u32 / 2) + 1;

            match role_str {
                "system" | "developer" => {
                    let text = extract_tool_result_text(msg.get("content"));
                    deconstruct_system_text(&text, &format!("{ptr}/content"), 1, None, nodes);
                }
                "tool" | "function_call_output" => {
                    let call_id = msg
                        .get("tool_call_id")
                        .or_else(|| msg.get("call_id"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let content =
                        extract_tool_result_text(msg.get("content").or_else(|| msg.get("output")));
                    let (tool_name, call_sem) = call_meta_by_id
                        .get(&call_id)
                        .cloned()
                        .unwrap_or_else(|| ("tool".to_string(), None));
                    let is_error = content.contains("Command exited with code")
                        && !content.contains("Command exited with code 0");
                    let sem = enrich_result_semantic_meta(call_sem, &content);
                    push_node(
                        nodes,
                        approx_turn,
                        &ptr,
                        Role::Tool,
                        NodeKind::ToolResult {
                            call_id,
                            tool_name,
                            is_error,
                            content,
                            semantic_meta: sem,
                        },
                        None,
                    );
                }
                "function_call" => {
                    let call_id = msg
                        .get("call_id")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    let tool_name = msg
                        .get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("tool")
                        .to_string();
                    let arguments = match msg.get("arguments") {
                        Some(Value::String(s)) => {
                            serde_json::from_str(s).unwrap_or(Value::String(s.clone()))
                        }
                        Some(v) => v.clone(),
                        None => Value::Null,
                    };
                    let semantic_meta = infer_call_semantic_meta(&tool_name, &arguments);
                    call_meta_by_id
                        .insert(call_id.clone(), (tool_name.clone(), semantic_meta.clone()));
                    push_node(
                        nodes,
                        approx_turn,
                        &ptr,
                        Role::Assistant,
                        NodeKind::ToolCall {
                            call_id,
                            tool_name,
                            arguments,
                            semantic_meta,
                        },
                        None,
                    );
                }
                "assistant" => {
                    let content = extract_tool_result_text(msg.get("content"));
                    if !content.is_empty() {
                        push_node(
                            nodes,
                            approx_turn,
                            &format!("{ptr}/content"),
                            Role::Assistant,
                            NodeKind::AssistantText { text: content },
                            None,
                        );
                    }
                    if let Some(tcs) = msg.get("tool_calls").and_then(|v| v.as_array()) {
                        for (tc_idx, tc) in tcs.iter().enumerate() {
                            let call_id = tc
                                .get("id")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let func = tc.get("function").unwrap_or(tc);
                            let tool_name = func
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("tool")
                                .to_string();
                            let args_val = match func.get("arguments") {
                                Some(Value::String(s)) => {
                                    serde_json::from_str(s).unwrap_or(Value::String(s.clone()))
                                }
                                Some(v) => v.clone(),
                                None => Value::Null,
                            };
                            let sem = infer_call_semantic_meta(&tool_name, &args_val);
                            call_meta_by_id
                                .insert(call_id.clone(), (tool_name.clone(), sem.clone()));
                            push_node(
                                nodes,
                                approx_turn,
                                &format!("{ptr}/tool_calls/{tc_idx}"),
                                Role::Assistant,
                                NodeKind::ToolCall {
                                    call_id,
                                    tool_name,
                                    arguments: args_val,
                                    semantic_meta: sem,
                                },
                                None,
                            );
                        }
                    }
                }
                _ => {
                    let text = extract_tool_result_text(msg.get("content"));
                    push_node(
                        nodes,
                        approx_turn,
                        &format!("{ptr}/content"),
                        Role::User,
                        NodeKind::UserMessage { text },
                        None,
                    );
                }
            }
        }
    }
}

fn parse_gemini(payload: &Value, turn_seq: u32, nodes: &mut Vec<ContextNode>) {
    // 1. System Instruction (`/systemInstruction`)
    if let Some(sys) = payload.get("systemInstruction")
        && let Some(parts) = sys.get("parts").and_then(|v| v.as_array())
    {
        for (i, part) in parts.iter().enumerate() {
            if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                deconstruct_system_text(
                    text,
                    &format!("/systemInstruction/parts/{i}/text"),
                    1,
                    None,
                    nodes,
                );
            }
        }
    }

    // 2. Tools (`/tools/0/functionDeclarations/i`)
    if let Some(tools) = payload.get("tools").and_then(|v| v.as_array()) {
        for (t_idx, tool_group) in tools.iter().enumerate() {
            if let Some(decls) = tool_group
                .get("functionDeclarations")
                .and_then(|v| v.as_array())
            {
                for (d_idx, decl) in decls.iter().enumerate() {
                    let name = decl
                        .get("name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("tool")
                        .to_string();
                    let desc = decl
                        .get("description")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let schema = decl
                        .get("parameters")
                        .cloned()
                        .unwrap_or(Value::Object(Default::default()));
                    let (server_name, short_name) = split_mcp_tool_name(&name);
                    push_node(
                        nodes,
                        1,
                        &format!("/tools/{t_idx}/functionDeclarations/{d_idx}"),
                        Role::System,
                        NodeKind::McpToolSchema {
                            server_name,
                            tool_name: short_name,
                            description: desc,
                            schema_json: schema,
                        },
                        None,
                    );
                }
            }
        }
    }
    let _ = turn_seq;

    // 3. Contents (`/contents/i/parts/j`)
    let mut last_call_by_name: HashMap<String, Option<ToolSemanticMeta>> = HashMap::new();

    if let Some(contents) = payload.get("contents").and_then(|v| v.as_array()) {
        for (c_idx, content) in contents.iter().enumerate() {
            let role_str = content
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("user");
            let role = if role_str == "model" {
                Role::Assistant
            } else {
                Role::User
            };
            let approx_turn = (c_idx as u32 / 2) + 1;

            if let Some(parts) = content.get("parts").and_then(|v| v.as_array()) {
                for (p_idx, part) in parts.iter().enumerate() {
                    let ptr = format!("/contents/{c_idx}/parts/{p_idx}");
                    if part.get("thought").and_then(|v| v.as_bool()) == Some(true) {
                        let thought = part
                            .get("text")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        let sig = part
                            .get("thoughtSignature")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        push_node(
                            nodes,
                            approx_turn,
                            &ptr,
                            Role::Assistant,
                            NodeKind::AssistantThinking {
                                thought,
                                signature: sig,
                            },
                            None,
                        );
                    } else if let Some(fc) = part.get("functionCall") {
                        let name = fc
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("tool")
                            .to_string();
                        let args = fc.get("args").cloned().unwrap_or(Value::Null);
                        let sem = infer_call_semantic_meta(&name, &args);
                        last_call_by_name.insert(name.clone(), sem.clone());
                        push_node(
                            nodes,
                            approx_turn,
                            &ptr,
                            Role::Assistant,
                            NodeKind::ToolCall {
                                call_id: format!("gemini_{c_idx}_{p_idx}"),
                                tool_name: name,
                                arguments: args,
                                semantic_meta: sem,
                            },
                            None,
                        );
                    } else if let Some(fr) = part.get("functionResponse") {
                        let name = fr
                            .get("name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("tool")
                            .to_string();
                        let resp_str = fr
                            .get("response")
                            .map(extract_value_string)
                            .unwrap_or_default();
                        let call_sem = last_call_by_name.get(&name).cloned().flatten();
                        let sem = enrich_result_semantic_meta(call_sem, &resp_str);
                        push_node(
                            nodes,
                            approx_turn,
                            &ptr,
                            Role::Tool,
                            NodeKind::ToolResult {
                                call_id: format!("gemini_res_{c_idx}_{p_idx}"),
                                tool_name: name,
                                is_error: false,
                                content: resp_str,
                                semantic_meta: sem,
                            },
                            None,
                        );
                    } else if let Some(text) = part.get("text").and_then(|v| v.as_str()) {
                        let kind = if role == Role::Assistant {
                            NodeKind::AssistantText {
                                text: text.to_string(),
                            }
                        } else {
                            NodeKind::UserMessage {
                                text: text.to_string(),
                            }
                        };
                        push_node(nodes, approx_turn, &ptr, role, kind, None);
                    }
                }
            }
        }
    }
}

/// Deconstructs system text into base SystemPrompt, InjectedRule (`<RULE[...]>`, `CLAUDE.md`),
/// and SkillCatalog (`<skills>`).
fn deconstruct_system_text(
    text: &str,
    base_pointer: &str,
    turn_seq: u32,
    cache_ctrl: Option<CacheControlBreakpoint>,
    nodes: &mut Vec<ContextNode>,
) {
    let mut rules_found = Vec::new();
    for cap in XML_RULE_RE.captures_iter(text) {
        let source = cap.get(1).map(|m| m.as_str().to_string());
        let content = cap
            .get(2)
            .map(|m| m.as_str().trim().to_string())
            .unwrap_or_default();
        rules_found.push((source, content));
    }

    let mut skills_found: Option<(Vec<SkillEntry>, String)> = None;
    if let Some(cap) = XML_SKILLS_RE.captures(text) {
        let raw_skills = cap.get(1).map(|m| m.as_str()).unwrap_or("");
        let mut entries = Vec::new();
        for s_cap in SKILL_LINE_RE.captures_iter(raw_skills) {
            entries.push(SkillEntry {
                name: s_cap[1].to_string(),
                path: Some(s_cap[2].to_string()),
                description: s_cap[3].to_string(),
            });
        }
        skills_found = Some((entries, raw_skills.trim().to_string()));
    }

    // Push base system prompt (with extracted blocks stripped so we don't double-count tokens)
    let stripped = XML_RULE_RE.replace_all(text, "");
    let stripped = XML_SKILLS_RE.replace_all(&stripped, "");
    let clean_sys = stripped.trim();
    if !clean_sys.is_empty() || (rules_found.is_empty() && skills_found.is_none()) {
        push_node(
            nodes,
            turn_seq,
            base_pointer,
            Role::System,
            NodeKind::SystemPrompt {
                text: if clean_sys.is_empty() {
                    text.to_string()
                } else {
                    clean_sys.to_string()
                },
            },
            cache_ctrl.clone(),
        );
    }

    for (idx, (source_file, content)) in rules_found.into_iter().enumerate() {
        push_node(
            nodes,
            turn_seq,
            &format!("{base_pointer}#rule-{idx}"),
            Role::System,
            NodeKind::InjectedRule {
                source_file,
                content,
            },
            None,
        );
    }

    if let Some((skills, raw_text)) = skills_found {
        push_node(
            nodes,
            turn_seq,
            &format!("{base_pointer}#skills"),
            Role::System,
            NodeKind::SkillCatalog { skills, raw_text },
            None,
        );
    }
}

fn push_node(
    nodes: &mut Vec<ContextNode>,
    turn_seq: u32,
    raw_json_pointer: &str,
    role: Role,
    kind: NodeKind,
    cache_control: Option<CacheControlBreakpoint>,
) {
    let identity = match &kind {
        NodeKind::ToolCall { call_id, .. } | NodeKind::ToolResult { call_id, .. } => {
            call_id.as_str()
        }
        _ => raw_json_pointer,
    };
    let fp = NodeFingerprint::compute(
        role,
        kind.kind_tag(),
        identity,
        &kind.text_for_tokenization(),
    );
    nodes.push(ContextNode {
        id: format!("node_{}_{}", turn_seq, nodes.len()),
        turn_seq,
        fingerprint: fp,
        raw_json_pointer: raw_json_pointer.to_string(),
        role,
        kind,
        token_span: TokenSpan::default(),
        kv_status: KvNodeStatus::Uncached,
        attention_salience: 1.0,
        cache_control,
        active_mutation: None,
        timestamp: None,
    });
}

fn extract_cache_control(val: &Value) -> Option<CacheControlBreakpoint> {
    let cc = val.get("cache_control")?;
    let cache_type = cc
        .get("type")
        .and_then(|v| v.as_str())
        .unwrap_or("ephemeral")
        .to_string();
    let ttl = cc
        .get("ttl")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    Some(CacheControlBreakpoint { cache_type, ttl })
}

fn split_mcp_tool_name(name: &str) -> (Option<String>, String) {
    if let Some(rest) = name.strip_prefix("mcp_")
        && let Some((srv, tool)) = rest.split_once('_')
    {
        return (Some(srv.to_string()), tool.to_string());
    }
    if let Some((srv, tool)) = name.split_once("__") {
        return (Some(srv.to_string()), tool.to_string());
    }
    (None, name.to_string())
}

fn extract_tool_result_text(val: Option<&Value>) -> String {
    match val {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .and_then(|t| t.as_str())
                    .map(|s| s.to_string())
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Some(other) => extract_value_string(other),
        None => String::new(),
    }
}

fn extract_value_string(val: &Value) -> String {
    match val {
        Value::String(s) => s.clone(),
        other => serde_json::to_string(other).unwrap_or_default(),
    }
}

/// Infers semantic tool metadata (`FileRead`, `FileWrite`, `CommandExec`) from tool name & arguments
/// across Antigravity (`read_file`, `edit_file`, `write_file`, `run_command`),
/// Claude Code (`Read`, `Edit`, `Write`, `Bash`), Codex, Pi, and OpenCode.
pub fn infer_call_semantic_meta(tool_name: &str, args: &Value) -> Option<ToolSemanticMeta> {
    let lower = tool_name.to_lowercase();
    let path = args
        .get("Path")
        .or_else(|| args.get("path"))
        .or_else(|| args.get("file_path"))
        .or_else(|| args.get("filePath"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    if matches!(
        lower.as_str(),
        "read_file" | "read" | "view_file" | "cat" | "open_file"
    ) && let Some(p) = path
    {
        let start_line = args
            .get("StartLine")
            .or_else(|| args.get("start_line"))
            .or_else(|| args.get("offset"))
            .and_then(|v| v.as_u64())
            .map(|n| n as u32);
        let mut end_line = args
            .get("EndLine")
            .or_else(|| args.get("end_line"))
            .or_else(|| args.get("limit"))
            .and_then(|v| v.as_u64())
            .map(|n| n as u32);
        if args
            .get("EndLine")
            .or_else(|| args.get("end_line"))
            .is_none()
        {
            end_line = args.get("limit").and_then(Value::as_u64).map(|limit| {
                start_line
                    .unwrap_or(1)
                    .saturating_add(limit.saturating_sub(1) as u32)
            });
        }
        return Some(ToolSemanticMeta::FileRead {
            path: p,
            start_line,
            end_line,
            content_hash: String::new(),
        });
    }

    if matches!(
        lower.as_str(),
        "edit_file" | "write_file" | "edit" | "write" | "replace_in_file" | "apply_patch"
    ) && let Some(p) = path
    {
        let content_str = args
            .get("Content")
            .or_else(|| args.get("Replacement"))
            .or_else(|| args.get("content"))
            .or_else(|| args.get("new_string"))
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let hash = blake3::hash(content_str.as_bytes()).to_hex()[..16].to_string();
        return Some(ToolSemanticMeta::FileWrite {
            path: p,
            content_hash: hash,
        });
    }

    if matches!(
        lower.as_str(),
        "run_command" | "bash" | "exec" | "shell" | "execute_command"
    ) {
        let cmd = args
            .get("CommandLine")
            .or_else(|| args.get("command"))
            .or_else(|| args.get("cmd"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        if !cmd.is_empty() {
            return Some(ToolSemanticMeta::CommandExec {
                command: cmd,
                exit_code: None,
            });
        }
    }

    None
}

pub fn enrich_result_semantic_meta(
    call_meta: Option<ToolSemanticMeta>,
    result_content: &str,
) -> Option<ToolSemanticMeta> {
    match call_meta {
        Some(ToolSemanticMeta::FileRead {
            path,
            start_line,
            end_line,
            ..
        }) => {
            let hash = blake3::hash(result_content.trim().as_bytes()).to_hex()[..16].to_string();
            Some(ToolSemanticMeta::FileRead {
                path,
                start_line,
                end_line,
                content_hash: hash,
            })
        }
        Some(ToolSemanticMeta::CommandExec { command, .. }) => {
            let exit_code = if result_content.contains("Command exited with code 0") {
                Some(0)
            } else if result_content.contains("Command exited with code ") {
                Some(1)
            } else {
                None
            };
            Some(ToolSemanticMeta::CommandExec { command, exit_code })
        }
        other => other,
    }
}

/// Active edits require a complete, supported text representation.
pub fn supports_mutation_payload(payload: &Value, provider: ProviderKind) -> bool {
    fn content(value: &Value) -> bool {
        match value {
            Value::String(_) | Value::Null => true,
            Value::Array(blocks) => {
                blocks
                    .iter()
                    .all(|block| match block.get("type").and_then(Value::as_str) {
                        Some(
                            "text" | "input_text" | "output_text" | "thinking" | "tool_use"
                            | "toolCall",
                        ) => true,
                        Some("tool_result") => block.get("content").is_none_or(content),
                        _ => false,
                    })
            }
            _ => false,
        }
    }
    if payload
        .get("cw_analysis_incomplete")
        .and_then(Value::as_bool)
        == Some(true)
        || payload
            .get("previous_response_id")
            .is_some_and(|v| !v.is_null())
        || payload.get("conversation").is_some_and(|v| !v.is_null())
    {
        return false;
    }
    if provider == ProviderKind::Gemini {
        return payload
            .get("contents")
            .and_then(Value::as_array)
            .is_some_and(|items| {
                items.iter().all(|item| {
                    item.get("parts")
                        .and_then(Value::as_array)
                        .is_some_and(|parts| {
                            parts.iter().all(|part| {
                                part.as_object().is_some_and(|map| {
                                    map.keys().all(|key| {
                                        matches!(
                                            key.as_str(),
                                            "text" | "functionCall" | "functionResponse"
                                        )
                                    })
                                })
                            })
                        })
                })
            });
    }
    if let Some(items) = payload.get("messages").and_then(Value::as_array) {
        return items
            .iter()
            .all(|item| item.get("content").is_none_or(content));
    }
    match payload.get("input") {
        Some(Value::String(_)) => true,
        Some(Value::Array(items)) => {
            items
                .iter()
                .all(|item| match item.get("type").and_then(Value::as_str) {
                    Some("function_call") => true,
                    Some("function_call_output") => item.get("output").is_none_or(content),
                    Some("message") | None => item.get("content").is_some_and(content),
                    _ => false,
                })
        }
        _ => false,
    }
}
