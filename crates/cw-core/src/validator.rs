use crate::ir::ProviderKind;
use serde_json::Value;
use std::collections::HashSet;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq, Clone)]
pub enum ProtocolViolation {
    #[error(
        "Anthropic invariant violation: roles must strictly alternate (found consecutive '{0}' at message index {1})"
    )]
    ConsecutiveRoles(String, usize),

    #[error("Anthropic invariant violation: first message must have role 'user' (found '{0}')")]
    InvalidFirstMessageRole(String),

    #[error(
        "Tool call pairing violation: orphan tool_result with call_id '{call_id}' has no matching prior tool_use"
    )]
    OrphanToolResult { call_id: String },

    #[error(
        "Anthropic thinking invariant violation: thinking block at index {index} appears after text block"
    )]
    ThinkingBlockAfterText { index: usize },

    #[error("Payload structure violation: {0}")]
    CorruptedStructure(String),
}

/// Checks locally modeled structural constraints; this is not full provider validation.
///
/// Returns `Ok(())` if valid, or `Err(ProtocolViolation)` if the payload contains
/// structural defects (orphan tools, broken role alternation, invalid thinking block positions)
/// that would trigger an HTTP 400 from Anthropic, OpenAI, or Gemini.
pub fn validate_protocol_invariants(
    payload: &Value,
    provider: ProviderKind,
) -> Result<(), ProtocolViolation> {
    match provider {
        ProviderKind::Anthropic => validate_anthropic_payload(payload),
        ProviderKind::OpenAI | ProviderKind::OpenAICompatible => validate_openai_payload(payload),
        ProviderKind::Gemini => validate_gemini_payload(payload),
    }
}

fn validate_anthropic_payload(payload: &Value) -> Result<(), ProtocolViolation> {
    let Some(messages) = payload.get("messages").and_then(|v| v.as_array()) else {
        // Payloads without a messages array (e.g. models list or token count) are valid
        return Ok(());
    };

    if messages.is_empty() {
        return Ok(());
    }

    let mut known_tool_use_ids: HashSet<String> = HashSet::new();

    for (idx, msg) in messages.iter().enumerate() {
        let role = msg
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("unknown");

        // 1. First message must be 'user'
        if idx == 0 && role != "user" {
            return Err(ProtocolViolation::InvalidFirstMessageRole(role.to_string()));
        }

        // Consecutive roles are combined by the Messages API.
        // 3. Inspect content blocks
        if let Some(content_arr) = msg.get("content").and_then(|c| c.as_array()) {
            let mut seen_text_block = false;

            for (c_idx, block) in content_arr.iter().enumerate() {
                let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");

                match block_type {
                    "thinking" => {
                        if seen_text_block && role == "assistant" {
                            return Err(ProtocolViolation::ThinkingBlockAfterText { index: c_idx });
                        }
                    }
                    "text" => {
                        seen_text_block = true;
                    }
                    "tool_use" => {
                        if let Some(id) = block.get("id").and_then(|i| i.as_str()) {
                            known_tool_use_ids.insert(id.to_string());
                        }
                    }
                    "tool_result" => {
                        if let Some(tool_use_id) = block.get("tool_use_id").and_then(|i| i.as_str())
                            && !known_tool_use_ids.contains(tool_use_id)
                        {
                            return Err(ProtocolViolation::OrphanToolResult {
                                call_id: tool_use_id.to_string(),
                            });
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

fn validate_openai_payload(payload: &Value) -> Result<(), ProtocolViolation> {
    let Some(messages) = payload.get("messages").and_then(|v| v.as_array()) else {
        return Ok(());
    };

    let mut known_tool_calls: HashSet<String> = HashSet::new();

    for msg in messages {
        let role = msg
            .get("role")
            .and_then(|r| r.as_str())
            .unwrap_or("unknown");

        if role == "assistant" {
            if let Some(tool_calls) = msg.get("tool_calls").and_then(|tc| tc.as_array()) {
                for tc in tool_calls {
                    if let Some(id) = tc.get("id").and_then(|i| i.as_str()) {
                        known_tool_calls.insert(id.to_string());
                    }
                }
            }
        } else if role == "tool" {
            let tool_call_id = msg
                .get("tool_call_id")
                .and_then(|i| i.as_str())
                .unwrap_or("");

            if !tool_call_id.is_empty() && !known_tool_calls.contains(tool_call_id) {
                return Err(ProtocolViolation::OrphanToolResult {
                    call_id: tool_call_id.to_string(),
                });
            }
        }
    }

    Ok(())
}

fn validate_gemini_payload(payload: &Value) -> Result<(), ProtocolViolation> {
    let Some(contents) = payload.get("contents").and_then(|v| v.as_array()) else {
        return Ok(());
    };

    let mut prev_role: Option<&str> = None;
    for (idx, item) in contents.iter().enumerate() {
        let role = item.get("role").and_then(|r| r.as_str()).unwrap_or("user");

        if let Some(prev) = prev_role
            && prev == role
        {
            return Err(ProtocolViolation::ConsecutiveRoles(role.to_string(), idx));
        }
        prev_role = Some(role);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_anthropic_valid_payload() {
        let payload = json!({
            "model": "claude-3-7-sonnet-20250219",
            "messages": [
                {
                    "role": "user",
                    "content": "Check auth.rs"
                },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "thinking", "thinking": "Let me read the file" },
                        { "type": "text", "text": "Reading auth.rs" },
                        { "type": "tool_use", "id": "call_123", "name": "read_file", "input": {} }
                    ]
                },
                {
                    "role": "user",
                    "content": [
                        { "type": "tool_result", "tool_use_id": "call_123", "content": "pub fn auth() {}" }
                    ]
                }
            ]
        });

        assert_eq!(
            validate_protocol_invariants(&payload, ProviderKind::Anthropic),
            Ok(())
        );
    }

    #[test]
    fn test_anthropic_accepts_consecutive_roles() {
        let payload = json!({
            "model": "claude-3-7-sonnet",
            "messages": [
                { "role": "user", "content": "Hello" },
                { "role": "user", "content": "Are you there?" }
            ]
        });

        let res = validate_protocol_invariants(&payload, ProviderKind::Anthropic);
        assert_eq!(res, Ok(()));
    }

    #[test]
    fn test_anthropic_detects_orphan_tool_result() {
        let payload = json!({
            "model": "claude-3-7-sonnet",
            "messages": [
                { "role": "user", "content": "Hello" },
                { "role": "assistant", "content": [{ "type": "text", "text": "Hi" }] },
                {
                    "role": "user",
                    "content": [
                        { "type": "tool_result", "tool_use_id": "orphan_call_999", "content": "result" }
                    ]
                }
            ]
        });

        let res = validate_protocol_invariants(&payload, ProviderKind::Anthropic);
        assert_eq!(
            res,
            Err(ProtocolViolation::OrphanToolResult {
                call_id: "orphan_call_999".to_string()
            })
        );
    }

    #[test]
    fn test_anthropic_detects_thinking_after_text() {
        let payload = json!({
            "model": "claude-3-7-sonnet",
            "messages": [
                { "role": "user", "content": "Hello" },
                {
                    "role": "assistant",
                    "content": [
                        { "type": "text", "text": "Hi" },
                        { "type": "thinking", "thinking": "Late thought" }
                    ]
                }
            ]
        });

        let res = validate_protocol_invariants(&payload, ProviderKind::Anthropic);
        assert_eq!(
            res,
            Err(ProtocolViolation::ThinkingBlockAfterText { index: 1 })
        );
    }

    #[test]
    fn test_openai_detects_orphan_tool_call_id() {
        let payload = json!({
            "model": "gpt-4o",
            "messages": [
                { "role": "user", "content": "Run tests" },
                { "role": "assistant", "content": "Sure" },
                { "role": "tool", "tool_call_id": "ghost_call", "content": "fail" }
            ]
        });

        let res = validate_protocol_invariants(&payload, ProviderKind::OpenAI);
        assert_eq!(
            res,
            Err(ProtocolViolation::OrphanToolResult {
                call_id: "ghost_call".to_string()
            })
        );
    }
}
