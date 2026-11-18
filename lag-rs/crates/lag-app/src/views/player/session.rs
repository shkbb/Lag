//! Playback state of the player page (`PlayerSessionState`) and what the video surface must show.

use lag_player::PlayerEvent;
use std::{path::Path, time::Instant};

pub(super) fn is_image(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| ["png", "jpg", "jpeg"].iter().any(|v| s.eq_ignore_ascii_case(v)))
}

pub(super) fn clock(seconds: f64) -> String {
    let ms = (seconds.max(0.0) * 1000.0) as u64;
    if ms >= 3_600_000 {
        format!("{}:{:02}:{:02}.{:03}", ms / 3_600_000 % 24, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
    } else {
        format!("{}:{:02}.{:03}", ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SurfaceVisibility {
    pub(super) page_is_player: bool,
    pub(super) window_visible: bool,
    pub(super) window_minimized: bool,
    pub(super) fs_transition: bool,
    pub(super) has_clip: bool,
    pub(super) is_image: bool,
    /// mpv has presented the current clip's first frame. Before that the child surface is an un-presented
    /// swapchain that DWM shows as a see-through hole (owner, 2026-11-16).
    pub(super) video_ready: bool,
    pub(super) width_px: i32,
    pub(super) height_px: i32,
}

impl SurfaceVisibility {
    pub(super) fn show_native(&self) -> bool {
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

    pub(super) fn show_overlay(&self) -> bool {
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
pub(super) struct PlayerSessionState {
    pub(super) load_gen: u64,
    pub(super) worker_gen: u64,
    pub(super) awaiting_loaded: bool,
    pub(super) has_clip: bool,
    pub(super) is_image: bool,
    pub(super) is_playing: bool,
    pub(super) is_loading: bool,
    pub(super) path: String,
    pub(super) position_sec: f64,
    pub(super) duration_sec: f64,
    pub(super) norm_position: f32,
    pub(super) sampled: Instant,
    pub(super) speed: f64,
    pub(super) audio_track: i32,
    pub(super) audio_track_count: i32,
    pub(super) error: String,
    pub(super) video_ready: bool,
    /// Stopped (Stop button, page left): the next play must be a fresh load so its events are accepted.
    pub(super) stopped: bool,
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
pub(super) struct StateDelta {
    pub(super) applied: bool,
    pub(super) select_default_track: bool,
}

impl PlayerSessionState {
    pub(super) fn begin_load(&mut self, path: &str) -> u64 {
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

    pub(super) fn begin_stop(&mut self) -> u64 {
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

    pub(super) fn on_worker_load_started(&mut self, gen: u64) {
        self.worker_gen = gen;
    }

    pub(super) fn apply_event(&mut self, event_gen: u64, event: &PlayerEvent, dragging: bool) -> StateDelta {
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

    pub(super) fn interpolated_time(&self) -> f64 {
        let mut time = self.position_sec;
        if self.is_playing {
            time += self.sampled.elapsed().as_secs_f64() * self.speed.max(0.01);
        }
        if self.duration_sec > 0.0 {
            time = time.clamp(0.0, self.duration_sec);
        }
        time
    }

    pub(super) fn time_display(&self) -> String {
        if self.is_image {
            String::new()
        } else {
            format!("{} / {}", clock(self.interpolated_time()), clock(self.duration_sec))
        }
    }
}
