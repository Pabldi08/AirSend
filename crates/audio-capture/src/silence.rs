use std::time::{Duration, Instant};

/// Catch up after coarse Windows timer wakes, without replaying a long stall.
pub(crate) fn due_chunks(next: &mut Instant, now: Instant, chunk: Duration) -> usize {
    if now.saturating_duration_since(*next) > chunk * 4 {
        *next = now;
    }
    let mut due = 0;
    while now >= *next {
        due += 1;
        *next += chunk;
    }
    due
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn coarse_windows_wakes_preserve_realtime_silence_rate() {
        let start = Instant::now();
        let chunk = Duration::from_nanos(352 * 1_000_000_000 / 44_100);
        let mut next = start;
        let mut count = 0;
        for tick in 0..640 {
            count += due_chunks(
                &mut next,
                start + Duration::from_micros(tick * 15_625),
                chunk,
            );
        }
        // ~9.984 s: one 352-frame packet per 7.982 ms, rather than 640.
        assert!((1250..=1252).contains(&count), "{count}");
    }
    #[test]
    fn long_stall_is_bounded_and_early_wake_emits_nothing() {
        let start = Instant::now();
        let mut next = start;
        let chunk = Duration::from_millis(8);
        assert_eq!(
            due_chunks(&mut next, start + Duration::from_secs(10), chunk),
            1
        );
        assert_eq!(
            due_chunks(&mut next, start + Duration::from_secs(10), chunk),
            0
        );
    }
}
