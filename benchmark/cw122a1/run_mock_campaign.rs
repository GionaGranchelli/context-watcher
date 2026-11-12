/// Deterministic mock-campaign prototype used by CW-122A1 notes.
///
/// This file is intentionally self-contained so it stays syntactically valid
/// even when it is not wired into a Cargo target.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MutationKind {
    PruneOrStub,
    ShadowDedupe,
    PinContext,
    ToolGate,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MockCampaign {
    seed: u32,
    turn_count: usize,
    mutation: MutationKind,
}

impl MockCampaign {
    pub fn from_seed(seed: u32) -> Self {
        let turn_count = ((seed >> 16) & 0xFF).clamp(1, 3) as usize;
        let mutation = match (seed >> 24) & 0x03 {
            0 => MutationKind::PruneOrStub,
            1 => MutationKind::ShadowDedupe,
            2 => MutationKind::PinContext,
            _ => MutationKind::ToolGate,
        };
        Self {
            seed,
            turn_count,
            mutation,
        }
    }

    pub fn run(&self) -> Vec<u64> {
        (0..self.turn_count)
            .map(|turn| deterministic_fingerprint(self.seed, turn as u32))
            .collect()
    }
}

fn deterministic_fingerprint(seed: u32, turn: u32) -> u64 {
    let mixed = seed as u64 ^ ((turn as u64) << 32) ^ 0x9E37_79B9_7F4A_7C15;
    mixed.rotate_left(13) ^ mixed.wrapping_mul(0xD6E8_FEB8_6659_FD93)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_campaign_determinism() {
        let campaign_a = MockCampaign::from_seed(0x0001_0203);
        let campaign_b = MockCampaign::from_seed(0x0001_0203);
        assert_eq!(campaign_a.run(), campaign_b.run());
    }
}
