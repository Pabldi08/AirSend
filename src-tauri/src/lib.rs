mod diagnostics;
use cap_core::streaming::{LocalBufferPolicy, PreparedLiveStream};
use diagnostics::{DiagnosticState, SessionRecord};
use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cap_core::{
    browse_once,
    pairing::{pair_homepod, DeviceDescriptor, PairedSession},
    probe::{manual_device, parse_manual_endpoint},
    probe_airplay, Device, Discovery,
};
use tauri::menu::{MenuBuilder, MenuItem, MenuItemBuilder};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, State, WindowEvent};
use tauri_plugin_store::StoreExt;
use tracing_subscriber::EnvFilter;

/// Nombre del archivo de la store en `%APPDATA%/<bundle-id>/`. Lo lleva
/// `tauri-plugin-store` por nosotros — sólo necesitamos una clave estable.
const STORE_FILE: &str = "settings.json";
const KEY_LAST_DEVICE: &str = "last_device";
const KEY_VOLUME: &str = "volume";
const KEY_LATENCY: &str = "latency";
const KEY_MULTI_DEVICE: &str = "multi_device";
const KEY_LOCAL_BUFFER: &str = "experimental_local_buffer";
const AUDIO_DIAGNOSTICS_INTERVAL: Duration = Duration::from_secs(10);
const LATENCY_CHANGE_COOLDOWN: Duration = Duration::from_secs(10);

fn latency_cooldown_remaining(last_change: Option<Instant>, now: Instant) -> Duration {
    last_change
        .map(|at| LATENCY_CHANGE_COOLDOWN.saturating_sub(now.saturating_duration_since(at)))
        .unwrap_or(Duration::ZERO)
}

#[cfg(test)]
mod latency_tests {
    use super::*;

    #[test]
    fn changes_unlock_after_ten_seconds() {
        let changed_at = Instant::now();
        assert_eq!(
            latency_cooldown_remaining(Some(changed_at), changed_at + Duration::from_secs(9)),
            Duration::from_secs(1)
        );
        assert_eq!(
            latency_cooldown_remaining(Some(changed_at), changed_at + Duration::from_secs(10)),
            Duration::ZERO
        );
    }

    #[test]
    fn older_latency_settings_survive_the_slider_upgrade() {
        assert_eq!(decode_latency(&serde_json::json!("music")), Some(3000));
        assert_eq!(decode_latency(&serde_json::json!("video")), Some(2000));
        assert_eq!(decode_latency(&serde_json::json!("gaming")), Some(1000));
        assert_eq!(decode_latency(&serde_json::json!(700)), Some(700));
        assert_eq!(decode_latency(&serde_json::json!(750)), None);
    }
}

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
                tracing::warn!("airplay-pump: no se pudo liberar el registro MMCSS");
            }
        }
    }
}

#[cfg(windows)]
fn register_audio_pump_mmcss() -> Option<MmcssGuard> {
    const AVRT_PRIORITY_CRITICAL: i32 = 2;
    #[link(name = "avrt")]
    extern "system" {
        fn AvSetMmThreadCharacteristicsW(
            task: *const u16,
            task_index: *mut u32,
        ) -> *mut std::ffi::c_void;
        fn AvSetMmThreadPriority(handle: *mut std::ffi::c_void, priority: i32) -> i32;
        fn AvRevertMmThreadCharacteristics(handle: *mut std::ffi::c_void) -> i32;
    }

    let task: Vec<u16> = "Pro Audio\0".encode_utf16().collect();
    let mut task_index = 0;
    unsafe {
        let handle = AvSetMmThreadCharacteristicsW(task.as_ptr(), &mut task_index);
        if handle.is_null() {
            tracing::warn!("airplay-pump: no se pudo registrar el hilo en MMCSS Pro Audio");
            return None;
        }
        if AvSetMmThreadPriority(handle, AVRT_PRIORITY_CRITICAL) == 0 {
            tracing::warn!("airplay-pump: no se pudo asignar prioridad MMCSS crítica");
            let _ = AvRevertMmThreadCharacteristics(handle);
            return None;
        }
        tracing::info!(
            task_index,
            "airplay-pump: hilo registrado en MMCSS Pro Audio"
        );
        Some(MmcssGuard(handle))
    }
}

#[cfg(not(windows))]
fn register_audio_pump_mmcss() {}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct LastDevice {
    ip: String,
    port: u16,
    name: String,
}

/// Mantiene el `Discovery` activo entre llamadas para no recrear el daemon
/// (cada uno abre montones de sockets en 5353 — sin esto, cada hot-reload
/// del frontend filtraba un daemon nuevo).
///
/// `cache` guarda todo dispositivo visto (mDNS o manual). El frontend llama
/// `start_discovery_stream` cada vez que el usuario pulsa "Buscar"; sin
/// replay, los anuncios mDNS ya consumidos no se vuelven a emitir y la lista
/// queda vacía hasta el siguiente broadcast (puede tardar minutos).
#[derive(Default)]
struct DiscoveryState {
    inner: Mutex<Option<Discovery>>,
    cache: Mutex<HashMap<String, Device>>,
}

/// Mantiene la sesión RTSP autenticada con el HomePod. Por ahora, sólo una a la
/// vez (alcance MVP: un solo HomePod). El Connection no es Send/Sync trivial
/// porque tiene streams TCP/UDP; usamos un Mutex async para acceso seguro.
#[derive(Default)]
struct ConnectionState {
    inner: tokio::sync::Mutex<Option<PairedSession>>,
}

/// Each receiver owns its RTSP connection and feedback task. All receivers in
/// an active stream share one system-audio capture and one pump.
struct ActiveOutput {
    connection: std::sync::Arc<tokio::sync::Mutex<cap_core::streaming::Connection>>,
    _heartbeat: cap_core::streaming::HeartbeatGuard,
    ip: String,
    port: u16,
    name: String,
    volume: f32,
}

impl ActiveOutput {
    fn info(&self, sample_rate: u32, channels: u8) -> StreamingInfo {
        StreamingInfo {
            ip: self.ip.clone(),
            port: self.port,
            name: self.name.clone(),
            sample_rate,
            channels,
            volume: self.volume,
        }
    }
}

struct ActiveStream {
    outputs: HashMap<String, ActiveOutput>,
    senders: std::sync::Arc<Mutex<HashMap<String, cap_core::streaming::LiveFrameSender>>>,
    _capture: Box<dyn audio_capture::Capture>,
    diagnostics: SessionRecord,
    policy: LocalBufferPolicy,
    pump: Option<std::thread::JoinHandle<()>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    latency_ms: u32,
    sample_rate: u32,
    channels: u8,
}

impl ActiveStream {
    async fn stop_outputs(&mut self) {
        for output in self.outputs.values_mut() {
            output._heartbeat.shutdown();
            let stopped = tokio::time::timeout(Duration::from_secs(3), async {
                output.connection.lock().await.stop().await
            })
            .await;
            if !matches!(stopped, Ok(Ok(()))) {
                tracing::warn!("AirPlay output did not finish FLUSH within the shutdown deadline");
            }
        }
    }
    fn shutdown(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(h) = self.pump.take() {
            let _ = h.join();
        }
        self.senders.lock().unwrap().clear();
        self.diagnostics.lock().unwrap().finish();
        // _heartbeat se aborta solo al dropearse.
    }
}

impl Drop for ActiveStream {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Default)]
struct StreamingState {
    inner: tokio::sync::Mutex<Option<ActiveStream>>,
    last_latency_change: tokio::sync::Mutex<Option<Instant>>,
}

fn output_key(ip: &str, port: u16) -> String {
    format!("{ip}:{port}")
}

#[tauri::command]
async fn discover_devices(timeout_ms: Option<u64>) -> Result<Vec<Device>, String> {
    let timeout = Duration::from_millis(timeout_ms.unwrap_or(3_000));
    browse_once(timeout).await.map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_discovery_stream(
    app: tauri::AppHandle,
    state: State<'_, DiscoveryState>,
) -> Result<(), String> {
    // Replay del cache: si el usuario pulsó "Buscar" otra vez, el listener de JS
    // se acaba de recrear con `known` vacío, así que reemitimos lo que ya
    // conocemos para rellenar la lista al instante.
    let cached: Vec<Device> = state
        .cache
        .lock()
        .map_err(|e| e.to_string())?
        .values()
        .cloned()
        .collect();
    for dev in &cached {
        let _ = app.emit("airplay://device", dev);
    }

    // Lock + spawn + drop del guard antes de devolver. Sin await durante el lock,
    // así el std::sync::Mutex es seguro aunque la fn sea async.
    let mut rx = {
        let mut slot = state.inner.lock().map_err(|e| e.to_string())?;
        if slot.is_some() {
            tracing::debug!("discovery ya en curso, replayé {} cacheados", cached.len());
            return Ok(());
        }
        let discovery = Discovery::new().map_err(|e| e.to_string())?;
        let rx = discovery.browse().map_err(|e| e.to_string())?;
        *slot = Some(discovery);
        rx
    };

    let app_pump = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(device) = rx.recv().await {
            if let Some(state) = app_pump.try_state::<DiscoveryState>() {
                if let Ok(mut cache) = state.cache.lock() {
                    cache.insert(device.id.clone(), device.clone());
                }
            }
            if app_pump.emit("airplay://device", &device).is_err() {
                break;
            }
        }
    });

    Ok(())
}

#[tauri::command]
async fn stop_discovery_stream(state: State<'_, DiscoveryState>) -> Result<(), String> {
    let mut slot = state.inner.lock().map_err(|e| e.to_string())?;
    if let Some(discovery) = slot.take() {
        discovery.shutdown();
    }
    Ok(())
}

#[derive(serde::Serialize, Clone)]
struct StreamingInfo {
    ip: String,
    port: u16,
    name: String,
    sample_rate: u32,
    channels: u8,
    volume: f32,
}

async fn prepare_output(
    ip: &str,
    port: u16,
    name: &str,
    volume: f32,
    latency_ms: u32,
    policy: LocalBufferPolicy,
    diagnostics: &SessionRecord,
) -> Result<PreparedLiveStream, String> {
    let parsed: IpAddr = ip.parse().map_err(|e| format!("IP inválida: {e}"))?;
    let descriptor = DeviceDescriptor {
        ip: parsed,
        port,
        name: name.to_string(),
        mac: None,
        model: None,
        features: None,
    };
    cap_core::streaming::prepare_live_stream(descriptor, Some(volume), Some(latency_ms), policy)
        .await
        .map_err(|e| {
            diagnostics.lock().unwrap().record_failure(e.stage_code());
            format!("stream: {e}")
        })
}

async fn start_output(
    prepared: PreparedLiveStream,
    ip: &str,
    port: u16,
    name: &str,
    volume: f32,
    diagnostics: &SessionRecord,
) -> Result<ActiveOutput, String> {
    let handle = prepared.start().await.map_err(|e| {
        diagnostics.lock().unwrap().record_failure(e.stage_code());
        format!("stream: {e}")
    })?;
    let (_, connection, heartbeat, _, _) = handle.into_parts();
    Ok(ActiveOutput {
        connection,
        _heartbeat: heartbeat,
        ip: ip.to_string(),
        port,
        name: name.to_string(),
        volume,
    })
}

async fn attach_output(
    active: &mut ActiveStream,
    ip: &str,
    port: u16,
    name: &str,
    volume: f32,
) -> Result<(), String> {
    let prepared = prepare_output(
        ip,
        port,
        name,
        volume,
        active.latency_ms,
        active.policy,
        &active.diagnostics,
    )
    .await?;
    if (prepared.sample_rate(), prepared.channels()) != (active.sample_rate, active.channels) {
        return Err("los receptores no comparten formato de audio".into());
    }
    let key = output_key(ip, port);
    let sender = prepared.sender();
    active
        .diagnostics
        .lock()
        .unwrap()
        .add_output(sender.clone(), prepared.metrics());
    active
        .senders
        .lock()
        .map_err(|e| e.to_string())?
        .insert(key.clone(), sender);
    // The existing pump feeds pre-roll while this receiver starts.
    match start_output(prepared, ip, port, name, volume, &active.diagnostics).await {
        Ok(output) => {
            active.outputs.insert(key, output);
            Ok(())
        }
        Err(error) => {
            active
                .senders
                .lock()
                .map_err(|e| e.to_string())?
                .remove(&key);
            Err(error)
        }
    }
}

/// Prepare the network before capturing. Feed live PCM before waiting for pre-roll.
/// The previous stream remains owned by the caller until this returns successfully.
async fn prepare_stream(
    app: tauri::AppHandle,
    targets: &[(String, u16, String, f32)],
    latency_ms: u32,
) -> Result<ActiveStream, String> {
    use audio_capture::{CaptureFormat, CapturePolicy};
    use std::sync::{atomic::AtomicBool, Arc};
    cap_core::streaming::validate_latency_ms(latency_ms).map_err(|e| e.to_string())?;
    let (first_ip, first_port, first_name, first_volume) =
        targets.first().ok_or("no hay receptores")?;
    let policy = if get_experimental_local_buffer(app.clone())? {
        LocalBufferPolicy::LowLatency
    } else {
        LocalBufferPolicy::Stable
    };
    let diagnostics = app.state::<DiagnosticState>().begin(latency_ms, policy);
    let prepared = match prepare_output(
        first_ip,
        *first_port,
        first_name,
        *first_volume,
        latency_ms,
        policy,
        &diagnostics,
    )
    .await
    {
        Ok(value) => value,
        Err(error) => {
            diagnostics.lock().unwrap().failed();
            return Err(error);
        }
    };
    let sample_rate = prepared.sample_rate();
    let channels = prepared.channels();
    let began = Instant::now();
    let capture_policy = if policy == LocalBufferPolicy::LowLatency {
        CapturePolicy::LowLatency
    } else {
        CapturePolicy::Stable
    };
    let (capture, rx) = match audio_capture::start_loopback_with_policy(
        CaptureFormat::AIRPLAY_DEFAULT,
        capture_policy,
    ) {
        Ok(value) => value,
        Err(error) => {
            diagnostics.lock().unwrap().record_failure("capture_start");
            diagnostics.lock().unwrap().failed();
            return Err(format!("captura: {error}"));
        }
    };
    diagnostics.lock().unwrap().capture_started(began.elapsed());
    let key = output_key(first_ip, *first_port);
    let sender = prepared.sender();
    diagnostics
        .lock()
        .unwrap()
        .add_output(sender.clone(), prepared.metrics());
    let senders = Arc::new(Mutex::new(HashMap::from([(key.clone(), sender)])));
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let pump_senders = senders.clone();
    let pump_diagnostics = diagnostics.clone();
    let pump = std::thread::Builder::new()
        .name("airplay-pump".into())
        .spawn(move || {
            pump_loop(
                app,
                rx,
                pump_senders,
                sample_rate,
                channels,
                stop_thread,
                pump_diagnostics,
            )
        })
        .map_err(|e| {
            diagnostics.lock().unwrap().failed();
            format!("pump thread: {e}")
        })?;
    let mut active = ActiveStream {
        outputs: HashMap::new(),
        senders,
        _capture: capture,
        diagnostics,
        policy,
        pump: Some(pump),
        stop,
        latency_ms,
        sample_rate,
        channels,
    };
    let first = start_output(
        prepared,
        first_ip,
        *first_port,
        first_name,
        *first_volume,
        &active.diagnostics,
    )
    .await?;
    if active.stop.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("capture_interrupted".into());
    }
    active.outputs.insert(key, first);
    for (ip, port, name, volume) in targets.iter().skip(1) {
        attach_output(&mut active, ip, *port, name, *volume).await?;
    }
    if active.stop.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("capture_interrupted".into());
    }
    active.diagnostics.lock().unwrap().streaming();
    Ok(active)
}

#[tauri::command]
async fn start_streaming(
    app: tauri::AppHandle,
    ip: String,
    port: Option<u16>,
    name: Option<String>,
    volume: Option<f32>,
    latency_ms: Option<u32>,
    state: State<'_, StreamingState>,
) -> Result<StreamingInfo, String> {
    let parsed: IpAddr = ip.parse().map_err(|e| format!("IP inválida: {e}"))?;
    let port = port.unwrap_or(7000);
    let name = name.unwrap_or_else(|| format!("HomePod {parsed}"));
    let volume = volume
        .unwrap_or(cap_core::streaming::DEFAULT_INITIAL_VOLUME)
        .clamp(0.0, 1.0);
    let latency_ms = latency_ms.unwrap_or(cap_core::streaming::DEFAULT_LATENCY_MS);
    let mut slot = state.inner.lock().await;
    let active =
        prepare_stream(app, &[(parsed.to_string(), port, name, volume)], latency_ms).await?;
    let info = active
        .outputs
        .values()
        .next()
        .unwrap()
        .info(active.sample_rate, active.channels);
    if let Some(mut previous) = slot.replace(active) {
        previous.shutdown();
        previous.stop_outputs().await;
    }
    Ok(info)
}

/// Add a receiver to the existing capture without interrupting other outputs.
#[tauri::command]
async fn add_streaming(
    app: tauri::AppHandle,
    ip: String,
    port: Option<u16>,
    name: Option<String>,
    volume: Option<f32>,
    latency_ms: Option<u32>,
    state: State<'_, StreamingState>,
) -> Result<StreamingInfo, String> {
    let parsed: IpAddr = ip.parse().map_err(|e| format!("IP inválida: {e}"))?;
    let port = port.unwrap_or(7000);
    let name = name.unwrap_or_else(|| format!("HomePod {parsed}"));
    let volume = volume
        .unwrap_or(cap_core::streaming::DEFAULT_INITIAL_VOLUME)
        .clamp(0.0, 1.0);
    let mut slot = state.inner.lock().await;
    let Some(active) = slot.as_mut() else {
        let latency = latency_ms.unwrap_or(cap_core::streaming::DEFAULT_LATENCY_MS);
        let stream =
            prepare_stream(app, &[(parsed.to_string(), port, name, volume)], latency).await?;
        let info = stream
            .outputs
            .values()
            .next()
            .unwrap()
            .info(stream.sample_rate, stream.channels);
        *slot = Some(stream);
        return Ok(info);
    };
    if let Some(requested) = latency_ms {
        if requested != active.latency_ms {
            return Err("la latencia no coincide con la reproducción actual".to_string());
        }
    }
    let key = output_key(&parsed.to_string(), port);
    if let Some(output) = active.outputs.get(&key) {
        return Ok(output.info(active.sample_rate, active.channels));
    }
    attach_output(active, &parsed.to_string(), port, &name, volume).await?;
    let info = active
        .outputs
        .get(&key)
        .unwrap()
        .info(active.sample_rate, active.channels);
    Ok(info)
}

fn pump_loop(
    app: tauri::AppHandle,
    rx: audio_capture::CaptureReceiver,
    senders: std::sync::Arc<Mutex<HashMap<String, cap_core::streaming::LiveFrameSender>>>,
    sample_rate: u32,
    channels: u8,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    diagnostics: SessionRecord,
) {
    use cap_core::streaming::LivePcmFrame;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    let _mmcss = register_audio_pump_mmcss();
    let mut unexpected_exit = false;
    let mut frames_forwarded = 0u64;
    let mut frames_dropped = 0u64;
    let mut last_frame = Instant::now();
    let mut last_report = last_frame;
    let mut max_capture_gap = Duration::ZERO;
    while !stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(frame) => {
                let now = Instant::now();
                max_capture_gap = max_capture_gap.max(now.saturating_duration_since(last_frame));
                last_frame = now;
                let Ok(targets) = senders.lock() else {
                    unexpected_exit = true;
                    break;
                };
                let mut samples = Some(frame.samples);
                let count = targets.len();
                for (index, sender) in targets.values().enumerate() {
                    let payload = if index + 1 == count {
                        samples.take().unwrap()
                    } else {
                        samples.as_ref().unwrap().clone()
                    };
                    if !sender.try_send_at(
                        LivePcmFrame {
                            samples: payload,
                            channels,
                            sample_rate,
                        },
                        frame.captured_at,
                    ) {
                        frames_dropped += 1;
                    } else {
                        frames_forwarded += 1;
                    }
                }

                if last_report.elapsed() >= AUDIO_DIAGNOSTICS_INTERVAL {
                    tracing::info!(
                        frames_forwarded,
                        frames_dropped,
                        max_capture_gap_ms = max_capture_gap.as_millis(),
                        "airplay-pump diagnostics"
                    );
                    diagnostics.lock().unwrap().update(
                        rx.diagnostics(),
                        frames_forwarded,
                        frames_dropped,
                        max_capture_gap,
                    );
                    last_report = Instant::now();
                    max_capture_gap = Duration::ZERO;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(_) => {
                // El canal de captura se cerró sin que nosotros pidamos shutdown:
                // device removed, driver crash, parec mató al subproceso, etc.
                unexpected_exit = !stop.load(Ordering::SeqCst);
                break;
            }
        }
    }
    diagnostics.lock().unwrap().update(
        rx.diagnostics(),
        frames_forwarded,
        frames_dropped,
        max_capture_gap,
    );
    if unexpected_exit {
        let notify_active_session = diagnostics.lock().unwrap().can_emit_capture_error();
        stop.store(true, Ordering::SeqCst);
        diagnostics.lock().unwrap().record_failure("capture");
        diagnostics.lock().unwrap().failed();
        tracing::warn!("airplay-pump: canal captura cerrado inesperadamente, emitiendo error");
        // A failing replacement must not stop the previous healthy receiver.
        // Preparation failures return synchronously to the switch caller.
        if notify_active_session {
            let _ = app.emit("airplay://error", "capture_interrupted");
        }
    } else {
        tracing::info!("airplay-pump thread exit");
    }
}

#[tauri::command]
async fn stop_streaming(state: State<'_, StreamingState>) -> Result<(), String> {
    let mut slot = state.inner.lock().await;
    if let Some(mut active) = slot.take() {
        active.shutdown();
        active.stop_outputs().await;
    }
    Ok(())
}

#[tauri::command]
async fn remove_streaming(
    ip: String,
    port: u16,
    state: State<'_, StreamingState>,
) -> Result<(), String> {
    let mut slot = state.inner.lock().await;
    let Some(active) = slot.as_mut() else {
        return Ok(());
    };
    let key = output_key(&ip, port);
    active
        .senders
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&key);
    if let Some(mut output) = active.outputs.remove(&key) {
        output._heartbeat.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(3), async {
            output.connection.lock().await.stop().await
        })
        .await;
    }
    if active.outputs.is_empty() {
        if let Some(mut empty) = slot.take() {
            empty.shutdown();
        }
    }
    Ok(())
}

#[tauri::command]
async fn set_stream_volume(volume: f32, state: State<'_, StreamingState>) -> Result<f32, String> {
    let mut slot = state.inner.lock().await;
    let active = slot
        .as_mut()
        .ok_or_else(|| "no hay streaming activo".to_string())?;
    let v = volume.clamp(0.0, 1.0);
    let mut errors = Vec::new();
    for output in active.outputs.values_mut() {
        let mut conn = output.connection.lock().await;
        match conn.set_volume(v).await {
            Ok(()) => output.volume = v,
            Err(error) => errors.push(format!("{}: {error}", output.name)),
        }
    }
    if errors.is_empty() {
        Ok(v)
    } else {
        Err(errors.join("; "))
    }
}

#[tauri::command]
async fn is_streaming(state: State<'_, StreamingState>) -> Result<Option<StreamingInfo>, String> {
    let slot = state.inner.lock().await;
    Ok(slot.as_ref().and_then(|a| {
        a.outputs
            .values()
            .next()
            .map(|o| o.info(a.sample_rate, a.channels))
    }))
}

#[tauri::command]
async fn list_streaming(state: State<'_, StreamingState>) -> Result<Vec<StreamingInfo>, String> {
    let slot = state.inner.lock().await;
    Ok(slot
        .as_ref()
        .map(|a| {
            a.outputs
                .values()
                .map(|o| o.info(a.sample_rate, a.channels))
                .collect()
        })
        .unwrap_or_default())
}

#[derive(serde::Serialize)]
struct ConnectionInfo {
    ip: String,
    port: u16,
    name: String,
}

/// Hace pair-setup transient + pair-verify contra el HomePod indicado y
/// mantiene la sesión RTSP abierta hasta que se llame a `disconnect_device`.
#[tauri::command]
async fn connect_device(
    ip: String,
    port: Option<u16>,
    name: Option<String>,
    state: State<'_, ConnectionState>,
) -> Result<ConnectionInfo, String> {
    let parsed: IpAddr = ip.parse().map_err(|e| format!("IP inválida '{ip}': {e}"))?;
    let port = port.unwrap_or(7000);
    let display_name = name.clone().unwrap_or_else(|| format!("HomePod {parsed}"));

    let descriptor = DeviceDescriptor {
        ip: parsed,
        port,
        name: display_name.clone(),
        mac: None,
        model: None,
        features: None,
    };

    // Keep the previous pairing until the new target succeeds.
    let mut slot = state.inner.lock().await;
    let session = pair_homepod(descriptor)
        .await
        .map_err(|e| format!("pairing falló: {e}"))?;
    *slot = Some(session);

    Ok(ConnectionInfo {
        ip: parsed.to_string(),
        port,
        name: display_name,
    })
}

/// Suelta la sesión RTSP actual (si hay).
#[tauri::command]
async fn disconnect_device(state: State<'_, ConnectionState>) -> Result<(), String> {
    let mut slot = state.inner.lock().await;
    *slot = None;
    Ok(())
}

#[tauri::command]
async fn is_connected(state: State<'_, ConnectionState>) -> Result<bool, String> {
    let slot = state.inner.lock().await;
    Ok(slot.is_some())
}

/// Añade un dispositivo introducido manualmente por IP. Útil en redes donde
/// mDNS no se propaga (típicamente router Movistar HGU sin reflexión multicast
/// entre 2.4 y 5 GHz). Verifica que hay un AirPlay escuchando antes de emitir.
#[tauri::command]
async fn add_manual_device(
    app: tauri::AppHandle,
    ip: String,
    port: Option<u16>,
    name: Option<String>,
    state: State<'_, DiscoveryState>,
) -> Result<Device, String> {
    let (parsed, port) =
        parse_manual_endpoint(&ip, port).map_err(|e| format!("endpoint inválido '{ip}': {e}"))?;

    let probe = probe_airplay(parsed, port)
        .await
        .map_err(|e| format!("no parece un AirPlay en {parsed}:{port} — {e}"))?;

    let mut device = manual_device(parsed, Some(port), name);
    if let Some(server) = probe.server_header.as_deref() {
        device.features = Some(server.to_string());
        if server.to_lowercase().contains("airtunes") {
            device.supports_airplay2 = true;
        }
    }

    if let Ok(mut cache) = state.cache.lock() {
        cache.insert(device.id.clone(), device.clone());
    }

    app.emit("airplay://device", &device)
        .map_err(|e| e.to_string())?;

    Ok(device)
}

// ── Persistencia (C1) ────────────────────────────────────────────────────────
//
// Usamos `tauri-plugin-store` para volcar a `%APPDATA%/<bundle-id>/settings.json`
// las preferencias del usuario que sobreviven a reinicios de la app:
// - Último HomePod conectado: para reconectar en frío y para el menú del tray.
// - Volumen: para que la sesión nueva arranque al nivel que dejaste.
//
// El plugin maneja serialización JSON, escritura atómica y carga lazy.

#[tauri::command]
fn save_last_device(
    app: tauri::AppHandle,
    ip: String,
    port: u16,
    name: String,
) -> Result<(), String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    let dev = LastDevice { ip, port, name };
    store.set(
        KEY_LAST_DEVICE,
        serde_json::to_value(&dev).map_err(|e| e.to_string())?,
    );
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn get_last_device(app: tauri::AppHandle) -> Result<Option<LastDevice>, String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    let value = store.get(KEY_LAST_DEVICE);
    match value {
        Some(v) => serde_json::from_value(v)
            .map(Some)
            .map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

#[tauri::command]
fn clear_last_device(app: tauri::AppHandle) -> Result<(), String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    store.delete(KEY_LAST_DEVICE);
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn save_volume(app: tauri::AppHandle, volume: f32) -> Result<(), String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    let v = volume.clamp(0.0, 1.0);
    store.set(
        KEY_VOLUME,
        serde_json::to_value(v).map_err(|e| e.to_string())?,
    );
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn get_volume(app: tauri::AppHandle) -> Result<Option<f32>, String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    let value = store.get(KEY_VOLUME);
    match value {
        Some(v) => serde_json::from_value(v)
            .map(Some)
            .map_err(|e| e.to_string()),
        None => Ok(None),
    }
}

#[tauri::command]
fn save_multi_device(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    store.set(KEY_MULTI_DEVICE, serde_json::json!(enabled));
    store.save().map_err(|e| e.to_string())
}

#[tauri::command]
fn get_multi_device(app: tauri::AppHandle) -> Result<bool, String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    Ok(store
        .get(KEY_MULTI_DEVICE)
        .as_ref()
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
}

#[tauri::command]
fn get_experimental_local_buffer(app: tauri::AppHandle) -> Result<bool, String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    Ok(store
        .get(KEY_LOCAL_BUFFER)
        .and_then(|v| v.as_bool())
        .unwrap_or(false))
}

#[tauri::command]
async fn save_experimental_local_buffer(
    app: tauri::AppHandle,
    enabled: bool,
    state: State<'_, StreamingState>,
) -> Result<(), String> {
    let stream = state.inner.lock().await;
    if stream.is_some() {
        return Err("stop_before_buffer_change".into());
    }
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    store.set(KEY_LOCAL_BUFFER, serde_json::json!(enabled));
    store.save().map_err(|e| e.to_string())
}

#[tauri::command]
fn export_audio_diagnostics(state: State<'_, DiagnosticState>) -> Result<String, String> {
    serde_json::to_string_pretty(&state.report()).map_err(|e| e.to_string())
}

fn save_latency(app: &tauri::AppHandle, latency_ms: u32) -> Result<(), String> {
    cap_core::streaming::validate_latency_ms(latency_ms).map_err(|e| e.to_string())?;
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    store.set(KEY_LATENCY, serde_json::json!(latency_ms));
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

fn decode_latency(value: &serde_json::Value) -> Option<u32> {
    let latency_ms = match value.as_str() {
        Some("music") => Some(3000),
        Some("video") => Some(2000),
        Some("gaming") => Some(1000),
        Some(_) => None,
        None => value.as_u64().and_then(|n| u32::try_from(n).ok()),
    };
    latency_ms.filter(|ms| cap_core::streaming::validate_latency_ms(*ms).is_ok())
}

#[tauri::command]
fn get_latency(app: tauri::AppHandle) -> Result<Option<u32>, String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    Ok(store.get(KEY_LATENCY).as_ref().and_then(decode_latency))
}

#[tauri::command]
async fn get_latency_cooldown_ms(state: State<'_, StreamingState>) -> Result<u64, String> {
    let changed = state.last_latency_change.lock().await;
    Ok(latency_cooldown_remaining(*changed, Instant::now()).as_millis() as u64)
}

/// Confirm one negotiated buffer change at most every ten seconds. A live
/// stream needs a fresh AirPlay SETUP, so restart it only after confirmation.
#[tauri::command]
async fn confirm_latency(
    app: tauri::AppHandle,
    latency_ms: u32,
    state: State<'_, StreamingState>,
) -> Result<bool, String> {
    cap_core::streaming::validate_latency_ms(latency_ms).map_err(|e| e.to_string())?;
    let mut changed = state.last_latency_change.lock().await;
    let remaining = latency_cooldown_remaining(*changed, Instant::now());
    if !remaining.is_zero() {
        return Err(format!("latency_cooldown:{}", remaining.as_millis()));
    }

    let previous = get_latency(app.clone())?.unwrap_or(cap_core::streaming::DEFAULT_LATENCY_MS);
    if previous == latency_ms {
        return Ok(false);
    }

    let mut slot = state.inner.lock().await;
    let restarted = if let Some(mut stream) = slot.take() {
        let targets = stream
            .outputs
            .values()
            .map(|output| {
                (
                    output.ip.clone(),
                    output.port,
                    output.name.clone(),
                    output.volume,
                )
            })
            .collect::<Vec<_>>();
        let old_latency_ms = stream.latency_ms;
        if let Err(error) = save_latency(&app, latency_ms) {
            *slot = Some(stream);
            return Err(error);
        }
        // A receiver may reject a second RTSP session to itself. Close the old
        // sessions before reopening them, then restore the old setup on error.
        stream.shutdown();
        stream.stop_outputs().await;
        drop(stream);
        match prepare_stream(app.clone(), &targets, latency_ms).await {
            Ok(replacement) => *slot = Some(replacement),
            Err(error) => {
                let _ = save_latency(&app, previous);
                if let Ok(restored) = prepare_stream(app, &targets, old_latency_ms).await {
                    *slot = Some(restored);
                }
                return Err(error);
            }
        }
        true
    } else {
        save_latency(&app, latency_ms)?;
        false
    };

    *changed = Some(Instant::now());
    Ok(restarted)
}

// ── System tray (C3) ─────────────────────────────────────────────────────────
//
// La app vive en bandeja del sistema. Cerrar la ventana la oculta pero el
// proceso sigue (streaming continúa). Sólo "Salir" del menú o un kill mata el
// daemon. Esto es lo esperado para una app de tipo "siempre disponible".

struct TrayItems {
    show: MenuItem<tauri::Wry>,
    quit: MenuItem<tauri::Wry>,
}

fn tray_labels(lang: &str) -> Option<(&'static str, &'static str)> {
    match lang {
        "es" => Some(("Mostrar / ocultar ventana", "Salir")),
        "en" => Some(("Show / hide window", "Quit")),
        _ => None,
    }
}

#[tauri::command]
fn set_tray_language(lang: String, state: State<'_, TrayItems>) -> Result<(), String> {
    let (show, quit) = tray_labels(&lang).ok_or_else(|| "unsupported language".to_string())?;
    state.show.set_text(show).map_err(|e| e.to_string())?;
    state.quit.set_text(quit).map_err(|e| e.to_string())?;
    Ok(())
}

fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<TrayItems> {
    let show_item = MenuItemBuilder::with_id("show", "Mostrar / ocultar ventana").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Salir").build(app)?;
    let menu = MenuBuilder::new(app)
        .item(&show_item)
        .separator()
        .item(&quit_item)
        .build()?;

    let mut builder = TrayIconBuilder::with_id("main")
        .tooltip("AirSend")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => toggle_main_window(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            // Click izquierdo en el icono = toggle ventana, comportamiento
            // típico de apps de tray en Windows.
            if let TrayIconEvent::Click { button, .. } = event {
                if matches!(button, tauri::tray::MouseButton::Left) {
                    toggle_main_window(tray.app_handle());
                }
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;

    Ok(TrayItems {
        show: show_item,
        quit: quit_item,
    })
}

fn toggle_main_window(app: &tauri::AppHandle) {
    if let Some(win) = app.get_webview_window("main") {
        let visible = win.is_visible().unwrap_or(false);
        if visible {
            let _ = win.hide();
        } else {
            let _ = win.show();
            let _ = win.set_focus();
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────

/// Localización de los logs por plataforma. Windows usa %APPDATA% (igual que
/// hace Tauri por defecto para `app_log_dir` con el bundle id de la app).
/// Si no se puede resolver, devolvemos None y los logs quedan solo en stdout.
fn resolve_log_dir() -> Option<std::path::PathBuf> {
    const APP_DIRNAME: &str = "ConexionAirPlay";
    #[cfg(windows)]
    {
        let base = std::env::var_os("APPDATA")?;
        Some(
            std::path::PathBuf::from(base)
                .join(APP_DIRNAME)
                .join("logs"),
        )
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var_os("HOME")?;
        Some(
            std::path::PathBuf::from(home)
                .join("Library")
                .join("Logs")
                .join(APP_DIRNAME),
        )
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_STATE_HOME") {
            return Some(std::path::PathBuf::from(xdg).join(APP_DIRNAME).join("logs"));
        }
        let home = std::env::var_os("HOME")?;
        Some(
            std::path::PathBuf::from(home)
                .join(".local")
                .join("state")
                .join(APP_DIRNAME)
                .join("logs"),
        )
    }
}

/// Configura `tracing`: stdout siempre + archivo rotado por día si el directorio
/// de logs es resoluble y escribible. Devuelve el `WorkerGuard` que mantiene
/// vivo el writer non-blocking (se debe retener todo el lifetime del programa;
/// si se dropea, los logs pendientes se pierden).
fn init_tracing() -> Option<tracing_appender::non_blocking::WorkerGuard> {
    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;
    use tracing_subscriber::Layer;

    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));

    let stdout_layer = tracing_subscriber::fmt::layer()
        .with_target(true)
        .with_ansi(true);

    let (file_layer, guard) = match resolve_log_dir() {
        Some(dir) => match std::fs::create_dir_all(&dir) {
            Ok(()) => {
                let appender = tracing_appender::rolling::daily(&dir, "app.log");
                let (writer, guard) = tracing_appender::non_blocking(appender);
                let layer = tracing_subscriber::fmt::layer()
                    .with_writer(writer)
                    .with_ansi(false)
                    .with_target(true);
                eprintln!("→ logs a {}", dir.display());
                (Some(layer), Some(guard))
            }
            Err(e) => {
                eprintln!("→ no pude crear dir de logs {}: {e}", dir.display());
                (None, None)
            }
        },
        None => (None, None),
    };

    tracing_subscriber::registry()
        .with(env_filter)
        .with(stdout_layer)
        .with(file_layer.map(|l| l.boxed()))
        .init();

    guard
}

/// Sube la clase de prioridad del proceso a HIGH_PRIORITY_CLASS en Windows.
/// Combinado con MMCSS "Pro Audio" en el sender thread (fork airplay2-rs),
/// elimina los tics audibles cada 20-30 s que ocurren porque el scheduler
/// fairness de Windows degrada threads de proceso NORMAL bajo carga.
#[cfg(windows)]
fn raise_process_priority() {
    use std::ffi::c_void;

    type Handle = *mut c_void;
    const HIGH_PRIORITY_CLASS: u32 = 0x0000_0080;

    extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn SetPriorityClass(process: Handle, class: u32) -> i32;
    }

    unsafe {
        if SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS) != 0 {
            tracing::info!("Windows: process priority elevated to HIGH_PRIORITY_CLASS");
        } else {
            tracing::warn!("Windows: SetPriorityClass(HIGH) failed");
        }
    }
}

#[cfg(not(windows))]
fn raise_process_priority() {}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // `_log_guard` debe vivir todo el programa para que el writer non-blocking
    // siga drenando logs al archivo. Lo "olvidamos" con un binding mut a static.
    let _log_guard = init_tracing();
    raise_process_priority();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .manage(DiscoveryState::default())
        .manage(ConnectionState::default())
        .manage(StreamingState::default())
        .manage(DiagnosticState::default())
        .invoke_handler(tauri::generate_handler![
            discover_devices,
            start_discovery_stream,
            stop_discovery_stream,
            add_manual_device,
            connect_device,
            disconnect_device,
            is_connected,
            start_streaming,
            add_streaming,
            remove_streaming,
            stop_streaming,
            set_stream_volume,
            is_streaming,
            list_streaming,
            save_last_device,
            get_last_device,
            clear_last_device,
            save_volume,
            get_volume,
            save_multi_device,
            get_multi_device,
            get_latency,
            get_latency_cooldown_ms,
            confirm_latency,
            set_tray_language,
            get_experimental_local_buffer,
            save_experimental_local_buffer,
            export_audio_diagnostics,
        ])
        .setup(|app| {
            let tray_items = setup_tray(app.handle())?;
            app.manage(tray_items);

            // Cerrar la ventana (X) la oculta en vez de matar el proceso —
            // la app sigue viva en el tray. "Salir" del menú del tray sí
            // termina el proceso.
            if let Some(win) = app.get_webview_window("main") {
                let app_handle = app.handle().clone();
                win.on_window_event(move |event| {
                    if let WindowEvent::CloseRequested { api, .. } = event {
                        api.prevent_close();
                        if let Some(w) = app_handle.get_webview_window("main") {
                            let _ = w.hide();
                        }
                    }
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    // `_log_guard` se dropea aquí al salir, drenando los últimos logs.
    drop(_log_guard);
}
