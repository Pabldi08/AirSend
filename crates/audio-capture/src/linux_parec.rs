//! Captura del audio del sistema usando `parec` (PulseAudio/PipeWire CLI).
//!
//! `parec` viene con `pulseaudio-utils` (también disponible en setups PipeWire
//! que llevan compatibilidad PA). Apuntamos al monitor del sink default con
//! `--device=@DEFAULT_MONITOR@`, formato S16LE 44.1k estéreo — exactamente lo
//! que pide AirPlay.
//!
//! Lanzamos `parec` como subproceso, leemos su stdout en un hilo dedicado y
//! convertimos cada bloque de bytes en un `CapturedFrame` que se publica por
//! un canal `crossbeam`.

use std::io::Read;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver};

use crate::{Capture, CaptureError, CaptureFormat, CapturedFrame};

// 352 frames @ 44.1k = ~8 ms por chunk, exactamente un paquete RTP/ALAC
// (`frames_per_packet = 352`). Alinear la captura al tamaño de paquete del
// encoder hace que cada chunk capturado se mapee 1:1 a un paquete enviado: el
// ritmo de salida a la red es regular en vez de arrastrar restos. Antes 512 →
// el encoder troceaba a 352 y dejaba 160 frames colgando, con jitter de pacing
// que vacía el buffer del receptor y "robotiza". También recorta ~3.6 ms de
// latencia de captura. Igualado con `CHUNK_FRAMES` del backend WASAPI para que
// el pipeline vea el mismo tamaño en ambos OS.
const READ_CHUNK_FRAMES: usize = 352;

pub struct ParecCapture {
    name: String,
    running: Arc<AtomicBool>,
    child: Option<Child>,
    handle: Option<thread::JoinHandle<()>>,
}

impl Capture for ParecCapture {
    fn name(&self) -> &str {
        &self.name
    }
    fn stop(mut self: Box<Self>) {
        self.shutdown();
    }
}

impl ParecCapture {
    fn shutdown(&mut self) {
        self.running.store(false, Ordering::SeqCst);
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for ParecCapture {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn available() -> bool {
    which("parec").is_some()
}

fn which(bin: &str) -> Option<std::path::PathBuf> {
    std::env::var_os("PATH")?
        .to_string_lossy()
        .split(':')
        .map(|p| std::path::PathBuf::from(p).join(bin))
        .find(|p| p.is_file())
}

pub fn start(
    fmt: CaptureFormat,
) -> Result<(Box<dyn Capture>, Receiver<CapturedFrame>), CaptureError> {
    if fmt.channels != 2 {
        return Err(CaptureError::UnsupportedConfig {
            wanted: fmt.sample_rate,
            channels: fmt.channels,
        });
    }

    // Resolvemos el nombre del monitor default — `@DEFAULT_MONITOR@` también
    // funciona pero loguear el nombre real ayuda al diagnóstico.
    let default_monitor = resolve_default_monitor().unwrap_or_else(|| "@DEFAULT_MONITOR@".into());
    tracing::info!(device = %default_monitor, "parec: usando monitor");

    let rate_arg = format!("--rate={}", fmt.sample_rate);
    // `--latency-msec=5` pide a PipeWire/PA que entregue bloques de ~5 ms
    // (mucho menos de los ~20-100 ms por defecto). Algunos servidores no lo
    // respetan exactamente pero suele bajar bastante; no se nota artefacto.
    let mut child = Command::new("parec")
        .args([
            "--format=s16le",
            &rate_arg,
            "--channels=2",
            "--client-name=ConexionAirPlay",
            &format!("--device={}", default_monitor),
            "--latency-msec=5",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| CaptureError::Backend(format!("parec no se pudo lanzar: {e}")))?;

    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| CaptureError::Backend("parec sin stdout".into()))?;

    let (tx, rx) = bounded::<CapturedFrame>(64);
    let running = Arc::new(AtomicBool::new(true));
    let running_thread = running.clone();
    let sample_rate = fmt.sample_rate;

    let handle = thread::Builder::new()
        .name("audio-capture-parec".into())
        .spawn(move || {
            const BYTES_PER_FRAME: usize = 2 * 2; // i16 * 2 canales
            let chunk_bytes = READ_CHUNK_FRAMES * BYTES_PER_FRAME;
            let mut buf = vec![0u8; chunk_bytes];
            // Acumulador para arrastrar los bytes de un frame que el pipe haya
            // partido entre dos lecturas. Sin esto, una lectura no alineada a
            // frame (4 bytes) perdería parte de un sample y desincronizaría L/R
            // de forma permanente — se oye como clicks. Sólo retiene < 1 frame.
            let mut acc: Vec<u8> = Vec::with_capacity(chunk_bytes + BYTES_PER_FRAME);
            while running_thread.load(Ordering::SeqCst) {
                match stdout.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        acc.extend_from_slice(&buf[..n]);
                        // Sólo convertimos frames completos; el remanente queda
                        // en `acc` para la próxima lectura.
                        let usable = acc.len() - (acc.len() % BYTES_PER_FRAME);
                        if usable == 0 {
                            continue;
                        }
                        let mut samples = Vec::with_capacity(usable / 2);
                        for s in acc[..usable].chunks_exact(2) {
                            samples.push(i16::from_le_bytes([s[0], s[1]]));
                        }
                        acc.drain(..usable);
                        let _ = tx.try_send(CapturedFrame {
                            samples,
                            channels: 2,
                            sample_rate,
                        });
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "parec stdout read error");
                        break;
                    }
                }
            }
            tracing::info!("parec reader thread exit");
        })
        .map_err(|e| CaptureError::Backend(e.to_string()))?;

    // Pequeña espera para detectar fallo inmediato de parec (p.ej. sin device).
    thread::sleep(Duration::from_millis(150));
    if let Ok(Some(status)) = child.try_wait() {
        return Err(CaptureError::Backend(format!(
            "parec salió pronto con status {status}"
        )));
    }

    let cap: Box<dyn Capture> = Box::new(ParecCapture {
        name: default_monitor,
        running,
        child: Some(child),
        handle: Some(handle),
    });
    Ok((cap, rx))
}

fn resolve_default_monitor() -> Option<String> {
    let output = Command::new("pactl").arg("info").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let info = String::from_utf8_lossy(&output.stdout);
    let sink = info
        .lines()
        .find(|l| l.starts_with("Default Sink:"))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())?;
    Some(format!("{sink}.monitor"))
}
