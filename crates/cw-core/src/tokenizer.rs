use crate::ir::{ContextNode, NodeKind, ProviderKind, TokenSpan};

/// Estimates token count deterministically without requiring network downloads.
/// Uses subword segmentation heuristics calibrated against cl100k_base / Claude / Gemini tokenizers
/// (~3.6 chars per token for prose/code, counting punctuation/delimiters and whitespace boundaries).
pub fn estimate_tokens(text: &str) -> u32 {
    if text.is_empty() {
        return 0;
    }
    let mut tokens: u32 = 0;
    let mut current_word_len: u32 = 0;

    for ch in text.chars() {
        if ch.is_alphanumeric() || ch == '_' {
            current_word_len += 1;
            // Long identifiers/base64 split roughly every 4 chars
            if current_word_len >= 5 {
                tokens += 1;
                current_word_len = 0;
            }
        } else {
            if current_word_len > 0 {
                tokens += 1;
                current_word_len = 0;
            }
            if !ch.is_whitespace() {
                tokens += 1;
            }
        }
    }
    if current_word_len > 0 {
        tokens += 1;
    }
    tokens.max(1)
}

/// Orders indices of `nodes` according to how the provider serializes the KV-cache prefix:
/// - Anthropic: `tools` -> `system` -> `messages`
/// - OpenAI / Gemini: `system` -> `tools` -> `messages`
pub fn prefix_serialization_order(nodes: &[ContextNode], provider: ProviderKind) -> Vec<usize> {
    let mut tool_schemas = Vec::new();
    let mut system_blocks = Vec::new();
    let mut conversation = Vec::new();

    for (idx, node) in nodes.iter().enumerate() {
        match &node.kind {
            NodeKind::McpToolSchema { .. } => tool_schemas.push(idx),
            NodeKind::SystemPrompt { .. }
            | NodeKind::InjectedRule { .. }
            | NodeKind::SkillCatalog { .. } => system_blocks.push(idx),
            _ => conversation.push(idx),
        }
    }

    let mut ordered = Vec::with_capacity(nodes.len());
    match provider {
        ProviderKind::Anthropic => {
            ordered.extend(tool_schemas);
            ordered.extend(system_blocks);
            ordered.extend(conversation);
        }
        ProviderKind::OpenAI | ProviderKind::Gemini | ProviderKind::OpenAICompatible => {
            ordered.extend(system_blocks);
            ordered.extend(tool_schemas);
            ordered.extend(conversation);
        }
    }
    ordered
}

/// Populates `token_span` and `attention_salience` across all nodes in a turn,
/// returning the total input token count.
pub fn assign_token_spans_and_salience(nodes: &mut [ContextNode], provider: ProviderKind) -> u32 {
    let order = prefix_serialization_order(nodes, provider);
    let mut cursor: u32 = 0;

    for &idx in &order {
        let count = estimate_tokens(&nodes[idx].kind.text_for_tokenization()).max(4);
        nodes[idx].token_span = TokenSpan {
            start: cursor,
            end: cursor + count,
            count,
        };
        cursor += count;
    }

    let total_tokens = cursor.max(1);
    for node in nodes.iter_mut() {
        let midpoint = (node.token_span.start + node.token_span.end) as f32 / 2.0;
        let rel_pos = (midpoint / total_tokens as f32).clamp(0.0, 1.0);
        node.attention_salience = compute_u_shaped_salience(rel_pos);
    }

    cursor
}

/// U-shaped positional salience curve ("Lost in the Middle" model, Liu et al.):
/// Primacy zone (0.0..0.15) and Recency zone (0.80..1.0) have high attention (0.75..1.0);
/// middle trough (0.35..0.65) drops down toward ~0.25.
pub fn compute_u_shaped_salience(rel_pos: f32) -> f32 {
    // Centered parabola with slight recency boost
    let centered = (rel_pos - 0.45) / 0.55; // [-0.81 .. +1.0]
    let base = 0.25 + 0.65 * (centered * centered).clamp(0.0, 1.0);
    let recency_bonus = if rel_pos > 0.85 {
        (rel_pos - 0.85) * 0.66
    } else {
        0.0
    };
    (base + recency_bonus).clamp(0.20, 1.0)
}
