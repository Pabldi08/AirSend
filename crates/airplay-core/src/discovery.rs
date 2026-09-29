use std::collections::HashMap;
use std::net::IpAddr;
use std::time::Duration;

use mdns_sd::{ServiceDaemon, ServiceEvent};
use serde::Serialize;
use thiserror::Error;
use tokio::sync::mpsc;

const SVC_AIRPLAY: &str = "_airplay._tcp.local.";
const SVC_RAOP: &str = "_raop._tcp.local.";

#[derive(Debug, Error)]
pub enum DiscoveryError {
    #[error("mDNS daemon error: {0}")]
    Daemon(#[from] mdns_sd::Error),
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    HomePod,
    AppleTv,
    AirportExpress,
    OtherAirPlay,
}

impl DeviceKind {
    fn from_model(model: Option<&str>) -> Self {
        match model {
            Some(m) if m.starts_with("AudioAccessory") => Self::HomePod,
            Some(m) if m.starts_with("AppleTV") => Self::AppleTv,
            Some(m) if m.starts_with("AirPort") => Self::AirportExpress,
            _ => Self::OtherAirPlay,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub id: String,
    pub hardware_id: Option<String>,
    pub name: String,
    pub host: String,
    pub addresses: Vec<IpAddr>,
    pub port: u16,
    pub kind: DeviceKind,
    pub model: Option<String>,
    pub features: Option<String>,
    pub supports_airplay2: bool,
    pub available: bool,
    pub manual: bool,
    pub server_header: Option<String>,
    pub group_id: Option<String>,
    pub tight_sync_id: Option<String>,
    pub group_name: Option<String>,
    pub is_group_leader: bool,
    /// Keep receiver capabilities on the backend; diagnostics export never includes these.
    #[serde(skip_serializing)]
    pub txt: HashMap<String, String>,
}

impl Device {
    pub fn receiver_id(&self) -> String {
        self.hardware_id
            .as_ref()
            .map(|id| format!("hardware:{id}"))
            .unwrap_or_else(|| {
                if self.manual {
                    self.id.clone()
                } else {
                    format!(
                        "host:{}|{}",
                        self.host.to_lowercase(),
                        self.name
                            .split('@')
                            .next_back()
                            .unwrap_or(&self.name)
                            .to_lowercase()
                    )
                }
            })
    }

    pub fn from_txt(
        id: String,
        name: String,
        host: String,
        addresses: Vec<IpAddr>,
        port: u16,
        txt: HashMap<String, String>,
    ) -> Self {
        let model = txt.get("model").or_else(|| txt.get("am")).cloned();
        let hardware_id = txt
            .get("deviceid")
            .and_then(|id| normalize_hardware_id(id))
            .or_else(|| {
                id.contains(SVC_RAOP)
                    .then(|| name.split('@').next())
                    .flatten()
                    .and_then(normalize_hardware_id)
            });
        let features = txt.get("features").or_else(|| txt.get("ft")).cloned();
        let supports_airplay2 = features
            .as_deref()
            .and_then(|s| ap2rs_core::features::Features::from_txt_value(s).ok())
            .map(|f| f.supports_buffered_audio() || f.supports_ptp())
            .unwrap_or(false);
        Self {
            id,
            hardware_id,
            name,
            host,
            addresses,
            port,
            kind: DeviceKind::from_model(model.as_deref()),
            model,
            features,
            supports_airplay2,
            available: true,
            manual: false,
            server_header: None,
            group_id: txt.get("gid").cloned(),
            tight_sync_id: txt.get("tsid").cloned(),
            group_name: txt.get("gpn").cloned(),
            is_group_leader: txt.get("igl").is_some_and(|v| v == "1" || v == "true"),
            txt,
        }
    }

    pub fn descriptor(
        &self,
        ip: IpAddr,
        peers: &[Device],
    ) -> Result<crate::pairing::DeviceDescriptor, crate::pairing::PairingError> {
        let mut descriptor = crate::pairing::DeviceDescriptor {
            ip,
            port: self.port,
            name: self
                .name
                .split('@')
                .next_back()
                .unwrap_or(&self.name)
                .to_owned(),
            mac: self.hardware_id.clone(),
            model: self.model.clone(),
            features: self.features.clone(),
            advertised: None,
        };
        if self.manual {
            return Ok(descriptor);
        }
        let mut txt = HashMap::new();
        for peer in peers
            .iter()
            .filter(|p| p.receiver_id() == self.receiver_id())
        {
            txt.extend(peer.txt.clone());
        }
        txt.extend(self.txt.clone());
        // Parse a single combined advertisement so group and RAOP metadata coexist.
        if let Some(mac) = &self.hardware_id {
            txt.insert("deviceid".into(), mac.clone());
        }
        if let Some(model) = txt.get("am").cloned() {
            txt.entry("model".into()).or_insert(model);
        }
        if let Some(features) = txt.get("ft").cloned() {
            txt.entry("features".into()).or_insert(features);
        }
        if let Some(version) = txt.get("vs").cloned() {
            txt.entry("srcvers".into()).or_insert(version);
        }
        let mut advertised = ap2rs_discovery::TxtRecordParser::parse_airplay_txt(
            &descriptor.name,
            &txt,
            vec![ip],
            self.port,
        )
        .map_err(|e| crate::pairing::PairingError::Client(format!("receiver capabilities: {e}")))?;
        let list = |key: &str| {
            txt.get(key)
                .map(|s| s.split(',').filter_map(|v| v.trim().parse().ok()).collect())
        };
        advertised.raop_codecs = list("cn");
        advertised.raop_encryption_types = list("et");
        advertised.raop_transport = txt.get("tp").cloned();
        advertised.raop_digest_auth = txt.get("da").is_some_and(|v| v == "true" || v == "1");
        advertised.raop_port = peers
            .iter()
            .find(|p| p.receiver_id() == self.receiver_id() && p.id.contains(SVC_RAOP))
            .map(|p| p.port);
        descriptor.advertised = Some(advertised);
        Ok(descriptor)
    }
}

#[derive(Debug, Clone)]
pub enum DiscoveryEvent {
    Resolved(Device),
    Removed(String),
}

pub struct Discovery {
    daemon: ServiceDaemon,
}

impl Discovery {
    pub fn new() -> Result<Self, DiscoveryError> {
        let daemon = ServiceDaemon::new()?;
        Ok(Self { daemon })
    }

    /// Inicia browsing y emite dispositivos por el canal según se descubren.
    /// Se cancela cerrando el receptor o llamando a `shutdown`.
    pub fn browse_events(&self) -> Result<mpsc::UnboundedReceiver<DiscoveryEvent>, DiscoveryError> {
        let (tx, rx) = mpsc::unbounded_channel();
        for svc in [SVC_AIRPLAY, SVC_RAOP] {
            let receiver = self.daemon.browse(svc)?;
            let tx = tx.clone();
            tokio::spawn(async move {
                while let Ok(event) = receiver.recv_async().await {
                    let event = match event {
                        ServiceEvent::ServiceResolved(info) => {
                            let txt = info
                                .get_properties()
                                .iter()
                                .map(|p| (p.key().to_string(), p.val_str().to_string()))
                                .collect();
                            DiscoveryEvent::Resolved(Device::from_txt(
                                info.get_fullname().to_string(),
                                info.get_fullname()
                                    .split('.')
                                    .next()
                                    .unwrap_or("?")
                                    .to_string(),
                                info.get_hostname().to_string(),
                                info.get_addresses().iter().copied().collect(),
                                info.get_port(),
                                txt,
                            ))
                        }
                        ServiceEvent::ServiceRemoved(_, id) => DiscoveryEvent::Removed(id),
                        _ => continue,
                    };
                    if tx.send(event).is_err() {
                        break;
                    }
                }
            });
        }
        Ok(rx)
    }

    pub fn browse(&self) -> Result<mpsc::UnboundedReceiver<Device>, DiscoveryError> {
        let mut events = self.browse_events()?;
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                if let DiscoveryEvent::Resolved(device) = event {
                    if tx.send(device).is_err() {
                        break;
                    }
                }
            }
        });
        Ok(rx)
    }

    pub fn shutdown(&self) {
        let _ = self.daemon.shutdown();
    }
}

pub fn normalize_hardware_id(id: &str) -> Option<String> {
    let hex: String = id.chars().filter(|c| *c != ':' && *c != '-').collect();
    (hex.len() == 12 && hex.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| hex.to_ascii_lowercase())
}

#[cfg(test)]
mod tests {
    use super::normalize_hardware_id;

    #[test]
    fn normalizes_airplay_and_raop_device_ids() {
        assert_eq!(
            normalize_hardware_id("00:06:78:AA:BB:CC"),
            Some("000678aabbcc".into())
        );
        assert_eq!(
            normalize_hardware_id("000678AABBCC"),
            Some("000678aabbcc".into())
        );
        assert_eq!(normalize_hardware_id("Denon"), None);
    }
}

/// Lanza un browse de una sola pasada con timeout y devuelve la lista acumulada.
/// Útil para tests y para el primer fetch de la UI.
pub async fn browse_once(timeout: Duration) -> Result<Vec<Device>, DiscoveryError> {
    let discovery = Discovery::new()?;
    let mut rx = discovery.browse_events()?;

    let mut devices: HashMap<String, Device> = HashMap::new();
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);

    loop {
        tokio::select! {
            _ = &mut deadline => break,
            maybe = rx.recv() => {
                match maybe {
                    Some(DiscoveryEvent::Resolved(device)) => { devices.insert(device.id.clone(), device); }
                    Some(DiscoveryEvent::Removed(id)) => { devices.remove(&id); }
                    None => break,
                }
            }
        }
    }

    discovery.shutdown();
    Ok(devices.into_values().collect())
}

impl Drop for Discovery {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod capability_tests {
    use super::*;
    #[test]
    fn denon_txt_keeps_codec_auth_model_and_identity_across_routes() {
        let ip = "192.0.2.93".parse().unwrap();
        let raop = Device::from_txt(
            "000678aabbcc@Denon._raop._tcp.local.".into(),
            "000678aabbcc@Denon".into(),
            "HEOS-Player.local".into(),
            vec![ip],
            7000,
            HashMap::from([
                ("am".into(), "Denon AVC-X4800H".into()),
                ("ft".into(), "0x445F8A00,0x801C340".into()),
                ("vs".into(), "366.0".into()),
                ("cn".into(), "0,1".into()),
                ("et".into(), "0,4".into()),
                ("da".into(), "true".into()),
                ("tp".into(), "UDP".into()),
            ]),
        );
        let airplay = Device::from_txt(
            "Denon._airplay._tcp.local.".into(),
            "Denon".into(),
            "HEOS-Player.local".into(),
            vec![ip],
            7000,
            HashMap::from([
                ("deviceid".into(), "00:06:78:AA:BB:CC".into()),
                ("model".into(), "Denon AVC-X4800H".into()),
                ("features".into(), "0x445F8A00,0x801C340".into()),
                ("srcvers".into(), "366.0".into()),
                ("acl".into(), "0".into()),
            ]),
        );
        assert_eq!(raop.receiver_id(), airplay.receiver_id());
        assert_eq!(raop.kind, DeviceKind::OtherAirPlay);
        let descriptor = raop.descriptor(ip, &[raop.clone(), airplay]).unwrap();
        let device = descriptor.into_ap2_device().unwrap();
        assert_eq!(device.model, "Denon AVC-X4800H");
        assert_eq!(device.raop_codecs, Some(vec![0, 1]));
        assert_eq!(device.raop_encryption_types, Some(vec![0, 4]));
        assert!(device.raop_digest_auth);
        assert_eq!(device.access_control, Some(0));
        assert_eq!(device.source_version.major, 366);
    }
    #[test]
    fn unknown_receiver_is_not_advertised_as_a_homepod() {
        let descriptor = crate::pairing::DeviceDescriptor {
            ip: "192.0.2.1".parse().unwrap(),
            port: 7000,
            name: "Unknown".into(),
            mac: None,
            model: None,
            features: None,
            advertised: None,
        };
        let device = descriptor.into_ap2_device().unwrap();
        assert!(device.model.is_empty());
        assert_eq!(device.features.0, 0);
    }
}
