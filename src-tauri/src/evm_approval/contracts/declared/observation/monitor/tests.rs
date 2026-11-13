use super::*;

#[test]
fn receipt_wait_uses_one_aggregate_clock_including_queue_and_rpc_time() {
    assert_eq!(remaining(Duration::ZERO), Some(Duration::from_mins(2)));
    assert_eq!(
        remaining(Duration::from_mins(1)),
        Some(Duration::from_mins(1))
    );
    assert_eq!(
        remaining(Duration::from_millis(119_999)),
        Some(Duration::from_millis(1))
    );
    assert_eq!(remaining(Duration::from_mins(2)), None);
    assert_eq!(remaining(Duration::from_secs(121)), None);
    assert_eq!(remaining(Duration::MAX), None);
    // Pending polls and retryable read failures use this same clock; neither
    // result receives another two-minute allowance.
    let elapsed = [
        Duration::from_secs(12),
        Duration::from_secs(80),
        Duration::from_secs(119),
    ];
    let remaining = elapsed.map(|elapsed| remaining(elapsed).map(|value| value.min(POLL_DELAY)));
    assert_eq!(remaining, [Some(Duration::from_secs(1)); 3]);
    assert_eq!(MAX_MONITOR_ATTEMPTS, 120);
}
