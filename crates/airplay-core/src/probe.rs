//! Verificación de un dispositivo AirPlay por IP (sin pasar por mDNS).
//!
//! En redes donde mDNS no se propaga (típicamente routers que no reflejan
//! multicast entre las bandas 2.4 y 5 GHz, o redes corporativas), el usuario
//! introduce la IP del HomePod manualmente. Antes de aceptarla hacemos:
//!
//! 1. Un connect TCP al puerto AirPlay (7000 por defecto). Si falla, no hay
//!    nada útil escuchando ahí.
//! 2. Un `OPTIONS * RTSP/1.0` con cabecera `User-Agent`. AirPlay 2 responde
//!    `RTSP/1.0 200 OK` con cabeceras propias (`Server: AirTunes/...`).
//!
//! Esto basta para confirmar que el endpoint es un receptor AirPlay y para
//! sacar la versión (firmware) que el HomePod expone.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use thiserror::Error;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::timeout;

use crate::discovery::{Device, DeviceKind};

pub const DEFAULT_AIRPLAY_PORT: u16 = 7000;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ManualEndpointError {
    #[error("the endpoint is empty")]
    Empty,
    #[error("port 0 is not valid")]
    ZeroPort,
    #[error("'{0}' is not an IP address or IP:port endpoint")]
    Invalid(String),
}

/// Parses a manual AirPlay endpoint while retaining compatibility with callers
/// that already pass the address and port separately. A port embedded in the
/// endpoint wins over `fallback_port`.
pub fn parse_manual_endpoint(
    input: &str,
    fallback_port: Option<u16>,
) -> Result<(IpAddr, u16), ManualEndpointError> {
    let input = input.trim();
    if input.is_empty() {
        return Err(ManualEndpointError::Empty);
    }

    if let Ok(ip) = input.parse::<IpAddr>() {
        let port = fallback_port.unwrap_or(DEFAULT_AIRPLAY_PORT);
        if port == 0 {
            return Err(ManualEndpointError::ZeroPort);
        }
        return Ok((ip, port));
    }

    let endpoint = input
        .parse::<SocketAddr>()
        .map_err(|_| ManualEndpointError::Invalid(input.to_string()))?;
    if endpoint.port() == 0 {
        return Err(ManualEndpointError::ZeroPort);
    }

    Ok((endpoint.ip(), endpoint.port()))
}

#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("connection to {addr} failed: {source}")]
    Connect {
        addr: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("timeout after {0:?}")]
    Timeout(Duration),
    #[error("response is not an RTSP/AirPlay reply: {0}")]
    NotAirPlay(String),
}

#[derive(Debug, Clone)]
pub struct ProbeResult {
    pub server_header: Option<String>,
    pub raw_response: String,
}

/// Conecta a `ip:port` y emite un OPTIONS RTSP para confirmar AirPlay.
pub async fn probe_airplay(ip: IpAddr, port: u16) -> Result<ProbeResult, ProbeError> {
    let addr = SocketAddr::new(ip, port);
    let connect_timeout = Duration::from_secs(2);
    let response_timeout = Duration::from_secs(3);

    let mut stream = timeout(connect_timeout, TcpStream::connect(addr))
        .await
        .map_err(|_| ProbeError::Timeout(connect_timeout))?
        .map_err(|source| ProbeError::Connect { addr, source })?;

    let request = format!(
        "OPTIONS * RTSP/1.0\r\n\
         CSeq: 1\r\n\
         User-Agent: ConexionAirPlay/0.1\r\n\
         \r\n"
    );
    stream.write_all(request.as_bytes()).await?;
    stream.flush().await?;

    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0u8; 512];
    let deadline = tokio::time::Instant::now() + response_timeout;
    loop {
        let remaining = deadline
            .checked_duration_since(tokio::time::Instant::now())
            .ok_or(ProbeError::Timeout(response_timeout))?;
        let n = timeout(remaining, stream.read(&mut chunk))
            .await
            .map_err(|_| ProbeError::Timeout(response_timeout))??;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > 8 * 1024 {
            break;
        }
    }

    let response = String::from_utf8_lossy(&buf).to_string();
    if !response.starts_with("RTSP/1.0") {
        return Err(ProbeError::NotAirPlay(
            response.lines().next().unwrap_or("").to_string(),
        ));
    }

    let server_header = response.lines().find_map(|line| {
        let mut parts = line.splitn(2, ':');
        let key = parts.next()?.trim();
        let val = parts.next()?.trim();
        key.eq_ignore_ascii_case("Server").then(|| val.to_string())
    });

    Ok(ProbeResult {
        server_header,
        raw_response: response,
    })
}

/// Construye un `Device` "manual" a partir de una IP y opcionalmente nombre/puerto.
/// No marca `supports_airplay2 = true` hasta que el probe lo confirme.
pub fn manual_device(ip: IpAddr, port: Option<u16>, name: Option<String>) -> Device {
    let port = port.unwrap_or(DEFAULT_AIRPLAY_PORT);
    let display = name.unwrap_or_else(|| format!("Dispositivo manual {ip}"));
    Device {
        id: format!("manual://{ip}:{port}"),
        name: display,
        host: ip.to_string(),
        addresses: vec![ip],
        port,
        kind: DeviceKind::OtherAirPlay,
        model: None,
        features: None,
        supports_airplay2: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn parses_bare_ipv4_with_default_port() {
        assert_eq!(
            parse_manual_endpoint("192.168.86.20", None),
            Ok((IpAddr::V4(Ipv4Addr::new(192, 168, 86, 20)), 7000))
        );
    }

    #[test]
    fn parses_ipv4_with_custom_port() {
        assert_eq!(
            parse_manual_endpoint("192.168.86.20:7453", None),
            Ok((IpAddr::V4(Ipv4Addr::new(192, 168, 86, 20)), 7453))
        );
    }

    #[test]
    fn parses_bare_and_bracketed_ipv6() {
        let localhost = IpAddr::V6(Ipv6Addr::LOCALHOST);
        assert_eq!(parse_manual_endpoint("::1", None), Ok((localhost, 7000)));
        assert_eq!(
            parse_manual_endpoint("[::1]:7453", None),
            Ok((localhost, 7453))
        );
    }

    #[test]
    fn embedded_port_wins_over_fallback() {
        assert_eq!(
            parse_manual_endpoint("192.168.1.10:1", Some(u16::MAX)),
            Ok((IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), 1))
        );
        assert_eq!(
            parse_manual_endpoint("192.168.1.10:65535", None),
            Ok((IpAddr::V4(Ipv4Addr::new(192, 168, 1, 10)), u16::MAX))
        );
    }

    #[test]
    fn rejects_invalid_endpoints_and_zero_ports() {
        assert_eq!(
            parse_manual_endpoint("", None),
            Err(ManualEndpointError::Empty)
        );
        assert_eq!(
            parse_manual_endpoint("192.168.1.10:0", None),
            Err(ManualEndpointError::ZeroPort)
        );
        assert!(matches!(
            parse_manual_endpoint("192.168.1.10:65536", None),
            Err(ManualEndpointError::Invalid(_))
        ));
        assert!(matches!(
            parse_manual_endpoint("homepod.local:7000", None),
            Err(ManualEndpointError::Invalid(_))
        ));
    }
}
