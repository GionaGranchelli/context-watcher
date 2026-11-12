/// Deterministic fault-matrix prototype used by CW-122A1 notes.
///
/// This file is intentionally self-contained so it can be kept in-repo as a
/// reproducible sketch without depending on crate-internal benchmark plumbing.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    OpenAI,
    Anthropic,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessKind {
    GenericProxy,
    Omp,
    Copilot,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MutationKind {
    PruneOrStub { head_tail_gap: usize },
    ShadowDedupe { message_span: usize },
    PinContext { kv_slot: usize },
    ToolGate { tool_id: usize, arg_ty: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaultMatrix {
    seed: u32,
    nodes: (u32, u32, u32),
    provider: ProviderKind,
    harness: HarnessKind,
    mutation: MutationKind,
}

impl FaultMatrix {
    pub fn from_seed(seed: u32) -> Self {
        let provider = if (seed >> 19) & 0x01 == 0 {
            ProviderKind::OpenAI
        } else {
            ProviderKind::Anthropic
        };
        let harness = match (seed >> 16) & 0x03 {
            0 => HarnessKind::GenericProxy,
            1 => HarnessKind::Omp,
            _ => HarnessKind::Copilot,
        };
        let n_id = seed & 0x03FF;
        let nodes = (n_id, (n_id >> 3) & 0x1F, (n_id >> 8) & 0x03);
        let mutation = match (seed >> 7) & 0x03 {
            0 => MutationKind::PruneOrStub {
                head_tail_gap: (nodes.0 + nodes.1) as usize,
            },
            1 => MutationKind::ShadowDedupe {
                message_span: (nodes.1.max(nodes.2) + 1) as usize,
            },
            2 => MutationKind::PinContext {
                kv_slot: nodes.0 as usize,
            },
            _ => MutationKind::ToolGate {
                tool_id: (nodes.0 % 10) as usize,
                arg_ty: (nodes.1 % 10) as usize,
            },
        };

        Self {
            seed,
            nodes,
            provider,
            harness,
            mutation,
        }
    }

    pub fn run(&self) -> Vec<u64> {
        (0..3)
            .map(|step| {
                let base = self.seed as u64
                    ^ ((self.nodes.0 as u64) << 16)
                    ^ ((self.nodes.1 as u64) << 8)
                    ^ self.nodes.2 as u64;
                let with_step = base ^ ((step as u64) << 32);
                with_step.rotate_left(11) ^ with_step.wrapping_mul(0xA24B_AED4_963E_E407)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_replay_determinism() {
        let matrix = FaultMatrix::from_seed(0x1234_5678);
        assert_eq!(matrix.run(), matrix.run());
    }
}
