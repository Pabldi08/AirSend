//! Per-session timings and health counters; intentionally contains no identities.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};
use std::time::{Duration, Instant};

#[derive(Debug, Default, Clone, serde::Serialize)]
pub struct StreamTimings {
    pub connect_pair_ms: f64,
    pub setup_ms: f64,
    pub initial_volume_ms: f64,
    pub start_ms: f64,
    pub ready_ms: f64,
}

pub struct StreamMetrics {
    began: Instant,
    timings: Mutex<StreamTimings>,
    feedback_failures: AtomicU64,
    feedback_successes: AtomicU64,
}

impl Default for StreamMetrics {
    fn default() -> Self {
        Self {
            began: Instant::now(),
            timings: Mutex::new(StreamTimings::default()),
            feedback_failures: AtomicU64::new(0),
            feedback_successes: AtomicU64::new(0),
        }
    }
}
impl StreamMetrics {
    pub(crate) fn stage(&self, stage: &str, elapsed: Duration) {
        let ms = elapsed.as_secs_f64() * 1000.0;
        let mut timings = self.timings.lock().unwrap();
        match stage {
            "connect_pair" => timings.connect_pair_ms = ms,
            "setup" => timings.setup_ms = ms,
            "initial_volume" => timings.initial_volume_ms = ms,
            "start" => {
                timings.start_ms = ms;
                timings.ready_ms = self.began.elapsed().as_secs_f64() * 1000.0;
            }
            _ => return,
        }
        tracing::info!(stage, elapsed_ms = ms, "AirPlay session timing");
    }
    pub(crate) fn feedback(&self, success: bool) {
        if success {
            self.feedback_successes.fetch_add(1, Ordering::Relaxed);
        } else {
            self.feedback_failures.fetch_add(1, Ordering::Relaxed);
        }
    }
    pub fn snapshot(&self) -> (StreamTimings, u64, u64) {
        (
            self.timings.lock().unwrap().clone(),
            self.feedback_successes.load(Ordering::Relaxed),
            self.feedback_failures.load(Ordering::Relaxed),
        )
    }
}
