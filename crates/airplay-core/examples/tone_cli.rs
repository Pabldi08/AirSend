//! Reproduce un tono sintético en el HomePod para validar end-to-end RTP + cifrado + timing.
//!
//!   cargo run -p cap-core --example tone_cli -- 192.168.1.50 5 440 0.2
//!
//! args: <ip> [duración_seg=5] [freq_hz=440] [amplitud_0..1=0.2]

use std::net::IpAddr;
use std::time::Duration;

use cap_core::pairing::DeviceDescriptor;
use cap_core::streaming::{open_live_stream, play_test_tone};

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,airplay_client=info,airplay_audio=info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let ip: IpAddr = args
        .get(1)
        .expect("uso: tone_cli <ip> [duración_seg=5] [freq=440] [amp=0.2]")
        .parse()
        .expect("IP inválida");
    let duration_secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);
    let freq: f32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(440.0);
    let amp: f32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(0.2);

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
            eprintln!("✗ no se pudo abrir stream: {e}");
            std::process::exit(1);
        }
    };
    println!(
        "✓ Stream abierto ({} Hz / {} ch). Reproduciendo {} Hz durante {} s.",
        handle.sample_rate(),
        handle.channels(),
        freq,
        duration_secs
    );

    if let Err(e) = play_test_tone(&handle, freq, Duration::from_secs(duration_secs), amp).await {
        eprintln!("✗ error reproduciendo: {e}");
        std::process::exit(1);
    }

    // Damos un margen para que se vacíen los buffers de red.
    tokio::time::sleep(Duration::from_millis(500)).await;
    drop(handle);
    println!("✓ Tono completado.");
}
