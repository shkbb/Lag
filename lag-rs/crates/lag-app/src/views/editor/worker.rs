//! Background threads of the editor: the mpv control worker, probing a clip, timeline / filter thumbnails.
//! They talk to the UI thread only through `UiMessage`.

use super::{model::*, preview::*, unique_hex_id};
use crate::services::ffmpeg::{self, ProbeResult};
use lag_player::{MpvPlayer, PlayerEvent};
use slint::{Image, Rgba8Pixel, SharedPixelBuffer};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

pub(super) struct Pixels {
    width: u32,
    height: u32,
    bytes: Vec<u8>,
}

impl Pixels {
    fn decode(path: &Path, thumb: bool) -> Option<Self> {
        let mut img = image::open(path).ok()?;
        if thumb {
            img = img.thumbnail(320, 180);
        }
        let rgba = img.to_rgba8();
        Some(Self { width: rgba.width(), height: rgba.height(), bytes: rgba.into_raw() })
    }

    pub(super) fn into_image(self) -> Image {
        let buf = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&self.bytes, self.width, self.height);
        Image::from_rgba8(buf)
    }
}

#[derive(Debug)]
pub(super) enum MpvCommand {
    /// The native video surface exists: create mpv on it (player v2 engine, owner 2026-11-17).
    InitNative(isize),
    Load(String),
    /// Load a montage segment: start at its trim start, then play or stay paused.
    LoadAt {
        path: String,
        start: f64,
        play: bool,
    },
    Play,
    Pause(bool),
    Stop,
    Seek(f64, bool),
    Speed(f64),
    Vf(String),
    /// glsl-shader-opts for PREVIEW_SHADER
    Look(String),
    Crop(Option<(f64, f64, f64, f64)>),
    Rotate(i32),
    /// mpv video equalizer brightness (-100 = black): the reveal fade of a freshly loaded clip.
    Brightness(i64),
    Audio(bool, usize, i32),
}

impl MpvCommand {
    fn supersedes(&self, older: &Self) -> bool {
        matches!(
            (self, older),
            (Self::Seek(..), Self::Seek(..))
                | (Self::Vf(_), Self::Vf(_))
                | (Self::Look(_), Self::Look(_))
                | (Self::Crop(_), Self::Crop(_))
                | (Self::Rotate(_), Self::Rotate(_))
                | (Self::Brightness(_), Self::Brightness(_))
        )
    }
}

pub(super) enum UiMessage {
    MpvReady(Arc<MpvPlayer>),
    MpvEvent(PlayerEvent),
    ProbeDone(u64, String, ProbeResult),
    TimelineThumb(u64, usize, Pixels),
    FilterThumb(u64, usize, Pixels),
    ExportProgress(u64, i32),
    ExportFinished(u64, Result<String, Option<String>>),
    /// Clips probed for the montage: (montage_gen, [(path, probe)], replace the montage instead of adding)
    MontageProbed(u64, Vec<(String, ProbeResult)>, bool), // Ok(filename), Err(None) = cancelled, Err(Some(err))
}

/// Editor engine = the player's v2 engine (owner, 2026-11-17: "the editor is dead"): mpv draws straight into a
/// native child window (vo=gpu-next, d3d11, hwdec=d3d11va) instead of a GL texture inside the Slint scene.
/// Commands wait until `InitNative` brings the surface.
pub(super) fn mpv_worker(rx: mpsc::Receiver<MpvCommand>, tx: mpsc::Sender<UiMessage>) {
    let mut player: Option<Arc<MpvPlayer>> = None;
    let mut pending = VecDeque::new();
    // current hwdec mode: filters (vf) need frames in system memory, plain playback stays zero-copy
    let mut copy_decode = false;
    let mut last_vf: Option<String> = None;
    let mut last_look: Option<String> = None;
    let mut last_crop: Option<String> = None;
    let mut last_rotate: Option<i32> = None;
    loop {
        let cmd = if player.is_some() && !pending.is_empty() {
            pending.pop_front().unwrap()
        } else {
            match rx.recv() {
                Ok(c) => c,
                Err(_) => break,
            }
        };
        if let MpvCommand::InitNative(wid) = cmd {
            if player.is_none() && wid != 0 {
                let events = tx.clone();
                match MpvPlayer::prewarm_native(wid, move |e| {
                    let _ = events.send(UiMessage::MpvEvent(e));
                })
                .recv()
                {
                    Ok(Ok(p)) => {
                        let p = Arc::new(p);
                        // the preview stretches the video to the reframe box (C#: Image Stretch=Fill)
                        let _ = p.command(&["set", "keepaspect", "no"]);
                        let _ = p.command(&["set", "background-color", "#07090C"]);
                        let shader = std::env::temp_dir().join("Lag-editor").join("lag_look.glsl");
                        let _ = std::fs::create_dir_all(shader.parent().unwrap());
                        if std::fs::write(&shader, PREVIEW_SHADER).is_ok() {
                            if let Err(e) = p.command(&["set", "glsl-shaders", &shader.to_string_lossy()]) {
                                log::warn!("[Editor] preview shader not loaded: {e}");
                            }
                        }
                        let _ = tx.send(UiMessage::MpvReady(p.clone()));
                        player = Some(p);
                    }
                    res => {
                        let err = match res {
                            Ok(Err(e)) => e.to_string(),
                            _ => "mpv native worker unavailable".into(),
                        };
                        let _ = tx.send(UiMessage::MpvEvent(PlayerEvent::Error(err)));
                    }
                }
            }
            continue;
        }
        let Some(p) = player.clone() else {
            pending.push_back(cmd);
            continue;
        };
        let mut cmd = cmd;
        while let Ok(next) = rx.try_recv() {
            if pending.is_empty() && next.supersedes(&cmd) {
                cmd = next;
            } else {
                pending.push_back(next);
            }
        }
        let player = &p;
        let report_pause =
            matches!(cmd, MpvCommand::Play | MpvCommand::Pause(_) | MpvCommand::Load(_) | MpvCommand::LoadAt { .. });
        let res = (|| -> lag_player::Result<()> {
            match cmd {
                MpvCommand::InitNative(_) => {}
                MpvCommand::Load(path) => {
                    log::info!("[Editor] mpv load");
                    last_crop = None;
                    player.load(&path)?;
                    player.play()?;
                }
                MpvCommand::LoadAt { path, start, play } => {
                    log::info!("[Editor] mpv load at {start:.2}s (play={play})");
                    last_crop = None;
                    player.load_at(&path, start)?;
                    player.pause(!play)?;
                }
                MpvCommand::Play => {
                    player.play()?;
                }
                MpvCommand::Pause(p) => {
                    player.pause(p)?;
                }
                MpvCommand::Stop => {
                    player.stop()?;
                }
                MpvCommand::Seek(pos_frac, exact) => {
                    if player.property("seekable").as_deref() == Ok("yes") {
                        let dur = player.property("duration").ok().and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                        let target = pos_frac.clamp(0.0, 1.0) * dur;
                        if exact {
                            player.seek_exact(target)?;
                        } else {
                            player.seek_keyframe(target)?;
                        }
                    }
                }
                MpvCommand::Speed(s) => {
                    player.set_speed(s)?;
                }
                MpvCommand::Vf(vf) => {
                    let want_copy = !vf.is_empty();
                    if want_copy != copy_decode {
                        copy_decode = want_copy;
                        let _ = player.command(&["set", "hwdec", if want_copy { "d3d11va-copy" } else { "d3d11va" }]);
                    }
                    // re-setting the same chain restarts the filters (a visible hitch on every play)
                    if last_vf.as_deref() != Some(vf.as_str()) {
                        player.command(&["set", "vf", &vf])?;
                        last_vf = Some(vf);
                    }
                }
                MpvCommand::Look(opts) => {
                    if last_look.as_deref() != Some(opts.as_str()) {
                        player.command(&["set", "glsl-shader-opts", &opts])?;
                        last_look = Some(opts);
                    }
                }
                MpvCommand::Crop(crop) => {
                    // video-crop is in source pixels: needs the loaded file's size (re-sent on FileLoaded)
                    let size = (|| -> Option<(f64, f64)> {
                        let w = player.property("width").ok()?.parse::<f64>().ok()?;
                        let h = player.property("height").ok()?.parse::<f64>().ok()?;
                        (w > 0.0 && h > 0.0).then_some((w, h))
                    })();
                    let value = match (crop, size) {
                        (Some((x, y, w, h)), Some((sw, sh))) => format!(
                            "{}x{}+{}+{}",
                            (w * sw).round().max(2.0) as i64,
                            (h * sh).round().max(2.0) as i64,
                            (x * sw).round().max(0.0) as i64,
                            (y * sh).round().max(0.0) as i64
                        ),
                        (Some(_), None) => return Ok(()),
                        (None, _) => String::new(),
                    };
                    if last_crop.as_deref() != Some(value.as_str()) {
                        player.command(&["set", "video-crop", &value])?;
                        last_crop = Some(value);
                    }
                }
                MpvCommand::Rotate(deg) => {
                    if last_rotate != Some(deg) {
                        player.command(&["set", "video-rotate", &deg.to_string()])?;
                        last_rotate = Some(deg);
                    }
                }
                MpvCommand::Brightness(v) => {
                    player.command(&["set", "brightness", &v.to_string()])?;
                }
                MpvCommand::Audio(mute, stream_index, volume) => {
                    player.set_mute(mute)?;
                    if !mute {
                        let _ = player.select_audio_track(stream_index);
                        player.set_volume(volume as f64)?;
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = res {
            log::warn!("[Editor] mpv op failed: {e}");
        }
        if report_pause {
            if let Ok(v) = p.property("pause") {
                let _ = tx.send(UiMessage::MpvEvent(PlayerEvent::Paused(v == "yes")));
            }
        }
    }
    if let Some(p) = player {
        let _ = p.stop();
    }
}

pub(super) fn spawn_probe_and_thumbs(
    gen: u64,
    clip_path: String,
    initial_duration: f64,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<UiMessage>,
) {
    // Probe thread
    let clip_p = clip_path.clone();
    let cancel_p = cancel.clone();
    let tx_p = tx.clone();
    let _ = std::thread::Builder::new().name("Lag.EditorProbe".into()).spawn(move || {
        if cancel_p.load(Ordering::Acquire) {
            return;
        }
        let probed = ffmpeg::probe(&clip_p);
        if cancel_p.load(Ordering::Acquire) {
            return;
        }
        match probed {
            Some(res) => {
                let _ = tx_p.send(UiMessage::ProbeDone(gen, clip_p, res));
            }
            None => log::debug!("[Editor] ffprobe failed for {clip_p}"),
        }
    });

    // Sequential thumbnail & filter preview thread
    let _ = std::thread::Builder::new().name("Lag.EditorThumbs".into()).spawn(move || {
        let temp_dir = std::env::temp_dir().join("Lag-editor");
        let _ = std::fs::create_dir_all(&temp_dir);
        let mut dur = initial_duration.max(1.0);

        // Best-effort quick duration query if initial_duration <= 1.0
        if dur <= 1.0 {
            if let Some(res) = ffmpeg::probe(&clip_path).filter(|r| r.duration_sec > 0.0) {
                dur = res.duration_sec;
            }
        }

        const COUNT: usize = 10;
        for i in 0..COUNT {
            if cancel.load(Ordering::Acquire) {
                return;
            }
            let t = dur * (i as f64 + 0.5) / COUNT as f64;
            let out_png = temp_dir.join(format!("thumb-{}.png", unique_hex_id()));
            let args = vec![
                "-ss".to_string(),
                format!("{t}"),
                "-i".to_string(),
                clip_path.clone(),
                "-frames:v".to_string(),
                "1".to_string(),
                "-vf".to_string(),
                "scale=160:-2".to_string(),
                "-y".to_string(),
                out_png.to_string_lossy().into_owned(),
            ];
            if ffmpeg::run_simple(&args, &cancel) == Ok(0) && out_png.exists() {
                if let Some(px) = Pixels::decode(&out_png, false) {
                    let _ = tx.send(UiMessage::TimelineThumb(gen, i, px));
                }
            }
            let _ = std::fs::remove_file(&out_png);
        }

        // Filter previews
        if cancel.load(Ordering::Acquire) {
            return;
        }
        let base_png = temp_dir.join(format!("fbase-{}.png", unique_hex_id()));
        let base_args = vec![
            "-ss".to_string(),
            format!("{}", dur / 2.0),
            "-i".to_string(),
            clip_path.clone(),
            "-frames:v".to_string(),
            "1".to_string(),
            "-vf".to_string(),
            "scale=128:-2".to_string(),
            "-y".to_string(),
            base_png.to_string_lossy().into_owned(),
        ];
        if ffmpeg::run_simple(&base_args, &cancel) == Ok(0) && base_png.exists() {
            for (idx, filter) in default_filter_options().iter().enumerate() {
                if cancel.load(Ordering::Acquire) {
                    break;
                }
                let mut out_png = base_png.clone();
                if let Some(graph) = filter.graph {
                    out_png = temp_dir.join(format!("f-{}.png", unique_hex_id()));
                    let fargs = vec![
                        "-i".to_string(),
                        base_png.to_string_lossy().into_owned(),
                        "-vf".to_string(),
                        graph.to_string(),
                        "-y".to_string(),
                        out_png.to_string_lossy().into_owned(),
                    ];
                    if ffmpeg::run_simple(&fargs, &cancel) != Ok(0) {
                        let _ = std::fs::remove_file(&out_png);
                        continue;
                    }
                }
                if out_png.exists() {
                    if let Some(px) = Pixels::decode(&out_png, false) {
                        let _ = tx.send(UiMessage::FilterThumb(gen, idx, px));
                    }
                }
                if out_png != base_png {
                    let _ = std::fs::remove_file(&out_png);
                }
            }
        }
        let _ = std::fs::remove_file(&base_png);
    });
}
