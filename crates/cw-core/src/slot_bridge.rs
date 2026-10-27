//! Engine-Level PagedAttention & Slot Splicing Bridge (Horizon 3 / CW-070).
//!
//! Provides direct hardware/engine level slot management and PagedAttention KV-cache
//! page block mapping for local inference engines (llama.cpp, vLLM, SGLang, Strata).
//! Enables physical page unmapping, block-boundary alignment, and zero-recompute prefix mutations.

use serde::{Deserialize, Serialize};

/// Standard token capacity of a PagedAttention memory block (page) in vLLM & llama.cpp.
pub const DEFAULT_PAGED_ATTENTION_BLOCK_SIZE: u32 = 16;

/// High-throughput token capacity of a PagedAttention block in SGLang.
pub const SGLANG_PAGED_ATTENTION_BLOCK_SIZE: u32 = 64;

/// Status and metadata for a physical engine KV-cache slot on upstream local engines.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EngineSlotInfo {
    pub id: u32,
    pub n_ctx: u32,
    pub is_processing: bool,
    pub n_prompt_tokens: u32,
    pub bound_session_id: Option<String>,
    pub allocated_blocks: u32,
    pub vram_usage_mb: f32,
}

/// Represents a contiguous range of PagedAttention memory blocks (pages).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PagedAttentionBlockRange {
    pub block_start: u32,
    pub block_count: u32,
    pub token_start: u32,
    pub token_end: u32,
    pub is_boundary_aligned: bool,
}

/// Comprehensive page-block utilization map for an agent context turn or session.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PagedAttentionBlockMap {
    pub block_size: u32,
    pub total_tokens: u32,
    pub total_blocks: u32,
    pub prefix_locked_blocks: u32,
    pub mutable_blocks: u32,
    pub wasted_slack_tokens: u32,
    pub memory_efficiency_pct: f32,
}

/// Type of engine slot mutation or splicing action.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SlotSplicingAction {
    /// Zero-recompute physical page unmapping: cleanly frees entire physical pages from GPU VRAM.
    PhysicalPageUnmap {
        blocks_freed: u32,
        tokens_freed: u32,
        vram_freed_mb: f32,
    },
    /// Truncate tail tokens from slot cache (e.g. during rollback or compaction).
    TailTruncation {
        new_n_past: u32,
        tokens_dropped: u32,
    },
    /// Block alignment padding recommendation: adjusts cut boundary to match page bounds.
    BlockAlignmentPad {
        original_range: (u32, u32),
        aligned_range: (u32, u32),
        slack_tokens: u32,
    },
    /// Range falls entirely within a single block or locked prefix; cannot unmap full page.
    SubBlockNoop {
        unaligned_tokens: u32,
        reason: String,
    },
}

/// Detailed plan for splicing a pruned range directly in engine KV-cache memory.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlotSplicingPlan {
    pub slot_id: u32,
    pub block_size: u32,
    pub target_token_range: (u32, u32),
    pub action: SlotSplicingAction,
    pub clean_blocks_evicted: u32,
    pub partial_boundary_slack_tokens: u32,
    pub estimated_vram_saved_mb: f32,
    pub prefill_recompute_avoided_ms: u64,
}

/// Assigns a deterministic engine slot ID (0..total_slots-1) for a session ID.
/// Ensures consistent slot affinity across multi-turn interactions.
pub fn assign_deterministic_slot(session_id: &str, total_slots: u32) -> u32 {
    if total_slots <= 1 {
        return 0;
    }
    let hash = blake3::hash(session_id.as_bytes());
    let b = hash.as_bytes()[0] as u32;
    b % total_slots
}

/// Computes a PagedAttention memory block map for a given token count and locked prefix.
pub fn calculate_block_map(
    total_tokens: u32,
    prefix_locked_tokens: u32,
    block_size: u32,
) -> PagedAttentionBlockMap {
    let bs = if block_size == 0 { DEFAULT_PAGED_ATTENTION_BLOCK_SIZE } else { block_size };
    let total_blocks = total_tokens.div_ceil(bs);
    let prefix_locked_blocks = prefix_locked_tokens.div_ceil(bs);
    let mutable_blocks = total_blocks.saturating_sub(prefix_locked_blocks);
    let wasted_slack_tokens = if total_tokens.is_multiple_of(bs) {
        0
    } else {
        bs - (total_tokens % bs)
    };

    let allocated_tokens = total_blocks * bs;
    let efficiency = if allocated_tokens > 0 {
        (total_tokens as f32 / allocated_tokens as f32) * 100.0
    } else {
        100.0
    };

    PagedAttentionBlockMap {
        block_size: bs,
        total_tokens,
        total_blocks,
        prefix_locked_blocks,
        mutable_blocks,
        wasted_slack_tokens,
        memory_efficiency_pct: efficiency,
    }
}

/// Aligns an arbitrary token cut range `(start, end)` to enclosing PagedAttention block boundaries.
/// Expands outward to the nearest block boundaries: `start` rounded down, `end` rounded up.
pub fn align_to_block_boundary(range: (u32, u32), block_size: u32) -> (u32, u32) {
    let bs = if block_size == 0 { DEFAULT_PAGED_ATTENTION_BLOCK_SIZE } else { block_size };
    let aligned_start = (range.0 / bs) * bs;
    let aligned_end = range.1.div_ceil(bs) * bs;
    (aligned_start, aligned_end)
}

/// Formulates a zero-recompute PagedAttention slot splicing plan for a target prune range.
pub fn plan_slot_splicing(
    slot_id: u32,
    total_tokens: u32,
    prefix_locked_tokens: u32,
    prune_range: (u32, u32),
    block_size: u32,
) -> SlotSplicingPlan {
    let bs = if block_size == 0 { DEFAULT_PAGED_ATTENTION_BLOCK_SIZE } else { block_size };
    let (mut start, end) = prune_range;

    // Guard against invalid or inverted ranges
    if start >= end || end > total_tokens {
        return SlotSplicingPlan {
            slot_id,
            block_size: bs,
            target_token_range: prune_range,
            action: SlotSplicingAction::SubBlockNoop {
                unaligned_tokens: 0,
                reason: "Empty or invalid token range".to_string(),
            },
            clean_blocks_evicted: 0,
            partial_boundary_slack_tokens: 0,
            estimated_vram_saved_mb: 0.0,
            prefill_recompute_avoided_ms: 0,
        };
    }

    // Clamp start if it intersects with locked prefix
    if start < prefix_locked_tokens {
        if end <= prefix_locked_tokens {
            return SlotSplicingPlan {
                slot_id,
                block_size: bs,
                target_token_range: prune_range,
                action: SlotSplicingAction::SubBlockNoop {
                    unaligned_tokens: end - start,
                    reason: "Range lies entirely within pinned system prefix".to_string(),
                },
                clean_blocks_evicted: 0,
                partial_boundary_slack_tokens: end - start,
                estimated_vram_saved_mb: 0.0,
                prefill_recompute_avoided_ms: 0,
            };
        }
        start = prefix_locked_tokens;
    }

    // Check if range extends to the tail of the context (Tail Truncation)
    if end >= total_tokens {
        let tokens_dropped = total_tokens.saturating_sub(start);
        let blocks_freed = tokens_dropped / bs;
        let vram_saved = (tokens_dropped as f32) * 0.25; // ~0.25 MB per token in 32B FP16 KV
        let avoided_ms = ((tokens_dropped as u64) * 1000) / 650; // ~650 tps prefill speed

        return SlotSplicingPlan {
            slot_id,
            block_size: bs,
            target_token_range: (start, total_tokens),
            action: SlotSplicingAction::TailTruncation {
                new_n_past: start,
                tokens_dropped,
            },
            clean_blocks_evicted: blocks_freed,
            partial_boundary_slack_tokens: tokens_dropped % bs,
            estimated_vram_saved_mb: vram_saved,
            prefill_recompute_avoided_ms: avoided_ms,
        };
    }

    // Calculate strictly enclosed blocks (wholly contained inside the range)
    let first_enclosed_block = start.div_ceil(bs);
    let last_enclosed_block = end / bs;

    if last_enclosed_block > first_enclosed_block {
        let clean_blocks = last_enclosed_block - first_enclosed_block;
        let tokens_freed = clean_blocks * bs;
        let vram_saved = (tokens_freed as f32) * 0.25;
        let avoided_ms = ((tokens_freed as u64) * 1000) / 650;

        let left_slack = (first_enclosed_block * bs).saturating_sub(start);
        let right_slack = end.saturating_sub(last_enclosed_block * bs);
        let total_slack = left_slack + right_slack;

        SlotSplicingPlan {
            slot_id,
            block_size: bs,
            target_token_range: (start, end),
            action: SlotSplicingAction::PhysicalPageUnmap {
                blocks_freed: clean_blocks,
                tokens_freed,
                vram_freed_mb: vram_saved,
            },
            clean_blocks_evicted: clean_blocks,
            partial_boundary_slack_tokens: total_slack,
            estimated_vram_saved_mb: vram_saved,
            prefill_recompute_avoided_ms: avoided_ms,
        }
    } else {
        // Range does not span a complete PagedAttention page block
        let slack = end - start;
        let aligned = align_to_block_boundary((start, end), bs);
        SlotSplicingPlan {
            slot_id,
            block_size: bs,
            target_token_range: (start, end),
            action: SlotSplicingAction::BlockAlignmentPad {
                original_range: (start, end),
                aligned_range: aligned,
                slack_tokens: (aligned.1 - aligned.0).saturating_sub(slack),
            },
            clean_blocks_evicted: 0,
            partial_boundary_slack_tokens: slack,
            estimated_vram_saved_mb: 0.0,
            prefill_recompute_avoided_ms: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deterministic_slot_assignment() {
        let s1 = "sess_backend_lead";
        let s2 = "sess_backend_lead";
        let s3 = "sess_frontend_tester";

        let slot1 = assign_deterministic_slot(s1, 4);
        let slot2 = assign_deterministic_slot(s2, 4);
        let slot3 = assign_deterministic_slot(s3, 4);

        assert_eq!(slot1, slot2, "Same session ID must map to the same slot");
        assert!(slot1 < 4);
        assert!(slot3 < 4);
    }

    #[test]
    fn test_block_map_calculation() {
        let map = calculate_block_map(100, 32, 16);
        assert_eq!(map.block_size, 16);
        assert_eq!(map.total_blocks, 7); // ceil(100/16) = 7 (112 tok capacity)
        assert_eq!(map.prefix_locked_blocks, 2); // 32 / 16 = 2
        assert_eq!(map.mutable_blocks, 5);
        assert_eq!(map.wasted_slack_tokens, 12); // 112 - 100 = 12
        assert!(map.memory_efficiency_pct > 89.0);
    }

    #[test]
    fn test_align_to_block_boundary() {
        let aligned = align_to_block_boundary((35, 78), 16);
        assert_eq!(aligned, (32, 80));
    }

    #[test]
    fn test_slot_splicing_physical_page_unmap() {
        // 2000 total tokens, 256 locked prefix, pruning [320, 800]
        let plan = plan_slot_splicing(0, 2000, 256, (320, 800), 16);
        assert_eq!(plan.slot_id, 0);
        assert_eq!(plan.clean_blocks_evicted, 30); // (800 - 320) / 16 = 30
        assert_eq!(plan.partial_boundary_slack_tokens, 0);
        assert_eq!(plan.estimated_vram_saved_mb, 120.0); // 480 * 0.25 MB

        if let SlotSplicingAction::PhysicalPageUnmap { blocks_freed, tokens_freed, .. } = plan.action {
            assert_eq!(blocks_freed, 30);
            assert_eq!(tokens_freed, 480);
        } else {
            panic!("Expected PhysicalPageUnmap action");
        }
    }

    #[test]
    fn test_slot_splicing_tail_truncation() {
        let plan = plan_slot_splicing(1, 2000, 256, (1500, 2000), 16);
        if let SlotSplicingAction::TailTruncation { new_n_past, tokens_dropped } = plan.action {
            assert_eq!(new_n_past, 1500);
            assert_eq!(tokens_dropped, 500);
        } else {
            panic!("Expected TailTruncation action");
        }
    }

    #[test]
    fn test_slot_splicing_locked_prefix_protection() {
        let plan = plan_slot_splicing(0, 2000, 256, (10, 200), 16);
        if let SlotSplicingAction::SubBlockNoop { reason, .. } = plan.action {
            assert!(reason.contains("pinned system prefix"));
        } else {
            panic!("Expected SubBlockNoop due to locked prefix");
        }
    }
}
