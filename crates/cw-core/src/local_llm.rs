use crate::ir::{ContextNode, ContextTurn, NodeKind, ProviderKind};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CostAndPerfReport {
    pub basis: String,
    pub model_family: String,
    pub is_local_model: bool,
    pub turn_cost_usd: f64,
    pub unoptimized_cost_usd: f64,
    pub saved_by_kv_cache_usd: f64,
    pub saved_by_cw_overlays_usd: f64,
    pub estimated_prefill_ms: u64,
    pub saved_prefill_ms_by_kv: u64,
    pub saved_prefill_ms_by_overlays: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalEngineRecommendation {
    pub id: String,
    pub severity: String, // "critical" | "warning" | "info"
    pub title: String,
    pub explanation: String,
    pub cli_or_api_fix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocalModelAnalysis {
    pub is_local_model: bool,
    pub engine_kind: String, // "ollama" | "llamacpp" | "vllm" | "sglang" | "lmstudio" | "local_compatible"
    pub model_name: String,
    pub num_ctx_limit: u32,
    pub used_ctx_pct: f32,
    pub recommended_n_keep_tokens: u32,
    pub recommended_num_ctx: u32,
    pub recommended_slot_id: u32,
    pub paged_attention_block_size: u32,
    pub prefix_block_aligned_tokens: u32,
    pub prefix_block_waste_tokens: u32,
    pub vram_fp16_mb: f32,
    pub vram_q8_0_mb: f32,
    pub vram_q4_0_mb: f32,
    pub vram_saved_by_overlays_fp16_mb: f32,
    pub sliding_window_overflow_risk: bool,
    pub sliding_window_overflow_tokens: u32,
    pub estimated_prefill_tps: u32,
    pub prefill_latency_ms: u64,
    pub prefill_saved_by_kv_ms: u64,
    pub recommendations: Vec<LocalEngineRecommendation>,
}

/// Detects whether a model name or provider corresponds to a local LLM engine
/// (`Ollama`, `llama.cpp`, `vLLM`, `SGLang`, `LM Studio`).
pub fn detect_local_engine(model: &str, provider: ProviderKind) -> (bool, &'static str) {
    let m = model.to_lowercase();
    if m.contains("ollama") || m.contains(':') && !m.starts_with("claude") && !m.starts_with("gpt")
    {
        return (true, "ollama");
    }
    if m.contains("gguf") || m.contains("llamacpp") || m.contains("llama.cpp") {
        return (true, "llamacpp");
    }
    if m.contains("vllm") {
        return (true, "vllm");
    }
    if m.contains("sglang") {
        return (true, "sglang");
    }
    if m.contains("lmstudio") {
        return (true, "lmstudio");
    }
    if m.starts_with("qwen")
        || m.starts_with("llama")
        || m.starts_with("deepseek")
        || m.starts_with("mistral")
        || m.starts_with("devstral")
        || m.starts_with("codellama")
        || m.starts_with("gemma")
        || m.starts_with("phi")
        || m.starts_with("starcoder")
        || m.starts_with("glm")
    {
        return (true, "ollama");
    }
    if provider == ProviderKind::OpenAICompatible {
        return (true, "local_compatible");
    }
    (false, "cloud")
}

/// Estimates USD ($) cost, KV Cache ($) savings, `cw` overlay ($) savings,
/// and prefill latency (ms) for any `ContextTurn`.
pub fn calculate_cost_and_perf(
    turn: &ContextTurn,
    model: &str,
    provider: ProviderKind,
) -> CostAndPerfReport {
    let m = model.to_lowercase();
    let (is_local, _) = detect_local_engine(model, provider);

    // Pricing per 1M tokens: (base_input_usd, cache_write_usd, cache_read_usd, output_usd, prefill_tok_per_sec)
    let (family, in_per_m, write_per_m, read_per_m, out_per_m, prefill_tps) = if is_local {
        (
            "Local LLM (Zero API Cost • VRAM Bound)",
            0.0,
            0.0,
            0.0,
            0.0,
            650u64,
        )
    } else if m.contains("opus") {
        ("Claude Opus 4", 15.0, 18.75, 1.50, 75.0, 18_000)
    } else if m.contains("sonnet") || m.contains("claude") {
        ("Claude Sonnet 4", 3.0, 3.75, 0.30, 15.0, 28_000)
    } else if m.contains("haiku") {
        ("Claude Haiku", 0.80, 1.00, 0.08, 4.0, 45_000)
    } else if m.contains("gemini") || m.contains("argon") {
        ("Gemini Pro / Antigravity", 1.25, 1.25, 0.3125, 5.0, 32_000)
    } else if m.contains("gpt-5") || m.contains("o3") || m.contains("gpt-4") {
        ("OpenAI GPT-5 / o3", 2.50, 2.50, 0.25, 10.0, 30_000)
    } else {
        ("Standard Cloud LLM", 2.50, 2.50, 0.25, 10.0, 25_000)
    };

    let hit_tok = turn.kv_cache_report.hit_tokens as f64;
    let new_tok = (turn.kv_cache_report.newly_cached_tokens
        + turn.kv_cache_report.invalidated_tokens
        + turn.kv_cache_report.uncached_tail_tokens) as f64;
    let out_tok = turn.token_usage.output_tokens as f64;
    let saved_overlay_tok = turn.token_usage.saved_by_mutations as f64;
    let total_in_tok = turn.token_usage.input_tokens as f64;

    let turn_cost_usd =
        ((hit_tok * read_per_m) + (new_tok * write_per_m) + (out_tok * out_per_m)) / 1_000_000.0;

    let unoptimized_cost_usd =
        (((total_in_tok + saved_overlay_tok) * in_per_m) + (out_tok * out_per_m)) / 1_000_000.0;

    let saved_by_kv_cache_usd = ((hit_tok * (in_per_m - read_per_m)) / 1_000_000.0).max(0.0);
    let saved_by_cw_overlays_usd = ((saved_overlay_tok * in_per_m) / 1_000_000.0).max(0.0);

    // Prefill latency: cached tokens evaluate ~12x faster than uncached tokens (especially on local GPUs!)
    let uncached_ms = ((new_tok as u64) * 1000) / prefill_tps.max(1);
    let cached_ms = ((hit_tok as u64) * 1000) / (prefill_tps * 12).max(1);
    let estimated_prefill_ms = uncached_ms + cached_ms;
    let saved_prefill_ms_by_kv =
        (((hit_tok as u64) * 1000) / prefill_tps.max(1)).saturating_sub(cached_ms);
    let saved_prefill_ms_by_overlays = ((saved_overlay_tok as u64) * 1000) / prefill_tps.max(1);

    CostAndPerfReport {
        basis: "heuristic token spans, legacy family rates, and fixed throughput; not billing or measured latency".into(),        model_family: family.to_string(),
        is_local_model: is_local,
        turn_cost_usd,
        unoptimized_cost_usd,
        saved_by_kv_cache_usd,
        saved_by_cw_overlays_usd,
        estimated_prefill_ms,
        saved_prefill_ms_by_kv,
        saved_prefill_ms_by_overlays,
    }
}

/// Deep KV Cache, VRAM, Slot Affinity, and `n_keep` / `num_keep` analyzer specifically designed
/// for local LLM inference engines (`Ollama`, `llama.cpp`, `vLLM`, `SGLang`, `LM Studio`).
pub fn analyze_local_model_kv(
    turn: &ContextTurn,
    session_id: &str,
    model: &str,
    provider: ProviderKind,
) -> LocalModelAnalysis {
    let (is_local, engine_kind) = detect_local_engine(model, provider);

    // Calculate exact prefix tokens (`SystemPrompt` + `InjectedRule` + `SkillCatalog` + `McpToolSchema`)
    // that MUST be pinned via `n_keep` (llama.cpp) or `options.num_keep` (Ollama) so a sliding-window
    // shift never evicts the system prompt or tool definitions!
    let mut prefix_keep_tokens: u32 = 0;
    for node in &turn.nodes {
        match node.kind {
            NodeKind::SystemPrompt { .. }
            | NodeKind::InjectedRule { .. }
            | NodeKind::SkillCatalog { .. }
            | NodeKind::McpToolSchema { .. } => {
                prefix_keep_tokens += node.token_span.count;
            }
            _ => {}
        }
    }
    let recommended_n_keep_tokens = prefix_keep_tokens.max(256);

    // Local models typically default to 32,768 (or even 4,096 in stock Ollama!)
    let num_ctx_limit =
        if turn.token_usage.context_limit > 0 && turn.token_usage.context_limit <= 131_072 {
            turn.token_usage.context_limit
        } else {
            32_768
        };

    let input_tokens = turn.token_usage.input_tokens;
    let used_ctx_pct = ((input_tokens as f32 / num_ctx_limit.max(1) as f32) * 100.0).min(999.9);
    let recommended_num_ctx = (input_tokens + 8192)
        .next_power_of_two()
        .clamp(16_384, 131_072);

    // Deterministic slot ID (0..3) for `llama.cpp` `id_slot` so concurrent agents don't overwrite each other's KV slot
    let slot_hash = blake3::hash(session_id.as_bytes());
    let recommended_slot_id = (slot_hash.as_bytes()[0] % 4) as u32;

    // PagedAttention block alignment (16 tokens per block in vLLM / llama.cpp, 64 in SGLang)
    let block_size = if engine_kind == "sglang" {
        64u32
    } else {
        16u32
    };
    let prefix_block_aligned_tokens = (recommended_n_keep_tokens / block_size) * block_size;
    let prefix_block_waste_tokens = recommended_n_keep_tokens % block_size;

    // KV Cache VRAM Footprint estimation (calibrated for a 32B GQA model: ~0.25 MB per token in FP16,
    // ~0.13 MB/tok in Q8_0, ~0.075 MB/tok in Q4_0)
    let mb_per_tok_fp16 = 0.25_f32;
    let vram_fp16_mb = input_tokens as f32 * mb_per_tok_fp16;
    let vram_q8_0_mb = vram_fp16_mb * 0.53;
    let vram_q4_0_mb = vram_fp16_mb * 0.30;
    let vram_saved_by_overlays_fp16_mb =
        turn.token_usage.saved_by_mutations as f32 * mb_per_tok_fp16;

    // Sliding window overflow risk: when `input_tokens > 80% of num_ctx_limit`, Ollama / llama.cpp
    // triggers a middle-context shift that invalidates all KV tokens after `n_keep`!
    let sliding_window_overflow_risk = input_tokens > (num_ctx_limit * 4 / 5);
    let sliding_window_overflow_tokens = input_tokens.saturating_sub(num_ctx_limit * 4 / 5);

    // Local GPU prefill speed (~650 tok/s on consumer GPU / Apple M-series Max)
    let estimated_prefill_tps = 650u32;
    let uncached_tokens = turn.kv_cache_report.newly_cached_tokens
        + turn.kv_cache_report.invalidated_tokens
        + turn.kv_cache_report.uncached_tail_tokens;
    let prefill_latency_ms = ((uncached_tokens as u64) * 1000) / estimated_prefill_tps as u64;
    let prefill_saved_by_kv_ms =
        ((turn.kv_cache_report.hit_tokens as u64) * 1000) / estimated_prefill_tps as u64;

    let mut recommendations = Vec::new();

    // Recommendation 1: Pin `n_keep` / `num_keep` to protect System Prompt & Tools
    recommendations.push(LocalEngineRecommendation {
        id: "pin_n_keep".to_string(),
        severity: "info".to_string(),
        title: format!(
            "Lock System Prefix in VRAM (n_keep / num_keep = {} tok)",
            recommended_n_keep_tokens
        ),
        explanation: format!(
            "Your System Prompt, Project Rules (AGENTS.md), and Tool Schemas occupy the first {} tokens. When routed through contextWatcher's local proxy, cw automatically injects `n_keep: {}` (llama.cpp) and `options.num_keep: {}` (Ollama) so context shifts NEVER evict your system instructions or tool schemas from VRAM.",
            recommended_n_keep_tokens, recommended_n_keep_tokens, recommended_n_keep_tokens
        ),
        cli_or_api_fix: format!(
            "{{\"cache_prompt\": true, \"id_slot\": {}, \"n_keep\": {}, \"options\": {{\"num_keep\": {}, \"num_ctx\": {}}}}}",
            recommended_slot_id, recommended_n_keep_tokens, recommended_n_keep_tokens, recommended_num_ctx
        ),
    });

    // Recommendation 2: Sliding-Window Context Overflow Guard
    if sliding_window_overflow_risk {
        recommendations.push(LocalEngineRecommendation {
            id: "sliding_window_overflow".to_string(),
            severity: "critical".to_string(),
            title: format!(
                "⚠️ Local Context Window Overflow Risk ({} / {} tok = {:.1}%)",
                input_tokens, num_ctx_limit, used_ctx_pct
            ),
            explanation: format!(
                "When a prompt exceeds `num_ctx` ({}), Ollama and llama.cpp silently drop middle conversation turns and recompute the entire shifted KV cache (causing a 15–60 second freeze!). Run 'Surgical Tool Log Diet' or 'Max Token Diet' right now to free tokens cleanly before hitting the wall.",
                num_ctx_limit
            ),
            cli_or_api_fix: "cw optimize --strategy max_token_diet".to_string(),
        });
    }

    // Recommendation 3: KV Cache Quantization (`Q8_0` / `Q4_0`)
    if vram_fp16_mb > 1024.0 {
        recommendations.push(LocalEngineRecommendation {
            id: "kv_cache_quantization".to_string(),
            severity: "warning".to_string(),
            title: format!(
                "KV Cache Consumes {:.1} GB VRAM in FP16 — Enable Q8_0 KV Quantization to Save {:.0} MB",
                vram_fp16_mb / 1024.0,
                vram_fp16_mb - vram_q8_0_mb
            ),
            explanation: "Enabling 8-bit KV cache quantization (`-ctk q8_0 -ctv q8_0` in llama.cpp or `OLLAMA_KV_CACHE_TYPE=q8_0` in Ollama) cuts KV VRAM usage by ~47% with virtually zero perplexity loss, allowing 2x longer agent sessions without VRAM swapping.".to_string(),
            cli_or_api_fix: "OLLAMA_KV_CACHE_TYPE=q8_0 FLASH_ATTENTION=1 ollama serve   # or: llama-server -fa -ctk q8_0 -ctv q8_0".to_string(),
        });
    }

    // Recommendation 4: Slot Affinity (`id_slot`)
    recommendations.push(LocalEngineRecommendation {
        id: "slot_affinity".to_string(),
        severity: "info".to_string(),
        title: format!(
            "Deterministic KV Slot Affinity (Slot #{recommended_slot_id} for session '{session_id}')"
        ),
        explanation: "When harnesses spawn subagents (e.g. a research subagent alongside your main coding agent), local servers with `--parallel 4` can accidentally assign the subagent to your main agent's KV slot, wiping 30k+ cached tokens. contextWatcher pins each session to a deterministic `id_slot` / `cache_salt`.".to_string(),
        cli_or_api_fix: format!("id_slot: {recommended_slot_id}, cache_prompt: true"),
    });

    LocalModelAnalysis {
        is_local_model: is_local,
        engine_kind: engine_kind.to_string(),
        model_name: model.to_string(),
        num_ctx_limit,
        used_ctx_pct,
        recommended_n_keep_tokens,
        recommended_num_ctx,
        recommended_slot_id,
        paged_attention_block_size: block_size,
        prefix_block_aligned_tokens,
        prefix_block_waste_tokens,
        vram_fp16_mb,
        vram_q8_0_mb,
        vram_q4_0_mb,
        vram_saved_by_overlays_fp16_mb,
        sliding_window_overflow_risk,
        sliding_window_overflow_tokens,
        estimated_prefill_tps,
        prefill_latency_ms,
        prefill_saved_by_kv_ms,
        recommendations,
    }
}

/// Injects local-engine KV Cache optimization parameters (`cache_prompt`, `n_keep`, `id_slot`,
/// `options.num_keep`, `options.num_ctx`, `keep_alive`) directly into outgoing local LLM payloads.
pub fn inject_local_engine_kv_optimizations(
    payload: &mut Value,
    session_id: &str,
    nodes: &[ContextNode],
    engine_hint: &str,
) -> Vec<String> {
    let mut applied = Vec::new();
    let Some(obj) = payload.as_object_mut() else {
        return applied;
    };

    let mut prefix_keep_tokens: u32 = 0;
    let mut total_tokens: u32 = 0;
    for node in nodes {
        total_tokens += node.token_span.count;
        if matches!(
            node.kind,
            NodeKind::SystemPrompt { .. }
                | NodeKind::InjectedRule { .. }
                | NodeKind::SkillCatalog { .. }
                | NodeKind::McpToolSchema { .. }
        ) {
            prefix_keep_tokens += node.token_span.count;
        }
    }
    let n_keep = prefix_keep_tokens.max(256);
    let recommended_ctx = (total_tokens + 8192)
        .next_power_of_two()
        .clamp(16_384, 131_072);
    let slot_id = (blake3::hash(session_id.as_bytes()).as_bytes()[0] % 4) as u32;

    match engine_hint {
        "ollama" => {
            obj.entry("keep_alive".to_string())
                .or_insert_with(|| Value::String("30m".to_string()));
            let options = obj
                .entry("options".to_string())
                .or_insert_with(|| json!({}));
            if let Some(opt_map) = options.as_object_mut() {
                opt_map
                    .entry("num_keep".to_string())
                    .or_insert_with(|| json!(n_keep));
                opt_map
                    .entry("num_ctx".to_string())
                    .or_insert_with(|| json!(recommended_ctx));
            }
            applied.push(format!(
                "ollama:keep_alive=30m,num_keep={n_keep},num_ctx={recommended_ctx}"
            ));
        }
        "llamacpp" => {
            obj.entry("cache_prompt".to_string())
                .or_insert(Value::Bool(true));
            obj.entry("id_slot".to_string())
                .or_insert_with(|| json!(slot_id));
            obj.entry("n_keep".to_string())
                .or_insert_with(|| json!(n_keep));
            applied.push(format!(
                "llamacpp:cache_prompt=true,id_slot={slot_id},n_keep={n_keep}"
            ));
        }
        "vllm" | "sglang" => {
            let extra = obj
                .entry("extra_body".to_string())
                .or_insert_with(|| json!({}));
            if let Some(ext_map) = extra.as_object_mut() {
                ext_map
                    .entry("cache_salt".to_string())
                    .or_insert_with(|| Value::String(session_id.to_string()));
            }
            applied.push(format!("{engine_hint}:cache_salt={session_id}"));
        }
        _ => {}
    }

    applied
}

/// Strips any internal `cw_*` metadata fields (`cw_timestamp`, `cw_turn_seq`, etc.) from a JSON
/// payload before forwarding to strict upstream LLM providers (Anthropic, OpenAI, Gemini).
pub fn strip_internal_cw_fields(val: &mut Value) {
    for key in ["messages", "input", "contents"] {
        if let Some(items) = val.get_mut(key).and_then(Value::as_array_mut) {
            for item in items {
                if let Some(map) = item.as_object_mut() {
                    map.remove("cw_timestamp");
                    map.remove("cw_turn_seq");
                }
            }
        }
    }
}
