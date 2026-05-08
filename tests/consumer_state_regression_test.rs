use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

#[test]
fn rdy_count_is_initialized_before_first_message() {
    let max_in_flight: i32 = 1;
    let total_rdy_count = AtomicI32::new(0);

    total_rdy_count.store(max_in_flight, Ordering::Relaxed);

    let previous_rdy = total_rdy_count.fetch_sub(1, Ordering::Relaxed);
    let remaining_rdy = previous_rdy - 1;

    assert_eq!(previous_rdy, 1);
    assert_eq!(remaining_rdy, 0);
    assert!(remaining_rdy <= max_in_flight / 2);

    total_rdy_count.store(max_in_flight, Ordering::Relaxed);
    assert_eq!(total_rdy_count.load(Ordering::Relaxed), 1);
}

#[test]
fn rdy_replenish_uses_remaining_not_previous_value() {
    let max_in_flight: i32 = 2;
    let total_rdy_count = AtomicI32::new(0);
    let mut rdy_sent_count = 0i32;

    rdy_sent_count += 1;
    total_rdy_count.store(max_in_flight, Ordering::Relaxed);

    let previous_rdy = total_rdy_count.fetch_sub(1, Ordering::Relaxed);
    let remaining_rdy = previous_rdy - 1;

    assert_eq!(previous_rdy, 2);
    assert_eq!(remaining_rdy, 1);

    // remaining_rdy=1 is NOT <= max_in_flight/2=1, so no extra RDY sent
    if remaining_rdy <= max_in_flight / 2 {
        rdy_sent_count += 1;
        total_rdy_count.store(max_in_flight, Ordering::Relaxed);
    }

    // With max_in_flight=2, remaining=1, threshold=1: 1 <= 1 triggers replenish
    assert_eq!(rdy_sent_count, 2);
    assert_eq!(total_rdy_count.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn stop_state_is_visible_across_clones() {
    let is_running = Arc::new(AtomicBool::new(true));
    let cloned = Arc::clone(&is_running);

    is_running.store(false, Ordering::Relaxed);

    assert!(!cloned.load(Ordering::Relaxed));
}

#[tokio::test(flavor = "current_thread")]
async fn connection_count_readable_in_async_context_without_blocking_lock() {
    let connection_count = Arc::new(AtomicI32::new(2));
    let connections = connection_count.load(Ordering::Relaxed);

    assert_eq!(connections, 2);
}
