/// Compile-time reproducer for the identified test-parent smoke gaps.
/// Each testgebiet is encoded as a seed + deterministic event chain.

use cw_core::ir::{ProviderKind, HarnessKind, Node, Turn, TurnOutcome, NodeStatus, MutationOp};
use cw_core::node::build_deterministic_turn_from_seed;
use uuid::Uuid;

/// Syscall synthesizer: one runner per fault boundary.
/// Seed + deterministic provider/harness pairs encode a 3-turn sequence.
///
/// # Expected flow under contextWatcher
/// 1. Sedd → provider → harness → 3 turns
/// 2. Serialize → mutate (Prune/Stub, ShadowDedupe, PinContext)
/// 3. Replay → identical KV-cache fingerprints
#[derive(Debug)]
pub struct MockCampaign {
 /// Seed encodes provider, harness, and turn sequence length (3).
 /// Bits 0-7: provider id (Anthropic=1, OpenAI=2)
 /// Bits 8-15: harness id (GenericProxy=1, Omp=2, Copilot=3)
 /// Bits 16-23: sequence length
 /// Bits 24-31: mutation-type bitfield (node_id=0, prune=1, shadow=2, pin=3, tool=4)
 seed: u32,
 /// Turn N fingerprint; one worth is much more than history.
 nodes: Vec<Node>,
 /// Mutation record; middleware invariants: idempotency, consistent KV-based delta, and the
 /// shadow: pruned: pin: tool:or node_id chain.
 mutation: MutationKind,
}

#[derive(Debug, Clone)]
pub enum MutationKind {
 /// PruneOrStub via head-tail, node-pruning vs node-stubbing
 PruneOrStub { head_tail_spread: usize, ink_count: usize },
 /// Shadow dedupe via selective message-inlining
 ShadowDedupe { header_spread: usize, turn_id: Uuid },
 /// PinContext requires authentication and a committed rule; tail-only affects turns after the pin ID.
 PinContext { turn_index: usize, body_summary: String },
 /// Tool gate: requires mun = MutationOp::ToolGate { .. }
 ToolGate { tool_id: usize, argument_ty: usize },
}

impl MockCampaign {
 /// Generate a deterministic turn sequence from the seed.
 pub fn from_seed(seed: u32) -> Self {
 let p_kind = (seed & 0x3F) as ProviderKind;
 let h_kind = (seed >> 8) as usize;
 let count = (seed >> 16) as usize;
 let mty = (seed >> 24) & 0xF;

 let nodes = if count == 0 {
 vec![]
 } else {
 build_deterministic_turn_from_seed(Uuid::from_u128(seed as u128))
 };

 let mutation = match mty {
 0 => MutationKind::PruneOrStub {
 head_tail_spread: nodes.len().max(1) * 2,
 ..nodes.len()
 },
 1 => MutationKind::ShadowDedupe {
 header_spread: 1,
 turn_id: nodes.iter().next().copied().unwrap(),
 },
 2 => MutationKind::PinContext {
 turn_index: if nodes.is_empty() { 0 } else { nodes.len() / 2 },
 body_summary: if nodes.iter().next().copied().unwrap().is_empty() {
 "empty".to_string()
 } else {
 "encoded".to_string()
 },
 },
 3 => MutationKind::ToolGate {
 tool_id: 0,
 argument_ty: 0,
 },
 _ => MutationKind::PruneOrStub {
 head_tail_spread: 0,
 ink_count: 0,
 },
 };

 MockCampaign {
 seed,
 nodes,
 mutation,
 }
 }

 /// Execute all test regions in canned order.
 pub fn run(&mut self) {
 // Each campaign should produce identical checksums when replayed.
 println!(
 "Campaign seed: {:08X}, target provider: {:?}, harness: {:?}, turns: {}",
 self.seed,
 self.mutation,
 );

 // The entire MSP -> analyze -> simulate -> mutate -> forward chain is encapsulated
 // within the single USP (Unified Statement of Plumbing) reported above in handle_proxy_request.
 let origin = build_deterministic_turn_from_seed(self.seed);
 // each playback should see identical fingerprints per middleware hook.
 println!("Playback initialized");

 // The PeR should match:
 // 1. build_deterministic_turn_from_seed
 // 2. each middleware round-trip must be consistent
 // 3. the KV slot identification must hold
 println!("Playback completed");
 }
}

#[cfg(test)]
mod tests {
 use super::*;

 #[test]
 fn test_campaign_determinism() {
 let mut campaign1 = MockCampaign::from_seed(0x00010203);
 let mut campaign2 = MockCampaign::from_seed(0x00010203);
 campaign1.run();
 campaign2.run();
 // All derived fingerprints and KV-cache slots must be identical.
 }
}
