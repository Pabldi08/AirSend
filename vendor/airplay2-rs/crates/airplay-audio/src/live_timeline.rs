//! Skip missing capture time instead of permanently sending late RTP packets.
use tokio::time::Instant;
#[derive(Default)]
pub(crate) struct LiveTimeline {
    anchor: Option<(Instant, u64)>,
    offset: u64,
}
impl LiveTimeline {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn timestamp(&mut self, frame: u64, now: Instant, rate: u32) -> (u32, u64) {
        let base = frame.saturating_add(self.offset);
        let (at, ts) = *self.anchor.get_or_insert((now, base));
        let expected = ts.saturating_add(
            (now.saturating_duration_since(at).as_nanos() * u128::from(rate) / 1_000_000_000)
                as u64,
        );
        let lag = expected.saturating_sub(base);
        let skipped = if lag > u64::from(rate) / 50 { lag } else { 0 };
        self.offset = self.offset.saturating_add(skipped);
        (frame.saturating_add(self.offset) as u32, skipped)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    #[test]
    fn continuous_audio_does_not_skip() {
        let mut t = LiveTimeline::default();
        let start = Instant::now();
        for n in 0..1000u64 {
            let frame = n * 352;
            let now = start + Duration::from_secs_f64(frame as f64 / 44100.0);
            assert_eq!(t.timestamp(frame, now, 44100), (frame as u32, 0));
        }
    }
    #[test]
    fn capture_gap_is_skipped_once_and_next_packet_stays_contiguous() {
        let mut t = LiveTimeline::default();
        let start = Instant::now();
        t.timestamp(0, start, 44100);
        let (ts, skip) = t.timestamp(352, start + Duration::from_secs(1), 44100);
        assert_eq!((ts, skip), (44100, 43748));
        assert_eq!(
            t.timestamp(
                704,
                start + Duration::from_secs_f64(1.0 + 352.0 / 44100.0),
                44100
            ),
            (44452, 0)
        );
    }
    #[test]
    fn jitter_inside_twenty_ms_does_not_jump() {
        let mut t = LiveTimeline::default();
        let start = Instant::now();
        t.timestamp(0, start, 44100);
        assert_eq!(
            t.timestamp(352, start + Duration::from_millis(25), 44100),
            (352, 0)
        );
    }
    #[test]
    fn reset_discards_previous_skip_and_wrap_is_preserved() {
        let mut t = LiveTimeline::default();
        let start = Instant::now();
        t.timestamp(0, start, 44100);
        t.timestamp(352, start + Duration::from_secs(1), 44100);
        t.reset();
        let frame = u64::from(u32::MAX) - 100;
        assert_eq!(t.timestamp(frame, start, 44100), (frame as u32, 0));
        assert_eq!(
            t.timestamp(
                frame + 352,
                start + Duration::from_secs_f64(352.0 / 44100.0),
                44100
            ),
            ((frame + 352) as u32, 0)
        );
    }
}
