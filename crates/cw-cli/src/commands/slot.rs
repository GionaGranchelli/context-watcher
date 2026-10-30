use anyhow::Result;
use cw_core::slot_bridge::{
    DEFAULT_PAGED_ATTENTION_BLOCK_SIZE, EngineSlotInfo, SlotSplicingAction, SlotSplicingPlan,
    assign_deterministic_slot, plan_slot_splicing,
};
use cw_store::CwStore;

use super::session::resolve_session;

/// Generates and prints the Engine-Level PagedAttention & Slot Bridge report (Horizon 3 / CW-070).
pub async fn run_slot_bridge_report(
    store: &CwStore,
    session: Option<String>,
    engine: Option<String>,
    slot_id_filter: Option<u32>,
    splice_range_str: Option<String>,
    output_json: bool,
) -> Result<()> {
    let upstream_url = engine
        .or_else(|| std::env::var("CW_LLAMACPP_UPSTREAM").ok())
        .unwrap_or_else(|| "http://127.0.0.1:8080".to_string());

    let target_session_id = resolve_session(store, session)?;
    let target_turn = match &target_session_id {
        Some(sid) => store.get_latest_turn(sid)?,
        None => None,
    };

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_millis(2500))
        .build()
        .unwrap_or_default();

    let slots_url = format!("{}/slots", upstream_url.trim_end_matches('/'));
    let mut req_b = client.get(&slots_url);
    if let Ok(key) = std::env::var("CW_LLAMACPP_API_KEY") {
        req_b = req_b.header("Authorization", format!("Bearer {key}"));
    }

    let mut upstream_slots = Vec::new();
    let upstream_reachable = match req_b.send().await {
        Ok(resp) if resp.status().is_success() => {
            if let Ok(slots) = resp.json::<Vec<serde_json::Value>>().await {
                upstream_slots = slots;
                true
            } else {
                false
            }
        }
        _ => false,
    };

    let sessions = store.list_sessions()?;
    let mut slots_info = Vec::new();

    if upstream_reachable && !upstream_slots.is_empty() {
        let total_slots = upstream_slots.len() as u32;
        for slot in upstream_slots {
            let id = slot.get("id").and_then(|v| v.as_u64()).unwrap_or(0) as u32;
            let n_ctx = slot.get("n_ctx").and_then(|v| v.as_u64()).unwrap_or(131072) as u32;
            let is_processing = slot
                .get("is_processing")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let n_prompt_tokens = slot
                .get("n_prompt_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0) as u32;

            let bound_session_id = sessions
                .iter()
                .find(|s| assign_deterministic_slot(&s.id, total_slots) == id)
                .map(|s| s.id.clone());

            let allocated_blocks = n_prompt_tokens.div_ceil(DEFAULT_PAGED_ATTENTION_BLOCK_SIZE);
            let vram_usage_mb = (n_prompt_tokens as f32) * 0.25;

            slots_info.push(EngineSlotInfo {
                id,
                n_ctx,
                is_processing,
                n_prompt_tokens,
                bound_session_id,
                allocated_blocks,
                vram_usage_mb,
            });
        }
    } else {
        let total_slots = 4u32;
        for id in 0..total_slots {
            let bound_session = sessions
                .iter()
                .find(|s| assign_deterministic_slot(&s.id, total_slots) == id);
            let (n_prompt_tokens, bound_id) = if let Some(s) = bound_session {
                let tok = store
                    .get_latest_turn(&s.id)
                    .ok()
                    .flatten()
                    .map(|t| t.token_usage.input_tokens)
                    .unwrap_or(0);
                (tok, Some(s.id.clone()))
            } else {
                (0, None)
            };

            let allocated_blocks = n_prompt_tokens.div_ceil(DEFAULT_PAGED_ATTENTION_BLOCK_SIZE);
            let vram_usage_mb = (n_prompt_tokens as f32) * 0.25;

            slots_info.push(EngineSlotInfo {
                id,
                n_ctx: 131072,
                is_processing: false,
                n_prompt_tokens,
                bound_session_id: bound_id,
                allocated_blocks,
                vram_usage_mb,
            });
        }
    }

    let mut splice_plan: Option<SlotSplicingPlan> = None;
    if let Some(range_str) = splice_range_str {
        let parts: Vec<&str> = range_str.split("..").collect();
        if parts.len() == 2
            && let (Ok(start), Ok(end)) = (parts[0].parse::<u32>(), parts[1].parse::<u32>())
        {
            let target_slot = slot_id_filter.unwrap_or(0);
            let total_tok = target_turn
                .as_ref()
                .map(|t| t.token_usage.input_tokens)
                .unwrap_or(end.max(2048));
            let mut locked_prefix = 256u32;
            if let Some(ref t) = target_turn {
                for n in &t.nodes {
                    match n.kind {
                        cw_core::ir::NodeKind::SystemPrompt { .. }
                        | cw_core::ir::NodeKind::InjectedRule { .. }
                        | cw_core::ir::NodeKind::SkillCatalog { .. }
                        | cw_core::ir::NodeKind::McpToolSchema { .. } => {
                            locked_prefix += n.token_span.count;
                        }
                        _ => {}
                    }
                }
            }
            splice_plan = Some(plan_slot_splicing(
                target_slot,
                total_tok,
                locked_prefix,
                (start, end),
                DEFAULT_PAGED_ATTENTION_BLOCK_SIZE,
            ));
        }
    }

    if output_json {
        let out = serde_json::json!({
            "upstream_url": upstream_url,
            "upstream_reachable": upstream_reachable,
            "block_size": DEFAULT_PAGED_ATTENTION_BLOCK_SIZE,
            "slots": slots_info,
            "splice_plan": splice_plan,
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }

    println!("\n🧩 contextWatcher Engine-Level PagedAttention & Slot Bridge (Horizon 3)");
    println!(
        "════════════════════════════════════════════════════════════════════════════════════"
    );
    println!(
        "Upstream Engine : {} ({})",
        upstream_url,
        if upstream_reachable {
            "🟢 Online"
        } else {
            "⚪ Simulated/Offline"
        }
    );
    println!(
        "PagedAttention  : {} tokens / block (16-token page granularity)",
        DEFAULT_PAGED_ATTENTION_BLOCK_SIZE
    );
    if let Some(ref sid) = target_session_id {
        println!("Target Session  : {}", sid);
    }
    println!("────────────────────────────────────────────────────────────────────────────────");
    println!(
        "  {:<6} {:<10} {:>12} {:>14} {:>12}  {:<26}",
        "SLOT", "STATUS", "TOKENS", "PAGES (16t)", "VRAM (MB)", "BOUND SESSION"
    );
    println!("  {}", "─".repeat(84));

    for s in &slots_info {
        if let Some(target_slot) = slot_id_filter
            && s.id != target_slot
        {
            continue;
        }
        let status_str = if s.is_processing {
            "🟡 Busy"
        } else {
            "🟢 Idle"
        };
        let bound_str = s.bound_session_id.as_deref().unwrap_or("<unbound>");
        println!(
            "  #{:<5} {:<10} {:>12} {:>14} {:>11.1}M  {:<26}",
            s.id, status_str, s.n_prompt_tokens, s.allocated_blocks, s.vram_usage_mb, bound_str
        );
    }

    if let Some(ref plan) = splice_plan {
        println!(
            "\n✂️  PAGEDATTENTION SPLICING SIMULATION (Range: {}..{} tok):",
            plan.target_token_range.0, plan.target_token_range.1
        );
        println!("  Target Slot              : #{}", plan.slot_id);
        println!(
            "  Clean Pages Evicted      : {} blocks ({} tokens)",
            plan.clean_blocks_evicted,
            plan.clean_blocks_evicted * plan.block_size
        );
        println!(
            "  Partial Boundary Slack   : {} tokens (sub-block remainder)",
            plan.partial_boundary_slack_tokens
        );
        println!(
            "  GPU VRAM Saved           : {:.1} MB",
            plan.estimated_vram_saved_mb
        );
        println!(
            "  Prefill Avoided Latency  : ~{} ms (zero-recompute speedup)",
            plan.prefill_recompute_avoided_ms
        );
        match &plan.action {
            SlotSplicingAction::PhysicalPageUnmap {
                blocks_freed,
                tokens_freed,
                vram_freed_mb,
            } => {
                println!(
                    "  Action Result            : 🟢 PhysicalPageUnmap: unmapped {} physical GPU pages ({} tok, {:.1} MB)",
                    blocks_freed, tokens_freed, vram_freed_mb
                );
            }
            SlotSplicingAction::TailTruncation {
                new_n_past,
                tokens_dropped,
            } => {
                println!(
                    "  Action Result            : 🟡 TailTruncation: set n_past to {} (dropped {} tail tokens)",
                    new_n_past, tokens_dropped
                );
            }
            SlotSplicingAction::BlockAlignmentPad {
                original_range,
                aligned_range,
                slack_tokens,
            } => {
                println!(
                    "  Action Result            : 🔵 BlockAlignmentPad: estimated expansion {:?} -> {:?} (+{} tok); physical unmap unsupported",
                    original_range, aligned_range, slack_tokens
                );
            }
            SlotSplicingAction::SubBlockNoop {
                unaligned_tokens,
                reason,
            } => {
                println!(
                    "  Action Result            : ⚪ SubBlockNoop: {} ({} tokens)",
                    reason, unaligned_tokens
                );
            }
        }
    }

    println!(
        "\n💡 Tip: When pruning intermediate nodes, cw splices physical PagedAttention blocks without invalidating the cached prefix!\n"
    );
    Ok(())
}
