//! Local live-audio buffering. These limits do not configure receiver latency.
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy)]
pub struct LiveStreamOptions {
    pub buffer_ms: u32,
    pub startup_ms: u32,
    pub refill_ms: u32,
    pub resume_ms: u32,
    pub startup_timeout: Duration,
    pub sender_capacity: usize,
    pub max_age: Option<Duration>,
}

impl Default for LiveStreamOptions {
    fn default() -> Self {
        Self {
            buffer_ms: 2000,
            startup_ms: 1000,
            refill_ms: 800,
            resume_ms: 200,
            startup_timeout: Duration::from_secs(5),
            sender_capacity: 8,
            max_age: None,
        }
    }
}

impl LiveStreamOptions {
    pub fn low_latency() -> Self {
        Self {
            buffer_ms: 80,
            startup_ms: 24,
            refill_ms: 24,
            resume_ms: 8,
            startup_timeout: Duration::from_millis(500),
            sender_capacity: 2,
            max_age: Some(Duration::from_millis(120)),
        }
    }

    pub(crate) fn validate(self) -> airplay_core::error::Result<()> {
        if self.buffer_ms == 0
            || self.startup_ms > self.buffer_ms
            || self.refill_ms > self.buffer_ms
            || self.resume_ms > self.buffer_ms
            || self.sender_capacity == 0
            || self.startup_timeout.is_zero()
        {
            return Err(airplay_core::error::StreamingError::Encoding(
                "invalid live buffer policy".into(),
            )
            .into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Default)]
pub struct LiveDiagnosticsSnapshot {
    pub retransmit_requested: u64,
    pub retransmit_sent: u64,
    pub retransmit_missed: u64,
    pub sender_mmcss: Option<&'static str>,
    pub queued_blocks: usize,
    pub queue_drops: u64,
    pub stale_drops: u64,
    pub packets_sent: u64,
    pub send_errors: u64,
    pub underruns: u64,
    pub encoder_buffer_ms: u64,
    pub max_queue_age_us: u64,
    pub capture_to_send_p95_us: Option<u64>,
    pub first_packet_age_us: Option<u64>,
    pub ptp_reference: PtpReferenceSnapshot,
    pub rtp_sync_scheduled: u64,
    pub rtp_timeline_catchups: u64,
    pub rtp_timeline_skipped_frames: u64,
}

#[derive(Debug, Clone, Default)]
pub struct PtpReferenceSnapshot {
    pub updates_applied: u64,
    pub last_update_age_ms: Option<f64>,
    pub last_offset_change_ms: Option<f64>,
    pub max_abs_offset_change_ms: Option<f64>,
}

#[derive(Default)]
struct PtpReference {
    last_update: Option<(Instant, i64)>,
    updates_applied: u64,
    last_offset_change_ms: Option<f64>,
    max_abs_offset_change_ms: Option<f64>,
}

impl PtpReference {
    fn observe(&mut self, at: Instant, offset_ns: i64) {
        if let Some((_, previous_ns)) = self.last_update {
            let change_ms = (i128::from(offset_ns) - i128::from(previous_ns)) as f64 / 1_000_000.0;
            self.last_offset_change_ms = Some(change_ms);
            self.max_abs_offset_change_ms =
                Some(self.max_abs_offset_change_ms.unwrap_or(0.0).max(change_ms.abs()));
        }
        self.updates_applied += 1;
        self.last_update = Some((at, offset_ns));
    }

    fn snapshot(&self, at: Instant) -> PtpReferenceSnapshot {
        PtpReferenceSnapshot {
            updates_applied: self.updates_applied,
            last_update_age_ms: self.last_update.map(|(last, _)| {
                at.saturating_duration_since(last).as_secs_f64() * 1000.0
            }),
            last_offset_change_ms: self.last_offset_change_ms,
            max_abs_offset_change_ms: self.max_abs_offset_change_ms,
        }
    }
}

/// Bounded measurements shared by capture input, encoder and UDP sender.
#[derive(Default)]
pub struct LiveDiagnostics {
    pub(crate) retransmit_requested: AtomicU64,
    pub(crate) retransmit_sent: AtomicU64,
    pub(crate) retransmit_missed: AtomicU64,
    #[cfg(windows)]
    pub(crate) sender_mmcss: std::sync::atomic::AtomicU8,
    pub(crate) queue_drops: AtomicU64,
    pub(crate) evicted_samples: AtomicU64,
    pub(crate) stale_drops: AtomicU64,
    pub(crate) packets_sent: AtomicU64,
    pub(crate) send_errors: AtomicU64,
    pub(crate) underruns: AtomicU64,
    pub(crate) encoder_buffer_ms: AtomicU64,
    pub(crate) max_queue_age_us: AtomicU64,
    pub(crate) force_sync: AtomicBool,
    pub(crate) rtp_sync_scheduled: AtomicU64,
    rtp_timeline_catchups: AtomicU64,
    rtp_timeline_skipped_frames: AtomicU64,
    ptp_reference: Mutex<PtpReference>,
    ages: Mutex<VecDeque<u64>>,
    first_packet_age_us: Mutex<Option<u64>>,
}

impl LiveDiagnostics {
    pub(crate) fn observe_ptp_update(&self, offset_ns: i64) {
        self.ptp_reference
            .lock()
            .unwrap()
            .observe(Instant::now(), offset_ns);
    }

    pub(crate) fn observe_rtp_catchup(&self, skipped_frames: u64) {
        self.rtp_timeline_catchups.fetch_add(1, Ordering::Relaxed);
        self.rtp_timeline_skipped_frames.fetch_add(skipped_frames, Ordering::Relaxed);
    }

    pub(crate) fn observe_queue_age(&self, captured: Instant) {
        self.max_queue_age_us
            .fetch_max(captured.elapsed().as_micros() as u64, Ordering::Relaxed);
    }

    pub(crate) fn observe_send(&self, captured: Option<Instant>) {
        self.packets_sent.fetch_add(1, Ordering::Relaxed);
        if let Some(captured) = captured {
            let age = captured.elapsed().as_micros() as u64;
            let mut first = self.first_packet_age_us.lock().unwrap();
            first.get_or_insert(age);
            drop(first);
            let mut ages = self.ages.lock().unwrap();
            if ages.len() == 256 {
                ages.pop_front();
            }
            ages.push_back(age);
        }
    }

    pub fn snapshot(&self, queued_blocks: usize) -> LiveDiagnosticsSnapshot {
        self.snapshot_at(queued_blocks, Instant::now())
    }

    pub(crate) fn snapshot_at(&self, queued_blocks: usize, at: Instant) -> LiveDiagnosticsSnapshot {
        let mut ages: Vec<_> = self.ages.lock().unwrap().iter().copied().collect();
        ages.sort_unstable();
        LiveDiagnosticsSnapshot {
            retransmit_requested: self.retransmit_requested.load(Ordering::Relaxed),
            retransmit_sent: self.retransmit_sent.load(Ordering::Relaxed),
            retransmit_missed: self.retransmit_missed.load(Ordering::Relaxed),
            #[cfg(windows)]
            sender_mmcss: match self.sender_mmcss.load(Ordering::Relaxed) {
                1 => Some("audio"),
                2 => Some("unavailable"),
                _ => None,
            },
            #[cfg(not(windows))]
            sender_mmcss: None,
            queued_blocks,
            queue_drops: self.queue_drops.load(Ordering::Relaxed),
            stale_drops: self.stale_drops.load(Ordering::Relaxed),
            packets_sent: self.packets_sent.load(Ordering::Relaxed),
            send_errors: self.send_errors.load(Ordering::Relaxed),
            underruns: self.underruns.load(Ordering::Relaxed),
            encoder_buffer_ms: self.encoder_buffer_ms.load(Ordering::Relaxed),
            max_queue_age_us: self.max_queue_age_us.load(Ordering::Relaxed),
            capture_to_send_p95_us: ages.get(ages.len() * 95 / 100).copied(),
            first_packet_age_us: *self.first_packet_age_us.lock().unwrap(),
            ptp_reference: self.ptp_reference.lock().unwrap().snapshot(at),
            rtp_sync_scheduled: self.rtp_sync_scheduled.load(Ordering::Relaxed),
            rtp_timeline_catchups: self.rtp_timeline_catchups.load(Ordering::Relaxed),
            rtp_timeline_skipped_frames: self.rtp_timeline_skipped_frames.load(Ordering::Relaxed),
        }
    }
}

pub(crate) fn expired(captured: Option<Instant>, max_age: Option<Duration>) -> bool {
    captured
        .zip(max_age)
        .is_some_and(|(at, limit)| at.elapsed() > limit)
}

pub(crate) type SharedDiagnostics = Arc<LiveDiagnostics>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ptp_reference_reports_offset_changes_and_growing_age_without_updates() {
        let mut reference = PtpReference::default();
        let start = Instant::now();
        assert_eq!(reference.snapshot(start).last_update_age_ms, None);

        reference.observe(start, 500_000);
        let first = reference.snapshot(start);
        assert_eq!(first.updates_applied, 1);
        assert_eq!(first.last_offset_change_ms, None);

        reference.observe(start + Duration::from_secs(1), 2_500_000);
        reference.observe(start + Duration::from_secs(2), 2_000_000);
        let during_gap = reference.snapshot(start + Duration::from_secs(7));
        assert_eq!(during_gap.updates_applied, 3);
        assert_eq!(during_gap.last_update_age_ms, Some(5000.0));
        assert_eq!(during_gap.last_offset_change_ms, Some(-0.5));
        assert_eq!(during_gap.max_abs_offset_change_ms, Some(2.0));

        let resumed = start + Duration::from_secs(8);
        reference.observe(resumed, 2_000_000);
        let snapshot = reference.snapshot(resumed);
        assert_eq!(snapshot.updates_applied, 4);
        assert_eq!(snapshot.last_update_age_ms, Some(0.0));
        assert_eq!(snapshot.last_offset_change_ms, Some(0.0));
        assert_eq!(snapshot.max_abs_offset_change_ms, Some(2.0));
    }
}
