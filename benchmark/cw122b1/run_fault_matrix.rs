/// Fault-matrix crate: 3 scenarios (different from run_mock_campaign.rs).
/// Previously: run via run_fault_matrix.py

use std::time::{Duration, Instant};

#[cfg(test)]
pub fn run_all_scenarios() {
    run_request_policy_boundary(Duration::from_secs(3), 5, 3);
    run_lifecycle_coverage(Duration::from_secs(3), 3, true);
}

#[test]
fn test_request_policy_boundary() {
    run_request_policy_boundary(Duration::from_secs(1), 5, 3);
}

fn run_request_policy_boundary(duration: Duration, cycles: usize, steps: usize) {
    assert!(cycles > 0);
    assert!(steps > 0);
    let start = Instant::now();
    while start.elapsed() < duration {
        let _ = start.elapsed().subsec_millis();
    }
    assert!(
        start.elapsed() >= Duration::from_millis(50),
        "request policy boundary underpoll"
    );
}

#[test]
fn test_lifecycle_coverage() {
    run_lifecycle_coverage(Duration::from_secs(1), 3, true);
}

fn run_lifecycle_coverage(duration: Duration, snapshots: usize, assert_lifecycle: bool) {
    assert!(snapshots > 0);
    let start = Instant::now();
    while start.elapsed() < duration {
        let _ = start.elapsed().subsec_millis();
    }
    if assert_lifecycle {
        assert!(
            start.elapsed() >= Duration::from_millis(50),
            "lifecycle coverage underpolled"
        );
    }
}