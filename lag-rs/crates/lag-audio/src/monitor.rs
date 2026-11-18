//! Mirrors Services/AudioMonitor.cs: raw peak meters, gate preview, and optional audible mic preview.
use crate::{
    aac::Resampler,
    pcm,
    wasapi::{Apartment, CaptureStream, Target},
    AudioControls, MicDenoiser, NoiseGate, Result,
};
use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
};
use windows::Win32::{Media::Audio::*, System::Com::CLSCTX_ALL};
type PreviewQueue = Arc<Mutex<VecDeque<[f32; 2]>>>;
struct Dsp {
    gate: NoiseGate,
    denoiser: MicDenoiser,
    last_gain: f32,
    resampler: Option<Resampler>,
}
struct MeterState {
    mic: AtomicU32,
    output: AtomicU32,
    threshold: AtomicU32,
    open: AtomicBool,
    preview: Mutex<Option<PreviewQueue>>,
    controls: AudioControls,
    dsp: Mutex<Dsp>,
}
pub struct AudioMonitor {
    state: Arc<MeterState>,
    mic: Option<CaptureStream>,
    output: Option<CaptureStream>,
    render: Option<Renderer>,
}
impl Default for AudioMonitor {
    fn default() -> Self {
        Self {
            state: Arc::new(MeterState {
                mic: AtomicU32::new(0),
                output: AtomicU32::new(0),
                threshold: AtomicU32::new(0.08f32.to_bits()),
                open: AtomicBool::new(false),
                preview: Mutex::new(None),
                controls: AudioControls::default(),
                dsp: Mutex::new(Dsp {
                    gate: NoiseGate::default(),
                    denoiser: MicDenoiser::default(),
                    last_gain: 1.,
                    resampler: None,
                }),
            }),
            mic: None,
            output: None,
            render: None,
        }
    }
}
impl AudioMonitor {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn read_mic_peak(&self) -> f32 {
        f32::from_bits(self.state.mic.swap(0, Ordering::Relaxed))
    }
    pub fn read_output_peak(&self) -> f32 {
        f32::from_bits(self.state.output.swap(0, Ordering::Relaxed))
    }
    pub fn gate_threshold(&self) -> f32 {
        f32::from_bits(self.state.threshold.load(Ordering::Relaxed))
    }
    pub fn gate_open(&self) -> bool {
        self.state.open.load(Ordering::Relaxed)
    }
    pub fn set_input_gate(&self, auto: bool, threshold: f32) {
        self.state.controls.set_input_gate(auto, threshold);
    }
    pub fn set_noise_suppression(&self, on: bool) {
        self.state.controls.set_noise_suppression(on);
    }
    /// Best effort, matching Settings: invalid IDs fall back to the corresponding default.
    pub fn start(&mut self, microphone_id: Option<String>, output_id: Option<String>) {
        self.stop();
        let start_mic = |id| {
            let state = self.state.clone();
            CaptureStream::start(Target::Mic(id), move |data, format| {
                let raw = pcm::peak(data, format);
                state.mic.fetch_max(raw.to_bits(), Ordering::Relaxed);
                let mut dsp = state.dsp.lock().unwrap();
                let dsp = &mut *dsp;
                state.controls.configure(&mut dsp.gate, &mut dsp.denoiser);
                let gain = dsp.gate.process(pcm::curve(raw), format.duration(data.len()));
                state.threshold.store(dsp.gate.threshold.to_bits(), Ordering::Relaxed);
                state.open.store(dsp.gate.is_open, Ordering::Relaxed);
                let q = state.preview.lock().unwrap().clone();
                if let Some(q) = q {
                    if format.is_float && (gain < 0.999 || dsp.last_gain < 0.999) {
                        pcm::ramp(data, dsp.last_gain, gain);
                    }
                    let bytes = if format.is_float {
                        dsp.denoiser.process(data, format.sample_rate, format.channels).unwrap_or(data)
                    } else {
                        data
                    };
                    if dsp.resampler.as_ref().is_none_or(|r| r.format != format) {
                        dsp.resampler = Resampler::new(format).ok();
                    }
                    if let Some(r) = &mut dsp.resampler {
                        if let Ok(n) = r.convert(bytes) {
                            let mut q = q.lock().unwrap();
                            let remaining = 24000usize.saturating_sub(q.len());
                            // NAudio MonoToStereoSampleProvider duplicates at unity; swresample's
                            // default center-to-stereo matrix attenuates each side by sqrt(1/2).
                            let mono_gain = if format.channels == 1 { std::f32::consts::SQRT_2 } else { 1. };
                            q.extend(r.samples(n).take(remaining).map(|s| [s[0] * mono_gain, s[1] * mono_gain]));
                        }
                    }
                }
                dsp.last_gain = gain;
            })
        };
        match start_mic(microphone_id).or_else(|_| start_mic(None)) {
            Ok(c) => self.mic = Some(c),
            Err(e) => log::warn!("[AudioMonitor] mic meter unavailable: {e}"),
        }
        let state = self.state.clone();
        match CaptureStream::start(Target::Render(output_id), move |b, f| {
            state.output.fetch_max(pcm::peak(b, f).to_bits(), Ordering::Relaxed);
        }) {
            Ok(c) => self.output = Some(c),
            Err(e) => log::warn!("[AudioMonitor] output meter unavailable: {e}"),
        }
    }
    pub fn set_monitoring(&mut self, on: bool, output_id: Option<String>) {
        self.stop_monitoring();
        if !on || self.mic.is_none() {
            return;
        }
        let q = Arc::new(Mutex::new(VecDeque::with_capacity(24000)));
        match Renderer::start(output_id, q.clone()) {
            Ok(r) => {
                *self.state.preview.lock().unwrap() = Some(q);
                self.render = Some(r);
            }
            Err(e) => log::warn!("[AudioMonitor] monitor meter unavailable: {e}"),
        }
    }
    fn stop_monitoring(&mut self) {
        self.render.take();
        *self.state.preview.lock().unwrap() = None;
    }
    pub fn stop(&mut self) {
        self.stop_monitoring();
        self.mic.take();
        self.output.take();
        self.state.dsp.lock().unwrap().denoiser.teardown();
        self.state.mic.store(0, Ordering::Relaxed);
        self.state.output.store(0, Ordering::Relaxed);
    }
}
impl Drop for AudioMonitor {
    fn drop(&mut self) {
        self.stop();
    }
}
struct Renderer {
    stop: crossbeam_channel::Sender<()>,
    worker: Option<JoinHandle<()>>,
}
impl Renderer {
    fn start(id: Option<String>, queue: PreviewQueue) -> Result<Self> {
        let (stop, rx) = crossbeam_channel::bounded(1);
        let (ready, wait) = crossbeam_channel::bounded(1);
        let worker = std::thread::Builder::new().name("MicPreviewRender".into()).spawn(move || {
            let run = || -> Result<()> {
                let _com = Apartment::mta()?;
                let client: IAudioClient =
                    unsafe { crate::wasapi::endpoint(&Target::Render(id))?.Activate(CLSCTX_ALL, None)? };
                let format = WAVEFORMATEX {
                    wFormatTag: 3,
                    nChannels: 2,
                    nSamplesPerSec: 48000,
                    nAvgBytesPerSec: 384000,
                    nBlockAlign: 8,
                    wBitsPerSample: 32,
                    cbSize: 0,
                };
                unsafe {
                    client.Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                        1_000_000,
                        0,
                        &format,
                        None,
                    )?;
                }
                let render: IAudioRenderClient = unsafe { client.GetService()? };
                let capacity = unsafe { client.GetBufferSize()? };
                unsafe {
                    client.Start()?;
                }
                let _ = ready.send(Ok(()));
                let result = (|| -> Result<()> {
                    loop {
                        if rx.recv_timeout(std::time::Duration::from_millis(10)).is_ok() {
                            break;
                        }
                        let frames = capacity - unsafe { client.GetCurrentPadding()? };
                        if frames == 0 {
                            continue;
                        }
                        let p = unsafe { render.GetBuffer(frames)? };
                        let samples = unsafe { std::slice::from_raw_parts_mut(p.cast::<f32>(), frames as usize * 2) };
                        let mut q = queue.lock().unwrap();
                        for frame in samples.as_chunks_mut::<2>().0.iter_mut() {
                            frame.copy_from_slice(&q.pop_front().unwrap_or([0., 0.]));
                        }
                        drop(q);
                        unsafe {
                            render.ReleaseBuffer(frames, 0)?;
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
                let _ = ready.try_send(Err(e));
            }
        })?;
        match wait.recv() {
            Ok(Ok(())) => Ok(Self { stop, worker: Some(worker) }),
            r => {
                let _ = worker.join();
                Err(match r {
                    Ok(Err(e)) => e,
                    _ => crate::AudioError("Monitor render thread exited".into()),
                })
            }
        }
    }
}
impl Drop for Renderer {
    fn drop(&mut self) {
        let _ = self.stop.try_send(());
        if let Some(t) = self.worker.take() {
            let _ = t.join();
        }
    }
}
