//! Smoke test de la captura: graba 5 s del audio del sistema y vuelca a WAV.
//!
//!   cargo run -p audio-capture --example dump_wav -- out.wav 5
//!
//! En Windows usa WASAPI loopback del render device default.
//! En Linux usa `parec` (PipeWire/PulseAudio monitor).
//!
//! Si el WAV resultante suena bien en VLC/Audacity, la captura está OK y
//! cualquier problema posterior es del lado AirPlay (ALAC, RTP, HomePod).

use std::fs::File;
use std::io::{BufWriter, Write};
use std::time::{Duration, Instant};

use audio_capture::{start_loopback, CaptureFormat};
use crossbeam_channel::RecvTimeoutError;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let out_path = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "out.wav".to_string());
    let duration_secs: u64 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(5);

    let fmt = CaptureFormat::AIRPLAY_DEFAULT; // 44.1k / 2ch / i16
    let (cap, rx) = match start_loopback(fmt) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("✗ captura: {e}");
            std::process::exit(1);
        }
    };
    println!("✓ captura abierta: {}", cap.name());
    println!("  -> grabando {duration_secs} s a {out_path}");
    println!("  -> reproduce algo (música, vídeo, lo que sea) AHORA");

    let mut samples_buf: Vec<i16> = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(duration_secs);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(frame) => samples_buf.extend_from_slice(&frame.samples),
            Err(RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
    }
    cap.stop();

    let n_samples = samples_buf.len();
    let n_frames = n_samples / fmt.channels as usize;
    let approx_secs = n_frames as f64 / fmt.sample_rate as f64;
    println!(
        "✓ capturadas {n_samples} samples (~{:.2} s de audio)",
        approx_secs
    );

    if let Err(e) = write_wav(&out_path, &samples_buf, fmt.sample_rate, fmt.channels) {
        eprintln!("✗ escribir WAV: {e}");
        std::process::exit(1);
    }
    println!("✓ WAV guardado en {out_path}. Ábrelo en VLC/Audacity para verificar.");
}

fn write_wav(path: &str, samples: &[i16], sample_rate: u32, channels: u16) -> std::io::Result<()> {
    let mut w = BufWriter::new(File::create(path)?);

    let byte_rate = sample_rate * channels as u32 * 2;
    let block_align = channels * 2;
    let data_bytes = (samples.len() * 2) as u32;
    let chunk_size = 36 + data_bytes;

    w.write_all(b"RIFF")?;
    w.write_all(&chunk_size.to_le_bytes())?;
    w.write_all(b"WAVE")?;
    w.write_all(b"fmt ")?;
    w.write_all(&16u32.to_le_bytes())?; // PCM subchunk size
    w.write_all(&1u16.to_le_bytes())?; // PCM format
    w.write_all(&channels.to_le_bytes())?;
    w.write_all(&sample_rate.to_le_bytes())?;
    w.write_all(&byte_rate.to_le_bytes())?;
    w.write_all(&block_align.to_le_bytes())?;
    w.write_all(&16u16.to_le_bytes())?; // bits per sample
    w.write_all(b"data")?;
    w.write_all(&data_bytes.to_le_bytes())?;
    for &s in samples {
        w.write_all(&s.to_le_bytes())?;
    }
    w.flush()?;
    Ok(())
}
