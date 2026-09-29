//! Captura del audio del sistema.
//!
//! - **Windows (target real)**: WASAPI loopback del render device default, con
//!   `AUTOCONVERTPCM` para que Windows entregue 44.1 kHz / i16 / estéreo
//!   directamente — sin resampler propio.
//! - **Linux (dev)**: `parec` del monitor del sink default de PipeWire/PA,
//!   fallback a `cpal` (mic) si parec no está.
//!
//! Quien consume estos frames (el `StreamHandle` de cap-core) recibe Vec<i16>
//! interleaved listos para encolar al encoder ALAC.

use std::fmt;
use std::time::{Duration, Instant};
mod queue;
pub use queue::{CaptureDiagnostics, CapturePolicy, CaptureReceiver, CaptureSender};

use thiserror::Error;

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("no se encontró ningún dispositivo de captura adecuado")]
    NoDevice,
    #[error("no se pudo configurar el stream con sample rate {wanted} Hz / {channels} ch")]
    UnsupportedConfig { wanted: u32, channels: u16 },
    #[error("backend de captura: {0}")]
    Backend(String),
    #[error("backend no implementado en esta plataforma")]
    UnsupportedPlatform,
}

#[derive(Debug, Clone, Copy)]
pub struct CaptureFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

impl CaptureFormat {
    pub const AIRPLAY_DEFAULT: Self = Self {
        sample_rate: 44_100,
        channels: 2,
    };
}

/// Frame de audio capturado: PCM i16 interleaved.
#[derive(Debug)]
pub struct CapturedFrame {
    /// Software delivery timestamp; excludes hardware/audio-engine delay.
    pub captured_at: Instant,
    pub samples: Vec<i16>,
    pub channels: u16,
    pub sample_rate: u32,
}

impl CapturedFrame {
    pub fn new(samples: Vec<i16>, channels: u16, sample_rate: u32) -> Self {
        Self {
            samples,
            channels,
            sample_rate,
            captured_at: Instant::now(),
        }
    }
    pub fn duration(&self) -> Duration {
        Duration::from_secs_f64(
            self.samples.len() as f64
                / self.channels.max(1) as f64
                / self.sample_rate.max(1) as f64,
        )
    }
}

/// Cierra el handle al droparlo.
pub trait Capture: Send + Sync {
    fn name(&self) -> &str;
    /// Cierra la captura explícitamente. El Drop también la cierra.
    fn stop(self: Box<Self>);
    fn request_local_mute(&self, _journal: std::path::PathBuf) -> Result<(), CaptureError> {
        Err(CaptureError::UnsupportedPlatform)
    }
    fn mute_status(&self) -> &'static str {
        "unavailable"
    }
}

impl fmt::Debug for dyn Capture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Capture(\"{}\")", self.name())
    }
}

#[cfg(windows)]
mod endpoints;
#[cfg(unix)]
pub mod linux;
#[cfg(unix)]
pub mod linux_parec;
#[cfg(windows)]
pub mod windows;

/// Arranca la captura del audio del sistema y manda frames por el canal devuelto.
///
/// La elección de dispositivo depende de la plataforma — ver módulos `linux` /
/// `windows`. El formato deseado es 44.1 kHz estéreo i16; si el dispositivo no
/// lo soporta directamente, lo convertimos por la vía rápida (interleave + cast).
pub fn start_loopback(
    fmt: CaptureFormat,
) -> Result<(Box<dyn Capture>, CaptureReceiver), CaptureError> {
    start_loopback_with_policy(fmt, CapturePolicy::Stable)
}

pub fn start_loopback_with_policy(
    fmt: CaptureFormat,
    policy: CapturePolicy,
) -> Result<(Box<dyn Capture>, CaptureReceiver), CaptureError> {
    #[cfg(unix)]
    {
        // Preferimos parec porque ve el monitor del sink default de PipeWire/PA.
        // Si no está disponible, caemos a cpal (que en Linux suele acabar
        // capturando del micrófono, no del sistema — útil para tests offline).
        if linux_parec::available() {
            return linux_parec::start_with_policy(fmt, policy);
        }
        tracing::warn!(
            "parec no disponible — fallback a cpal (probable mic, no audio del sistema)"
        );
        linux::start_with_policy(fmt, policy)
    }
    #[cfg(windows)]
    {
        windows::start_with_policy(fmt, policy)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (fmt, policy);
        Err(CaptureError::UnsupportedPlatform)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct AudioEndpoint {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

pub fn list_audio_outputs() -> Result<Vec<AudioEndpoint>, CaptureError> {
    #[cfg(windows)]
    {
        endpoints::enumerate()
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}
pub fn recover_local_mute(journal: &std::path::Path) -> Result<(), CaptureError> {
    #[cfg(windows)]
    {
        endpoints::recover(journal)
    }
    #[cfg(not(windows))]
    {
        let _ = journal;
        Ok(())
    }
}
pub fn start_loopback_with_device(
    fmt: CaptureFormat,
    policy: CapturePolicy,
    device_id: Option<String>,
) -> Result<(Box<dyn Capture>, CaptureReceiver), CaptureError> {
    #[cfg(windows)]
    {
        windows::start_with_device(fmt, policy, device_id)
    }
    #[cfg(not(windows))]
    {
        if device_id.is_some() {
            return Err(CaptureError::UnsupportedPlatform);
        }
        start_loopback_with_policy(fmt, policy)
    }
}
