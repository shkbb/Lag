//! Mirrors Services/VfrCapture/WasapiAudioSource.cs: endpoint/app mixer and mic DSP ordering.
use crate::{
    pcm,
    wasapi::{CaptureStream, Target},
    MicDenoiser, NoiseGate, PcmFormat, ProcessLoopbackCapture, ProcessLoopbackMode, Result,
};
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
};
#[derive(Clone, Debug)]
pub struct AudioSourceOptions {
    /// None = default communications mic; Some("") = disabled.
    pub microphone_id: Option<String>,
    pub apps_mode: bool,
    pub apps: Vec<(String, f32)>,
    pub system_enabled: bool,
    pub system_volume: f32,
    pub mic_mono: bool,
    pub game_pid: u32,
    pub game_volume: f32,
    pub game_enabled: bool,
    pub system_device_id: Option<String>,
}
impl Default for AudioSourceOptions {
    fn default() -> Self {
        Self {
            microphone_id: None,
            apps_mode: false,
            apps: Vec::new(),
            system_enabled: true,
            system_volume: 1.,
            mic_mono: false,
            game_pid: 0,
            game_volume: 1.,
            game_enabled: true,
            system_device_id: None,
        }
    }
}
pub struct AudioControls {
    mic_gain: AtomicU32,
    muted: AtomicBool,
    auto: AtomicBool,
    threshold: AtomicU32,
    suppression: AtomicBool,
}
impl Default for AudioControls {
    fn default() -> Self {
        Self {
            mic_gain: AtomicU32::new(1f32.to_bits()),
            muted: AtomicBool::new(false),
            auto: AtomicBool::new(true),
            threshold: AtomicU32::new(0.08f32.to_bits()),
            suppression: AtomicBool::new(false),
        }
    }
}
impl AudioControls {
    pub fn set_mic_volume(&self, gain: f32) {
        self.mic_gain.store(gain.max(0.).to_bits(), Ordering::Relaxed);
    }
    pub fn set_mic_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Relaxed);
    }
    pub fn set_input_gate(&self, auto: bool, threshold: f32) {
        self.auto.store(auto, Ordering::Relaxed);
        self.threshold.store(threshold.to_bits(), Ordering::Relaxed);
    }
    pub fn set_noise_suppression(&self, on: bool) {
        self.suppression.store(on, Ordering::Relaxed);
    }
    pub(crate) fn configure(&self, gate: &mut NoiseGate, denoiser: &mut MicDenoiser) {
        gate.auto = self.auto.load(Ordering::Relaxed);
        gate.manual_threshold = f32::from_bits(self.threshold.load(Ordering::Relaxed));
        denoiser.enabled = self.suppression.load(Ordering::Relaxed);
    }
}
pub(crate) type PcmCallback = Arc<dyn Fn(&[u8], PcmFormat) + Send + Sync>;
pub struct WasapiAudioSource {
    controls: Arc<AudioControls>,
    game_format: PcmFormat,
    loopback: Option<CaptureStream>,
    mic: Option<CaptureStream>,
    apps: Vec<ProcessLoopbackCapture>,
    mix_stop: Option<crossbeam_channel::Sender<()>>,
    mixer: Option<JoinHandle<()>>,
}
impl WasapiAudioSource {
    /// PCM callbacks execute on audio workers; retain bytes only by copying. Do not acquire video locks.
    pub fn start(
        options: AudioSourceOptions,
        game: impl Fn(&[u8], PcmFormat) + Send + Sync + 'static,
        mic: impl Fn(&[u8], PcmFormat) + Send + Sync + 'static,
    ) -> Result<Self> {
        Self::start_with_controls(options, Arc::new(AudioControls::default()), Arc::new(game), Arc::new(mic))
    }
    pub fn start_with_controls(
        options: AudioSourceOptions,
        controls: Arc<AudioControls>,
        game: PcmCallback,
        mic_callback: PcmCallback,
    ) -> Result<Self> {
        let mut s = Self {
            controls,
            game_format: PcmFormat::SYSTEM,
            loopback: None,
            mic: None,
            apps: Vec::new(),
            mix_stop: None,
            mixer: None,
        };
        let effective_apps =
            options.apps_mode && (!options.apps.is_empty() || (options.game_enabled && options.game_pid != 0));
        if effective_apps {
            let mut pids = HashSet::new();
            let mut queues = Vec::new();
            if options.game_enabled
                && options.game_pid != 0
                && s.add_app(options.game_pid, options.game_volume, "game audio", &mut queues)
            {
                pids.insert(options.game_pid);
            }
            for (exe, volume) in &options.apps {
                let Some(pid) = crate::sessions::find_process(exe) else {
                    log::info!("[WasapiAudioSource] app '{exe}' not running — skipped.");
                    continue;
                };
                if pids.contains(&pid) {
                    log::info!(
                        "[WasapiAudioSource] app '{exe}' is the captured game (pid={pid}) — skipping duplicate."
                    );
                    continue;
                }
                if s.add_app(pid, *volume, &format!("app {exe}"), &mut queues) {
                    pids.insert(pid);
                }
            }
            if s.apps.is_empty() {
                log::info!("[WasapiAudioSource] no app captures available — falling back to full-system loopback.");
                s.start_system(&options, game.clone())?;
            } else {
                let (tx, rx) = crossbeam_channel::bounded(1);
                s.mix_stop = Some(tx);
                s.mixer = Some(std::thread::Builder::new().name("AudioAppMixer".into()).spawn(move || {
                    let mut mixer = AppMixer::new(queues.len());
                    let mut pace = MixPacer::new(std::time::Instant::now());
                    let mut mixed = Vec::new();
                    let mut bytes = Vec::new();
                    let timer = crossbeam_channel::tick(Duration::from_millis(10));
                    loop { crossbeam_channel::select! { recv(rx) -> _ => break, recv(timer) -> _ => {
                        let frames = pace.due(std::time::Instant::now());
                        if frames == 0 { continue; }
                        mixed.resize(frames * 2, 0f32);
                        mixer.mix(&queues, &mut mixed);
                        bytes.resize(mixed.len() * 4, 0u8);
                        for (b, v) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(&mixed) { b.copy_from_slice(&v.clamp(-1., 1.).to_le_bytes()); }
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| game(&bytes, PcmFormat::SYSTEM)));
                    } } }
                })?);
            }
        } else if options.system_enabled {
            s.start_system(&options, game)?;
        } else {
            log::info!("[WasapiAudioSource] system audio disabled (mic-only / silent system).");
        }
        if options.microphone_id.as_deref() == Some("") {
            log::info!("[WasapiAudioSource] mic disabled.");
            return Ok(s);
        }
        let controls = s.controls.clone();
        let mono = options.mic_mono;
        let mut gate = NoiseGate::default();
        let mut last_gain = 1.;
        let mut denoiser = MicDenoiser::default();
        let mut processed = Vec::new();
        match CaptureStream::start(Target::Mic(options.microphone_id), move |b, mut format| {
            if b.is_empty() || controls.muted.load(Ordering::Relaxed) {
                return;
            }
            if !format.is_float {
                mic_callback(b, format);
                return;
            }
            let raw = pcm::peak(b, format);
            let gain = f32::from_bits(controls.mic_gain.load(Ordering::Relaxed));
            if gain != 1. {
                pcm::gain(b, gain);
            }
            controls.configure(&mut gate, &mut denoiser);
            let g = gate.process(pcm::curve(raw), format.duration(b.len()));
            if g < 0.999 || last_gain < 0.999 {
                pcm::ramp(b, last_gain, g);
            }
            last_gain = g;
            let data = denoiser.process(b, format.sample_rate, format.channels).unwrap_or(b);
            if data.is_empty() {
                return;
            }
            if mono && format.channels == 2 {
                processed.clear();
                for frame in data.as_chunks::<8>().0.iter() {
                    let l = f32::from_le_bytes(frame[..4].try_into().unwrap());
                    let r = f32::from_le_bytes(frame[4..].try_into().unwrap());
                    processed.extend_from_slice(&((l + r) * 0.5).to_le_bytes());
                }
                format.channels = 1;
                mic_callback(&processed, format);
            } else {
                mic_callback(data, format);
            }
        }) {
            Ok(c) => {
                let f = c.format;
                log::info!(
                    "[WasapiAudioSource] Mic '{}': {}Hz {}ch {}bit {}.",
                    c.name,
                    f.sample_rate,
                    f.channels,
                    f.bits_per_sample,
                    encoding(f)
                );
                s.mic = Some(c);
            }
            Err(e) => log::warn!("[WasapiAudioSource] mic unavailable — game audio only: {e}"),
        }
        Ok(s)
    }
    fn start_system(&mut self, o: &AudioSourceOptions, game: PcmCallback) -> Result<()> {
        let gain = o.system_volume.max(0.);
        let c = CaptureStream::start(Target::Render(o.system_device_id.clone()), move |b, f| {
            if f.is_float && gain != 1. {
                pcm::gain(b, gain);
            }
            game(b, f);
        })?;
        self.game_format = c.format;
        let f = c.format;
        log::info!(
            "[WasapiAudioSource] Loopback (full system) on '{}': {}Hz {}ch {}bit {}.",
            c.name,
            f.sample_rate,
            f.channels,
            f.bits_per_sample,
            encoding(f)
        );
        self.loopback = Some(c);
        Ok(())
    }
    fn add_app(&mut self, pid: u32, gain: f32, label: &str, queues: &mut Vec<Arc<Mutex<VecDeque<f32>>>>) -> bool {
        let queue = Arc::new(Mutex::new(VecDeque::with_capacity(96000 + 19200)));
        let q = queue.clone();
        match ProcessLoopbackCapture::start(pid, ProcessLoopbackMode::IncludeTree, move |b| {
            let mut q = q.lock().unwrap();
            if q.len() > 96000 {
                return;
            }
            for b in b.as_chunks::<4>().0.iter() {
                q.push_back(f32::from_le_bytes(*b) * gain);
            }
        }) {
            Ok(c) => {
                self.apps.push(c);
                queues.push(queue);
                log::info!("[WasapiAudioSource] {label} capture: pid={pid} gain={gain:.2}.");
                true
            }
            Err(e) => {
                log::warn!("[WasapiAudioSource] {label} loopback failed: {e}");
                false
            }
        }
    }
    pub fn controls(&self) -> Arc<AudioControls> {
        self.controls.clone()
    }
    pub fn game_format(&self) -> PcmFormat {
        self.game_format
    }
    pub fn stop(&mut self) {
        if let Some(tx) = self.mix_stop.take() {
            let _ = tx.send(());
        }
        if let Some(t) = self.mixer.take() {
            let _ = t.join();
        }
        self.apps.clear();
        self.mic.take();
        self.loopback.take();
    }
}
impl Drop for WasapiAudioSource {
    fn drop(&mut self) {
        self.stop();
    }
}
fn encoding(f: PcmFormat) -> &'static str {
    if f.is_float {
        "IeeeFloat"
    } else {
        "Pcm"
    }
}

/// Per-app loopback mixer (apps mode). Owner, 2026-11-16: "crackling" in saved replays. The old mixer took a
/// fixed 20 ms from every queue on every 20 ms tick; capture packets and the tick both jitter, so a queue was
/// often a few samples short and the rest of the block became silence -> ~80-sample zero holes (clicks) that
/// also pushed the audio later each time. Now each source keeps a small jitter buffer: it only plays once
/// MIX_PRIME samples are queued, and only drops back to silence when it really ran dry (app went quiet).
pub(crate) struct AppMixer {
    primed: Vec<bool>,
}
/// 30 ms of interleaved 48 kHz stereo.
pub(crate) const MIX_PRIME: usize = 2 * 48 * 30;
/// Backlog above this (clock drift / stalled consumer) is trimmed back to MIX_PRIME: a rare single skip
/// instead of ever-growing latency.
pub(crate) const MIX_HIGH: usize = MIX_PRIME + 2 * 48 * 120;
impl AppMixer {
    pub(crate) fn new(sources: usize) -> Self {
        Self { primed: vec![false; sources] }
    }
    /// Fills `out` (interleaved stereo f32) with the sum of all queues.
    pub(crate) fn mix(&mut self, queues: &[Arc<Mutex<VecDeque<f32>>>], out: &mut [f32]) {
        out.fill(0.);
        for (i, q) in queues.iter().enumerate() {
            let mut q = q.lock().unwrap();
            if !self.primed[i] {
                if q.len() < MIX_PRIME {
                    continue;
                }
                self.primed[i] = true;
            }
            if q.len() > MIX_HIGH {
                let drop = (q.len() - MIX_PRIME) & !1;
                q.drain(..drop);
            }
            let take = out.len().min(q.len()) & !1;
            for (o, v) in out[..take].iter_mut().zip(q.drain(..take)) {
                *o += v;
            }
            if take < out.len() {
                self.primed[i] = false; // ran dry: wait for a full jitter buffer again
            }
        }
    }
}
/// How many 48 kHz frames are due, from the wall clock (not from counting timer ticks, which drift).
pub(crate) struct MixPacer {
    start: std::time::Instant,
    emitted: u64,
}
impl MixPacer {
    pub(crate) fn new(start: std::time::Instant) -> Self {
        Self { start, emitted: 0 }
    }
    pub(crate) fn due(&mut self, now: std::time::Instant) -> usize {
        let target = (now.saturating_duration_since(self.start).as_secs_f64() * 48000.0) as u64;
        // after a long stall, catch up at most 200 ms (no burst of stale silence)
        if target > self.emitted + 9600 {
            self.emitted = target - 9600;
        }
        let n = target.saturating_sub(self.emitted);
        self.emitted += n;
        n as usize
    }
}
#[cfg(test)]
mod mixer_tests {
    use super::*;
    fn queue() -> Arc<Mutex<VecDeque<f32>>> {
        Arc::new(Mutex::new(VecDeque::new()))
    }
    #[test]
    fn jittery_capture_has_no_holes_after_priming() {
        let q = queue();
        let qs = vec![q.clone()];
        let mut m = AppMixer::new(1);
        let mut out = vec![0f32; 960]; // 10 ms pulls
        let mut zeros_after_start = 0;
        let mut started = false;
        // producer: 10 ms per step on average, delivered in uneven bursts
        let pattern = [0usize, 2, 1, 0, 2, 1, 1, 0, 3, 0];
        for step in 0..2000 {
            let packets = pattern[step % pattern.len()];
            {
                let mut g = q.lock().unwrap();
                for _ in 0..packets * 960 {
                    g.push_back(1.0);
                }
            }
            m.mix(&qs, &mut out);
            for v in &out {
                if *v != 0.0 {
                    started = true;
                } else if started {
                    zeros_after_start += 1;
                }
            }
        }
        assert!(started);
        assert_eq!(zeros_after_start, 0, "holes in steady (jittery) audio");
    }
    #[test]
    fn silent_app_contributes_nothing_and_backlog_is_bounded() {
        let a = queue();
        let b = queue(); // never produces
        let mut m = AppMixer::new(2);
        let mut out = vec![0f32; 960];
        for _ in 0..100 {
            {
                let mut g = a.lock().unwrap();
                for _ in 0..1920 {
                    g.push_back(0.5); // twice as fast as consumed
                }
            }
            m.mix(&[a.clone(), b.clone()], &mut out);
        }
        assert!(a.lock().unwrap().len() <= MIX_HIGH + 1920);
        assert!(out.iter().all(|v| *v == 0.5));
    }
    #[test]
    fn pacer_follows_wall_clock() {
        let t0 = std::time::Instant::now();
        let mut p = MixPacer::new(t0);
        let mut total = 0;
        for ms in [7u64, 21, 30, 52, 60, 100] {
            total += p.due(t0 + Duration::from_millis(ms));
        }
        assert_eq!(total, 4800); // exactly 100 ms regardless of tick spacing
        assert_eq!(p.due(t0 + Duration::from_secs(5)), 9600); // stall: capped catch-up
    }
}
