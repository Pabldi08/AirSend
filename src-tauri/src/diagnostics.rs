//! Opt-in export of bounded, typed measurements. Never includes raw logs,
//! device addresses/names, TXT data, credentials or arbitrary error strings.
use cap_core::{
    stream_metrics::StreamMetrics,
    streaming::{LiveDiagnosticsHandle, LiveFrameSender, LocalBufferPolicy},
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub type SessionRecord = Arc<Mutex<Record>>;

struct Output {
    sender: LiveDiagnosticsHandle,
    metrics: Arc<StreamMetrics>,
    transport_scope: &'static str,
}

pub struct PreparationGuard(SessionRecord);
impl PreparationGuard {
    pub fn new(record: SessionRecord) -> Self {
        Self(record)
    }
}
impl Drop for PreparationGuard {
    fn drop(&mut self) {
        let mut record = self.0.lock().unwrap();
        if record.status == "preparing" {
            record.failed();
        }
    }
}

pub struct Record {
    began: Instant,
    ended: Option<Instant>,
    status: &'static str,
    last_failure_stage: Option<&'static str>,
    requested_latency_ms: u32,
    policy: LocalBufferPolicy,
    capture_start_ms: f64,
    capture: audio_capture::CaptureDiagnostics,
    forwarded: u64,
    rejected: u64,
    max_capture_gap_ms: f64,
    outputs: Vec<Output>,
}

impl Record {
    pub fn add_output(&mut self, sender: LiveFrameSender, metrics: Arc<StreamMetrics>) {
        self.outputs.push(Output {
            sender: sender.diagnostics_handle(),
            metrics,
            transport_scope: "receiver",
        });
    }
    pub fn add_group_output(&mut self, sender: LiveFrameSender, metrics: Arc<StreamMetrics>) {
        self.outputs.push(Output {
            sender: sender.diagnostics_handle(),
            metrics,
            transport_scope: "group",
        });
    }
    pub fn capture_started(&mut self, elapsed: std::time::Duration) {
        self.capture_start_ms = elapsed.as_secs_f64() * 1000.0;
    }
    pub fn update(
        &mut self,
        capture: audio_capture::CaptureDiagnostics,
        forwarded: u64,
        rejected: u64,
        gap: std::time::Duration,
    ) {
        self.capture = capture;
        self.forwarded = forwarded;
        self.rejected = rejected;
        self.max_capture_gap_ms = self.max_capture_gap_ms.max(gap.as_secs_f64() * 1000.0);
        tracing::info!(
            queued_capture_ms = self.capture.queued_ms,
            capture_dropped_oldest = self.capture.dropped_oldest,
            capture_dropped_newest = self.capture.dropped_newest,
            capture_stale_drops = self.capture.stale_drops,
            max_capture_delivery_age_ms = self.capture.max_delivery_age_ms,
            "capture queue diagnostics"
        );
        for (index, output) in self.outputs.iter().enumerate() {
            let live = output.sender.snapshot();
            let (_, _, feedback_failures) = output.metrics.snapshot();
            tracing::info!(
                receiver = index + 1,
                input_queued_blocks = live.queued_blocks,
                input_queue_drops = live.queue_drops,
                stale_drops = live.stale_drops,
                encoder_buffer_ms = live.encoder_buffer_ms,
                encoder_underruns = live.underruns,
                capture_delivery_to_udp_p95_ms =
                    live.capture_to_send_p95_us.map(|v| v as f64 / 1000.0),
                packets_sent = live.packets_sent,
                send_errors = live.send_errors,
                feedback_failures,
                "receiver audio diagnostics"
            );
        }
    }
    pub fn send_errors(&self) -> u64 {
        self.outputs
            .iter()
            .map(|o| o.sender.snapshot().send_errors)
            .sum()
    }
    pub fn streaming(&mut self) {
        self.status = "streaming";
    }
    pub fn record_failure(&mut self, stage: &'static str) {
        self.last_failure_stage = Some(stage);
    }
    pub fn failed(&mut self) {
        self.status = "failed";
        self.ended.get_or_insert(Instant::now());
    }
    pub fn finish(&mut self) {
        if self.status == "preparing" {
            self.failed();
        } else if self.status == "streaming" {
            self.status = "stopped";
            self.ended = Some(Instant::now());
        }
    }
    fn snapshot(&self) -> serde_json::Value {
        let outputs: Vec<_> = self.outputs.iter().enumerate().map(|(index, output)| {
            let s = output.sender.snapshot();
            let (timings, successes, failures) = output.metrics.snapshot();
            serde_json::json!({
                "receiver": index + 1, "transport_scope": output.transport_scope, "timings": timings,
                "feedback_successes": successes, "feedback_failures": failures,
                "input_queued_blocks": s.queued_blocks, "input_queue_drops": s.queue_drops,
                "stale_drops": s.stale_drops, "packets_sent": s.packets_sent, "send_errors": s.send_errors,
                "encoder_buffer_ms": s.encoder_buffer_ms, "encoder_underruns": s.underruns, "max_input_queue_age_ms": s.max_queue_age_us as f64 / 1000.0,
                "capture_delivery_to_udp_p95_ms": s.capture_to_send_p95_us.map(|v| v as f64 / 1000.0),
                "first_packet_age_ms": s.first_packet_age_us.map(|v| v as f64 / 1000.0),
            })
        }).collect();
        serde_json::json!({
            "status": self.status, "last_failure_stage": self.last_failure_stage, "session_duration_ms": self.ended.unwrap_or_else(Instant::now).saturating_duration_since(self.began).as_millis() as u64,
            "audio_format": { "sample_rate": 44_100, "channels": 2, "sample_bits": 16, "codec": "alac", "timing": "ntp" },
            "requested_receiver_latency_ms": self.requested_latency_ms, "local_buffer_policy": self.policy,
            "capture_start_ms": self.capture_start_ms, "capture": self.capture,
            "blocks_forwarded": self.forwarded, "blocks_rejected": self.rejected,
            "max_capture_gap_ms": self.max_capture_gap_ms, "outputs": outputs,
        })
    }
}

#[derive(Default)]
pub struct DiagnosticState {
    sessions: Mutex<VecDeque<SessionRecord>>,
}
impl DiagnosticState {
    pub fn begin(&self, latency: u32, policy: LocalBufferPolicy) -> SessionRecord {
        let record = Arc::new(Mutex::new(Record {
            began: Instant::now(),
            ended: None,
            status: "preparing",
            last_failure_stage: None,
            requested_latency_ms: latency,
            policy,
            capture_start_ms: 0.0,
            capture: Default::default(),
            forwarded: 0,
            rejected: 0,
            max_capture_gap_ms: 0.0,
            outputs: Vec::new(),
        }));
        let mut sessions = self.sessions.lock().unwrap();
        if sessions.len() == 6 {
            sessions.pop_front();
        }
        sessions.push_back(record.clone());
        record
    }
    pub fn report(&self) -> serde_json::Value {
        let sessions: Vec<_> = self
            .sessions
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.lock().unwrap().snapshot())
            .collect();
        serde_json::json!({
            "schema_version": 1, "app_version": env!("CARGO_PKG_VERSION"),
            "build_revision": option_env!("AIRSEND_BUILD_REVISION"),
            "os": std::env::consts::OS, "architecture": std::env::consts::ARCH,
            "measurement": "software capture delivery to local UDP send; excludes audio-engine, receiver and acoustic delay",
            "sessions": sessions,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn history_is_bounded_and_export_only_contains_typed_metrics() {
        let state = DiagnosticState::default();
        for _ in 0..10 {
            state
                .begin(0, LocalBufferPolicy::LowLatency)
                .lock()
                .unwrap()
                .finish();
        }
        let report = state.report();
        assert_eq!(report["sessions"].as_array().unwrap().len(), 6);
        assert_eq!(report["sessions"][0]["status"], "failed");
        let json = report.to_string();
        for field in [
            "password",
            "device_name",
            "address",
            "public_key",
            "raw_log",
        ] {
            assert!(!json.contains(field));
        }
    }
}
