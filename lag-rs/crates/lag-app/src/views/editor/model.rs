//! The edit of one clip as plain data: trim, cuts, audio tracks, speed, frame shape, crop/reframe,
//! rotation, look, captions, undo. No UI and no FFmpeg here (pure, unit-tested).

use super::geometry::*;
use crate::services::ffmpeg::ProbeResult;
use std::path::Path;

pub(super) const MIN_TRIM_GAP_SEC: f64 = 0.2;
pub(super) const MIN_AUDIO_GAP_SEC: f64 = 0.1;
pub(super) const MIN_CUT_LEN_SEC: f64 = 0.05;
pub(super) const CUT_MERGE_TOLERANCE_SEC: f64 = 0.01;
pub(super) const MIN_NORM_SIZE: f64 = 0.05;

#[derive(Debug, Clone, PartialEq)]
pub struct AudioTrack {
    pub stream_index: usize,
    pub name: String,
    pub volume: i32,
    pub is_muted: bool,
    pub is_selected: bool,
    pub start_sec: f64,
    pub end_sec: f64,
    pub clip_duration_sec: f64,
}

impl AudioTrack {
    pub fn new(stream_index: usize, name: impl Into<String>, clip_duration_sec: f64) -> Self {
        let dur = clip_duration_sec.max(0.1);
        Self {
            stream_index,
            name: name.into(),
            volume: 100,
            is_muted: false,
            is_selected: false,
            start_sec: 0.0,
            end_sec: dur,
            clip_duration_sec: dur,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextItem {
    pub text: String,
    pub nx: f64,
    pub ny: f64,
    pub font_percent: i32,
    pub color: String,
}

impl TextItem {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into(), nx: 0.5, ny: 0.12, font_percent: 8, color: "#FFFFFF".to_string() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Cut {
    pub start_sec: f64,
    pub end_sec: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AspectOption {
    pub display: String,
    pub ratio: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SpeedOption {
    pub display: &'static str,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ExportRes {
    pub display: String,
    pub height: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct FilterOption {
    pub name: &'static str,
    pub graph: Option<&'static str>,
}

pub fn default_speed_options() -> Vec<SpeedOption> {
    vec![
        SpeedOption { display: "0.5×", value: 0.5 },
        SpeedOption { display: "1×", value: 1.0 },
        SpeedOption { display: "1.5×", value: 1.5 },
        SpeedOption { display: "2×", value: 2.0 },
    ]
}

pub fn default_aspect_options(native_label: &str) -> Vec<AspectOption> {
    vec![
        AspectOption { display: native_label.to_string(), ratio: None },
        AspectOption { display: "16:9".to_string(), ratio: Some(16.0 / 9.0) },
        AspectOption { display: "9:16".to_string(), ratio: Some(9.0 / 16.0) },
        AspectOption { display: "16:10".to_string(), ratio: Some(16.0 / 10.0) },
        AspectOption { display: "21:9".to_string(), ratio: Some(21.0 / 9.0) },
        AspectOption { display: "4:3".to_string(), ratio: Some(4.0 / 3.0) },
        AspectOption { display: "1:1".to_string(), ratio: Some(1.0) },
    ]
}

pub fn default_export_resolutions(native_label: &str) -> Vec<ExportRes> {
    vec![
        ExportRes { display: native_label.to_string(), height: 0 },
        ExportRes { display: "1080p".to_string(), height: 1080 },
        ExportRes { display: "720p".to_string(), height: 720 },
        ExportRes { display: "480p".to_string(), height: 480 },
    ]
}

pub const EXPORT_FORMATS: &[&str] = &["mp4", "mkv", "mov"];

pub fn default_filter_options() -> Vec<FilterOption> {
    vec![
        FilterOption { name: "Normal", graph: None },
        FilterOption { name: "Grayscale", graph: Some("hue=s=0") },
        FilterOption {
            name: "Sepia",
            graph: Some("colorchannelmixer=.393:.769:.189:0:.349:.686:.168:0:.272:.534:.131"),
        },
        FilterOption { name: "Warm", graph: Some("colortemperature=temperature=4500") },
        FilterOption { name: "Cold", graph: Some("colortemperature=temperature=8500") },
        FilterOption { name: "Vibrant", graph: Some("eq=saturation=1.6:contrast=1.05") },
        FilterOption { name: "Dramatic", graph: Some("eq=contrast=1.35:brightness=-0.06:saturation=0.8") },
        FilterOption { name: "Bright", graph: Some("eq=brightness=0.08:saturation=1.15") },
    ]
}

#[derive(Debug, Clone, PartialEq)]
pub struct EditSnap {
    pub trim_start: f64,
    pub trim_end: f64,
    pub speed: String,
    pub aspect: usize,
    pub filter: usize,
    pub crop_x: f64,
    pub crop_y: f64,
    pub crop_w: f64,
    pub crop_h: f64,
    pub vid_x: f64,
    pub vid_y: f64,
    pub vid_w: f64,
    pub vid_h: f64,
    pub rotation: i32,
    pub flip_h: bool,
    pub flip_v: bool,
    pub color_b: i32,
    pub color_c: i32,
    pub color_s: i32,
    pub cuts: Vec<Cut>,
    pub texts: Vec<TextItem>,
}

/// Display names of the audio lanes (localized by the caller).
#[derive(Debug, Clone)]
pub struct TrackNames {
    pub single: String,
    pub system: String,
    pub mic: String,
}

#[derive(Debug, Clone)]
pub struct EditModel {
    /// Audio lanes take the video's trim (default on; owner 2026-11-17). Off = lanes are trimmed on their own.
    pub audio_follows_video: bool,
    pub current_clip_path: Option<String>,
    pub clip_name: String,
    pub duration_sec: f64,
    pub trim_start_sec: f64,
    pub trim_end_sec: f64,
    pub position: f64,
    pub is_playing: bool,
    pub is_video_selected: bool,

    pub speed_options: Vec<SpeedOption>,
    pub selected_speed_index: Option<usize>,
    pub speed_text: String,

    pub aspect_options: Vec<AspectOption>,
    pub selected_aspect_index: usize,
    pub source_width: i32,
    pub source_height: i32,

    pub crop_tool_active: bool,
    pub crop_x: f64,
    pub crop_y: f64,
    pub crop_w: f64,
    pub crop_h: f64,

    pub vid_x: f64,
    pub vid_y: f64,
    pub vid_w: f64,
    pub vid_h: f64,

    pub rotation: i32,
    pub flip_h: bool,
    pub flip_v: bool,

    pub filter_options: Vec<FilterOption>,
    pub selected_filter_index: usize,
    pub color_brightness: i32,
    pub color_contrast: i32,
    pub color_saturation: i32,

    pub audio_tracks: Vec<AudioTrack>,
    pub cuts: Vec<Cut>,
    pub pending_cut_start: Option<f64>,

    pub texts: Vec<TextItem>,
    pub selected_text_index: Option<usize>,

    pub export_resolutions: Vec<ExportRes>,
    pub selected_export_res_index: usize,
    pub selected_export_format_index: usize,
    pub export_as_gif: bool,

    pub undo_stack: Vec<EditSnap>,
    pub suppress_undo: bool,
}

impl EditModel {
    pub fn new(native_label: &str) -> Self {
        Self {
            audio_follows_video: true,
            current_clip_path: None,
            clip_name: String::new(),
            duration_sec: 1.0,
            trim_start_sec: 0.0,
            trim_end_sec: 1.0,
            position: 0.0,
            is_playing: false,
            is_video_selected: false,
            speed_options: default_speed_options(),
            selected_speed_index: Some(1),
            speed_text: "1".to_string(),
            aspect_options: default_aspect_options(native_label),
            selected_aspect_index: 0,
            source_width: 0,
            source_height: 0,
            crop_tool_active: false,
            crop_x: 0.0,
            crop_y: 0.0,
            crop_w: 1.0,
            crop_h: 1.0,
            vid_x: 0.0,
            vid_y: 0.0,
            vid_w: 1.0,
            vid_h: 1.0,
            rotation: 0,
            flip_h: false,
            flip_v: false,
            filter_options: default_filter_options(),
            selected_filter_index: 0,
            color_brightness: 100,
            color_contrast: 100,
            color_saturation: 100,
            audio_tracks: Vec::new(),
            cuts: Vec::new(),
            pending_cut_start: None,
            texts: Vec::new(),
            selected_text_index: None,
            export_resolutions: default_export_resolutions(native_label),
            selected_export_res_index: 0,
            selected_export_format_index: 0,
            export_as_gif: false,
            undo_stack: Vec::new(),
            suppress_undo: false,
        }
    }

    pub fn effective_speed(&self) -> f64 {
        parse_effective_speed(&self.speed_text)
    }

    pub fn selected_aspect(&self) -> &AspectOption {
        &self.aspect_options[self.selected_aspect_index.min(self.aspect_options.len().saturating_sub(1))]
    }

    pub fn selected_filter(&self) -> &FilterOption {
        &self.filter_options[self.selected_filter_index.min(self.filter_options.len().saturating_sub(1))]
    }

    pub fn selected_export_res(&self) -> &ExportRes {
        &self.export_resolutions[self.selected_export_res_index.min(self.export_resolutions.len().saturating_sub(1))]
    }

    pub fn selected_export_format(&self) -> &'static str {
        EXPORT_FORMATS[self.selected_export_format_index.min(EXPORT_FORMATS.len() - 1)]
    }

    pub fn source_aspect_raw(&self) -> f64 {
        if self.source_width > 0 && self.source_height > 0 {
            self.source_width as f64 / self.source_height as f64
        } else {
            16.0 / 9.0
        }
    }

    pub fn source_aspect(&self) -> f64 {
        if self.source_width > 0 && self.source_height > 0 {
            (self.source_width as f64 * self.crop_w) / (self.source_height as f64 * self.crop_h)
        } else {
            16.0 / 9.0
        }
    }

    pub fn frame_aspect(&self) -> f64 {
        match self.selected_aspect().ratio {
            Some(r) if r > 0.0 => r,
            _ => self.source_aspect(),
        }
    }

    pub fn display_source_aspect_raw(&self) -> f64 {
        let a = self.source_aspect_raw();
        if self.rotation == 90 || self.rotation == 270 {
            1.0 / a
        } else {
            a
        }
    }

    pub fn display_frame_aspect(&self) -> f64 {
        let a = self.frame_aspect();
        if self.rotation == 90 || self.rotation == 270 {
            1.0 / a
        } else {
            a
        }
    }

    /// The exported picture is taller than wide (after crop, frame shape and rotation).
    pub fn is_portrait_output(&self) -> bool {
        self.display_frame_aspect() < 0.999
    }

    pub fn is_cropped(&self) -> bool {
        self.crop_x > 0.002 || self.crop_y > 0.002 || self.crop_w < 0.998 || self.crop_h < 0.998
    }

    pub fn cover_placement(&self) -> (f64, f64, f64, f64) {
        let sa = self.source_aspect();
        let fa = self.frame_aspect();
        if sa >= fa {
            let w = sa / fa;
            ((1.0 - w) / 2.0, 0.0, w, 1.0)
        } else {
            let h = fa / sa;
            (0.0, (1.0 - h) / 2.0, 1.0, h)
        }
    }

    pub fn is_reframed(&self) -> bool {
        let (x, y, w, h) = self.cover_placement();
        (self.vid_x - x).abs() > 0.003
            || (self.vid_y - y).abs() > 0.003
            || (self.vid_w - w).abs() > 0.003
            || (self.vid_h - h).abs() > 0.003
    }

    pub fn has_color_adjust(&self) -> bool {
        self.color_brightness != 100 || self.color_contrast != 100 || self.color_saturation != 100
    }

    pub fn set_crop(&mut self, x: f64, y: f64, w: f64, h: f64) {
        let w = w.clamp(MIN_NORM_SIZE, 1.0);
        let h = h.clamp(MIN_NORM_SIZE, 1.0);
        let x = x.clamp(0.0, 1.0 - w);
        let y = y.clamp(0.0, 1.0 - h);
        self.crop_x = x;
        self.crop_y = y;
        self.crop_w = w;
        self.crop_h = h;
        self.reset_reframe();
    }

    pub fn set_video_rect(&mut self, x: f64, y: f64, w: f64, h: f64) {
        self.vid_w = w.clamp(MIN_NORM_SIZE, 6.0);
        self.vid_h = h.clamp(MIN_NORM_SIZE, 6.0);
        self.vid_x = x.clamp(-3.0, 3.0);
        self.vid_y = y.clamp(-3.0, 3.0);
    }

    pub fn reset_reframe(&mut self) {
        let (x, y, w, h) = self.cover_placement();
        self.set_video_rect(x, y, w, h);
    }

    pub fn reset_crop(&mut self) {
        self.set_crop(0.0, 0.0, 1.0, 1.0);
    }

    pub fn active_norm(&self) -> (f64, f64, f64, f64) {
        if self.crop_tool_active {
            (self.crop_x, self.crop_y, self.crop_w, self.crop_h)
        } else {
            (self.vid_x, self.vid_y, self.vid_w, self.vid_h)
        }
    }

    pub fn display_edit_norm(&self) -> (f64, f64, f64, f64) {
        let (x, y, w, h) = self.active_norm();
        forward_transform_rect(x, y, w, h, self.flip_h, self.flip_v, self.rotation)
    }

    pub fn push_undo(&mut self) {
        if self.suppress_undo {
            return;
        }
        self.undo_stack.push(EditSnap {
            trim_start: self.trim_start_sec,
            trim_end: self.trim_end_sec,
            speed: self.speed_text.clone(),
            aspect: self.selected_aspect_index,
            filter: self.selected_filter_index,
            crop_x: self.crop_x,
            crop_y: self.crop_y,
            crop_w: self.crop_w,
            crop_h: self.crop_h,
            vid_x: self.vid_x,
            vid_y: self.vid_y,
            vid_w: self.vid_w,
            vid_h: self.vid_h,
            rotation: self.rotation,
            flip_h: self.flip_h,
            flip_v: self.flip_v,
            color_b: self.color_brightness,
            color_c: self.color_contrast,
            color_s: self.color_saturation,
            cuts: self.cuts.clone(),
            texts: self.texts.clone(),
        });
    }

    pub fn undo(&mut self) -> bool {
        let Some(s) = self.undo_stack.pop() else {
            return false;
        };
        self.suppress_undo = true;
        self.selected_aspect_index = s.aspect.min(self.aspect_options.len().saturating_sub(1));
        self.selected_filter_index = s.filter.min(self.filter_options.len().saturating_sub(1));
        self.set_speed_text(s.speed);
        self.trim_start_sec = s.trim_start;
        self.trim_end_sec = s.trim_end;
        if self.duration_sec > 0.0 {
            self.position = (self.trim_start_sec / self.duration_sec).clamp(0.0, 1.0);
        }
        self.crop_x = s.crop_x;
        self.crop_y = s.crop_y;
        self.crop_w = s.crop_w;
        self.crop_h = s.crop_h;
        self.vid_x = s.vid_x;
        self.vid_y = s.vid_y;
        self.vid_w = s.vid_w;
        self.vid_h = s.vid_h;
        self.rotation = s.rotation;
        self.flip_h = s.flip_h;
        self.flip_v = s.flip_v;
        self.color_brightness = s.color_b;
        self.color_contrast = s.color_c;
        self.color_saturation = s.color_s;
        self.cuts = s.cuts;
        self.selected_text_index = None;
        self.texts = s.texts;
        self.suppress_undo = false;
        true
    }

    pub fn set_speed_text(&mut self, text: String) {
        self.speed_text = text;
        let s = self.effective_speed();
        self.selected_speed_index = self.speed_options.iter().position(|o| (o.value - s).abs() < 0.0001);
    }

    pub fn add_cut(&mut self, a: f64, b: f64) {
        let mut list = self.cuts.clone();
        list.push(Cut { start_sec: a.min(b), end_sec: a.max(b) });
        list.sort_by(|x, y| x.start_sec.total_cmp(&y.start_sec));

        let mut merged: Vec<Cut> = Vec::with_capacity(list.len());
        for c in list {
            if let Some(last) = merged.last_mut() {
                if c.start_sec <= last.end_sec + CUT_MERGE_TOLERANCE_SEC {
                    last.end_sec = last.end_sec.max(c.end_sec);
                    continue;
                }
            }
            merged.push(c);
        }
        self.cuts = merged;
    }

    pub fn remove_cut(&mut self, idx: usize) -> bool {
        if idx >= self.cuts.len() {
            return false;
        }
        self.push_undo();
        self.cuts.remove(idx);
        true
    }

    pub fn clear_cuts(&mut self) {
        self.pending_cut_start = None;
        self.cuts.clear();
    }

    pub fn kept_segments(&self, start: f64, end: f64) -> Vec<(f64, f64)> {
        let mut sorted = self.cuts.clone();
        sorted.sort_by(|a, b| a.start_sec.total_cmp(&b.start_sec));
        let mut segs = Vec::new();
        let mut cur = start;
        for c in sorted {
            let cs = c.start_sec.clamp(start, end);
            let ce = c.end_sec.clamp(start, end);
            if ce <= cur {
                continue;
            }
            if cs > cur {
                segs.push((cur, cs));
            }
            cur = cur.max(ce);
        }
        if cur < end - CUT_MERGE_TOLERANCE_SEC {
            segs.push((cur, end));
        }
        segs
    }

    pub fn reset_for_clip(&mut self, path: &str, duration_sec: f64, default_track_name: &str) {
        self.current_clip_path = Some(path.to_string());
        self.clip_name = Path::new(path).file_stem().and_then(|s| s.to_str()).unwrap_or("").to_string();
        self.undo_stack.clear();
        self.suppress_undo = true;
        self.trim_start_sec = 0.0;
        self.duration_sec = duration_sec.max(1.0);
        self.trim_end_sec = self.duration_sec;
        self.position = 0.0;
        self.set_speed_text("1".to_string());
        self.source_width = 0;
        self.source_height = 0;
        self.crop_tool_active = false;
        self.selected_aspect_index = 0;
        self.set_crop(0.0, 0.0, 1.0, 1.0);
        self.rotation = 0;
        self.flip_h = false;
        self.flip_v = false;
        self.color_brightness = 100;
        self.color_contrast = 100;
        self.color_saturation = 100;
        self.clear_cuts();
        self.selected_text_index = None;
        self.texts.clear();
        self.selected_filter_index = 0;
        self.is_video_selected = false;
        self.audio_tracks = vec![AudioTrack::new(0, default_track_name, self.duration_sec)];
        self.suppress_undo = false;
    }

    pub fn snap_to_track_edges(&self, sec: f64, width_px: f64) -> f64 {
        if width_px <= 0.0 {
            return sec;
        }
        let threshold = 10.0 / width_px * self.duration_sec;
        for t in &self.audio_tracks {
            if (sec - t.start_sec).abs() <= threshold {
                return t.start_sec;
            }
            if (sec - t.end_sec).abs() <= threshold {
                return t.end_sec;
            }
        }
        sec
    }

    /// Every audio lane takes the video trim range (when `audio_follows_video`).
    /// Has ffprobe's result been applied yet (size, length, audio tracks)? Re-opening an edited montage segment
    /// must not reset its tracks or framing.
    pub fn is_probed(&self) -> bool {
        self.source_width > 0 && self.source_height > 0
    }

    /// Fills in what ffprobe found: picture size (re-centres the frame), real length (keeps an end trim that was
    /// at the very end there), and the audio tracks the recorder writes: one mixed track, or All + System + Mic
    /// (the editor edits System and Mic and leaves the All mix out), or two separate tracks.
    pub fn apply_probe(&mut self, res: &ProbeResult, names: &TrackNames) {
        self.source_width = res.video_width;
        self.source_height = res.video_height;
        self.reset_reframe();
        let end_at_max = (self.trim_end_sec - self.duration_sec).abs() < 0.05;
        if res.duration_sec > 0.0 {
            self.duration_sec = res.duration_sec.max(1.0);
        }
        if end_at_max || self.trim_end_sec > self.duration_sec {
            self.trim_end_sec = self.duration_sec;
        }
        self.trim_start_sec = self.trim_start_sec.min(self.trim_end_sec);
        self.set_audio_layout(res.audio_streams, names);
    }

    /// The audio lanes for a file with `streams` audio streams (Lag's own layouts: one mix; system + mic; or a
    /// mix followed by system + mic, where the mix gets no lane of its own).
    pub fn set_audio_layout(&mut self, streams: usize, names: &TrackNames) {
        let dur = self.duration_sec;
        self.audio_tracks.clear();
        match streams {
            0 => {}
            1 => self.audio_tracks.push(AudioTrack::new(0, names.single.clone(), dur)),
            2 => {
                self.audio_tracks.push(AudioTrack::new(0, names.system.clone(), dur));
                self.audio_tracks.push(AudioTrack::new(1, names.mic.clone(), dur));
            }
            _ => {
                self.audio_tracks.push(AudioTrack::new(1, names.system.clone(), dur));
                self.audio_tracks.push(AudioTrack::new(2, names.mic.clone(), dur));
            }
        }
        self.link_audio_to_trim();
    }

    pub fn link_audio_to_trim(&mut self) {
        if !self.audio_follows_video {
            return;
        }
        let (a, b) = (self.trim_start_sec, self.trim_end_sec);
        for t in &mut self.audio_tracks {
            let dur = if t.clip_duration_sec > 0.0 { t.clip_duration_sec } else { self.duration_sec };
            t.start_sec = a.clamp(0.0, dur);
            t.end_sec = b.clamp(t.start_sec, dur);
        }
    }

    pub fn apply_timeline_drag(&mut self, mode: i32, frac: f64, width_px: f64) -> Option<f64> {
        if width_px <= 0.0 || self.duration_sec <= 0.0 {
            return None;
        }
        let sec = frac.clamp(0.0, 1.0) * self.duration_sec;
        match mode {
            1 => {
                // TrimStart
                let snapped = self.snap_to_track_edges(sec, width_px);
                let v = snapped.clamp(0.0, (self.trim_end_sec - MIN_TRIM_GAP_SEC).max(0.0));
                self.trim_start_sec = v;
                if self.is_video_selected {
                    for t in &mut self.audio_tracks {
                        if t.is_selected {
                            t.start_sec = v.clamp(0.0, (t.end_sec - MIN_AUDIO_GAP_SEC).max(0.0));
                        }
                    }
                }
                self.link_audio_to_trim();
                let pos = (v / self.duration_sec).clamp(0.0, 1.0);
                self.position = pos;
                Some(pos)
            }
            2 => {
                // TrimEnd
                let snapped = self.snap_to_track_edges(sec, width_px);
                let v =
                    snapped.clamp((self.trim_start_sec + MIN_TRIM_GAP_SEC).min(self.duration_sec), self.duration_sec);
                self.trim_end_sec = v;
                if self.is_video_selected {
                    for t in &mut self.audio_tracks {
                        if t.is_selected {
                            t.end_sec = v
                                .clamp((t.start_sec + MIN_AUDIO_GAP_SEC).min(t.clip_duration_sec), t.clip_duration_sec);
                        }
                    }
                }
                self.link_audio_to_trim();
                None
            }
            3 => {
                // Scrub
                let lo = self.trim_start_sec / self.duration_sec;
                let hi = self.trim_end_sec / self.duration_sec;
                let pos = frac.clamp(lo, hi);
                self.position = pos;
                Some(pos)
            }
            _ => None,
        }
    }
}

pub fn parse_effective_speed(raw: &str) -> f64 {
    let normalized = raw.trim().replace(',', ".");
    match normalized.parse::<f64>() {
        Ok(v) if v.is_finite() => v.clamp(0.1, 10.0),
        _ => 1.0,
    }
}

pub fn format_time(seconds: f64) -> String {
    let secs = seconds.max(0.0).floor() as u64;
    if secs >= 3600 {
        format!("{}:{:02}:{:02}", secs / 3600, (secs / 60) % 60, secs % 60)
    } else {
        format!("{}:{:02}", (secs / 60) % 60, secs % 60)
    }
}

pub fn interpolate_playhead(
    last_pos: f64,
    elapsed_sec: f64,
    speed: f64,
    duration_sec: f64,
    cuts: &[Cut],
    trim_start_sec: f64,
    trim_end_sec: f64,
) -> f64 {
    if duration_sec <= 0.0 {
        return 0.0;
    }
    let mut predicted = last_pos + elapsed_sec * speed / duration_sec;
    let pred_sec = predicted * duration_sec;
    for c in cuts {
        if pred_sec >= c.start_sec && pred_sec < c.end_sec {
            predicted = c.end_sec / duration_sec;
            break;
        }
    }
    let lo = trim_start_sec / duration_sec;
    let hi = trim_end_sec / duration_sec;
    predicted.clamp(lo.min(hi), hi.max(lo))
}

pub fn should_skip_cut(pos_sec: f64, cuts: &[Cut], trim_end_sec: f64) -> Option<f64> {
    for c in cuts {
        if pos_sec >= c.start_sec - 0.03 && pos_sec < c.end_sec - 0.03 {
            return Some(c.end_sec.min(trim_end_sec));
        }
    }
    None
}
