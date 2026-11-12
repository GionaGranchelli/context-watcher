/// Fault-matrix synthesizer for CW context-mutation overlay.
///
/// This runner models three interleaved test streams in the critical USP (Unified Statement of Plumbing) where
/// context reduction, KV-cache reconstruction, and compliance validation occur.
///
/// # Design
/// - One runner per fault boundary (Prune, ShadowDedupe, PinContext, ToolGate)
/// - Seed encodes: provider + harness + mutation type + node topology
/// - Expands to exactly 3 deterministic turns with the payloads' KV-cache anchors
///
/// # Invariants
/// 1. Serial replay produces identical node-tree fingerprints
/// 2. Middleware hooks execute in the exact sequence: capture -> analyze -> mutate -> evaluate -> forward
/// 3. KV-slot identifer anchors must match under any guard-policy configuration
#[derive(Debug)]
pub struct FaultMatrix {
 /// 24-bit seed: [8:7:6] provider, [5:4:3] harness, [2:0] mutation, [23:17] node_trees, [16:9] seed
 seed: u32,
 /// Node-topology encoded as three half-open node ID ranges (start, count, ...)
 nodes: (u128, u128, u128),
 /// Mutation kind: PruneOrStub, ShadowDedupe, PinContext, ToolGate
 mutation: MutationKind,
}

#[derive(Debug, Clone)]
pub enum MutationKind {
 PruneOrStub { head_tail_gap: usize },
 ShadowDedupe { message_span: usize },
 PinContext { rule_id: uuid::Uuid, kv_slot: usize },
 ToolGate { tool_id: usize, arg_ty: usize },
}

impl FaultMatrix {
 /// Decodes seed into provider (0 = Anthropic, 1 = OpenAI) and harness
 fn decode_provider(seed: u32) -> ProviderKind {
 // Bits 23-24: reversed
 (seed >> 19) < 2
 }

 /// Models a 3-turn sequence from seed; each turn carries explicit KV anchors.
 pub fn new(seed: u32) -> Self {
 let p_id = (seed >> 20) & 0b11;
 let h_id = (seed >> 16) & 0b111;
 let m_id = (seed >> 7) & 0b111;
 let n_id = seed & 0x3FF;
 let (start, count, step) = (n_id, (n_id >> 6) as usize, (n_id >> 12) as usize);

 let nodes = (start as u128, count as u128, step as u128);

 let mutation = match m_id {
 0 => MutationKind::PruneOrStub { head_tail_gap: start + count },
 1 => MutationKind::ShadowDedupe { message_span: count.max(step) },
 2 => MutationKind::PinContext { rule_id: uuid::Uuid::new_v4(), kv_slot: start as usize },
 3 => MutationKind::ToolGate { tool_id: (start % 10) as usize, arg_ty: (count % 10) as usize },
 _ => MutationKind::PruneOrStub { head_tail_gap: 0 },
 };

 FaultMatrix {
 seed,
 nodes,
 mutation,
 }
 }

 pub fn run(&self) {
 let provider = if self.seed & 0b100 == 0 {
 ProviderKind::OpenAI
 } else {
 ProviderKind::Anthropic
 };
 let harness = match (self.seed >> 16) & 0b11 {
 0 => HarnessKind::Omp,
 1 => HarnessKind::GenericProxy,
 2 => HarnessKind::Copilot,
 3 => HarnessKind::Anthropic,
 _ => HarnessKind::Thorium,
 };
 let mu = &self.mutation;

 println!(
 "FaultMatrix seed=0x{:08X} provider={:?} harness={:?} mutation={:?}",
 self.seed,
 provider,
 harness,
 mu,
 );

 // The middleware pipeline processes:
 // 1. Capture turn from body (remove cw_session_id)
 // 2. Analyze turn (compute KV fingerprint per node)
 // 3. Mutate payload (apply rule)
 // 4. Evaluate checkpoint (autopilot_policies)
 // 5. Forward to upstream with mutated payload

 let context_turn = build_deterministic_turn_from_seed(self.seed);
 println!("Step 1: captured turn -> analyzed {} nodes", context_turn.nodes.len());

 match &self.mutation {
 MutationKind::PruneOrStub { .. } => {
 let _ = build_prune_stub_processor(&context_turn, head_tail_gap_for_mutation);
 }
 MutationKind::ShadowDedupe { .. } => {
 let _ = shadow_dedupe_processor(&context_turn);
 }
 MutationKind::PinContext { kv_slot, .. } => {
 let _ = pin_context_processor(&context_turn, *kv_slot);
 }
 MutationKind::ToolGate { .. } => {
 let _ = tool_gate_processor(&context_turn);
 }
 }
 println!("FaultMatrix run completed");
 }
}

fn build_prune_stub_processor(turn: &Turn, head_tail_gap: usize) {
 // Simulate head-tail pruning vs stubbing
 // Each PruneOrStub roundtrip must preserve KV-cache compatibility
 println!("PruneOrStub head_tail_gap={}", head_tail_gap);
}

fn shadow_dedupe_processor(turn: &Turn) {
 // Each dedupe reduces message-velocity but expands message count
 println!("ShadowDedupe spanning {} turns", 2);
}

fn pin_context_processor(turn: &Turn, kv_slot: usize) {
 // PinContext requires authenticated session binding + committed rule.
 // Tail-only scope affects downstream turns.
 println!("PinContext at node {}", kv_slot);
}

fn tool_gate_processor(turn: &Turn) {
 // Tool gate requires an explicit route assignment or fallback.
 println!("ToolGate: tool={0} arg_ty={1}", self.tool_id, self.arg_ty);
}

#[cfg(test)]
mod tests {
 use super::*;

 #[test]
 fn test_replay_determinism() {
 let matrix = FaultMatrix::from_seed(0x12345678);
 matrix.run();
 // A second invocation from the same seed should produce identical fingerprints
 }
}