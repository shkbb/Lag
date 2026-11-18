//! Implementation behind [`crate::engine::ReplayEngine`] (port of `VfrReplayEngine.cs`).
//!
//! Locking (deadlock-free by construction):
//! * `state` (C# `_gate`) guards the session handles, carry-over, target info;
//! * `Pipeline` (per session) guards the encoder + cadence grid + stall monitor and is locked by the WGC
//!   frame callback for each frame (uncontended in steady state);
//! * ORDER: a holder of a `Pipeline` lock may take `state`; nobody takes a `Pipeline` lock while holding
//!   `state` (teardown/save clone the handles out of `state` first).
//!
//! Everything from the C# engine is wired: video, audio (lag-audio, one shared clock per session), game
//! detection (detect), overlays (overlay). New vs C#: hot-swappable capture sources (game window <-> monitor in
//! the SAME session) for the "record the desktop when the game is out of focus" mode and for continuing on the
//! desktop after the game exits.

use crate::cadence::CadenceGrid;
use crate::carry::{self, CarryAudioTrack, CarryOverSegment, OwnedAudioTrack, Trimmed};
use crate::d3d::D3D11Context;
use crate::encoder::{self, EncoderChoice, VideoEncoder};
use crate::engine::{CaptureMode, EngineEvent, EngineOptions, EngineStatus, EventSink};
use crate::mux;
use crate::ring::PacketRing;
use crate::stall::CaptureStallMonitor;
use crate::wgc::{CaptureTarget, WgcCapture};
use lag_audio::clock::EngineClock;
use lag_audio::tracks::AudioSession;
use lag_audio::{AudioControls, AudioSourceOptions};
use lag_ffi::media::StreamSpec;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, RECT, STILL_ACTIVE};
use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFOEXW};
use windows::Win32::System::Threading::{
    GetCurrentProcess, GetExitCodeProcess, OpenProcess, QueryFullProcessImageNameW, SetPriorityClass,
    HIGH_PRIORITY_CLASS, NORMAL_PRIORITY_CLASS, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::WindowsAndMessaging::{GetWindowRect, GetWindowThreadProcessId, IsWindow};

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

// ---------------------------------------------------------------- engine clock (pause-aware, QPC based)

/// Session clock (C# EngineNowUs + PauseState): ONE `lag_audio::clock::EngineClock` per capture session,
/// shared by video and every audio track so they stay in sync and pause together.
struct Clock {
    inner: Mutex<Arc<EngineClock>>,
    pause_total: Mutex<(Duration, Option<Instant>)>, // only for the "total paused" log line
}

impl Clock {
    fn new() -> Self {
        Self { inner: Mutex::new(Arc::new(EngineClock::new())), pause_total: Mutex::new((Duration::ZERO, None)) }
    }
    /// A fresh origin (new session): never paused.
    fn restart(&self) {
        *lock(&self.inner) = Arc::new(EngineClock::new());
        *lock(&self.pause_total) = (Duration::ZERO, None);
    }
    fn current(&self) -> Arc<EngineClock> {
        lock(&self.inner).clone()
    }
    fn now_us(&self) -> i64 {
        lock(&self.inner).engine_now_us()
    }
    fn is_paused(&self) -> bool {
        lock(&self.inner).is_paused()
    }
    /// Returns `Some(total paused ms)` when the state changed.
    fn set_paused(&self, paused: bool) -> Option<u128> {
        let clock = self.current();
        if clock.is_paused() == paused {
            return None;
        }
        clock.set_paused(paused);
        let mut p = lock(&self.pause_total);
        match (paused, p.1) {
            (true, None) => p.1 = Some(Instant::now()),
            (false, Some(since)) => {
                p.0 += since.elapsed();
                p.1 = None;
            }
            _ => {}
        }
        Some(p.0.as_millis())
    }
}

// ---------------------------------------------------------------- per-session pipeline

struct Pipeline {
    encoder: VideoEncoder,
    grid: CadenceGrid,
    stalls: CaptureStallMonitor,
    timeline_reset_pending: bool,
    fps_adapt_done: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum LockKind {
    None,
    Game,
    Desktop,
}

/// Which capture source feeds the session's pipeline. Sources are hot-swappable: a new source is started
/// while the old one keeps delivering, becomes active on ITS first frame, and only then is the old one
/// stopped, so a switch (game window <-> monitor) never leaves a hole in the recording.
struct Sources {
    /// Id of the source whose frames are encoded.
    active: AtomicU64,
    /// Id of the source that becomes active on its first frame (0 = none).
    pending: AtomicU64,
}

/// What the current session's active source shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Showing {
    GameWindow,
    Monitor,
}

/// The game the session is locked to; kept while the game lives, even while the monitor is shown
/// ("follow screen" mode, Alt+Tab).
#[derive(Clone, Debug)]
struct GameRef {
    hwnd: isize,
    pid: u32,
    exe: String,
    name: String,
}

struct Session {
    capture: WgcCapture,
    /// Replaced sources, stopped off-thread once the new source is live.
    retiring: Vec<WgcCapture>,
    pipeline: Arc<Mutex<Pipeline>>,
    sources: Arc<Sources>,
    next_source_id: u64,
    out_w: i32,
    out_h: i32,
    showing: Showing,
    game: Option<GameRef>,
    /// HMONITOR recorded when the desktop is shown (the game's monitor, or the configured one).
    monitor: isize,
    /// Audio for the whole session (keeps running across source switches).
    audio: Option<AudioState>,
}

struct AudioState {
    session: AudioSession,
    /// Ring per track, indexed by `TrackPacket::track_index` (same order as `session.tracks().specs()`).
    rings: Arc<Vec<Arc<PacketRing>>>,
}

impl AudioState {
    /// Audio packets for a save/carry: (spec, packets, title) for every track that has a spec and data.
    fn tracks_with(&self, f: impl Fn(&PacketRing) -> Vec<lag_ffi::media::EncodedPacket>) -> Vec<OwnedAudioTrack> {
        self.session
            .tracks()
            .specs()
            .into_iter()
            .zip(self.rings.iter())
            .filter_map(|(spec, ring)| {
                let stream = spec.stream?;
                let packets = f(ring);
                (!packets.is_empty()).then(|| OwnedAudioTrack { spec: stream, packets, title: spec.name.to_string() })
            })
            .collect()
    }
}

/// Starts the session's audio (C# StartAudio). Failure is non-fatal: video-only.
fn start_audio(opts: &EngineOptions, clock: Arc<EngineClock>, is_game: bool, game_pid: u32) -> Option<AudioState> {
    if !opts.audio_enabled {
        log::info!("[VfrReplayEngine] audio disabled (video-only).");
        return None;
    }
    let controls = Arc::new(AudioControls::default());
    controls.set_mic_volume(opts.mic_volume);
    controls.set_mic_muted(opts.mic_start_muted);
    controls.set_input_gate(opts.input_gate_auto, opts.input_gate_threshold);
    controls.set_noise_suppression(opts.noise_suppression);
    let source = AudioSourceOptions {
        microphone_id: opts.microphone_id.clone(),
        apps_mode: if opts.audio_mode_by_target { is_game } else { opts.apps_audio_mode },
        apps: opts.audio_apps.clone(),
        system_enabled: opts.system_audio_enabled,
        system_volume: opts.system_volume,
        mic_mono: opts.mic_mono,
        game_pid: if is_game { game_pid } else { 0 },
        game_volume: opts.game_volume,
        game_enabled: opts.game_audio_enabled,
        system_device_id: opts.system_audio_device_id.clone(),
    };
    // Rings are created lazily per track index (the mic track appears on the first mic buffer).
    let rings: Arc<Mutex<Vec<Arc<PacketRing>>>> = Arc::new(Mutex::new(Vec::new()));
    let buffer = opts.buffer_seconds;
    let sink_rings = rings.clone();
    let sink = move |p: lag_audio::tracks::TrackPacket| {
        let ring = {
            let mut r = lock(&sink_rings);
            while r.len() <= p.track_index {
                r.push(Arc::new(PacketRing::new(buffer)));
            }
            r[p.track_index].clone()
        };
        ring.add(p.packet);
    };
    match AudioSession::start(source, controls, opts.separate_audio_tracks, clock, sink) {
        Ok(session) => {
            // Pre-create one ring per known track so ordering/indexing is fixed from the start.
            let n = session.tracks().specs().len();
            {
                let mut r = lock(&rings);
                while r.len() < n.max(1) {
                    r.push(Arc::new(PacketRing::new(buffer)));
                }
            }
            let fixed = Arc::new(lock(&rings).clone());
            Some(AudioState { session, rings: fixed })
        }
        Err(e) => {
            log::info!("[VfrReplayEngine] audio capture unavailable \u{2014} video-only: {e}");
            None
        }
    }
}

struct State {
    options: EngineOptions,
    d3d: Option<Arc<D3D11Context>>,
    encoder_choice: Option<EncoderChoice>,
    session: Option<Session>,
    ring: Option<Arc<PacketRing>>,
    carry: Option<CarryOverSegment>,
    lock_kind: LockKind,
    locked_hwnd: isize,
    locked_pid: u32,
    lock_cooldown_until: Option<Instant>,
    target_active: bool,
    target_is_game: bool,
    target_name: String,
    target_exe: Option<String>,
    last_saved_game_tag: Option<(String, Option<String>)>,
}

struct Shared {
    events: EventSink,
    state: Mutex<State>,
    clock: Clock,
    running: AtomicBool,
    frames_captured: AtomicU64,
    frames_encoded: AtomicU64,
    stats: Mutex<StatsWindow>,
    /// Recording overlays (webcam / keys / stats), engine-wide like C#'s `_overlay`; used by the frame path.
    overlay: Mutex<Option<SendOverlay>>,
}

/// The compositor holds D2D/DirectWrite COM objects. They are only ever touched under the `overlay` mutex
/// (one frame at a time), which is the serialised access D2D allows; the D3D device is multithread-protected.
struct SendOverlay(crate::overlay::OverlayCompositor);
// SAFETY: see the type docs: access is serialised by `Shared::overlay`.
unsafe impl Send for SendOverlay {}

#[derive(Default)]
struct StatsWindow {
    pipeline: Option<usize>, // identity (Arc ptr) of the pipeline the baseline belongs to
    start_us: i64,
    frames0: i64,
    convert0: i64,
    encode0: i64,
    captured0: u64,
}

pub(crate) struct Engine {
    shared: Arc<Shared>,
    timer: Mutex<Option<(JoinHandle<()>, Arc<(Mutex<bool>, Condvar)>)>>,
}

impl Engine {
    pub(crate) fn new(events: EventSink) -> Self {
        let state = State {
            options: EngineOptions::default(),
            d3d: None,
            encoder_choice: None,
            session: None,
            ring: None,
            carry: None,
            lock_kind: LockKind::None,
            locked_hwnd: 0,
            locked_pid: 0,
            lock_cooldown_until: None,
            target_active: false,
            target_is_game: false,
            target_name: String::new(),
            target_exe: None,
            last_saved_game_tag: None,
        };
        Self {
            shared: Arc::new(Shared {
                events,
                state: Mutex::new(state),
                clock: Clock::new(),
                running: AtomicBool::new(false),
                frames_captured: AtomicU64::new(0),
                frames_encoded: AtomicU64::new(0),
                stats: Mutex::new(StatsWindow::default()),
                overlay: Mutex::new(None),
            }),
            timer: Mutex::new(None),
        }
    }

    pub(crate) fn start(&self, options: EngineOptions) -> Result<(), String> {
        crate::perf_guard::apply_once();
        {
            let mut st = lock(&self.shared.state);
            if st.d3d.is_none() {
                st.d3d = Some(Arc::new(D3D11Context::new(options.adapter_index).map_err(|e| e.message())?));
            }
            let choice = encoder::select(options.preferred_codec.as_deref()).ok_or("No usable video encoder.")?;
            log::info!(
                "[VfrReplayEngine] Armed. Encoder: {}, buffer {}s.",
                choice.friendly_name,
                options.buffer_seconds
            );
            st.encoder_choice = Some(choice);
            let overlay_opts = crate::overlay::OverlayOptions {
                webcam_enabled: options.webcam_overlay,
                webcam_device_id: options.webcam_device_id.clone(),
                webcam_x: options.webcam_x,
                webcam_y: options.webcam_y,
                webcam_scale: options.webcam_scale,
                webcam_format: options.webcam_format,
                keys_enabled: options.keys_overlay,
                keys_x: options.keys_x,
                keys_y: options.keys_y,
                keys_scale: options.keys_scale,
                stats_enabled: options.stats_overlay,
                stats_x: options.stats_x,
                stats_y: options.stats_y,
                stats_scale: options.stats_scale,
                stats_detail: options.stats_detail,
            };
            st.options = options;
            // Recreated on every Start, like C# (dispose the old one first: it owns the webcam + key hook).
            let mut ov = lock(&self.shared.overlay);
            *ov = None;
            if overlay_opts.any_enabled() {
                let d3d = st.d3d.clone().expect("device created above");
                match crate::overlay::OverlayCompositor::new(&d3d, overlay_opts) {
                    Ok(o) => *ov = Some(SendOverlay(o)),
                    Err(e) => log::info!("[VfrReplayEngine] overlay init failed: {}", e.message()),
                }
            }
        }
        self.shared.running.store(true, Ordering::Release);
        let mut timer = lock(&self.timer);
        if timer.is_none() {
            let stop = Arc::new((Mutex::new(false), Condvar::new()));
            let (shared, stop2) = (self.shared.clone(), stop.clone());
            let handle = std::thread::Builder::new()
                .name("LagLockTimer".into())
                .spawn(move || lock_timer(shared, stop2))
                .map_err(|e| e.to_string())?;
            *timer = Some((handle, stop));
        }
        Ok(())
    }

    pub(crate) fn stop(&self) {
        self.shared.running.store(false, Ordering::Release);
        if let Some((handle, stop)) = lock(&self.timer).take() {
            *lock(&stop.0) = true;
            stop.1.notify_all();
            let _ = handle.join();
        }
        teardown(&self.shared, false);
        *lock(&self.shared.overlay) = None; // C# Stop(): _overlay.Dispose()
    }

    pub(crate) fn frames_captured(&self) -> u64 {
        self.shared.frames_captured.load(Ordering::Relaxed)
    }

    pub(crate) fn status(&self) -> EngineStatus {
        let st = lock(&self.shared.state);
        let buffered = st.ring.as_ref().map_or(0.0, |r| r.buffered_seconds())
            + st.carry.as_ref().map_or(0.0, |c| c.duration_seconds());
        let avg = st
            .session
            .as_ref()
            .and_then(|s| s.pipeline.try_lock().ok().map(|p| p.encoder.timings))
            .filter(|t| t.frames > 0)
            .map_or((0.0, 0.0), |t| {
                (t.convert_us as f64 / 1000.0 / t.frames as f64, t.encode_us as f64 / 1000.0 / t.frames as f64)
            });
        EngineStatus {
            running: self.shared.running.load(Ordering::Acquire),
            paused: self.shared.clock.is_paused(),
            target_active: st.target_active,
            target_is_game: st.target_is_game,
            target_name: st.target_name.clone(),
            target_exe: st.target_exe.clone(),
            buffered_seconds: buffered,
            frames_captured: self.shared.frames_captured.load(Ordering::Relaxed),
            frames_encoded: self.shared.frames_encoded.load(Ordering::Relaxed),
            friendly_encoder: st.encoder_choice.as_ref().map_or("\u{2014}".into(), |c| c.friendly_name.to_string()),
            avg_stage_ms: avg,
        }
    }

    pub(crate) fn last_saved_game_tag(&self) -> Option<(String, Option<String>)> {
        lock(&self.shared.state).last_saved_game_tag.clone()
    }

    pub(crate) fn set_paused(&self, paused: bool) {
        if !self.shared.running.load(Ordering::Acquire) {
            return;
        }
        let st = lock(&self.shared.state);
        if st.session.is_none() {
            return;
        }
        if let Some(total_ms) = self.shared.clock.set_paused(paused) {
            if !paused {
                // The clock jumped back by the pause: re-anchor grid and stall tracking on the next frame.
                if let Some(s) = &st.session {
                    if let Ok(mut p) = s.pipeline.lock() {
                        p.timeline_reset_pending = true;
                    }
                }
            }
            log::info!("[VfrReplayEngine] {} (total paused {total_ms}ms).", if paused { "paused" } else { "resumed" });
        }
    }

    pub(crate) fn set_mic_muted(&self, muted: bool) {
        let st = lock(&self.shared.state);
        if let Some(a) = st.session.as_ref().and_then(|s| s.audio.as_ref()) {
            a.session.controls().set_mic_muted(muted);
        }
    }

    pub(crate) fn save_replay(&self) -> Option<String> {
        save_replay(&self.shared)
    }
}

// ---------------------------------------------------------------- lock timer (C# _lockTimer, 500 ms then every 2 s)

fn lock_timer(shared: Arc<Shared>, stop: Arc<(Mutex<bool>, Condvar)>) {
    // The detector holds PDH handles (not Send): it lives on this thread, like C#'s timer-owned detector.
    let mut detector = crate::detect::GameDetector::new();
    let mut wait = Duration::from_millis(500);
    loop {
        {
            let (m, cv) = &*stop;
            let g = lock(m);
            let (g, _) = cv.wait_timeout_while(g, wait, |s| !*s).unwrap_or_else(|e| e.into_inner());
            if *g {
                return;
            }
        }
        wait = Duration::from_secs(2);
        if !shared.running.load(Ordering::Acquire) {
            continue;
        }
        evaluate_lock(&shared, &mut detector);
        trim_carry_over(&shared);
        log_capture_stats(&shared);
    }
}

fn evaluate_lock(shared: &Arc<Shared>, detector: &mut crate::detect::GameDetector) {
    let (kind, hwnd, pid, cooldown, forced, mode, monitor) = {
        let st = lock(&shared.state);
        (
            st.lock_kind,
            st.locked_hwnd,
            st.locked_pid,
            st.lock_cooldown_until,
            st.options.forced_window,
            st.options.mode,
            st.options.monitor_device_id.clone(),
        )
    };
    let mut kind = kind;
    if kind == LockKind::Game {
        if is_window(hwnd) && is_process_alive(pid) {
            return;
        }
        // The game is gone. Keep recording without a hole: the SAME session switches to the desktop, so the
        // game footage stays in the buffer and a save shows game + desktop as one continuous clip.
        log::info!("[VfrReplayEngine] Locked window ended \u{2014} continuing on the desktop (same recording).");
        if game_ended_switch_to_desktop(shared) {
            return;
        }
        log::info!("[VfrReplayEngine] desktop switch failed \u{2014} releasing capture (carry-over keeps the game).");
        teardown(shared, true);
        kind = LockKind::None;
    }
    if cooldown.is_some_and(|t| Instant::now() < t) {
        return;
    }
    if forced != 0 {
        if kind != LockKind::None || !is_window(forced) {
            return;
        }
        let fpid = window_pid(forced);
        if fpid == 0 {
            return;
        }
        let Some(exe) = process_exe_name(fpid) else { return };
        lock_onto_window(shared, forced, fpid, &exe, None);
        return;
    }
    if mode == CaptureMode::Display {
        if kind != LockKind::Desktop {
            teardown(shared, true);
            lock_onto_monitor(shared, monitor.as_deref());
        }
        return;
    }
    if let Some(cand) = detector.find_best_game(monitor.as_deref()) {
        if kind == LockKind::Desktop {
            log::info!("[VfrReplayEngine] Game appeared ({}) \u{2014} switching from desktop to game.", cand.exe);
            // Same session when the game fits the session frame (no stitch, no hole); else a new session
            // and the desktop part is carried over (C# behaviour).
            if desktop_to_game_in_place(shared, &cand) {
                return;
            }
            teardown(shared, true);
            kind = LockKind::None;
        }
        if kind == LockKind::None {
            log::info!(
                "[VfrReplayEngine] Game detected: {} ({}) score={} fullscreen={}",
                cand.game_name,
                cand.exe,
                cand.score,
                cand.fullscreen
            );
            lock_onto_window(shared, cand.hwnd, cand.pid, &cand.exe, Some(&cand.game_name));
        }
        return;
    }
    if kind == LockKind::None {
        lock_onto_monitor(shared, monitor.as_deref());
    }
}

// ---------------------------------------------------------------- hot-swappable sources

fn window_rect(hwnd: isize) -> Option<(i32, i32)> {
    let mut r = RECT::default();
    unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut r) }.ok()?;
    Some((r.right - r.left, r.bottom - r.top))
}

fn monitor_of_window(hwnd: isize) -> isize {
    use windows::Win32::Graphics::Gdi::{MonitorFromWindow, MONITOR_DEFAULTTONEAREST};
    unsafe { MonitorFromWindow(HWND(hwnd as *mut _), MONITOR_DEFAULTTONEAREST) }.0 as isize
}

/// Same aspect within 2%: the source fills the session frame without bars.
fn aspect_compatible(w: i32, h: i32, out_w: i32, out_h: i32) -> bool {
    w > 1 && h > 1 && ((w as f64 / h as f64) / (out_w as f64 / out_h as f64) - 1.0).abs() < 0.02
}

/// Starts `target` as a new source of the current session and makes it active on its first frame; the
/// previous source keeps feeding the encoder until then and is stopped afterwards (off-thread).
fn switch_source(shared: &Arc<Shared>, target: CaptureTarget, showing: Showing) -> Result<(), String> {
    let (d3d, opts, pipeline, sources, id) = {
        let mut st = lock(&shared.state);
        let d3d = st.d3d.clone().ok_or("no device")?;
        let opts = st.options.clone();
        let s = st.session.as_mut().ok_or("no session")?;
        let id = s.next_source_id;
        s.next_source_id += 1;
        (d3d, opts, s.pipeline.clone(), s.sources.clone(), id)
    };
    sources.pending.store(id, Ordering::Release); // before start: the first frame may arrive during start
    let mut capture = WgcCapture::new(target);
    let cb = source_callback(shared, &pipeline, &sources, id);
    if let Err(e) = capture.start(&d3d, opts.capture_cursor, opts.max_fps, cb, Box::new(|| {})) {
        let _ = sources.pending.compare_exchange(id, 0, Ordering::AcqRel, Ordering::Acquire);
        return Err(e.message());
    }
    {
        let mut st = lock(&shared.state);
        let Some(s) = st.session.as_mut() else { return Err("session ended".into()) };
        let old = std::mem::replace(&mut s.capture, capture);
        s.retiring.push(old);
        s.showing = showing;
    }
    // Reaper: once the new source is live (or after 1 s, e.g. a static screen that sends no new frame), make
    // it active and stop the replaced sources. Never on a WGC callback thread.
    let shared2 = shared.clone();
    let _ = std::thread::Builder::new().name("LagSourceReaper".into()).spawn(move || {
        let t = Instant::now();
        while sources.active.load(Ordering::Acquire) != id && t.elapsed() < Duration::from_secs(1) {
            std::thread::sleep(Duration::from_millis(5));
        }
        if sources.pending.compare_exchange(id, 0, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            sources.active.store(id, Ordering::Release); // timed out waiting for its first frame
        }
        let retired: Vec<WgcCapture> = {
            let mut st = lock(&shared2.state);
            st.session.as_mut().map(|s| std::mem::take(&mut s.retiring)).unwrap_or_default()
        };
        for mut c in retired {
            c.stop();
        }
        log::info!(
            "[VfrReplayEngine] source switch {} after {}ms.",
            if t.elapsed() < Duration::from_secs(1) { "seamless" } else { "forced" },
            t.elapsed().as_millis()
        );
    });
    Ok(())
}

fn set_target(shared: &Arc<Shared>, game: Option<&GameRef>, showing: Showing) {
    {
        let mut st = lock(&shared.state);
        st.target_active = true;
        st.target_is_game = showing == Showing::GameWindow;
        st.target_name = if showing == Showing::GameWindow {
            game.map(|g| g.name.clone()).unwrap_or_default()
        } else {
            String::new()
        };
        st.target_exe = if showing == Showing::GameWindow { game.map(|g| g.exe.clone()) } else { None };
    }
    (shared.events)(EngineEvent::TargetChanged);
}

/// The locked game ended: show the desktop in the same session (always, in both capture modes).
fn game_ended_switch_to_desktop(shared: &Arc<Shared>) -> bool {
    let (showing, monitor) = {
        let st = lock(&shared.state);
        match st.session.as_ref() {
            Some(s) => (s.showing, s.monitor),
            None => return false,
        }
    };
    if showing == Showing::GameWindow
        && switch_source(shared, CaptureTarget::Monitor(monitor), Showing::Monitor).is_err()
    {
        return false;
    }
    {
        let mut st = lock(&shared.state);
        if let Some(s) = st.session.as_mut() {
            s.game = None;
        }
        st.lock_kind = LockKind::Desktop;
        st.locked_hwnd = 0;
        st.locked_pid = 0;
    }
    set_target(shared, None, Showing::Monitor);
    true
}

/// A game appeared while recording the desktop: switch the same session to it when its window fits the
/// session frame (same aspect); otherwise the caller starts a new session (with carry-over).
fn desktop_to_game_in_place(shared: &Arc<Shared>, cand: &crate::detect::GameCandidate) -> bool {
    let Some((w, h)) = window_rect(cand.hwnd) else { return false };
    let fits = {
        let st = lock(&shared.state);
        st.session.as_ref().is_some_and(|s| aspect_compatible(w, h, s.out_w, s.out_h))
    };
    if !fits {
        return false;
    }
    let follow = lock(&shared.state).options.follow_focus;
    let game = GameRef {
        hwnd: cand.hwnd,
        pid: cand.pid,
        exe: cand.exe.clone(),
        name: if cand.game_name.trim().is_empty() { cand.exe.clone() } else { cand.game_name.clone() },
    };
    // In "follow screen" mode the game only replaces the desktop while it has focus.
    let show_game = !follow || game_has_focus(&game);
    if show_game && switch_source(shared, CaptureTarget::Window(game.hwnd), Showing::GameWindow).is_err() {
        return false;
    }
    {
        let mut st = lock(&shared.state);
        if let Some(s) = st.session.as_mut() {
            s.game = Some(game.clone());
            s.monitor = monitor_of_window(game.hwnd);
        }
        st.lock_kind = LockKind::Game;
        st.locked_hwnd = game.hwnd;
        st.locked_pid = game.pid;
    }
    set_target(shared, Some(&game), if show_game { Showing::GameWindow } else { Showing::Monitor });
    true
}

fn game_has_focus(game: &GameRef) -> bool {
    use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;
    let fg = unsafe { GetForegroundWindow() }.0 as isize;
    fg != 0 && (fg == game.hwnd || window_pid(fg) == game.pid)
}

/// "Follow screen" mode: called on every foreground change (focus watcher thread).
fn on_foreground_changed(shared: &Arc<Shared>, fg: isize) {
    if fg == 0 {
        return; // transient "no foreground" during Alt+Tab
    }
    let (follow, game, showing, monitor) = {
        let st = lock(&shared.state);
        let Some(s) = st.session.as_ref() else { return };
        (st.options.follow_focus, s.game.clone(), s.showing, s.monitor)
    };
    let Some(game) = game else { return };
    if !follow {
        return;
    }
    let focused = fg == game.hwnd || window_pid(fg) == game.pid;
    let want = if focused { Showing::GameWindow } else { Showing::Monitor };
    if want == showing {
        return;
    }
    let target = if focused { CaptureTarget::Window(game.hwnd) } else { CaptureTarget::Monitor(monitor) };
    match switch_source(shared, target, want) {
        Ok(()) => {
            log::info!(
                "[VfrReplayEngine] focus {} \u{2014} recording {}.",
                if focused { "returned to the game" } else { "left the game" },
                if focused { "the game" } else { "the screen" }
            );
            set_target(shared, Some(&game), want);
        }
        Err(e) => log::info!("[VfrReplayEngine] focus switch failed: {e}"),
    }
}

// ---------------------------------------------------------------- focus watcher (EVENT_SYSTEM_FOREGROUND)

static FOCUS_SINK: Mutex<Option<std::sync::mpsc::Sender<isize>>> = Mutex::new(None);

unsafe extern "system" fn on_win_event(
    _hook: windows::Win32::UI::Accessibility::HWINEVENTHOOK,
    _event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _thread: u32,
    _time: u32,
) {
    // Hook callback: only enqueue (never block the system's event delivery).
    if let Some(tx) = lock(&FOCUS_SINK).as_ref() {
        let _ = tx.send(hwnd.0 as isize);
    }
}

/// Starts (once per process) the foreground watcher: a message-loop thread with an out-of-context
/// WinEvent hook, plus a worker that applies the switches.
fn start_focus_watcher(shared: &Arc<Shared>) {
    static STARTED: std::sync::Once = std::sync::Once::new();
    let shared = Arc::downgrade(shared);
    STARTED.call_once(move || {
        let (tx, rx) = std::sync::mpsc::channel::<isize>();
        *lock(&FOCUS_SINK) = Some(tx);
        let _ = std::thread::Builder::new().name("LagFocusHook".into()).spawn(|| unsafe {
            use windows::Win32::UI::Accessibility::SetWinEventHook;
            use windows::Win32::UI::WindowsAndMessaging::{
                DispatchMessageW, GetMessageW, TranslateMessage, EVENT_SYSTEM_FOREGROUND, MSG, WINEVENT_OUTOFCONTEXT,
                WINEVENT_SKIPOWNPROCESS,
            };
            let hook = SetWinEventHook(
                EVENT_SYSTEM_FOREGROUND,
                EVENT_SYSTEM_FOREGROUND,
                None,
                Some(on_win_event),
                0,
                0,
                WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
            );
            if hook.is_invalid() {
                log::info!("[VfrReplayEngine] foreground hook unavailable \u{2014} follow-screen mode disabled.");
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        });
        let _ = std::thread::Builder::new().name("LagFocusWorker".into()).spawn(move || {
            while let Ok(mut fg) = rx.recv() {
                // Alt+Tab fires a burst of foreground changes; act on the latest one.
                while let Ok(next) = rx.try_recv() {
                    fg = next;
                }
                let Some(shared) = shared.upgrade() else { return };
                if shared.running.load(Ordering::Acquire) {
                    on_foreground_changed(&shared, fg);
                }
            }
        });
    });
}

fn output_dims(target_height: i32, src_w: i32, src_h: i32) -> (i32, i32) {
    let out_h = if target_height > 0 && target_height < src_h { target_height } else { src_h };
    let out_w = ((src_w as f64 * out_h as f64 / src_h as f64).round()) as i32;
    (out_w & !1, out_h & !1)
}

fn lock_onto_window(shared: &Arc<Shared>, hwnd: isize, pid: u32, exe: &str, game_name: Option<&str>) {
    let mut r = RECT::default();
    if unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut r) }.is_err() {
        return;
    }
    let (w, h) = (r.right - r.left, r.bottom - r.top);
    if w < 2 || h < 2 {
        return;
    }
    let th = lock(&shared.state).options.target_height;
    let (ow, oh) = output_dims(th, w, h);
    let name = game_name.filter(|n| !n.trim().is_empty()).unwrap_or(exe).to_string();
    start_session(
        shared,
        CaptureTarget::Window(hwnd),
        ow,
        oh,
        LockKind::Game,
        hwnd,
        pid,
        &format!("game {exe}"),
        name,
        Some(exe.to_string()),
    );
}

fn lock_onto_monitor(shared: &Arc<Shared>, device: Option<&str>) {
    let Some((hmon, w, h, name)) = resolve_monitor(device) else {
        log::info!("[VfrReplayEngine] no monitor to capture (id='{}').", device.unwrap_or(""));
        return;
    };
    if w < 2 || h < 2 {
        log::info!("[VfrReplayEngine] no monitor to capture (id='{}').", device.unwrap_or(""));
        return;
    }
    let th = lock(&shared.state).options.target_height;
    let (ow, oh) = output_dims(th, w, h);
    start_session(
        shared,
        CaptureTarget::Monitor(hmon),
        ow,
        oh,
        LockKind::Desktop,
        0,
        0,
        &format!("desktop {name} ({w}x{h})"),
        String::new(),
        None,
    );
}

#[allow(clippy::too_many_arguments)]
fn start_session(
    shared: &Arc<Shared>,
    target: CaptureTarget,
    out_w: i32,
    out_h: i32,
    kind: LockKind,
    hwnd: isize,
    pid: u32,
    label: &str,
    target_name: String,
    target_exe: Option<String>,
) {
    let result = (|| -> Result<(), String> {
        let (d3d, choice, opts) = {
            let st = lock(&shared.state);
            (st.d3d.clone().ok_or("no device")?, st.encoder_choice.clone().ok_or("no encoder")?, st.options.clone())
        };
        let fps_hint = if opts.max_fps > 0 { opts.max_fps } else { 120 };
        let enc = VideoEncoder::new(&d3d, choice, out_w, out_h, opts.bitrate_kbps, opts.preset, fps_hint)?;
        let ring = Arc::new(PacketRing::new(opts.buffer_seconds));
        let pipeline = Arc::new(Mutex::new(Pipeline {
            encoder: enc,
            grid: CadenceGrid::new(if opts.max_fps > 0 { opts.max_fps as i64 } else { 0 }),
            stalls: CaptureStallMonitor::default(),
            timeline_reset_pending: true, // new session: re-anchor grid + stall tracking on frame 1
            fps_adapt_done: false,
        }));
        let mut capture = WgcCapture::new(target);
        let game = (kind == LockKind::Game).then(|| GameRef {
            hwnd,
            pid,
            exe: target_exe.clone().unwrap_or_default(),
            name: target_name.clone(),
        });
        {
            let mut st = lock(&shared.state);
            st.ring = Some(ring);
            st.lock_kind = kind;
            st.locked_hwnd = hwnd;
            st.locked_pid = pid;
            st.target_active = true;
            st.target_is_game = kind == LockKind::Game;
            st.target_name = target_name;
            st.target_exe = target_exe;
        }
        shared.clock.restart(); // a fresh session/clock is never paused
        let sources = Arc::new(Sources { active: AtomicU64::new(1), pending: AtomicU64::new(0) });
        let on_frame = source_callback(shared, &pipeline, &sources, 1);
        let on_closed = Box::new(|| {});
        capture.start(&d3d, opts.capture_cursor, opts.max_fps, on_frame, on_closed).map_err(|e| e.message())?;
        let monitor = match target {
            CaptureTarget::Monitor(m) => m,
            CaptureTarget::Window(h) => monitor_of_window(h),
        };
        lock(&shared.state).session = Some(Session {
            capture,
            retiring: Vec::new(),
            pipeline,
            sources,
            next_source_id: 2,
            out_w,
            out_h,
            showing: if kind == LockKind::Game { Showing::GameWindow } else { Showing::Monitor },
            game: game.clone(),
            monitor,
            audio: None,
        });
        // Audio for the whole session (C# StartAudio): same clock as video.
        let audio = start_audio(&opts, shared.clock.current(), kind == LockKind::Game, pid);
        if let Some(s) = lock(&shared.state).session.as_mut() {
            s.audio = audio;
        }
        if opts.follow_focus {
            start_focus_watcher(shared);
            // Locked while the game is not in front (e.g. detected in the background): show the screen now.
            if let Some(g) = &game {
                if !game_has_focus(g) {
                    on_foreground_changed(
                        shared,
                        unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() }.0 as isize,
                    );
                }
            }
        }
        unsafe {
            let _ = SetPriorityClass(GetCurrentProcess(), HIGH_PRIORITY_CLASS);
        }
        // AUDIO: start the audio session (game pid for apps mode) here.
        log::info!(
            "[VfrReplayEngine] Locked onto {label} @ {out_w}x{out_h} (WGC {} capture).",
            if kind == LockKind::Desktop { "monitor" } else { "window" }
        );
        (shared.events)(EngineEvent::TargetChanged);
        Ok(())
    })();
    if let Err(e) = result {
        log::info!("[VfrReplayEngine] Failed to lock onto {label}: {e} \u{2014} backing off 15s (no retry-thrash).");
        lock(&shared.state).lock_cooldown_until = Some(Instant::now() + Duration::from_secs(15));
        teardown(shared, true);
    }
}

fn source_callback(
    shared: &Arc<Shared>,
    pipeline: &Arc<Mutex<Pipeline>>,
    sources: &Arc<Sources>,
    id: u64,
) -> crate::wgc::FrameCallback {
    let (shared, pipeline, sources) = (shared.clone(), pipeline.clone(), sources.clone());
    Box::new(move |tex: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D, _ticks: i64| {
        if sources.active.load(Ordering::Acquire) != id {
            // First frame of the source we are switching to: it takes over from here.
            if sources.pending.compare_exchange(id, 0, Ordering::AcqRel, Ordering::Acquire).is_err() {
                return; // a replaced (retiring) source: ignore
            }
            sources.active.store(id, Ordering::Release);
        }
        on_frame(&shared, &pipeline, tex);
    })
}

fn on_frame(
    shared: &Arc<Shared>,
    pipeline: &Arc<Mutex<Pipeline>>,
    tex: &windows::Win32::Graphics::Direct3D11::ID3D11Texture2D,
) {
    if shared.clock.is_paused() {
        return;
    }
    let mut p = lock(pipeline);
    shared.frames_captured.fetch_add(1, Ordering::Relaxed);
    if p.timeline_reset_pending {
        p.timeline_reset_pending = false;
        p.grid.repin();
        p.stalls.reset();
    }
    let now_us = shared.clock.now_us();
    p.stalls.frame_arrived(now_us, now_us);
    let (mut overlay_us, mut convert_us, mut encode_us, mut retune_us) = (0, 0, 0, 0);
    if let Some(pts) = p.grid.snap(now_us) {
        let o0 = Instant::now();
        let composed = {
            let mut ov = lock(&shared.overlay);
            match ov.as_mut().map(|o| o.0.compose(tex)) {
                Some(Ok(Some(t))) => Some(t),
                Some(Err(e)) => {
                    log::info!("[VfrReplayEngine] overlay compose error: {}", e.message());
                    None
                }
                _ => None,
            }
        };
        overlay_us = o0.elapsed().as_micros() as i64;
        let frame = composed.as_ref().unwrap_or(tex);
        let t0 = p.encoder.timings;
        match p.encoder.encode(frame, pts) {
            Ok(packets) => {
                if let Some(ring) = lock(&shared.state).ring.clone() {
                    for pk in packets {
                        ring.add(pk);
                    }
                }
            }
            Err(_) => {}
        }
        let t1 = p.encoder.timings;
        convert_us = t1.convert_us - t0.convert_us;
        encode_us = t1.encode_us - t0.encode_us;
        shared.frames_encoded.fetch_add(1, Ordering::Relaxed);
        let bitrate = lock(&shared.state).options.bitrate_kbps;
        if bitrate > 0 && !p.fps_adapt_done {
            let r0 = Instant::now();
            maybe_adapt_fps_hint(shared, &mut p, now_us);
            retune_us = r0.elapsed().as_micros() as i64;
        }
    }
    p.stalls.frame_done(overlay_us, convert_us, encode_us, retune_us);
}

/// Fixed-bitrate only: after ~3 s, reopen the encoder with the measured fps as the rate-control hint when
/// it is >15% below the configured one (CBR splits bits by the hint). One-shot per session.
fn maybe_adapt_fps_hint(shared: &Arc<Shared>, p: &mut Pipeline, elapsed_us: i64) {
    if elapsed_us < 3_000_000 {
        return;
    }
    p.fps_adapt_done = true;
    let hint = p.encoder.fps_hint();
    let encoded = shared.frames_encoded.load(Ordering::Relaxed) as i64;
    let measured = ((encoded * 1_000_000) / elapsed_us.max(1)).clamp(5, hint as i64) as i32;
    if ((hint - measured) as f64) < hint as f64 * 0.15 {
        return;
    }
    log::info!("[fps-adapt] reopening encoder {hint}\u{2192}{measured} ...");
    let (d3d, choice, opts) = {
        let st = lock(&shared.state);
        match (st.d3d.clone(), st.encoder_choice.clone()) {
            (Some(d), Some(c)) => (d, c, st.options.clone()),
            _ => return,
        }
    };
    let fresh = match VideoEncoder::new(
        &d3d,
        choice,
        p.encoder.width(),
        p.encoder.height(),
        opts.bitrate_kbps,
        opts.preset,
        measured,
    ) {
        Ok(e) => e,
        Err(e) => {
            log::info!("[fps-adapt] reopen failed, keeping current encoder: {e}");
            return;
        }
    };
    lock(&shared.state).ring = Some(Arc::new(PacketRing::new(opts.buffer_seconds)));
    log::info!("[fps-adapt] new encoder live, disposing old ...");
    p.encoder = fresh; // old encoder drops here
    log::info!("[fps-adapt] done \u{2014} fixed-bitrate fps-hint {hint}\u{2192}{measured}, buffer reset.");
}

// ---------------------------------------------------------------- stats ([capture] + [stall] lines)

fn log_capture_stats(shared: &Arc<Shared>) {
    let Ok(mut w) = shared.stats.try_lock() else { return };
    let pipeline = lock(&shared.state).session.as_ref().map(|s| s.pipeline.clone());
    let Some(pipeline) = pipeline else {
        w.pipeline = None;
        return;
    };
    let id = Arc::as_ptr(&pipeline) as usize;
    let now_us = shared.clock.now_us();
    let p = lock(&pipeline);
    let t = p.encoder.timings;
    let captured = shared.frames_captured.load(Ordering::Relaxed);
    if w.pipeline != Some(id) {
        *w = StatsWindow {
            pipeline: Some(id),
            start_us: now_us,
            frames0: t.frames,
            convert0: t.convert_us,
            encode0: t.encode_us,
            captured0: captured,
        };
        p.grid.take_collisions();
        p.stalls.drain_report(); // stalls from the previous session belong to it
        return;
    }
    let secs = (now_us - w.start_us) as f64 / 1e6;
    if secs < 1.0 {
        return;
    }
    let df = t.frames - w.frames0;
    let dcap = captured - w.captured0;
    let drops = p.grid.take_collisions();
    if df > 0 {
        let conv = (t.convert_us - w.convert0) as f64 / 1000.0 / df as f64;
        let enc = (t.encode_us - w.encode0) as f64 / 1000.0 / df as f64;
        let preset = lock(&shared.state).options.preset;
        log::info!(
            "[capture] encoded {:.0} fps (wgc {:.0}) | convert {conv:.1}ms + encode {enc:.1}ms/frame | preset p{preset} | grid {:.0}fps, {drops} off-slot",
            df as f64 / secs,
            dcap as f64 / secs,
            p.grid.fps()
        );
    }
    *w = StatsWindow {
        pipeline: Some(id),
        start_us: now_us,
        frames0: t.frames,
        convert0: t.convert_us,
        encode0: t.encode_us,
        captured0: captured,
    };
    if let Some(r) = p.stalls.drain_report() {
        log::info!("{r}");
    }
}

// ---------------------------------------------------------------- teardown + carry-over

fn teardown(shared: &Arc<Shared>, keep_carry: bool) {
    let (session, ring, was_active, was_game, tname, texe) = {
        let mut st = lock(&shared.state);
        let s = st.session.take();
        let r = st.ring.take();
        let was_active = st.target_active;
        let info = (st.target_is_game, std::mem::take(&mut st.target_name), st.target_exe.take());
        st.locked_hwnd = 0;
        st.locked_pid = 0;
        st.lock_kind = LockKind::None;
        st.target_active = false;
        st.target_is_game = false;
        (s, r, was_active, info.0, info.1, info.2)
    };
    shared.clock.restart(); // capture gone: clear any pause state
    if was_active {
        (shared.events)(EngineEvent::TargetChanged);
    }
    let Some(mut session) = session else {
        if !keep_carry {
            lock(&shared.state).carry = None;
        }
        return;
    };
    let t = Instant::now();
    log::info!("[Teardown] begin");
    session.capture.stop(); // no more frames after this
    let (spec, tail) = {
        let mut p = lock(&session.pipeline);
        let tail = p.encoder.flush();
        log::info!("[Teardown] encoder flushed @ {}ms", t.elapsed().as_millis());
        (p.encoder.stream_spec(), tail)
    };
    if let Some(r) = &ring {
        for pk in tail {
            r.add(pk);
        }
    }
    let audio = session.audio.take();
    drop(session); // encoder + capture released
    log::info!("[Teardown] encoder disposed @ {}ms", t.elapsed().as_millis());
    // Stop WASAPI + the pump thread FIRST (joined: no pump can touch an encoder being freed - a C# race),
    // then publish the delayed AAC packets into the rings and keep them for the carry-over.
    let carry_audio: Vec<CarryAudioTrack> = match audio {
        Some(mut a) => {
            if let Err(e) = a.session.finish() {
                log::info!("[Teardown] audio finish: {e}");
            }
            log::info!("[Teardown] audio disposed @ {}ms", t.elapsed().as_millis());
            a.tracks_with(|r| r.snapshot_absolute())
                .into_iter()
                .map(|t| CarryAudioTrack { spec: t.spec, packets: t.packets, title: t.title })
                .collect()
        }
        None => Vec::new(),
    };
    if keep_carry {
        capture_carry_over(shared, ring.as_deref(), spec, carry_audio, was_game, tname, texe);
    } else {
        lock(&shared.state).carry = None;
    }
    log::info!("[Teardown] complete @ {}ms", t.elapsed().as_millis());
    unsafe {
        let _ = SetPriorityClass(GetCurrentProcess(), NORMAL_PRIORITY_CLASS);
    }
}

fn capture_carry_over(
    shared: &Arc<Shared>,
    ring: Option<&PacketRing>,
    video_spec: StreamSpec,
    audio: Vec<CarryAudioTrack>,
    was_game: bool,
    target_name: String,
    target_exe: Option<String>,
) {
    let Some(ring) = ring else { return };
    let video = ring.snapshot_absolute();
    if video.is_empty() {
        return;
    }
    let buffer = lock(&shared.state).options.buffer_seconds;
    let seg = CarryOverSegment { video_spec, video, audio, was_game, target_name, target_exe };
    let seg = match seg.trimmed_to_last(buffer) {
        Trimmed::Same => Some(seg),
        Trimmed::New(s) => Some(s),
        Trimmed::Gone => None,
    };
    if let Some(s) = &seg {
        log::info!(
            "[CarryOver] holding {:.1}s of {} @ {}x{} \u{2014} still saveable.",
            s.duration_seconds(),
            if s.was_game { format!("game '{}'", s.target_name) } else { "desktop".into() },
            s.video_spec.width,
            s.video_spec.height
        );
    }
    lock(&shared.state).carry = seg;
}

fn trim_carry_over(shared: &Arc<Shared>) {
    let mut st = lock(&shared.state);
    let Some(carry) = st.carry.as_ref() else { return };
    let live = st.ring.as_ref().map_or(0.0, |r| r.buffered_seconds());
    let room = st.options.buffer_seconds - live;
    let trimmed = if room > 0.5 { carry.trimmed_to_last(room) } else { Trimmed::Gone };
    match trimmed {
        Trimmed::Same => {}
        Trimmed::New(s) => st.carry = Some(s),
        Trimmed::Gone => {
            st.carry = None;
            log::info!("[CarryOver] released \u{2014} the live buffer now covers the full window.");
        }
    }
}

// ---------------------------------------------------------------- save

fn new_clip_path(library: &str, format: &str) -> String {
    let ext = match format.trim().to_ascii_lowercase().as_str() {
        "mkv" => "mkv",
        "mov" => "mov",
        _ => "mp4",
    };
    let _ = std::fs::create_dir_all(library);
    let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
    let name = format!(
        "{:04}-{:02}-{:02}_{:02}-{:02}-{:02}-Replay.{ext}",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    );
    std::path::Path::new(library).join(name).to_string_lossy().into_owned()
}

fn save_replay(shared: &Arc<Shared>) -> Option<String> {
    let (ring, pipeline, carry, opts, choice) = {
        let st = lock(&shared.state);
        (
            st.ring.clone(),
            st.session.as_ref().map(|s| s.pipeline.clone()),
            st.carry.clone(),
            st.options.clone(),
            st.encoder_choice.clone(),
        )
    };
    let mut live_audio: Vec<OwnedAudioTrack> = Vec::new();
    let mut live_video = Vec::new();
    let mut live_spec = None;
    if let (Some(ring), Some(pipeline)) = (&ring, &pipeline) {
        // Audio: pump silence up to "now", then start the clip where the audio starts (C# minStart).
        let st = lock(&shared.state);
        let audio = st.session.as_ref().and_then(|s| s.audio.as_ref());
        if let Some(a) = audio {
            a.session.pump();
        }
        let min_start = audio.and_then(|a| a.rings.iter().filter_map(|r| r.oldest_pts_us()).max());
        let (video, base) = ring.snapshot(min_start);
        if !video.is_empty() {
            if let Some(a) = audio {
                live_audio = a.tracks_with(|r| r.snapshot_rebased_from(base));
            }
            drop(st);
            live_spec = Some(lock(pipeline).encoder.stream_spec());
            live_video = video;
        }
    }
    if live_spec.is_none() && carry.is_none() {
        log::info!("[VfrReplayEngine] SaveReplay: no active capture and nothing carried over.");
        return None;
    }
    let path = new_clip_path(&opts.library_path, &opts.file_format);
    let mut tag = None;
    let ok = match carry {
        None => mux::write(&path, live_spec.as_ref()?, &live_video, &carry::as_mux_tracks(&live_audio)),
        Some(carry) => {
            let head_video = carry.video_rebased();
            let head_audio: Vec<OwnedAudioTrack> = carry
                .audio
                .iter()
                .map(|t| OwnedAudioTrack {
                    spec: t.spec.clone(),
                    packets: carry.audio_rebased(t),
                    title: t.title.clone(),
                })
                .filter(|t| !t.packets.is_empty())
                .collect();
            if carry.was_game {
                tag = Some((carry.target_name.clone(), carry.target_exe.clone()));
            }
            match &live_spec {
                Some(live) if !head_video.is_empty() => {
                    let offset = head_video.last().map_or(0, |p| p.pts_us) + carry::SEAM_GAP_US;
                    log::info!(
                        "[VfrReplayEngine] stitching {:.1}s carried ({}x{}) + live ({}x{}).",
                        carry.duration_seconds(),
                        carry.video_spec.width,
                        carry.video_spec.height,
                        live.width,
                        live.height
                    );
                    if carry::can_concat_direct(&carry.video_spec, live) {
                        let mut video = head_video;
                        video.extend(carry::shift(&live_video, offset));
                        let merged = carry::merge_audio(&head_audio, &live_audio, offset);
                        mux::write(&path, &carry.video_spec, &video, &carry::as_mux_tracks(&merged))
                    } else {
                        let enc_name = choice.as_ref().map_or("libx264", |c| c.ffmpeg_name);
                        let ok = carry::transcode_stitch(
                            &path,
                            &carry.video_spec,
                            &head_video,
                            &head_audio,
                            live,
                            &live_video,
                            &live_audio,
                            opts.bitrate_kbps,
                            opts.preset,
                            enc_name,
                        );
                        if ok {
                            true
                        } else {
                            log::info!("[VfrReplayEngine] stitch failed - saving the halves separately.");
                            let ok = mux::write(&path, live, &live_video, &carry::as_mux_tracks(&live_audio));
                            let p = std::path::Path::new(&path);
                            let head_path = p
                                .with_file_name(format!(
                                    "{}-Game.{}",
                                    p.file_stem().map(|s| s.to_string_lossy()).unwrap_or_default(),
                                    p.extension().map(|s| s.to_string_lossy()).unwrap_or_default()
                                ))
                                .to_string_lossy()
                                .into_owned();
                            if mux::write(
                                &head_path,
                                &carry.video_spec,
                                &head_video,
                                &carry::as_mux_tracks(&head_audio),
                            ) {
                                (shared.events)(EngineEvent::ReplaySaved(head_path));
                            }
                            ok
                        }
                    }
                }
                _ => {
                    !head_video.is_empty()
                        && mux::write(&path, &carry.video_spec, &head_video, &carry::as_mux_tracks(&head_audio))
                }
            }
        }
    };
    lock(&shared.state).last_saved_game_tag = tag;
    if !ok {
        return None;
    }
    (shared.events)(EngineEvent::ReplaySaved(path.clone()));
    Some(path)
}

// ---------------------------------------------------------------- Win32 helpers

fn is_window(hwnd: isize) -> bool {
    hwnd != 0 && unsafe { IsWindow(Some(HWND(hwnd as *mut _))) }.as_bool()
}

fn window_pid(hwnd: isize) -> u32 {
    let mut pid = 0u32;
    unsafe { GetWindowThreadProcessId(HWND(hwnd as *mut _), Some(&mut pid)) };
    pid
}

fn is_process_alive(pid: u32) -> bool {
    if pid == 0 {
        return false;
    }
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return false };
        let mut code = 0u32;
        let ok = GetExitCodeProcess(h, &mut code).is_ok() && code == STILL_ACTIVE.0 as u32;
        let _ = CloseHandle(h);
        ok
    }
}

/// `<ProcessName>.exe` like C# `Process.ProcessName + ".exe"`.
pub(crate) fn process_exe_name(pid: u32) -> Option<String> {
    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let r = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        r.ok()?;
        let full = String::from_utf16_lossy(&buf[..len as usize]);
        let file = full.rsplit(['\\', '/']).next()?.to_string();
        let stem = file.strip_suffix(".exe").or_else(|| file.strip_suffix(".EXE")).unwrap_or(&file);
        Some(format!("{stem}.exe"))
    }
}

/// HMONITOR + size + device name for `device` (`\\.\DISPLAYn`), or the primary monitor when None/empty.
fn resolve_monitor(device: Option<&str>) -> Option<(isize, i32, i32, String)> {
    struct Ctx<'a> {
        want: Option<&'a str>,
        found: Option<(isize, i32, i32, String)>,
    }
    unsafe extern "system" fn cb(h: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
        let ctx = unsafe { &mut *(data.0 as *mut Ctx) };
        if ctx.found.is_some() {
            return true.into();
        }
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(h, &mut info.monitorInfo as *mut _ as *mut _) }.as_bool() {
            let len = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
            let name = String::from_utf16_lossy(&info.szDevice[..len]);
            let primary = info.monitorInfo.dwFlags & 1 != 0;
            let matched = match ctx.want {
                None | Some("") => primary,
                Some(w) => name.eq_ignore_ascii_case(w),
            };
            if matched {
                let m = info.monitorInfo.rcMonitor;
                ctx.found = Some((h.0 as isize, m.right - m.left, m.bottom - m.top, name));
            }
        }
        true.into()
    }
    let mut ctx = Ctx { want: device, found: None };
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut ctx as *mut Ctx as isize));
    }
    ctx.found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_dims_preserve_aspect_and_even() {
        assert_eq!(output_dims(1080, 3440, 1440), (2580, 1080)); // Medal's ultrawide sweet spot
        assert_eq!(output_dims(0, 1921, 1081), (1920, 1080));
        assert_eq!(output_dims(2160, 1920, 1080), (1920, 1080)); // never upscale
    }

    #[test]
    fn primary_monitor_resolves() {
        let (h, w, hgt, name) = resolve_monitor(None).expect("primary monitor");
        assert!(h != 0 && w > 0 && hgt > 0 && name.starts_with("\\\\.\\DISPLAY"), "{name}");
    }

    #[test]
    fn exe_name_of_self() {
        let n = process_exe_name(std::process::id()).unwrap();
        assert!(n.ends_with(".exe"), "{n}");
        assert!(is_process_alive(std::process::id()));
        assert!(!is_process_alive(0));
    }

    #[test]
    fn clock_pauses() {
        let c = Clock::new();
        std::thread::sleep(Duration::from_millis(20));
        assert!(c.set_paused(true).is_some());
        let a = c.now_us();
        std::thread::sleep(Duration::from_millis(30));
        assert!((c.now_us() - a).abs() < 2000, "clock must not advance while paused");
        assert!(c.set_paused(true).is_none());
        c.set_paused(false);
        assert!(c.now_us() >= a);
    }
}
