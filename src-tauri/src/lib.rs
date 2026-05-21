use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Duration;

use cap_core::{
    browse_once,
    pairing::{pair_homepod, DeviceDescriptor, PairedSession},
    probe::manual_device,
    probe_airplay, Device, Discovery,
};
use tauri::{Emitter, State};
use tracing_subscriber::EnvFilter;

/// Mantiene el `Discovery` activo entre llamadas para no recrear el daemon
/// (cada uno abre montones de sockets en 5353 — sin esto, cada hot-reload
/// del frontend filtraba un daemon nuevo).
#[derive(Default)]
struct DiscoveryState {
    inner: Mutex<Option<Discovery>>,
}

/// Mantiene la sesión RTSP autenticada con el HomePod. Por ahora, sólo una a la
/// vez (alcance MVP: un solo HomePod). El Connection no es Send/Sync trivial
/// porque tiene streams TCP/UDP; usamos un Mutex async para acceso seguro.
#[derive(Default)]
struct ConnectionState {
    inner: tokio::sync::Mutex<Option<PairedSession>>,
}

/// Streaming activo: connection (para set_volume) + capture + hilo bombeador
/// + guard del heartbeat (al dropearse, aborta la tarea de feedback RTSP).
struct ActiveStream {
    connection: std::sync::Arc<tokio::sync::Mutex<cap_core::streaming::Connection>>,
    _heartbeat: cap_core::streaming::HeartbeatGuard,
    capture: Box<dyn audio_capture::Capture>,
    pump: Option<std::thread::JoinHandle<()>>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ip: String,
    sample_rate: u32,
    channels: u8,
}

impl ActiveStream {
    fn shutdown(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(h) = self.pump.take() {
            let _ = h.join();
        }
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
    // Lock + spawn + drop del guard antes de devolver. Sin await durante el lock,
    // así el std::sync::Mutex es seguro aunque la fn sea async.
    let mut rx = {
        let mut slot = state.inner.lock().map_err(|e| e.to_string())?;
        if slot.is_some() {
            tracing::debug!("discovery ya en curso, ignoro start_discovery_stream");
            return Ok(());
        }
        let discovery = Discovery::new().map_err(|e| e.to_string())?;
        let rx = discovery.browse().map_err(|e| e.to_string())?;
        *slot = Some(discovery);
        rx
    };

    tauri::async_runtime::spawn(async move {
        while let Some(device) = rx.recv().await {
            if app.emit("airplay://device", &device).is_err() {
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
    name: String,
    sample_rate: u32,
    channels: u8,
    volume: f32,
}

/// Arranca captura del audio del sistema y la envía al HomePod indicado.
/// Si había un streaming activo, lo para antes de abrir el nuevo.
#[tauri::command]
async fn start_streaming(
    app: tauri::AppHandle,
    ip: String,
    port: Option<u16>,
    name: Option<String>,
    volume: Option<f32>,
    state: State<'_, StreamingState>,
) -> Result<StreamingInfo, String> {
    use audio_capture::CaptureFormat;
    use cap_core::streaming::open_live_stream;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    let parsed: std::net::IpAddr = ip.parse().map_err(|e| format!("IP inválida: {e}"))?;
    let port = port.unwrap_or(7000);
    let display_name = name.clone().unwrap_or_else(|| format!("HomePod {parsed}"));

    // Cerrar cualquier stream previo.
    {
        let mut slot = state.inner.lock().await;
        if let Some(mut prev) = slot.take() {
            prev.shutdown();
        }
    }

    // Captura del sistema.
    let (capture, rx) = audio_capture::start_loopback(CaptureFormat::AIRPLAY_DEFAULT)
        .map_err(|e| format!("captura: {e}"))?;

    // Pair + setup + start_streaming_live + set volumen.
    let descriptor = DeviceDescriptor {
        ip: parsed,
        port,
        name: display_name.clone(),
        mac: None,
        model: None,
        features: None,
    };
    let stream_handle = open_live_stream(descriptor, volume)
        .await
        .map_err(|e| format!("stream: {e}"))?;
    let (sender, connection, heartbeat, sample_rate, channels) = stream_handle.into_parts();

    // Hilo bombeador. Le pasamos un clone del AppHandle para que pueda emitir
    // `airplay://error` si la captura muere inesperadamente (device removed,
    // parec/wasapi cierra el canal, etc.) — la UI lo escucha y muestra toast.
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let app_pump = app.clone();
    let pump = std::thread::Builder::new()
        .name("airplay-pump".into())
        .spawn(move || pump_loop(app_pump, rx, sender, sample_rate, channels, stop_thread))
        .map_err(|e| format!("pump thread: {e}"))?;

    let active = ActiveStream {
        connection,
        _heartbeat: heartbeat,
        capture,
        pump: Some(pump),
        stop,
        ip: parsed.to_string(),
        sample_rate,
        channels,
    };
    let info = StreamingInfo {
        ip: active.ip.clone(),
        name: display_name,
        sample_rate,
        channels,
        volume: volume.unwrap_or(cap_core::streaming::DEFAULT_INITIAL_VOLUME),
    };

    let mut slot = state.inner.lock().await;
    *slot = Some(active);

    Ok(info)
}

fn pump_loop(
    app: tauri::AppHandle,
    rx: crossbeam_channel::Receiver<audio_capture::CapturedFrame>,
    sender: cap_core::streaming::LiveFrameSender,
    sample_rate: u32,
    channels: u8,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use cap_core::streaming::LivePcmFrame;
    use std::sync::atomic::Ordering;
    use std::time::Duration;

    let mut unexpected_exit = false;
    while !stop.load(Ordering::SeqCst) {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(frame) => {
                let _ = sender.try_send(LivePcmFrame {
                    samples: frame.samples,
                    channels,
                    sample_rate,
                });
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
            Err(_) => {
                // El canal de captura se cerró sin que nosotros pidamos shutdown:
                // device removed, driver crash, parec mató al subproceso, etc.
                unexpected_exit = true;
                break;
            }
        }
    }
    if unexpected_exit {
        tracing::warn!("airplay-pump: canal captura cerrado inesperadamente, emitiendo error");
        let _ = app.emit(
            "airplay://error",
            "captura del audio interrumpida (dispositivo desconectado o driver caído)",
        );
    } else {
        tracing::info!("airplay-pump thread exit");
    }
}

#[tauri::command]
async fn stop_streaming(state: State<'_, StreamingState>) -> Result<(), String> {
    let mut slot = state.inner.lock().await;
    if let Some(mut active) = slot.take() {
        active.shutdown();
    }
    Ok(())
}

#[tauri::command]
async fn set_stream_volume(
    volume: f32,
    state: State<'_, StreamingState>,
) -> Result<f32, String> {
    let mut slot = state.inner.lock().await;
    let active = slot
        .as_mut()
        .ok_or_else(|| "no hay streaming activo".to_string())?;
    let v = volume.clamp(0.0, 1.0);
    let mut conn = active.connection.lock().await;
    conn.set_volume(v).await.map_err(|e| e.to_string())?;
    Ok(v)
}

#[tauri::command]
async fn is_streaming(state: State<'_, StreamingState>) -> Result<Option<StreamingInfo>, String> {
    let slot = state.inner.lock().await;
    Ok(slot.as_ref().map(|a| StreamingInfo {
        ip: a.ip.clone(),
        name: format!("HomePod {}", a.ip),
        sample_rate: a.sample_rate,
        channels: a.channels,
        volume: 0.0, // volumen actual no lo retenemos (TODO: cache)
    }))
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

    // Si ya había una sesión previa, la soltamos antes de abrir otra.
    {
        let mut slot = state.inner.lock().await;
        *slot = None;
    }

    let session = pair_homepod(descriptor)
        .await
        .map_err(|e| format!("pairing falló: {e}"))?;

    let mut slot = state.inner.lock().await;
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
) -> Result<Device, String> {
    let parsed: IpAddr = ip.parse().map_err(|e| format!("IP inválida '{ip}': {e}"))?;
    let port = port.unwrap_or(cap_core::probe::DEFAULT_AIRPLAY_PORT);

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

    app.emit("airplay://device", &device)
        .map_err(|e| e.to_string())?;

    Ok(device)
}

/// Localización de los logs por plataforma. Windows usa %APPDATA% (igual que
/// hace Tauri por defecto para `app_log_dir` con el bundle id de la app).
/// Si no se puede resolver, devolvemos None y los logs quedan solo en stdout.
fn resolve_log_dir() -> Option<std::path::PathBuf> {
    const APP_DIRNAME: &str = "ConexionAirPlay";
    #[cfg(windows)]
    {
        let base = std::env::var_os("APPDATA")?;
        Some(std::path::PathBuf::from(base).join(APP_DIRNAME).join("logs"))
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
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(DiscoveryState::default())
        .manage(ConnectionState::default())
        .manage(StreamingState::default())
        .invoke_handler(tauri::generate_handler![
            discover_devices,
            start_discovery_stream,
            stop_discovery_stream,
            add_manual_device,
            connect_device,
            disconnect_device,
            is_connected,
            start_streaming,
            stop_streaming,
            set_stream_volume,
            is_streaming
        ])
        .setup(|app| {
            let _ = app.handle();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");

    // `_log_guard` se dropea aquí al salir, drenando los últimos logs.
    drop(_log_guard);
}
