//! Streaming en tiempo real al HomePod sobre una sesión ya pareada.
//!
//! Para Hito 4a generamos un tono sintético de prueba. La misma API
//! (`StreamHandle::push_pcm`) sirve para conectar el WASAPI loopback en Hito 3+:
//! quien produce el audio sólo necesita mandar `LivePcmFrame`s al sender.

use std::f32::consts::TAU;
use std::sync::Arc;
use std::time::Duration;

use ap2rs_audio::{AlacEncoder, LiveAudioDecoder};
pub use ap2rs_audio::{LiveFrameSender, LivePcmFrame};
pub use ap2rs_client::Connection;
use ap2rs_core::codec::{AudioCodec, AudioFormat};
use ap2rs_core::stream::{PtpMode, StreamConfig, StreamType, TimingProtocol};
use ap2rs_core::Device as Ap2Device;
use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;

use crate::pairing::{DeviceDescriptor, PairingError, HOMEPOD_TRANSIENT_PIN};

/// Capacidad de la cola PCM hacia el encoder ALAC. Cada slot lleva ~10-20 ms
/// de audio según el tamaño del chunk que envíe la captura. 64 ≈ 0.6-1.3 s de
/// headroom; alineado con el buffer de la captura (`parec` también usa 64) para
/// que productor y consumidor tengan el mismo margen.
const QUEUE_CAPACITY: usize = 64;

/// Intervalo entre RTSP `feedback` requests. El HomePod cierra la sesión si no
/// recibe nada del sender durante ~10 s (timeout RTSP en
/// `airplay-rtsp/src/connection.rs:137`). El TUI upstream usa 2 s; lo
/// replicamos para tener margen amplio.
const FEEDBACK_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Error)]
pub enum StreamError {
    #[error("pairing failed: {0}")]
    Pairing(#[from] PairingError),
    #[error("AirPlay client error: {0}")]
    Client(String),
    #[error("audio encoder error: {0}")]
    Encoder(String),
}

/// Perfil de latencia: define el rango (`latency_min`/`latency_max`) con el que
/// el HomePod dimensiona su buffer de recepción. Menos latencia = más reactivo
/// pero más sensible a hipos de Wi-Fi: si el buffer del receptor se vacía, el
/// audio "se robotiza" (repite el último paquete) aunque nuestro sender vaya a
/// cero drops. `Music` es el rango seguro probado; `Video`/`Gaming` bajan el
/// suelo para quien tenga buena red y quiera menos retardo. El usuario elige el
/// perfil en la UI y se persiste como el volumen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LatencyProfile {
    /// ~500 ms – 3 s. Seguro sobre Wi-Fi normal. Default.
    #[default]
    Music,
    /// ~350 ms – 2 s. Menos retardo para vídeo; necesita red estable.
    Video,
    /// ~250 ms – 1 s. Mínimo retardo; sólo redes buenas, puede robotizar.
    Gaming,
}

impl LatencyProfile {
    /// `(latency_min, latency_max)` en frames @ 44.1 kHz.
    fn frames(self) -> (u32, u32) {
        match self {
            LatencyProfile::Music => (22_050, 132_300), // ~500 ms – 3 s
            LatencyProfile::Video => (15_435, 88_200),  // ~350 ms – 2 s
            LatencyProfile::Gaming => (11_025, 44_100), // ~250 ms – 1 s
        }
    }
}

/// Configuración usada para abrir el stream. Para Hito 4a fijamos ALAC 44.1k/16/2,
/// timing NTP. Tono o captura, el formato es el mismo. `profile` decide el rango
/// de latencia que negociamos con el HomePod.
fn streaming_stream_config(profile: LatencyProfile) -> Result<StreamConfig, StreamError> {
    let audio_format = AudioFormat {
        codec: AudioCodec::Alac,
        sample_rate: ap2rs_core::codec::SampleRate::Hz44100,
        bit_depth: 16,
        channels: 2,
        frames_per_packet: 352,
    };

    // Magic cookie del codec ALAC: el HomePod lo necesita en SETUP para
    // saber descodificar. Lo extraemos de un encoder temporal.
    let asc = AlacEncoder::new(audio_format.clone())
        .map_err(|e| StreamError::Encoder(e.to_string()))?
        .magic_cookie();

    // latency_min/max define el buffer interno del HomePod (frames @ 44.1k).
    // El HomePod elige dentro del rango según condiciones de red. Probado:
    // 100–500 ms es demasiado ajustado sobre Wi-Fi — cualquier hipo vacía el
    // buffer del receptor y el audio "se robotiza" (repite paquete) aunque
    // nuestro sender vaya a cero drops. El default (`Music`) usa los mismos
    // valores que el test upstream de live capture (500 ms / 3 s), estable.
    // `Video`/`Gaming` bajan el suelo para quien quiera menos retardo y tenga
    // red buena; el `LatencyProfile` documenta el riesgo.
    // `ptp_mode` es ignorado por upstream cuando `timing_protocol == Ntp`.
    let (latency_min, latency_max) = profile.frames();
    Ok(StreamConfig {
        stream_type: StreamType::Realtime,
        audio_format,
        timing_protocol: TimingProtocol::Ntp,
        ptp_mode: PtpMode::Master,
        latency_min,
        latency_max,
        supports_dynamic_stream_id: true,
        asc: Some(asc),
    })
}

/// Volumen por defecto al abrir el stream. AirPlay usa escala 0..1
/// (0 = mute, 1 = máximo). 0.20 ≈ -25 dB, suave para no asustar pruebas.
pub const DEFAULT_INITIAL_VOLUME: f32 = 0.20;

/// Mantiene viva la tarea de heartbeat hasta que se dropea, momento en el que
/// la aborta. Útil para que el caller pueda quedarse con `Arc<Mutex<Connection>>`
/// y aun así garantizar que el heartbeat se para cuando el caller termina.
pub struct HeartbeatGuard {
    handle: Option<JoinHandle<()>>,
}

impl HeartbeatGuard {
    /// Aborta la tarea de heartbeat sin esperar.
    pub fn shutdown(&mut self) {
        if let Some(h) = self.handle.take() {
            h.abort();
        }
    }
}

impl Drop for HeartbeatGuard {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Maneja la sesión de streaming: la conexión + el sender de frames PCM + la
/// tarea de heartbeat que mantiene viva la sesión RTSP.
pub struct StreamHandle {
    sender: LiveFrameSender,
    /// `Connection` compartida con la tarea de heartbeat. Necesita `Mutex`
    /// porque varias rutas (`set_volume`, `send_feedback`) requieren `&mut`.
    connection: Arc<AsyncMutex<Connection>>,
    heartbeat: HeartbeatGuard,
    sample_rate: u32,
    channels: u8,
}

impl StreamHandle {
    pub fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub fn channels(&self) -> u8 {
        self.channels
    }

    /// Encola un frame PCM interleaved i16. Si la cola está llena, lo descarta.
    /// Devuelve `false` cuando se descartó.
    pub fn push_pcm(&self, samples: Vec<i16>) -> bool {
        self.sender.try_send(LivePcmFrame {
            samples,
            channels: self.channels,
            sample_rate: self.sample_rate,
        })
    }

    /// Ajusta el volumen del receptor AirPlay. Escala 0.0..=1.0.
    pub async fn set_volume(&self, volume: f32) -> Result<(), StreamError> {
        let v = volume.clamp(0.0, 1.0);
        let mut conn = self.connection.lock().await;
        conn.set_volume(v)
            .await
            .map_err(|e| StreamError::Client(e.to_string()))
    }

    /// Descompone el handle en sus partes para poder pasar el `sender` a un
    /// hilo bombeador y mantener la `Connection` en el caller para control
    /// (volumen, stop, etc.). El `HeartbeatGuard` debe mantenerse vivo todo el
    /// tiempo que dure el streaming; en cuanto se dropea, el heartbeat para y
    /// el HomePod cerrará la sesión al expirar el timeout RTSP de ~10 s.
    pub fn into_parts(
        self,
    ) -> (
        LiveFrameSender,
        Arc<AsyncMutex<Connection>>,
        HeartbeatGuard,
        u32,
        u8,
    ) {
        (
            self.sender,
            self.connection,
            self.heartbeat,
            self.sample_rate,
            self.channels,
        )
    }
}

/// Abre un stream de audio en vivo al HomePod: pair-setup + setup() + start_streaming_live().
/// Si `initial_volume` es `None`, usa `DEFAULT_INITIAL_VOLUME` (bajo). Si
/// `latency` es `None`, usa el perfil por defecto (`LatencyProfile::Music`).
pub async fn open_live_stream(
    descriptor: DeviceDescriptor,
    initial_volume: Option<f32>,
    latency: Option<LatencyProfile>,
) -> Result<StreamHandle, StreamError> {
    let device = build_device(&descriptor)?;
    let config = streaming_stream_config(latency.unwrap_or_default())?;
    let sample_rate = config.audio_format.sample_rate.as_hz();
    let channels = config.audio_format.channels;

    tracing::info!(ip = %descriptor.ip, "open_live_stream: conectando + pairing");
    let mut connection = Connection::connect_with_pin(device, config, HOMEPOD_TRANSIENT_PIN)
        .await
        .map_err(|e| StreamError::Client(e.to_string()))?;

    tracing::info!("connection establecida — setup() RTP");
    connection
        .setup()
        .await
        .map_err(|e| StreamError::Client(e.to_string()))?;

    // Ajustamos volumen ANTES de empezar streaming para que las primeras
    // muestras no salgan al volumen que tuviera el HomePod previamente.
    let target_vol = initial_volume.unwrap_or(DEFAULT_INITIAL_VOLUME).clamp(0.0, 1.0);
    if let Err(e) = connection.set_volume(target_vol).await {
        tracing::warn!(error = %e, "set_volume inicial falló, sigo igualmente");
    } else {
        tracing::info!(volume = target_vol, "volumen inicial aplicado");
    }

    let (sender, decoder) =
        LiveAudioDecoder::create_pair(sample_rate, channels, QUEUE_CAPACITY);

    tracing::info!("setup OK — start_streaming_live()");
    connection
        .start_streaming_live(decoder)
        .await
        .map_err(|e| StreamError::Client(e.to_string()))?;

    let connection = Arc::new(AsyncMutex::new(connection));

    // Heartbeat: el HomePod cierra la sesión si no recibe tráfico RTSP en ~10s.
    // `start_streaming_live` upstream NO arranca ningún keepalive; el TUI
    // upstream lo hace manualmente (airplay-tui/src/app.rs:713) cada 2 s.
    let heartbeat_conn = connection.clone();
    let heartbeat = tokio::spawn(async move {
        let mut interval = tokio::time::interval(FEEDBACK_INTERVAL);
        // Saltamos el primer tick inmediato — `start_streaming_live` acaba de
        // hablar con el HomePod, no hace falta otro RTSP ya mismo.
        interval.tick().await;
        loop {
            interval.tick().await;
            let mut conn = heartbeat_conn.lock().await;
            match conn.send_feedback().await {
                Ok(()) => tracing::debug!("heartbeat feedback OK"),
                Err(e) => {
                    tracing::warn!(error = %e, "heartbeat feedback falló");
                    // Si falla varias veces seguidas, igualmente seguimos: el
                    // siguiente tick lo reintentará. Si la conexión está
                    // realmente muerta, el pump empezará a recibir errores y
                    // el caller hará shutdown del stream.
                }
            }
        }
    });

    let heartbeat = HeartbeatGuard {
        handle: Some(heartbeat),
    };

    Ok(StreamHandle {
        sender,
        connection,
        heartbeat,
        sample_rate,
        channels,
    })
}

fn build_device(d: &DeviceDescriptor) -> Result<Ap2Device, PairingError> {
    // Reutilizamos la construcción de pairing.rs vía un descriptor clonado.
    d.clone().into_ap2_device()
}

/// Tono sintético de prueba (Hito 4a). Genera un seno a `freq` Hz durante
/// `duration` y lo encola al stream a un ritmo de tiempo real.
pub async fn play_test_tone(
    handle: &StreamHandle,
    freq: f32,
    duration: Duration,
    amplitude: f32,
) -> Result<(), StreamError> {
    const FRAMES_PER_PACKET: usize = 352;
    let sample_rate = handle.sample_rate as f32;
    let channels = handle.channels as usize;
    let total_samples = (handle.sample_rate as u64 * duration.as_millis() as u64 / 1000) as usize;
    let amp = (amplitude.clamp(0.0, 1.0) * i16::MAX as f32) as f32;

    // Ritmo en tiempo real: ~352 frames a 44.1k = ~8 ms por paquete.
    let packet_dur = Duration::from_micros(
        (FRAMES_PER_PACKET as u64 * 1_000_000) / handle.sample_rate as u64,
    );

    let mut phase: f32 = 0.0;
    let phase_inc = TAU * freq / sample_rate;
    let mut produced = 0usize;

    while produced < total_samples {
        let remaining = (total_samples - produced).min(FRAMES_PER_PACKET);
        let mut buf = Vec::with_capacity(remaining * channels);
        for _ in 0..remaining {
            let s = (phase.sin() * amp) as i16;
            for _ in 0..channels {
                buf.push(s);
            }
            phase += phase_inc;
            if phase > TAU {
                phase -= TAU;
            }
        }
        if !handle.push_pcm(buf) {
            tracing::warn!("cola llena, dropping frames (productor más rápido que red)");
        }
        produced += remaining;
        tokio::time::sleep(packet_dur).await;
    }

    Ok(())
}
