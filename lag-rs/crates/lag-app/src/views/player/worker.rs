//! The mpv control thread: commands from the UI (coalesced), events and frames back as `Message`s.

use super::session::*;
use lag_player::{MpvPlayer, PlayerEvent};
use std::{
    collections::VecDeque,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::Duration,
};

#[derive(Debug, PartialEq)]
pub(super) enum Command {
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
    pub(super) fn supersedes(&self, older: &Self) -> bool {
        matches!(
            (self, older),
            (Self::Seek(..), Self::Seek(..))
                | (Self::Volume(_), Self::Volume(_))
                | (Self::Mute(_), Self::Mute(_))
                | (Self::Load { .. }, Self::Load { .. })
        )
    }

    pub(super) fn cancelled_by_new_load(&self) -> bool {
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
pub(super) fn coalesce_command_burst(
    first: Command,
    pending: &mut VecDeque<Command>,
    rx: &mpsc::Receiver<Command>,
) -> Command {
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

pub(super) struct Pixels {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) bytes: Vec<u8>,
}

impl Pixels {
    pub(super) fn decode(path: &Path) -> Option<Self> {
        let image = image::open(path).ok()?.to_rgba8();
        Some(Self { width: image.width(), height: image.height(), bytes: image.into_raw() })
    }
    pub(super) fn into_image(self) -> slint::Image {
        let buffer =
            slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(&self.bytes, self.width, self.height);
        slint::Image::from_rgba8(buffer)
    }
}

pub(super) enum Message {
    LoadStarted(u64),
    Event(u64, PlayerEvent),
    Still(u64, String, Option<Pixels>),
}

pub(super) fn worker(rx: mpsc::Receiver<Command>, tx: mpsc::Sender<Message>, latest_gen: Arc<AtomicU64>) {
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
                    let cloud = crate::services::cloud::stream::is_cloud_key(&path);
                    if is_image(&path) {
                        p.stop()?;
                        // a cloud-only screenshot is fetched into a temporary file first
                        let local = if cloud { crate::services::cloud::stream::fetch_image(&path) } else { Some(path.clone().into()) };
                        let pixels = local.and_then(|l| Pixels::decode(&l));
                        let _ = tx.send(Message::Still(gen, path.clone(), pixels));
                    } else if cloud {
                        // a cloud-only clip plays straight from the cloud (signed URL + header, seeking works)
                        let _ = p.command(&["set", "brightness", "0"]);
                        match crate::services::cloud::stream::resolve(&path) {
                            Some(source) => p.load_stream(&source.url, &source.headers)?,
                            None => {
                                let error = PlayerEvent::Error(lag_core::i18n::tr("Cloud_Error_Stream").to_owned());
                                let _ = tx.send(Message::Event(gen, error));
                                return Ok(());
                            }
                        }
                        p.pause(false)?;
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
pub(super) fn log_playback_stats(p: Arc<MpvPlayer>, path: String, gen: u64, latest: Arc<AtomicU64>) {
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
