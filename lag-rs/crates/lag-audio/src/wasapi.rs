//! Mirrors ProcessLoopbackCapture.cs and NAudio WASAPI use in WasapiAudioSource.cs.
//! Each stream creates, uses, stops and releases all COM objects on its own MTA worker.
use crate::{AudioError, PcmFormat, Result};
use std::{
    ptr::null_mut,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};
use windows::{
    core::{implement, Interface, Ref, HRESULT, PCWSTR},
    Win32::{
        Foundation::{CloseHandle, HANDLE},
        Media::Audio::*,
        System::{Com::StructuredStorage::*, Com::*, Threading::*, Variant::VT_BLOB},
    },
};

pub(crate) struct Apartment(std::marker::PhantomData<std::rc::Rc<()>>);
impl Apartment {
    pub fn mta() -> Result<Self> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        }
        Ok(Self(std::marker::PhantomData))
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}
pub(crate) struct Event(pub HANDLE);
impl Event {
    fn new() -> Result<Self> {
        Ok(Self(unsafe { CreateEventW(None, false, false, None)? }))
    }
}
impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}
pub(crate) fn enumerator() -> Result<IMMDeviceEnumerator> {
    Ok(unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? })
}
pub(crate) fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

#[derive(Debug, Clone, Copy)]
pub enum ProcessLoopbackMode {
    IncludeTree,
    ExcludeTree,
}
#[derive(Clone)]
pub(crate) enum Target {
    Render(Option<String>),
    Mic(Option<String>),
    Process(u32, ProcessLoopbackMode),
}
pub(crate) fn endpoint(target: &Target) -> Result<IMMDevice> {
    endpoint_quiet(target, true)
}

/// The device a target means now: the chosen one if it is there (and active), else Windows' default one.
/// "Default" is Windows' default device for both directions (eConsole), not the "communications" one: with a
/// headset connected that is the headset's own mic (QA 2026-11-27: a whole session recorded the G522's unused mic
/// instead of the QuadCast the owner talks into).
fn endpoint_quiet(target: &Target, say: bool) -> Result<IMMDevice> {
    let en = enumerator()?;
    let (id, flow) = match target {
        Target::Render(id) => (id, eRender),
        Target::Mic(id) => (id, eCapture),
        _ => unreachable!(),
    };
    unsafe {
        if let Some(id) = id.as_ref().filter(|s| !s.is_empty()) {
            let chosen = en.GetDevice(PCWSTR(wide(id).as_ptr())).and_then(|d| {
                // a device that is unplugged / disabled is still found by its id: not usable
                if d.GetState()? == DEVICE_STATE_ACTIVE {
                    Ok(d)
                } else {
                    Err(windows::core::Error::from_hresult(AUDCLNT_E_DEVICE_INVALIDATED))
                }
            });
            match chosen {
                Ok(d) => return Ok(d),
                Err(e) => {
                    if say {
                        let kind = if matches!(target, Target::Render(_)) { "output" } else { "microphone" };
                        log::warn!("[WasapiAudioSource] chosen {kind} '{id}' unavailable ({e}) — using Windows' default until it is back.");
                    }
                }
            }
        }
        Ok(en.GetDefaultAudioEndpoint(flow, eConsole)?)
    }
}

/// The id of a device.
unsafe fn device_id(d: &IMMDevice) -> String {
    match d.GetId() {
        Ok(id) => {
            let s = id.to_string().unwrap_or_default();
            CoTaskMemFree(Some(id.0.cast()));
            s
        }
        Err(_) => String::new(),
    }
}
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Activation(crossbeam_channel::Sender<()>);
impl IActivateAudioInterfaceCompletionHandler_Impl for Activation_Impl {
    fn ActivateCompleted(&self, _: Ref<'_, IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
        let _ = self.0.try_send(());
        Ok(())
    }
}
fn activate_process(pid: u32, mode: ProcessLoopbackMode) -> Result<IAudioClient> {
    unsafe {
        let mut params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: match mode {
                        ProcessLoopbackMode::IncludeTree => PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                        ProcessLoopbackMode::ExcludeTree => PROCESS_LOOPBACK_MODE_EXCLUDE_TARGET_PROCESS_TREE,
                    },
                },
            },
        };
        // Borrowed blob: never PropVariantClear this value (it points into the stack).
        let mut prop = std::mem::ManuallyDrop::new(PROPVARIANT::default());
        (*prop.Anonymous.Anonymous).vt = VT_BLOB;
        (*prop.Anonymous.Anonymous).Anonymous.blob = BLOB {
            cbSize: std::mem::size_of_val(&params) as u32,
            pBlobData: (&mut params as *mut AUDIOCLIENT_ACTIVATION_PARAMS).cast(),
        };
        let (tx, rx) = crossbeam_channel::bounded(1);
        let handler: IActivateAudioInterfaceCompletionHandler = Activation(tx).into();
        let op = ActivateAudioInterfaceAsync(
            windows::core::w!("VAD\\Process_Loopback"),
            &IAudioClient::IID,
            Some(&*prop),
            &handler,
        )?;
        rx.recv_timeout(Duration::from_secs(5))
            .map_err(|_| AudioError("ActivateAudioInterfaceAsync did not complete (process loopback).".into()))?;
        let mut hr = HRESULT(0);
        let mut iface = None;
        op.GetActivateResult(&mut hr, &mut iface)?;
        hr.ok()?;
        Ok(iface.ok_or_else(|| AudioError("Process-loopback activation failed".into()))?.cast()?)
    }
}
pub(crate) unsafe fn pcm_format(p: *const WAVEFORMATEX) -> PcmFormat {
    let f = p.read_unaligned();
    let mut float = f.wFormatTag == 3;
    if f.wFormatTag == 0xfffe && f.cbSize >= 22 {
        // WAVEFORMATEXTENSIBLE SubFormat starts at byte 24; no unaligned Rust references.
        float = p.cast::<u8>().add(24).cast::<windows::core::GUID>().read_unaligned().data1 == 3;
    }
    PcmFormat {
        sample_rate: f.nSamplesPerSec,
        channels: f.nChannels,
        bits_per_sample: f.wBitsPerSample,
        is_float: float,
    }
}
pub(crate) struct CaptureStream {
    pub format: PcmFormat,
    pub name: String,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}
/// Why one opened stream ended.
enum Ended {
    /// Lag stops the capture
    Stopped,
    /// another device is meant now (Windows' default changed, the chosen one is back)
    Moved,
}

/// One opened WASAPI stream.
struct Opened {
    client: IAudioClient,
    capture: IAudioCaptureClient,
    event: Event,
    format: PcmFormat,
    name: String,
    /// the endpoint it is on ("" = a process loopback)
    device: String,
}

/// A WAVEFORMATEX for a PcmFormat (what a reopened stream asks Windows to convert to).
fn wave_format(f: PcmFormat) -> WAVEFORMATEX {
    let block = f.channels * f.bits_per_sample / 8;
    WAVEFORMATEX {
        wFormatTag: if f.is_float { 3 } else { 1 },
        nChannels: f.channels,
        nSamplesPerSec: f.sample_rate,
        nAvgBytesPerSec: f.sample_rate * u32::from(block),
        nBlockAlign: block,
        wBitsPerSample: f.bits_per_sample,
        cbSize: 0,
    }
}

/// Opens the stream a target means now. `keep`: the format of the first stream; a reopened one (another device,
/// the same one back) is converted by Windows to it, so everything after the capture sees one format.
fn open(target: &Target, keep: Option<PcmFormat>) -> Result<Opened> {
    let (client, name, device): (IAudioClient, String, String) = match target {
        Target::Process(pid, mode) => (activate_process(*pid, *mode)?, String::new(), String::new()),
        _ => unsafe {
            let device = endpoint_quiet(target, keep.is_none())?;
            (device.Activate(CLSCTX_ALL, None)?, crate::devices::friendly_name(&device), device_id(&device))
        },
    };
    let forced = WAVEFORMATEX {
        wFormatTag: 3,
        nChannels: 2,
        nSamplesPerSec: 48000,
        nAvgBytesPerSec: 384000,
        nBlockAlign: 8,
        wBitsPerSample: 32,
        cbSize: 0,
    };
    let event = Event::new()?;
    let loopback = !matches!(target, Target::Mic(_));
    let flags = AUDCLNT_STREAMFLAGS_EVENTCALLBACK | if loopback { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };
    let format;
    unsafe {
        if matches!(target, Target::Process(_, _)) {
            format = pcm_format(&forced);
            client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, &forced, None)?;
        } else if let Some(keep) = keep {
            let want = wave_format(keep);
            let mix = client.GetMixFormat()?;
            let same = pcm_format(mix) == keep;
            let init = if same {
                client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, mix, None)
            } else {
                client.Initialize(
                    AUDCLNT_SHAREMODE_SHARED,
                    flags | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                    2_000_000,
                    0,
                    &want,
                    None,
                )
            };
            CoTaskMemFree(Some(mix.cast()));
            init?;
            format = keep;
        } else {
            let p = client.GetMixFormat()?;
            format = pcm_format(p);
            let init = client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, p, None);
            CoTaskMemFree(Some(p.cast()));
            init?;
        }
        client.SetEventHandle(event.0)?;
    }
    let capture: IAudioCaptureClient = unsafe { client.GetService()? };
    unsafe {
        client.Start()?;
    }
    Ok(Opened { client, capture, event, format, name, device })
}

/// Reads one opened stream until Lag stops it, it breaks (Err), or another device is meant now.
fn pump(
    o: &Opened,
    target: &Target,
    flag: &AtomicBool,
    buffer: &mut Vec<u8>,
    callback: &mut impl FnMut(&mut [u8], PcmFormat),
) -> Result<Ended> {
    let format = o.format;
    let mut checked = std::time::Instant::now();
    let mut heard = std::time::Instant::now();
    while !flag.load(Ordering::Acquire) {
        unsafe {
            WaitForSingleObject(o.event.0, 100);
        }
        if flag.load(Ordering::Acquire) {
            break;
        }
        while unsafe { o.capture.GetNextPacketSize()? } > 0 {
            let mut p = null_mut();
            let mut frames = 0;
            let mut flags = 0;
            unsafe {
                o.capture.GetBuffer(&mut p, &mut frames, &mut flags, None, None)?;
            }
            let bytes = frames as usize * format.block_align();
            if bytes > buffer.len() {
                buffer.resize(bytes, 0);
            }
            if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                buffer[..bytes].fill(0);
            } else if bytes > 0 {
                unsafe {
                    std::ptr::copy_nonoverlapping(p, buffer.as_mut_ptr(), bytes);
                }
            }
            unsafe {
                o.capture.ReleaseBuffer(frames)?;
            }
            if frames == 0 {
                break;
            }
            heard = std::time::Instant::now();
            // Release the endpoint buffer BEFORE calling downstream processing.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| callback(&mut buffer[..bytes], format)));
        }
        // a working mic sends packets all the time (silence too): none for a while = it is stuck
        if matches!(target, Target::Mic(_)) && heard.elapsed() > Duration::from_secs(3) {
            return Err(AudioError("the microphone sent nothing for 3 s".into()));
        }
        // the device meant now: Windows' default may have changed, a chosen device may be back
        if !matches!(target, Target::Process(_, _)) && checked.elapsed() > Duration::from_secs(2) {
            checked = std::time::Instant::now();
            if let Ok(d) = endpoint_quiet(target, false) {
                if unsafe { device_id(&d) } != o.device {
                    return Ok(Ended::Moved);
                }
            }
        }
    }
    Ok(Ended::Stopped)
}

impl CaptureStream {
    /// Starts capturing `target`. Err only when it cannot start at all; after that the stream heals itself: a
    /// device that is lost (a wireless headset reconnecting, a USB mic resetting, Windows' default changing) is
    /// reopened, and the recording goes on with the same format (QA 2026-11-27: a lost stream ended silently and
    /// the track stayed silent until Lag restarted).
    pub fn start(target: Target, mut callback: impl FnMut(&mut [u8], PcmFormat) + Send + 'static) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let kind = match target {
            Target::Process(pid, _) => format!("app capture (pid {pid})"),
            Target::Mic(_) => "microphone".to_owned(),
            _ => "system audio".to_owned(),
        };
        let worker = thread::Builder::new()
            .name(match target {
                Target::Process(pid, _) => format!("ProcLoopback{pid}"),
                Target::Mic(_) => "WasapiMic".into(),
                _ => "WasapiLoopback".into(),
            })
            .spawn(move || {
                let _com = match Apartment::mta() {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = tx.try_send(Err(e));
                        return;
                    }
                };
                let mut first = Some(tx);
                let mut keep: Option<PcmFormat> = None;
                let mut buffer = Vec::new();
                let mut said_at: Option<std::time::Instant> = None;
                while !flag.load(Ordering::Acquire) {
                    match open(&target, keep) {
                        Ok(o) => {
                            if let Some(tx) = first.take() {
                                let _ = tx.send(Ok((o.format, o.name.clone())));
                            } else {
                                log::info!("[WasapiAudioSource] {kind} back{}.", if o.name.is_empty() { String::new() } else { format!(" on '{}'", o.name) });
                            }
                            keep = Some(o.format);
                            said_at = None;
                            let ended = pump(&o, &target, &flag, &mut buffer, &mut callback);
                            unsafe {
                                let _ = o.client.Stop();
                            }
                            match ended {
                                Ok(Ended::Stopped) => break,
                                Ok(Ended::Moved) => log::info!("[WasapiAudioSource] {kind}: another device is meant now — switching."),
                                Err(e) => log::warn!("[WasapiAudioSource] {kind} lost ({e}) — reopening."),
                            }
                        }
                        Err(e) => {
                            if let Some(tx) = first.take() {
                                let _ = tx.send(Err(e));
                                return;
                            }
                            // not back yet: say so now and then, try again every second
                            if said_at.is_none_or(|t| t.elapsed() > Duration::from_secs(30)) {
                                log::warn!("[WasapiAudioSource] {kind} still unavailable ({e}) — trying again.");
                                said_at = Some(std::time::Instant::now());
                            }
                            for _ in 0..10 {
                                if flag.load(Ordering::Acquire) {
                                    break;
                                }
                                thread::sleep(Duration::from_millis(100));
                            }
                        }
                    }
                }
            })?;
        match rx.recv_timeout(Duration::from_secs(6)) {
            Ok(Ok((format, name))) => Ok(Self { format, name, stop, worker: Some(worker) }),
            result => {
                stop.store(true, Ordering::Release);
                let _ = worker.join();
                Err(match result {
                    Ok(Err(e)) => e,
                    _ => AudioError("WASAPI capture setup timed out".into()),
                })
            }
        }
    }
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.worker.take() {
            let _ = t.join();
        }
    }
}
impl Drop for CaptureStream {
    fn drop(&mut self) {
        self.stop();
    }
}
pub struct ProcessLoopbackCapture {
    stream: CaptureStream,
}
impl ProcessLoopbackCapture {
    pub fn start(
        pid: u32,
        mode: ProcessLoopbackMode,
        mut callback: impl FnMut(&[u8]) + Send + 'static,
    ) -> Result<Self> {
        Ok(Self { stream: CaptureStream::start(Target::Process(pid, mode), move |b, _| callback(b))? })
    }
    pub fn format(&self) -> PcmFormat {
        self.stream.format
    }
    pub fn stop(&mut self) {
        self.stream.stop();
    }
    pub fn self_test(pid: i32) -> Result<()> {
        log::info!("[ProcLoopbackTest] capturing pid {pid} for 5s (play sound in that app)...");
        let totals = Arc::new(std::sync::Mutex::new((0u64, 0u64, 0f32)));
        let t = totals.clone();
        let mut cap = match Self::start(pid as u32, ProcessLoopbackMode::IncludeTree, move |b| {
            let mut t = t.lock().unwrap();
            t.0 += 1;
            t.1 += b.len() as u64;
            for sample in b.as_chunks::<4>().0 {
                t.2 = t.2.max(f32::from_le_bytes(*sample).abs());
            }
        }) {
            Ok(c) => c,
            Err(e) => {
                log::error!("[ProcLoopbackTest] Start FAILED: {e}");
                return Err(e);
            }
        };
        thread::sleep(Duration::from_secs(5));
        cap.stop();
        let (callbacks, bytes, peak) = *totals.lock().unwrap();
        log::info!(
            "[ProcLoopbackTest] done: {callbacks} callbacks, {bytes} bytes (~{:.2}s), peak amplitude {peak:.4} ({}).",
            bytes as f64 / 8. / 48000.,
            if peak > 0.0001 { "REAL AUDIO ✓" } else { "silent/none" }
        );
        Ok(())
    }
}

#[cfg(test)]
mod heal_tests {
    use super::*;

    /// On this PC's real devices: a reopened stream gets the format of the first one even when the device's own
    /// format differs (Windows converts). Run by hand: cargo test -p lag-audio reopened -- --ignored --nocapture
    #[test]
    #[ignore]
    fn a_reopened_stream_keeps_the_first_format() {
        let _com = Apartment::mta().unwrap();
        let odd = PcmFormat { sample_rate: 44100, channels: 1, bits_per_sample: 32, is_float: true };
        for target in [Target::Render(None), Target::Mic(None)] {
            let o = open(&target, Some(odd)).expect("open with a kept format");
            eprintln!("{} -> {:?} on '{}'", if matches!(target, Target::Mic(_)) { "mic" } else { "system" }, o.format, o.name);
            assert_eq!(o.format, odd);
            // a few reads: packets come in the kept format
            let mut got = 0usize;
            let start = std::time::Instant::now();
            while start.elapsed() < Duration::from_millis(600) {
                unsafe { WaitForSingleObject(o.event.0, 100) };
                while unsafe { o.capture.GetNextPacketSize().unwrap() } > 0 {
                    let (mut p, mut frames, mut flags) = (null_mut(), 0, 0);
                    unsafe { o.capture.GetBuffer(&mut p, &mut frames, &mut flags, None, None).unwrap() };
                    got += frames as usize;
                    unsafe { o.capture.ReleaseBuffer(frames).unwrap() };
                    if frames == 0 { break; }
                }
            }
            eprintln!("  {got} frames in 0.6 s");
            unsafe { let _ = o.client.Stop(); }
        }
    }
}
