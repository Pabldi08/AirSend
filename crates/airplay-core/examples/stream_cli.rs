//! Captura el audio del sistema (PipeWire monitor en Linux) y lo manda
//! al HomePod indicado. Esto es el "modo demo" del MVP en Linux.
//!
//!   cargo run -p cap-core --example stream_cli -- 192.168.1.50 30
//!
//! Pon música en Spotify/YouTube/lo que sea ANTES de lanzar y debería sonar
//! en el HomePod durante <duración> segundos.

use std::net::IpAddr;
use std::time::{Duration, Instant};

use audio_capture::{start_loopback, CaptureFormat};
use crossbeam_channel::RecvTimeoutError;
use cap_core::pairing::DeviceDescriptor;
use cap_core::streaming::open_live_stream;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,airplay_client=info,airplay_audio=warn,audio_capture=info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let ip: IpAddr = args
        .get(1)
        .expect("uso: stream_cli <ip> [duración_seg=30]")
        .parse()
        .expect("IP inválida");
    let duration_secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(30);

    // 1. Captura del sistema (PipeWire monitor).
    let (cap, rx) = match start_loopback(CaptureFormat::AIRPLAY_DEFAULT) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("✗ captura: {e}");
            std::process::exit(1);
        }
    };
    println!("✓ captura: {}", cap.name());

    // 2. Abre stream al HomePod (pair + setup + RTP listo).
    let desc = DeviceDescriptor {
        ip,
        port: 7000,
        name: format!("HomePod {ip}"),
        mac: None,
        model: None,
        features: None,
    };
    let handle = match open_live_stream(desc, None, None).await {
        Ok(h) => h,
        Err(e) => {
            eprintln!("✗ stream: {e}");
            std::process::exit(1);
        }
    };
    println!("✓ stream abierto. Reproduce algo en el PC durante {duration_secs}s.");

    // 3. Bombea frames desde la captura al stream.
    let deadline = Instant::now() + Duration::from_secs(duration_secs);
    let mut total_samples = 0u64;
    let mut dropped = 0u64;
    while Instant::now() < deadline {
        // recv con timeout para poder salir limpio.
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(frame) => {
                total_samples += frame.samples.len() as u64;
                if !handle.push_pcm(frame.samples) {
                    dropped += 1;
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }

    cap.stop();
    drop(handle);
    let approx_secs = total_samples as f64 / 2.0 / 44_100.0;
    println!(
        "✓ stop. samples capturadas={} (~{:.1}s de audio), drops={}",
        total_samples, approx_secs, dropped
    );
}
