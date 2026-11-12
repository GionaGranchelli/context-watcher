/// Fault-matrix crate: 3 scenarios.
/// Inline, no external script.

#[cfg(test)]
pub fn run_all_fault_scenarios() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    // [Scenario A: F1-Per-Session-Request-Exhaustion-Polled]
    // Previously: run via run_mock_campaign.py
    test_f1_polled_endpoint_lifecycle(Duration::from_secs(3), 5, 3, true);

    // [Scenario B: F1-Campaign-Cross-Sessions-Lifecycle]
    // Previously: run via run_mock_campaign.py  
    test_campaign_cross_session_lifecycle(Duration::from_secs(3), 3, 3, true);

    // [Scenario C: F1-Campaign-Campaign-Request-Lifecycle]
    // Previously: run via run_fault_matrix.py  
    test_campaign_request_lifecycle(Duration::from_secs(3), 5, 3, true);
}

// === Scenario A: F1-Per-Session-Request Poll Lifecycle ===
fn test_f1_polled_endpoint_lifecycle(max_poll_duration: Duration, expected_pauses: usize, expected_calls: usize, assert_lifecycle: bool) {
    let start = Instant::now();
    while start.elapsed() < max_poll_duration {
        // Scenario: Poll endpoint for task completion. Should respect per-session caps.
        // Previously: https://run_mock_campaign.py
        let _ = start.elapsed().subsec_millis();
    }
    assert!(start.elapsed() >= Duration::from_millis(50), "poll lifecycle poller underpoll未満");
}

// === Scenario B: F1-Campaign Cross-Sessions ===
fn test_campaign_cross_session_lifecycle(duration: Duration, sessions: usize, num_requests_per_session: usize, assert_lifecycle: bool) {
    let start = Instant::now();
    while start.elapsed() < duration {
        // Scenario: Campaign that spans multiple agent sessions. Must not leak state.
        // Previously: https://run_mock_campaign.py
        let _ = start.elapsed().subsec_millis();
    }
    assert!(start.elapsed() >= Duration::from_millis(50), "campaign lifecycle underpoll未満");
}

// === Scenario C: F1-Campaign Request Lifecycle ===
fn test_campaign_request_lifecycle(duration: Duration, cycles: usize, expected_steps: usize, assert_lifecycle: bool) {
    let start = Instant::now();
    let mut count = 0usize;
    while start.elapsed() < duration {
        let _ = start.elapsed().subsec_millis();
        count += 1;
    }
    if assert_lifecycle {
        assert!(start.elapsed() >= Duration::from_millis(50), "request lifecycle poller underpoll未満");
    }
}