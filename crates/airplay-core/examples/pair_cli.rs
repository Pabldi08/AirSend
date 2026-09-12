//! Pairing rápido por línea de comandos para validar Hito 2.
//!
//!   cargo run -p cap-core --example pair_cli -- 192.168.1.50 "Dormitorio Pablo"
//!
//! Si todo va bien deberías ver en el HomePod el indicador de "ocupado/conectado"
//! mientras esta sesión esté viva.

use std::net::IpAddr;
use std::time::Duration;

use cap_core::pairing::{pair_homepod, DeviceDescriptor};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                tracing_subscriber::EnvFilter::new("info,ap2rs_client=debug,ap2rs_pairing=debug")
            }),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let ip: IpAddr = args
        .get(1)
        .expect("uso: pair_cli <ip> [name]")
        .parse()
        .expect("IP inválida");
    let name = args
        .get(2)
        .cloned()
        .unwrap_or_else(|| format!("HomePod {ip}"));

    let desc = DeviceDescriptor {
        ip,
        port: 7000,
        name,
        mac: None,
        model: None,
        features: None,
    };

    match pair_homepod(desc).await {
        Ok(session) => {
            println!("✓ Pairing OK. Sesión RTSP abierta.");
            println!("  Mantengo la sesión 20 s; mira el HomePod (debería estar 'ocupado').");
            drop(session.connection); // explícito por claridad
            tokio::time::sleep(Duration::from_secs(20)).await;
        }
        Err(e) => {
            eprintln!("✗ Pairing falló: {e}");
            std::process::exit(1);
        }
    }
}
