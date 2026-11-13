//! Opt-in export of bounded, typed measurements. Never includes raw logs,
//! device addresses/names, TXT data, credentials or arbitrary error strings.
use cap_core::{
    stream_metrics::StreamMetrics,
    streaming::{LiveDiagnosticsHandle, LiveFrameSender, LocalBufferPolicy},
};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

pub type SessionRecord = Arc<Mutex<Record>>;

#[derive(Clone, Copy, Default, serde::Serialize)]
struct Counters {
    underruns: u64,
    discarded: u64,
    udp_errors: u64,
    retransmit_requested: u64,
    retransmit_sent: u64,
    retransmit_missed: u64,
}

#[derive(serde::Serialize)]
struct Interval {
    from_ms: u64,
    to_ms: u64,
    counts: Counters,
}

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
    last_failure_code: Option<&'static str>,
    requested_latency_ms: u32,
    policy: LocalBufferPolicy,
    capture_start_ms: f64,
    capture: audio_capture::CaptureDiagnostics,
    forwarded: u64,
    rejected: u64,
    max_capture_gap_ms: f64,
    outputs: Vec<Output>,
    intervals: VecDeque<Interval>,
    previous: Counters,
    sampled_ms: u64,
    distortion_marks_ms: VecDeque<u64>,
}

impl Record {
    fn sample(&mut self) {
        let elapsed = self.began.elapsed().as_millis() as u64;
        let mut total = Counters::default();
        let mut group_seen = false;
        for output in &self.outputs {
            if output.transport_scope == "group" && std::mem::replace(&mut group_seen, true) {
                continue;
            }
            let s = output.sender.snapshot();
            total.underruns += s.underruns;
            total.discarded += s.queue_drops + s.stale_drops;
            total.udp_errors += s.send_errors;
            total.retransmit_requested += s.retransmit_requested;
            total.retransmit_sent += s.retransmit_sent;
            total.retransmit_missed += s.retransmit_missed;
        }
        let previous = std::mem::replace(&mut self.previous, total);
        if self.intervals.len() == 180 {
            self.intervals.pop_front();
        }
        self.intervals.push_back(Interval {
            from_ms: self.sampled_ms,
            to_ms: elapsed,
            counts: Counters {
                underruns: total.underruns.saturating_sub(previous.underruns),
                discarded: total.discarded.saturating_sub(previous.discarded),
                udp_errors: total.udp_errors.saturating_sub(previous.udp_errors),
                retransmit_requested: total.retransmit_requested.saturating_sub(previous.retransmit_requested),
                retransmit_sent: total.retransmit_sent.saturating_sub(previous.retransmit_sent),
                retransmit_missed: total.retransmit_missed.saturating_sub(previous.retransmit_missed),
            },
        });
        self.sampled_ms = elapsed;
    }
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
        self.sample();
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
        self.last_failure_code = None;
    }
    pub fn record_stream_failure(&mut self, error: &cap_core::streaming::StreamError) {
        self.last_failure_stage = Some(error.stage_code());
        self.last_failure_code = Some(error.failure_code());
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
        let at = self.ended.unwrap_or_else(Instant::now);
        let outputs: Vec<_> = self.outputs.iter().enumerate().map(|(index, output)| {
            let s = output.sender.snapshot_at(at);
            let (timings, successes, failures) = output.metrics.snapshot();
            let ptp_reference = (output.metrics.timing_protocol() == "ptp").then(|| serde_json::json!({
                "updates_applied": s.ptp_reference.updates_applied,
                "last_update_age_ms": s.ptp_reference.last_update_age_ms,
                "last_offset_change_ms": s.ptp_reference.last_offset_change_ms,
                "max_abs_offset_change_ms": s.ptp_reference.max_abs_offset_change_ms,
            }));
            serde_json::json!({
                "receiver": index + 1, "transport_scope": output.transport_scope, "timings": timings, "timing_protocol": output.metrics.timing_protocol(),
                "feedback_successes": successes, "feedback_failures": failures,
                "input_queued_blocks": s.queued_blocks, "input_queue_drops": s.queue_drops,
                "stale_drops": s.stale_drops, "packets_sent": s.packets_sent, "send_errors": s.send_errors,
                "encoder_buffer_ms": s.encoder_buffer_ms, "encoder_underruns": s.underruns, "max_input_queue_age_ms": s.max_queue_age_us as f64 / 1000.0,
                "capture_delivery_to_udp_p95_ms": s.capture_to_send_p95_us.map(|v| v as f64 / 1000.0),
                "first_packet_age_ms": s.first_packet_age_us.map(|v| v as f64 / 1000.0),
                "ptp_reference": ptp_reference,
                "rtp_sync_scheduled": s.rtp_sync_scheduled,
                "rtp_timeline_catchups": s.rtp_timeline_catchups,
                "rtp_timeline_skipped_frames": s.rtp_timeline_skipped_frames,
                "sender_mmcss": s.sender_mmcss,
                "retransmit_requested": s.retransmit_requested,
                "retransmit_sent": s.retransmit_sent,
                "retransmit_missed": s.retransmit_missed,
            })
        }).collect();
        let timing = self
            .outputs
            .first()
            .map(|first| {
                let protocol = first.metrics.timing_protocol();
                if self
                    .outputs
                    .iter()
                    .all(|o| o.metrics.timing_protocol() == protocol)
                {
                    protocol
                } else {
                    "mixed"
                }
            })
            .unwrap_or("unknown");
        serde_json::json!({
            "status": self.status, "last_failure_stage": self.last_failure_stage, "last_failure_code": self.last_failure_code, "session_duration_ms": at.saturating_duration_since(self.began).as_millis() as u64,
            "audio_format": { "sample_rate": 44_100, "channels": 2, "sample_bits": 16, "codec": "alac", "timing": timing },
            "requested_receiver_latency_ms": self.requested_latency_ms, "local_buffer_policy": self.policy,
            "capture_start_ms": self.capture_start_ms, "capture": self.capture,
            "blocks_forwarded": self.forwarded, "blocks_rejected": self.rejected,
            "max_capture_gap_ms": self.max_capture_gap_ms, "outputs": outputs,
            "intervals": self.intervals, "distortion_marks_ms": self.distortion_marks_ms,
        })
    }
}

#[derive(Default)]
pub struct DiagnosticState {
    sessions: Mutex<VecDeque<SessionRecord>>,
    reconnections: AtomicU64,
}
impl DiagnosticState {
    pub fn mark_distortion(&self) -> Result<(), String> {
        let sessions = self.sessions.lock().unwrap();
        let mut record = sessions.iter().rev().find_map(|session| {
            let record = session.lock().unwrap();
            (record.status == "streaming").then_some(record)
        }).ok_or("no_active_session")?;
        record.sample();
        let at = record.sampled_ms;
        if record.distortion_marks_ms.len() == 60 {
            record.distortion_marks_ms.pop_front();
        }
        record.distortion_marks_ms.push_back(at);
        Ok(())
    }
    pub fn record_reconnection(&self) {
        self.reconnections.fetch_add(1, Ordering::Relaxed);
    }
    pub fn begin(&self, latency: u32, policy: LocalBufferPolicy) -> SessionRecord {
        let record = Arc::new(Mutex::new(Record {
            began: Instant::now(),
            ended: None,
            status: "preparing",
            last_failure_stage: None,
            last_failure_code: None,
            requested_latency_ms: latency,
            policy,
            capture_start_ms: 0.0,
            capture: Default::default(),
            forwarded: 0,
            rejected: 0,
            max_capture_gap_ms: 0.0,
            outputs: Vec::new(),
            intervals: VecDeque::new(),
            previous: Counters::default(),
            sampled_ms: 0,
            distortion_marks_ms: VecDeque::new(),
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
            "reconnections": self.reconnections.load(Ordering::Relaxed),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_failure_exports_only_the_stage_and_classification() {
        let state = DiagnosticState::default();
        let record = state.begin(100, LocalBufferPolicy::Stable);
        record.lock().unwrap().record_stream_failure(&cap_core::streaming::StreamError::Encoder(
            "private diagnostic detail".into(),
        ));
        let report = state.report();
        assert_eq!(report["sessions"][0]["last_failure_code"], "encoder");
        assert!(!report.to_string().contains("private diagnostic detail"));
    }
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
