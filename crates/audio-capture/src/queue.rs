//! Capture delivery queue with a stable policy and an opt-in time budget.
use std::collections::VecDeque;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::CapturedFrame;
use crossbeam_channel::{RecvTimeoutError, TrySendError};

#[derive(Debug, Clone, Copy, Default)]
pub enum CapturePolicy {
    #[default]
    Stable,
    LowLatency,
}

#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct CaptureDiagnostics {
    pub queued_blocks: usize,
    pub queued_ms: f64,
    pub dropped_oldest: u64,
    pub dropped_newest: u64,
    pub stale_drops: u64,
    pub max_delivery_age_ms: f64,
}

struct Queue {
    frames: VecDeque<CapturedFrame>,
    duration: Duration,
    producers: usize,
    receiver_alive: bool,
    policy: CapturePolicy,
    stats: CaptureDiagnostics,
}
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

pub struct CaptureSender(Arc<Shared>);
pub struct CaptureReceiver(Arc<Shared>);

pub fn capture_channel(policy: CapturePolicy) -> (CaptureSender, CaptureReceiver) {
    let shared = Arc::new(Shared {
        queue: Mutex::new(Queue {
            frames: VecDeque::with_capacity(64),
            duration: Duration::ZERO,
            producers: 1,
            receiver_alive: true,
            policy,
            stats: CaptureDiagnostics::default(),
        }),
        ready: Condvar::new(),
    });
    (CaptureSender(shared.clone()), CaptureReceiver(shared))
}

impl Queue {
    fn pop(&mut self) -> Option<CapturedFrame> {
        let frame = self.frames.pop_front()?;
        self.duration = self.duration.saturating_sub(frame.duration());
        Some(frame)
    }
    fn expire(&mut self, now: Instant) {
        if matches!(self.policy, CapturePolicy::LowLatency) {
            while self.frames.front().is_some_and(|f| {
                now.saturating_duration_since(f.captured_at) > Duration::from_millis(60)
            }) {
                self.pop();
                self.stats.stale_drops += 1;
            }
        }
    }
}

impl CaptureSender {
    pub fn len(&self) -> usize {
        self.0.queue.lock().unwrap().frames.len()
    }
    pub fn try_send(&self, frame: CapturedFrame) -> Result<(), TrySendError<CapturedFrame>> {
        let mut q = self.0.queue.lock().unwrap();
        if !q.receiver_alive {
            return Err(TrySendError::Disconnected(frame));
        }
        q.expire(Instant::now());
        if matches!(q.policy, CapturePolicy::LowLatency) {
            let budget = Duration::from_millis(60);
            if frame.duration() > budget || frame.captured_at.elapsed() > budget {
                q.stats.stale_drops += 1;
                return Err(TrySendError::Full(frame));
            }
            while !q.frames.is_empty()
                && (q.duration + frame.duration() > budget || q.frames.len() >= 64)
            {
                q.pop();
                q.stats.dropped_oldest += 1;
            }
        } else if q.frames.len() >= 64 {
            q.stats.dropped_newest += 1;
            return Err(TrySendError::Full(frame));
        }
        q.duration += frame.duration();
        q.frames.push_back(frame);
        drop(q);
        self.0.ready.notify_one();
        Ok(())
    }
}

impl Clone for CaptureSender {
    fn clone(&self) -> Self {
        self.0.queue.lock().unwrap().producers += 1;
        Self(self.0.clone())
    }
}
impl Drop for CaptureSender {
    fn drop(&mut self) {
        self.0.queue.lock().unwrap().producers -= 1;
        self.0.ready.notify_one();
    }
}
impl Drop for CaptureReceiver {
    fn drop(&mut self) {
        let mut q = self.0.queue.lock().unwrap();
        q.receiver_alive = false;
        q.frames.clear();
        q.duration = Duration::ZERO;
    }
}
impl CaptureReceiver {
    pub fn recv_timeout(&self, timeout: Duration) -> Result<CapturedFrame, RecvTimeoutError> {
        let deadline = Instant::now() + timeout;
        let mut q = self.0.queue.lock().unwrap();
        loop {
            let now = Instant::now();
            q.expire(now);
            if let Some(frame) = q.pop() {
                q.stats.max_delivery_age_ms = q.stats.max_delivery_age_ms.max(
                    now.saturating_duration_since(frame.captured_at)
                        .as_secs_f64()
                        * 1000.0,
                );
                return Ok(frame);
            }
            if q.producers == 0 {
                return Err(RecvTimeoutError::Disconnected);
            }
            if now >= deadline {
                return Err(RecvTimeoutError::Timeout);
            }
            q = self
                .0
                .ready
                .wait_timeout(q, deadline.saturating_duration_since(now))
                .unwrap()
                .0;
        }
    }
    pub fn diagnostics(&self) -> CaptureDiagnostics {
        let q = self.0.queue.lock().unwrap();
        let mut stats = q.stats.clone();
        stats.queued_blocks = q.frames.len();
        stats.queued_ms = q.duration.as_secs_f64() * 1000.0;
        stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn block(value: i16) -> CapturedFrame {
        CapturedFrame::new(vec![value; 704], 2, 44_100)
    }
    #[test]
    fn saturated_experimental_queue_keeps_recent_audio_within_budget() {
        let (tx, rx) = capture_channel(CapturePolicy::LowLatency);
        for i in 0..100 {
            tx.try_send(block(i)).unwrap();
        }
        let stats = rx.diagnostics();
        assert!(stats.queued_ms <= 60.0);
        assert!(stats.dropped_oldest > 0);
        assert!(rx.recv_timeout(Duration::ZERO).unwrap().samples[0] >= 93);
    }
    #[test]
    fn stable_queue_keeps_legacy_overflow_behavior() {
        let (tx, rx) = capture_channel(CapturePolicy::Stable);
        for i in 0..64 {
            tx.try_send(block(i)).unwrap();
        }
        assert!(tx.try_send(block(64)).is_err());
        assert_eq!(rx.recv_timeout(Duration::ZERO).unwrap().samples[0], 0);
        assert_eq!(rx.diagnostics().dropped_newest, 1);
    }
    #[test]
    fn stalled_consumer_discards_expired_audio_and_reports_closed_producer() {
        let (tx, rx) = capture_channel(CapturePolicy::LowLatency);
        tx.try_send(block(1)).unwrap();
        rx.0.queue.lock().unwrap().frames[0].captured_at = Instant::now() - Duration::from_secs(1);
        drop(tx);
        assert!(matches!(
            rx.recv_timeout(Duration::ZERO),
            Err(RecvTimeoutError::Disconnected)
        ));
        assert_eq!(rx.diagnostics().stale_drops, 1);
    }
    #[test]
    fn dropping_consumer_does_not_leave_a_live_producer_channel() {
        let (tx, rx) = capture_channel(CapturePolicy::LowLatency);
        drop(rx);
        assert!(matches!(
            tx.try_send(block(1)),
            Err(TrySendError::Disconnected(_))
        ));
    }
}
