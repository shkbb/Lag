//! Ports of `CarryOverSegment.cs` and `ReplayStitcher.cs`: keep the tail of a capture session that just
//! ended (e.g. the user quit the game) so "save" right afterwards still contains it, and join it with
//! the live session at save time.
//!
//! Fixes vs C#:
//! * the transcode fallback hard-coded NVIDIA encoders (`h264_nvenc`/`hevc_nvenc`), so on AMD/Intel PCs the
//!   first stitch attempt always failed before the slow CPU retry; it now re-encodes with the encoder the
//!   engine is actually using;
//! * `merge_audio` keeps carried tracks that have no live counterpart yet (the microphone track is created
//!   lazily on the first mic buffer, so a save right after a session switch silently dropped the carried
//!   game's microphone audio). Track order is preserved: live order first, then carried-only tracks.

use crate::mux::{self, AudioTrack};
use lag_ffi::ffmpeg as ff;
use lag_ffi::media::{EncodedPacket, StreamSpec};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Instant;

/// Gap inserted between the carried head and the live tail (one 60 fps frame).
pub const SEAM_GAP_US: i64 = 16_000;
/// Bitrate of the mixed "All" track: highest of all tracks, because Windows' player picks the audio track
/// with the highest bitrate, not the default-flagged one (VfrReplayEngine.AllTrackKbps).
pub const ALL_TRACK_KBPS: i32 = 192;
pub const SOURCE_TRACK_KBPS: i32 = 160;

#[derive(Debug, Clone)]
pub struct CarryAudioTrack {
    pub spec: StreamSpec,
    pub packets: Vec<EncodedPacket>,
    pub title: String,
}

#[derive(Debug, Clone)]
pub struct CarryOverSegment {
    pub video_spec: StreamSpec,
    pub video: Vec<EncodedPacket>,
    pub audio: Vec<CarryAudioTrack>,
    pub was_game: bool,
    pub target_name: String,
    pub target_exe: Option<String>,
}

impl CarryOverSegment {
    pub fn is_empty(&self) -> bool {
        self.video.is_empty()
    }
    pub fn start_us(&self) -> i64 {
        self.video.first().map_or(0, |p| p.pts_us)
    }
    pub fn end_us(&self) -> i64 {
        self.video.last().map_or(0, |p| p.pts_us)
    }
    pub fn duration_seconds(&self) -> f64 {
        if self.video.is_empty() {
            0.0
        } else {
            (self.end_us() - self.start_us()) as f64 / 1e6
        }
    }

    /// The last `seconds` of the segment starting on a keyframe (latest keyframe at/before the cutoff;
    /// if every keyframe is newer, the earliest one). `None` when nothing decodable remains.
    /// Returns `Ok(self)` unchanged (as `Trimmed::Same`) when no trimming is needed, so callers can tell
    /// "unchanged" from "replaced" like the C# `ReferenceEquals` check.
    pub fn trimmed_to_last(&self, seconds: f64) -> Trimmed {
        if self.video.is_empty() || seconds <= 0.0 {
            return Trimmed::Gone;
        }
        let cutoff = self.end_us() - (seconds * 1e6) as i64;
        let mut start: Option<usize> = None;
        for (i, p) in self.video.iter().enumerate() {
            if !p.is_keyframe {
                continue;
            }
            if p.pts_us <= cutoff {
                start = Some(i);
            } else {
                if start.is_none() {
                    start = Some(i);
                }
                break;
            }
        }
        match start {
            None => Trimmed::Gone, // no keyframe at all: undecodable
            Some(0) => Trimmed::Same,
            Some(s) => {
                let video = self.video[s..].to_vec();
                let new_start = video[0].pts_us;
                let audio = self
                    .audio
                    .iter()
                    .filter_map(|t| {
                        let kept: Vec<_> = t.packets.iter().filter(|p| p.pts_us >= new_start).cloned().collect();
                        (!kept.is_empty()).then(|| CarryAudioTrack { packets: kept, ..t.clone() })
                    })
                    .collect();
                Trimmed::New(CarryOverSegment { video, audio, ..self.clone_meta() })
            }
        }
    }

    fn clone_meta(&self) -> CarryOverSegment {
        CarryOverSegment {
            video_spec: self.video_spec.clone(),
            video: Vec::new(),
            audio: Vec::new(),
            was_game: self.was_game,
            target_name: self.target_name.clone(),
            target_exe: self.target_exe.clone(),
        }
    }

    pub fn video_rebased(&self) -> Vec<EncodedPacket> {
        let b = self.start_us();
        self.video.iter().map(|p| p.shifted(-b)).collect()
    }

    pub fn audio_rebased(&self, track: &CarryAudioTrack) -> Vec<EncodedPacket> {
        let b = self.start_us();
        track.packets.iter().filter(|p| p.pts_us >= b).map(|p| p.shifted(-b)).collect()
    }
}

/// Result of [`CarryOverSegment::trimmed_to_last`].
#[derive(Debug)]
pub enum Trimmed {
    /// Nothing to trim; keep the existing segment.
    Same,
    /// A shorter segment replaces the existing one.
    New(CarryOverSegment),
    /// Nothing decodable remains; drop the carry-over.
    Gone,
}

pub fn can_concat_direct(a: &StreamSpec, b: &StreamSpec) -> bool {
    a.codec_id == b.codec_id && a.width == b.width && a.height == b.height && a.extra_data == b.extra_data
}

pub fn can_concat_audio(a: &StreamSpec, b: &StreamSpec) -> bool {
    a.codec_id == b.codec_id
        && a.sample_rate == b.sample_rate
        && a.channels == b.channels
        && a.extra_data == b.extra_data
}

pub fn shift(packets: &[EncodedPacket], offset_us: i64) -> Vec<EncodedPacket> {
    packets.iter().map(|p| p.shifted(offset_us)).collect()
}

/// Owned audio track for building a save.
#[derive(Debug, Clone)]
pub struct OwnedAudioTrack {
    pub spec: StreamSpec,
    pub packets: Vec<EncodedPacket>,
    pub title: String,
}

impl OwnedAudioTrack {
    pub fn as_mux(&self) -> AudioTrack<'_> {
        AudioTrack { spec: self.spec.clone(), packets: &self.packets, title: self.title.clone() }
    }
}

pub fn as_mux_tracks(t: &[OwnedAudioTrack]) -> Vec<AudioTrack<'_>> {
    t.iter().map(OwnedAudioTrack::as_mux).collect()
}

/// Joins carried (head) and live audio tracks by title; live tracks are shifted by `offset_us`.
pub fn merge_audio(head: &[OwnedAudioTrack], live: &[OwnedAudioTrack], offset_us: i64) -> Vec<OwnedAudioTrack> {
    let mut merged: Vec<OwnedAudioTrack> = Vec::with_capacity(live.len().max(head.len()));
    let mut used_head = vec![false; head.len()];
    for l in live {
        let shifted = shift(&l.packets, offset_us);
        let h = head.iter().enumerate().find(|(i, h)| {
            !used_head[*i] && h.title.eq_ignore_ascii_case(&l.title) && can_concat_audio(&h.spec, &l.spec)
        });
        match h {
            Some((i, h)) => {
                used_head[i] = true;
                let mut packets = h.packets.clone();
                packets.extend(shifted);
                merged.push(OwnedAudioTrack { spec: l.spec.clone(), packets, title: l.title.clone() });
            }
            None => merged.push(OwnedAudioTrack { packets: shifted, ..l.clone() }),
        }
    }
    // Carried-only tracks (see module docs): kept, after the live ones, so track order stays stable.
    for (i, h) in head.iter().enumerate() {
        if !used_head[i] && !merged.iter().any(|m| m.title.eq_ignore_ascii_case(&h.title)) {
            log::info!("[CarryOver] keeping carried-only audio track '{}'.", h.title);
            merged.push(h.clone());
        }
    }
    merged
}

fn ffmpeg_exe() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("ffmpeg").join("ffmpeg.exe")))
        .unwrap_or_else(|| "ffmpeg.exe".into())
}

/// Re-encodes both halves into one file when they cannot be concatenated directly (different size or
/// codec config). `video_encoder` is the FFmpeg name of the encoder the engine records with.
/// One half of a stitched clip: its video stream format, packets and audio tracks.
#[derive(Clone, Copy)]
pub struct StitchPart<'a> {
    pub spec: &'a StreamSpec,
    pub video: &'a [EncodedPacket],
    pub audio: &'a [OwnedAudioTrack],
}

/// Encoder settings for the re-encode: bitrate, quality preset and the FFmpeg encoder the engine records with.
#[derive(Clone, Copy)]
pub struct StitchEncoder<'a> {
    pub bitrate_kbps: i32,
    pub preset: i32,
    pub video_encoder: &'a str,
}

pub fn transcode_stitch(out_path: &str, head: StitchPart, live: StitchPart, encoder: StitchEncoder) -> bool {
    transcode_stitch_with(&ffmpeg_exe(), out_path, head, live, encoder)
}

pub fn transcode_stitch_with(
    ffmpeg: &Path,
    out_path: &str,
    head: StitchPart,
    live: StitchPart,
    encoder: StitchEncoder,
) -> bool {
    let StitchPart { spec: head_spec, video: head_video, audio: head_audio } = head;
    let StitchPart { spec: live_spec, video: live_video, audio: live_audio } = live;
    let StitchEncoder { bitrate_kbps, preset, video_encoder } = encoder;
    if !ffmpeg.exists() {
        log::info!("[Stitch] bundled ffmpeg.exe missing ({}) \u{2014} cannot re-encode.", ffmpeg.display());
        return false;
    }
    let tmp_dir = std::env::temp_dir().join(format!("Lag-stitch-{:08x}", rand_u32()));
    if std::fs::create_dir_all(&tmp_dir).is_err() {
        return false;
    }
    let result = (|| {
        let head_tmp = tmp_dir.join("head.mp4");
        let live_tmp = tmp_dir.join("live.mp4");
        // The concat filter needs the same stream count per segment: keep the common prefix of tracks.
        let a_count = head_audio.len().min(live_audio.len());
        if head_audio.len() != live_audio.len() {
            log::info!(
                "[Stitch] audio track count differs ({} vs {}) \u{2014} keeping {a_count}.",
                head_audio.len(),
                live_audio.len()
            );
        }
        if !mux::write(&head_tmp.to_string_lossy(), head_spec, head_video, &as_mux_tracks(&head_audio[..a_count])) {
            return false;
        }
        if !mux::write(&live_tmp.to_string_lossy(), live_spec, live_video, &as_mux_tracks(&live_audio[..a_count])) {
            return false;
        }
        let (w, h) = (live_spec.width, live_spec.height);
        let mut fc = format!(
            "[0:v]scale={w}:{h}:force_original_aspect_ratio=decrease,pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,format=yuv420p[v0];\
             [1:v]scale={w}:{h},setsar=1,format=yuv420p[v1];[v0]"
        );
        for i in 0..a_count {
            fc += &format!("[0:a:{i}]");
        }
        fc += "[v1]";
        for i in 0..a_count {
            fc += &format!("[1:a:{i}]");
        }
        fc += &format!("concat=n=2:v=1:a={a_count}[v]");
        for i in 0..a_count {
            fc += &format!("[a{i}]");
        }
        let build = |venc_args: &[String]| -> Vec<String> {
            let mut a: Vec<String> = vec!["-y".into(), "-hide_banner".into(), "-loglevel".into(), "error".into()];
            a.extend([
                "-i".into(),
                head_tmp.to_string_lossy().into_owned(),
                "-i".into(),
                live_tmp.to_string_lossy().into_owned(),
            ]);
            a.extend(["-filter_complex".into(), fc.clone(), "-map".into(), "[v]".into()]);
            for i in 0..a_count {
                a.extend(["-map".into(), format!("[a{i}]")]);
            }
            a.extend(venc_args.iter().cloned());
            a.extend(["-b:v".into(), format!("{}k", bitrate_kbps.max(1000))]);
            if a_count > 0 {
                a.extend(["-c:a".into(), "aac".into(), "-b:a".into(), format!("{SOURCE_TRACK_KBPS}k")]);
                a.extend(["-b:a:0".into(), format!("{ALL_TRACK_KBPS}k")]);
            }
            for (i, t) in live_audio[..a_count].iter().enumerate() {
                let title = t.title.replace('"', "");
                if !title.is_empty() {
                    a.extend([format!("-metadata:s:a:{i}"), format!("title={title}")]);
                }
            }
            a.push(out_path.to_string());
            a
        };
        let hw = stitch_encoder_args(video_encoder, live_spec.codec_id, preset);
        if run(ffmpeg, &build(&hw)) {
            return true;
        }
        log::info!("[Stitch] hardware re-encode failed \u{2014} retrying with libx264.");
        run(ffmpeg, &build(&["-c:v".into(), "libx264".into(), "-preset".into(), "veryfast".into()]))
    })();
    let _ = std::fs::remove_dir_all(&tmp_dir);
    result
}

/// Encoder args for the stitch re-encode: the engine's own encoder when it is a hardware one of the same
/// codec, else the codec's x264 equivalent.
fn stitch_encoder_args(video_encoder: &str, codec: ff::AVCodecID, preset: i32) -> Vec<String> {
    let preset = preset.clamp(1, 7);
    if video_encoder.contains("nvenc") {
        vec!["-c:v".into(), video_encoder.into(), "-preset".into(), format!("p{preset}")]
    } else if video_encoder.contains("amf") || video_encoder.contains("qsv") {
        vec!["-c:v".into(), video_encoder.into()]
    } else if codec == ff::AV_CODEC_ID_H264 {
        vec!["-c:v".into(), "libx264".into(), "-preset".into(), "veryfast".into()]
    } else {
        // Same mapping as C# for non-hardware encoders of other codecs.
        let name = match codec {
            ff::AV_CODEC_ID_AV1 => "av1_nvenc",
            ff::AV_CODEC_ID_HEVC => "hevc_nvenc",
            _ => "h264_nvenc",
        };
        vec!["-c:v".into(), name.into(), "-preset".into(), format!("p{preset}")]
    }
}

fn rand_u32() -> u32 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut h = RandomState::new().build_hasher();
    h.write_u128(Instant::now().elapsed().as_nanos());
    h.finish() as u32
}

fn run(exe: &Path, args: &[String]) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let t = Instant::now();
    let out = Command::new(exe)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output();
    match out {
        Err(_) => {
            log::info!("[Stitch] failed to start ffmpeg.");
            false
        }
        Ok(o) if !o.status.success() => {
            let err = String::from_utf8_lossy(&o.stderr);
            let err = err.trim();
            let tail = if err.len() <= 400 { err } else { &err[err.len() - 400..] };
            log::info!("[Stitch] ffmpeg exit {}: {tail}", o.status.code().unwrap_or(-1));
            false
        }
        Ok(_) => {
            log::info!("[Stitch] re-encoded and joined in {}ms.", t.elapsed().as_millis());
            true
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vspec() -> StreamSpec {
        StreamSpec {
            codec_id: ff::AV_CODEC_ID_H264,
            is_video: true,
            width: 4,
            height: 4,
            sample_rate: 0,
            channels: 0,
            extra_data: vec![1],
        }
    }
    fn aspec() -> StreamSpec {
        StreamSpec {
            codec_id: ff::AV_CODEC_ID_AAC,
            is_video: false,
            width: 0,
            height: 0,
            sample_rate: 48000,
            channels: 2,
            extra_data: vec![2],
        }
    }
    fn pk(ms: i64, key: bool) -> EncodedPacket {
        EncodedPacket::new(vec![0u8], ms * 1000, ms * 1000, key)
    }

    fn seg() -> CarryOverSegment {
        // 10 s, keyframes every 2 s, 10 fps; audio every 100 ms.
        CarryOverSegment {
            video_spec: vspec(),
            video: (0..100).map(|i| pk(i * 100, i % 20 == 0)).collect(),
            audio: vec![CarryAudioTrack {
                spec: aspec(),
                packets: (0..100).map(|i| pk(i * 100, true)).collect(),
                title: "All".into(),
            }],
            was_game: true,
            target_name: "Counter-Strike 2".into(),
            target_exe: Some("cs2.exe".into()),
        }
    }

    #[test]
    fn trim_semantics_match_csharp() {
        let s = seg();
        assert!(matches!(s.trimmed_to_last(20.0), Trimmed::Same));
        assert!(matches!(s.trimmed_to_last(0.0), Trimmed::Gone));
        match s.trimmed_to_last(5.0) {
            // end 9.9 s, cutoff 4.9 s -> latest keyframe <= cutoff is 4 s.
            Trimmed::New(t) => {
                assert_eq!(t.start_us(), 4_000_000);
                assert_eq!(t.audio[0].packets[0].pts_us, 4_000_000);
                assert_eq!(t.video_rebased()[0].pts_us, 0);
            }
            other => panic!("{other:?}"),
        }
        let mut nokey = seg();
        nokey.video.iter_mut().for_each(|p| p.is_keyframe = false);
        assert!(matches!(nokey.trimmed_to_last(5.0), Trimmed::Gone));
    }

    #[test]
    fn merge_keeps_carried_only_tracks_in_stable_order() {
        let t = |title: &str, ms: &[i64]| OwnedAudioTrack {
            spec: aspec(),
            packets: ms.iter().map(|&m| pk(m, true)).collect(),
            title: title.into(),
        };
        let head = vec![t("All", &[0, 10]), t("System", &[0]), t("Microphone", &[0, 5])];
        let live = vec![t("All", &[0]), t("System", &[0])];
        let m = merge_audio(&head, &live, 1000);
        let titles: Vec<_> = m.iter().map(|x| x.title.as_str()).collect();
        assert_eq!(titles, ["All", "System", "Microphone"]);
        assert_eq!(m[0].packets.len(), 3);
        assert_eq!(m[0].packets[2].pts_us, 1000); // live pts 0 shifted by the 1000 us offset
        assert_eq!(m[2].packets.len(), 2);
    }

    #[test]
    fn stitch_uses_engine_encoder() {
        assert_eq!(stitch_encoder_args("h264_amf", ff::AV_CODEC_ID_H264, 4), vec!["-c:v", "h264_amf"]);
        assert_eq!(stitch_encoder_args("hevc_nvenc", ff::AV_CODEC_ID_HEVC, 9)[3], "p7");
        assert_eq!(stitch_encoder_args("libx264", ff::AV_CODEC_ID_H264, 4)[1], "libx264");
    }
}
