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
    let en = enumerator()?;
    let (id, flow, role) = match target {
        Target::Render(id) => (id, eRender, eMultimedia),
        Target::Mic(id) => (id, eCapture, eCommunications),
        _ => unreachable!(),
    };
    unsafe {
        if let Some(id) = id.as_ref().filter(|s| !s.is_empty()) {
            match en.GetDevice(PCWSTR(wide(id).as_ptr())) {
                Ok(d) => return Ok(d),
                Err(e) if matches!(target, Target::Render(_)) => {
                    log::warn!("[WasapiAudioSource] output device '{id}' unavailable ({e}) — using default.")
                }
                Err(e) => return Err(e.into()),
            }
        }
        Ok(en.GetDefaultAudioEndpoint(flow, role)?)
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
impl CaptureStream {
    pub fn start(target: Target, mut callback: impl FnMut(&mut [u8], PcmFormat) + Send + 'static) -> Result<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let (tx, rx) = crossbeam_channel::bounded(1);
        let worker = thread::Builder::new()
            .name(match target {
                Target::Process(pid, _) => format!("ProcLoopback{pid}"),
                Target::Mic(_) => "WasapiMic".into(),
                _ => "WasapiLoopback".into(),
            })
            .spawn(move || {
                let mut run = || -> Result<()> {
                    let _com = Apartment::mta()?;
                    let (client, name): (IAudioClient, String) = match &target {
                        Target::Process(pid, mode) => (activate_process(*pid, *mode)?, String::new()),
                        _ => unsafe {
                            let device = endpoint(&target)?;
                            (device.Activate(CLSCTX_ALL, None)?, crate::devices::friendly_name(&device))
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
                    let format;
                    unsafe {
                        let p = if matches!(target, Target::Process(_, _)) {
                            &forced as *const _ as *mut WAVEFORMATEX
                        } else {
                            client.GetMixFormat()?
                        };
                        format = pcm_format(p);
                        let loopback = !matches!(target, Target::Mic(_));
                        let init = client.Initialize(
                            AUDCLNT_SHAREMODE_SHARED,
                            AUDCLNT_STREAMFLAGS_EVENTCALLBACK | if loopback { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 },
                            2_000_000,
                            0,
                            p,
                            None,
                        );
                        if !matches!(target, Target::Process(_, _)) {
                            CoTaskMemFree(Some(p.cast()));
                        }
                        init?;
                        client.SetEventHandle(event.0)?;
                    }
                    let capture: IAudioCaptureClient = unsafe { client.GetService()? };
                    let mut buffer = vec![0u8; unsafe { client.GetBufferSize()? } as usize * format.block_align()];
                    unsafe {
                        client.Start()?;
                    }
                    let _ = tx.send(Ok((format, name)));
                    let result = (|| -> Result<()> {
                        while !flag.load(Ordering::Acquire) {
                            unsafe {
                                WaitForSingleObject(event.0, 100);
                            }
                            if flag.load(Ordering::Acquire) {
                                break;
                            }
                            while unsafe { capture.GetNextPacketSize()? } > 0 {
                                let mut p = null_mut();
                                let mut frames = 0;
                                let mut flags = 0;
                                unsafe {
                                    capture.GetBuffer(&mut p, &mut frames, &mut flags, None, None)?;
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
                                    capture.ReleaseBuffer(frames)?;
                                }
                                if frames == 0 {
                                    break;
                                }
                                // Release the endpoint buffer BEFORE calling downstream processing.
                                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    callback(&mut buffer[..bytes], format)
                                }));
                            }
                        }
                        Ok(())
                    })();
                    unsafe {
                        let _ = client.Stop();
                    }
                    result
                };
                if let Err(e) = run() {
                    let _ = tx.try_send(Err(e));
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
