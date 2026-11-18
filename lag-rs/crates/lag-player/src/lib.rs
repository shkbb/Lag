//! Mirrors ViewModels/PlayerViewModel.cs and Services/VlcEngine.cs using libmpv.
//! Construction and synchronous controls belong on a worker, never the UI thread.
pub mod native;
pub mod render;
use lag_ffi::mpv::*;
pub use native::NativeVideoWindow;
pub use render::MpvGlRenderer;
use std::{
    ffi::{CStr, CString},
    fmt,
    ptr::NonNull,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Error(pub String);
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;
fn check(code: i32) -> Result<()> {
    if code >= 0 {
        Ok(())
    } else {
        Err(Error(unsafe { CStr::from_ptr(mpv_error_string(code)) }.to_string_lossy().into_owned()))
    }
}
fn cstr(s: &str) -> Result<CString> {
    CString::new(s).map_err(|_| Error("embedded NUL".into()))
}

#[derive(Debug, Clone, PartialEq)]
pub enum PlayerEvent {
    Position(f64),
    Duration(f64),
    Paused(bool),
    Eof(bool),
    FileLoaded,
    PlaybackRestart,
    Stopped,
    VideoSize(u32, u32),
    AudioTracks(Vec<AudioTrack>),
    Error(String),
}
#[derive(Debug, Clone, PartialEq)]
pub struct AudioTrack {
    /// Audio stream ordinal, independent of title and native track id.
    pub index: usize,
    pub id: i64,
}
pub(crate) struct Handle(NonNull<mpv_handle>);
// libmpv's client API is thread safe. Exactly one event thread calls wait_event.
unsafe impl Send for Handle {}
unsafe impl Sync for Handle {}
impl Handle {
    pub(crate) fn raw(&self) -> *mut mpv_handle {
        self.0.as_ptr()
    }
    fn option(&self, key: &str, value: &str) -> Result<()> {
        let (key, value) = (cstr(key)?, cstr(value)?);
        check(unsafe { mpv_set_option_string(self.raw(), key.as_ptr(), value.as_ptr()) })
    }
    fn property(&self, key: &str) -> Result<String> {
        let key = cstr(key)?;
        unsafe {
            let p = mpv_get_property_string(self.raw(), key.as_ptr());
            if p.is_null() {
                return Err(Error("property unavailable".into()));
            }
            let value = CStr::from_ptr(p).to_string_lossy().into_owned();
            mpv_free(p.cast());
            Ok(value)
        }
    }
}
impl Drop for Handle {
    fn drop(&mut self) {
        // The last render reference may be released by the UI. Shutdown must not wait there.
        let raw = self.raw() as usize;
        let _ = thread::Builder::new().name("Lag.MpvDispose".into()).spawn(move || unsafe {
            mpv_terminate_destroy(raw as *mut mpv_handle);
        });
    }
}
pub struct MpvPlayer {
    pub(crate) handle: Arc<Handle>,
    quit: Arc<AtomicBool>,
    events: Option<thread::JoinHandle<()>>,
}
impl MpvPlayer {
    /// Start engine initialization immediately on a worker. Receiving is optional and must not
    /// block a UI thread; controls and property queries also belong on a worker.
    pub fn prewarm(callback: impl Fn(PlayerEvent) + Send + 'static) -> std::sync::mpsc::Receiver<Result<Self>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = thread::Builder::new().name("Lag.MpvPrewarm".into()).spawn(move || {
            let start = std::time::Instant::now();
            let result = Self::new(callback);
            match &result {
                Ok(_) => log::info!("[MpvEngine] ready in {} ms (background).", start.elapsed().as_millis()),
                Err(e) => log::error!("[MpvEngine] init failed after {} ms: {e}", start.elapsed().as_millis()),
            }
            let _ = tx.send(result);
        });
        rx
    }
    /// Start native-window (`wid`) engine initialization on a worker.
    pub fn prewarm_native(
        wid: isize,
        callback: impl Fn(PlayerEvent) + Send + 'static,
    ) -> std::sync::mpsc::Receiver<Result<Self>> {
        let (tx, rx) = std::sync::mpsc::channel();
        let _ = thread::Builder::new().name("Lag.MpvNativePrewarm".into()).spawn(move || {
            let start = std::time::Instant::now();
            let result = Self::native(wid, callback);
            match &result {
                Ok(_) => log::info!("[MpvEngine] native ready in {} ms (background).", start.elapsed().as_millis()),
                Err(e) => log::error!("[MpvEngine] native init failed after {} ms: {e}", start.elapsed().as_millis()),
            }
            let _ = tx.send(result);
        });
        rx
    }
    pub fn new(callback: impl Fn(PlayerEvent) + Send + 'static) -> Result<Self> {
        Self::create(false, callback)
    }
    /// Never creates a window, a GL context, or an audio output.
    pub fn headless(callback: impl Fn(PlayerEvent) + Send + 'static) -> Result<Self> {
        Self::create(true, callback)
    }
    /// Player v2 native child-window backend (`vo=gpu-next`, `gpu-api=d3d11`, `hwdec=d3d11va`, `wid`).
    pub fn native(wid: isize, callback: impl Fn(PlayerEvent) + Send + 'static) -> Result<Self> {
        if wid == 0 {
            return Err(Error("null wid".into()));
        }
        let mut opts = native_mpv_options(wid);
        // Diagnostics only: LAG_MPV_OPTS="video-sync=audio,profile=default" overrides for live A/B tests.
        if let Ok(extra) = std::env::var("LAG_MPV_OPTS") {
            for (k, v) in extra.split(',').filter_map(|kv| kv.split_once('=')) {
                let (k, v) = (k.trim(), v.trim());
                log::info!("[MpvEngine] LAG_MPV_OPTS override {k}={v}");
                opts.retain(|(ok, _)| *ok != k);
                opts.push((Box::leak(k.to_owned().into_boxed_str()), v.to_owned()));
            }
        }
        Self::create_with_options(&opts, callback)
    }
    /// Arbitrary option set (probes/tests: e.g. `native_mpv_options` with `ao=null`).
    pub fn with_options(options: &[(&str, String)], callback: impl Fn(PlayerEvent) + Send + 'static) -> Result<Self> {
        Self::create_with_options(options, callback)
    }
    fn create(headless: bool, callback: impl Fn(PlayerEvent) + Send + 'static) -> Result<Self> {
        let opts = [
            ("config", "no".to_string()),
            ("vo", if headless { "null" } else { "libmpv" }.to_string()),
            ("ao", if headless { "null" } else { "wasapi" }.to_string()),
            ("hwdec", "d3d11va-copy".to_string()),
            ("keep-open", "yes".to_string()),
            ("idle", "yes".to_string()),
            ("osd-level", "0".to_string()),
            ("osc", "no".to_string()),
            ("input-default-bindings", "no".to_string()),
            ("input-vo-keyboard", "no".to_string()),
            ("input-terminal", "no".to_string()),
            ("terminal", "no".to_string()),
            ("volume", "80".to_string()),
            ("pause", "yes".to_string()),
        ];
        Self::create_with_options(&opts, callback)
    }
    fn create_with_options(
        options: &[(&str, String)],
        callback: impl Fn(PlayerEvent) + Send + 'static,
    ) -> Result<Self> {
        let handle =
            Arc::new(Handle(NonNull::new(unsafe { mpv_create() }).ok_or_else(|| Error("mpv_create failed".into()))?));
        for (key, value) in options {
            handle.option(key, value)?;
        }
        check(unsafe { mpv_initialize(handle.raw()) })?;
        for (id, name, format) in [
            (1, "time-pos", MPV_FORMAT_DOUBLE),
            (2, "duration", MPV_FORMAT_DOUBLE),
            (3, "pause", MPV_FORMAT_FLAG),
            (4, "eof-reached", MPV_FORMAT_FLAG),
        ] {
            check(unsafe { mpv_observe_property(handle.raw(), id, cstr(name)?.as_ptr(), format) })?;
        }
        let quit = Arc::new(AtomicBool::new(false));
        let (h, q) = (handle.clone(), quit.clone());
        let events = thread::Builder::new()
            .name("Lag.MpvEvents".into())
            .spawn(move || {
                while !q.load(Ordering::Acquire) {
                    // Copy event payloads before the next wait_event invalidates them.
                    let event = unsafe { &*mpv_wait_event(h.raw(), 0.1) };
                    let value = unsafe {
                        match event.event_id {
                            MPV_EVENT_PROPERTY_CHANGE if !event.data.is_null() => {
                                let p = &*event.data.cast::<mpv_event_property>();
                                match (event.reply_userdata, p.format) {
                                    (1, MPV_FORMAT_DOUBLE) if !p.data.is_null() => {
                                        Some(PlayerEvent::Position(*p.data.cast::<f64>()))
                                    }
                                    (2, MPV_FORMAT_DOUBLE) if !p.data.is_null() => {
                                        Some(PlayerEvent::Duration(*p.data.cast::<f64>()))
                                    }
                                    (3, MPV_FORMAT_FLAG) if !p.data.is_null() => {
                                        Some(PlayerEvent::Paused(*p.data.cast::<i32>() != 0))
                                    }
                                    (4, MPV_FORMAT_FLAG) if !p.data.is_null() => {
                                        Some(PlayerEvent::Eof(*p.data.cast::<i32>() != 0))
                                    }
                                    _ => None,
                                }
                            }
                            MPV_EVENT_FILE_LOADED => {
                                callback(PlayerEvent::AudioTracks(audio_tracks(&h)));
                                Some(PlayerEvent::FileLoaded)
                            }
                            MPV_EVENT_VIDEO_RECONFIG => {
                                let w = h.property("dwidth").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
                                let h = h.property("dheight").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
                                Some(PlayerEvent::VideoSize(w, h))
                            }
                            MPV_EVENT_PLAYBACK_RESTART => Some(PlayerEvent::PlaybackRestart),
                            MPV_EVENT_END_FILE if !event.data.is_null() => {
                                let end = &*event.data.cast::<mpv_event_end_file>();
                                if end.error < 0 {
                                    Some(PlayerEvent::Error(
                                        CStr::from_ptr(mpv_error_string(end.error)).to_string_lossy().into_owned(),
                                    ))
                                } else {
                                    Some(PlayerEvent::Stopped)
                                }
                            }
                            _ => None,
                        }
                    };
                    if let Some(value) = value {
                        callback(value);
                    }
                }
            })
            .map_err(|e| Error(e.to_string()))?;
        Ok(Self { handle, quit, events: Some(events) })
    }
    pub fn command(&self, args: &[&str]) -> Result<()> {
        let args: Vec<_> = args.iter().map(|a| cstr(a)).collect::<Result<_>>()?;
        let mut pointers: Vec<_> = args.iter().map(|a| a.as_ptr()).collect();
        pointers.push(std::ptr::null());
        check(unsafe { mpv_command(self.handle.raw(), pointers.as_mut_ptr()) })
    }
    pub fn property(&self, name: &str) -> Result<String> {
        self.handle.property(name)
    }
    pub fn load(&self, path: &str) -> Result<()> {
        self.set_speed(1.0)?;
        let _ = self.set_slowmo_smoothing(1.0);
        self.command(&["set", "loop-file", if path.to_ascii_lowercase().ends_with(".gif") { "inf" } else { "no" }])?;
        self.command(&["set", "aid", "auto"])?;
        self.command(&["loadfile", path, "replace"])
    }
    /// Like `load`, but playback starts at `start` seconds (mpv's per-file `start` option: no frame of the file's
    /// beginning is shown first, unlike load + seek).
    pub fn load_at(&self, path: &str, start: f64) -> Result<()> {
        self.set_speed(1.0)?;
        let _ = self.set_slowmo_smoothing(1.0);
        self.command(&["set", "loop-file", "no"])?;
        self.command(&["set", "aid", "auto"])?;
        self.command(&["loadfile", path, "replace", "-1", &format!("start={:.3}", start.max(0.0))])
    }
    pub fn play(&self) -> Result<()> {
        if self.property("eof-reached").as_deref() == Ok("yes") && self.property("seekable").as_deref() == Ok("yes") {
            let _ = self.seek_exact(0.0);
        }
        self.pause(false)
    }
    pub fn pause(&self, paused: bool) -> Result<()> {
        self.command(&["set", "pause", if paused { "yes" } else { "no" }])
    }
    pub fn stop(&self) -> Result<()> {
        self.command(&["stop"])
    }
    pub fn seek_exact(&self, seconds: f64) -> Result<()> {
        self.seek(seconds, true)
    }
    pub fn seek_keyframe(&self, seconds: f64) -> Result<()> {
        self.seek(seconds, false)
    }
    fn seek(&self, seconds: f64, exact: bool) -> Result<()> {
        if !seconds.is_finite() {
            return Err(Error("invalid seek".into()));
        }
        self.command(&[
            "seek",
            &seconds.max(0.0).to_string(),
            if exact { "absolute+exact" } else { "absolute+keyframes" },
        ])
    }
    pub fn set_volume(&self, volume: f64) -> Result<()> {
        if !volume.is_finite() {
            return Err(Error("invalid volume".into()));
        }
        self.command(&["set", "volume", &volume.clamp(0.0, 100.0).to_string()])
    }
    pub fn set_mute(&self, mute: bool) -> Result<()> {
        self.command(&["set", "mute", if mute { "yes" } else { "no" }])
    }
    pub fn set_speed(&self, speed: f64) -> Result<()> {
        if !speed.is_finite() || !(0.01..=100.0).contains(&speed) {
            return Err(Error("invalid speed".into()));
        }
        self.command(&["set", "speed", &speed.to_string()])
    }
    pub fn audio_tracks(&self) -> Vec<AudioTrack> {
        audio_tracks(&self.handle)
    }
    /// Recording order: 0=mixed, 1=system, 2=microphone. Never guess from absent MP4 titles.
    pub fn select_audio_track(&self, index: usize) -> Result<()> {
        let track = self
            .audio_tracks()
            .into_iter()
            .find(|t| t.index == index)
            .ok_or_else(|| Error(format!("audio track {index} unavailable")))?;
        self.command(&["set", "aid", &track.id.to_string()])
    }
    pub fn frame_step(&self) -> Result<()> {
        self.command(&["frame-step"])
    }
    /// Slow motion (< 1x): synthesize in-between frames for the display (mpv interpolation needs display
    /// sync); 1x and faster: real frames only, audio-synced.
    pub fn set_slowmo_smoothing(&self, speed: f64) -> Result<()> {
        self.set_slowmo_smoothing_enabled(speed, true)
    }
    /// `enabled = false`: slow motion shows the real frames only (owner can turn blending off).
    pub fn set_slowmo_smoothing_enabled(&self, speed: f64, enabled: bool) -> Result<()> {
        let slow = enabled && speed < 0.99;
        self.command(&["set", "video-sync", if slow { "display-resample" } else { "audio" }])?;
        // `oversample` only blends during the one vsync where frames switch: at 0.25x of a VFR replay
        // (12-23 source fps) that still read as a slideshow (owner, 2026-11-16). `linear` cross-fades over the
        // whole frame interval. True motion interpolation (NVIDIA Optical Flow) would be a separate feature.
        self.command(&["set", "tscale", if slow { "linear" } else { "oversample" }])?;
        self.command(&["set", "interpolation", if slow { "yes" } else { "no" }])
    }
}
fn audio_tracks(handle: &Handle) -> Vec<AudioTrack> {
    let count: usize = handle.property("track-list/count").ok().and_then(|s| s.parse().ok()).unwrap_or(0);
    let mut tracks = Vec::new();
    for i in 0..count {
        if handle.property(&format!("track-list/{i}/type")).as_deref() == Ok("audio") {
            if let Ok(id) = handle
                .property(&format!("track-list/{i}/id"))
                .and_then(|s| s.parse().map_err(|_| Error("invalid track id".into())))
            {
                tracks.push(AudioTrack { index: tracks.len(), id });
            }
        }
    }
    tracks
}
impl Drop for MpvPlayer {
    fn drop(&mut self) {
        self.quit.store(true, Ordering::Release);
        unsafe {
            mpv_wakeup(self.handle.raw());
        }
        if let Some(events) = self.events.take() {
            let _ = thread::Builder::new().name("Lag.MpvJoin".into()).spawn(move || {
                let _ = events.join();
            });
        }
    }
}

/// Exact libmpv options for Player v2 native child-window playback (`docs/PLAYER_DESIGN.md`).
pub fn native_mpv_options(wid: isize) -> Vec<(&'static str, String)> {
    vec![
        ("config", "no".into()),
        ("vo", "gpu-next".into()),
        ("gpu-api", "d3d11".into()),
        ("hwdec", "d3d11va".into()),
        ("hwdec-codecs", "all".into()),
        ("profile", "high-quality".into()),
        // high-quality's ewa_lanczossharp upscaler cannot keep up with 130+ fps clips stretched to 3440x1440 at
        // 165 Hz (measured on the owner's clip: 34 dropped frames in 15 s, fullscreen felt "less smooth").
        // Plain lanczos is just as sharp for a 1.33x upscale and dropped 0-3.
        ("scale", "lanczos".into()),
        // Owner test 2026-11-16: display-resample on VFR replays constantly resampled the AUDIO to chase uneven
        // frame times -> warbling/crackling sound (the recorded audio itself is clean). 1x plays audio-synced;
        // slow motion switches to display-resample + interpolation at runtime (set_slowmo_smoothing).
        ("video-sync", "audio".into()),
        ("interpolation", "no".into()),
        ("tscale", "oversample".into()),
        ("keep-open", "yes".into()),
        ("idle", "yes".into()),
        ("osc", "no".into()),
        ("osd-level", "0".into()),
        ("input-default-bindings", "no".into()),
        ("input-vo-keyboard", "no".into()),
        ("input-terminal", "no".into()),
        ("terminal", "no".into()),
        ("cursor-autohide", "no".into()),
        ("background", "color".into()),
        ("background-color", "#000000".into()),
        ("target-colorspace-hint", "no".into()),
        ("volume", "80".into()),
        ("pause", "yes".into()),
        ("ao", "wasapi".into()),
        ("wid", wid.to_string()),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_options_are_accepted_by_libmpv() {
        let handle = Handle(NonNull::new(unsafe { mpv_create() }).expect("mpv_create"));
        for (k, v) in native_mpv_options(0x1234) {
            handle.option(k, &v).unwrap_or_else(|e| panic!("option {k}={v} rejected: {e}"));
        }
        assert!(MpvPlayer::native(0, |_| {}).is_err());
        assert!(NativeVideoWindow::new(0).is_err());
    }
}
