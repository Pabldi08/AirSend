mod diagnostics;
mod preferences;
mod session;
use cap_core::discovery::DiscoveryEvent;
use cap_core::streaming::{LocalBufferPolicy, PreparedLiveStream};
use diagnostics::{DiagnosticState, SessionRecord};
use preferences::{AudioOptions, ReceiverPreferences, ReceiverPreferencesPatch};
use session::{SessionControl, SessionEvent};
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
static RECEIVER_SETTINGS_LOCK: Mutex<()> = Mutex::new(());
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
    control: SessionControl,
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
    metrics: std::sync::Arc<cap_core::stream_metrics::StreamMetrics>,
    volume_read_supported: Option<bool>,
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
    _capture: Option<Box<dyn audio_capture::Capture>>,
    diagnostics: SessionRecord,
    policy: LocalBufferPolicy,
    pump: Option<std::thread::JoinHandle<()>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    latency_ms: u32,
    sample_rate: u32,
    channels: u8,
    generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
    group_mode: bool,
    target_order: Vec<String>,
}

impl ActiveStream {
    fn apply_mute(&self, app: &tauri::AppHandle) -> Result<(), String> {
        if get_audio_options(app.clone())?.mute_local {
            let path = app
                .path()
                .app_data_dir()
                .map_err(|e| e.to_string())?
                .join("local-mute-recovery.json");
            self._capture
                .as_ref()
                .ok_or("capture_interrupted")?
                .request_local_mute(path)
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
    async fn stop_outputs(&mut self) {
        for output in self.outputs.values_mut() {
            output._heartbeat.shutdown();
            let stopped = tokio::time::timeout(Duration::from_secs(3), async {
                output.connection.lock().await.disconnect().await
            })
            .await;
            if !matches!(stopped, Ok(Ok(()))) {
                tracing::warn!("AirPlay output did not disconnect within the shutdown deadline");
            }
        }
    }
    fn shutdown(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(h) = self.pump.take() {
            let _ = h.join();
        }
        self.senders.lock().unwrap().clear();
        if let Some(capture) = self._capture.take() {
            capture.stop();
        }
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
    control: SessionControl,
    operation: tokio::sync::Mutex<()>,
    inner: tokio::sync::Mutex<Option<ActiveStream>>,
    last_latency_change: tokio::sync::Mutex<Option<Instant>>,
    phase: Mutex<(u64, &'static str)>,
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
        let rx = discovery.browse_events().map_err(|e| e.to_string())?;
        *slot = Some(discovery);
        rx
    };

    let app_pump = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(event) = rx.recv().await {
            match event {
                DiscoveryEvent::Resolved(device) => {
                    if let Some(state) = app_pump.try_state::<DiscoveryState>() {
                        if let Ok(mut cache) = state.cache.lock() {
                            cache.insert(device.id.clone(), device.clone());
                        }
                    }
                    let _ = app_pump.emit("airplay://device", &device);
                }
                DiscoveryEvent::Removed(id) => {
                    if let Some(state) = app_pump.try_state::<DiscoveryState>() {
                        if let Ok(mut cache) = state.cache.lock() {
                            if let Some(device) = cache.get_mut(&id) {
                                device.available = false;
                            }
                        }
                    }
                    let _ = app_pump.emit("airplay://device-removed", &id);
                }
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

fn resolve_descriptor(
    app: &tauri::AppHandle,
    device_id: Option<&str>,
    ip: IpAddr,
    port: u16,
    name: &str,
) -> Result<DeviceDescriptor, String> {
    let state = app.state::<DiscoveryState>();
    let cache = state.cache.lock().map_err(|e| e.to_string())?;
    let route = if let Some(id) = device_id {
        cache
            .get(id)
            .filter(|d| d.port == port && d.addresses.contains(&ip))
    } else {
        cache
            .values()
            .find(|d| d.port == port && d.addresses.contains(&ip) && !d.manual)
            .or_else(|| {
                cache
                    .values()
                    .find(|d| d.port == port && d.addresses.contains(&ip))
            })
    };
    if device_id.is_some() && route.is_none() {
        return Err("receiver_unavailable".into());
    }
    match route {
        Some(device) => device
            .descriptor(ip, &cache.values().cloned().collect::<Vec<_>>())
            .map_err(|e| e.to_string()),
        None => Ok(DeviceDescriptor {
            ip,
            port,
            name: name.into(),
            mac: None,
            model: None,
            features: None,
            advertised: None,
        }),
    }
}

async fn prepare_output(
    ip: &str,
    port: u16,
    name: &str,
    volume: f32,
    latency_ms: u32,
    policy: LocalBufferPolicy,
    diagnostics: &SessionRecord,
    app: &tauri::AppHandle,
) -> Result<PreparedLiveStream, String> {
    let parsed: IpAddr = ip.parse().map_err(|e| format!("IP inválida: {e}"))?;
    let descriptor = resolve_descriptor(app, None, parsed, port, name)?;
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
    let metrics = prepared.metrics();
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
        metrics,
        volume_read_supported: None,
    })
}

async fn attach_output(
    active: &mut ActiveStream,
    ip: &str,
    port: u16,
    name: &str,
    volume: f32,
    app: &tauri::AppHandle,
) -> Result<(), String> {
    let prepared = prepare_output(
        ip,
        port,
        name,
        volume,
        active.latency_ms,
        active.policy,
        &active.diagnostics,
        app,
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
    generation: u64,
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
    let _preparation = diagnostics::PreparationGuard::new(diagnostics.clone());
    let prepared = match prepare_output(
        first_ip,
        *first_port,
        first_name,
        *first_volume,
        latency_ms,
        policy,
        &diagnostics,
        &app,
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
    let audio_options = get_audio_options(app.clone())?;
    let (capture, rx) = match tokio::task::spawn_blocking(move || {
        audio_capture::start_loopback_with_device(
            CaptureFormat::AIRPLAY_DEFAULT,
            capture_policy,
            audio_options.device_id,
        )
    })
    .await
    .map_err(|e| e.to_string())?
    {
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
    let pump_app = app.clone();
    let generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(generation));
    let pump_generation = generation.clone();
    let pump = std::thread::Builder::new()
        .name("airplay-pump".into())
        .spawn(move || {
            pump_loop(
                pump_app,
                rx,
                pump_senders,
                sample_rate,
                channels,
                stop_thread,
                pump_diagnostics,
                pump_generation,
            )
        })
        .map_err(|e| {
            diagnostics.lock().unwrap().failed();
            format!("pump thread: {e}")
        })?;
    let mut active = ActiveStream {
        outputs: HashMap::new(),
        senders,
        _capture: Some(capture),
        diagnostics,
        policy,
        pump: Some(pump),
        stop,
        latency_ms,
        sample_rate,
        channels,
        generation,
        group_mode: false,
        target_order: targets
            .iter()
            .map(|(ip, port, _, _)| output_key(ip, *port))
            .collect(),
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
        attach_output(&mut active, ip, *port, name, *volume, &app).await?;
    }
    if active.stop.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("capture_interrupted".into());
    }
    active.diagnostics.lock().unwrap().streaming();
    Ok(active)
}

async fn prepare_targets(
    app: tauri::AppHandle,
    targets: &[(String, u16, String, f32)],
    latency_ms: u32,
    generation: u64,
    group_mode: bool,
) -> Result<ActiveStream, String> {
    if group_mode {
        prepare_synchronized_stream(app, targets, latency_ms, generation).await
    } else {
        prepare_stream(app, targets, latency_ms, generation).await
    }
}

async fn prepare_synchronized_stream(
    app: tauri::AppHandle,
    targets: &[(String, u16, String, f32)],
    latency_ms: u32,
    generation: u64,
) -> Result<ActiveStream, String> {
    if targets.len() != 2 {
        return Err("group_requires_two_receivers".into());
    }
    let policy = if get_experimental_local_buffer(app.clone())? {
        LocalBufferPolicy::LowLatency
    } else {
        LocalBufferPolicy::Stable
    };
    let diagnostics = app.state::<DiagnosticState>().begin(latency_ms, policy);
    let _preparation = diagnostics::PreparationGuard::new(diagnostics.clone());
    let descriptors = targets
        .iter()
        .map(|(ip, port, name, _)| {
            resolve_descriptor(
                &app,
                None,
                ip.parse().map_err(|_| "invalid_ip")?,
                *port,
                name,
            )
        })
        .collect::<Result<Vec<_>, String>>()?;
    let volumes = targets.iter().map(|t| t.3).collect::<Vec<_>>();
    let prepared =
        cap_core::streaming::prepare_group_live_stream(descriptors, &volumes, latency_ms, policy)
            .await
            .map_err(|e| {
                diagnostics.lock().unwrap().record_failure(e.stage_code());
                format!("stream: {e}")
            })?;
    let options = get_audio_options(app.clone())?;
    let capture_policy = if policy == LocalBufferPolicy::LowLatency {
        audio_capture::CapturePolicy::LowLatency
    } else {
        audio_capture::CapturePolicy::Stable
    };
    let began = Instant::now();
    let (capture, rx) = tokio::task::spawn_blocking(move || {
        audio_capture::start_loopback_with_device(
            audio_capture::CaptureFormat::AIRPLAY_DEFAULT,
            capture_policy,
            options.device_id,
        )
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| {
        diagnostics.lock().unwrap().record_failure("capture_start");
        e.to_string()
    })?;
    diagnostics.lock().unwrap().capture_started(began.elapsed());
    let sender = prepared.sender.clone();
    for metrics in &prepared.metrics {
        diagnostics
            .lock()
            .unwrap()
            .add_group_output(sender.clone(), metrics.clone());
    }
    let senders = std::sync::Arc::new(Mutex::new(HashMap::from([(
        output_key(&targets[0].0, targets[0].1),
        sender,
    )])));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let (pump_senders, pump_stop, pump_diagnostics, pump_app) = (
        senders.clone(),
        stop.clone(),
        diagnostics.clone(),
        app.clone(),
    );
    let generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(generation));
    let pump_generation = generation.clone();
    let pump = std::thread::Builder::new()
        .name("airplay-group-pump".into())
        .spawn(move || {
            pump_loop(
                pump_app,
                rx,
                pump_senders,
                44100,
                2,
                pump_stop,
                pump_diagnostics,
                pump_generation,
            )
        })
        .map_err(|e| e.to_string())?;
    let mut active = ActiveStream {
        outputs: HashMap::new(),
        senders,
        _capture: Some(capture),
        diagnostics,
        policy,
        pump: Some(pump),
        stop,
        latency_ms,
        sample_rate: 44100,
        channels: 2,
        generation,
        group_mode: true,
        target_order: targets
            .iter()
            .map(|(ip, port, _, _)| output_key(ip, *port))
            .collect(),
    };
    let handle = prepared.start().await.map_err(|e| {
        active
            .diagnostics
            .lock()
            .unwrap()
            .record_failure(e.stage_code());
        format!("stream: {e}")
    })?;
    for ((connection, heartbeat, metrics), (ip, port, name, volume)) in
        handle.outputs.into_iter().zip(targets)
    {
        active.outputs.insert(
            output_key(ip, *port),
            ActiveOutput {
                connection,
                _heartbeat: heartbeat,
                ip: ip.clone(),
                port: *port,
                name: name.clone(),
                volume: *volume,
                metrics,
                volume_read_supported: None,
            },
        );
    }
    if active.stop.load(std::sync::atomic::Ordering::SeqCst) {
        return Err("capture_interrupted".into());
    }
    active.diagnostics.lock().unwrap().streaming();
    Ok(active)
}

#[tauri::command]
async fn start_group_streaming(
    app: tauri::AppHandle,
    device_ids: Vec<String>,
    stereo_pair: bool,
    latency_ms: u32,
    state: State<'_, StreamingState>,
) -> Result<Vec<StreamingInfo>, String> {
    if device_ids.len() != 2 || device_ids[0] == device_ids[1] {
        return Err("group_requires_two_receivers".into());
    }
    let targets = {
        let discovery = app.state::<DiscoveryState>();
        let cache = discovery.cache.lock().map_err(|e| e.to_string())?;
        let devices = device_ids
            .iter()
            .map(|id| {
                cache
                    .get(id)
                    .filter(|d| d.available)
                    .cloned()
                    .ok_or_else(|| "receiver_unavailable".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if devices[0].receiver_id() == devices[1].receiver_id() {
            return Err("group_requires_two_receivers".into());
        }
        if stereo_pair
            && (devices[0].tight_sync_id.is_none()
                || devices[0].tight_sync_id != devices[1].tight_sync_id)
        {
            return Err("incomplete_stereo_pair".into());
        }
        devices
            .into_iter()
            .map(|d| {
                let ip = d
                    .addresses
                    .iter()
                    .find(|ip| ip.is_ipv4())
                    .or(d.addresses.first())
                    .ok_or_else(|| "receiver_unavailable".to_string())?;
                let preferences = get_receiver_preferences(app.clone(), d.receiver_id())?;
                Ok((ip.to_string(), d.port, d.name, preferences.volume))
            })
            .collect::<Result<Vec<_>, String>>()?
    };
    let mut token = state.control.begin();
    let generation = token.generation;
    let _operation = state.operation.lock().await;
    emit_session(&app, generation, "preparing");
    let prepared = tokio::select! { biased; _ = token.cancelled() => return Err("session_cancelled".into()), result = prepare_synchronized_stream(app.clone(), &targets, latency_ms, generation) => result };
    let mut active = match prepared {
        Ok(value) => value,
        Err(error) => {
            monitor_active(&app, &state, generation).await;
            return Err(error);
        }
    };
    let info = active.outputs.values().map(|o| o.info(44100, 2)).collect();
    let previous = {
        let mut slot = state.inner.lock().await;
        if state.control.current() != generation {
            active.shutdown();
            return Err("session_cancelled".into());
        }
        slot.replace(active)
    };
    if let Some(mut previous) = previous {
        previous.shutdown();
        previous.stop_outputs().await;
    }
    if let Some(active) = state.inner.lock().await.as_ref() {
        let _ = active.apply_mute(&app);
    }
    start_session_monitor(app.clone(), generation);
    emit_session(&app, generation, "streaming");
    Ok(info)
}

#[tauri::command]
async fn start_streaming(
    app: tauri::AppHandle,
    device_id: Option<String>,
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
    if !volume.is_finite() {
        return Err("invalid_volume".into());
    }
    resolve_descriptor(&app, device_id.as_deref(), parsed, port, &name)?;
    let mut token = state.control.begin();
    let _operation = state.operation.lock().await;
    if token.generation != state.control.current() {
        return Err("session_cancelled".into());
    }
    let generation = token.generation;
    emit_session(&app, generation, "preparing");
    let targets = [(parsed.to_string(), port, name, volume)];
    let prepared = tokio::select! {
        biased;
        _ = token.cancelled() => return Err("session_cancelled".into()),
        result = prepare_stream(app.clone(), &targets, latency_ms, generation) => result,
    };
    let mut active = match prepared {
        Ok(value) => value,
        Err(error) => {
            monitor_active(&app, &state, generation).await;
            return Err(error);
        }
    };
    let info = active
        .outputs
        .values()
        .next()
        .unwrap()
        .info(active.sample_rate, active.channels);
    let previous = {
        let mut slot = state.inner.lock().await;
        if generation != state.control.current() {
            active.shutdown();
            return Err("session_cancelled".into());
        }
        slot.replace(active)
    };
    if let Some(mut previous) = previous {
        previous.shutdown();
        previous.stop_outputs().await;
    }
    if let Some(active) = state.inner.lock().await.as_ref() {
        let _ = active.apply_mute(&app);
    }
    start_session_monitor(app.clone(), generation);
    emit_session(&app, generation, "streaming");
    Ok(info)
}

/// Add a receiver to the existing capture without interrupting other outputs.
#[tauri::command]
async fn add_streaming(
    app: tauri::AppHandle,
    device_id: Option<String>,
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
    if !volume.is_finite() {
        return Err("invalid_volume".into());
    }
    resolve_descriptor(&app, device_id.as_deref(), parsed, port, &name)?;
    let mut token = state.control.begin();
    let _operation = state.operation.lock().await;
    if token.generation != state.control.current() {
        return Err("session_cancelled".into());
    }
    let mut slot = state.inner.lock().await;
    let Some(active) = slot.as_mut() else {
        let latency = latency_ms.unwrap_or(cap_core::streaming::DEFAULT_LATENCY_MS);
        let generation = token.generation;
        let targets = [(parsed.to_string(), port, name, volume)];
        drop(slot);
        let stream = tokio::select! { biased; _ = token.cancelled() => return Err("session_cancelled".into()), result = prepare_stream(
            app.clone(),
            &targets,
            latency,
            generation,
        ) => result? };
        let info = stream
            .outputs
            .values()
            .next()
            .unwrap()
            .info(stream.sample_rate, stream.channels);
        let mut slot = state.inner.lock().await;
        if state.control.current() != token.generation {
            return Err("session_cancelled".into());
        }
        *slot = Some(stream);
        drop(slot);
        monitor_active(&app, &state, token.generation).await;
        return Ok(info);
    };
    if active.group_mode {
        drop(slot);
        monitor_active(&app, &state, token.generation).await;
        return Err("stop_before_group_change".into());
    }
    if let Some(requested) = latency_ms {
        if requested != active.latency_ms {
            drop(slot);
            monitor_active(&app, &state, token.generation).await;
            return Err("la latencia no coincide con la reproducción actual".to_string());
        }
    }
    let key = output_key(&parsed.to_string(), port);
    if let Some(output) = active.outputs.get(&key) {
        let info = output.info(active.sample_rate, active.channels);
        drop(slot);
        monitor_active(&app, &state, token.generation).await;
        return Ok(info);
    }
    let address = parsed.to_string();
    let attached = tokio::select! {
        biased;
        _ = token.cancelled() => Err("session_cancelled".into()),
        result = attach_output(active, &address, port, &name, volume, &app) => result,
    };
    if let Err(error) = attached {
        active.senders.lock().unwrap().remove(&key);
        drop(slot);
        monitor_active(&app, &state, token.generation).await;
        return Err(error);
    }

    active.target_order.push(key.clone());
    let info = active
        .outputs
        .get(&key)
        .unwrap()
        .info(active.sample_rate, active.channels);
    drop(slot);
    monitor_active(&app, &state, token.generation).await;
    Ok(info)
}

fn pump_loop(
    _app: tauri::AppHandle,
    rx: audio_capture::CaptureReceiver,
    senders: std::sync::Arc<Mutex<HashMap<String, cap_core::streaming::LiveFrameSender>>>,
    sample_rate: u32,
    channels: u8,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    diagnostics: SessionRecord,
    generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
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
        stop.store(true, Ordering::SeqCst);
        diagnostics.lock().unwrap().record_failure("capture");
        diagnostics.lock().unwrap().failed();
        tracing::warn!(
            generation = generation.load(Ordering::SeqCst),
            "capture stopped unexpectedly; session monitor will recover or report the failure"
        );
    } else {
        tracing::info!("airplay-pump thread exit");
    }
}

#[tauri::command]
async fn stop_streaming(
    app: tauri::AppHandle,
    state: State<'_, StreamingState>,
) -> Result<(), String> {
    state.control.cancel();
    emit_session(&app, state.control.current(), "disconnected");
    let active = state.inner.lock().await.take();
    if let Some(mut active) = active {
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
    if active.group_mode {
        let mut group = slot.take().unwrap();
        drop(slot);
        state.control.cancel();
        group.shutdown();
        group.stop_outputs().await;
        return Ok(());
    }
    let key = output_key(&ip, port);
    active
        .senders
        .lock()
        .map_err(|e| e.to_string())?
        .remove(&key);
    active.target_order.retain(|target| target != &key);
    if let Some(mut output) = active.outputs.remove(&key) {
        output._heartbeat.shutdown();
        let _ = tokio::time::timeout(Duration::from_secs(3), async {
            output.connection.lock().await.disconnect().await
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
async fn set_stream_volume(
    app: tauri::AppHandle,
    volume: f32,
    state: State<'_, StreamingState>,
) -> Result<f32, String> {
    let mut slot = state.inner.lock().await;
    let active = slot
        .as_mut()
        .ok_or_else(|| "no hay streaming activo".to_string())?;
    if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
        return Err("invalid_volume".into());
    }
    let v = volume;
    let mut errors = Vec::new();
    for output in active.outputs.values_mut() {
        let mut conn = output.connection.lock().await;
        match tokio::time::timeout(Duration::from_secs(3), conn.set_volume(v)).await {
            Ok(Ok(())) => {
                output.volume = v;
                let receiver_id = {
                    let discovery = app.state::<DiscoveryState>();
                    let cache = discovery.cache.lock().unwrap();
                    cache
                        .values()
                        .find(|d| {
                            d.port == output.port
                                && d.addresses.iter().any(|a| a.to_string() == output.ip)
                        })
                        .map(|d| d.receiver_id())
                        .unwrap_or_else(|| format!("manual://{}:{}", output.ip, output.port))
                };
                if let Err(error) = patch_receiver_preferences(
                    app.clone(),
                    receiver_id,
                    ReceiverPreferencesPatch {
                        volume: Some(v),
                        latency_ms: None,
                        reconnect: None,
                    },
                ) {
                    errors.push(error);
                }
            }
            Ok(Err(error)) => errors.push(format!("{}: {error}", output.name)),
            Err(_) => errors.push(format!("{}: volume_timeout", output.name)),
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
    app: tauri::AppHandle,
    device_id: Option<String>,
    ip: String,
    port: Option<u16>,
    name: Option<String>,
    state: State<'_, ConnectionState>,
) -> Result<ConnectionInfo, String> {
    let parsed: IpAddr = ip.parse().map_err(|e| format!("IP inválida '{ip}': {e}"))?;
    let port = port.unwrap_or(7000);
    let display_name = name.clone().unwrap_or_else(|| format!("HomePod {parsed}"));

    let descriptor = resolve_descriptor(&app, device_id.as_deref(), parsed, port, &display_name)?;
    // Keep the previous pairing until the new target succeeds.
    let mut token = state.control.begin();
    let mut slot = state.inner.lock().await;
    let session = tokio::select! {
        biased;
        _ = token.cancelled() => return Err("session_cancelled".into()),
        result = pair_homepod(descriptor) => result.map_err(|e| format!("pairing falló: {e}"))?,
    };
    if let Some(mut previous) = slot.replace(session) {
        let _ =
            tokio::time::timeout(Duration::from_secs(3), previous.connection.disconnect()).await;
    }

    Ok(ConnectionInfo {
        ip: parsed.to_string(),
        port,
        name: display_name,
    })
}

/// Suelta la sesión RTSP actual (si hay).
#[tauri::command]
async fn disconnect_device(state: State<'_, ConnectionState>) -> Result<(), String> {
    state.control.cancel();
    let mut slot = state.inner.lock().await;
    if let Some(mut session) = slot.take() {
        let _ = tokio::time::timeout(Duration::from_secs(3), session.connection.disconnect()).await;
    }
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
    device.server_header = probe.server_header;

    if let Ok(mut cache) = state.cache.lock() {
        cache.insert(device.id.clone(), device.clone());
    }

    app.emit("airplay://device", &device)
        .map_err(|e| e.to_string())?;

    Ok(device)
}

#[tauri::command]
async fn list_audio_outputs() -> Result<Vec<audio_capture::AudioEndpoint>, String> {
    tokio::task::spawn_blocking(audio_capture::list_audio_outputs)
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
}
#[tauri::command]
fn get_audio_options(app: tauri::AppHandle) -> Result<AudioOptions, String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    store
        .get("audio_options")
        .map(serde_json::from_value)
        .transpose()
        .map_err(|e| e.to_string())
        .map(|v| v.unwrap_or_default())
}
#[tauri::command]
async fn save_audio_options(
    app: tauri::AppHandle,
    options: AudioOptions,
    state: State<'_, StreamingState>,
) -> Result<(), String> {
    let _operation = state.operation.lock().await;
    if state.inner.lock().await.is_some() {
        return Err("stop_before_source_change".into());
    }
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    store.set(
        "audio_options",
        serde_json::to_value(options).map_err(|e| e.to_string())?,
    );
    store.save().map_err(|e| e.to_string())
}
#[tauri::command]
async fn get_session_status(state: State<'_, StreamingState>) -> Result<serde_json::Value, String> {
    let stream = state.inner.lock().await;
    Ok(match stream.as_ref() {
        Some(active) => {
            serde_json::json!({ "generation": active.generation.load(std::sync::atomic::Ordering::SeqCst), "status": "streaming", "mute_status": active._capture.as_ref().map_or("off", |c| c.mute_status()), "group_mode": active.group_mode })
        }
        None => {
            let phase = state.phase.lock().unwrap();
            let status = if phase.1.is_empty() {
                "disconnected"
            } else {
                phase.1
            };
            serde_json::json!({ "generation": state.control.current(), "status": status, "mute_status": "off" })
        }
    })
}
#[tauri::command]
fn get_receiver_preferences(
    app: tauri::AppHandle,
    receiver_id: String,
) -> Result<ReceiverPreferences, String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    if let Some(value) = store.get(format!("receiver:{receiver_id}")) {
        return serde_json::from_value(value).map_err(|e| e.to_string());
    }
    // Lazy migration: retain the user's existing global settings for first use.
    Ok(ReceiverPreferences {
        volume: get_volume(app.clone())?.unwrap_or(0.2),
        latency_ms: get_latency(app)?.unwrap_or(3000),
        reconnect: false,
    })
}
#[tauri::command]
fn save_receiver_preferences(
    app: tauri::AppHandle,
    receiver_id: String,
    preferences: ReceiverPreferences,
) -> Result<(), String> {
    preferences.validate()?;
    if receiver_id.len() > 512 {
        return Err("invalid_receiver_id".into());
    }
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    store.set(
        format!("receiver:{receiver_id}"),
        serde_json::to_value(preferences).map_err(|e| e.to_string())?,
    );
    store.save().map_err(|e| e.to_string())
}
#[tauri::command]
fn patch_receiver_preferences(
    app: tauri::AppHandle,
    receiver_id: String,
    patch: ReceiverPreferencesPatch,
) -> Result<(), String> {
    let _guard = RECEIVER_SETTINGS_LOCK.lock().map_err(|e| e.to_string())?;
    let mut preferences = get_receiver_preferences(app.clone(), receiver_id.clone())?;
    patch.apply(&mut preferences);
    save_receiver_preferences(app, receiver_id, preferences)
}

#[tauri::command]
fn open_latest_release(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_shell::ShellExt;
    app.shell()
        .open("https://github.com/Pabldi08/AirSend/releases/latest", None)
        .map_err(|e| e.to_string())
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
    let mut token = state.control.begin();
    let generation = token.generation;
    let _operation = state.operation.lock().await;
    if generation != state.control.current() {
        return Err("session_cancelled".into());
    }
    let mut changed = state.last_latency_change.lock().await;
    let remaining = latency_cooldown_remaining(*changed, Instant::now());
    if !remaining.is_zero() {
        monitor_active(&app, &state, generation).await;
        return Err(format!("latency_cooldown:{}", remaining.as_millis()));
    }

    let previous = state
        .inner
        .lock()
        .await
        .as_ref()
        .map(|s| s.latency_ms)
        .unwrap_or(get_latency(app.clone())?.unwrap_or(cap_core::streaming::DEFAULT_LATENCY_MS));
    if previous == latency_ms {
        monitor_active(&app, &state, generation).await;
        return Ok(false);
    }

    let mut slot = state.inner.lock().await;
    let restarted = if let Some(mut stream) = slot.take() {
        let targets = stream
            .target_order
            .iter()
            .filter_map(|key| stream.outputs.get(key))
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
        let group_mode = stream.group_mode;
        if let Err(error) = save_latency(&app, latency_ms) {
            *slot = Some(stream);
            return Err(error);
        }
        // A receiver may reject a second RTSP session to itself. Close the old
        // sessions before reopening them, then restore the old setup on error.
        stream.shutdown();
        stream.stop_outputs().await;
        drop(stream);
        match tokio::select! { biased; _ = token.cancelled() => Err("session_cancelled".into()), result = prepare_targets(app.clone(), &targets, latency_ms, generation, group_mode) => result }
        {
            Ok(replacement) => *slot = Some(replacement),
            Err(error) => {
                let _ = save_latency(&app, previous);
                if token.generation != state.control.current() {
                    return Err(error);
                }
                let restored = tokio::select! { biased; _ = token.cancelled() => return Err("session_cancelled".into()), result = prepare_targets(
                    app.clone(),
                    &targets,
                    old_latency_ms,
                    generation,
                    group_mode,
                ) => result };
                if let Ok(restored) = restored {
                    *slot = Some(restored);
                }
                drop(slot);
                monitor_active(&app, &state, generation).await;
                return Err(error);
            }
        }
        true
    } else {
        save_latency(&app, latency_ms)?;
        false
    };

    drop(slot);
    monitor_active(&app, &state, generation).await;
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
            "quit" => {
                let app = app.clone();
                tauri::async_runtime::spawn(async move {
                    let _ = stop_streaming(app.clone(), app.state::<StreamingState>()).await;
                    let _ = disconnect_device(app.state::<ConnectionState>()).await;
                    app.exit(0);
                });
            }
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

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // `_log_guard` debe vivir todo el programa para que el writer non-blocking
    // siga drenando logs al archivo. Lo "olvidamos" con un binding mut a static.
    let _log_guard = init_tracing();

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
            start_group_streaming,
            get_receiver_volumes,
            set_receiver_volume,
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
            list_audio_outputs,
            get_audio_options,
            save_audio_options,
            get_session_status,
            get_receiver_preferences,
            save_receiver_preferences,
            patch_receiver_preferences,
            open_latest_release,
        ])
        .setup(|app| {
            let journal = app.path().app_data_dir()?.join("local-mute-recovery.json");
            // Recovery runs before the window can start another mute session.
            match std::thread::spawn(move || audio_capture::recover_local_mute(&journal)).join() {
                Ok(Ok(())) => {}
                result => tracing::warn!(?result, "local mute recovery remains pending"),
            }
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

fn emit_session(app: &tauri::AppHandle, generation: u64, code: &'static str) {
    let state = app.state::<StreamingState>();
    let mut phase = state.phase.lock().unwrap();
    if generation < phase.0 {
        return;
    }
    *phase = (generation, code);
    let _ = app.emit(
        "airplay://session",
        SessionEvent {
            generation,
            code: code.into(),
        },
    );
}

async fn monitor_active(app: &tauri::AppHandle, state: &StreamingState, generation: u64) {
    let slot = state.inner.lock().await;
    if state.control.current() != generation {
        return;
    }
    if let Some(active) = slot.as_ref() {
        active
            .generation
            .store(generation, std::sync::atomic::Ordering::SeqCst);
        if active
            ._capture
            .as_ref()
            .is_some_and(|c| c.mute_status() == "off")
        {
            if let Err(error) = active.apply_mute(app) {
                tracing::warn!(%error, "local mute unavailable");
            }
        }
        start_session_monitor(app.clone(), generation);
        emit_session(app, generation, "streaming");
    } else {
        emit_session(app, generation, "disconnected");
    }
}

fn start_session_monitor(app: tauri::AppHandle, generation: u64) {
    tauri::async_runtime::spawn(async move {
        let mut last_send_errors = 0;
        let mut failed_sends = 0;
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let state = app.state::<StreamingState>();
            let reason = {
                let slot = state.inner.lock().await;
                let Some(active) = slot.as_ref().filter(|s| {
                    s.generation.load(std::sync::atomic::Ordering::SeqCst) == generation
                }) else {
                    return;
                };
                let sends = active.diagnostics.lock().unwrap().send_errors();
                if sends > last_send_errors {
                    failed_sends += 1;
                } else {
                    failed_sends = 0;
                }
                last_send_errors = sends;
                if active.stop.load(std::sync::atomic::Ordering::SeqCst) {
                    Some("capture_interrupted")
                } else if active.outputs.values().any(|o| o.metrics.unhealthy()) {
                    Some("feedback_failed")
                } else if failed_sends >= 3 {
                    Some("transport_failed")
                } else {
                    None
                }
            };
            let Some(reason) = reason else {
                continue;
            };
            let mut failed = {
                let mut slot = state.inner.lock().await;
                if slot.as_ref().is_none_or(|s| {
                    s.generation.load(std::sync::atomic::Ordering::SeqCst) != generation
                }) {
                    return;
                }
                slot.take().unwrap()
            };
            let targets = failed
                .target_order
                .iter()
                .filter_map(|key| failed.outputs.get(key))
                .map(|o| (o.ip.clone(), o.port, o.name.clone(), o.volume))
                .collect::<Vec<_>>();
            let latency_ms = failed.latency_ms;
            let group_mode = failed.group_mode;
            failed.diagnostics.lock().unwrap().record_failure(reason);
            failed.shutdown();
            failed.stop_outputs().await;
            drop(failed);
            // A user-requested replacement/Stop always takes priority over automatic recovery.
            if state.control.current() != generation {
                return;
            }
            let reconnect = targets.iter().any(|(ip, port, _, _)| {
                let discovery = app.state::<DiscoveryState>();
                let cache = discovery.cache.lock().unwrap();
                let id = cache
                    .values()
                    .find(|d| d.port == *port && d.addresses.iter().any(|a| a.to_string() == *ip))
                    .map(|d| d.receiver_id())
                    .unwrap_or_else(|| format!("manual://{ip}:{port}"));
                let pair_reconnect = group_mode
                    && cache
                        .values()
                        .find(|d| {
                            d.port == *port && d.addresses.iter().any(|a| a.to_string() == *ip)
                        })
                        .and_then(|d| d.tight_sync_id.as_ref())
                        .is_some_and(|id| {
                            get_receiver_preferences(app.clone(), format!("stereo:{id}"))
                                .is_ok_and(|p| p.reconnect)
                        });
                pair_reconnect
                    || get_receiver_preferences(app.clone(), id).is_ok_and(|p| p.reconnect)
            });
            if reconnect {
                let mut token = state.control.subscribe(generation);
                emit_session(&app, generation, "recovering");
                for delay in [1, 2, 4] {
                    let restored = tokio::select! {
                        biased;
                        _ = token.cancelled() => return,
                        result = async {
                            tokio::time::sleep(Duration::from_secs(delay)).await;
                            prepare_targets(app.clone(), &targets, latency_ms, generation, group_mode).await
                        } => result,
                    };
                    match restored {
                        Ok(mut stream) => {
                            let mut slot = state.inner.lock().await;
                            if state.control.current() != generation {
                                stream.shutdown();
                                return;
                            }
                            let _ = stream.apply_mute(&app);
                            *slot = Some(stream);
                            emit_session(&app, generation, "streaming");
                            last_send_errors = 0;
                            failed_sends = 0;
                            break;
                        }
                        Err(error) => {
                            // Authentication/configuration failures require user action.
                            if !session::recoverable_failure(&error) {
                                break;
                            }
                        }
                    }
                }
                if state.inner.lock().await.is_some() {
                    continue;
                }
            }
            let _ = app.emit(
                "airplay://error",
                SessionEvent {
                    generation,
                    code: reason.into(),
                },
            );
            return;
        }
    });
}

#[derive(serde::Serialize)]
struct ReceiverVolume {
    ip: String,
    port: u16,
    volume: f32,
    readable: bool,
}
#[tauri::command]
async fn get_receiver_volumes(
    state: State<'_, StreamingState>,
) -> Result<Vec<ReceiverVolume>, String> {
    let mut slot = state.inner.lock().await;
    let Some(active) = slot.as_mut() else {
        return Ok(Vec::new());
    };
    let mut values = Vec::new();
    for output in active.outputs.values_mut() {
        let Ok(mut connection) = output.connection.try_lock() else {
            values.push(ReceiverVolume {
                ip: output.ip.clone(),
                port: output.port,
                volume: output.volume,
                readable: output.volume_read_supported.unwrap_or(false),
            });
            continue;
        };
        let volume = if output.volume_read_supported == Some(false) {
            None
        } else {
            match tokio::time::timeout(Duration::from_secs(2), connection.read_volume()).await {
                Ok(Ok(Some(v))) => {
                    output.volume_read_supported = Some(true);
                    Some(v)
                }
                _ => {
                    output.volume_read_supported = Some(false);
                    None
                }
            }
        };
        if let Some(volume) = volume {
            output.volume = volume;
        }
        values.push(ReceiverVolume {
            ip: output.ip.clone(),
            port: output.port,
            volume: output.volume,
            readable: volume.is_some(),
        });
    }
    Ok(values)
}
#[tauri::command]
async fn set_receiver_volume(
    app: tauri::AppHandle,
    ip: String,
    port: u16,
    volume: f32,
    state: State<'_, StreamingState>,
) -> Result<f32, String> {
    if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
        return Err("invalid_volume".into());
    }
    let mut slot = state.inner.lock().await;
    let output = slot
        .as_mut()
        .and_then(|s| s.outputs.get_mut(&output_key(&ip, port)))
        .ok_or("receiver_not_streaming")?;
    tokio::time::timeout(
        Duration::from_secs(3),
        output.connection.lock().await.set_volume(volume),
    )
    .await
    .map_err(|_| "volume_timeout")?
    .map_err(|e| e.to_string())?;
    output.volume = volume;
    let receiver_id = {
        let discovery = app.state::<DiscoveryState>();
        let cache = discovery.cache.lock().unwrap();
        cache
            .values()
            .find(|d| d.port == port && d.addresses.iter().any(|a| a.to_string() == ip))
            .map(|d| d.receiver_id())
            .unwrap_or_else(|| format!("manual://{ip}:{port}"))
    };
    patch_receiver_preferences(
        app,
        receiver_id,
        ReceiverPreferencesPatch {
            volume: Some(volume),
            latency_ms: None,
            reconnect: None,
        },
    )?;
    Ok(volume)
}
