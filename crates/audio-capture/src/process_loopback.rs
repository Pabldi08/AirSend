//! WASAPI capture for an explicitly selected executable and its process trees.
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    mem::ManuallyDrop,
    path::Path,
    ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};

use windows::{
    core::{implement, Interface, Result as WinResult, HRESULT, PROPVARIANT, PWSTR},
    Win32::{
        Foundation::{CloseHandle, FILETIME, HANDLE, WAIT_TIMEOUT},
        Media::Audio::*,
        System::{
            Com::{CoCreateInstance, IAgileObject, IAgileObject_Impl, CLSCTX_ALL},
            Diagnostics::ToolHelp::*,
            Threading::{
                CreateEventW, GetProcessTimes, OpenProcess, QueryFullProcessImageNameW,
                WaitForSingleObject, PROCESS_NAME_FORMAT, PROCESS_QUERY_LIMITED_INFORMATION,
                PROCESS_SYNCHRONIZE,
            },
        },
    },
};

use crate::{
    endpoints::ComGuard, windows::CHUNK_FRAMES, Capture, CaptureError, CaptureFormat,
    CapturePolicy, CaptureReceiver, CaptureSender, CapturedFrame,
};

const REFRESH: Duration = Duration::from_secs(1);
const ACTIVATION_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_AFTER_FAILURE: Duration = Duration::from_secs(5);
const MAX_QUEUE_FRAMES: usize = 8 * CHUNK_FRAMES;

struct OwnedHandle(HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

#[derive(serde::Serialize)]
pub struct AudioApp {
    pub path: String,
    pub name: String,
}

struct Process {
    handle: OwnedHandle,
    path: String,
    created: u64,
}

impl Process {
    fn open(pid: u32) -> Option<Self> {
        let handle = OwnedHandle(
            unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    false,
                    pid,
                )
            }
            .ok()?,
        );
        let mut path = vec![0; 32_768];
        let mut len = path.len() as u32;
        let (mut created, mut exited, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        unsafe {
            QueryFullProcessImageNameW(
                handle.0,
                PROCESS_NAME_FORMAT(0),
                PWSTR(path.as_mut_ptr()),
                &mut len,
            )
            .ok()?;
            GetProcessTimes(handle.0, &mut created, &mut exited, &mut kernel, &mut user).ok()?;
        }
        let process = Self {
            handle,
            path: String::from_utf16(&path[..len as usize]).ok()?,
            created: (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime),
        };
        process.alive().then_some(process)
    }

    fn alive(&self) -> bool {
        unsafe { WaitForSingleObject(self.handle.0, 0) == WAIT_TIMEOUT }
    }
}

fn active_pids() -> WinResult<HashSet<u32>> {
    let mut pids = HashSet::new();
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let outputs = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        for index in 0..outputs.GetCount()? {
            let Ok(device) = outputs.Item(index) else {
                continue;
            };
            let Ok(manager) = device.Activate::<IAudioSessionManager2>(CLSCTX_ALL, None) else {
                continue;
            };
            let Ok(sessions) = manager.GetSessionEnumerator() else {
                continue;
            };
            for index in 0..sessions.GetCount().unwrap_or(0) {
                let Ok(session) = sessions.GetSession(index) else {
                    continue;
                };
                if session.GetState().ok() != Some(AudioSessionStateActive) {
                    continue;
                }
                let Ok(session) = session.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                if session.IsSystemSoundsSession() != HRESULT(1) {
                    continue;
                }
                // GetProcessId may return a success code for cross-process sessions.
                // In that case the returned PID identifies the initial process only.
                let mut pid = 0;
                let status = (session.vtable().GetProcessId)(session.as_raw(), &mut pid);
                if status == HRESULT(0) && pid != 0 && pid != std::process::id() {
                    pids.insert(pid);
                }
            }
        }
    }
    Ok(pids)
}

pub fn list_apps() -> Result<Vec<AudioApp>, CaptureError> {
    let _com = ComGuard::new()?;
    let pids = active_pids().map_err(|error| CaptureError::Backend(error.to_string()))?;
    let mut apps = BTreeMap::new();
    for process in pids.into_iter().filter_map(Process::open) {
        let name = Path::new(&process.path)
            .file_stem()
            .and_then(|name| name.to_str())
            .unwrap_or(&process.path)
            .to_owned();
        apps.entry(process.path.to_lowercase()).or_insert(AudioApp {
            path: process.path,
            name,
        });
    }
    Ok(apps.into_values().collect())
}

fn process_parents() -> WinResult<HashMap<u32, u32>> {
    let snapshot = OwnedHandle(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0)? });
    let mut entry = PROCESSENTRY32W {
        dwSize: size_of::<PROCESSENTRY32W>() as u32,
        ..Default::default()
    };
    let mut parents = HashMap::new();
    unsafe {
        Process32FirstW(snapshot.0, &mut entry)?;
        loop {
            parents.insert(entry.th32ProcessID, entry.th32ParentProcessID);
            if Process32NextW(snapshot.0, &mut entry).is_err() {
                break;
            }
        }
    }
    Ok(parents)
}

fn selected_roots(path: &str) -> WinResult<HashMap<u32, Process>> {
    let pids = active_pids()?;
    let parents = process_parents()?;
    let identity = path.to_lowercase();
    let mut roots = HashMap::new();
    for pid in pids {
        let Some(mut process) = Process::open(pid) else {
            continue;
        };
        if process.path.to_lowercase() != identity {
            continue;
        }
        let mut root = pid;
        // Browser/Electron helpers commonly use the same executable as the
        // browser. Climb only through live parents of the selected identity.
        while let Some(parent) = parents
            .get(&root)
            .copied()
            .and_then(|id| Process::open(id).map(|p| (id, p)))
        {
            if parent.1.created >= process.created || parent.1.path.to_lowercase() != identity {
                break;
            }
            root = parent.0;
            process = parent.1;
        }
        roots.insert(root, process);
    }
    // A selected process can also be below a helper with a different image.
    // Remove nested roots so their samples don't enter the mix twice.
    let root_times = roots
        .iter()
        .map(|(&pid, process)| (pid, process.created))
        .collect();
    let nested = nested_roots(&root_times, &parents, |pid| {
        Process::open(pid).map(|p| p.created)
    });
    for pid in nested {
        roots.remove(&pid);
    }
    Ok(roots)
}

fn nested_roots(
    roots: &HashMap<u32, u64>,
    parents: &HashMap<u32, u32>,
    created_at: impl Fn(u32) -> Option<u64>,
) -> Vec<u32> {
    roots
        .iter()
        .filter_map(|(&pid, &creation)| {
            let mut current = pid;
            let mut created = creation;
            while let Some(&parent) = parents.get(&current) {
                let Some(ancestor) = created_at(parent) else {
                    break;
                };
                if ancestor >= created {
                    break;
                }
                if roots.get(&parent) == Some(&ancestor) {
                    return Some(pid);
                }
                current = parent;
                created = ancestor;
            }
            None
        })
        .collect()
}

#[implement(IActivateAudioInterfaceCompletionHandler, IAgileObject)]
struct Activation(mpsc::SyncSender<()>);

impl IAgileObject_Impl for Activation {}

#[allow(non_snake_case)]
impl IActivateAudioInterfaceCompletionHandler_Impl for Activation {
    fn ActivateCompleted(
        &self,
        _: Option<&IActivateAudioInterfaceAsyncOperation>,
    ) -> WinResult<()> {
        let _ = self.0.try_send(());
        Ok(())
    }
}

fn activate(pid: u32) -> Result<IAudioClient, String> {
    use windows_core::imp::{BLOB, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0};
    let mut parameters = AUDIOCLIENT_ACTIVATION_PARAMS {
        ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
        Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
            ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                TargetProcessId: pid,
                ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
            },
        },
    };
    // The input blob borrows stack memory for the API call. It must not
    // be cleared as an owning PROPVARIANT.
    let variant = ManuallyDrop::new(unsafe {
        PROPVARIANT::from_raw(windows_core::imp::PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: PROPVARIANT_0_0 {
                    vt: 65, // VT_BLOB
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 {
                        blob: BLOB {
                            cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
                            pBlobData: (&mut parameters as *mut AUDIOCLIENT_ACTIVATION_PARAMS)
                                .cast(),
                        },
                    },
                },
            },
        })
    });
    let (tx, rx) = mpsc::sync_channel(1);
    let callback: IActivateAudioInterfaceCompletionHandler = Activation(tx).into();
    let operation = unsafe {
        ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&*variant),
            &callback,
        )
    }
    .map_err(|error| error.to_string())?;
    rx.recv_timeout(ACTIVATION_TIMEOUT)
        .map_err(|_| "process_loopback_activation_timeout".to_string())?;
    let mut status = HRESULT(0);
    let mut client = None;
    unsafe { operation.GetActivateResult(&mut status, &mut client) }
        .map_err(|error| error.to_string())?;
    status.ok().map_err(|error| error.to_string())?;
    client
        .ok_or("process_loopback_activation_failed")?
        .cast()
        .map_err(|error| error.to_string())
}

struct Input {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    _event: OwnedHandle,
    process: Process,
    samples: VecDeque<i16>,
}

impl Input {
    fn new(pid: u32, process: Process, fmt: CaptureFormat) -> Result<Self, String> {
        let client = activate(pid)?;
        if !process.alive() {
            return Err("selected_audio_app_unavailable".into());
        }
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: fmt.channels,
            nSamplesPerSec: fmt.sample_rate,
            nAvgBytesPerSec: fmt.sample_rate * u32::from(fmt.channels) * 2,
            nBlockAlign: fmt.channels * 2,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        unsafe {
            let flags = AUDCLNT_STREAMFLAGS_LOOPBACK
                | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
            client
                .Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 0, 0, &format, None)
                .map_err(|error| error.to_string())?;
            let event = OwnedHandle(
                CreateEventW(None, false, false, None).map_err(|error| error.to_string())?,
            );
            client
                .SetEventHandle(event.0)
                .map_err(|error| error.to_string())?;
            let capture = client
                .GetService::<IAudioCaptureClient>()
                .map_err(|error| error.to_string())?;
            client.Start().map_err(|error| error.to_string())?;
            Ok(Self {
                client,
                capture,
                _event: event,
                process,
                samples: VecDeque::new(),
            })
        }
    }

    fn drain(&mut self, channels: usize) -> WinResult<()> {
        unsafe {
            while self.capture.GetNextPacketSize()? > 0 {
                let mut data = ptr::null_mut();
                let mut frames = 0;
                let mut flags = 0;
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                let count = frames as usize * channels;
                if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 {
                    self.samples.clear();
                }
                let limit = MAX_QUEUE_FRAMES * channels;
                let kept = count.min(limit);
                let excess = (self.samples.len() + kept).saturating_sub(limit);
                self.samples.drain(..excess);
                if data.is_null() || flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    self.samples.resize(self.samples.len() + kept, 0);
                } else {
                    let bytes = std::slice::from_raw_parts(data.add((count - kept) * 2), kept * 2);
                    self.samples.extend(
                        bytes
                            .chunks_exact(2)
                            .map(|b| i16::from_le_bytes([b[0], b[1]])),
                    );
                }
                self.capture.ReleaseBuffer(frames)?;
            }
        }
        Ok(())
    }
}

impl Drop for Input {
    fn drop(&mut self) {
        let _ = unsafe { self.client.Stop() };
    }
}

struct AppCapture {
    running: Arc<AtomicBool>,
    waiting: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Capture for AppCapture {
    fn name(&self) -> &str {
        "WASAPI process loopback"
    }
    fn stop(self: Box<Self>) {}
    fn mute_status(&self) -> &'static str {
        "off"
    }
    fn source_waiting(&self) -> bool {
        self.waiting.load(Ordering::Relaxed)
    }
}

impl Drop for AppCapture {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn start(
    fmt: CaptureFormat,
    policy: CapturePolicy,
    path: String,
) -> Result<(Box<dyn Capture>, CaptureReceiver), CaptureError> {
    if fmt.sample_rate == 0 || fmt.channels != 2 {
        return Err(CaptureError::UnsupportedConfig {
            wanted: fmt.sample_rate,
            channels: fmt.channels,
        });
    }
    let (sender, receiver) = crate::queue::capture_channel(policy);
    let running = Arc::new(AtomicBool::new(true));
    let waiting = Arc::new(AtomicBool::new(false));
    let worker_running = running.clone();
    let worker_waiting = waiting.clone();
    let (init, ready) = mpsc::sync_channel(1);
    let thread = thread::Builder::new()
        .name("wasapi-process-loopback".into())
        .spawn(move || {
            if let Err(error) =
                capture_app(fmt, &path, sender, &worker_running, &worker_waiting, &init)
            {
                tracing::warn!("WASAPI process capture ended");
                let _ = init.try_send(Err(error));
            }
        })
        .map_err(|error| CaptureError::Backend(error.to_string()))?;
    match ready.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok((
            Box::new(AppCapture {
                running,
                waiting,
                thread: Some(thread),
            }),
            receiver,
        )),
        Ok(Err(error)) if error == "selected_audio_app_unavailable" => {
            Err(CaptureError::Backend(error))
        }
        Ok(Err(error)) => Err(CaptureError::ProcessLoopback(error)),
        Err(_) => {
            running.store(false, Ordering::Relaxed);
            Err(CaptureError::ProcessLoopback(
                "process_loopback_activation_timeout".into(),
            ))
        }
    }
}

fn capture_app(
    fmt: CaptureFormat,
    path: &str,
    sender: CaptureSender,
    running: &AtomicBool,
    waiting: &AtomicBool,
    init: &mpsc::SyncSender<Result<(), String>>,
) -> Result<(), String> {
    let _com = ComGuard::new().map_err(|error| error.to_string())?;
    let mut inputs = HashMap::new();
    let mut roots = selected_roots(path).map_err(|error| error.to_string())?;
    if roots.is_empty() {
        return Err("selected_audio_app_unavailable".into());
    }
    reconcile_inputs(&mut inputs, &mut roots, fmt, running)?;
    init.send(Ok(())).map_err(|error| error.to_string())?;
    let _mmcss = crate::windows::register_mmcss();
    sender.set_mmcss(_mmcss.is_some());
    let channels = usize::from(fmt.channels);
    let samples_per_chunk = CHUNK_FRAMES * channels;
    let period = Duration::from_secs_f64(CHUNK_FRAMES as f64 / f64::from(fmt.sample_rate));
    let mut next = Instant::now() + period * 2;
    let mut refresh = next + REFRESH;
    let mut mixed = vec![0i64; samples_per_chunk];
    while running.load(Ordering::Relaxed) {
        let now = Instant::now();
        if now >= refresh {
            let result = selected_roots(path)
                .map_err(|error| error.to_string())
                .and_then(|mut roots| {
                    // A paused process can keep its client. A changed set of
                    // active roots is replaced before any new samples are mixed.
                    if !roots.is_empty() {
                        reconcile_inputs(&mut inputs, &mut roots, fmt, running)?;
                    }
                    Ok(())
                });
            refresh = Instant::now()
                + if result.is_ok() {
                    REFRESH
                } else {
                    RETRY_AFTER_FAILURE
                };
        }
        inputs.retain(|_, input| input.process.alive() && input.drain(channels).is_ok());
        waiting.store(inputs.is_empty(), Ordering::Relaxed);
        let now = Instant::now();
        if now < next {
            thread::sleep(next - now);
            continue;
        }
        for _ in 0..crate::silence::due_chunks(&mut next, now, period) {
            mixed.fill(0);
            for input in inputs.values_mut() {
                if input.samples.len() >= samples_per_chunk {
                    for (sum, sample) in mixed
                        .iter_mut()
                        .zip(input.samples.drain(..samples_per_chunk))
                    {
                        *sum += i64::from(sample);
                    }
                }
            }
            let samples = mixed
                .iter()
                .map(|&sample| sample.clamp(i16::MIN as i64, i16::MAX as i64) as i16)
                .collect();
            if matches!(
                sender.try_send(CapturedFrame::new(samples, fmt.channels, fmt.sample_rate)),
                Err(crossbeam_channel::TrySendError::Disconnected(_))
            ) {
                return Ok(());
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_roots_are_deduplicated_across_helpers_and_reused_pids_are_ignored() {
        let roots = HashMap::from([(10, 100), (12, 300), (20, 150), (30, 400)]);
        let parents = HashMap::from([(12, 11), (11, 10), (20, 10)]);
        let creation = HashMap::from([(10, 100), (11, 200), (12, 300), (20, 150), (30, 400)]);
        let mut nested = nested_roots(&roots, &parents, |pid| creation.get(&pid).copied());
        nested.sort_unstable();
        assert_eq!(nested, [12, 20]);

        let nested = nested_roots(&roots, &parents, |pid| {
            if pid == 10 {
                Some(500)
            } else {
                creation.get(&pid).copied()
            }
        });
        assert!(nested.is_empty());
    }
}

fn reconcile_inputs(
    inputs: &mut HashMap<u32, Input>,
    roots: &mut HashMap<u32, Process>,
    fmt: CaptureFormat,
    running: &AtomicBool,
) -> Result<(), String> {
    inputs.retain(|pid, input| {
        roots
            .get(pid)
            .is_some_and(|root| root.created == input.process.created)
    });
    let mut failure = None;
    for (pid, process) in roots.drain() {
        if !running.load(Ordering::Relaxed) {
            return Err("session_cancelled".into());
        }
        if let std::collections::hash_map::Entry::Vacant(entry) = inputs.entry(pid) {
            match Input::new(pid, process, fmt) {
                Ok(input) => {
                    entry.insert(input);
                }
                Err(error) => {
                    failure = Some(error);
                }
            }
        }
    }
    failure.map_or(Ok(()), Err)
}
