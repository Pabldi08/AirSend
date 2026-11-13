use airsend_engine::host::init_tracing;
use airsend_engine::preferences::{AudioOptions, ReceiverPreferences, ReceiverPreferencesPatch};
use airsend_engine::{AppHandle, ConnectionInfo, LastDevice, ReceiverVolume, StreamingInfo};
use cap_core::Device;
use tauri::menu::{MenuBuilder, MenuItem, MenuItemBuilder};
use tauri::tray::{TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, State, WindowEvent};

#[tauri::command]
async fn discover_devices(timeout_ms: Option<u64>) -> Result<Vec<Device>, String> {
    airsend_engine::discover_devices(timeout_ms).await
}

#[tauri::command]
async fn start_discovery_stream(engine: State<'_, AppHandle>) -> Result<(), String> {
    airsend_engine::start_discovery_stream(engine.inner().clone(), engine.inner().discovery()).await
}

#[tauri::command]
async fn stop_discovery_stream(engine: State<'_, AppHandle>) -> Result<(), String> {
    airsend_engine::stop_discovery_stream(engine.inner().discovery()).await
}

#[tauri::command]
async fn start_group_streaming(
    engine: State<'_, AppHandle>,
    device_ids: Vec<String>,
    stereo_pair: bool,
    latency_ms: u32,
) -> Result<Vec<StreamingInfo>, String> {
    airsend_engine::start_group_streaming(
        engine.inner().clone(),
        device_ids,
        stereo_pair,
        latency_ms,
        engine.inner().streaming(),
    )
    .await
}

#[tauri::command]
async fn start_streaming(
    engine: State<'_, AppHandle>,
    device_id: Option<String>,
    ip: String,
    port: Option<u16>,
    name: Option<String>,
    volume: Option<f32>,
    latency_ms: Option<u32>,
) -> Result<StreamingInfo, String> {
    airsend_engine::start_streaming(
        engine.inner().clone(),
        device_id,
        ip,
        port,
        name,
        volume,
        latency_ms,
        engine.inner().streaming(),
    )
    .await
}

#[tauri::command]
async fn add_streaming(
    engine: State<'_, AppHandle>,
    device_id: Option<String>,
    ip: String,
    port: Option<u16>,
    name: Option<String>,
    volume: Option<f32>,
    latency_ms: Option<u32>,
) -> Result<StreamingInfo, String> {
    airsend_engine::add_streaming(
        engine.inner().clone(),
        device_id,
        ip,
        port,
        name,
        volume,
        latency_ms,
        engine.inner().streaming(),
    )
    .await
}

#[tauri::command]
async fn stop_streaming(engine: State<'_, AppHandle>) -> Result<(), String> {
    airsend_engine::stop_streaming(engine.inner().clone(), engine.inner().streaming()).await
}

#[tauri::command]
async fn remove_streaming(
    engine: State<'_, AppHandle>,
    ip: String,
    port: u16,
) -> Result<(), String> {
    airsend_engine::remove_streaming(ip, port, engine.inner().streaming()).await
}

#[tauri::command]
async fn set_stream_volume(engine: State<'_, AppHandle>, volume: f32) -> Result<f32, String> {
    airsend_engine::set_stream_volume(engine.inner().clone(), volume, engine.inner().streaming())
        .await
}

#[tauri::command]
async fn is_streaming(engine: State<'_, AppHandle>) -> Result<Option<StreamingInfo>, String> {
    airsend_engine::is_streaming(engine.inner().streaming()).await
}

#[tauri::command]
async fn list_streaming(engine: State<'_, AppHandle>) -> Result<Vec<StreamingInfo>, String> {
    airsend_engine::list_streaming(engine.inner().streaming()).await
}

#[tauri::command]
async fn connect_device(
    engine: State<'_, AppHandle>,
    device_id: Option<String>,
    ip: String,
    port: Option<u16>,
    name: Option<String>,
) -> Result<ConnectionInfo, String> {
    airsend_engine::connect_device(
        engine.inner().clone(),
        device_id,
        ip,
        port,
        name,
        engine.inner().connection(),
    )
    .await
}

#[tauri::command]
async fn disconnect_device(engine: State<'_, AppHandle>) -> Result<(), String> {
    airsend_engine::disconnect_device(engine.inner().connection()).await
}

#[tauri::command]
async fn is_connected(engine: State<'_, AppHandle>) -> Result<bool, String> {
    airsend_engine::is_connected(engine.inner().connection()).await
}

#[tauri::command]
async fn add_manual_device(
    engine: State<'_, AppHandle>,
    ip: String,
    port: Option<u16>,
    name: Option<String>,
) -> Result<Device, String> {
    airsend_engine::add_manual_device(
        engine.inner().clone(),
        ip,
        port,
        name,
        engine.inner().discovery(),
    )
    .await
}

#[tauri::command]
async fn list_audio_outputs() -> Result<Vec<airsend_engine::AudioEndpoint>, String> {
    airsend_engine::list_audio_outputs().await
}

#[tauri::command]
fn get_audio_options(engine: State<'_, AppHandle>) -> Result<AudioOptions, String> {
    airsend_engine::get_audio_options(engine.inner().clone())
}

#[tauri::command]
async fn save_audio_options(
    engine: State<'_, AppHandle>,
    options: AudioOptions,
) -> Result<(), String> {
    airsend_engine::save_audio_options(engine.inner().clone(), options, engine.inner().streaming())
        .await
}

#[tauri::command]
async fn get_session_status(engine: State<'_, AppHandle>) -> Result<serde_json::Value, String> {
    airsend_engine::get_session_status(engine.inner().streaming()).await
}

#[tauri::command]
fn get_receiver_preferences(
    engine: State<'_, AppHandle>,
    receiver_id: String,
) -> Result<ReceiverPreferences, String> {
    airsend_engine::get_receiver_preferences(engine.inner().clone(), receiver_id)
}

#[tauri::command]
fn save_receiver_preferences(
    engine: State<'_, AppHandle>,
    receiver_id: String,
    preferences: ReceiverPreferences,
) -> Result<(), String> {
    airsend_engine::save_receiver_preferences(engine.inner().clone(), receiver_id, preferences)
}

#[tauri::command]
fn patch_receiver_preferences(
    engine: State<'_, AppHandle>,
    receiver_id: String,
    patch: ReceiverPreferencesPatch,
) -> Result<(), String> {
    airsend_engine::patch_receiver_preferences(engine.inner().clone(), receiver_id, patch)
}

#[tauri::command]
fn save_last_device(
    engine: State<'_, AppHandle>,
    ip: String,
    port: u16,
    name: String,
) -> Result<(), String> {
    airsend_engine::save_last_device(engine.inner().clone(), ip, port, name)
}

#[tauri::command]
fn get_last_device(engine: State<'_, AppHandle>) -> Result<Option<LastDevice>, String> {
    airsend_engine::get_last_device(engine.inner().clone())
}

#[tauri::command]
fn clear_last_device(engine: State<'_, AppHandle>) -> Result<(), String> {
    airsend_engine::clear_last_device(engine.inner().clone())
}

#[tauri::command]
fn save_volume(engine: State<'_, AppHandle>, volume: f32) -> Result<(), String> {
    airsend_engine::save_volume(engine.inner().clone(), volume)
}

#[tauri::command]
fn get_volume(engine: State<'_, AppHandle>) -> Result<Option<f32>, String> {
    airsend_engine::get_volume(engine.inner().clone())
}

#[tauri::command]
fn save_multi_device(engine: State<'_, AppHandle>, enabled: bool) -> Result<(), String> {
    airsend_engine::save_multi_device(engine.inner().clone(), enabled)
}

#[tauri::command]
fn get_multi_device(engine: State<'_, AppHandle>) -> Result<bool, String> {
    airsend_engine::get_multi_device(engine.inner().clone())
}

#[tauri::command]
fn get_experimental_local_buffer(engine: State<'_, AppHandle>) -> Result<bool, String> {
    airsend_engine::get_experimental_local_buffer(engine.inner().clone())
}

#[tauri::command]
async fn save_experimental_local_buffer(
    engine: State<'_, AppHandle>,
    enabled: bool,
) -> Result<(), String> {
    airsend_engine::save_experimental_local_buffer(
        engine.inner().clone(),
        enabled,
        engine.inner().streaming(),
    )
    .await
}

#[tauri::command]
fn export_audio_diagnostics(engine: State<'_, AppHandle>) -> Result<String, String> {
    airsend_engine::export_audio_diagnostics(engine.inner().diagnostics())
}

#[tauri::command]
fn get_latency(engine: State<'_, AppHandle>) -> Result<Option<u32>, String> {
    airsend_engine::get_latency(engine.inner().clone())
}

#[tauri::command]
async fn get_latency_cooldown_ms(engine: State<'_, AppHandle>) -> Result<u64, String> {
    airsend_engine::get_latency_cooldown_ms(engine.inner().streaming()).await
}

#[tauri::command]
async fn confirm_latency(engine: State<'_, AppHandle>, latency_ms: u32) -> Result<bool, String> {
    airsend_engine::confirm_latency(
        engine.inner().clone(),
        latency_ms,
        engine.inner().streaming(),
    )
    .await
}

#[tauri::command]
async fn get_receiver_volumes(engine: State<'_, AppHandle>) -> Result<Vec<ReceiverVolume>, String> {
    airsend_engine::get_receiver_volumes(engine.inner().streaming()).await
}

#[tauri::command]
async fn set_receiver_volume(
    engine: State<'_, AppHandle>,
    ip: String,
    port: u16,
    volume: f32,
) -> Result<f32, String> {
    airsend_engine::set_receiver_volume(
        engine.inner().clone(),
        ip,
        port,
        volume,
        engine.inner().streaming(),
    )
    .await
}

#[tauri::command]
fn open_latest_release(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_shell::ShellExt;
    app.shell()
        .open("https://github.com/Pabldi08/AirSend/releases/latest", None)
        .map_err(|e| e.to_string())
}

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
                let engine = app.state::<AppHandle>().inner().clone();
                tauri::async_runtime::spawn(async move {
                    let _ =
                        airsend_engine::stop_streaming(engine.clone(), engine.streaming()).await;
                    let _ = airsend_engine::disconnect_device(engine.connection()).await;
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
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // `_log_guard` debe vivir todo el programa para que el writer non-blocking
    // siga drenando logs al archivo. Lo "olvidamos" con un binding mut a static.
    let _log_guard = init_tracing();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
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
            let handle = app.handle().clone();
            let engine = AppHandle::new(app.path().app_data_dir()?, move |name, value| {
                let _ = handle.emit(name, value);
            })?;
            app.manage(engine);
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
