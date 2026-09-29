//! WASAPI loopback capture (Windows).
//!
//! Captura el audio del *render device* default usando el flag LOOPBACK.
//! Aprovechamos `AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM` (el flag `convert=true` de
//! la wasapi-rs crate) para que Windows resamplee internamente a 44.1k i16
//! estéreo, evitando tener que meter `rubato` y mantener el pipeline simple.
//!
//! Funcionamiento:
//! 1. Hilo dedicado: inicializa COM (MTA), abre el render device default,
//!    arranca un IAudioClient en modo SHARED con LOOPBACK + AUTOCONVERTPCM
//!    pidiendo 44.1k/16/2ch.
//! 2. Loop event-driven: espera a `h_event`, drena el capture client a un
//!    VecDeque<u8>, y cada vez que hay ≥ CHUNK_FRAMES, parte un Vec<i16>
//!    y lo envía por el `crossbeam_channel<CapturedFrame>`.
//!
//! El handle (`WindowsCapture`) detiene el hilo al dropearse / `.stop()`.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use wasapi::{Direction, SampleType, ShareMode, WaveFormat};

use crate::{Capture, CaptureError, CaptureFormat, CapturedFrame};

/// Frames por chunk entregado aguas arriba. 352 frames @ 44.1k = ~8 ms,
/// exactamente un paquete RTP/ALAC (`frames_per_packet = 352`). Alinear el
/// chunk al tamaño de paquete del encoder hace que cada chunk se mapee 1:1 a un
/// paquete enviado: pacing regular hacia la red en vez de dejar restos (512
/// dejaba 160 frames colgando por chunk, jitter que vacía el buffer del
/// receptor y "robotiza"). Antes 512 (~11 ms) y 1024 (~23 ms) antes aún; cada
/// bajada recorta latencia de captura. Igualado con `READ_CHUNK_FRAMES` del
/// backend `parec` para que el pipeline vea el mismo tamaño en ambos OS.
const CHUNK_FRAMES: usize = 352;

/// Timeout del `wait_for_event` (ms) mientras hay audio real en curso.
const EVENT_TIMEOUT_MS: u32 = 3_000;

/// Cuánto tiempo sin chunks reales toleramos antes de dar el endpoint por
/// inactivo y empezar a inyectar silencio. Por debajo de esto es un hueco
/// normal entre chunks (uno cada ~8 ms); por encima, que no hay nadie
/// reproduciendo audio en el dispositivo por defecto.
const SILENCE_AFTER_IDLE: Duration = Duration::from_millis(100);

/// Intervalo de sondeo (ms) mientras inyectamos silencio. En ese modo el
/// `wait_for_event` no va a llegar —el endpoint está parado y WASAPI no manda
/// eventos—, así que despertamos a menudo para mantener el ritmo de los frames.
const IDLE_POLL_MS: u32 = 8;

/// Como mucho un aviso de "sin eventos" cada este tiempo (si no, el log se
/// llena a razón de uno cada 3 s durante todo un silencio).
const IDLE_WARN_INTERVAL: Duration = Duration::from_secs(10);

const DIAGNOSTICS_INTERVAL: Duration = Duration::from_secs(10);

struct MmcssGuard(*mut std::ffi::c_void);

impl Drop for MmcssGuard {
    fn drop(&mut self) {
        unsafe {
            if AvRevertMmThreadCharacteristics(self.0) == 0 {
                tracing::warn!("WASAPI: no se pudo liberar el registro MMCSS");
            }
        }
    }
}

#[link(name = "avrt")]
extern "system" {
    fn AvSetMmThreadCharacteristicsW(
        task: *const u16,
        task_index: *mut u32,
    ) -> *mut std::ffi::c_void;
    fn AvSetMmThreadPriority(handle: *mut std::ffi::c_void, priority: i32) -> i32;
    fn AvRevertMmThreadCharacteristics(handle: *mut std::ffi::c_void) -> i32;
}

fn register_mmcss() -> Option<MmcssGuard> {
    const AVRT_PRIORITY_CRITICAL: i32 = 2;
    let task: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
    let mut task_index = 0;
    unsafe {
        let handle = AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut task_index);
        if handle.is_null() {
            tracing::warn!("WASAPI: no se pudo registrar el hilo en MMCSS Pro Audio");
            return None;
        }
        if AvSetMmThreadPriority(handle, AVRT_PRIORITY_CRITICAL) == 0 {
            tracing::warn!("WASAPI: no se pudo asignar prioridad MMCSS crítica");
            let _ = AvRevertMmThreadCharacteristics(handle);
            return None;
        }
        tracing::info!(task_index, "WASAPI: hilo registrado en MMCSS Pro Audio");
        Some(MmcssGuard(handle))
    }
}

pub struct WindowsCapture {
    name: String,
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    mute_request: Arc<std::sync::Mutex<Option<std::path::PathBuf>>>,
    mute_state: Arc<AtomicU8>,
}

impl Capture for WindowsCapture {
    fn name(&self) -> &str {
        &self.name
    }
    fn request_local_mute(&self, journal: std::path::PathBuf) -> Result<(), CaptureError> {
        *self.mute_request.lock().unwrap() = Some(journal);
        self.mute_state.store(1, Ordering::SeqCst);
        Ok(())
    }
    fn mute_status(&self) -> &'static str {
        match self.mute_state.load(Ordering::SeqCst) {
            1 => "waiting_signal",
            2 => "checking",
            3 => "muted",
            4 => "unsupported",
            5 => "user_changed",
            _ => "off",
        }
    }
    fn stop(mut self: Box<Self>) {
        self.shutdown();
    }
}

impl WindowsCapture {
    fn shutdown(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for WindowsCapture {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn start(
    fmt: CaptureFormat,
) -> Result<(Box<dyn Capture>, crate::CaptureReceiver), CaptureError> {
    start_with_policy(fmt, crate::CapturePolicy::Stable)
}

pub fn start_with_policy(
    fmt: CaptureFormat,
    policy: crate::CapturePolicy,
) -> Result<(Box<dyn Capture>, crate::CaptureReceiver), CaptureError> {
    start_with_device(fmt, policy, None)
}

pub fn start_with_device(
    fmt: CaptureFormat,
    policy: crate::CapturePolicy,
    device_id: Option<String>,
) -> Result<(Box<dyn Capture>, crate::CaptureReceiver), CaptureError> {
    if fmt.channels != 2 || fmt.sample_rate == 0 {
        return Err(CaptureError::UnsupportedConfig {
            wanted: fmt.sample_rate,
            channels: fmt.channels,
        });
    }

    let target_rate = fmt.sample_rate;
    let target_channels = fmt.channels;

    let (tx, rx) = crate::queue::capture_channel(policy);
    let running = Arc::new(AtomicBool::new(true));
    let running_thread = running.clone();
    let mute_request = Arc::new(std::sync::Mutex::new(None));
    let mute_state = Arc::new(AtomicU8::new(0));
    let thread_mute_request = mute_request.clone();
    let thread_mute_state = mute_state.clone();

    // Canal síncrono para confirmar (o reportar fallo de) la inicialización
    // antes de devolver. Si WASAPI rechaza el formato, queremos enterarnos en
    // `start_loopback()` y no descubrirlo más tarde.
    let (init_tx, init_rx) = std::sync::mpsc::sync_channel::<Result<String, String>>(1);

    let handle = thread::Builder::new()
        .name("audio-capture-wasapi".into())
        .spawn(move || {
            let result = (|| {
                let _com = crate::endpoints::ComGuard::new().map_err(|e| e.to_string())?;
                loop {
                    let restart = capture_thread_main(
                        running_thread.clone(),
                        target_rate,
                        target_channels,
                        &init_tx,
                        tx.clone(),
                        device_id.as_deref(),
                        &thread_mute_request,
                        &thread_mute_state,
                    )?;
                    if !restart || !running_thread.load(Ordering::SeqCst) {
                        break;
                    }
                }
                Ok::<(), String>(())
            })();
            if let Err(e) = result {
                tracing::error!(error = %e, "WASAPI capture thread exit con error");
                // Si init_tx aún no ha sido consumido, asegurar que se envía el error.
                let _ = init_tx.send(Err(e));
            } else {
                tracing::info!("WASAPI capture thread exit limpio");
            }
        })
        .map_err(|e| CaptureError::Backend(format!("spawn capture thread: {e}")))?;

    // Esperamos a la inicialización. Si tarda >5 s, asumimos cuelgue.
    let name = match init_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(name)) => name,
        Ok(Err(e)) => {
            running.store(false, Ordering::SeqCst);
            let _ = handle.join();
            return Err(CaptureError::Backend(e));
        }
        Err(_) => {
            running.store(false, Ordering::SeqCst);
            // A driver call cannot be forcibly cancelled; the worker exits as
            // soon as initialization returns, without starting capture.
            if handle.is_finished() {
                let _ = handle.join();
            }
            return Err(CaptureError::Backend(
                "WASAPI no completó init en 5 s".into(),
            ));
        }
    };

    Ok((
        Box::new(WindowsCapture {
            name,
            running,
            handle: Some(handle),
            mute_request,
            mute_state,
        }),
        rx,
    ))
}

/// Cuerpo del hilo de captura. Devuelve Err en caso de fallo de WASAPI;
/// el caller se encarga de propagarlo por `init_tx` si la falla es temprana.
fn capture_thread_main(
    running: Arc<AtomicBool>,
    target_rate: u32,
    target_channels: u16,
    init_tx: &std::sync::mpsc::SyncSender<Result<String, String>>,
    tx: crate::CaptureSender,
    selected_id: Option<&str>,
    mute_request: &std::sync::Mutex<Option<std::path::PathBuf>>,
    mute_state: &AtomicU8,
) -> Result<bool, String> {
    let _mmcss = register_mmcss();

    let notifications = crate::endpoints::Notifications::new().map_err(|e| e.to_string())?;
    let mut generation = notifications.generation.load(Ordering::Relaxed);
    let device = crate::endpoints::select(selected_id)?;
    let endpoint_id = device.get_id().map_err(|e| e.to_string())?;
    if mute_request.lock().unwrap().is_some() {
        mute_state.store(1, Ordering::SeqCst);
    }
    let mut mute: Option<crate::endpoints::MuteGuard> = None;
    let mut mute_started: Option<Instant> = None;
    let mut signal_after_mute = false;
    let device_name = device
        .get_friendlyname()
        .unwrap_or_else(|_| "default render".to_string());
    tracing::info!(device = %device_name, "WASAPI loopback target");

    let mut audio_client = device
        .get_iaudioclient()
        .map_err(|e| format!("get_iaudioclient: {e}"))?;

    // Formato deseado: 16-bit signed int, 44.1k, estéreo. Con AUTOCONVERTPCM
    // (convert=true en initialize_client) WASAPI hace la conversión desde el
    // mix format del dispositivo (típicamente 48k float32).
    let desired_format = WaveFormat::new(
        16,
        16,
        &SampleType::Int,
        target_rate as usize,
        target_channels as usize,
        None,
    );
    let bytes_per_frame = desired_format.get_blockalign() as usize; // 4 bytes (2ch * 2B)

    let (def_period, _min_period) = audio_client
        .get_periods()
        .map_err(|e| format!("get_periods: {e}"))?;

    // Direction::Capture + dispositivo abierto como Render = la combinación
    // que wasapi-rs traduce a AUDCLNT_STREAMFLAGS_LOOPBACK | EVENTCALLBACK.
    // `convert=true` añade AUTOCONVERTPCM, evitando que tengamos que resamplear.
    audio_client
        .initialize_client(
            &desired_format,
            def_period,
            &Direction::Capture,
            &ShareMode::Shared,
            true,
        )
        .map_err(|e| format!("initialize_client (loopback): {e}"))?;

    let h_event = audio_client
        .set_get_eventhandle()
        .map_err(|e| format!("set_get_eventhandle: {e}"))?;

    let buffer_frame_count = audio_client
        .get_bufferframecount()
        .map_err(|e| format!("get_bufferframecount: {e}"))?;
    tracing::info!(buffer_frames = buffer_frame_count, "WASAPI buffer size");

    let capture_client = audio_client
        .get_audiocaptureclient()
        .map_err(|e| format!("get_audiocaptureclient: {e}"))?;

    audio_client
        .start_stream()
        .map_err(|e| format!("start_stream: {e}"))?;

    // Comunicamos al caller que la inicialización fue OK y devolvemos el name.
    let _ = init_tx.try_send(Ok(device_name));

    // VecDeque<u8> donde wasapi-rs escribe los bytes crudos del capture client.
    // Pre-reservamos espacio para varios chunks para evitar realloc en el path
    // caliente.
    let mut byte_queue: VecDeque<u8> =
        VecDeque::with_capacity(bytes_per_frame * (buffer_frame_count as usize + CHUNK_FRAMES) * 4);

    let chunk_bytes = CHUNK_FRAMES * bytes_per_frame;
    let mut chunks_captured = 0u64;
    let mut chunks_silence = 0u64;
    let mut chunks_dropped = 0u64;
    let mut last_iteration = Instant::now();
    let mut last_report = last_iteration;
    let mut max_event_gap = Duration::ZERO;

    // Ritmo real de un chunk (352 frames @ 44.1 kHz ≈ 7.982 ms): a ese ritmo hay
    // que soltar el silencio para que el receptor no note el hueco.
    let chunk_duration =
        Duration::from_nanos(CHUNK_FRAMES as u64 * 1_000_000_000 / target_rate as u64);
    // Samples que ocupa un chunk (352 frames * 2 canales).
    let chunk_samples = CHUNK_FRAMES * target_channels as usize;
    let mut last_real_audio = Instant::now();
    let mut silence_next = last_real_audio + SILENCE_AFTER_IDLE;
    let mut last_idle_warn: Option<Instant> = None;

    while running.load(Ordering::SeqCst) {
        let new_generation = notifications.generation.load(Ordering::Relaxed);
        if new_generation != generation {
            generation = new_generation;
            if selected_id.is_none()
                && crate::endpoints::select(None)?
                    .get_id()
                    .map_err(|e| e.to_string())?
                    != endpoint_id
            {
                let _ = audio_client.stop_stream();
                return Ok(true);
            }
            if !matches!(device.get_state(), Ok(wasapi::DeviceState::Active)) {
                return Err("selected_audio_output_unavailable".into());
            }
        }
        let now = Instant::now();
        if let Some(guard) = &mute {
            if guard.external_change() {
                mute_state.store(5, Ordering::SeqCst);
                mute = None;
                *mute_request.lock().unwrap() = None;
            } else if mute_started.is_some_and(|at| at.elapsed() >= Duration::from_millis(800)) {
                if signal_after_mute {
                    mute_state.store(3, Ordering::SeqCst);
                } else {
                    mute = None;
                    *mute_request.lock().unwrap() = None;
                    mute_state.store(4, Ordering::SeqCst);
                }
                mute_started = None;
            }
        }
        max_event_gap = max_event_gap.max(now.saturating_duration_since(last_iteration));
        last_iteration = now;

        // Drenamos lo que haya disponible y, mientras tengamos ≥ un chunk,
        // empaquetamos y mandamos.
        capture_client
            .read_from_device_to_deque(&mut byte_queue)
            .map_err(|e| format!("read_from_device_to_deque: {e}"))?;

        let mut produced = false;
        while byte_queue.len() >= chunk_bytes {
            chunks_captured += 1;
            produced = true;
            let mut samples = Vec::with_capacity(CHUNK_FRAMES * target_channels as usize);
            // bytes_per_frame = 2 canales * 2 bytes/sample = 4. Consumimos
            // exactamente chunk_bytes bytes y los convertimos a i16 LE.
            for _ in 0..(CHUNK_FRAMES * target_channels as usize) {
                let lo = byte_queue.pop_front().unwrap();
                let hi = byte_queue.pop_front().unwrap();
                samples.push(i16::from_le_bytes([lo, hi]));
            }

            let pending_frames = byte_queue.len() / bytes_per_frame;
            let captured_at = Instant::now()
                - Duration::from_secs_f64(
                    (pending_frames + CHUNK_FRAMES) as f64 / target_rate as f64,
                );
            let signal = samples.iter().any(|s| s.unsigned_abs() > 64);
            // Ignore audio queued before the endpoint mute took effect.
            if mute_started.is_some_and(|at| at.elapsed() >= Duration::from_millis(150)) && signal {
                signal_after_mute = true;
            }
            let requested_journal = mute_request.lock().unwrap().clone();
            if mute.is_none() && signal {
                if let Some(journal) = requested_journal {
                    match crate::endpoints::MuteGuard::apply(&endpoint_id, &journal) {
                        Ok(guard) => {
                            // Do not count buffered pre-mute samples as proof of live capture.
                            byte_queue.clear();
                            mute = Some(guard);
                            mute_started = Some(Instant::now());
                            signal_after_mute = false;
                            mute_state.store(2, Ordering::SeqCst);
                        }
                        Err(error) => {
                            tracing::warn!(%error, "local mute unavailable");
                            mute_state.store(4, Ordering::SeqCst);
                            *mute_request.lock().unwrap() = None;
                        }
                    }
                }
            }
            let mut frame = CapturedFrame::new(samples, target_channels, target_rate);
            frame.captured_at = captured_at;
            // try_send: si el consumidor (pump → ALAC) está saturado,
            // soltamos el chunk para no inflar latencia indefinidamente.
            if tx.try_send(frame).is_err() {
                chunks_dropped += 1;
            }
        }

        if produced {
            last_real_audio = Instant::now();
        }

        // ¿El endpoint lleva un rato sin entregar nada? Entonces no hay nadie
        // reproduciendo audio en él (o está en standby) y WASAPI no va a mandar
        // más eventos. Rellenamos con silencio al ritmo real para que la sesión
        // AirPlay siga recibiendo datos y el receptor no la dé por muerta; en
        // cuanto vuelva el audio de verdad, esto se corta solo.
        let idle = last_real_audio.elapsed() >= SILENCE_AFTER_IDLE;
        if idle {
            let now = Instant::now();
            if now >= silence_next {
                // No acumulamos silencio en la cola. Si vuelve el audio real,
                // como máximo habrá dos paquetes mudos por delante.
                if tx.len() < 2 {
                    match tx.try_send(CapturedFrame::new(
                        vec![0i16; chunk_samples],
                        target_channels,
                        target_rate,
                    )) {
                        Ok(()) => chunks_silence += 1,
                        Err(_) => chunks_dropped += 1,
                    }
                }
                // Un bloqueo largo descarta tiempo pasado en vez de enviar
                // una ráfaga de paquetes que elevaría la latencia.
                silence_next = now + chunk_duration;
            }
        } else {
            silence_next = last_real_audio + SILENCE_AFTER_IDLE;
        }

        if last_report.elapsed() >= DIAGNOSTICS_INTERVAL {
            tracing::info!(
                chunks_captured,
                chunks_silence,
                chunks_dropped,
                max_event_gap_ms = max_event_gap.as_millis(),
                queued_frames = byte_queue.len() / bytes_per_frame,
                idle,
                "WASAPI capture diagnostics"
            );
            last_report = Instant::now();
            max_event_gap = Duration::ZERO;
        }

        // Esperar hasta el umbral de inactividad evita el hueco inicial de 3 s
        // antes de empezar el silencio, pero conserva la espera por eventos
        // durante el audio normal.
        let wait_ms = if idle {
            IDLE_POLL_MS
        } else {
            let until_idle = SILENCE_AFTER_IDLE.saturating_sub(last_real_audio.elapsed());
            (until_idle.as_millis().max(1) as u32).min(EVENT_TIMEOUT_MS)
        };

        if h_event.wait_for_event(wait_ms).is_err() {
            // Si el running ya está a false, es un shutdown ordenado.
            if !running.load(Ordering::SeqCst) {
                break;
            }
            // Un timeout NO es fatal: lo normal es que el endpoint esté parado
            // porque nadie reproduce audio, no que el driver esté colgado.
            // Antes dábamos el hilo por muerto aquí, y el streaming se quedaba
            // mudo para siempre hasta reiniciar la app. Ahora seguimos: el
            // silencio mantiene el flujo vivo y el audio real se reanuda solo.
            let should_warn = last_idle_warn
                .map(|t| t.elapsed() >= IDLE_WARN_INTERVAL)
                .unwrap_or(true);
            if should_warn {
                tracing::warn!(
                    wait_ms,
                    "loopback sin eventos — endpoint inactivo, seguimos esperando"
                );
                last_idle_warn = Some(Instant::now());
            }
        }
    }

    let _ = audio_client.stop_stream();
    Ok(false)
}
