//! Mirrors ViewModels/PlayerViewModel.cs, Views/PlayerView.axaml.cs and Services/VlcEngine.cs.
//! Player v2 (`docs/PLAYER_DESIGN.md`): native child HWND (`wid`) + owned non-activating transparent
//! Slint overlay (`ui/player_overlay.slint`).
use crate::services::AppCtxPlaybackExt;
use crate::shell::nofocus;
use crate::{
    app_ctx::AppCtx, AppWindow, LibraryVM, MainVM, Page, PlayerClip, PlayerOverlayVM, PlayerOverlayWindow, PlayerVM, Tr,
};
use lag_player::{MpvPlayer, NativeVideoWindow, PlayerEvent};
use slint::{ComponentHandle, Model};
use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    path::Path,
    rc::Rc,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::ClientToScreen,
    UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
    UI::WindowsAndMessaging::{
        GetCursorPos, GetWindowRect, IsIconic, IsWindowVisible, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
        WM_DPICHANGED, WM_MOVE, WM_NCDESTROY, WM_SIZE, WM_WINDOWPOSCHANGED,
    },
};

thread_local! {
    static HOST_SYNC: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
    /// Visible main-window UI floating over the video area (window coords, logical px).
    static OCCLUDER: Cell<Option<(f32, f32, f32, f32)>> = const { Cell::new(None) };
    /// Set by `init`: runs a fullscreen toggle behind a fade to black when video is on screen.
    static FULLSCREEN_FADE: RefCell<Option<Rc<dyn Fn(Box<dyn FnOnce()>)>>> = const { RefCell::new(None) };
    /// Screen position of the cursor at the last pointer event the player UI saw (see `synthetic_pointer_move`).
    static LAST_POINTER: Cell<(i32, i32)> = const { Cell::new((i32::MIN, i32::MIN)) };
    /// `--ui-check` / `--gpu-check` feed pointer events without moving the OS cursor: every one is real.
    static SCRIPTED_INPUT: Cell<bool> = const { Cell::new(false) };
    /// Screen position where the player hid the cursor; None = not hidden. See `cursor_proc`.
    static CURSOR_HIDDEN_AT: Cell<Option<(i32, i32)>> = const { Cell::new(None) };
}

const CURSOR_SUBCLASS_ID: usize = 0x4c4143;

/// Owner, 2026-11-17: the hidden cursor flickered in fullscreen. Windows asks the window under the cursor for
/// its cursor (WM_SETCURSOR) not only on movement but also on repaints and window changes, and winit answered
/// with the arrow every time; the tick hid it again ~30 ms later. While the cursor is hidden and has not moved
/// since, our player windows now answer "no cursor" themselves. Any movement releases it at once.
unsafe extern "system" fn cursor_proc(
    hwnd: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{SetCursor, HTCLIENT, WM_SETCURSOR};
    if message == WM_SETCURSOR && (lp.0 as u32 & 0xFFFF) == HTCLIENT {
        if let Some(at) = CURSOR_HIDDEN_AT.with(|c| c.get()) {
            let mut pt = POINT::default();
            if unsafe { GetCursorPos(&mut pt) }.is_ok() && (pt.x, pt.y) == at {
                unsafe {
                    let _ = SetCursor(None);
                }
                return LRESULT(1);
            }
            CURSOR_HIDDEN_AT.with(|c| c.set(None)); // moved: show it again (the window sets its own below)
        }
    } else if message == WM_NCDESTROY {
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(cursor_proc), id);
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wp, lp) }
}

fn install_cursor_proc(hwnd: HWND) {
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(cursor_proc), CURSOR_SUBCLASS_ID, 0);
    }
}

fn release_cursor_hide() {
    CURSOR_HIDDEN_AT.with(|c| c.set(None));
}

/// Visible main-window UI floating over a video area (window coords, logical px), shared with the editor.
pub(crate) fn occluder_rect() -> Option<(f32, f32, f32, f32)> {
    OCCLUDER.with(|o| o.get())
}

pub(crate) fn mark_scripted_input() {
    SCRIPTED_INPUT.with(|s| s.set(true));
}

/// True when the OS cursor has not moved since the previous call. Windows sends a fake WM_MOUSEMOVE whenever
/// a window under a still cursor is shown, hidden or moved (the overlay window going away in fullscreen);
/// those must not count as "the user moved the mouse" or the controls/cursor never stay hidden.
fn synthetic_pointer_move() -> bool {
    if SCRIPTED_INPUT.with(|s| s.get()) {
        return false;
    }
    let mut pt = POINT::default();
    if unsafe { GetCursorPos(&mut pt) }.is_err() {
        return false;
    }
    LAST_POINTER.with(|last| {
        let same = last.get() == (pt.x, pt.y);
        last.set((pt.x, pt.y));
        same
    })
}

/// Owner, 2026-11-17: "the cursor does not hide, neither with the buttons nor by itself". Slint only applies
/// `mouse-cursor` while it processes a pointer event, so switching it to `none` on an idle timer (no mouse
/// movement, by definition) never reached the screen. While the player wants it hidden and the cursor is
/// inside `area` over one of our windows, the cursor is cleared directly; the next real mouse move makes the
/// window set its cursor again and wakes the controls.
fn hide_cursor_over(area: RECT) {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, SetCursor, WindowFromPoint};
    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err()
            || pt.x < area.left
            || pt.x >= area.right
            || pt.y < area.top
            || pt.y >= area.bottom
        {
            release_cursor_hide();
            return;
        }
        let under = WindowFromPoint(pt);
        let mut pid = 0u32;
        if under.0.is_null() {
            return;
        }
        GetWindowThreadProcessId(under, Some(&mut pid));
        if pid != GetCurrentProcessId() {
            release_cursor_hide();
            return; // another app's window is under the cursor: not ours to change
        }
        if CURSOR_HIDDEN_AT.with(|c| c.replace(Some((pt.x, pt.y)))) != Some((pt.x, pt.y)) {
            let _ = SetCursor(None);
        }
    }
}

fn h_ref_smooth(host: &Rc<RefCell<Option<NativeHost>>>, on: bool) {
    if let Some(h) = host.borrow().as_ref() {
        h.overlay.global::<PlayerOverlayVM>().set_smooth_slowmo(on);
    }
}

/// Fullscreen switch with the video faded to black first (owner: the resize/rescale moment must not be
/// visible; "fade to black and back like other players"). Without visible video it switches at once.
pub fn toggle_fullscreen_with_fade(apply: Box<dyn FnOnce()>) {
    let fade = FULLSCREEN_FADE.with(|f| f.borrow().clone());
    match fade {
        Some(fade) => fade(apply),
        None => apply(),
    }
}

#[derive(Debug, PartialEq)]
enum Command {
    InitNative(isize),
    Load {
        path: String,
        gen: u64,
    },
    Toggle,
    Stop,
    Seek(f64, bool),
    Skip(f64),
    Volume(f64),
    Mute(bool),
    Speed(f64),
    Track(usize),
    Step,
    /// Slow-motion frame blending on/off (Settings.slowmo_interpolation).
    Smooth(bool),
    /// Log [PlayerStats] a few seconds from now (after a fullscreen switch).
    Stats,
}

impl Command {
    fn supersedes(&self, older: &Self) -> bool {
        matches!(
            (self, older),
            (Self::Seek(..), Self::Seek(..))
                | (Self::Volume(_), Self::Volume(_))
                | (Self::Mute(_), Self::Mute(_))
                | (Self::Load { .. }, Self::Load { .. })
        )
    }

    fn cancelled_by_new_load(&self) -> bool {
        matches!(
            self,
            Self::Load { .. }
                | Self::Toggle
                | Self::Stop
                | Self::Seek(..)
                | Self::Skip(_)
                | Self::Speed(_)
                | Self::Track(_)
                | Self::Step
        )
    }
}

/// Coalesces a burst of queued commands: keeps only the latest Seek/Volume/Mute/Load, and drops
/// stale playback/seek commands queued before a newer Load.
fn coalesce_command_burst(first: Command, pending: &mut VecDeque<Command>, rx: &mpsc::Receiver<Command>) -> Command {
    let mut current = first;
    while let Ok(next) = rx.try_recv() {
        if matches!(next, Command::Load { .. }) {
            pending.retain(|c| !c.cancelled_by_new_load());
            if current.cancelled_by_new_load() {
                current = next;
            } else {
                pending.push_back(next);
            }
            continue;
        }
        if pending.is_empty() && next.supersedes(&current) {
            current = next;
        } else {
            pending.push_back(next);
        }
    }
    current
}

struct Pixels {
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

impl Pixels {
    fn decode(path: &Path) -> Option<Self> {
        let image = image::open(path).ok()?.to_rgba8();
        Some(Self { width: image.width(), height: image.height(), bytes: image.into_raw() })
    }
    fn into_image(self) -> slint::Image {
        let buffer =
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&self.bytes, self.width, self.height);
        slint::Image::from_rgba8(buffer)
    }
}

enum Message {
    LoadStarted(u64),
    Event(u64, PlayerEvent),
    Still(u64, String, Option<Pixels>),
}

fn is_image(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| ["png", "jpg", "jpeg"].iter().any(|v| s.eq_ignore_ascii_case(v)))
}

fn clock(seconds: f64) -> String {
    let ms = (seconds.max(0.0) * 1000.0) as u64;
    if ms >= 3_600_000 {
        format!("{}:{:02}:{:02}.{:03}", ms / 3_600_000 % 24, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
    } else {
        format!("{}:{:02}.{:03}", ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
    }
}

/// Converts logical Slint rectangle to physical pixels, optionally insetting by `inset_logical`
/// (e.g. 1px windowed border).
pub(crate) fn logical_to_physical_rect(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    scale: f32,
    inset_logical: f32,
) -> (i32, i32, i32, i32) {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let inset = inset_logical.max(0.0);
    let lx = x + inset;
    let ly = y + inset;
    let lw = (w - inset * 2.0).max(0.0);
    let lh = (h - inset * 2.0).max(0.0);
    let px = (lx * scale).round() as i32;
    let py = (ly * scale).round() as i32;
    let pr = ((lx + lw) * scale).round() as i32;
    let pb = ((ly + lh) * scale).round() as i32;
    (px, py, (pr - px).max(0), (pb - py).max(0))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SurfaceVisibility {
    page_is_player: bool,
    window_visible: bool,
    window_minimized: bool,
    fs_transition: bool,
    has_clip: bool,
    is_image: bool,
    /// mpv has presented the current clip's first frame. Before that the child surface is an un-presented
    /// swapchain that DWM shows as a see-through hole (owner, 2026-11-16).
    video_ready: bool,
    width_px: i32,
    height_px: i32,
}

impl SurfaceVisibility {
    fn show_native(&self) -> bool {
        self.page_is_player
            && self.window_visible
            && !self.window_minimized
            && !self.fs_transition
            && self.has_clip
            && !self.is_image
            && self.video_ready
            && self.width_px > 0
            && self.height_px > 0
    }

    fn show_overlay(&self) -> bool {
        self.page_is_player
            && self.window_visible
            && !self.window_minimized
            && !self.fs_transition
            && self.width_px > 0
            && self.height_px > 0
    }
}

/// Pure session state tracking clip-load generations so rapid clip switches never leak stale
/// mpv events or old durations/positions into the UI.
#[derive(Debug)]
struct PlayerSessionState {
    load_gen: u64,
    worker_gen: u64,
    awaiting_loaded: bool,
    has_clip: bool,
    is_image: bool,
    is_playing: bool,
    is_loading: bool,
    path: String,
    position_sec: f64,
    duration_sec: f64,
    norm_position: f32,
    sampled: Instant,
    speed: f64,
    audio_track: i32,
    audio_track_count: i32,
    error: String,
    video_ready: bool,
    /// Stopped (Stop button, page left): the next play must be a fresh load so its events are accepted.
    stopped: bool,
}

impl Default for PlayerSessionState {
    fn default() -> Self {
        Self {
            load_gen: 0,
            worker_gen: 0,
            awaiting_loaded: false,
            has_clip: false,
            is_image: false,
            is_playing: false,
            is_loading: false,
            path: String::new(),
            position_sec: 0.0,
            duration_sec: 0.0,
            norm_position: 0.0,
            sampled: Instant::now(),
            speed: 1.0,
            audio_track: 0,
            audio_track_count: 0,
            error: String::new(),
            video_ready: false,
            stopped: false,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
struct StateDelta {
    applied: bool,
    select_default_track: bool,
}

impl PlayerSessionState {
    fn begin_load(&mut self, path: &str) -> u64 {
        self.load_gen = self.load_gen.saturating_add(1);
        self.has_clip = true;
        self.is_image = is_image(path);
        self.is_loading = !self.is_image;
        self.awaiting_loaded = !self.is_image;
        self.is_playing = false;
        self.path = path.to_owned();
        self.position_sec = 0.0;
        self.duration_sec = 0.0;
        self.norm_position = 0.0;
        self.sampled = Instant::now();
        self.speed = 1.0;
        self.audio_track = 0;
        self.audio_track_count = 0;
        self.error.clear();
        self.video_ready = false;
        self.stopped = false;
        self.load_gen
    }

    fn begin_stop(&mut self) -> u64 {
        self.load_gen = self.load_gen.saturating_add(1);
        self.video_ready = false; // mpv tears the VO down on stop
        self.stopped = true;
        self.awaiting_loaded = false;
        self.is_loading = false;
        self.is_playing = false;
        self.position_sec = 0.0;
        self.duration_sec = 0.0;
        self.norm_position = 0.0;
        self.sampled = Instant::now();
        self.load_gen
    }

    fn on_worker_load_started(&mut self, gen: u64) {
        self.worker_gen = gen;
    }

    fn apply_event(&mut self, event_gen: u64, event: &PlayerEvent, dragging: bool) -> StateDelta {
        let mut delta = StateDelta::default();
        // Ignore events belonging to a superseded load or emitted before the worker started our load.
        if event_gen != self.load_gen || self.worker_gen != self.load_gen {
            return delta;
        }
        if self.is_image {
            return delta;
        }
        match event {
            PlayerEvent::FileLoaded => {
                self.awaiting_loaded = false;
                self.is_loading = false;
                self.is_playing = self.has_clip;
                self.position_sec = 0.0;
                self.norm_position = 0.0;
                self.sampled = Instant::now();
                delta.applied = true;
                delta.select_default_track = true;
            }
            PlayerEvent::PlaybackRestart => {
                if !self.awaiting_loaded {
                    self.video_ready = true;
                    self.is_loading = false;
                    self.sampled = Instant::now();
                    delta.applied = true;
                }
            }
            PlayerEvent::Duration(v) if *v > 0.0 => {
                self.duration_sec = *v;
                delta.applied = true;
            }
            PlayerEvent::Position(v) => {
                if self.awaiting_loaded {
                    return delta;
                }
                self.position_sec = *v;
                self.sampled = Instant::now();
                if !dragging && self.duration_sec > 0.0 {
                    self.norm_position = (*v / self.duration_sec).clamp(0.0, 1.0) as f32;
                }
                delta.applied = true;
            }
            PlayerEvent::Paused(paused) => {
                if self.awaiting_loaded {
                    return delta;
                }
                if self.is_playing {
                    self.position_sec += self.sampled.elapsed().as_secs_f64() * self.speed.max(0.01);
                }
                self.sampled = Instant::now();
                self.is_playing = !*paused && self.has_clip;
                delta.applied = true;
            }
            PlayerEvent::Eof(true) => {
                if self.awaiting_loaded {
                    return delta;
                }
                self.is_playing = false;
                self.norm_position = 1.0;
                self.position_sec = self.duration_sec;
                self.sampled = Instant::now();
                delta.applied = true;
            }
            PlayerEvent::Stopped => {
                // `loadfile <path> replace` emits END_FILE for the previous clip before FileLoaded.
                if !self.awaiting_loaded && !self.is_loading {
                    self.is_playing = false;
                    self.norm_position = 0.0;
                    self.position_sec = 0.0;
                    self.duration_sec = 0.0;
                    delta.applied = true;
                }
            }
            PlayerEvent::AudioTracks(tracks) => {
                self.audio_track_count = tracks.len() as i32;
                delta.applied = true;
            }
            PlayerEvent::Error(e) => {
                self.video_ready = false;
                self.error = e.clone();
                self.awaiting_loaded = false;
                self.is_loading = false;
                self.is_playing = false;
                delta.applied = true;
            }
            _ => (),
        }
        delta
    }

    fn interpolated_time(&self) -> f64 {
        let mut time = self.position_sec;
        if self.is_playing {
            time += self.sampled.elapsed().as_secs_f64() * self.speed.max(0.01);
        }
        if self.duration_sec > 0.0 {
            time = time.clamp(0.0, self.duration_sec);
        }
        time
    }

    fn time_display(&self) -> String {
        if self.is_image {
            String::new()
        } else {
            format!("{} / {}", clock(self.interpolated_time()), clock(self.duration_sec))
        }
    }
}

fn worker(rx: mpsc::Receiver<Command>, tx: mpsc::Sender<Message>, latest_gen: Arc<AtomicU64>) {
    let mut player: Option<Arc<MpvPlayer>> = None;
    let active_gen = Arc::new(AtomicU64::new(0));
    let mut pending = VecDeque::new();
    let mut path = String::new();
    let mut stopped = true;
    let mut speed = 1.0f64;
    let mut smooth = true;

    loop {
        let first = if player.is_some() && !pending.is_empty() {
            pending.pop_front().unwrap()
        } else {
            match rx.recv() {
                Ok(c) => c,
                Err(_) => break,
            }
        };

        if let Command::InitNative(wid) = first {
            if player.is_none() && wid != 0 {
                let events = tx.clone();
                let gen_ref = active_gen.clone();
                match MpvPlayer::prewarm_native(wid, move |e| {
                    let g = gen_ref.load(Ordering::Acquire);
                    let _ = events.send(Message::Event(g, e));
                })
                .recv()
                {
                    Ok(Ok(p)) => player = Some(Arc::new(p)),
                    Ok(Err(e)) => {
                        let _ = tx.send(Message::Event(
                            latest_gen.load(Ordering::Acquire),
                            PlayerEvent::Error(e.to_string()),
                        ));
                    }
                    Err(_) => {
                        let _ = tx.send(Message::Event(
                            latest_gen.load(Ordering::Acquire),
                            PlayerEvent::Error("mpv prewarm worker unavailable".into()),
                        ));
                    }
                }
            }
            continue;
        }

        if player.is_none() {
            pending.push_back(first);
            continue;
        }

        let command = coalesce_command_burst(first, &mut pending, &rx);
        let p = player.as_ref().unwrap();
        let result = (|| -> lag_player::Result<()> {
            match command {
                Command::Load { path: next_path, gen } => {
                    if gen < latest_gen.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    active_gen.store(gen, Ordering::Release);
                    let _ = tx.send(Message::LoadStarted(gen));
                    path = next_path;
                    stopped = false;
                    if is_image(&path) {
                        p.stop()?;
                        let pixels = Pixels::decode(Path::new(&path));
                        let _ = tx.send(Message::Still(gen, path.clone(), pixels));
                    } else {
                        let _ = p.command(&["set", "brightness", "0"]); // never inherit a half-done fade
                        p.load(&path)?;
                        p.pause(false)?;
                        log_playback_stats(p.clone(), path.clone(), gen, latest_gen.clone());
                    }
                }
                Command::Toggle if !path.is_empty() && !is_image(&path) => {
                    if stopped {
                        p.load(&path)?;
                        p.pause(false)?;
                        stopped = false;
                    } else if p.property("pause").as_deref() == Ok("yes")
                        || p.property("eof-reached").as_deref() == Ok("yes")
                    {
                        p.play()?;
                    } else {
                        p.pause(true)?;
                    }
                }
                Command::Stop => {
                    stopped = true;
                    p.stop()?;
                }
                Command::Seek(fraction, exact) if !is_image(&path) => {
                    if p.property("seekable").as_deref() == Ok("yes") {
                        let duration = p.property("duration").ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                        if exact {
                            p.seek_exact(fraction.clamp(0.0, 1.0) * duration)?;
                        } else {
                            p.seek_keyframe(fraction.clamp(0.0, 1.0) * duration)?;
                        }
                    }
                }
                Command::Skip(delta) if !is_image(&path) => {
                    let pos = p.property("time-pos").ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                    p.seek_exact((pos + delta).max(0.0))?;
                }
                Command::Volume(value) => p.set_volume(value)?,
                Command::Mute(value) => p.set_mute(value)?,
                Command::Speed(value) => {
                    p.set_speed(value)?;
                    speed = value;
                    p.set_slowmo_smoothing_enabled(speed, smooth)?;
                }
                Command::Stats if !path.is_empty() && !is_image(&path) => {
                    log_playback_stats(p.clone(), path.clone(), latest_gen.load(Ordering::Acquire), latest_gen.clone());
                }
                Command::Smooth(on) => {
                    smooth = on;
                    p.set_slowmo_smoothing_enabled(speed, smooth)?;
                }
                Command::Track(index) => p.select_audio_track(index)?,
                Command::Step if !is_image(&path) => p.frame_step()?,
                Command::InitNative(_)
                | Command::Toggle
                | Command::Seek(..)
                | Command::Skip(_)
                | Command::Step
                | Command::Stats => (),
            }
            Ok(())
        })();

        if let Err(e) = result {
            log::warn!("[Player] mpv op failed: {e}");
            let g = active_gen.load(Ordering::Acquire);
            let _ = tx.send(Message::Event(g, PlayerEvent::Error(e.to_string())));
        }
    }

    if let Some(p) = player {
        let _ = p.stop();
    }
}

/// Player v2 acceptance numbers (docs/PLAYER_DESIGN.md) in the log, 6 s into each clip, if it is still
/// the current one. No UI; read the log after a live test.
fn log_playback_stats(p: Arc<MpvPlayer>, path: String, gen: u64, latest: Arc<AtomicU64>) {
    let _ = std::thread::Builder::new().name("Lag.PlayerStats".into()).spawn(move || {
        std::thread::sleep(Duration::from_secs(6));
        if latest.load(Ordering::Acquire) != gen {
            return;
        }

        let q = |n: &str| p.property(n).unwrap_or_else(|_| "-".into());
        log::info!(
            "[PlayerStats] {} | {}x{} on {}x{} fps={}/{} hwdec={} vo={} sync={} video-sync={} interp={} speed={} display-fps={} | drops frame={} decoder={} vo-delayed={} mistimed={} | pos={} paused={}",
            Path::new(&path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(),
            q("width"), q("height"), q("osd-width"), q("osd-height"), q("container-fps"), q("estimated-vf-fps"), q("hwdec-current"), q("current-vo"),
            q("display-sync-active"), q("video-sync"), q("interpolation"), q("speed"), q("display-fps"), q("frame-drop-count"), q("decoder-frame-drop-count"),
            q("vo-delayed-frame-count"), q("mistimed-frame-count"), q("time-pos"), q("pause"),
        );
    });
}

/// Owner, 2026-11-17: no player key worked (Space, Esc, M, arrows). While the player page is up, keyboard
/// focus could sit on one of our child/owned windows (the native video surface, mpv's own child window or the
/// non-activating overlay), none of which handle keys - only the main window does (Slint key handlers, Esc in
/// main_vm). If the foreground window is ours and focus is on any other window of our process, focus goes back
/// to the main window. Every focus owner change is logged once (class name) to see who takes it.
/// Only focus inside the player's own windows (child surfaces of the main window, mpv's child window, the overlay)
/// is pulled back. Owner, 2026-11-17: the themed tray menu (another window of ours) opened and closed at once
/// and the main window jumped to the front - this pulled the focus away from the menu.
fn keep_keyboard_focus(main: HWND, overlay: Option<HWND>, last: &RefCell<String>) {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO,
    };
    unsafe {
        let mut gti = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        if GetGUIThreadInfo(0, &mut gti).is_err() {
            return;
        }
        let focus = gti.hwndFocus;
        if focus.0.is_null() {
            return;
        }
        let mut pid = 0u32;
        let thread = GetWindowThreadProcessId(focus, Some(&mut pid));
        if pid != GetCurrentProcessId() {
            return; // another app is in front: not ours to touch
        }
        let mut buf = [0u16; 128];
        let n = GetClassNameW(focus, &mut buf).max(0) as usize;
        let class = String::from_utf16_lossy(&buf[..n]);
        if *last.borrow() != class {
            log::info!("[Player] keyboard focus on window class '{class}'");
            *last.borrow_mut() = class;
        }
        if focus == main {
            return;
        }
        use windows::Win32::UI::WindowsAndMessaging::IsChild;
        let in_player =
            IsChild(main, focus).as_bool() || overlay.is_some_and(|ov| focus == ov || IsChild(ov, focus).as_bool());
        if !in_player {
            return; // tray menu, dialogs, splash...: leave them their focus
        }
        let me = GetCurrentThreadId();
        let attached = thread != me && AttachThreadInput(me, thread, true).as_bool();
        let _ = SetFocus(Some(main));
        if attached {
            let _ = AttachThreadInput(me, thread, false);
        }
    }
}

fn sidebar_totals(clips: &[crate::app_ctx::SharedClip]) -> (u64, u64) {
    let total_ticks: i64 = clips.iter().map(|c| c.duration_ticks.max(0)).sum();
    let seconds = (total_ticks / 10_000_000) as u64;
    let bytes: u64 = clips.iter().map(|c| c.file_size.max(0) as u64).sum();
    (seconds, bytes)
}

fn format_total_duration(seconds: u64) -> String {
    let total = if seconds >= 3600 {
        lag_core::i18n::format("Time_HourMinShort", &[&(seconds / 3600).to_string(), &(seconds / 60 % 60).to_string()])
    } else if seconds >= 60 {
        lag_core::i18n::format("Time_MinShort", &[&(seconds / 60).to_string()])
    } else {
        lag_core::i18n::format("Time_SecShort", &[&seconds.to_string()])
    };
    total.unwrap_or_default()
}

fn header_meta_for(c: &crate::app_ctx::SharedClip) -> String {
    format!("{} · {}", c.created_date.format("%-d %b, %H:%M"), c.duration_display())
}

fn build_player_clips(clips: &[crate::app_ctx::SharedClip]) -> Vec<PlayerClip> {
    clips
        .iter()
        .map(|c| PlayerClip {
            path: c.file_path.as_str().into(),
            title: c.display_name().into(),
            meta: c.short_meta().into(),
            header_meta: header_meta_for(c).into(),
            is_image: c.is_image,
            thumbnail: c.thumbnail.as_ref().map(|b| slint::Image::from_rgba8(b.clone())).unwrap_or_default(),
        })
        .collect()
}

fn sync_sidebar_from_shared(
    ui: &AppWindow,
    overlay: Option<&PlayerOverlayWindow>,
    clips: &[crate::app_ctx::SharedClip],
) {
    let vm = ui.global::<PlayerVM>();
    let (seconds, bytes) = sidebar_totals(clips);
    let total_duration = format_total_duration(seconds);
    let total_size = if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else {
        format!("{:.0} MB", bytes as f64 / 1_048_576.0)
    };
    let items = build_player_clips(clips);
    vm.set_total_duration(total_duration.clone().into());
    vm.set_total_size(total_size.clone().into());
    vm.set_clips(Rc::new(slint::VecModel::from(items.clone())).into());
    if vm.get_has_clip() {
        let cur = vm.get_path();
        if let Some(active) = clips.iter().find(|c| c.file_path == cur.as_str()) {
            vm.set_title(active.display_name().into());
            vm.set_meta(header_meta_for(active).into());
        }
    }
    if let Some(ov) = overlay {
        let ovm = ov.global::<PlayerOverlayVM>();
        ovm.set_total_duration(total_duration.into());
        ovm.set_total_size(total_size.into());
        ovm.set_clips(Rc::new(slint::VecModel::from(items)).into());
    }
}

fn sync_overlay_from_vm(ui: &AppWindow, ov: &PlayerOverlayWindow, session: &PlayerSessionState) {
    let vm = ui.global::<PlayerVM>();
    let mvm = ui.global::<MainVM>();
    let ovm = ov.global::<PlayerOverlayVM>();
    ovm.set_has_clip(vm.get_has_clip());
    ovm.set_is_playing(vm.get_is_playing());
    ovm.set_is_loading(vm.get_is_loading());
    ovm.set_is_image(vm.get_is_image());
    ovm.set_is_fullscreen(mvm.get_is_fullscreen());
    ovm.set_path(vm.get_path());
    ovm.set_time_text(vm.get_time_text());
    if !ovm.get_dragging() {
        ovm.set_position(vm.get_position());
    }
    ovm.set_volume(vm.get_volume());
    ovm.set_muted(vm.get_muted());
    ovm.set_speed(vm.get_speed());
    ovm.set_audio_track(vm.get_audio_track());
    ovm.set_audio_track_count(vm.get_audio_track_count());
    ovm.set_error(vm.get_error());
    ovm.set_library_loading(ui.global::<LibraryVM>().get_is_loading());
    let _ = session;
}

struct NativeHost {
    main_hwnd: HWND,
    surface: NativeVideoWindow,
    overlay: PlayerOverlayWindow,
    last_overlay_rect: Cell<(i32, i32, i32, i32)>,
    overlay_shown: Cell<bool>,
    /// Fullscreen while playing with no controls on screen: the full-screen transparent overlay window is hidden
    /// so DWM does not blend it with every 3440x1440 frame. Mouse movement over the video brings it back.
    overlay_suppressed: Cell<bool>,
}

unsafe extern "system" fn main_host_proc(
    hwnd: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    let result = unsafe { DefSubclassProc(hwnd, message, wp, lp) };
    if matches!(message, WM_MOVE | WM_SIZE | WM_WINDOWPOSCHANGED | WM_DPICHANGED) {
        HOST_SYNC.with(|cb| {
            let cb = cb.borrow().clone();
            if let Some(cb) = cb {
                cb();
            }
        });
    } else if message == WM_NCDESTROY {
        HOST_SYNC.with(|cb| {
            *cb.borrow_mut() = None;
        });
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(main_host_proc), id);
        }
    }
    result
}

impl NativeHost {
    fn new(ui: &AppWindow, main_hwnd: HWND, ctx: &Rc<AppCtx>) -> Result<Self, lag_player::Error> {
        let surface = NativeVideoWindow::new(main_hwnd.0 as isize)?;
        let overlay = PlayerOverlayWindow::new()
            .map_err(|e| lag_player::Error(format!("PlayerOverlayWindow init failed: {e}")))?;

        overlay.global::<Tr>().on_t(|key, _rev| lag_core::i18n::tr(&key).into());
        overlay.global::<Tr>().set_rev(ui.global::<Tr>().get_rev());
        overlay.set_theme_mode(ui.get_theme_mode());

        // Wire overlay callbacks -> AppWindow PlayerVM / MainVM / LibraryVM callbacks.
        let ovm = overlay.global::<PlayerOverlayVM>();
        ovm.on_synthetic_move(synthetic_pointer_move);
        {
            let weak = ui.as_weak();
            ovm.on_toggle(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_toggle();
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_stop(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_stop();
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_seek(move |pos, exact| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().set_position(pos);
                    ui.global::<PlayerVM>().invoke_seek(pos, exact);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_skip(move |delta| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_skip(delta);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_next(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_next();
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_play_clip(move |path| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_play_clip(path);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_edit(move |path| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_edit(path);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_show_in_folder(move |path| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<LibraryVM>().invoke_context_show_in_folder(path);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_delete_clip(move |path| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<LibraryVM>().invoke_context_delete(path);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_volume_changed(move |vol| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().set_volume(vol);
                    ui.global::<PlayerVM>().invoke_volume_changed(vol);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_mute(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_mute();
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_speed_changed(move |speed| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_speed_changed(speed);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_audio_changed(move |idx| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_audio_changed(idx);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_toggle_fullscreen(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<MainVM>().invoke_toggle_fullscreen();
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_exit_fullscreen(move || {
                if let Some(ui) = weak.upgrade() {
                    if ui.global::<MainVM>().get_is_fullscreen() {
                        ui.global::<MainVM>().invoke_toggle_fullscreen();
                    }
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_dragging_changed(move |dragging| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().set_dragging(dragging);
                }
            });
        }

        // Sync initial sidebar items to the overlay.
        let clips = ctx.clips();
        let (seconds, bytes) = sidebar_totals(&clips);
        ovm.set_total_duration(format_total_duration(seconds).into());
        ovm.set_total_size(
            if bytes >= 1_073_741_824 {
                format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
            } else {
                format!("{:.0} MB", bytes as f64 / 1_048_576.0)
            }
            .into(),
        );
        ovm.set_clips(Rc::new(slint::VecModel::from(build_player_clips(&clips))).into());

        unsafe {
            let _ = SetWindowSubclass(main_hwnd, Some(main_host_proc), 0x4c4148, 0);
            install_cursor_proc(main_hwnd);
        }

        Ok(Self {
            main_hwnd,
            surface,
            overlay,
            last_overlay_rect: Cell::new((0, 0, 0, 0)),
            overlay_shown: Cell::new(false),
            overlay_suppressed: Cell::new(false),
        })
    }

    fn sync(&self, ui: &AppWindow, logical_rect: (f32, f32, f32, f32), fs_transition: bool, video_ready: bool) {
        let occluder = OCCLUDER.with(|o| o.get());
        let vm = ui.global::<PlayerVM>();
        let mvm = ui.global::<MainVM>();
        let is_fullscreen = mvm.get_is_fullscreen();
        let scale = ui.window().scale_factor();

        let (nx, ny, nw, nh) = logical_to_physical_rect(
            logical_rect.0,
            logical_rect.1,
            logical_rect.2,
            logical_rect.3,
            scale,
            if is_fullscreen { 0.0 } else { 1.0 },
        );
        let (ox, oy, ow, oh) =
            logical_to_physical_rect(logical_rect.0, logical_rect.1, logical_rect.2, logical_rect.3, scale, 0.0);

        let minimized = ui.window().is_minimized() || unsafe { IsIconic(self.main_hwnd).as_bool() };
        let visible = ui.window().is_visible() && unsafe { IsWindowVisible(self.main_hwnd).as_bool() };
        let vis = SurfaceVisibility {
            page_is_player: mvm.get_current_page() == Page::Player,
            window_visible: visible,
            window_minimized: minimized,
            fs_transition,
            has_clip: vm.get_has_clip(),
            is_image: vm.get_is_image(),
            video_ready,
            width_px: ow,
            height_px: oh,
        };

        // Size the surface even while hidden: mpv builds its swapchain at the window's size when the first
        // frame arrives; a 1x1 hidden surface made it build tiny and visibly rescale on show.
        if nw > 0 && nh > 0 && !fs_transition {
            self.surface.set_bounds(nx, ny, nw, nh);
        }
        // Cut floating main-window UI (capture flyout) out of the video instead of hiding it under the surface.
        let hole = occluder.and_then(|(x, y, w, h)| {
            let (px, py, pw, ph) = logical_to_physical_rect(x, y, w, h, scale, 0.0);
            let (l, t) = ((px - nx).max(0), (py - ny).max(0));
            let (r, b) = ((px + pw - nx).min(nw), (py + ph - ny).min(nh));
            (r > l && b > t).then_some((l, t, r - l, b - t))
        });
        // the video frame (views/player.slint) is 12 px rounded with a 1 px border and the surface sits 1 px
        // inside it: clip to the inner 11 px radius so no square corner shows; fullscreen is square
        self.surface.set_corner_radius(if is_fullscreen { 0 } else { (11.0 * scale).round() as i32 });
        self.surface.set_exclusion(hole);
        if vis.show_native() && nw > 0 && nh > 0 {
            self.surface.show();
        } else {
            self.surface.hide();
        }

        if vis.show_overlay() && !(is_fullscreen && self.overlay_suppressed.get()) {
            let mut pt = POINT { x: ox, y: oy };
            unsafe {
                let _ = ClientToScreen(self.main_hwnd, &mut pt);
            }
            let target = (pt.x, pt.y, ow.max(1), oh.max(1));

            if !self.overlay_shown.get() {
                nofocus::apply_owned_no_focus(self.overlay.window(), self.main_hwnd);
                let _ = self.overlay.show();
                nofocus::apply_owned_no_focus(self.overlay.window(), self.main_hwnd);
                nofocus::show_owned_no_activate(self.overlay.window());
                self.overlay_shown.set(true);
                self.last_overlay_rect.set((0, 0, 0, 0));
            }
            if nofocus::hwnd(self.overlay.window()).is_some() && self.last_overlay_rect.get() != target {
                if let Some(ov_hwnd) = nofocus::hwnd(self.overlay.window()) {
                    nofocus::apply_owned_no_focus(self.overlay.window(), self.main_hwnd);
                    unsafe {
                        let _ = SetWindowPos(
                            ov_hwnd,
                            None,
                            target.0,
                            target.1,
                            target.2,
                            target.3,
                            SWP_NOACTIVATE | SWP_NOZORDER,
                        );
                    }
                    self.last_overlay_rect.set(target);
                }
            }
        } else if self.overlay_shown.get() {
            let _ = self.overlay.hide();
            self.overlay_shown.set(false);
        }
    }

    fn check_cursor_auto_hide(&self, is_fullscreen: bool, pinned: bool) {
        if !self.overlay_shown.get() || pinned {
            return;
        }
        let ovm = self.overlay.global::<PlayerOverlayVM>();
        if !ovm.get_controls() && !(is_fullscreen && ovm.get_sidebar()) {
            return;
        }
        let Some(ov_hwnd) = nofocus::hwnd(self.overlay.window()) else {
            return;
        };
        let mut pt = POINT::default();
        let mut rect = RECT::default();
        unsafe {
            if GetCursorPos(&mut pt).is_ok()
                && GetWindowRect(ov_hwnd, &mut rect).is_ok()
                && (pt.x < rect.left || pt.x >= rect.right || pt.y < rect.top || pt.y >= rect.bottom)
            {
                ovm.set_controls(false);
                if is_fullscreen {
                    ovm.set_sidebar(false);
                }
            }
        }
    }
}

impl Drop for NativeHost {
    fn drop(&mut self) {
        HOST_SYNC.with(|cb| {
            *cb.borrow_mut() = None;
        });
        let _ = self.overlay.hide();
        self.surface.destroy();
    }
}

/// Initialize native surface host, typed event delivery, transport, sidebar and shared editor hook dispatcher.
pub fn init(ui: &AppWindow, ctx: &Rc<AppCtx>) {
    let (commands, rx) = mpsc::channel();
    let (messages, events) = mpsc::channel();
    let latest_gen = Arc::new(AtomicU64::new(0));
    {
        let tx = messages.clone();
        let latest = latest_gen.clone();
        std::thread::Builder::new()
            .name("Lag.MpvControl".into())
            .spawn(move || worker(rx, tx, latest))
            .expect("player control worker");
    }

    let session = Rc::new(RefCell::new(PlayerSessionState::default()));
    let host: Rc<RefCell<Option<NativeHost>>> = Rc::new(RefCell::new(None));
    let logical_rect = Rc::new(Cell::new((0.0f32, 0.0f32, 0.0f32, 0.0f32)));
    let fs_transition = Rc::new(Cell::new(false));
    let overlay_pinned = Rc::new(Cell::new(false));
    let fs_timer = Rc::new(slint::Timer::default());
    // Fullscreen fade: out (0 -> -100 brightness) before the switch, in (-100 -> 0) once the video is shown
    // again at its final size.
    let fade_timer = Rc::new(slint::Timer::default());
    let fade_in_pending = Rc::new(Cell::new(false));
    let cover_applied_at: Rc<Cell<Option<Instant>>> = Rc::new(Cell::new(None));
    const FADE_OUT_STEPS: i64 = 8; // x 15 ms ~ 120 ms
    const FADE_IN_STEPS: i64 = 12; // x 15 ms ~ 180 ms

    let sync_now: Rc<dyn Fn()> = {
        let weak = ui.as_weak();
        let host = host.clone();
        let logical_rect = logical_rect.clone();
        let fs_transition = fs_transition.clone();
        let session = session.clone();
        Rc::new(move || {
            let Some(ui) = weak.upgrade() else { return };
            if let Ok(h) = host.try_borrow() {
                if let Some(h) = h.as_ref() {
                    sync_overlay_from_vm(&ui, &h.overlay, &session.borrow());
                    let ready = session.borrow().video_ready;
                    h.sync(&ui, logical_rect.get(), fs_transition.get(), ready);
                }
            }
        })
    };

    let ensure_native_host: Rc<dyn Fn()> = {
        let weak = ui.as_weak();
        let ctx_weak = Rc::downgrade(ctx);
        let host = host.clone();
        let commands = commands.clone();
        let sync_now = sync_now.clone();
        let overlay_pinned = overlay_pinned.clone();
        Rc::new(move || {
            let (Some(ui), Some(ctx)) = (weak.upgrade(), ctx_weak.upgrade()) else {
                return;
            };
            if host.borrow().is_some() {
                return;
            }
            let Some(main_hwnd) = nofocus::hwnd(ui.window()) else {
                return;
            };
            match NativeHost::new(&ui, main_hwnd, &ctx) {
                Ok(h) => {
                    let wid = h.surface.wid();
                    let pinned_flag = overlay_pinned.clone();
                    h.overlay.global::<PlayerOverlayVM>().on_pinned_changed(move |p| {
                        pinned_flag.set(p);
                    });
                    *host.borrow_mut() = Some(h);
                    let sync_cb = sync_now.clone();
                    HOST_SYNC.with(|slot| *slot.borrow_mut() = Some(sync_cb));
                    let _ = commands.send(Command::InitNative(wid));
                    // slow-motion blending switch in the speed menu (persisted)
                    {
                        let on = ctx.settings().slowmo_interpolation;
                        h_ref_smooth(&host, on);
                        let _ = commands.send(Command::Smooth(on));
                        let tx = commands.clone();
                        let ctx_w = Rc::downgrade(&ctx);
                        let host_w = Rc::downgrade(&host);
                        if let Some(h) = host.borrow().as_ref() {
                            h.overlay.global::<PlayerOverlayVM>().on_smooth_slowmo_changed(move |v| {
                                if let Some(ctx) = ctx_w.upgrade() {
                                    ctx.update_settings(|s| s.slowmo_interpolation = v);
                                }
                                if let Some(host) = host_w.upgrade() {
                                    h_ref_smooth(&host, v);
                                }
                                let _ = tx.send(Command::Smooth(v));
                            });
                        }
                    }
                    sync_now();
                }
                Err(e) => {
                    log::error!("[Player] native surface init failed: {e}");
                    ui.global::<PlayerVM>().set_error(e.to_string().into());
                }
            }
        })
    };

    // Attempt creating native host on the first event loop turn (no-op in headless `--snapshot`).
    {
        let ensure = ensure_native_host.clone();
        slint::Timer::single_shot(Duration::ZERO, move || ensure());
    }

    sync_sidebar_from_shared(ui, None, &ctx.clips());
    {
        let weak = ui.as_weak();
        let host = host.clone();
        ctx.on_clips_changed(move |clips| {
            if let Some(ui) = weak.upgrade() {
                let h = host.borrow();
                sync_sidebar_from_shared(&ui, h.as_ref().map(|x| &x.overlay), clips);
            }
        });
    }
    {
        let weak = ui.as_weak();
        let ctx_weak = Rc::downgrade(ctx);
        let host = host.clone();
        ctx.on_settings_changed(move |s| {
            if let (Some(ui), Some(ctx)) = (weak.upgrade(), ctx_weak.upgrade()) {
                let (seconds, _) = sidebar_totals(&ctx.clips());
                let dur = format_total_duration(seconds);
                ui.global::<PlayerVM>().set_total_duration(dur.clone().into());
                if let Some(h) = host.borrow().as_ref() {
                    h.overlay.set_theme_mode(s.theme.as_deref().unwrap_or("dark").into());
                    h.overlay.global::<Tr>().set_rev(ui.global::<Tr>().get_rev());
                    h.overlay.global::<PlayerOverlayVM>().set_total_duration(dur.into());
                }
            }
        });
    }

    let weak = ui.as_weak();
    let load: Rc<dyn Fn(&str)> = {
        let commands = commands.clone();
        let weak = weak.clone();
        let session = session.clone();
        let latest_gen = latest_gen.clone();
        let ensure_native_host = ensure_native_host.clone();
        let sync_now = sync_now.clone();
        let host = host.clone();
        Rc::new(move |path| {
            ensure_native_host();
            let Some(ui) = weak.upgrade() else { return };
            let vm = ui.global::<PlayerVM>();
            let gen = session.borrow_mut().begin_load(path);
            latest_gen.store(gen, Ordering::Release);

            let is_img = is_image(path);
            vm.set_path(path.into());
            vm.set_has_clip(true);
            vm.set_is_image(is_img);
            vm.set_is_loading(!is_img);
            vm.set_is_playing(false);
            vm.set_position(0.0);
            vm.set_time_text(if is_img { "" } else { "0:00.000 / 0:00.000" }.into());
            vm.set_speed(1.0);
            vm.set_audio_track(0);
            vm.set_audio_track_count(0);
            vm.set_error("".into());
            vm.set_image(slint::Image::default());

            if let Some(h) = host.borrow().as_ref() {
                let ovm = h.overlay.global::<PlayerOverlayVM>();
                ovm.set_position(0.0);
                ovm.set_dragging(false);
            }

            let clip = vm.get_clips().iter().find(|c| c.path == path);
            vm.set_title(clip.as_ref().map(|c| c.title.clone()).unwrap_or_else(|| {
                Path::new(path).file_stem().unwrap_or_default().to_string_lossy().to_string().into()
            }));
            vm.set_meta(clip.map(|c| c.header_meta).unwrap_or_default());
            ui.global::<MainVM>().set_current_page(Page::Player);
            let _ = commands.send(Command::Load { path: path.to_owned(), gen });
            sync_now();
        })
    };
    {
        let load = load.clone();
        ctx.on_play_clip(move |p| load(p));
    }
    {
        let load = load.clone();
        ui.global::<PlayerVM>().on_play_clip(move |p| load(&p));
    }
    {
        let ctx = Rc::downgrade(ctx);
        ui.global::<PlayerVM>().on_edit(move |p| {
            if let Some(ctx) = ctx.upgrade() {
                if !p.is_empty() {
                    ctx.edit_clip(&p);
                }
            }
        });
    }
    {
        let weak = weak.clone();
        let load = load.clone();
        ui.global::<PlayerVM>().on_next(move || {
            let Some(ui) = weak.upgrade() else { return };
            let vm = ui.global::<PlayerVM>();
            let clips = vm.get_clips();
            let n = clips.row_count();
            if n == 0 {
                return;
            }
            let start = clips.iter().position(|c| c.path == vm.get_path()).unwrap_or(n - 1);
            for step in 1..=n {
                if let Some(c) = clips.row_data((start + step) % n) {
                    if !c.is_image {
                        load(&c.path);
                        break;
                    }
                }
            }
        });
    }

    {
        let tx = commands.clone();
        let session = session.clone();
        let load = load.clone();
        ui.global::<PlayerVM>().on_toggle(move || {
            let (stopped, path) = {
                let s = session.borrow();
                (s.stopped && s.has_clip && !s.is_image, s.path.clone())
            };
            if stopped && !path.is_empty() {
                load(&path); // new generation: its events (first frame, time, pause) are accepted again
            } else {
                let _ = tx.send(Command::Toggle);
            }
        });
    }
    {
        let tx = commands.clone();
        let weak = weak.clone();
        let session = session.clone();
        let latest_gen = latest_gen.clone();
        let sync_now = sync_now.clone();
        ui.global::<PlayerVM>().on_stop(move || {
            let gen = session.borrow_mut().begin_stop();
            latest_gen.store(gen, Ordering::Release);
            let _ = tx.send(Command::Stop);
            if let Some(ui) = weak.upgrade() {
                let vm = ui.global::<PlayerVM>();
                vm.set_is_playing(false);
                vm.set_is_loading(false);
                vm.set_position(0.0);
                vm.set_time_text(if vm.get_is_image() { "" } else { "0:00.000 / 0:00.000" }.into());
            }
            sync_now();
        });
    }
    {
        let tx = commands.clone();
        ui.global::<PlayerVM>().on_seek(move |v, exact| {
            let _ = tx.send(Command::Seek(v as f64, exact));
        });
    }
    {
        let tx = commands.clone();
        ui.global::<PlayerVM>().on_skip(move |v| {
            let _ = tx.send(Command::Skip(v as f64));
        });
    }
    {
        let tx = commands.clone();
        ui.global::<PlayerVM>().on_volume_changed(move |v| {
            let _ = tx.send(Command::Volume(v as f64));
        });
    }
    {
        let tx = commands.clone();
        let weak = weak.clone();
        let host = host.clone();
        ui.global::<PlayerVM>().on_mute(move || {
            if let Some(ui) = weak.upgrade() {
                let vm = ui.global::<PlayerVM>();
                let muted = !vm.get_muted();
                vm.set_muted(muted);
                vm.set_mute_flash(false);
                vm.set_mute_flash(true);
                if let Some(h) = host.borrow().as_ref() {
                    let ovm = h.overlay.global::<PlayerOverlayVM>();
                    ovm.set_muted(muted);
                    ovm.set_mute_flash(false);
                    ovm.set_mute_flash(true);
                }
                let _ = tx.send(Command::Mute(muted));
            }
        });
    }
    {
        let tx = commands.clone();
        let weak = weak.clone();
        let session = session.clone();
        let sync_now = sync_now.clone();
        ui.global::<PlayerVM>().on_speed_changed(move |v| {
            session.borrow_mut().speed = v as f64;
            if let Some(ui) = weak.upgrade() {
                ui.global::<PlayerVM>().set_speed(v);
            }
            let _ = tx.send(Command::Speed(v as f64));
            sync_now();
        });
    }
    {
        let tx = commands.clone();
        let weak = weak.clone();
        let session = session.clone();
        let sync_now = sync_now.clone();
        ui.global::<PlayerVM>().on_audio_changed(move |v| {
            session.borrow_mut().audio_track = v;
            if let Some(ui) = weak.upgrade() {
                ui.global::<PlayerVM>().set_audio_track(v);
            }
            let _ = tx.send(Command::Track(v.max(0) as usize));
            sync_now();
        });
    }
    {
        let tx = commands.clone();
        let weak = weak.clone();
        let session = session.clone();
        let latest_gen = latest_gen.clone();
        let sync_now = sync_now.clone();
        let ensure_native_host = ensure_native_host.clone();
        ui.global::<PlayerVM>().on_active_changed(move |active| {
            if active {
                ensure_native_host();
            } else {
                let gen = session.borrow_mut().begin_stop();
                latest_gen.store(gen, Ordering::Release);
                let _ = tx.send(Command::Stop);
                if let Some(ui) = weak.upgrade() {
                    let vm = ui.global::<PlayerVM>();
                    vm.set_is_playing(false);
                    vm.set_is_loading(false);
                }
            }
            sync_now();
        });
    }
    {
        let tx = commands.clone();
        let weak = weak.clone();
        let session = session.clone();
        let latest_gen = latest_gen.clone();
        let sync_now = sync_now.clone();
        ctx.on_stop_playback(move |_| {
            let gen = session.borrow_mut().begin_stop();
            latest_gen.store(gen, Ordering::Release);
            let _ = tx.send(Command::Stop);
            if let Some(ui) = weak.upgrade() {
                let vm = ui.global::<PlayerVM>();
                vm.set_is_playing(false);
                vm.set_is_loading(false);
            }
            sync_now();
        });
    }
    {
        let logical_rect = logical_rect.clone();
        let fs_transition = fs_transition.clone();
        let fs_timer = fs_timer.clone();
        let sync_now = sync_now.clone();
        ui.global::<PlayerVM>().on_video_rect_changed(move |x, y, w, h| {
            logical_rect.set((x, y, w, h));
            // During a fullscreen switch the window resizes in several steps; showing the video on the first
            // one let DWM stretch mpv's old-size frame (visible warp, owner 2026-11-16). Keep it hidden until
            // the size has been stable for 150 ms, then show it once at the final size.
            if fs_transition.get() {
                let fs_transition = fs_transition.clone();
                let sync_now = sync_now.clone();
                fs_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(150), move || {
                    fs_transition.set(false);
                    sync_now();
                });
            }
            sync_now();
        });
    }
    {
        let fs_transition = fs_transition.clone();
        let fs_timer = fs_timer.clone();
        let sync_now = sync_now.clone();
        let stats_tx = commands.clone();
        ui.global::<PlayerVM>().on_fullscreen_changed(move |full| {
            log::info!("[Player] fullscreen {}", if full { "on" } else { "off" });
            let _ = stats_tx.send(Command::Stats);
            fs_transition.set(true);
            sync_now();
            let fs_transition = fs_transition.clone();
            let sync_now = sync_now.clone();
            fs_timer.start(slint::TimerMode::SingleShot, Duration::from_millis(250), move || {
                fs_transition.set(false);
                sync_now();
            });
        });
    }
    {
        let weak = weak.clone();
        let host = host.clone();
        let overlay_pinned = overlay_pinned.clone();
        ui.global::<PlayerVM>().on_pointer_left_video(move || {
            let Some(ui) = weak.upgrade() else { return };
            if !ui.global::<MainVM>().get_is_fullscreen() && !overlay_pinned.get() {
                if let Some(h) = host.borrow().as_ref() {
                    h.overlay.global::<PlayerOverlayVM>().set_controls(false);
                }
            }
        });
    }

    ui.global::<PlayerVM>().on_synthetic_move(synthetic_pointer_move);
    {
        // fullscreen: movement over the video re-shows the overlay where its controls live (bottom bar, right list)
        let weak = weak.clone();
        let host = host.clone();
        let logical_rect = logical_rect.clone();
        let sync_now = sync_now.clone();
        ui.global::<PlayerVM>().on_video_pointer(move |x, y| {
            let Some(ui) = weak.upgrade() else { return };
            if !ui.global::<MainVM>().get_is_fullscreen() {
                return;
            }
            let (_, _, w, hgt) = logical_rect.get();
            let in_bottom = y >= hgt - 140.0;
            let in_right = x >= w - 280.0 && !in_bottom;
            if let Some(h) = host.borrow().as_ref() {
                let paused = !ui.global::<PlayerVM>().get_is_playing();
                if in_bottom || in_right || paused {
                    let ovm = h.overlay.global::<PlayerOverlayVM>();
                    ovm.set_controls(in_bottom);
                    ovm.set_sidebar(in_right);
                    h.overlay_suppressed.set(false);
                }
            }
            sync_now();
        });
    }
    {
        let sync_now = sync_now.clone();
        ui.global::<MainVM>().on_occluder_changed(move |visible, x, y, w, h| {
            OCCLUDER.with(|o| o.set(visible.then_some((x, y, w, h))));
            sync_now();
        });
    }
    // Fullscreen switch behind a full-monitor black cover (shell::fade_cover): fade in ~120 ms, switch, the
    // per-tick check below fades it out once the window and the video have settled.
    {
        let fade_timer_c = fade_timer.clone();
        let fade_in_pending = fade_in_pending.clone();
        let applied_at = cover_applied_at.clone();
        let weak = ui.as_weak();
        let fade: Rc<dyn Fn(Box<dyn FnOnce()>)> = Rc::new(move |apply: Box<dyn FnOnce()>| {
            let main = weak.upgrade().and_then(|ui| {
                if ui.window().is_visible() && !ui.window().is_minimized() {
                    nofocus::hwnd(ui.window())
                } else {
                    None
                }
            });
            let Some(main) = main else {
                apply();
                return;
            };
            if fade_in_pending.get() {
                return; // a switch is already running
            }
            crate::shell::fade_cover::show(main, 0);
            let step = Cell::new(0i64);
            let apply = RefCell::new(Some(apply));
            let pending = fade_in_pending.clone();
            let applied_at = applied_at.clone();
            let timer = fade_timer_c.clone();
            fade_timer_c.start(slint::TimerMode::Repeated, Duration::from_millis(15), move || {
                let n = step.get() + 1;
                step.set(n);
                crate::shell::fade_cover::set_alpha((255 * n.min(FADE_OUT_STEPS) / FADE_OUT_STEPS) as u8);
                // one extra tick so the black cover is on screen before the window changes
                if n > FADE_OUT_STEPS {
                    timer.stop();
                    pending.set(true);
                    applied_at.set(Some(Instant::now()));
                    if let Some(a) = apply.borrow_mut().take() {
                        a();
                    }
                }
            });
        });
        FULLSCREEN_FADE.with(|slot| *slot.borrow_mut() = Some(fade));
    }

    let timer = Rc::new(slint::Timer::default());
    let keep_timer = timer.clone();
    ui.global::<PlayerVM>().on_frame_step({
        let tx = commands.clone();
        move || {
            let _ = &keep_timer;
            let _ = tx.send(Command::Step);
        }
    });

    let mut was_visible = true;
    let last_focus_class = RefCell::new(String::new());
    // fullscreen idle: when the controls went away (their fade-out must finish before the overlay window hides)
    let idle_since: Cell<Option<Instant>> = Cell::new(None);
    // OS cursor position at the last tick and when it last changed (fullscreen cursor idle)
    let cursor_last = Cell::new((i32::MIN, i32::MIN));
    let cursor_moved_at = Cell::new(Instant::now());
    let cursor_proc_on = Cell::new(0isize); // overlay HWND that already has cursor_proc
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || {
        ensure_native_host();
        let Some(ui) = weak.upgrade() else { return };
        let vm = ui.global::<PlayerVM>();
        let mvm = ui.global::<MainVM>();
        let page_visible =
            ui.window().is_visible() && !ui.window().is_minimized() && mvm.get_current_page() == Page::Player;
        if was_visible && !page_visible {
            let gen = session.borrow_mut().begin_stop();
            latest_gen.store(gen, Ordering::Release);
            let _ = commands.send(Command::Stop);
            vm.set_is_playing(false);
            vm.set_is_loading(false);
        }
        let just_hidden = was_visible && !page_visible;
        was_visible = page_visible;

        for message in events.try_iter() {
            match message {
                Message::LoadStarted(gen) => {
                    session.borrow_mut().on_worker_load_started(gen);
                }
                Message::Event(event_gen, event) => {
                    let dragging = vm.get_dragging();
                    let delta = session.borrow_mut().apply_event(event_gen, &event, dragging);
                    if delta.applied {
                        let s = session.borrow();
                        vm.set_is_loading(s.is_loading);
                        vm.set_is_playing(s.is_playing);
                        vm.set_position(s.norm_position);
                        vm.set_audio_track_count(s.audio_track_count);
                        vm.set_error(s.error.as_str().into());
                        if delta.select_default_track {
                            let _ = commands.send(Command::Track(0));
                        }
                    }
                }
                Message::Still(gen, path, pixels) => {
                    let s = session.borrow();
                    if gen == s.load_gen && vm.get_path() == path {
                        if let Some(pixels) = pixels {
                            vm.set_image(pixels.into_image());
                        } else {
                            log::warn!("[Player] Failed to load screenshot: {path}");
                        }
                    }
                }
            }
        }

        // Lift the fullscreen cover once the window has settled and the video (if any) is back on screen.
        if fade_in_pending.get() {
            let since = cover_applied_at.get().map(|t| t.elapsed()).unwrap_or_default();
            let video_back = host.borrow().as_ref().is_some_and(|h| h.surface.is_visible())
                || !vm.get_has_clip()
                || vm.get_is_image()
                || !page_visible;
            let settled = !fs_transition.get() && since >= Duration::from_millis(140);
            if (settled && video_back) || since >= Duration::from_millis(1500) {
                fade_in_pending.set(false);
                let step = Cell::new(0i64);
                let timer = fade_timer.clone();
                fade_timer.start(slint::TimerMode::Repeated, Duration::from_millis(15), move || {
                    let n = step.get() + 1;
                    step.set(n);
                    let a = 255 - 255 * n.min(FADE_IN_STEPS) / FADE_IN_STEPS;
                    crate::shell::fade_cover::set_alpha(a as u8);
                    if n >= FADE_IN_STEPS {
                        timer.stop();
                        crate::shell::fade_cover::hide();
                    }
                });
            }
        }
        // Nothing to draw while the player page is not on screen (tray, minimized, other page, game in
        // front): skip the per-tick overlay/property sync, but run it once on the hide transition so the
        // surface and the overlay window actually get hidden.
        if !page_visible && !just_hidden {
            return;
        }
        if !vm.get_is_image() {
            vm.set_time_text(session.borrow().time_display().into());
        }

        if let Some(h) = host.borrow().as_ref() {
            h.check_cursor_auto_hide(mvm.get_is_fullscreen(), overlay_pinned.get());
            keep_keyboard_focus(h.main_hwnd, nofocus::hwnd(h.overlay.window()), &last_focus_class);
            {
                let ovm = h.overlay.global::<PlayerOverlayVM>();
                let idle = mvm.get_is_fullscreen()
                    && session.borrow().is_playing
                    && !ovm.get_controls()
                    && !ovm.get_sidebar()
                    && !overlay_pinned.get();
                if idle {
                    let since = idle_since.get().unwrap_or_else(|| {
                        let now = Instant::now();
                        idle_since.set(Some(now));
                        now
                    });
                    // 300 ms > the 250 ms fade-out of the bar / clip list: hide the window only after it played
                    if since.elapsed() >= Duration::from_millis(300) {
                        if !h.overlay_suppressed.get() {
                            // hidden by inactivity -> keep the cursor hidden over the main window too
                            ui.global::<PlayerVM>().set_cursor_hidden(ovm.get_cursor_idle());
                        }
                        h.overlay_suppressed.set(true);
                    }
                } else {
                    idle_since.set(None);
                    if !mvm.get_is_fullscreen() || !session.borrow().is_playing {
                        h.overlay_suppressed.set(false); // windowed, or paused (the big play button must show)
                    }
                }
                // keep the cursor hidden while idle and playing (see hide_cursor_over). Owner, 2026-11-17:
                // windowed worked, fullscreen did not - there the hide depended on a main-window Slint timer
                // that only a pointer event over the main window started (and the overlay hiding under a
                // still cursor sends none that counts). Idle is now measured here from the OS cursor itself.
                let mut pt = POINT::default();
                if unsafe { GetCursorPos(&mut pt) }.is_ok() && cursor_last.get() != (pt.x, pt.y) {
                    cursor_last.set((pt.x, pt.y));
                    cursor_moved_at.set(Instant::now());
                }
                if session.borrow().is_playing && mvm.get_current_page() == Page::Player {
                    let mut area = RECT::default();
                    let still = cursor_moved_at.get().elapsed() >= Duration::from_millis(2500);
                    let target = if h.overlay_shown.get() && ovm.get_cursor_idle() {
                        nofocus::hwnd(h.overlay.window())
                    } else if mvm.get_is_fullscreen() && still && !overlay_pinned.get() {
                        Some(h.main_hwnd)
                    } else {
                        None
                    };
                    if let Some(ov) = nofocus::hwnd(h.overlay.window()) {
                        if cursor_proc_on.get() != ov.0 as isize {
                            install_cursor_proc(ov);
                            cursor_proc_on.set(ov.0 as isize);
                        }
                    }
                    match target {
                        Some(w) if unsafe { GetWindowRect(w, &mut area) }.is_ok() => hide_cursor_over(area),
                        _ => release_cursor_hide(),
                    }
                } else {
                    release_cursor_hide();
                }
            }
            sync_overlay_from_vm(&ui, &h.overlay, &session.borrow());
            let ready = session.borrow().video_ready;
            h.sync(&ui, logical_rect.get(), fs_transition.get(), ready);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clock_matches_csharp() {
        assert_eq!(clock(0.0), "0:00.000");
        assert_eq!(clock(65.123), "1:05.123");
        assert_eq!(clock(3661.001), "1:01:01.001");
        assert_eq!(clock(-2.0), "0:00.000");
    }

    #[test]
    fn drag_release_supersedes_pending_keyframe() {
        assert!(Command::Seek(0.8, true).supersedes(&Command::Seek(0.7, false)));
        assert!(!Command::Load { path: "next.mp4".into(), gen: 2 }.supersedes(&Command::Seek(0.7, false)));
    }

    #[test]
    fn rapid_clip_switching_coalesces_obsolete_loads_and_seeks_preserving_volume() {
        let (tx, rx) = mpsc::channel();
        let _ = tx.send(Command::Seek(0.5, false));
        let _ = tx.send(Command::Volume(45.0));
        let _ = tx.send(Command::Load { path: "clip2.mp4".into(), gen: 2 });
        let _ = tx.send(Command::Seek(0.2, true));
        let _ = tx.send(Command::Load { path: "clip3.mp4".into(), gen: 3 });

        let mut pending = VecDeque::new();
        let first = coalesce_command_burst(Command::Load { path: "clip1.mp4".into(), gen: 1 }, &mut pending, &rx);
        assert_eq!(first, Command::Load { path: "clip3.mp4".into(), gen: 3 });
        let rest: Vec<_> = pending.into_iter().collect();
        assert_eq!(rest, vec![Command::Volume(45.0)]);
    }

    #[test]
    fn clip_switch_resets_state_and_filters_stale_events() {
        let mut s = PlayerSessionState::default();
        let g1 = s.begin_load("a.mp4");
        assert_eq!(g1, 1);
        assert!(s.is_loading);
        assert!(!s.is_playing);
        assert_eq!(s.time_display(), "0:00.000 / 0:00.000");

        s.on_worker_load_started(g1);
        assert!(s.apply_event(g1, &PlayerEvent::Duration(20.0), false).applied);
        assert!(s.apply_event(g1, &PlayerEvent::FileLoaded, false).select_default_track);
        assert!(s.apply_event(g1, &PlayerEvent::Position(10.0), false).applied);
        assert!((s.norm_position - 0.5).abs() < 0.001);

        // Switch to clip B: resets position, duration, playing, loading.
        let g2 = s.begin_load("b.mp4");
        assert_eq!(g2, 2);
        assert!(s.is_loading);
        assert!(!s.is_playing);
        assert_eq!(s.position_sec, 0.0);
        assert_eq!(s.duration_sec, 0.0);
        assert_eq!(s.norm_position, 0.0);
        assert_eq!(s.time_display(), "0:00.000 / 0:00.000");

        // Stale position/stopped from clip A must be ignored.
        assert!(!s.apply_event(g1, &PlayerEvent::Position(11.0), false).applied);
        assert!(!s.apply_event(g1, &PlayerEvent::Stopped, false).applied);
        assert_eq!(s.position_sec, 0.0);

        // Even before FileLoaded for B, an old Stopped event emitted after worker started B is ignored.
        s.on_worker_load_started(g2);
        assert!(!s.apply_event(g2, &PlayerEvent::Stopped, false).applied);
        assert!(!s.apply_event(g2, &PlayerEvent::Position(12.0), false).applied);
        assert!(s.is_loading);

        // B loads and reports its own duration.
        assert!(s.apply_event(g2, &PlayerEvent::Duration(8.0), false).applied);
        assert!(s.apply_event(g2, &PlayerEvent::FileLoaded, false).applied);
        assert!(!s.is_loading);
        assert!(s.is_playing);
        assert_eq!(s.duration_sec, 8.0);
    }

    #[test]
    fn switching_to_screenshot_clears_video_loading_and_time() {
        let mut s = PlayerSessionState::default();
        let g1 = s.begin_load("a.mp4");
        s.on_worker_load_started(g1);
        s.apply_event(g1, &PlayerEvent::Duration(12.0), false);
        s.apply_event(g1, &PlayerEvent::FileLoaded, false);

        let g2 = s.begin_load("shot.png");
        assert_ne!(g1, g2);
        assert!(s.is_image);
        assert!(!s.is_loading);
        assert!(!s.is_playing);
        assert_eq!(s.time_display(), "");
    }

    #[test]
    fn logical_to_physical_rect_scales_and_insets() {
        assert_eq!(logical_to_physical_rect(102.0, 148.0, 900.0, 600.0, 1.25, 1.0), (129, 186, 1122, 748));
        assert_eq!(logical_to_physical_rect(0.0, 0.0, 1920.0, 1080.0, 1.0, 0.0), (0, 0, 1920, 1080));
        assert_eq!(logical_to_physical_rect(10.0, 10.0, 1.0, 1.0, 1.0, 1.0), (11, 11, 0, 0));
    }

    #[test]
    fn surface_visibility_hides_when_inactive_minimized_transitioning_or_image() {
        let base = SurfaceVisibility {
            page_is_player: true,
            window_visible: true,
            window_minimized: false,
            fs_transition: false,
            has_clip: true,
            is_image: false,
            video_ready: true,
            width_px: 1280,
            height_px: 720,
        };
        assert!(!SurfaceVisibility { video_ready: false, ..base }.show_native());
        assert!(base.show_native());
        assert!(base.show_overlay());

        assert!(!SurfaceVisibility { page_is_player: false, ..base }.show_native());
        assert!(!SurfaceVisibility { page_is_player: false, ..base }.show_overlay());

        assert!(!SurfaceVisibility { window_minimized: true, ..base }.show_native());
        assert!(!SurfaceVisibility { window_minimized: true, ..base }.show_overlay());

        assert!(!SurfaceVisibility { window_visible: false, ..base }.show_native());
        assert!(!SurfaceVisibility { window_visible: false, ..base }.show_overlay());

        assert!(!SurfaceVisibility { fs_transition: true, ..base }.show_native());
        assert!(!SurfaceVisibility { fs_transition: true, ..base }.show_overlay());

        let img = SurfaceVisibility { is_image: true, ..base };
        assert!(!img.show_native());
        assert!(img.show_overlay());

        let no_clip = SurfaceVisibility { has_clip: false, ..base };
        assert!(!no_clip.show_native());
        assert!(no_clip.show_overlay());
    }

    #[test]
    fn sidebar_totals_sum_durations_and_sizes() {
        let clips = vec![
            crate::app_ctx::SharedClip { duration_ticks: 90 * 10_000_000, file_size: 2_000_000, ..Default::default() },
            crate::app_ctx::SharedClip {
                duration_ticks: 30 * 10_000_000,
                file_size: 500_000,
                is_image: true,
                ..Default::default()
            },
        ];
        assert_eq!(sidebar_totals(&clips), (120, 2_500_000));
    }
}
