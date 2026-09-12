//! Captura en Linux usando cpal contra el "monitor" del sink default de PipeWire.
//!
//! cpal::Stream no es Send, así que vive en un hilo dedicado. El hilo se para
//! cuando recibe la señal `stop` por su canal.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, StreamConfig};
use crossbeam_channel::{bounded, Receiver, Sender};

use crate::{Capture, CaptureError, CaptureFormat, CapturedFrame};

pub struct LinuxCapture {
    name: String,
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Capture for LinuxCapture {
    fn name(&self) -> &str {
        &self.name
    }
    fn stop(mut self: Box<Self>) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for LinuxCapture {
    fn drop(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

pub fn start(
    fmt: CaptureFormat,
) -> Result<(Box<dyn Capture>, Receiver<CapturedFrame>), CaptureError> {
    let (tx, rx) = bounded::<CapturedFrame>(64);
    let (ready_tx, ready_rx) = bounded::<Result<String, CaptureError>>(1);
    let running = Arc::new(AtomicBool::new(true));
    let running_thread = running.clone();

    let handle = thread::Builder::new()
        .name("audio-capture-cpal".into())
        .spawn(move || run_capture(fmt, tx, ready_tx, running_thread))
        .map_err(|e| CaptureError::Backend(e.to_string()))?;

    let chosen_name = ready_rx
        .recv()
        .map_err(|e| CaptureError::Backend(e.to_string()))??;

    let cap: Box<dyn Capture> = Box::new(LinuxCapture {
        name: chosen_name,
        running,
        handle: Some(handle),
    });
    Ok((cap, rx))
}

fn run_capture(
    fmt: CaptureFormat,
    tx: Sender<CapturedFrame>,
    ready: Sender<Result<String, CaptureError>>,
    running: Arc<AtomicBool>,
) {
    let host = cpal::default_host();
    let devices = match host.input_devices() {
        Ok(d) => d,
        Err(e) => {
            let _ = ready.send(Err(CaptureError::Backend(e.to_string())));
            return;
        }
    };

    let mut chosen: Option<cpal::Device> = None;
    let mut chosen_name = String::new();
    for d in devices {
        let n = d.name().unwrap_or_default();
        if n.to_lowercase().contains("monitor") {
            tracing::info!(device = %n, "encontrado monitor PipeWire/PulseAudio");
            chosen_name = n;
            chosen = Some(d);
            break;
        }
    }
    let device = match chosen.or_else(|| host.default_input_device()) {
        Some(d) => d,
        None => {
            let _ = ready.send(Err(CaptureError::NoDevice));
            return;
        }
    };
    if chosen_name.is_empty() {
        chosen_name = device.name().unwrap_or_else(|_| "default".to_string());
        tracing::warn!(
            device = %chosen_name,
            "no se encontró 'monitor', uso input por defecto (probablemente micrófono)"
        );
    }

    let supported = match device.default_input_config() {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(CaptureError::Backend(e.to_string())));
            return;
        }
    };
    let sample_format = supported.sample_format();
    let config: StreamConfig = supported.clone().into();
    let in_rate = config.sample_rate.0;
    let in_channels = config.channels;

    tracing::info!(
        sample_rate = in_rate,
        channels = in_channels,
        format = ?sample_format,
        "abriendo captura"
    );

    let want_rate = fmt.sample_rate;
    let want_channels = fmt.channels;
    let err_fn = |e| tracing::error!(error = %e, "cpal stream error");

    let stream_result = match sample_format {
        SampleFormat::F32 => {
            let tx_cb = tx.clone();
            device.build_input_stream(
                &config,
                move |data: &[f32], _| {
                    forward_f32(data, in_channels, in_rate, want_channels, want_rate, &tx_cb)
                },
                err_fn,
                None,
            )
        }
        SampleFormat::I16 => {
            let tx_cb = tx.clone();
            device.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    forward_i16(data, in_channels, in_rate, want_channels, want_rate, &tx_cb)
                },
                err_fn,
                None,
            )
        }
        SampleFormat::U16 => {
            let tx_cb = tx.clone();
            device.build_input_stream(
                &config,
                move |data: &[u16], _| {
                    let converted: Vec<i16> =
                        data.iter().map(|&u| (u as i32 - 32768) as i16).collect();
                    forward_i16(
                        &converted,
                        in_channels,
                        in_rate,
                        want_channels,
                        want_rate,
                        &tx_cb,
                    )
                },
                err_fn,
                None,
            )
        }
        other => {
            let _ = ready.send(Err(CaptureError::Backend(format!(
                "formato de muestra no soportado: {:?}",
                other
            ))));
            return;
        }
    };

    let stream = match stream_result {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(CaptureError::Backend(e.to_string())));
            return;
        }
    };

    if let Err(e) = stream.play() {
        let _ = ready.send(Err(CaptureError::Backend(e.to_string())));
        return;
    }

    let _ = ready.send(Ok(chosen_name));

    // El stream corre en hilos internos de cpal. Aquí simplemente esperamos
    // hasta que se pida parar, manteniendo `stream` vivo en este scope.
    while running.load(Ordering::SeqCst) {
        thread::park_timeout(std::time::Duration::from_millis(200));
    }
    drop(stream);
    tracing::info!("captura detenida");
}

fn forward_f32(
    data: &[f32],
    in_channels: u16,
    in_rate: u32,
    want_channels: u16,
    want_rate: u32,
    tx: &Sender<CapturedFrame>,
) {
    let frames = data.len() / in_channels as usize;
    let mut out: Vec<i16> = Vec::with_capacity(frames * want_channels as usize);
    for f in 0..frames {
        let base = f * in_channels as usize;
        let l = data[base];
        let r = if in_channels >= 2 { data[base + 1] } else { l };
        let mix = |x: f32| (x.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        out.push(mix(l));
        if want_channels >= 2 {
            out.push(mix(r));
        }
    }
    let resampled = if in_rate == want_rate {
        out
    } else {
        resample_linear(&out, want_channels as usize, in_rate, want_rate)
    };
    let _ = tx.try_send(CapturedFrame {
        samples: resampled,
        channels: want_channels,
        sample_rate: want_rate,
    });
}

fn forward_i16(
    data: &[i16],
    in_channels: u16,
    in_rate: u32,
    want_channels: u16,
    want_rate: u32,
    tx: &Sender<CapturedFrame>,
) {
    let frames = data.len() / in_channels as usize;
    let mut out: Vec<i16> = Vec::with_capacity(frames * want_channels as usize);
    for f in 0..frames {
        let base = f * in_channels as usize;
        let l = data[base];
        let r = if in_channels >= 2 { data[base + 1] } else { l };
        out.push(l);
        if want_channels >= 2 {
            out.push(r);
        }
    }
    let resampled = if in_rate == want_rate {
        out
    } else {
        resample_linear(&out, want_channels as usize, in_rate, want_rate)
    };
    let _ = tx.try_send(CapturedFrame {
        samples: resampled,
        channels: want_channels,
        sample_rate: want_rate,
    });
}

/// Resampling lineal interleaved (rápido y sin deps). Para Hito 3 vale; en
/// Hito 5 lo cambiamos por airplay-resampler (sinc de mejor calidad).
fn resample_linear(input: &[i16], channels: usize, src_rate: u32, dst_rate: u32) -> Vec<i16> {
    if src_rate == dst_rate || input.is_empty() {
        return input.to_vec();
    }
    let src_frames = input.len() / channels;
    let dst_frames = ((src_frames as u64 * dst_rate as u64) / src_rate as u64).max(1) as usize;
    let mut out = Vec::with_capacity(dst_frames * channels);
    let ratio = src_rate as f64 / dst_rate as f64;

    for d in 0..dst_frames {
        let src_pos = d as f64 * ratio;
        let src_i = src_pos.floor() as usize;
        let frac = src_pos - src_i as f64;
        let src_i_next = (src_i + 1).min(src_frames - 1);
        for ch in 0..channels {
            let a = input[src_i * channels + ch] as f64;
            let b = input[src_i_next * channels + ch] as f64;
            let v = a + (b - a) * frac;
            out.push(v.clamp(i16::MIN as f64, i16::MAX as f64) as i16);
        }
    }
    out
}
