//! Mirrors Services/VfrCapture/VfrRecorderAdapter.cs and MainViewModel.cs:462-564.
//! Settings restoration mirrors SettingsViewModel.cs (including first-device fallbacks).
use lag_capture::engine::{EngineEvent, EngineOptions, EngineStatus, ReplayEngine};
use lag_core::{
    hardware::{self, MonitorInfo},
    metadata::{ClipMetadata, ClipMetadataStore},
    presets::BUFFER_RUNGS,
    recorder_options::{AppAudioCapture, RecorderOptions},
    settings::SettingsData,
};
use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
};

#[derive(Default)]
pub struct Devices {
    pub monitors: Vec<MonitorInfo>,
    pub microphones: Vec<String>,
    pub outputs: Vec<String>,
    pub webcams: Vec<String>,
}
impl Devices {
    pub fn enumerate() -> Self {
        Self {
            monitors: hardware::get_available_monitors(),
            microphones: hardware::get_microphones().into_iter().map(|d| d.id).collect(),
            // C# prepends its synthetic Default output entry before the physical endpoints.
            outputs: std::iter::once(String::new())
                .chain(hardware::get_output_devices().into_iter().map(|d| d.id))
                .collect(),
            webcams: lag_capture::overlay::webcam::WebcamCaptureSource::list_devices()
                .into_iter()
                .map(|d| d.id)
                .collect(),
        }
    }
}
/// Saved mic/webcam id -> engine id. Empty, unset or not connected = None = the Windows default device
/// (owner decision 2026-11-16; C# fell back to the first enumerated device instead).
fn default_or_saved(saved: Option<&str>, available: &[String]) -> Option<String> {
    saved.filter(|id| !id.is_empty() && available.iter().any(|d| d == id)).map(str::to_owned)
}
fn device(saved: Option<&str>, available: &[String]) -> Option<String> {
    saved.and_then(|id| available.iter().find(|d| d.as_str() == id)).or_else(|| available.first()).cloned()
}
pub fn selected_monitor<'a>(s: &SettingsData, d: &'a Devices) -> Option<&'a MonitorInfo> {
    s.monitor_stable_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .and_then(|id| d.monitors.iter().find(|m| m.stable_id == id))
        .or_else(|| {
            s.monitor_device_name
                .as_deref()
                .filter(|id| !id.is_empty())
                .and_then(|id| d.monitors.iter().find(|m| m.device_name == id))
        })
        .or_else(|| d.monitors.first())
}
pub fn buffer_seconds(s: &SettingsData) -> i32 {
    let seconds = if s.buffer_seconds > 0 { s.buffer_seconds } else { s.buffer_minutes.saturating_mul(60) };
    if BUFFER_RUNGS.contains(&seconds) {
        seconds
    } else {
        300
    }
}
pub fn library_path(s: &SettingsData) -> String {
    s.library_path.clone().unwrap_or_else(|| {
        std::env::var_os("USERPROFILE")
            .map(std::path::PathBuf::from)
            .unwrap_or_default()
            .join("Videos/Lag")
            .to_string_lossy()
            .into_owned()
    })
}
/// Pure snapshot conversion; probing is deliberately separate so it is testable without capture.
pub fn settings_to_options(s: &SettingsData, d: &Devices) -> Result<EngineOptions, String> {
    let m = selected_monitor(s, d).ok_or_else(|| lag_core::i18n::tr("Status_NoMonitor").to_owned())?;
    let hz = m.refresh_rate as i32;
    let cap = if hz < 24 {
        360
    } else {
        [60, 75, 90, 100, 120, 144, 160, 165, 180, 200, 240, 280, 300, 360, 390, 480, 500]
            .into_iter()
            .find(|known| (hz - known).abs() <= 2)
            .unwrap_or(hz)
            .max(60)
    };
    let target = s.output_resolution_height;
    let output_height =
        if [720, 1080, 1440, 2160].contains(&target) && target < m.height as i32 { (target as u32) & !1 } else { 0 };
    let o = RecorderOptions {
        buffer_seconds: buffer_seconds(s),
        frame_rate: if s.frame_rate > 0 { s.frame_rate.clamp(1, cap) } else { 30 },
        width: m.width,
        height: m.height,
        output_height,
        output_width: if output_height > 0 && m.height > 0 {
            ((m.width as f64 * output_height as f64 / m.height as f64).round_ties_even() as u32) & !1
        } else {
            0
        },
        library_path: Some(library_path(s)),
        file_format: s.file_format.as_deref().filter(|s| ["mp4", "mkv", "mov"].contains(s)).unwrap_or("mp4").into(),
        monitor_id: Some(m.device_name.clone()),
        adapter_index: s.gpu_index.max(0),
        preferred_encoder: s.codec_name.clone(),
        video_bitrate_kbps: if s.bitrate_kbps <= 0 { -1 } else { s.bitrate_kbps },
        microphone_id: if s.mic_enabled {
            default_or_saved(s.microphone_id.as_deref(), &d.microphones)
        } else {
            Some(String::new())
        },
        mic_volume: s.mic_volume.clamp(0, 200) as f32 / 100.,
        mic_force_mono: s.mic_mono,
        mic_start_muted: s.ptt_enabled,
        input_gate_auto: s.input_sensitivity_auto,
        input_gate_threshold: s.input_sensitivity_threshold.clamp(0, 100) as f32 / 100.,
        noise_suppression: s.noise_suppression,
        system_audio_device_id: device(s.system_audio_device_id.as_deref(), &d.outputs).or(Some(String::new())),
        system_audio_enabled: s.system_audio_enabled,
        system_audio_volume: s.system_audio_volume,
        game_audio_enabled: s.game_audio_enabled,
        game_audio_volume: s.game_audio_volume,
        audio_capture_mode: s.audio_capture_mode.clone().unwrap_or_else(|| "all".into()),
        audio_mode_by_target: s.audio_mode_by_target,
        audio_apps: s
            .audio_apps
            .iter()
            .flatten()
            .filter(|a| a.enabled)
            .map(|a| AppAudioCapture { exe_name: a.exe.clone().unwrap_or_default(), volume: a.volume as f32 / 100. })
            .collect(),
        separate_audio_tracks: s.separate_audio_tracks,
        record_desktop_when_unfocused: s.game_focus_mode.as_deref() == Some("desktop"),
        webcam_overlay: s.overlay_enabled && s.webcam_shown,
        webcam_device_id: default_or_saved(s.webcam_device_id.as_deref(), &d.webcams),
        webcam_x: s.webcam_x.clamp(0., 1.),
        webcam_y: s.webcam_y.clamp(0., 1.),
        webcam_scale: s.webcam_scale.clamp(0.06, 0.45),
        webcam_format: lag_core::presets::webcam_mode(s.webcam_mode),
        keys_overlay: s.overlay_enabled && s.keys_shown,
        keys_x: s.keys_x.clamp(0., 1.),
        keys_y: s.keys_y.clamp(0., 1.),
        keys_scale: s.keys_scale.clamp(0.06, 0.42),
        // Stats are the live HUD, not baked into recordings (MainViewModel.cs:540-541).
        ..RecorderOptions::default()
    };
    Ok(map_options(&o))
}
fn codec(id: Option<&str>) -> Option<String> {
    let id = id?.to_ascii_lowercase();
    if id.contains("av1") {
        Some("av1")
    } else if id.contains("hevc") || id.contains("265") {
        Some("hevc")
    } else if id.contains("264") {
        Some("h264")
    } else if id.contains("cpu") {
        Some("x264")
    } else {
        None
    }
    .map(str::to_owned)
}
pub fn map_options(o: &RecorderOptions) -> EngineOptions {
    EngineOptions {
        library_path: o.library_path.clone().unwrap_or_default(),
        buffer_seconds: o.buffer_seconds as f64,
        preferred_codec: codec(o.preferred_encoder.as_deref()),
        bitrate_kbps: o.video_bitrate_kbps,
        file_format: if o.file_format.trim().is_empty() { "mp4".into() } else { o.file_format.clone() },
        target_height: o.output_height as i32,
        max_fps: if o.frame_rate > 0 { o.frame_rate } else { 120 },
        monitor_device_id: o.monitor_id.clone(),
        adapter_index: o.adapter_index,
        microphone_id: o.microphone_id.clone(),
        mic_volume: o.mic_volume,
        mic_start_muted: o.mic_start_muted,
        separate_audio_tracks: o.separate_audio_tracks,
        apps_audio_mode: o.audio_capture_mode.eq_ignore_ascii_case("apps"),
        audio_mode_by_target: o.audio_mode_by_target,
        audio_apps: o.audio_apps.iter().map(|a| (a.exe_name.clone(), a.volume)).collect(),
        system_audio_enabled: o.system_audio_enabled,
        system_audio_device_id: o.system_audio_device_id.clone(),
        system_volume: o.system_audio_volume as f32 / 100.,
        game_audio_enabled: o.game_audio_enabled,
        game_volume: o.game_audio_volume as f32 / 100.,
        mic_mono: o.mic_force_mono,
        input_gate_auto: o.input_gate_auto,
        input_gate_threshold: o.input_gate_threshold,
        noise_suppression: o.noise_suppression,
        capture_cursor: true,
        follow_focus: o.record_desktop_when_unfocused,
        webcam_overlay: o.webcam_overlay,
        webcam_device_id: o.webcam_device_id.clone(),
        webcam_x: o.webcam_x,
        webcam_y: o.webcam_y,
        webcam_scale: o.webcam_scale,
        webcam_format: o.webcam_format,
        keys_overlay: o.keys_overlay,
        keys_x: o.keys_x,
        keys_y: o.keys_y,
        keys_scale: o.keys_scale,
        stats_overlay: o.stats_overlay,
        stats_x: o.stats_x,
        stats_y: o.stats_y,
        stats_scale: o.stats_scale,
        stats_detail: o.stats_detail,
        ..EngineOptions::default()
    }
}

#[derive(Clone)]
pub enum Event {
    Started(Result<i32, String>),
    Stopped,
    Status(EngineStatus),
    Target(EngineStatus),
    ReplaySaved(String),
}
type Listener = Rc<dyn Fn(Event)>;
thread_local! { static LISTENERS: RefCell<HashMap<u64, Listener>> = RefCell::default(); }
fn deliver(id: u64, event: Event) {
    let _ = slint::invoke_from_event_loop(move || {
        let listener = LISTENERS.with(|l| l.borrow().get(&id).cloned());
        if let Some(listener) = listener {
            listener(event);
        }
    });
}
enum Command {
    Start(Box<SettingsData>),
    Stop,
    Pause(bool),
    Mic(bool),
    Poll,
    Shutdown(mpsc::Sender<()>),
}
pub struct RecorderService {
    commands: mpsc::Sender<Command>,
    engine: Arc<ReplayEngine>,
    id: u64,
}
impl RecorderService {
    pub fn new(listener: impl Fn(Event) + 'static) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        LISTENERS.with(|l| l.borrow_mut().insert(id, Rc::new(listener)));
        let (events_tx, events) = mpsc::channel();
        let engine = Arc::new(ReplayEngine::new(Arc::new(move |event| {
            let _ = events_tx.send(event);
        })));
        let weak = Arc::downgrade(&engine);
        std::thread::spawn(move || {
            while let Ok(event) = events.recv() {
                let Some(engine) = weak.upgrade() else {
                    break;
                };
                match event {
                    EngineEvent::TargetChanged => deliver(id, Event::Target(engine.status())),
                    EngineEvent::ReplaySaved(path) => {
                        let target = engine.status();
                        let carried = engine.last_saved_game_tag();
                        let metadata = ClipMetadata {
                            game: carried
                                .as_ref()
                                .map(|t| t.0.clone())
                                .or_else(|| target.target_is_game.then_some(target.target_name))
                                .map(|g| lag_core::metadata::normalize_game_name(&g)),
                            exe: carried.and_then(|t| t.1).or(target.target_exe),
                            ..ClipMetadata::default()
                        };
                        if let Some(store) = ClipMetadataStore::for_user() {
                            if let Err(e) = store.try_write(&path, &metadata) {
                                log::warn!("[ClipMetadata] write failed: {e}");
                            }
                        }
                        deliver(id, Event::ReplaySaved(path));
                    }
                }
            }
        });
        let (commands, rx) = mpsc::channel();
        let worker_engine = engine.clone();
        std::thread::spawn(move || {
            while let Ok(command) = rx.recv() {
                match command {
                    Command::Start(settings) => {
                        let result = settings_to_options(&settings, &Devices::enumerate()).and_then(|options| {
                            let seconds = options.buffer_seconds as i32;
                            worker_engine.start(options).map(|_| seconds)
                        });
                        deliver(id, Event::Started(result));
                    }
                    Command::Stop => {
                        worker_engine.stop();
                        deliver(id, Event::Stopped);
                    }
                    Command::Pause(paused) => worker_engine.set_paused(paused),
                    Command::Mic(muted) => worker_engine.set_mic_muted(muted),
                    Command::Poll => deliver(id, Event::Status(worker_engine.status())),
                    Command::Shutdown(done) => {
                        worker_engine.stop();
                        let _ = done.send(());
                        return;
                    }
                }
            }
            worker_engine.stop();
        });
        Self { commands, engine, id }
    }
    pub fn start(&self, settings: SettingsData) {
        let _ = self.commands.send(Command::Start(Box::new(settings)));
    }
    pub fn stop(&self) {
        let _ = self.commands.send(Command::Stop);
    }
    /// App exit: stop the engine (capture, NVENC, audio) and WAIT for it, bounded. Exiting with the encoder
    /// running let Windows kill its threads mid-driver-call; the process then hung half-dead holding the
    /// WGC capture (yellow screen border) — 2026-11-16. C# disposed its services before Environment.Exit too.
    pub fn shutdown_blocking(&self, timeout: std::time::Duration) -> bool {
        let (done, wait) = mpsc::channel();
        if self.commands.send(Command::Shutdown(done)).is_err() {
            return true;
        }
        let ok = wait.recv_timeout(timeout).is_ok();
        if !ok {
            log::warn!("[Recorder] engine did not stop within {} ms at exit", timeout.as_millis());
        }
        ok
    }
    pub fn pause(&self, value: bool) {
        let _ = self.commands.send(Command::Pause(value));
    }
    pub fn mute_mic(&self, value: bool) {
        let _ = self.commands.send(Command::Mic(value));
    }
    pub fn poll(&self) {
        let _ = self.commands.send(Command::Poll);
    }
    /// Live engine frame counter for the stats HUD (no queue, no state lock).
    pub fn frame_counter(&self) -> impl Fn() -> u64 + 'static {
        let engine = self.engine.clone();
        move || engine.frames_captured()
    }
    pub fn save(&self) {
        let engine = self.engine.clone();
        // Saving must not hold up stop/pause/PTT/status. ReplaySaved is authoritative, even for
        // partial stitch outputs; a missing event is handled by MainVM's 120-second watchdog.
        std::thread::spawn(move || {
            engine.save_replay();
        });
    }
}
impl Drop for RecorderService {
    fn drop(&mut self) {
        LISTENERS.with(|l| l.borrow_mut().remove(&self.id));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn devices() -> Devices {
        Devices {
            monitors: vec![MonitorInfo {
                index: 0,
                device_name: "display1".into(),
                stable_id: "stable1".into(),
                width: 2560,
                height: 1440,
                refresh_rate: 165,
                is_primary: true,
            }],
            microphones: vec!["first-mic".into(), "second-mic".into()],
            outputs: vec!["".into(), "speakers".into()],
            webcams: vec!["camera".into()],
        }
    }
    #[test]
    fn missing_devices_fall_back_to_windows_default() {
        let s = SettingsData {
            microphone_id: Some("unplugged".into()),
            system_audio_device_id: Some("unplugged".into()),
            monitor_device_name: Some("unplugged".into()),
            webcam_device_id: Some("unplugged".into()),
            ..SettingsData::default()
        };
        let o = settings_to_options(&s, &devices()).unwrap();
        // None = the Windows default device (Some("") would mean "no mic").
        assert_eq!(o.microphone_id, None);
        assert_eq!(o.system_audio_device_id.as_deref(), Some(""));
        assert_eq!(o.monitor_device_id.as_deref(), Some("display1"));
        assert_eq!(o.webcam_device_id, None);
        // A connected saved device is used as is; "" (the Default item) is the Windows default.
        let s = SettingsData {
            microphone_id: Some("second-mic".into()),
            webcam_device_id: Some("".into()),
            ..SettingsData::default()
        };
        let o = settings_to_options(&s, &devices()).unwrap();
        assert_eq!(o.microphone_id.as_deref(), Some("second-mic"));
        assert_eq!(o.webcam_device_id, None);
    }
    #[test]
    fn mapping_preserves_gains_and_capture_settings() {
        let s = SettingsData {
            buffer_seconds: 120,
            frame_rate: 240,
            output_resolution_height: 1080,
            codec_name: Some("hevc_nvenc".into()),
            mic_volume: 175,
            system_audio_volume: 37,
            game_audio_volume: 88,
            ptt_enabled: true,
            mic_mono: true,
            input_sensitivity_threshold: 17,
            noise_suppression: true,
            audio_capture_mode: Some("apps".into()),
            audio_mode_by_target: true,
            separate_audio_tracks: true,
            overlay_enabled: true,
            keys_shown: true,
            stats_shown: true,
            game_focus_mode: Some("desktop".into()),
            ..SettingsData::default()
        };
        let o = settings_to_options(&s, &devices()).unwrap();
        assert_eq!((o.buffer_seconds, o.max_fps, o.target_height), (120., 165, 1080));
        assert_eq!((o.mic_volume, o.system_volume, o.game_volume, o.input_gate_threshold), (1.75, 0.37, 0.88, 0.17));
        assert_eq!(o.preferred_codec.as_deref(), Some("hevc"));
        assert!(
            o.follow_focus
                && o.mic_start_muted
                && o.mic_mono
                && o.noise_suppression
                && o.apps_audio_mode
                && o.audio_mode_by_target
                && o.separate_audio_tracks
                && o.keys_overlay
        );
        assert!(!o.stats_overlay);
    }
    #[test]
    fn focus_modes_mic_disable_and_selected_devices() {
        for mode in [None, Some("game"), Some("desktop"), Some("unknown")] {
            let s = SettingsData {
                game_focus_mode: mode.map(str::to_owned),
                mic_enabled: false,
                system_audio_device_id: Some("speakers".into()),
                ..SettingsData::default()
            };
            let o = settings_to_options(&s, &devices()).unwrap();
            assert_eq!(o.follow_focus, mode == Some("desktop"));
            assert_eq!(o.microphone_id.as_deref(), Some(""));
            assert_eq!(o.system_audio_device_id.as_deref(), Some("speakers"));
        }
    }
    #[test]
    fn native_resolution_legacy_buffer_empty_devices_and_codec() {
        let s = SettingsData { output_resolution_height: 2160, buffer_minutes: 2, ..SettingsData::default() };
        let o = settings_to_options(&s, &devices()).unwrap();
        assert_eq!((o.target_height, o.buffer_seconds), (0, 120.));
        assert!(settings_to_options(&s, &Devices::default()).is_err());
        for (input, expected) in [
            ("", None),
            ("av1_amf", Some("av1")),
            ("h265", Some("hevc")),
            ("libx264", Some("h264")),
            ("cpu", Some("x264")),
        ] {
            assert_eq!(codec(Some(input)).as_deref(), expected);
        }
    }
}
