//! Pairing con HomePod usando `airplay-client` (parte de airplay2-rs).
//!
//! Para Hito 2 sólo nos interesa abrir la sesión RTSP encriptada: pair-setup
//! transient (PIN "3939") + pair-verify. No empezamos a enviar audio todavía.
//! Devolvemos el `Connection` para que el caller lo mantenga vivo o lo cierre.

use std::net::IpAddr;
use std::time::Duration;

use ap2rs_client::{Connection, Device as Ap2Device};
use ap2rs_core::{
    device::{DeviceId, Version},
    features::Features,
    stream::StreamConfig,
};
use thiserror::Error;

/// PIN fijo que usan los HomePod en transient pairing (HKP=4).
pub const HOMEPOD_TRANSIENT_PIN: &str = "3939";

#[derive(Debug, Error)]
pub enum PairingError {
    #[error("AirPlay client error: {0}")]
    Client(String),
    #[error("invalid device id (MAC): {0}")]
    InvalidDeviceId(String),
    #[error("invalid features: {0}")]
    InvalidFeatures(String),
}

/// Resultado de un pairing satisfactorio.
pub struct PairedSession {
    pub connection: Connection,
}

impl std::fmt::Debug for PairedSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairedSession").finish_non_exhaustive()
    }
}

/// Parámetros mínimos para construir un `Ap2Device` cuando sólo tenemos IP+puerto
/// (caso entrada manual). En el caso mDNS podemos rellenar más campos pero para
/// pair-setup transient basta con MAC, IP, puerto y un set de features razonable.
#[derive(Debug, Clone)]
pub struct DeviceDescriptor {
    pub ip: IpAddr,
    pub port: u16,
    pub name: String,
    /// MAC del HomePod en formato "AA:BB:CC:DD:EE:FF". Si no la conocemos, usamos
    /// una derivada de la IP — no afecta al pair-setup transient porque no se
    /// persiste identidad.
    pub mac: Option<String>,
    /// Modelo anunciado por mDNS (`AudioAccessory5,1` para HomePod Mini).
    pub model: Option<String>,
    /// String de features del TXT mDNS (`0x4A7FCA00,0x3C354BD0`).
    /// Si no la tenemos, usamos un set conservador.
    pub features: Option<String>,
}

impl DeviceDescriptor {
    /// Convierte este descriptor en el `Device` que espera airplay-client.
    pub fn into_ap2_device(self) -> Result<Ap2Device, PairingError> {
        self.build_ap2_device()
    }

    fn build_ap2_device(&self) -> Result<Ap2Device, PairingError> {
        // MAC sintética estable a partir de la IP cuando no la tengamos.
        let mac = self.mac.clone().unwrap_or_else(|| match self.ip {
            IpAddr::V4(v4) => {
                let o = v4.octets();
                format!("02:00:{:02X}:{:02X}:{:02X}:{:02X}", o[0], o[1], o[2], o[3])
            }
            IpAddr::V6(_) => "02:00:00:00:00:01".to_string(),
        });
        let id = DeviceId::from_mac_string(&mac)
            .map_err(|e| PairingError::InvalidDeviceId(e.to_string()))?;

        // Features conservadoras suficientes para pair-setup transient + RTSP.
        // Si el descriptor trae features explícitas (mDNS), las usamos.
        let features = match &self.features {
            Some(s) => Features::from_txt_value(s)
                .map_err(|e| PairingError::InvalidFeatures(e.to_string()))?,
            None => Features::from_txt_value("0x4A7FCA00,0x3C354BD0")
                .map_err(|e| PairingError::InvalidFeatures(e.to_string()))?,
        };

        let model = self
            .model
            .clone()
            .unwrap_or_else(|| "AudioAccessory5,1".to_string());

        Ok(Ap2Device {
            id,
            name: self.name.clone(),
            model,
            manufacturer: None,
            serial_number: None,
            addresses: vec![self.ip],
            port: self.port,
            features,
            required_sender_features: None,
            public_key: None,
            source_version: Version::default(),
            firmware_version: None,
            os_version: None,
            protocol_version: None,
            requires_password: false,
            status_flags: 0,
            access_control: None,
            pairing_identity: None,
            system_pairing_identity: None,
            bluetooth_address: None,
            homekit_home_id: None,
            group_id: None,
            is_group_leader: false,
            group_public_name: None,
            group_contains_discoverable_leader: false,
            home_group_id: None,
            household_id: None,
            parent_group_id: None,
            parent_group_contains_discoverable_leader: false,
            tight_sync_id: None,
            raop_port: None,
            raop_encryption_types: None,
            raop_codecs: None,
            raop_transport: None,
            raop_metadata_types: None,
            raop_digest_auth: false,
            vodka_version: None,
        })
    }
}

/// Configuración por defecto: ALAC 44.1k/16/2, timing NTP. Suficiente para
/// abrir la sesión RTSP; el audio real llega en Hito 4.
fn default_stream_config() -> StreamConfig {
    StreamConfig::default()
}

/// Hace pair-setup transient + pair-verify contra el HomePod descrito.
/// Devuelve la conexión RTSP abierta y autenticada.
pub async fn pair_homepod(descriptor: DeviceDescriptor) -> Result<PairedSession, PairingError> {
    let device = descriptor.build_ap2_device()?;
    let config = default_stream_config();

    tracing::info!(
        ip = %descriptor.ip,
        name = %descriptor.name,
        "iniciando pair-setup transient con HomePod"
    );

    let connection = tokio::time::timeout(
        Duration::from_secs(15),
        Connection::connect_with_pin(device, config, HOMEPOD_TRANSIENT_PIN),
    )
    .await
    .map_err(|_| PairingError::Client("pairing timeout (15s)".to_string()))?
    .map_err(|e| PairingError::Client(e.to_string()))?;

    tracing::info!("pairing completado, sesión RTSP abierta");
    Ok(PairedSession { connection })
}
