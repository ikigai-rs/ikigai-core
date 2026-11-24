use ikigai_core::{MonotonicClock, MonotonicError};
use std::cmp::Ordering;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn deadlines_are_source_bound_and_checked() {
    let clock = MonotonicClock::from_host(|| Duration::from_secs(1));
    let other = MonotonicClock::from_host(|| Duration::from_secs(1));
    let now = clock.now();
    assert_eq!(now.checked_cmp(&clock.clone().now()), Ok(Ordering::Equal));
    assert_eq!(
        now.checked_cmp(&other.now()),
        Err(MonotonicError::DifferentSources)
    );
    assert_eq!(
        now.checked_duration_since(&other.now()),
        Err(MonotonicError::DifferentSources)
    );
    assert_eq!(
        now.saturating_duration_since(&other.now()),
        Err(MonotonicError::DifferentSources)
    );
    assert_eq!(
        now.checked_add(Duration::MAX).unwrap_err(),
        MonotonicError::Overflow
    );

    let last = now
        .checked_add(Duration::MAX - Duration::from_secs(1))
        .unwrap();
    assert_eq!(
        last.checked_add(Duration::from_nanos(1)).unwrap_err(),
        MonotonicError::Overflow
    );
    assert_eq!(
        now.checked_duration_since(&last),
        Err(MonotonicError::Reversed)
    );
    assert_eq!(now.saturating_duration_since(&last), Ok(Duration::ZERO));
}

#[test]
fn wall_clock_corrections_do_not_change_window_or_cooldown() {
    let wall = AtomicU64::new(10_000);
    let ticks = Arc::new(AtomicU64::new(0));
    let clock = MonotonicClock::from_host({
        let ticks = ticks.clone();
        move || Duration::from_millis(ticks.load(Relaxed))
    });
    let span = Duration::from_millis(10);
    let wall_start = wall.load(Relaxed);
    let hit = clock.now();
    let cooldown_end = hit.checked_add(span).unwrap();

    // A forward step would expire an epoch-based window immediately.
    wall.store(20_000, Relaxed);
    ticks.store(9, Relaxed);
    assert!(wall.load(Relaxed).saturating_sub(wall_start) >= 10);
    let now = clock.now();
    assert!(now.checked_duration_since(&hit).unwrap() < span);
    assert_eq!(
        cooldown_end.saturating_duration_since(&now),
        Ok(Duration::from_millis(1))
    );

    // A backward step would keep that window open at the true boundary.
    wall.store(5_000, Relaxed);
    ticks.store(10, Relaxed);
    assert!(wall.load(Relaxed).saturating_sub(wall_start) < 10);
    let now = clock.now();
    assert_eq!(now.checked_duration_since(&hit), Ok(span));
    assert_eq!(cooldown_end.checked_cmp(&now), Ok(Ordering::Equal));
    assert_eq!(
        cooldown_end.saturating_duration_since(&now),
        Ok(Duration::ZERO)
    );
}

#[cfg(not(target_family = "wasm"))]
#[test]
fn native_adapter_produces_ordered_readings_without_a_wall_epoch() {
    let clock = MonotonicClock::native();
    let start = clock.now();
    assert!(clock.now().checked_duration_since(&start).is_ok());
    assert_eq!(
        MonotonicClock::native().now().checked_cmp(&start),
        Err(MonotonicError::DifferentSources)
    );
}

#[test]
fn arithmetic_errors_explain_what_failed_in_plain_text() {
    assert_eq!(
        MonotonicError::DifferentSources.to_string(),
        "monotonic instants come from different clock sources"
    );
    assert_eq!(
        MonotonicError::Overflow.to_string(),
        "monotonic deadline is outside the representable range"
    );
    assert_eq!(
        MonotonicError::Reversed.to_string(),
        "cannot measure elapsed time since a later monotonic instant"
    );
}
