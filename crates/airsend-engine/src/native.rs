use std::ffi::{c_char, CStr, CString};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{de::DeserializeOwned, Serialize};
use serde_json::{json, Map, Value};
use tokio::runtime::Runtime;

use crate::preferences::{AudioOptions, ReceiverPreferencesPatch};
use crate::AppHandle;

#[derive(Default)]
struct Updates {
    discovery: AtomicU64,
    error: Mutex<Option<Value>>,
}

struct NativeHost {
    runtime: Runtime,
    app: AppHandle,
    updates: Arc<Updates>,
    log: Mutex<Option<tracing_appender::non_blocking::WorkerGuard>>,
}

static HOST: OnceLock<Result<NativeHost, String>> = OnceLock::new();

impl NativeHost {
    fn new() -> Result<Self, String> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .map_err(|error| error.to_string())?;
        let root = std::env::var_os("APPDATA").ok_or("APPDATA unavailable")?;
        let path = std::path::PathBuf::from(root).join("com.pablodiaz.conexionairplay");
        let log = Mutex::new(crate::host::init_tracing());
        let updates = Arc::new(Updates::default());
        let notifications = updates.clone();
        let app = AppHandle::new(path, move |name, data| match name {
            "airplay://device" | "airplay://device-removed" => {
                notifications.discovery.fetch_add(1, Ordering::Release);
            }
            "airplay://error" => *notifications.error.lock().unwrap() = Some(data),
            _ => {}
        })
        .map_err(|error| error.to_string())?;
        app.streaming().manual_resume.store(true, Ordering::Relaxed);
        Ok(Self {
            runtime,
            app,
            updates,
            log,
        })
    }
}

fn argument<T: DeserializeOwned>(request: &mut Map<String, Value>, key: &str) -> Result<T, String> {
    serde_json::from_value(request.remove(key).unwrap_or(Value::Null))
        .map_err(|error| format!("{key}: {error}"))
}

fn serialize(value: impl Serialize) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

async fn execute(host: &NativeHost, mut request: Map<String, Value>) -> Result<Value, String> {
    let command: String = argument(&mut request, "command")?;
    let app = &host.app;
    match command.as_str() {
        "start_discovery_stream" => {
            serialize(crate::start_discovery_stream(app.clone(), app.discovery()).await?)
        }
        "start_group_streaming" => serialize(
            crate::start_group_streaming(
                app.clone(),
                argument::<Vec<String>>(&mut request, "device_ids")?,
                argument::<bool>(&mut request, "stereo_pair")?,
                argument::<u32>(&mut request, "latency_ms")?,
                app.streaming(),
            )
            .await?,
        ),
        "start_streaming" => serialize(
            crate::start_streaming(
                app.clone(),
                argument::<Option<String>>(&mut request, "device_id")?,
                argument::<String>(&mut request, "ip")?,
                argument::<Option<u16>>(&mut request, "port")?,
                argument::<Option<String>>(&mut request, "name")?,
                argument::<Option<f32>>(&mut request, "volume")?,
                argument::<Option<u32>>(&mut request, "latency_ms")?,
                app.streaming(),
            )
            .await?,
        ),
        "add_streaming" => serialize(
            crate::add_streaming(
                app.clone(),
                argument::<Option<String>>(&mut request, "device_id")?,
                argument::<String>(&mut request, "ip")?,
                argument::<Option<u16>>(&mut request, "port")?,
                argument::<Option<String>>(&mut request, "name")?,
                argument::<Option<f32>>(&mut request, "volume")?,
                argument::<Option<u32>>(&mut request, "latency_ms")?,
                app.streaming(),
            )
            .await?,
        ),
        "stop_streaming" => serialize(crate::stop_streaming(app.clone(), app.streaming()).await?),
        "resume_streaming" => serialize(crate::resume_streaming(app.clone()).await?),
        "get_standby_options" => serialize(crate::get_standby_options(app)?),
        "save_standby_options" => {
            serialize(crate::save_standby_options(app, argument(&mut request, "options")?).await?)
        }
        "remove_streaming" => serialize(
            crate::remove_streaming(
                argument::<String>(&mut request, "ip")?,
                argument::<u16>(&mut request, "port")?,
                app.streaming(),
            )
            .await?,
        ),
        "set_stream_volume" => serialize(
            crate::set_stream_volume(
                app.clone(),
                argument::<f32>(&mut request, "volume")?,
                app.streaming(),
            )
            .await?,
        ),
        "connect_device" => serialize(
            crate::connect_device(
                app.clone(),
                argument::<Option<String>>(&mut request, "device_id")?,
                argument::<String>(&mut request, "ip")?,
                argument::<Option<u16>>(&mut request, "port")?,
                argument::<Option<String>>(&mut request, "name")?,
                app.connection(),
            )
            .await?,
        ),
        "disconnect_device" => serialize(crate::disconnect_device(app.connection()).await?),
        "add_manual_device" => serialize(
            crate::add_manual_device(
                app.clone(),
                argument::<String>(&mut request, "ip")?,
                argument::<Option<u16>>(&mut request, "port")?,
                argument::<Option<String>>(&mut request, "name")?,
                app.discovery(),
            )
            .await?,
        ),
        "list_audio_outputs" => serialize(crate::list_audio_outputs().await?),
        "list_audio_apps" => serialize(
            tokio::task::spawn_blocking(audio_capture::process_loopback::list_apps)
                .await
                .map_err(|error| error.to_string())?
                .map_err(|error| error.to_string())?,
        ),
        "get_audio_options" => serialize(crate::get_audio_options(app.clone())?),
        "save_audio_options" => serialize(
            crate::save_audio_options(
                app.clone(),
                argument::<AudioOptions>(&mut request, "options")?,
                app.streaming(),
            )
            .await?,
        ),
        "get_session_status" => serialize(crate::get_session_status(app.streaming()).await?),
        "get_receiver_preferences" => serialize(crate::get_receiver_preferences(
            app.clone(),
            argument::<String>(&mut request, "receiver_id")?,
        )?),
        "patch_receiver_preferences" => serialize(crate::patch_receiver_preferences(
            app.clone(),
            argument::<String>(&mut request, "receiver_id")?,
            argument::<ReceiverPreferencesPatch>(&mut request, "patch")?,
        )?),
        "save_last_device" => serialize(crate::save_last_device(
            app.clone(),
            argument::<String>(&mut request, "ip")?,
            argument::<u16>(&mut request, "port")?,
            argument::<String>(&mut request, "name")?,
        )?),
        "get_last_device" => serialize(crate::get_last_device(app.clone())?),
        "clear_last_device" => serialize(crate::clear_last_device(app.clone())?),
        "save_volume" => serialize(crate::save_volume(
            app.clone(),
            argument::<f32>(&mut request, "volume")?,
        )?),
        "get_volume" => serialize(crate::get_volume(app.clone())?),
        "save_multi_device" => serialize(crate::save_multi_device(
            app.clone(),
            argument::<bool>(&mut request, "enabled")?,
        )?),
        "get_multi_device" => serialize(crate::get_multi_device(app.clone())?),
        "get_experimental_local_buffer" => {
            serialize(crate::get_experimental_local_buffer(app.clone())?)
        }
        "save_experimental_local_buffer" => serialize(
            crate::save_experimental_local_buffer(
                app.clone(),
                argument::<bool>(&mut request, "enabled")?,
                app.streaming(),
            )
            .await?,
        ),
        "export_audio_diagnostics" => {
            serialize(crate::export_audio_diagnostics(app.diagnostics())?)
        }
        "get_latency" => serialize(crate::get_latency(app.clone())?),
        "get_latency_cooldown_ms" => {
            serialize(crate::get_latency_cooldown_ms(app.streaming()).await?)
        }
        "confirm_latency" => serialize(
            crate::confirm_latency(
                app.clone(),
                argument::<u32>(&mut request, "latency_ms")?,
                app.streaming(),
            )
            .await?,
        ),
        "get_receiver_volumes" => serialize(crate::get_receiver_volumes(app.streaming()).await?),
        "set_receiver_volume" => serialize(
            crate::set_receiver_volume(
                app.clone(),
                argument::<String>(&mut request, "ip")?,
                argument::<u16>(&mut request, "port")?,
                argument::<f32>(&mut request, "volume")?,
                app.streaming(),
            )
            .await?,
        ),
        "devices" => {
            let since: Option<u64> = argument(&mut request, "since")?;
            let revision = host.updates.discovery.load(Ordering::Acquire);
            if since == Some(revision) {
                Ok(Value::Null)
            } else {
                Ok(json!({ "revision": revision, "devices": app.discovered_devices()? }))
            }
        }
        "take_error" => serialize(host.updates.error.lock().unwrap().take()),
        "get_language" => serialize(app.store().get("language")),
        "save_language" => serialize(
            app.store()
                .set("language", argument::<Value>(&mut request, "language")?)?,
        ),
        "get_theme" => serialize(app.store().get("theme")),
        "save_theme" => {
            let theme: String = argument(&mut request, "theme")?;
            if !matches!(theme.as_str(), "system" | "light" | "dark") {
                return Err("invalid_theme".into());
            }
            serialize(app.store().set("theme", Value::String(theme))?)
        }
        "shutdown" => {
            crate::stop_streaming(app.clone(), app.streaming()).await?;
            crate::disconnect_device(app.connection()).await?;
            crate::stop_discovery_stream(app.discovery()).await?;
            host.log.lock().unwrap().take();
            Ok(Value::Null)
        }
        _ => Err(format!("Unknown native command: {command}")),
    }
}

/// The caller provides a NUL-terminated UTF-8 request and owns the returned string.
#[no_mangle]
pub unsafe extern "C" fn airsend_request(request: *const c_char) -> *mut c_char {
    let result = (|| {
        if request.is_null() {
            return Err("Missing request".into());
        }
        // SAFETY: the caller keeps the NUL-terminated request live during this call.
        let request = unsafe { CStr::from_ptr(request) };
        let request =
            serde_json::from_slice(request.to_bytes()).map_err(|error| error.to_string())?;
        let host = HOST
            .get_or_init(NativeHost::new)
            .as_ref()
            .map_err(Clone::clone)?;
        host.runtime.block_on(execute(host, request))
    })();
    let response = match result {
        Ok(data) => json!({ "data": data }),
        Err(error) => json!({ "error": error }),
    };
    CString::new(response.to_string())
        .expect("JSON escapes embedded NULs")
        .into_raw()
}

/// Release a response returned by airsend_request exactly once.
#[no_mangle]
pub unsafe extern "C" fn airsend_free(response: *mut c_char) {
    if !response.is_null() {
        // SAFETY: ownership of this CString was transferred to the caller.
        drop(unsafe { CString::from_raw(response) });
    }
}
