//! Animated pictures and videos of themes (docs/spec/themes.md §21): `AnimatedImage { source: @image-url("girl.gif"); }`,
//! `Video { source: "videos/bg.mp4"; }`.
//!
//! A theme places the picture as any other one (the compiler resolves its path, the editor's picture picker works,
//! the first frame shows even where nothing plays it); Lag plays it. The `Media` global of every window gets a
//! `frame(picture, tick, speed)` callback and a `tick` Lag bumps whenever an animation moves on: every
//! `AnimatedImage` on screen then asks for its frame again.
//!
//! - Each animated file has one player, shared by every place that shows it (a frame on every clip card is one
//!   decoder, not one per card). Its frames are decoded on a thread of its own; a small animation is kept whole
//!   after its first loop (no more decoding), a big one keeps streaming (memory stays a few frames).
//! - It plays only while it is drawn: a picture on another page, in a hidden or minimised window, does not ask
//!   for frames, and its player stops (and is dropped a minute later). Nothing plays while something full screen
//!   (a game) is in front of Lag.
//! - A video is decoded by Lag's own FFmpeg (the one that makes the library's thumbnails), one process per file,
//!   at most 1920 px across and `max-fps` frames a second, read from its output as raw frames. A stopped video is
//!   simply not read: FFmpeg waits on the full pipe and costs nothing. A WebM with transparency (VP8/VP9 alpha)
//!   keeps it.
//! - The same on every OS: the picture decoders are Rust (image), FFmpeg runs everywhere; only "something full
//!   screen in front" asks the system.

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{Image, Rgb8Pixel, Rgba8Pixel, SharedPixelBuffer};
use slint_interpreter::{ComponentInstance, Value};

/// One animation is kept whole (every frame decoded once) up to this size...
const KEEP_ONE: usize = 64 << 20;
/// ...and all of them together up to this; bigger ones stream.
const KEEP_ALL: usize = 192 << 20;
/// A frame larger than this is not played (the first frame stays).
const MAX_PIXELS: u64 = 4096 * 4096;
/// Not drawn this long after a tick: the picture is off screen, its player stops.
const UNSEEN: Duration = Duration::from_millis(800);
/// Stopped this long: the player (its thread, its frames) is dropped.
const DROP_AFTER: Duration = Duration::from_secs(60);
/// How often players are looked at while something plays.
const STEP: Duration = Duration::from_millis(8);
/// How often "is a game still in front" is asked while it is.
const GAME_CHECK: Duration = Duration::from_millis(500);
/// Nothing on screen plays: a look now and then to drop the players left over.
const IDLE: Duration = Duration::from_secs(5);
/// A video is decoded at most this many pixels across (Lag scales it up to the window).
const VIDEO_SIDE: u32 = 1920;

/// One decoded frame: pictures and transparent videos have alpha, other videos do not (a quarter less to copy).
enum Pixels {
    Rgba(SharedPixelBuffer<Rgba8Pixel>),
    Rgb(SharedPixelBuffer<Rgb8Pixel>),
}

impl Pixels {
    fn bytes(&self) -> usize {
        match self {
            Pixels::Rgba(p) => p.width() as usize * p.height() as usize * 4,
            Pixels::Rgb(p) => p.width() as usize * p.height() as usize * 3,
        }
    }
    fn image(self) -> Image {
        match self {
            Pixels::Rgba(p) => Image::from_rgba8(p),
            Pixels::Rgb(p) => Image::from_rgb8(p),
        }
    }
}

enum Msg {
    Frame(Pixels, Duration),
    /// the last frame of a loop was sent; the next one is the first again
    End,
}

enum Feed {
    /// not animated (one frame, not a GIF/WebP/APNG, an error): the picture as it is
    Still,
    /// frames from the decoder thread; `kept` collects the first loop while it stays small enough
    Stream { rx: mpsc::Receiver<Msg>, kept: Option<(Vec<(Image, Duration)>, usize)> },
    /// every frame decoded, played from memory
    Kept { frames: Vec<(Image, Duration)>, at: usize, bytes: usize },
}

struct Player {
    feed: Feed,
    shown: Image,
    /// when the next frame is due
    due: Instant,
    speed: f32,
    /// the last tick it was drawn at
    seen_tick: i32,
    stopped_at: Option<Instant>,
    /// a video told `playing: false`: its first frame stays
    hold: bool,
    /// a frame of its own is shown (not the placeholder)
    started: bool,
}

#[derive(Default)]
struct Media {
    players: HashMap<PathBuf, Player>,
    tick: i32,
    bumped_at: Option<Instant>,
    /// windows whose `Media.tick` follows, by a number of their own (dead ones are dropped)
    sinks: Vec<(u64, slint::Weak<ComponentInstance>)>,
    next_sink: u64,
    timer: Option<slint::Timer>,
    /// how often the timer fires now
    pace: Duration,
    game: Option<(Instant, bool)>,
    /// the theme's top folder (built): `Video { source }` paths start there
    root: Option<PathBuf>,
}

thread_local! {
    static MEDIA: RefCell<Media> = RefCell::new(Media::default());
}

/// The theme whose videos play now (its build folder; set when a theme is compiled).
pub fn set_root(dir: &std::path::Path) {
    MEDIA.with(|m| m.borrow_mut().root = Some(dir.to_path_buf()));
}

/// Lets a window play animated pictures (every twin window and the editor's canvas).
pub fn attach(inst: &ComponentInstance) {
    // a UI without AnimatedImage has no Media global: nothing to do
    if inst
        .set_global_callback("Media", "frame", |args| {
            let (Some(Value::Image(source)), Some(Value::Number(tick))) = (args.first(), args.get(1)) else {
                return Value::Image(Image::default());
            };
            let speed = match args.get(2) {
                Some(Value::Number(s)) => *s as f32,
                _ => 1.0,
            };
            Value::Image(frame(source, *tick as i32, speed))
        })
        .is_err()
    {
        return;
    }
    let _ = inst.set_global_callback("Media", "video", |args| {
        let num = |i: usize, d: f64| match args.get(i) {
            Some(Value::Number(n)) => *n,
            _ => d,
        };
        let Some(Value::String(source)) = args.first() else { return Value::Image(Image::default()) };
        let playing = !matches!(args.get(4), Some(Value::Bool(false)));
        Value::Image(video(source, num(1, 0.0) as i32, num(2, 1.0) as f32, num(3, 30.0) as u32, playing))
    });
    MEDIA.with(|m| {
        let mut m = m.borrow_mut();
        let _ = inst.set_global_property("Media", "tick", Value::Number(m.tick as f64));
        let id = m.next_sink;
        m.next_sink += 1;
        m.sinks.push((id, slint::ComponentHandle::as_weak(inst)));
    });
}

/// The frame of `source` to show now; asked by a drawn `AnimatedImage` after every tick.
fn frame(source: &Image, tick: i32, speed: f32) -> Image {
    let Some(path) = source.path().map(|p| p.to_path_buf()) else { return source.clone() };
    let shown = MEDIA.with(|m| {
        let mut m = m.borrow_mut();
        let now = Instant::now();
        let p = m.players.entry(path).or_insert_with_key(|path| start(path, source, now));
        (seen(p, tick, speed, now), !matches!(p.feed, Feed::Still))
    });
    if shown.1 {
        ensure_timer();
    }
    shown.0
}

/// A drawn picture or video asked for its frame at `tick`: it is on screen, it plays on.
fn seen(p: &mut Player, tick: i32, speed: f32, now: Instant) -> Image {
    p.seen_tick = p.seen_tick.max(tick);
    p.speed = if speed.is_finite() && speed > 0.0 { speed.min(100.0) } else { 1.0 };
    if p.stopped_at.take().is_some() {
        p.due = now;
    }
    p.shown.clone()
}

/// The frame of the theme's video `source` (a path from the theme's top folder) to show now.
fn video(source: &str, tick: i32, speed: f32, max_fps: u32, playing: bool) -> Image {
    // only a video of the theme: a plain relative path, no way out of its folder
    let ext = source.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if lag_theme_file::check_path(source).is_err() || !["mp4", "webm"].contains(&ext.as_str()) {
        return Image::default();
    }
    let shown = MEDIA.with(|m| {
        let mut m = m.borrow_mut();
        let root = m.root.clone()?;
        let now = Instant::now();
        let p = m.players.entry(root.join(source)).or_insert_with_key(|path| start_video(path, max_fps, now));
        p.hold = !playing;
        Some((seen(p, tick, speed, now), !matches!(p.feed, Feed::Still)))
    });
    let Some((image, live)) = shown else { return Image::default() };
    if live {
        ensure_timer();
    }
    image
}

fn placeholder(feed: Feed, shown: Image, now: Instant) -> Player {
    Player { feed, shown, due: now, speed: 1.0, seen_tick: 0, stopped_at: None, hold: false, started: false }
}

/// A player for a theme's video: FFmpeg on a thread of its own.
fn start_video(path: &std::path::Path, max_fps: u32, now: Instant) -> Player {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if !path.is_file() {
        log::warn!("[Media] the theme has no video {}", path.display());
        return placeholder(Feed::Still, Image::default(), now);
    }
    let (tx, rx) = mpsc::sync_channel(2);
    let file = path.to_path_buf();
    let max_fps = max_fps.clamp(1, 60);
    match std::thread::Builder::new().name("lag-video".into()).spawn(move || play_video(&file, max_fps, &tx)) {
        Ok(_) => log::info!("[Media] {name} plays"),
        Err(e) => log::warn!("[Media] {name}: no decoder thread: {e}"),
    }
    placeholder(Feed::Stream { rx, kept: None }, Image::default(), now)
}

/// What FFmpeg needs to know before it decodes: size, frame rate, codec, transparency.
#[derive(Debug, PartialEq)]
struct Probe {
    width: u32,
    height: u32,
    fps: f64,
    codec: String,
    alpha: bool,
}

/// `ffprobe -of default=nw=1` lines -> a Probe.
fn parse_probe(text: &str) -> Option<Probe> {
    let mut p = Probe { width: 0, height: 0, fps: 0.0, codec: String::new(), alpha: false };
    let rate = |v: &str| {
        let (n, d) = v.split_once('/').unwrap_or((v, "1"));
        let (n, d) = (n.trim().parse::<f64>().ok()?, d.trim().parse::<f64>().ok()?);
        (d > 0.0 && n > 0.0).then_some(n / d)
    };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k.trim() {
            "width" => p.width = v.trim().parse().unwrap_or(0),
            "height" => p.height = v.trim().parse().unwrap_or(0),
            "codec_name" => p.codec = v.trim().to_owned(),
            // the average rate first; the base rate where it is unknown
            "avg_frame_rate" => p.fps = rate(v).unwrap_or(p.fps),
            "r_frame_rate" if p.fps == 0.0 => p.fps = rate(v).unwrap_or(0.0),
            "TAG:alpha_mode" | "TAG:ALPHA_MODE" => p.alpha = v.trim() == "1",
            _ => {}
        }
    }
    (p.width > 0 && p.height > 0).then_some(p)
}

/// The decoded size: at most `VIDEO_SIDE` across, the same shape, even numbers (what the scaler wants).
fn video_size(w: u32, h: u32) -> (u32, u32) {
    let k = (f64::from(VIDEO_SIDE) / f64::from(w.max(h))).min(1.0);
    let even = |v: f64| ((v.round() as u32) & !1).max(2);
    (even(f64::from(w) * k), even(f64::from(h) * k))
}

fn tool(name: &str) -> std::process::Command {
    let exe = if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() };
    let mut c = std::process::Command::new(crate::services::ffmpeg::resolve_tool(&exe));
    c.stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // no console window; below normal priority (a background never takes time from a game)
        c.creation_flags(0x0800_0000 | 0x0000_4000);
    }
    c
}

/// The video thread: FFmpeg decodes the file in a loop into raw frames; they go to the player one by one.
fn play_video(file: &std::path::Path, max_fps: u32, tx: &mpsc::SyncSender<Msg>) {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let probed = tool("ffprobe")
        .args(["-v", "error", "-select_streams", "v:0", "-show_entries"])
        .arg("stream=codec_name,width,height,avg_frame_rate,r_frame_rate:stream_tags=alpha_mode")
        .args(["-of", "default=nw=1"])
        .arg(file)
        .output();
    let Some(probe) = probed.ok().and_then(|o| parse_probe(&String::from_utf8_lossy(&o.stdout))) else {
        log::warn!("[Media] {name}: not a video FFmpeg can read");
        return;
    };
    let (w, h) = video_size(probe.width, probe.height);
    let fps = if probe.fps > 0.0 { probe.fps.min(f64::from(max_fps)) } else { f64::from(max_fps) };
    let mut cmd = tool("ffmpeg");
    cmd.args(["-hide_banner", "-loglevel", "error", "-nostdin"]);
    // FFmpeg's own VP8/VP9 decoders drop the transparency: libvpx keeps it
    if probe.alpha {
        cmd.args(["-c:v", if probe.codec == "vp8" { "libvpx" } else { "libvpx-vp9" }]);
    }
    cmd.args(["-stream_loop", "-1", "-i"]).arg(file);
    cmd.args(["-an", "-sn", "-vf", &format!("fps={fps:.3},scale={w}:{h}:flags=bicubic")]);
    cmd.args(["-pix_fmt", if probe.alpha { "rgba" } else { "rgb24" }, "-f", "rawvideo", "-"]);
    cmd.stdout(std::process::Stdio::piped());
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            log::warn!("[Media] {name}: FFmpeg did not start: {e}");
            return;
        }
    };
    log::info!("[Media] {name}: {}x{} {} at {fps:.0} fps{}", w, h, probe.codec, if probe.alpha { ", transparent" } else { "" });
    let delay = Duration::from_secs_f64(1.0 / fps);
    let bpp = if probe.alpha { 4 } else { 3 };
    let mut buf = vec![0u8; (w * h) as usize * bpp];
    if let Some(mut out) = child.stdout.take() {
        use std::io::Read;
        while out.read_exact(&mut buf).is_ok() {
            let px = if probe.alpha {
                Pixels::Rgba(SharedPixelBuffer::clone_from_slice(&buf, w, h))
            } else {
                Pixels::Rgb(SharedPixelBuffer::clone_from_slice(&buf, w, h))
            };
            // the player is gone: FFmpeg goes too
            if tx.send(Msg::Frame(px, delay)).is_err() {
                break;
            }
        }
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// A player for the file of `source`: animated GIF, WebP or APNG gets a decoder thread.
fn start(path: &std::path::Path, source: &Image, now: Instant) -> Player {
    let still = |feed| placeholder(feed, source.clone(), now);
    let Ok(bytes) = std::fs::read(path) else { return still(Feed::Still) };
    let bytes: Arc<[u8]> = bytes.into();
    let Some(kind) = kind(&bytes) else { return still(Feed::Still) };
    // a still PNG / WebP is not worth a thread
    if frames(&bytes, kind).is_none() {
        return still(Feed::Still);
    }
    let (tx, rx) = mpsc::sync_channel(2);
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let spawned = std::thread::Builder::new().name("lag-anim".into()).spawn(move || decode(&bytes, kind, &tx));
    if let Err(e) = spawned {
        log::warn!("[Media] {name}: no decoder thread: {e}");
        return still(Feed::Still);
    }
    log::info!("[Media] {name} plays");
    still(Feed::Stream { rx, kept: Some((Vec::new(), 0)) })
}

#[derive(Clone, Copy)]
enum Kind {
    Gif,
    Webp,
    Png,
}

fn kind(b: &[u8]) -> Option<Kind> {
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some(Kind::Gif)
    } else if b.len() > 12 && &b[..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        Some(Kind::Webp)
    } else if b.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Kind::Png)
    } else {
        None
    }
}

/// The frames of an animated file (None: not animated or not readable).
fn frames(bytes: &Arc<[u8]>, kind: Kind) -> Option<image::Frames<'static>> {
    use image::codecs::{gif::GifDecoder, png::PngDecoder, webp::WebPDecoder};
    use image::AnimationDecoder;
    let c = Cursor::new(bytes.clone());
    match kind {
        Kind::Gif => GifDecoder::new(c).ok().map(|d| d.into_frames()),
        Kind::Webp => WebPDecoder::new(c).ok().filter(|d| d.has_animation()).map(|d| d.into_frames()),
        Kind::Png => {
            let d = PngDecoder::new(c).ok()?;
            if !d.is_apng().ok()? {
                return None;
            }
            d.apng().ok().map(|d| d.into_frames())
        }
    }
}

/// The decoder thread: frame after frame, loop after loop, until the player is gone.
fn decode(bytes: &Arc<[u8]>, kind: Kind, tx: &mpsc::SyncSender<Msg>) {
    loop {
        let Some(frames) = frames(bytes, kind) else { return };
        let mut sent = 0;
        for f in frames {
            let Ok(f) = f else { break };
            let (num, den) = f.delay().numer_denom_ms();
            let ms = num.checked_div(den).unwrap_or(0);
            // as browsers do: a GIF frame of 0-10 ms is shown for 100 ms
            let ms = match kind {
                Kind::Gif if ms <= 10 => 100,
                _ => ms.max(10),
            };
            let buf = f.into_buffer();
            let (w, h) = buf.dimensions();
            if u64::from(w) * u64::from(h) > MAX_PIXELS {
                return;
            }
            let px = Pixels::Rgba(SharedPixelBuffer::clone_from_slice(buf.as_raw(), w, h));
            if tx.send(Msg::Frame(px, Duration::from_millis(u64::from(ms)))).is_err() {
                return;
            }
            sent += 1;
        }
        if sent == 0 || tx.send(Msg::End).is_err() {
            return;
        }
    }
}

fn ensure_timer() {
    MEDIA.with(|m| {
        let mut m = m.borrow_mut();
        let m = &mut *m;
        let timer = m.timer.get_or_insert_with(slint::Timer::default);
        if !timer.running() {
            timer.start(slint::TimerMode::Repeated, STEP, step);
            m.pace = STEP;
        } else if m.pace == IDLE {
            timer.set_interval(STEP);
            m.pace = STEP;
        }
    });
}

/// Moves every playing animation on; bumps the tick when one did.
fn step() {
    let now = Instant::now();
    let game = game_in_front(now);
    let (moved, tick, sinks) = MEDIA.with(|m| {
        let mut m = m.borrow_mut();
        let m = &mut *m;
        let (tick, bumped_at) = (m.tick, m.bumped_at);
        let mut moved = false;
        let mut playing = false;
        m.players.retain(|_, p| {
            if matches!(p.feed, Feed::Still) {
                return true;
            }
            // drawn at the last tick? (a tick it has not answered for a while: off screen)
            let unseen = p.seen_tick < tick && bumped_at.is_some_and(|b| now - b > UNSEEN);
            if unseen || game {
                let at = *p.stopped_at.get_or_insert(now);
                if now - at > DROP_AFTER {
                    return false;
                }
                // a game in front: the player waits (and goes on where it was)
                return true;
            }
            playing = true;
            p.stopped_at = None;
            // a held video: its first frame, then nothing more
            if p.hold && p.started {
                return true;
            }
            if now >= p.due {
                moved |= advance(p, now);
            }
            true
        });
        // keep the first loops of new players while the total of kept animations allows
        let mut total: usize = m.players.values().map(|p| if let Feed::Kept { bytes, .. } = &p.feed { *bytes } else { 0 }).sum();
        for p in m.players.values_mut() {
            if let Feed::Stream { kept, .. } = &mut p.feed {
                let b = kept.as_ref().map_or(0, |k| k.1);
                if total + b > KEEP_ALL {
                    *kept = None;
                } else {
                    total += b;
                }
            }
        }
        if moved {
            m.tick = m.tick.wrapping_add(1);
            m.bumped_at = Some(now);
        }
        // nothing on screen plays: rare wake-ups until the left-over players are dropped, then none until a drawn
        // AnimatedImage asks again
        let left = m.players.values().any(|p| !matches!(p.feed, Feed::Still));
        let pace = if game { GAME_CHECK } else if playing { STEP } else { IDLE };
        if let Some(t) = &m.timer {
            if !left {
                t.stop();
            } else if pace != m.pace {
                t.set_interval(pace);
                m.pace = pace;
            }
        }
        (moved, m.tick, if moved { m.sinks.clone() } else { Vec::new() })
    });
    if !moved {
        return;
    }
    // outside the borrow: setting the tick may draw at once (and ask for frames)
    let mut dead = Vec::new();
    for (id, w) in sinks {
        match w.upgrade() {
            Some(inst) => {
                let _ = inst.set_global_property("Media", "tick", Value::Number(tick as f64));
            }
            None => dead.push(id),
        }
    }
    if !dead.is_empty() {
        MEDIA.with(|m| m.borrow_mut().sinks.retain(|(id, _)| !dead.contains(id)));
    }
}

/// The next frame of a player. false: none ready yet (asked again at the next step).
fn advance(p: &mut Player, now: Instant) -> bool {
    let delay = match &mut p.feed {
        Feed::Still => return false,
        Feed::Kept { frames, at, .. } => {
            *at = (*at + 1) % frames.len();
            p.shown = frames[*at].0.clone();
            frames[*at].1
        }
        Feed::Stream { rx, kept } => match rx.try_recv() {
            Ok(Msg::Frame(px, d)) => {
                let size = px.bytes();
                let img = px.image();
                if let Some((frames, bytes)) = kept {
                    *bytes += size;
                    if *bytes > KEEP_ONE {
                        *kept = None;
                    } else {
                        frames.push((img.clone(), d));
                    }
                }
                p.shown = img;
                p.started = true;
                d
            }
            Ok(Msg::End) => {
                // the first loop is in memory: play it from there (the thread ends at its next send)
                if let Some((frames, bytes)) = kept.take() {
                    p.feed = if frames.len() > 1 {
                        Feed::Kept { at: frames.len() - 1, frames, bytes }
                    } else {
                        Feed::Still
                    };
                    return advance(p, now);
                }
                return false;
            }
            Err(mpsc::TryRecvError::Empty) => return false,
            Err(mpsc::TryRecvError::Disconnected) => {
                p.feed = Feed::Still;
                return false;
            }
        },
    };
    let d = delay.div_f32(p.speed);
    // late by more than a frame (a busy moment, a long pause): go on from now, do not rush to catch up
    p.due = if p.due + d < now { now + d } else { p.due + d };
    true
}

/// Something full screen that is not Lag is in front (a game, a video): nothing plays. Asked at most every
/// `GAME_CHECK` while it holds, at every step otherwise (a cheap call).
fn game_in_front(now: Instant) -> bool {
    let cached = MEDIA.with(|m| m.borrow().game);
    if let Some((at, v)) = cached {
        if now - at < GAME_CHECK {
            return v;
        }
    }
    let v = full_screen_in_front();
    let was = MEDIA.with(|m| m.borrow_mut().game.replace((now, v)).is_some_and(|(_, w)| w));
    if v != was {
        log::info!("[Media] {}", if v { "something full screen is in front: animations wait" } else { "animations play again" });
    }
    v
}

#[cfg(windows)]
fn full_screen_in_front() -> bool {
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST};
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetForegroundWindow, GetWindowLongW, GetWindowRect, GetWindowThreadProcessId, IsZoomed, GWL_STYLE, WS_CAPTION,
    };
    unsafe {
        let fg = GetForegroundWindow();
        if fg.is_invalid() {
            return false;
        }
        let mut pid = 0u32;
        GetWindowThreadProcessId(fg, Some(&mut pid));
        if pid == std::process::id() {
            return false;
        }
        // the desktop itself covers the monitor too
        let mut class = [0u16; 64];
        let n = GetClassNameW(fg, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..n]);
        if class == "Progman" || class == "WorkerW" {
            return false;
        }
        // a maximised window with a frame (a browser, an editor) covers a monitor without a taskbar too: not a game.
        // Full screen = borderless over the whole monitor (games, a video on F11).
        let style = GetWindowLongW(fg, GWL_STYLE) as u32;
        if IsZoomed(fg).as_bool() || style & WS_CAPTION.0 == WS_CAPTION.0 {
            return false;
        }
        let mut r = RECT::default();
        if GetWindowRect(fg, &mut r).is_err() {
            return false;
        }
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        if !GetMonitorInfoW(MonitorFromWindow(fg, MONITOR_DEFAULTTONEAREST), &mut info).as_bool() {
            return false;
        }
        let m = info.rcMonitor;
        r.left <= m.left && r.top <= m.top && r.right >= m.right && r.bottom >= m.bottom
    }
}

/// Linux: the same rule through the window manager (_NET_ACTIVE_WINDOW + _NET_WM_STATE_FULLSCREEN on X11, the
/// foreign-toplevel protocol on Wayland) comes with the Linux build; until then animations play there always.
#[cfg(not(windows))]
fn full_screen_in_front() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gif(frames: u32) -> Vec<u8> {
        use image::codecs::gif::{GifEncoder, Repeat};
        let mut out = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut out);
            enc.set_repeat(Repeat::Infinite).unwrap();
            for i in 0..frames {
                let img = image::RgbaImage::from_pixel(8, 6, image::Rgba([(i * 60) as u8, 0, 0, 255]));
                let delay = image::Delay::from_numer_denom_ms(50, 1);
                enc.encode_frame(image::Frame::from_parts(img, 0, 0, delay)).unwrap();
            }
        }
        out
    }

    #[test]
    fn animated_files_are_told_by_their_bytes() {
        let g: Arc<[u8]> = gif(3).into();
        assert!(matches!(kind(&g), Some(Kind::Gif)));
        assert_eq!(frames(&g, Kind::Gif).unwrap().count(), 3);
        // a still PNG is not played (no thread for it)
        let mut png = Vec::new();
        image::RgbaImage::from_pixel(4, 4, image::Rgba([1, 2, 3, 255]))
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        let png: Arc<[u8]> = png.into();
        assert!(matches!(kind(&png), Some(Kind::Png)));
        assert!(frames(&png, Kind::Png).is_none());
        assert!(kind(b"not a picture").is_none());
    }

    #[test]
    fn the_decoder_loops_with_an_end_between_loops() {
        let g: Arc<[u8]> = gif(2).into();
        let (tx, rx) = mpsc::sync_channel(1);
        let t = std::thread::spawn(move || decode(&g, Kind::Gif, &tx));
        let mut seen = Vec::new();
        for _ in 0..6 {
            seen.push(match rx.recv().unwrap() {
                Msg::Frame(Pixels::Rgba(px), d) => {
                    assert_eq!((px.width(), px.height()), (8, 6));
                    format!("f{}", d.as_millis())
                }
                Msg::End => "end".into(),
                Msg::Frame(Pixels::Rgb(_), _) => "rgb".into(),
            });
        }
        assert_eq!(seen, ["f50", "f50", "end", "f50", "f50", "end"]);
        // the player gone: the thread ends
        drop(rx);
        t.join().unwrap();
    }

    #[test]
    fn a_video_is_probed_and_sized() {
        let p = parse_probe("codec_name=vp9\nwidth=3840\nheight=2160\nr_frame_rate=60/1\navg_frame_rate=60000/1001\nTAG:alpha_mode=1\n").unwrap();
        assert_eq!((p.width, p.height, p.codec.as_str(), p.alpha), (3840, 2160, "vp9", true));
        assert!((p.fps - 59.94).abs() < 0.01);
        // an unknown average rate: the base one
        let p = parse_probe("codec_name=h264\nwidth=1280\nheight=720\nr_frame_rate=30/1\navg_frame_rate=0/0\n").unwrap();
        assert_eq!((p.fps, p.alpha), (30.0, false));
        assert!(parse_probe("codec_name=png\n").is_none());
        // at most 1920 across, the same shape, even
        assert_eq!(video_size(3840, 2160), (1920, 1080));
        assert_eq!(video_size(1080, 1920), (1080, 1920));
        assert_eq!(video_size(1281, 721), (1280, 720));
        assert_eq!(video_size(640, 360), (640, 360));
    }
}
