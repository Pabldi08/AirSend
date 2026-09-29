//! Windows endpoint enumeration and reversible, opt-in local mute.
use crate::{AudioEndpoint, CaptureError};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use windows::core::{implement, Result as WinResult, GUID, PCWSTR};
use windows::Win32::{
    Media::Audio::{Endpoints::*, *},
    System::Com::{CoCreateInstance, CLSCTX_ALL},
    UI::Shell::PropertiesSystem::PROPERTYKEY,
};

const MUTE_CONTEXT: GUID = GUID::from_u128(0x9a35fc48_9030_4c06_a95b_ef0326a28233);

pub struct ComGuard;
impl ComGuard {
    pub fn new() -> Result<Self, CaptureError> {
        wasapi::initialize_mta()
            .ok()
            .map_err(|e| CaptureError::Backend(e.to_string()))?;
        Ok(Self)
    }
}
impl Drop for ComGuard {
    fn drop(&mut self) {
        wasapi::deinitialize();
    }
}

pub fn enumerate() -> Result<Vec<AudioEndpoint>, CaptureError> {
    let _com = ComGuard::new()?;
    let default = wasapi::get_default_device(&wasapi::Direction::Render)
        .ok()
        .and_then(|d| d.get_id().ok());
    let collection = wasapi::DeviceCollection::new(&wasapi::Direction::Render)
        .map_err(|e| CaptureError::Backend(e.to_string()))?;
    let mut devices = Vec::new();
    for index in 0..collection
        .get_nbr_devices()
        .map_err(|e| CaptureError::Backend(e.to_string()))?
    {
        let device = collection
            .get_device_at_index(index)
            .map_err(|e| CaptureError::Backend(e.to_string()))?;
        let id = device
            .get_id()
            .map_err(|e| CaptureError::Backend(e.to_string()))?;
        devices.push(AudioEndpoint {
            is_default: default.as_ref() == Some(&id),
            id,
            name: device
                .get_friendlyname()
                .unwrap_or_else(|_| "Windows audio output".into()),
        });
    }
    Ok(devices)
}

pub fn select(id: Option<&str>) -> Result<wasapi::Device, String> {
    let Some(id) = id else {
        return wasapi::get_default_device(&wasapi::Direction::Render).map_err(|e| e.to_string());
    };
    let devices =
        wasapi::DeviceCollection::new(&wasapi::Direction::Render).map_err(|e| e.to_string())?;
    for i in 0..devices.get_nbr_devices().map_err(|e| e.to_string())? {
        let device = devices.get_device_at_index(i).map_err(|e| e.to_string())?;
        if device.get_id().map_err(|e| e.to_string())? == id {
            return Ok(device);
        }
    }
    Err("selected_audio_output_unavailable".into())
}

#[implement(IMMNotificationClient)]
struct EndpointEvents(Arc<AtomicU64>);
#[allow(non_snake_case)]
impl IMMNotificationClient_Impl for EndpointEvents {
    fn OnDeviceStateChanged(&self, _: &PCWSTR, _: DEVICE_STATE) -> WinResult<()> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn OnDeviceAdded(&self, _: &PCWSTR) -> WinResult<()> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn OnDeviceRemoved(&self, _: &PCWSTR) -> WinResult<()> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn OnDefaultDeviceChanged(&self, flow: EDataFlow, _: ERole, _: &PCWSTR) -> WinResult<()> {
        if flow == eRender {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        Ok(())
    }
    fn OnPropertyValueChanged(&self, _: &PCWSTR, _: &PROPERTYKEY) -> WinResult<()> {
        Ok(())
    }
}

pub struct Notifications {
    enumerator: IMMDeviceEnumerator,
    callback: IMMNotificationClient,
    pub generation: Arc<AtomicU64>,
}
impl Notifications {
    pub fn new() -> WinResult<Self> {
        let generation = Arc::new(AtomicU64::new(0));
        let callback: IMMNotificationClient = EndpointEvents(generation.clone()).into();
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        unsafe {
            enumerator.RegisterEndpointNotificationCallback(&callback)?;
        }
        Ok(Self {
            enumerator,
            callback,
            generation,
        })
    }
}
impl Drop for Notifications {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .enumerator
                .UnregisterEndpointNotificationCallback(&self.callback);
        }
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Recovery {
    endpoint_id: String,
    original_mute: bool,
}

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeEvents {
    external_change: Arc<AtomicBool>,
    journal: std::path::PathBuf,
}
#[allow(non_snake_case)]
impl IAudioEndpointVolumeCallback_Impl for VolumeEvents {
    fn OnNotify(&self, data: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> WinResult<()> {
        // Core Audio owns this pointer for the duration of the callback.
        if !data.is_null()
            && unsafe { (*data).guidEventContext } != MUTE_CONTEXT
            && !unsafe { (*data).bMuted.as_bool() }
        {
            self.external_change.store(true, Ordering::SeqCst);
            // Respect subsequent user actions, including after a crash.
            let _ = std::fs::remove_file(&self.journal);
        }
        Ok(())
    }
}

pub struct MuteGuard {
    volume: IAudioEndpointVolume,
    callback: IAudioEndpointVolumeCallback,
    external: Arc<AtomicBool>,
    original: bool,
    journal: std::path::PathBuf,
}
impl MuteGuard {
    pub fn apply(endpoint_id: &str, journal: &Path) -> Result<Self, String> {
        let volume = endpoint_volume(endpoint_id).map_err(|e| e.to_string())?;
        let original = unsafe { volume.GetMute().map_err(|e| e.to_string())?.as_bool() };
        if let Some(parent) = journal.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        // Persist the old state before changing Windows. Never overwrite another session's lease.
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(journal)
            .map_err(|e| e.to_string())?;
        use std::io::Write;
        let recovery = Recovery {
            endpoint_id: endpoint_id.into(),
            original_mute: original,
        };
        let data = serde_json::to_vec(&recovery).map_err(|e| e.to_string())?;
        if let Err(error) = file.write_all(&data).and_then(|_| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(journal);
            return Err(error.to_string());
        }
        drop(file);
        let external = Arc::new(AtomicBool::new(false));
        let callback: IAudioEndpointVolumeCallback = VolumeEvents {
            external_change: external.clone(),
            journal: journal.to_owned(),
        }
        .into();
        if let Err(e) = unsafe { volume.RegisterControlChangeNotify(&callback) } {
            let _ = std::fs::remove_file(journal);
            return Err(e.to_string());
        }
        let guard = Self {
            volume,
            callback,
            external,
            original,
            journal: journal.to_owned(),
        };
        unsafe {
            guard
                .volume
                .SetMute(true, &MUTE_CONTEXT)
                .map_err(|e| e.to_string())?;
        }
        Ok(guard)
    }
    pub fn external_change(&self) -> bool {
        self.external.load(Ordering::SeqCst)
    }
}
impl Drop for MuteGuard {
    fn drop(&mut self) {
        let mut restored = true;
        unsafe {
            if !self.external_change() {
                restored = match self.volume.GetMute() {
                    Ok(muted) if muted.as_bool() => {
                        self.volume.SetMute(self.original, &MUTE_CONTEXT).is_ok()
                    }
                    Ok(_) => true,
                    Err(_) => false,
                };
            }
            let _ = self.volume.UnregisterControlChangeNotify(&self.callback);
        }
        if restored {
            let _ = std::fs::remove_file(&self.journal);
        } else {
            tracing::warn!("local mute restore failed; recovery journal retained");
        }
    }
}
fn endpoint_volume(id: &str) -> WinResult<IAudioEndpointVolume> {
    let enumerator: IMMDeviceEnumerator =
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
    let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
    let device = unsafe { enumerator.GetDevice(PCWSTR(wide.as_ptr()))? };
    unsafe { device.Activate(CLSCTX_ALL, None) }
}
pub fn recover(journal: &Path) -> Result<(), CaptureError> {
    if !journal.exists() {
        return Ok(());
    }
    let _com = ComGuard::new()?;
    let recovery: Recovery = serde_json::from_slice(
        &std::fs::read(journal).map_err(|e| CaptureError::Backend(e.to_string()))?,
    )
    .map_err(|e| CaptureError::Backend(e.to_string()))?;
    let volume =
        endpoint_volume(&recovery.endpoint_id).map_err(|e| CaptureError::Backend(e.to_string()))?;
    unsafe {
        if volume
            .GetMute()
            .map_err(|e| CaptureError::Backend(e.to_string()))?
            .as_bool()
        {
            volume
                .SetMute(recovery.original_mute, &MUTE_CONTEXT)
                .map_err(|e| CaptureError::Backend(e.to_string()))?;
        }
    }
    std::fs::remove_file(journal).map_err(|e| CaptureError::Backend(e.to_string()))
}
