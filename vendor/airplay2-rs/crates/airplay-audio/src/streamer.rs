//! High-level audio streaming orchestrator.

use airplay_core::{StreamConfig, error::Result};
use crate::{AudioBuffer, AudioDecoder, RtpSender, LiveAudioDecoder};
use crate::encoder::{create_encoder, AudioEncoder};
use crate::eq::{EqConfig, EqParams, Equalizer};
use airplay_timing::{Clock, ClockOffset, unix_to_ntp};
use std::sync::{Arc, atomic::{AtomicU64, AtomicU8, Ordering}};
use tokio::sync::{Mutex, watch};
use tokio::task::JoinHandle;
use tokio::time::{sleep, Duration, Instant};
use crossbeam_channel::{bounded, Sender, Receiver};
use std::net::{SocketAddr, UdpSocket};

/// Streaming state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamerState {
    /// Not started.
    Idle,
    /// Buffering initial audio.
    Buffering,
    /// Actively streaming.
    Streaming,
    /// Paused.
    Paused,
    /// Stopped.
    Stopped,
    /// Error occurred.
    Error,
}

/// Try to set real-time priority for the current thread (Linux only).
#[cfg(target_os = "linux")]
fn set_realtime_priority() {
    use std::mem;
    unsafe {
        let param: libc::sched_param = mem::zeroed();
        let mut param = param;
        param.sched_priority = 50; // RT priority 50 (1-99 scale)

        let result = libc::sched_setscheduler(
            0, // current thread
            libc::SCHED_FIFO,
            &param as *const _,
        );

        if result == 0 {
            tracing::info!("Set real-time priority (SCHED_FIFO, priority 50)");
        } else {
            tracing::warn!("Failed to set RT priority (need CAP_SYS_NICE or root): errno={}",
                *libc::__errno_location());
        }
    }
}

/// Keep the current thread in the "Pro Audio" MMCSS task class for as long as
/// the returned guard is alive.
#[cfg(windows)]
struct MmcssGuard(*mut std::ffi::c_void);

#[cfg(windows)]
impl Drop for MmcssGuard {
    fn drop(&mut self) {
        #[link(name = "avrt")]
        extern "system" {
            fn AvRevertMmThreadCharacteristics(handle: *mut std::ffi::c_void) -> i32;
        }
        unsafe {
            if AvRevertMmThreadCharacteristics(self.0) == 0 {
                tracing::warn!("Windows: AvRevertMmThreadCharacteristics failed");
            }
        }
    }
}

#[cfg(windows)]
fn set_realtime_priority() -> Option<MmcssGuard> {
    use std::ffi::c_void;

    #[allow(non_camel_case_types)]
    type HANDLE = *mut c_void;
    #[allow(non_camel_case_types)]
    type BOOL = i32;
    const AVRT_PRIORITY_CRITICAL: i32 = 2;

    #[link(name = "avrt")]
    extern "system" {
        fn AvSetMmThreadCharacteristicsW(task: *const u16, task_index: *mut u32) -> HANDLE;
        fn AvSetMmThreadPriority(handle: HANDLE, priority: i32) -> BOOL;
        fn AvRevertMmThreadCharacteristics(handle: HANDLE) -> BOOL;
    }

    unsafe {
        let task: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
        let mut task_index: u32 = 0;
        let handle = AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut task_index as *mut _);
        if handle.is_null() {
            tracing::warn!("Windows: AvSetMmThreadCharacteristicsW(\"Pro Audio\") returned NULL");
            None
        } else if AvSetMmThreadPriority(handle, AVRT_PRIORITY_CRITICAL) == 0 {
            tracing::warn!("Windows: AvSetMmThreadPriority(CRITICAL) failed");
            let _ = AvRevertMmThreadCharacteristics(handle);
            None
        } else {
            tracing::info!(
                "Windows: MMCSS \"Pro Audio\" task associated at CRITICAL priority (task_index={task_index})"
            );
            Some(MmcssGuard(handle))
        }
    }
}

#[cfg(not(any(target_os = "linux", windows)))]
fn set_realtime_priority() {
    tracing::debug!("RT priority not supported on this platform");
}

/// Disable WiFi power save to prevent packet loss from radio sleep (Linux only).
///
/// WiFi power management causes the radio to periodically sleep, which drops
/// outgoing packets during sleep windows. For real-time audio streaming this
/// is a major source of packet loss, especially on Raspberry Pi.
#[cfg(target_os = "linux")]
fn disable_wifi_power_save() {
    use std::process::Command;
    // Try common wireless interface names
    for iface in &["wlan0", "wlp2s0", "wlp3s0"] {
        match Command::new("iw").args([*iface, "set", "power_save", "off"]).output() {
            Ok(output) if output.status.success() => {
                tracing::info!("Disabled WiFi power save on {}", iface);
                return;
            }
            _ => {}
        }
    }
    tracing::debug!("Could not disable WiFi power save (no wireless interface found or no permissions)");
}

#[cfg(not(target_os = "linux"))]
fn disable_wifi_power_save() {}

/// Message sent from the async producer to the dedicated sender thread.
enum SenderMessage {
    /// A fully serialized packet ready for timed transmission.
    Packet {
        /// Pre-serialized wire bytes per target (one entry per SendTarget).
        /// Single-device: 1 entry. Group: N entries (each encrypted with device-specific cipher).
        wire_packets: Vec<Vec<u8>>,
        /// Pre-serialized sync packet bytes, if sync is needed this frame.
        /// Shared across all targets (sync content is identical for all devices).
        sync_data: Option<Vec<u8>>,
        captured_at: Option<std::time::Instant>,
        max_age: Option<Duration>,
    },
    /// Pause: sender thread should stop advancing deadlines and wait for Resume.
    Pause,
    /// Resume: reset deadline to now and resume sending.
    Resume,
    /// Stop: sender thread should exit.
    Stop,
}

/// Sleep until an absolute deadline using the best available method.
///
/// On Linux with SCHED_FIFO, `clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME)`
/// provides ~10-50μs precision regardless of CONFIG_HZ. On other platforms,
/// falls back to spin_sleep.
#[cfg(target_os = "linux")]
fn precise_sleep_until(deadline_ns: u64) {
    use std::mem;

    let ts = libc::timespec {
        tv_sec: (deadline_ns / 1_000_000_000) as libc::time_t,
        tv_nsec: (deadline_ns % 1_000_000_000) as libc::c_long,
    };

    unsafe {
        // TIMER_ABSTIME = 1, CLOCK_MONOTONIC = 1
        libc::clock_nanosleep(
            libc::CLOCK_MONOTONIC,
            1, // TIMER_ABSTIME
            &ts as *const libc::timespec,
            std::ptr::null_mut(),
        );
    }
}

/// Get current CLOCK_MONOTONIC time in nanoseconds.
#[cfg(target_os = "linux")]
fn monotonic_now_ns() -> u64 {
    use std::mem;
    unsafe {
        let mut ts: libc::timespec = mem::zeroed();
        libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts as *mut _);
        ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
    }
}

/// A send target for the dedicated sender thread.
///
/// Each target has its own data socket and destination, plus optional control
/// socket/dest for sync packets. In single-device mode there is one target;
/// in group mode there is one per device.
struct SendTarget {
    data_socket: UdpSocket,
    data_dest: SocketAddr,
    control_socket: Option<UdpSocket>,
    control_dest: Option<SocketAddr>,
}

/// Dedicated OS thread for precise packet timing.
///
/// On Linux, uses `clock_nanosleep(CLOCK_MONOTONIC, TIMER_ABSTIME)` with
/// SCHED_FIFO for ~10-50μs precision. On other platforms, falls back to
/// `spin_sleep` hybrid kernel-sleep + spin-wait.
///
/// When `burst_size > 1`, packets are buffered and sent in bursts to reduce
/// WiFi/Bluetooth interference on shared-radio devices (e.g., Pi Zero 2 W).
/// For example, burst_size=4 sends 4 packets rapidly, then waits 4*frame_duration.
fn sender_thread_main(
    rx: Receiver<SenderMessage>,
    targets: Vec<SendTarget>,
    frame_duration: std::time::Duration,
    burst_size: usize,
    metrics: crate::live_policy::SharedDiagnostics,
    stop: Arc<std::sync::atomic::AtomicBool>,
) {
    let _priority_guard = set_realtime_priority();
    #[cfg(not(test))]
    disable_wifi_power_save();

    let burst_size = burst_size.max(1); // Minimum 1
    let frame_duration_ns = frame_duration.as_nanos() as u64;
    let burst_duration_ns = frame_duration_ns * burst_size as u64;
    #[cfg(target_os = "linux")]
    let mut next_deadline_ns = monotonic_now_ns();
    #[cfg(not(target_os = "linux"))]
    let sleeper = spin_sleep::SpinSleeper::default();
    #[cfg(not(target_os = "linux"))]
    let mut next_deadline = std::time::Instant::now();

    let mut last_send = std::time::Instant::now();
    let mut started = false;
    let mut packet_count: u64 = 0;
    let mut max_jitter_ms: f64 = 0.0;
    let mut jitter_sum_ms: f64 = 0.0;
    let mut jitter_exceed_count: u64 = 0;

    // Burst buffer for WiFi/BT coexistence
    // Each entry: (wire_packets per target, optional shared sync data)
    let mut burst_buffer: Vec<(Vec<Vec<u8>>, Option<Vec<u8>>, Option<std::time::Instant>, Option<Duration>)> = Vec::with_capacity(burst_size);

    let target_count = targets.len();
    tracing::info!(
        "Sender thread started: {} target(s), frame_duration={:.3}ms, burst_size={}, timing={}",
        target_count,
        frame_duration.as_secs_f64() * 1000.0,
        burst_size,
        if cfg!(target_os = "linux") { "clock_nanosleep(TIMER_ABSTIME)" } else { "spin_sleep" }
    );

    loop {
        if stop.load(Ordering::Acquire) { break; }
        // Receive next message (blocking with timeout for clean shutdown detection)
        let msg = match rx.recv_timeout(std::time::Duration::from_millis(20)) {
            Ok(msg) => msg,
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                tracing::info!("Sender thread: channel disconnected, exiting");
                break;
            }
        };

        match msg {
            SenderMessage::Stop => {
                tracing::info!("Sender thread: received Stop, exiting");
                break;
            }
            SenderMessage::Pause => {
                tracing::debug!("Sender thread: paused");
                loop {
                    if stop.load(Ordering::Acquire) { return; }
                    match rx.recv_timeout(std::time::Duration::from_millis(50)) {
                        Ok(SenderMessage::Resume) => {
                            tracing::debug!("Sender thread: resumed");
                            #[cfg(target_os = "linux")]
                            { next_deadline_ns = monotonic_now_ns(); }
                            #[cfg(not(target_os = "linux"))]
                            { next_deadline = std::time::Instant::now(); }
                            break;
                        }
                        Ok(SenderMessage::Stop) => {
                            tracing::info!("Sender thread: received Stop while paused, exiting");
                            return;
                        }
                        _ => continue,
                    }
                }
                continue;
            }
            SenderMessage::Resume => {
                #[cfg(target_os = "linux")]
                { next_deadline_ns = monotonic_now_ns(); }
                #[cfg(not(target_os = "linux"))]
                { next_deadline = std::time::Instant::now(); }
                continue;
            }
            SenderMessage::Packet { wire_packets, sync_data, captured_at, max_age } => {
                // Buffer packet for burst sending
                burst_buffer.push((wire_packets, sync_data, captured_at, max_age));

                // Only send when we have a full burst (or first packet to initialize timing)
                if burst_buffer.len() < burst_size && started {
                    continue;
                }

                if !started {
                    #[cfg(target_os = "linux")]
                    { next_deadline_ns = monotonic_now_ns(); }
                    #[cfg(not(target_os = "linux"))]
                    { next_deadline = std::time::Instant::now(); }
                    last_send = std::time::Instant::now();
                    started = true;
                } else {
                    // Wait until the precise deadline for this burst
                    #[cfg(target_os = "linux")]
                    {
                        precise_sleep_until(next_deadline_ns);
                    }
                    #[cfg(not(target_os = "linux"))]
                    {
                        let now = std::time::Instant::now();
                        if next_deadline > now {
                            sleeper.sleep(next_deadline - now);
                        }
                    }
                }

                // Measure actual interval and jitter (for burst timing)
                let send_time = std::time::Instant::now();
                let actual_interval = send_time.duration_since(last_send);
                let interval_ms = actual_interval.as_secs_f64() * 1000.0;
                let target_ms = frame_duration.as_secs_f64() * 1000.0 * burst_buffer.len() as f64;
                let jitter_ms = interval_ms - target_ms;

                // Track jitter stats (per burst)
                packet_count += burst_buffer.len() as u64;
                if packet_count > burst_size as u64 {
                    let abs_jitter = jitter_ms.abs();
                    if abs_jitter > max_jitter_ms {
                        max_jitter_ms = abs_jitter;
                    }
                    jitter_sum_ms += abs_jitter;
                    if abs_jitter > 1.0 * burst_size as f64 {
                        jitter_exceed_count += 1;
                    }

                    // Log burst with significant jitter
                    if abs_jitter > 2.0 * burst_size as f64 {
                        tracing::warn!(
                            "JITTER burst#{}: interval={:.3}ms target={:.3}ms jitter={:+.3}ms",
                            packet_count / burst_size as u64, interval_ms, target_ms, jitter_ms
                        );
                    }

                    // Log stats every 500 packets (~4s)
                    if packet_count % 500 < burst_size as u64 {
                        let bursts = packet_count / burst_size as u64;
                        let avg_jitter = if bursts > 1 { jitter_sum_ms / (bursts - 1) as f64 } else { 0.0 };
                        tracing::info!(
                            "TIMING STATS after {} pkts ({} bursts): avg_jitter={:.3}ms max_jitter={:.3}ms exceeds={}",
                            packet_count, bursts, avg_jitter, max_jitter_ms, jitter_exceed_count
                        );
                    }
                }

                last_send = send_time;

                // Send all buffered packets
                for (wire_packets, sync_data, captured_at, max_age) in burst_buffer.drain(..) {
                    if crate::live_policy::expired(captured_at, max_age) {
                        metrics.stale_drops.fetch_add(1, Ordering::Relaxed);
                        metrics.force_sync.store(true, Ordering::Release);
                        continue;
                    }
                    // Send sync packet to ALL targets' control dests
                    if let Some(ref sync) = sync_data {
                        for target in &targets {
                            let sock = target.control_socket.as_ref()
                                .unwrap_or(&target.data_socket);
                            let dest = target.control_dest
                                .unwrap_or(target.data_dest);
                            if let Err(e) = sock.send_to(sync, dest) {
                                tracing::error!("Failed to send sync packet to {}: {}", dest, e);
                            }
                        }
                    }

                    // Send per-target audio packets
                    let mut delivered = false;
                    for (i, target) in targets.iter().enumerate() {
                        if let Some(wire_data) = wire_packets.get(i) {
                            match target.data_socket.send_to(wire_data, target.data_dest) {
                                Ok(_) => delivered = true,
                                Err(e) => {
                                    metrics.send_errors.fetch_add(1, Ordering::Relaxed);
                                    tracing::error!("Failed to send audio packet to {}: {}", target.data_dest, e);
                                }
                            }
                        }
                    }
                    if delivered { metrics.observe_send(captured_at); }
                }

                // Advance deadline by burst duration
                #[cfg(target_os = "linux")]
                {
                    next_deadline_ns += burst_duration_ns;
                    // Catch up if we've fallen behind by more than one burst
                    let now_ns = monotonic_now_ns();
                    if now_ns > next_deadline_ns + burst_duration_ns {
                        tracing::warn!(
                            "Sender thread fell behind by {:.2}ms, resetting deadline",
                            (now_ns - next_deadline_ns) as f64 / 1_000_000.0
                        );
                        next_deadline_ns = now_ns;
                    }
                }
                #[cfg(not(target_os = "linux"))]
                {
                    next_deadline += frame_duration * burst_size as u32;
                    let now = std::time::Instant::now();
                    if now > next_deadline + frame_duration * burst_size as u32 {
                        tracing::warn!(
                            "Sender thread fell behind by {:.2}ms, resetting deadline",
                            (now - next_deadline).as_secs_f64() * 1000.0
                        );
                        next_deadline = now;
                    }
                }
            }
        }
    }

    tracing::info!("Sender thread exiting");
}

struct StreamerInner {
    state: StreamerState,
    config: StreamConfig,
    buffer: AudioBuffer,
    live_options: crate::LiveStreamOptions,
    metrics: crate::live_policy::SharedDiagnostics,
    rtp_senders: Vec<RtpSender>,
    current_timestamp: u64,
    last_sync_rtp: u32,
    clock: Clock,
    clock_offset: Option<ClockOffset>,
    timing_rx: Option<watch::Receiver<ClockOffset>>,
    decoder: Option<AudioDecoder>,
    /// Live audio decoder for streaming from external sources (e.g., Bluetooth).
    live_decoder: Option<LiveAudioDecoder>,
    encoder: Option<Box<dyn AudioEncoder>>,
    /// Audio equalizer for processing audio before encoding.
    equalizer: Option<Equalizer>,
    /// Track whether first audio packet has been sent (requires marker bit)
    first_packet_sent: bool,
    /// Render delay in nanoseconds added to NTP timestamps in sync packets.
    /// Tells the receiver to render audio this far in the future, giving more
    /// time for retransmit recovery of lost packets.
    render_delay_ns: u64,
    /// Whether to use PTP-mode sync packets (PT=87) instead of NTP (PT=84).
    use_ptp_sync: bool,
    /// PTP master clock identity (from BMCA, used in PT=87 packets).
    ptp_master_clock_id: [u8; 8],
    live_timeline: crate::live_timeline::LiveTimeline,
}

/// High-level audio streamer.
pub struct AudioStreamer {
    inner: Arc<Mutex<StreamerInner>>,
    task: Option<JoinHandle<Result<()>>>,
    state_cache: Arc<AtomicU8>,
    timestamp_cache: Arc<AtomicU64>,
    /// Total audio packets sent (lock-free, updated by run_streamer).
    packets_sent: Arc<AtomicU64>,
    /// Buffer underruns: times the buffer was empty when a packet was due.
    underruns: Arc<AtomicU64>,
    /// Dedicated sender thread for precise packet timing.
    sender_thread: Option<std::thread::JoinHandle<()>>,
    sender_stop: Arc<std::sync::atomic::AtomicBool>,
    /// Channel to send packets to the sender thread.
    sender_tx: Option<Sender<SenderMessage>>,
}

impl Drop for AudioStreamer {
    fn drop(&mut self) {
        // Only the owning streamer stops the worker; read/control clones have
        // neither a task nor a thread handle.
        if self.task.is_some() || self.sender_thread.is_some() {
            self.sender_stop.store(true, Ordering::Release);
            self.state_cache.store(StreamerState::Stopped as u8, Ordering::Release);
            if let Some(task) = self.task.take() { task.abort(); }
            self.sender_tx = None;
        }
    }
}

impl Clone for AudioStreamer {
    fn clone(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            task: None, // Can't clone JoinHandle, new clone doesn't own the task
            state_cache: Arc::clone(&self.state_cache),
            timestamp_cache: Arc::clone(&self.timestamp_cache),
            packets_sent: Arc::clone(&self.packets_sent),
            underruns: Arc::clone(&self.underruns),
            sender_stop: self.sender_stop.clone(),
            sender_thread: None, // Can't clone JoinHandle
            sender_tx: self.sender_tx.clone(),
        }
    }
}

impl AudioStreamer {
    /// Create new streamer.
    pub fn new(config: StreamConfig) -> Self {
        let audio_format = config.audio_format.clone();
        Self {
            inner: Arc::new(Mutex::new(StreamerInner {
                state: StreamerState::Idle,
                config,
                buffer: AudioBuffer::new(audio_format, 2000),
                live_options: crate::LiveStreamOptions::default(),
                metrics: Arc::new(crate::LiveDiagnostics::default()),
                rtp_senders: Vec::new(),
                current_timestamp: 0,
                last_sync_rtp: 0,
                clock: Clock::new(audio_format.sample_rate.as_hz()),
                clock_offset: None,
                timing_rx: None,
                decoder: None,
                live_decoder: None,
                encoder: None,
                equalizer: None,
                first_packet_sent: false,
                render_delay_ns: 0,
                use_ptp_sync: false,
                ptp_master_clock_id: [0u8; 8],
                live_timeline: Default::default(),
            })),
            task: None,
            state_cache: Arc::new(AtomicU8::new(StreamerState::Idle as u8)),
            timestamp_cache: Arc::new(AtomicU64::new(0)),
            packets_sent: Arc::new(AtomicU64::new(0)),
            underruns: Arc::new(AtomicU64::new(0)),
            sender_thread: None,
            sender_stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            sender_tx: None,
        }
    }

    /// Configure RTP sender (single device, backward compatible).
    pub async fn set_rtp_sender(&mut self, sender: RtpSender) {
        self.inner.lock().await.rtp_senders = vec![sender];
    }

    /// Configure multiple RTP senders for group streaming.
    ///
    /// Each sender targets a different device with its own encryption key.
    /// All senders share the same RTP sequence/timestamp space.
    pub async fn set_rtp_senders(&mut self, senders: Vec<RtpSender>) {
        self.inner.lock().await.rtp_senders = senders;
    }

    /// Enable PTP-mode sync packets (PT=87) instead of NTP sync (PT=84).
    ///
    /// When enabled, sync packets include PTP clock timestamps and the master
    /// clock identity instead of NTP epoch timestamps.
    pub async fn set_ptp_sync_mode(&mut self, clock_id: [u8; 8]) {
        let mut inner = self.inner.lock().await;
        inner.use_ptp_sync = true;
        inner.ptp_master_clock_id = clock_id;
        tracing::info!("PTP sync mode enabled, master clock ID: {:02x?}", clock_id);
    }

    /// Set render delay in milliseconds.
    ///
    /// This shifts NTP timestamps in sync packets into the future, telling
    /// the receiver to buffer audio longer before rendering. This gives more
    /// time for retransmit recovery of lost packets.
    pub async fn set_render_delay_ms(&mut self, delay_ms: u32) {
        let delay_ns = delay_ms as u64 * 1_000_000;
        self.inner.lock().await.render_delay_ns = delay_ns;
        tracing::info!("Render delay set to {}ms ({}ns)", delay_ms, delay_ns);
    }

    /// Set up the equalizer with shared parameters.
    ///
    /// The EQ will be initialized with the given configuration and parameters.
    /// The parameters can be updated atomically from another thread (e.g., the UI).
    pub async fn set_eq_params(&mut self, config: EqConfig, params: Arc<EqParams>) {
        let mut inner = self.inner.lock().await;
        let sample_rate = inner.config.audio_format.sample_rate.as_hz();
        inner.equalizer = Some(Equalizer::new(config, params, sample_rate));
        tracing::info!("Equalizer enabled with {} bands", inner.equalizer.as_ref().unwrap().config().num_bands());
    }

    /// Get a clone of the EQ params if the equalizer is set up.
    pub async fn eq_params(&self) -> Option<Arc<EqParams>> {
        let inner = self.inner.lock().await;
        inner.equalizer.as_ref().map(|eq| Arc::clone(eq.params()))
    }

    /// Handle retransmit request from control channel (delegates to first sender).
    pub async fn handle_retransmit(&self, request: &crate::rtp::RetransmitRequest) -> airplay_core::error::Result<u16> {
        self.handle_retransmit_for_target(0, request).await
    }

    /// Handle retransmit request for a specific target device.
    ///
    /// Each RtpSender has its own packet history ring buffer, so retransmit
    /// responses are per-device (encrypted with the correct device key).
    pub async fn handle_retransmit_for_target(&self, index: usize, request: &crate::rtp::RetransmitRequest) -> airplay_core::error::Result<u16> {
        let inner = self.inner.lock().await;
        if let Some(sender) = inner.rtp_senders.get(index) {
            sender.handle_retransmit(request)
        } else {
            Ok(0)
        }
    }

    /// Get the total number of audio packets sent.
    pub fn packets_sent(&self) -> u64 {
        self.packets_sent.load(Ordering::Relaxed)
    }

    /// Get the count of buffer underruns (buffer empty when a packet was due).
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    /// Get current state.
    /// Allows live retransmit workers to stop polling when their owner exits.
    pub fn is_finished(&self) -> bool {
        matches!(self.state(), StreamerState::Stopped | StreamerState::Error)
    }

    pub fn state(&self) -> StreamerState {
        match self.state_cache.load(Ordering::Relaxed) {
            0 => StreamerState::Idle,
            1 => StreamerState::Buffering,
            2 => StreamerState::Streaming,
            3 => StreamerState::Paused,
            4 => StreamerState::Stopped,
            _ => StreamerState::Error,
        }
    }

    /// Start streaming from decoder.
    pub async fn start(&mut self, decoder: AudioDecoder) -> Result<()> {
        let frame_duration_ns;
        {
            let mut inner = self.inner.lock().await;
            inner.decoder = Some(decoder);
            inner.encoder = Some(create_encoder(inner.config.audio_format.clone())?);
            inner.state = StreamerState::Buffering;
            frame_duration_ns = inner.config.audio_format.frames_per_packet as u64
                * 1_000_000_000u64
                / inner.config.audio_format.sample_rate.as_hz() as u64;
        }
        self.state_cache.store(StreamerState::Buffering as u8, Ordering::Relaxed);

        // Prime the buffer
        self.decode_some().await?;
        let level = self.buffer_level().await;
        if level > 10.0 {
            self.inner.lock().await.state = StreamerState::Streaming;
            self.state_cache.store(StreamerState::Streaming as u8, Ordering::Relaxed);
        }

        if self.task.is_none() {
            // Set up the dedicated sender thread with cloned sockets
            let (tx, rx) = bounded::<SenderMessage>(8);
            let frame_duration = std::time::Duration::from_nanos(frame_duration_ns);

            {
                let inner = self.inner.lock().await;
                let mut targets = Vec::new();
                for sender in &inner.rtp_senders {
                    if let Ok(Some((data_sock, data_dest))) = sender.clone_data_socket() {
                        let ctrl = sender.clone_control_socket().ok().flatten();
                        let (ctrl_sock, ctrl_dest) = match ctrl {
                            Some((s, d)) => (Some(s), Some(d)),
                            None => (None, None),
                        };
                        targets.push(SendTarget {
                            data_socket: data_sock,
                            data_dest,
                            control_socket: ctrl_sock,
                            control_dest: ctrl_dest,
                        });
                    }
                }

                if !targets.is_empty() {
                    // Burst size for WiFi/BT coexistence: send N packets rapidly, then wait.
                    // Creates gaps for Bluetooth to transmit cleanly.
                    // - burst_size=1: no bursting (original behavior)
                    // - burst_size=2: 16ms gaps
                    // - burst_size=4: 32ms gaps (may cause burst packet loss)
                    let burst_size = 1;

                    let metrics = inner.metrics.clone();
                    let sender_stop = self.sender_stop.clone();
                    sender_stop.store(false, Ordering::Release);
                    let thread = std::thread::Builder::new()
                        .name("rt-sender".into())
                        .spawn(move || {
                            sender_thread_main(
                                rx,
                                targets,
                                frame_duration,
                                burst_size,
                                metrics,
                                sender_stop,
                            );
                        })
                        .expect("Failed to spawn sender thread");
                    self.sender_thread = Some(thread);
                    self.sender_tx = Some(tx);
                } else {
                    tracing::warn!("No RTP senders have sockets, falling back to async timing");
                }
            }

            let inner = self.inner.clone();
            let state_cache = self.state_cache.clone();
            let timestamp_cache = self.timestamp_cache.clone();
            let packets_sent = self.packets_sent.clone();
            let underruns = self.underruns.clone();
            let sender_tx = self.sender_tx.clone();
            self.task = Some(tokio::spawn(async move {
                match run_streamer(inner.clone(), state_cache.clone(), timestamp_cache, packets_sent, underruns, sender_tx).await {
                    Ok(()) => tracing::debug!("Streaming task completed normally"),
                    Err(e) => {
                        tracing::error!("Streaming task error: {}", e);
                        state_cache.store(StreamerState::Error as u8, Ordering::Relaxed);
                        if let Ok(mut guard) = inner.try_lock() {
                            guard.state = StreamerState::Error;
                        }
                    }
                }
                Ok(())
            }));
        }

        Ok(())
    }

    /// Start streaming from live audio source (e.g., Bluetooth capture).
    ///
    /// This is similar to `start()` but uses a `LiveAudioDecoder` that receives
    /// PCM frames from a channel, enabling streaming from external sources like
    /// Bluetooth audio capture.
    pub async fn start_live(&mut self, live_decoder: LiveAudioDecoder) -> Result<()> {
        self.start_live_with_options(live_decoder, crate::LiveStreamOptions::default()).await
    }

    pub async fn start_live_with_options(&mut self, live_decoder: LiveAudioDecoder, options: crate::LiveStreamOptions) -> Result<()> {
        options.validate()?;
        let frame_duration_ns;
        {
            let mut inner = self.inner.lock().await;
            inner.metrics = live_decoder.metrics();
            inner.live_options = options;
            inner.buffer = AudioBuffer::new(inner.config.audio_format, options.buffer_ms);
            inner.live_decoder = Some(live_decoder);
            inner.decoder = None; // Clear file decoder if any
            inner.encoder = Some(create_encoder(inner.config.audio_format.clone())?);
            inner.state = StreamerState::Buffering;
            frame_duration_ns = inner.config.audio_format.frames_per_packet as u64
                * 1_000_000_000u64
                / inner.config.audio_format.sample_rate.as_hz() as u64;
        }
        self.state_cache.store(StreamerState::Buffering as u8, Ordering::Relaxed);

        // For live streaming, wait for initial buffer fill before streaming.
        // This prevents startup artifacts from sending packets before we have
        // enough audio data buffered. The local policy controls pre-roll;
        // callers must feed the decoder while awaiting this method.
        tracing::info!("Live streaming: waiting for initial buffer fill...");
        let buffer_start = std::time::Instant::now();
        let max_wait = options.startup_timeout;

        loop {
            // Try to decode some frames into the buffer
            {
                let mut guard = self.inner.lock().await;
                decode_some_inner(&mut guard)?;
                let fill_pct = guard.buffer.fill_percentage();
                let frame_count = guard.buffer.len();

                if guard.buffer.buffered_ms() >= options.startup_ms {
                    tracing::info!(
                        "Live streaming: buffer ready at {:.1}% ({} frames), starting playback",
                        fill_pct, frame_count
                    );
                    guard.state = StreamerState::Streaming;
                    self.state_cache.store(StreamerState::Streaming as u8, Ordering::Relaxed);
                    break;
                }

                if buffer_start.elapsed() > max_wait {
                    tracing::warn!(
                        "Live streaming: buffer timeout at {:.1}% ({} frames), starting anyway",
                        fill_pct, frame_count
                    );
                    guard.state = StreamerState::Streaming;
                    self.state_cache.store(StreamerState::Streaming as u8, Ordering::Relaxed);
                    break;
                }

                if buffer_start.elapsed().as_millis() % 500 == 0 {
                    tracing::debug!(
                        "Live streaming: buffering {:.1}% ({} frames)...",
                        fill_pct, frame_count
                    );
                }
            }
            // Small delay before retry
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }

        if self.task.is_none() {
            // Set up the dedicated sender thread with cloned sockets
            let (tx, rx) = bounded::<SenderMessage>(options.sender_capacity);
            let frame_duration = std::time::Duration::from_nanos(frame_duration_ns);

            {
                let inner = self.inner.lock().await;
                let mut targets = Vec::new();
                for sender in &inner.rtp_senders {
                    if let Ok(Some((data_sock, data_dest))) = sender.clone_data_socket() {
                        let ctrl = sender.clone_control_socket().ok().flatten();
                        let (ctrl_sock, ctrl_dest) = match ctrl {
                            Some((s, d)) => (Some(s), Some(d)),
                            None => (None, None),
                        };
                        targets.push(SendTarget {
                            data_socket: data_sock,
                            data_dest,
                            control_socket: ctrl_sock,
                            control_dest: ctrl_dest,
                        });
                    }
                }

                if !targets.is_empty() {
                    let burst_size = 1;

                    let metrics = inner.metrics.clone();
                    let sender_stop = self.sender_stop.clone();
                    sender_stop.store(false, Ordering::Release);
                    let thread = std::thread::Builder::new()
                        .name("rt-sender".into())
                        .spawn(move || {
                            sender_thread_main(
                                rx,
                                targets,
                                frame_duration,
                                burst_size,
                                metrics,
                                sender_stop,
                            );
                        })
                        .expect("Failed to spawn sender thread");
                    self.sender_thread = Some(thread);
                    self.sender_tx = Some(tx);
                } else {
                    tracing::warn!("No RTP senders have sockets, falling back to async timing");
                }
            }

            let inner = self.inner.clone();
            let state_cache = self.state_cache.clone();
            let timestamp_cache = self.timestamp_cache.clone();
            let packets_sent = self.packets_sent.clone();
            let underruns = self.underruns.clone();
            let sender_tx = self.sender_tx.clone();
            self.task = Some(tokio::spawn(async move {
                match run_streamer(inner.clone(), state_cache.clone(), timestamp_cache, packets_sent, underruns, sender_tx).await {
                    Ok(()) => tracing::debug!("Live streaming task completed normally"),
                    Err(e) => {
                        tracing::error!("Live streaming task error: {}", e);
                        state_cache.store(StreamerState::Error as u8, Ordering::Relaxed);
                        if let Ok(mut guard) = inner.try_lock() {
                            guard.state = StreamerState::Error;
                        }
                    }
                }
                Ok(())
            }));
        }

        Ok(())
    }

    /// Pause streaming.
    pub async fn pause(&mut self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        if inner.state == StreamerState::Streaming {
            if let Some(ref tx) = self.sender_tx {
                let _ = tx.try_send(SenderMessage::Pause);
            }
            inner.state = StreamerState::Paused;
            self.state_cache.store(StreamerState::Paused as u8, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Resume streaming.
    pub async fn resume(&mut self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        if inner.state == StreamerState::Paused {
            if let Some(ref tx) = self.sender_tx {
                let _ = tx.try_send(SenderMessage::Resume);
            }
            inner.state = StreamerState::Streaming;
            self.state_cache.store(StreamerState::Streaming as u8, Ordering::Relaxed);
        }
        Ok(())
    }

    /// Reset state after FLUSH so the next audio/sync packets have correct
    /// marker and extension bits set, as required by the AirPlay spec.
    pub async fn reset_after_flush(&mut self) {
        let mut inner = self.inner.lock().await;
        inner.first_packet_sent = false;
        inner.live_timeline.reset();
        for sender in &mut inner.rtp_senders {
            sender.reset_sync_state();
        }
    }

    /// Stop streaming.
    pub async fn stop(&mut self) -> Result<()> {
        // Retransmit/control clones may retain tx. Shutdown must not rely on
        // channel disconnection or delivery of a Stop message to a full queue.
        self.sender_stop.store(true, Ordering::Release);
        // Set state to Stopped FIRST so run_streamer breaks out of its loop
        // and stops producing into the bounded channel.
        self.state_cache.store(StreamerState::Stopped as u8, Ordering::Relaxed);

        // Cancel the producer before joining the sender. It owns a channel
        // clone, so dropping only self.sender_tx cannot disconnect a full queue.
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }

        // Signal sender thread to stop
        if let Some(ref tx) = self.sender_tx {
            let _ = tx.try_send(SenderMessage::Stop);
        }
        // Drop sender_tx to disconnect the channel — guarantees the sender
        // thread's rx.recv() returns Err and exits, even if the Stop message
        // couldn't be delivered (channel was full).
        self.sender_tx = None;

        // Now safe to join — sender thread will exit from channel disconnect.
        if let Some(handle) = self.sender_thread.take() {
            let _ = tokio::task::spawn_blocking(move || {
                let _ = handle.join();
            }).await;
        }

        let mut inner = self.inner.lock().await;
        inner.state = StreamerState::Stopped;
        inner.buffer.flush();
        inner.metrics.encoder_buffer_ms.store(0, Ordering::Relaxed);
        inner.decoder = None;
        inner.live_decoder = None;
        Ok(())
    }

    /// Seek to position in samples.
    pub async fn seek(&mut self, position_samples: u64) -> Result<()> {
        let mut inner = self.inner.lock().await;
        inner.buffer.flush();
        if let Some(ref mut decoder) = inner.decoder {
            decoder.seek(position_samples)?;
        }
        // Reset EQ filter state to avoid artifacts from previous audio
        if let Some(ref mut eq) = inner.equalizer {
            eq.reset();
        }
        inner.current_timestamp = position_samples;
        self.timestamp_cache.store(position_samples, Ordering::Relaxed);
        Ok(())
    }

    /// Get current playback position in samples.
    pub fn position(&self) -> u64 {
        self.timestamp_cache.load(Ordering::Relaxed)
    }

    /// Get buffer fill level percentage.
    pub async fn buffer_level(&self) -> f32 {
        self.inner.lock().await.buffer.fill_percentage()
    }

    /// Set volume (0.0 to 1.0).
    pub async fn set_volume(&mut self, _volume: f32) -> Result<()> {
        // Volume is set via RTSP SET_PARAMETER, not in audio stream
        // This is a no-op placeholder
        Ok(())
    }

    /// Set timing offset from sync protocol.
    pub async fn set_timing_offset(&mut self, offset: ClockOffset) {
        self.inner.lock().await.clock_offset = Some(offset);
    }

    /// Subscribe to timing updates.
    pub async fn set_timing_updates(&mut self, rx: watch::Receiver<ClockOffset>) {
        self.inner.lock().await.timing_rx = Some(rx);
    }

    /// Internal: decode some audio into buffer.
    async fn decode_some(&mut self) -> Result<()> {
        let mut inner = self.inner.lock().await;
        decode_some_inner(&mut inner)?;
        Ok(())
    }
}

fn decode_some_inner(inner: &mut StreamerInner) -> Result<()> {
    let format = inner.config.audio_format.clone();
    let frames_per_packet = format.frames_per_packet as usize;
    let is_live = inner.live_decoder.is_some();

    // Decode 3 frames per batch to minimize blocking in send loop.
    // Very small batches ensure minimal interference with precise timing.
    // With 2ms timeout per frame, worst case is ~6ms blocking.
    for _ in 0..3 {
        if inner.buffer.is_full() { break; }
        // Try live decoder first (for Bluetooth/external sources), then file decoder
        let frame = if let Some(ref mut live_decoder) = inner.live_decoder {
            live_decoder.decode_resampled(&format, frames_per_packet)?
        } else if let Some(ref mut decoder) = inner.decoder {
            decoder.decode_resampled(&format, frames_per_packet)?
        } else {
            break;
        };

        if let Some(frame) = frame {
            let mut audio_frame = crate::AudioFrame::new(frame.samples, frame.timestamp);
            audio_frame.captured_at = inner.live_decoder.as_ref().and_then(|d| d.packet_captured_at());
            if crate::live_policy::expired(audio_frame.captured_at, inner.live_options.max_age) {
                inner.metrics.stale_drops.fetch_add(1, Ordering::Relaxed);
                continue;
            }
            inner
                .buffer
                .push(audio_frame)
                .map_err(|_| airplay_core::error::StreamingError::BufferOverflow)?;
        } else if is_live {
            // For live streams, None means timeout (no data yet), not EOF.
            // Don't break - just return and try again later.
            // This prevents the streamer from stopping on temporary data gaps.
            break;
        } else {
            // For file decoders, None means EOF
            break;
        }
    }
    Ok(())
}

async fn run_streamer(
    inner: Arc<Mutex<StreamerInner>>,
    state_cache: Arc<AtomicU8>,
    timestamp_cache: Arc<AtomicU64>,
    packets_sent_counter: Arc<AtomicU64>,
    underrun_counter: Arc<AtomicU64>,
    sender_tx: Option<Sender<SenderMessage>>,
) -> Result<()> {
    // Compute frame duration once (constant for the session)
    let frame_duration_ns = {
        let guard = inner.lock().await;
        guard.config.audio_format.frames_per_packet as u64 * 1_000_000_000u64
            / guard.config.audio_format.sample_rate.as_hz() as u64
    };
    let frame_duration = Duration::from_nanos(frame_duration_ns);

    let has_sender_thread = sender_tx.is_some();

    // Only set RT priority if we're NOT using the dedicated sender thread
    // (the sender thread sets its own RT priority)
    if !has_sender_thread {
        #[cfg(not(windows))]
        set_realtime_priority();
        #[cfg(windows)]
        tracing::warn!("Windows: no dedicated sender thread; MMCSS cannot be retained safely");
    }

    // Use absolute deadline scheduling so processing time doesn't cause drift
    let mut next_deadline = Instant::now();

    loop {
        {
            let state = inner.lock().await.state;
            if state == StreamerState::Stopped || state == StreamerState::Error {
                break;
            }
            if state == StreamerState::Paused {
                sleep(Duration::from_millis(10)).await;
                // Reset deadline after pause so we don't burst-send
                next_deadline = Instant::now();
                continue;
            }
        }

        {
            let mut guard = inner.lock().await;
            if let Some(rx) = guard.timing_rx.as_mut() {
                if rx.has_changed().unwrap_or(false) {
                    let latest = *rx.borrow_and_update();
                    guard.clock_offset = Some(latest);
                }
            }
            // Preserve file buffering; live input uses an independent local
            // policy so receiver latency never silently configures these queues.
            let refill = if guard.live_decoder.is_some() {
                guard.buffer.buffered_ms() < guard.live_options.refill_ms
            } else { guard.buffer.fill_percentage() < 40.0 };
            if refill {
                let decode_start = Instant::now();
                decode_some_inner(&mut guard)?;
                let decode_elapsed = decode_start.elapsed();
                if decode_elapsed.as_millis() > 10 {
                    tracing::warn!(
                        "Decode took {:.2}ms (blocking send loop!)",
                        decode_elapsed.as_secs_f64() * 1000.0
                    );
                }
            }

            // Check for recovery from Buffering state
            if guard.state == StreamerState::Buffering {
                let ready = if guard.live_decoder.is_some() {
                    guard.buffer.buffered_ms() >= guard.live_options.resume_ms
                } else { guard.buffer.fill_percentage() > 10.0 };
                if ready {
                    guard.state = StreamerState::Streaming;
                    state_cache.store(StreamerState::Streaming as u8, Ordering::Relaxed);
                } else {
                    // Check if decoder(s) are exhausted
                    let decoder_eof = guard.decoder.as_ref().map_or(true, |d| d.is_eof());
                    let live_decoder_eof = guard.live_decoder.as_ref().map_or(true, |d| d.is_eof());
                    let has_any_decoder = guard.decoder.is_some() || guard.live_decoder.is_some();

                    // Only stop if we have a decoder and it's exhausted with empty buffer
                    // For live decoders, we keep waiting unless explicitly marked EOF
                    if has_any_decoder && decoder_eof && live_decoder_eof && guard.buffer.is_empty() {
                        // Decoder exhausted and buffer empty - playback complete
                        tracing::info!("Decoder EOF and buffer empty - stopping");
                        guard.state = StreamerState::Stopped;
                        state_cache.store(StreamerState::Stopped as u8, Ordering::Relaxed);
                        break;
                    } else {
                        // Still buffering, wait and retry
                        drop(guard);
                        sleep(Duration::from_millis(10)).await;
                        // Reset deadline after buffering stall
                        next_deadline = Instant::now();
                        continue;
                    }
                }
            }

            while guard.buffer.peek().is_some_and(|f| crate::live_policy::expired(f.captured_at, guard.live_options.max_age)) {
                guard.buffer.pop();
                guard.metrics.stale_drops.fetch_add(1, Ordering::Relaxed);
                guard.metrics.force_sync.store(true, Ordering::Release);
            }
            guard.metrics.encoder_buffer_ms.store(guard.buffer.buffered_ms() as u64, Ordering::Relaxed);
            let frame = guard.buffer.pop();
            if let Some(frame) = frame {
                // Diagnostic: log PCM sample energy for first few frames
                static DIAG_COUNT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
                let diag = DIAG_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if diag < 5 || diag % 500 == 0 {
                    let rms: f64 = if frame.samples.is_empty() {
                        0.0
                    } else {
                        let sum: f64 = frame.samples.iter().map(|&s| (s as f64).powi(2)).sum();
                        (sum / frame.samples.len() as f64).sqrt()
                    };
                    let max_abs = frame.samples.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
                    tracing::info!(
                        "DIAG PCM frame #{}: samples={}, rms={:.1}, max_abs={}, first_4={:?}",
                        diag, frame.samples.len(), rms, max_abs,
                        &frame.samples[..frame.samples.len().min(4)]
                    );
                }

                // Process through equalizer if enabled
                let samples_to_encode: Vec<i16> = if let Some(ref mut eq) = guard.equalizer {
                    let mut samples = (*frame.samples).clone();
                    eq.process(&mut samples);
                    samples
                } else {
                    (*frame.samples).clone()
                };

                // Encode synchronously
                let encode_start = Instant::now();
                let encoder = guard
                    .encoder
                    .as_mut()
                    .ok_or_else(|| airplay_core::error::StreamingError::Encoding("Encoder missing".into()))?;
                let packet = encoder.encode(&samples_to_encode)?;
                let encode_elapsed = encode_start.elapsed();

                // Diagnostic: log encoded ALAC data and timing for first few packets
                if diag < 5 || diag % 500 == 0 {
                    let all_zero = packet.data.iter().all(|&b| b == 0);
                    tracing::info!(
                        "DIAG ALAC packet #{}: encoded_len={}, all_zero={}, first_8={:02x?}, encode_time={:.2}ms",
                        diag, packet.data.len(), all_zero,
                        &packet.data[..packet.data.len().min(8)],
                        encode_elapsed.as_secs_f64() * 1000.0
                    );
                }

                let payload_type = guard.config.stream_type as u8;
                let sample_rate = guard.config.audio_format.sample_rate.as_hz();
                let last_sync_rtp = guard.last_sync_rtp;

                // Get current time and apply clock offset from PTP/NTP sync
                let local_wall = guard.clock.now_wall_ns();
                let adjusted = if let Some(offset) = guard.clock_offset {
                    let result = guard.clock.apply_offset(local_wall, &offset);
                    if diag < 3 {
                        tracing::info!(
                            "CLOCK OFFSET: offset_ns={}, local_wall={}, adjusted={}, diff={}",
                            offset.offset_ns,
                            local_wall,
                            result,
                            result as i128 - local_wall as i128
                        );
                    }
                    result
                } else {
                    if diag < 3 {
                        tracing::warn!("CLOCK OFFSET: None - using local time directly (may cause timing issues!)");
                    }
                    local_wall
                };

                // Apply render delay: shift NTP timestamp into the future so the
                // receiver buffers audio longer before rendering, giving more
                // time for retransmit recovery of lost packets.
                let render_adjusted = adjusted + guard.render_delay_ns;
                let ntp = unix_to_ntp(render_adjusted);

                if guard.live_options.max_age.is_some() && guard.metrics.packets_sent.load(Ordering::Relaxed) == 0
                    && guard.metrics.force_sync.load(Ordering::Acquire) {
                    guard.first_packet_sent = false;
                    guard.live_timeline.reset();
                }
                // Set marker bit on first audio packet (required by some receivers)
                let first_packet = !guard.first_packet_sent;
                let marker = first_packet;
                if marker {
                    tracing::info!("Sending first audio packet with marker bit set");
                }

                let rtp_ts = if guard.live_options.max_age.is_some() {
                    // Decoder timestamps only count received samples, so capture
                    // gaps (underruns, sender deadline resets) would leave the RTP
                    // timeline permanently behind real time and the receiver drops
                    // every packet as late once the lag exceeds its buffer. Skip
                    // the lost time instead; the timestamp jump forces a new sync.
                    let (timestamp, skipped) = guard.live_timeline.timestamp(frame.timestamp, Instant::now(), sample_rate);
                    if skipped > 0 {
                        tracing::info!(skipped_ms=skipped as f64 * 1000.0 / sample_rate as f64, "Live RTP timeline caught up after capture gap");
                    }
                    timestamp
                } else {
                    packet.timestamp as u32
                };

                // Determine if sync is needed BEFORE borrowing rtp_sender
                let need_sync = guard.metrics.force_sync.swap(false, Ordering::AcqRel)
                    || (guard.live_options.max_age.is_some() && guard.current_timestamp as u32 != rtp_ts)
                    || first_packet || last_sync_rtp == 0
                    || rtp_ts.wrapping_sub(last_sync_rtp) >= sample_rate;

                // Extract PTP sync mode state before borrowing rtp_sender
                let use_ptp_sync = guard.use_ptp_sync;
                let ptp_clock_id = guard.ptp_master_clock_id;

                if !guard.rtp_senders.is_empty() {
                    if let Some(ref tx) = sender_tx {
                        // Sender thread path: prepare sync from first sender (shared),
                        // then prepare audio from ALL senders (per-device encryption).
                        let sync_data = if need_sync {
                            if use_ptp_sync {
                                let next_rtp_ts = rtp_ts.wrapping_add(sample_rate / 44100 * 352);
                                guard.rtp_senders[0].prepare_ptp_sync(rtp_ts, render_adjusted, next_rtp_ts, &ptp_clock_id)?
                            } else {
                                guard.rtp_senders[0].prepare_sync(rtp_ts, ntp)?
                            }
                        } else {
                            None
                        };

                        let mut wire_packets = Vec::with_capacity(guard.rtp_senders.len());
                        for sender in &mut guard.rtp_senders {
                            wire_packets.push(sender.prepare_audio(payload_type, rtp_ts, &packet.data, marker)?);
                        }

                        if diag < 5 || diag % 500 == 0 {
                            tracing::info!(
                                "DIAG timing #{}: encode={:.2}ms, targets={} (sender thread handles send timing)",
                                diag,
                                encode_elapsed.as_secs_f64() * 1000.0,
                                wire_packets.len(),
                            );
                        }

                        // Update state BEFORE sending to channel
                        if need_sync {
                            guard.last_sync_rtp = rtp_ts;
                        }
                        if first_packet {
                            guard.first_packet_sent = true;
                        }
                        guard.current_timestamp = rtp_ts as u64 + packet.samples as u64;
                        timestamp_cache.store(guard.current_timestamp, Ordering::Relaxed);
                        packets_sent_counter.fetch_add(1, Ordering::Relaxed);

                        // Drop the mutex guard first so other async tasks can proceed
                        let captured_at = frame.captured_at;
                        let max_age = guard.live_options.max_age;
                        drop(guard);
                        let tx_clone = tx.clone();
                        let msg = SenderMessage::Packet { wire_packets, sync_data, captured_at, max_age };
                        let send_result = tokio::task::spawn_blocking(move || {
                            tx_clone.send(msg)
                        }).await;
                        match send_result {
                            Ok(Ok(())) => {},
                            _ => {
                                tracing::error!("Sender thread disconnected");
                                state_cache.store(StreamerState::Error as u8, Ordering::Relaxed);
                                break;
                            }
                        }
                        continue;
                    } else {
                        // Fallback: direct send (no sender thread)
                        // Send sync from first sender, audio from all senders
                        if need_sync {
                            if use_ptp_sync {
                                let next_rtp_ts = rtp_ts.wrapping_add(sample_rate / 44100 * 352);
                                guard.rtp_senders[0].send_ptp_sync(rtp_ts, render_adjusted, next_rtp_ts, &ptp_clock_id)?;
                            } else {
                                guard.rtp_senders[0].send_sync(rtp_ts, ntp)?;
                            }
                        }

                        let send_start = Instant::now();
                        for sender in &mut guard.rtp_senders {
                            sender.send_audio(payload_type, rtp_ts, &packet.data, marker)?;
                        }
                        let send_elapsed = send_start.elapsed();

                        if diag < 5 || diag % 500 == 0 {
                            tracing::info!(
                                "DIAG timing #{}: encode={:.2}ms, send={:.2}ms, targets={}, total={:.2}ms",
                                diag,
                                encode_elapsed.as_secs_f64() * 1000.0,
                                send_elapsed.as_secs_f64() * 1000.0,
                                guard.rtp_senders.len(),
                                (encode_elapsed + send_elapsed).as_secs_f64() * 1000.0
                            );
                        }

                        if need_sync {
                            guard.last_sync_rtp = rtp_ts;
                        }
                        if first_packet {
                            guard.first_packet_sent = true;
                        }
                        guard.current_timestamp = rtp_ts as u64 + packet.samples as u64;
                        timestamp_cache.store(guard.current_timestamp, Ordering::Relaxed);
                        packets_sent_counter.fetch_add(1, Ordering::Relaxed);
                    }
                }
            } else {
                let count = underrun_counter.fetch_add(1, Ordering::Relaxed) + 1;
                guard.metrics.underruns.fetch_add(1, Ordering::Relaxed);
                if count <= 5 || count % 50 == 0 {
                    tracing::warn!("Buffer underrun #{} (buffer empty, packet skipped)", count);
                }
                guard.state = StreamerState::Buffering;
                state_cache.store(StreamerState::Buffering as u8, Ordering::Relaxed);
            }
        }

        if has_sender_thread {
            // With sender thread: no sleep needed here. The bounded channel
            // provides natural backpressure - when it's full, the blocking send
            // (via spawn_blocking) throttles us to match the sender thread's
            // consumption rate. This keeps the channel maximally filled so the
            // sender thread never starves.
            //
            // Yield to let other Tokio tasks run (NTP, control, etc.)
            tokio::task::yield_now().await;
        } else {
            // Without sender thread: use deadline-based timing (original behavior)
            next_deadline += frame_duration;
            let now = Instant::now();
            if next_deadline > now {
                tokio::time::sleep(next_deadline - now).await;
            }
        }
    }

    // Signal sender thread to stop when streaming loop exits
    if let Some(ref tx) = sender_tx {
        let _ = tx.try_send(SenderMessage::Stop);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use airplay_core::StreamConfig;

    fn test_config() -> StreamConfig {
        StreamConfig::default()
    }

    mod state_machine {
        use super::*;

        #[test]
        fn starts_idle() {
            let streamer = AudioStreamer::new(test_config());
            assert_eq!(streamer.state(), StreamerState::Idle);
        }

        #[tokio::test]
        async fn start_transitions_to_buffering() {
            // Can't easily test without real audio file
            // Test that we start in Idle
            let streamer = AudioStreamer::new(test_config());
            assert_eq!(streamer.state(), StreamerState::Idle);
        }

        #[tokio::test]
        async fn buffering_transitions_to_streaming() {
            // Would need mock decoder for proper testing
            let streamer = AudioStreamer::new(test_config());
            assert_eq!(streamer.state(), StreamerState::Idle);
        }

        #[tokio::test]
        async fn pause_transitions_to_paused() {
            let mut streamer = AudioStreamer::new(test_config());
            {
                let mut inner = streamer.inner.lock().await;
                inner.state = StreamerState::Streaming;
            }
            streamer.state_cache.store(StreamerState::Streaming as u8, Ordering::Relaxed);
            streamer.pause().await.unwrap();
            assert_eq!(streamer.state(), StreamerState::Paused);
        }

        #[tokio::test]
        async fn resume_transitions_to_streaming() {
            let mut streamer = AudioStreamer::new(test_config());
            {
                let mut inner = streamer.inner.lock().await;
                inner.state = StreamerState::Paused;
            }
            streamer.state_cache.store(StreamerState::Paused as u8, Ordering::Relaxed);
            streamer.resume().await.unwrap();
            assert_eq!(streamer.state(), StreamerState::Streaming);
        }

        #[tokio::test]
        async fn stop_transitions_to_stopped() {
            let mut streamer = AudioStreamer::new(test_config());
            {
                let mut inner = streamer.inner.lock().await;
                inner.state = StreamerState::Streaming;
            }
            streamer.state_cache.store(StreamerState::Streaming as u8, Ordering::Relaxed);
            streamer.stop().await.unwrap();
            assert_eq!(streamer.state(), StreamerState::Stopped);
        }
    }

    mod streaming {
        use super::*;

        #[tokio::test]
        async fn decodes_and_buffers_audio() {
            // Would need mock decoder
            let streamer = AudioStreamer::new(test_config());
            assert_eq!(streamer.buffer_level().await, 0.0);
        }

        #[tokio::test]
        async fn sends_rtp_packets() {
            // Would need mock RTP sender
            let streamer = AudioStreamer::new(test_config());
            assert!(streamer.inner.lock().await.rtp_senders.is_empty());
        }

        #[tokio::test]
        async fn respects_buffer_level() {
            let streamer = AudioStreamer::new(test_config());
            let level = streamer.buffer_level().await;
            assert!(level >= 0.0 && level <= 100.0);
        }

        // End-of-stream handling requires a mock decoder to trigger EOF.
    }

    mod seeking {
        use super::*;

        #[tokio::test]
        async fn seek_clears_buffer() {
            let mut streamer = AudioStreamer::new(test_config());
            streamer.seek(1000).await.unwrap();
            assert_eq!(streamer.buffer_level().await, 0.0);
        }

        #[tokio::test]
        async fn seek_updates_position() {
            let mut streamer = AudioStreamer::new(test_config());
            streamer.seek(44100).await.unwrap();
            assert_eq!(streamer.position(), 44100);
        }

        #[tokio::test]
        async fn seek_while_paused() {
            let mut streamer = AudioStreamer::new(test_config());
            {
                let mut inner = streamer.inner.lock().await;
                inner.state = StreamerState::Paused;
            }
            streamer.state_cache.store(StreamerState::Paused as u8, Ordering::Relaxed);
            streamer.seek(22050).await.unwrap();
            assert_eq!(streamer.position(), 22050);
            assert_eq!(streamer.state(), StreamerState::Paused);
        }
    }

    mod timing {
        use super::*;

        #[tokio::test]
        async fn timestamps_increment_correctly() {
            let streamer = AudioStreamer::new(test_config());
            assert_eq!(streamer.position(), 0);
        }

        #[tokio::test]
        async fn position_tracks_playback() {
            let streamer = AudioStreamer::new(test_config());
            let pos = streamer.position();
            assert_eq!(pos, 0);
        }
    }
}

#[cfg(test)]
mod live_transport_regressions {
    use super::*;
    #[test]
    fn stalled_sender_discards_expired_wire_audio_and_measures_only_sent_packets() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        receiver.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
        let data_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let target = SendTarget { data_socket, data_dest: receiver.local_addr().unwrap(), control_socket: None, control_dest: None };
        let (tx, rx) = bounded(2);
        let metrics = Arc::new(crate::LiveDiagnostics::default());
        let worker_metrics = metrics.clone();
        tx.send(SenderMessage::Packet { wire_packets: vec![b"expired".to_vec()], sync_data: None,
            captured_at: Some(std::time::Instant::now() - Duration::from_secs(1)), max_age: Some(Duration::from_millis(120)) }).unwrap();
        tx.send(SenderMessage::Packet { wire_packets: vec![b"recent".to_vec()], sync_data: None,
            captured_at: Some(std::time::Instant::now()), max_age: Some(Duration::from_millis(120)) }).unwrap();
        let worker = std::thread::spawn(move || sender_thread_main(rx, vec![target], Duration::from_millis(1), 1, worker_metrics, Arc::new(std::sync::atomic::AtomicBool::new(false))));
        let mut data = [0; 64];
        let size = receiver.recv(&mut data).unwrap();
        assert_eq!(&data[..size], b"recent");
        drop(tx);
        worker.join().unwrap();
        let stats = metrics.snapshot(0);
        assert_eq!(stats.stale_drops, 1);
        assert_eq!(stats.packets_sent, 1);
        assert!(stats.capture_to_send_p95_us.unwrap() <= 120_000);
    }

    #[tokio::test]
    async fn stopping_a_live_udp_sender_releases_producer_and_decoder() {
        let receiver = UdpSocket::bind("127.0.0.1:0").unwrap();
        let mut rtp = RtpSender::new(receiver.local_addr().unwrap(), 42);
        rtp.bind(0).unwrap();
        let (sender, decoder) = LiveAudioDecoder::create_pair_with_max_age(44_100, 2, 7, Some(Duration::from_millis(120)));
        for _ in 0..7 { assert!(sender.try_send(crate::LivePcmFrame { samples: vec![1; 704], channels: 2, sample_rate: 44_100 })); }
        let mut streamer = AudioStreamer::new(StreamConfig::default());
        streamer.set_rtp_sender(rtp).await;
        streamer.start_live_with_options(decoder, crate::LiveStreamOptions::low_latency()).await.unwrap();
        for _ in 0..50 { sender.try_send(crate::LivePcmFrame { samples: vec![1; 704], channels: 2, sample_rate: 44_100 }); }
        tokio::time::sleep(Duration::from_millis(10)).await;
        let _control_clone = streamer.clone();
        tokio::time::timeout(Duration::from_secs(1), streamer.stop()).await.unwrap().unwrap();
        assert!(sender.is_closed());
    }
}

#[cfg(test)]
mod group_live_regressions {
    use super::*;
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn group_shares_rtp_timeline_but_uses_distinct_receiver_keys() {
        use crate::cipher::ChaChaPacketCipher;
        use airplay_crypto::chacha::AudioCipher;
        let sockets = [UdpSocket::bind("127.0.0.1:0").unwrap(), UdpSocket::bind("127.0.0.1:0").unwrap()];
        let mut targets = Vec::new();
        for (index, socket) in sockets.iter().enumerate() {
            socket.set_read_timeout(Some(Duration::from_secs(1))).unwrap();
            let mut target = RtpSender::new(socket.local_addr().unwrap(), 0);
            target.bind(0).unwrap();
            target.set_cipher(Box::new(ChaChaPacketCipher::new(AudioCipher::new([index as u8 + 1; 32]))));
            targets.push(target);
        }
        let (producer, decoder) = LiveAudioDecoder::create_pair_with_max_age(44_100, 2, 7, Some(Duration::from_millis(120)));
        for _ in 0..7 { assert!(producer.try_send(crate::LivePcmFrame { samples: vec![1000; 704], channels: 2, sample_rate: 44100 })); }
        let mut streamer = AudioStreamer::new(StreamConfig::default());
        streamer.set_rtp_senders(targets).await;
        streamer.set_ptp_sync_mode([9; 8]).await;
        streamer.start_live_with_options(decoder, crate::LiveStreamOptions::low_latency()).await.unwrap();
        let mut previous_nonce = None;
        for _ in 0..5 {
            let mut a = [0u8; 4096]; let mut b = [0u8; 4096];
            let alen = loop { let len = sockets[0].recv(&mut a).unwrap(); if a[1] & 0x7f == 96 { break len; } };
            let blen = loop { let len = sockets[1].recv(&mut b).unwrap(); if b[1] & 0x7f == 96 { break len; } };
            assert_eq!(&a[..12], &b[..12], "receivers must receive the same RTP sequence and timestamp");
            assert_ne!(&a[12..alen-8], &b[12..blen-8], "each receiver has its own stream key");
            let nonce = a[alen-8..alen].to_vec();
            if let Some(previous) = previous_nonce { assert_ne!(nonce, previous, "successive packets must not reuse a nonce"); }
            previous_nonce = Some(nonce);
        }
        tokio::time::timeout(Duration::from_secs(1), streamer.stop()).await.unwrap().unwrap();
        assert!(producer.is_closed());
    }
}
