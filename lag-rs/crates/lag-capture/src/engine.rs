//! Port of `VfrReplayEngine.cs` (+ `VfrEngineOptions`): the orchestrator. Locks onto a game window (or the
//! desktop), runs capture -> cadence grid -> overlay -> encoder -> ring, the audio tracks, carry-over on
//! session end, pause/resume, and saving.
//!
//! PUBLIC API IS STABLE (the app codes against it); the implementation is filled in incrementally.

use std::sync::Arc;

/// `CaptureMode` (VfrReplayEngine.cs).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CaptureMode {
    /// Auto-detect a game; fall back to the desktop.
    #[default]
    AutoGame,
    /// Always record the display.
    Display,
}

/// `VfrEngineOptions`, field for field (snake_case). Defaults equal the C# initialisers.
#[derive(Debug, Clone)]
pub struct EngineOptions {
    pub library_path: String,
    pub buffer_seconds: f64,
    /// "", "av1", "hevc", "h264", "nvenc", "amf", "qsv", "x264"/"cpu".
    pub preferred_codec: Option<String>,
    /// <= 0 = Auto (constant quality).
    pub bitrate_kbps: i32,
    /// mp4 / mkv / mov.
    pub file_format: String,
    /// 0 = native height.
    pub target_height: i32,
    pub max_fps: i32,
    pub preset: i32,
    pub capture_cursor: bool,
    /// Recorded monitor device name (`\\.\DISPLAY1`); None = primary.
    pub monitor_device_id: Option<String>,
    pub mode: CaptureMode,
    /// Game capture mode. false = "lock game": record the game window even while the user Alt+Tabs away
    /// (the C# behaviour, default). true = "follow screen": while the game is not in front, record the
    /// monitor instead; switching back and forth loses no frames. Either way, when the game exits the
    /// recording continues on the desktop in the same buffer.
    pub follow_focus: bool,
    pub adapter_index: i32,
    /// None = default mic, Some("") = no mic, else endpoint id.
    pub microphone_id: Option<String>,
    pub mic_volume: f32,
    pub mic_start_muted: bool,
    pub separate_audio_tracks: bool,
    pub apps_audio_mode: bool,
    pub audio_mode_by_target: bool,
    pub audio_apps: Vec<(String, f32)>,
    pub system_audio_enabled: bool,
    pub system_audio_device_id: Option<String>,
    pub system_volume: f32,
    pub mic_mono: bool,
    pub input_gate_auto: bool,
    pub input_gate_threshold: f32,
    pub noise_suppression: bool,
    pub game_audio_enabled: bool,
    pub game_volume: f32,
    pub audio_enabled: bool,
    /// Capture exactly this window (testing/manual), bypassing detection.
    pub forced_window: isize,
    pub webcam_overlay: bool,
    pub webcam_device_id: Option<String>,
    pub webcam_x: f64,
    pub webcam_y: f64,
    pub webcam_scale: f64,
    /// (width, height, fps) requested from the camera; zeros = Auto.
    pub webcam_format: (u32, u32, u32),
    pub keys_overlay: bool,
    pub keys_x: f64,
    pub keys_y: f64,
    pub keys_scale: f64,
    pub stats_overlay: bool,
    pub stats_x: f64,
    pub stats_y: f64,
    pub stats_scale: f64,
    pub stats_detail: i32,
}

impl Default for EngineOptions {
    fn default() -> Self {
        Self {
            library_path: String::new(),
            buffer_seconds: 120.0,
            preferred_codec: None,
            bitrate_kbps: 30_000,
            file_format: "mp4".into(),
            target_height: 0,
            max_fps: 120,
            preset: 4,
            capture_cursor: true,
            monitor_device_id: None,
            mode: CaptureMode::AutoGame,
            follow_focus: false,
            adapter_index: -1,
            microphone_id: None,
            mic_volume: 1.0,
            mic_start_muted: false,
            separate_audio_tracks: false,
            apps_audio_mode: false,
            audio_mode_by_target: false,
            audio_apps: Vec::new(),
            system_audio_enabled: true,
            system_audio_device_id: None,
            system_volume: 1.0,
            mic_mono: false,
            input_gate_auto: true,
            input_gate_threshold: 0.08,
            noise_suppression: false,
            game_audio_enabled: true,
            game_volume: 1.0,
            audio_enabled: true,
            forced_window: 0,
            webcam_overlay: false,
            webcam_device_id: None,
            webcam_x: 0.97,
            webcam_y: 0.95,
            webcam_scale: 0.24,
            webcam_format: (0, 0, 0),
            keys_overlay: false,
            keys_x: 0.03,
            keys_y: 0.95,
            keys_scale: 0.20,
            stats_overlay: false,
            stats_x: 0.5,
            stats_y: 0.02,
            stats_scale: 0.045,
            stats_detail: 1,
        }
    }
}

/// Snapshot of what the engine is doing (polled by the UI's buffer ticker).
#[derive(Debug, Clone, Default)]
pub struct EngineStatus {
    pub running: bool,
    pub paused: bool,
    pub target_active: bool,
    pub target_is_game: bool,
    pub target_name: String,
    pub target_exe: Option<String>,
    pub buffered_seconds: f64,
    pub frames_captured: u64,
    pub frames_encoded: u64,
    pub friendly_encoder: String,
    /// (convert ms, encode ms) per frame, averaged over the session.
    pub avg_stage_ms: (f64, f64),
}

/// Engine events, delivered on engine threads (the app forwards them to the UI thread).
#[derive(Debug, Clone)]
pub enum EngineEvent {
    /// A clip was written (`ReplaySaved`). Also fired for the "-Game" half of a failed stitch.
    ReplaySaved(String),
    /// Lock target changed (`TargetChanged`); read `status()` for details.
    TargetChanged,
}

pub type EventSink = Arc<dyn Fn(EngineEvent) + Send + Sync>;

/// The replay engine. Cheap to share (`Arc<ReplayEngine>`); every method is callable from any thread and
/// none of them blocks for long except `save_replay` (muxing) and `stop` (teardown).
pub struct ReplayEngine {
    inner: crate::engine_impl::Engine,
}

impl ReplayEngine {
    /// True when WGC, FFmpeg and at least one encoder are usable (`VfrReplayEngine.IsAvailable`).
    pub fn is_available() -> bool {
        crate::wgc::WgcCapture::is_supported() && !crate::encoder::available().is_empty()
    }

    pub fn new(events: EventSink) -> Self {
        Self { inner: crate::engine_impl::Engine::new(events) }
    }

    /// Arms the engine with `options` and starts the lock timer (`Start`). Errors if no encoder exists.
    pub fn start(&self, options: EngineOptions) -> Result<(), String> {
        self.inner.start(options)
    }

    /// Stops capture and the lock timer; drops any carry-over (`Stop`).
    pub fn stop(&self) {
        self.inner.stop()
    }

    /// Writes the replay; returns the clip path (`SaveReplay`).
    pub fn save_replay(&self) -> Option<String> {
        self.inner.save_replay()
    }

    pub fn set_paused(&self, paused: bool) {
        self.inner.set_paused(paused)
    }

    pub fn set_mic_muted(&self, muted: bool) {
        self.inner.set_mic_muted(muted)
    }

    pub fn status(&self) -> EngineStatus {
        self.inner.status()
    }

    /// Frames captured so far (one atomic read, no lock): the stats HUD's FPS counter
    /// (C#: `VfrRecorderAdapter.FramesCaptured`).
    pub fn frames_captured(&self) -> u64 {
        self.inner.frames_captured()
    }

    /// `(game name, exe)` of the game the last saved clip came from, if it was a game.
    pub fn last_saved_game_tag(&self) -> Option<(String, Option<String>)> {
        self.inner.last_saved_game_tag()
    }
}

impl Drop for ReplayEngine {
    fn drop(&mut self) {
        self.inner.stop();
    }
}
