//! Joining several clips into one video (owner, 2026-11-18: "add clip merging"). No UI here: the editor picks
//! the clips and shows progress, this module decides HOW to join them and runs FFmpeg.
//!
//! Two strategies:
//! * [`Strategy::StreamCopy`] - every clip has the same video codec, size and audio layout (all recordings of
//!   one Lag setup): the FFmpeg concat demuxer copies the packets. Instant, lossless, keeps every audio track.
//! * [`Strategy::Reencode`] - anything else (other resolution, codec, an imported phone video, a clip without
//!   sound): each clip is scaled into the first clip's size (letterboxed, never stretched) and its first audio
//!   track (silence when it has none) is resampled to 48 kHz stereo, then everything is concatenated and encoded
//!   once (NVENC, x264 as the fallback).

use super::ffmpeg::{self, ProbeResult};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// One input clip and what ffprobe found in it.
#[derive(Debug, Clone)]
pub struct MergeSource {
    pub path: String,
    pub info: ProbeResult,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Strategy {
    StreamCopy,
    /// Output frame size (the first clip's, made even).
    Reencode {
        width: i32,
        height: i32,
    },
}

/// Fewer than two clips is not a merge.
pub const MIN_CLIPS: usize = 2;

/// Stream copy only when nothing differs that a decoder would notice mid-file.
pub fn choose_strategy(sources: &[MergeSource]) -> Strategy {
    let first = &sources[0].info;
    let same = sources.iter().all(|s| {
        s.info.video_codec == first.video_codec
            && s.info.video_width == first.video_width
            && s.info.video_height == first.video_height
            && s.info.audio_formats == first.audio_formats
    });
    if same && !first.video_codec.is_empty() {
        Strategy::StreamCopy
    } else {
        Strategy::Reencode { width: even(first.video_width), height: even(first.video_height) }
    }
}

fn even(v: i32) -> i32 {
    (v.max(2) / 2) * 2
}

/// Total length of the result (progress denominator).
pub fn total_duration(sources: &[MergeSource]) -> f64 {
    sources.iter().map(|s| s.info.duration_sec.max(0.0)).sum()
}

/// Where the result goes: next to the first clip, so it shows up in the same library folder.
pub fn output_path(first_clip: &str) -> PathBuf {
    ffmpeg::unique_sibling_path(first_clip, "merged", "mp4")
}

/// The concat demuxer's list file. Paths are single-quoted; a quote inside a path becomes `'\''`.
pub fn concat_list(sources: &[MergeSource]) -> String {
    let mut list = String::from("ffconcat version 1.0\n");
    for s in sources {
        list.push_str(&format!("file '{}'\n", s.path.replace('\'', r"'\''")));
    }
    list
}

pub fn stream_copy_args(list_file: &str, out: &str) -> Vec<String> {
    let args = [
        "-y",
        "-f",
        "concat",
        "-safe",
        "0",
        "-i",
        list_file,
        // every video + audio stream (all audio tracks of a Lag recording), nothing else (data/subtitles)
        "-map",
        "0:v:0",
        "-map",
        "0:a?",
        "-c",
        "copy",
        "-movflags",
        "+faststart",
        "-progress",
        "pipe:1",
        out,
    ];
    args.iter().map(|s| s.to_string()).collect()
}

pub fn reencode_args(sources: &[MergeSource], width: i32, height: i32, nvenc: bool, out: &str) -> Vec<String> {
    let mut args: Vec<String> = vec!["-y".into()];
    for s in sources {
        args.extend(["-i".to_string(), s.path.clone()]);
    }
    let mut graph = Vec::new();
    let mut pairs = String::new();
    for (i, s) in sources.iter().enumerate() {
        graph.push(format!(
            "[{i}:v:0]scale={width}:{height}:force_original_aspect_ratio=decrease,\
             pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:black,setsar=1,format=yuv420p[v{i}]"
        ));
        if s.info.audio_streams > 0 {
            graph.push(format!("[{i}:a:0]aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo[a{i}]"));
        } else {
            let d = s.info.duration_sec.max(0.1);
            graph.push(format!("anullsrc=r=48000:cl=stereo,atrim=duration={d:.3}[a{i}]"));
        }
        pairs.push_str(&format!("[v{i}][a{i}]"));
    }
    graph.push(format!("{pairs}concat=n={}:v=1:a=1[v][a]", sources.len()));
    args.extend(["-filter_complex".to_string(), graph.join(";")]);
    args.extend(["-map", "[v]", "-map", "[a]", "-fps_mode", "vfr"].map(String::from));
    args.extend(ffmpeg::h264_args(nvenc));
    args.extend(["-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart", "-progress", "pipe:1"].map(String::from));
    args.push(out.to_string());
    args
}

/// Probes the clips, joins them into `out`. Blocking (run on a worker thread). `child` holds the running FFmpeg
/// so the UI can kill it; `cancel` makes the run return `Err("cancelled")`. On any failure the partial output
/// is deleted.
pub fn run(
    paths: &[String],
    out: &Path,
    cancel: &AtomicBool,
    child: &Arc<Mutex<Option<Child>>>,
    on_progress: &dyn Fn(f64),
) -> Result<(), String> {
    let result = run_inner(paths, out, cancel, child, on_progress);
    if result.is_err() {
        let _ = std::fs::remove_file(out);
    }
    result
}

fn run_inner(
    paths: &[String],
    out: &Path,
    cancel: &AtomicBool,
    child: &Arc<Mutex<Option<Child>>>,
    on_progress: &dyn Fn(f64),
) -> Result<(), String> {
    if paths.len() < MIN_CLIPS {
        return Err("pick at least two clips".into());
    }
    let mut sources = Vec::with_capacity(paths.len());
    for p in paths {
        let info = ffmpeg::probe(p).ok_or_else(|| format!("cannot read {p}"))?;
        sources.push(MergeSource { path: p.clone(), info });
    }
    let total = total_duration(&sources);
    let out_str = out.to_string_lossy().into_owned();
    let strategy = choose_strategy(&sources);
    log::info!("[ClipMerge] {} clips, {total:.1} s, {strategy:?} -> {out_str}", sources.len());

    let exit = match strategy {
        Strategy::StreamCopy => {
            let list = std::env::temp_dir().join(format!("lag-merge-{}.txt", std::process::id()));
            std::fs::write(&list, concat_list(&sources)).map_err(|e| e.to_string())?;
            let args = stream_copy_args(&list.to_string_lossy(), &out_str);
            let r = ffmpeg::run_with_progress(&args, cancel, Some(child), total, Some(on_progress));
            let _ = std::fs::remove_file(&list);
            r?
        }
        Strategy::Reencode { width, height } => {
            let run = |nvenc| {
                let args = reencode_args(&sources, width, height, nvenc, &out_str);
                ffmpeg::run_with_progress(&args, cancel, Some(child), total, Some(on_progress))
            };
            match run(true)? {
                0 => 0,
                _ if cancel.load(Ordering::Acquire) => return Err("cancelled".into()),
                code => {
                    log::info!("[ClipMerge] NVENC run failed (exit {code}), retrying with x264");
                    run(false)?
                }
            }
        }
    };
    if cancel.load(Ordering::Acquire) {
        return Err("cancelled".into());
    }
    if exit != 0 || !out.exists() {
        return Err(format!("ffmpeg exit {exit}"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::ffmpeg::AudioFormat;

    fn clip(path: &str, codec: &str, w: i32, h: i32, audio: usize, dur: f64) -> MergeSource {
        let fmt = AudioFormat { codec: "aac".into(), sample_rate: 48000, channels: 2 };
        MergeSource {
            path: path.into(),
            info: ProbeResult {
                duration_sec: dur,
                audio_streams: audio,
                video_width: w,
                video_height: h,
                video_codec: codec.into(),
                audio_formats: vec![fmt; audio],
            },
        }
    }

    #[test]
    fn same_recordings_are_stream_copied() {
        let a = clip(r"C:\c\a.mp4", "hevc", 2580, 1080, 3, 30.0);
        let b = clip(r"C:\c\b.mp4", "hevc", 2580, 1080, 3, 12.5);
        assert_eq!(choose_strategy(&[a.clone(), b.clone()]), Strategy::StreamCopy);
        assert_eq!(total_duration(&[a, b]), 42.5);
    }

    #[test]
    fn anything_different_is_reencoded_at_the_first_size() {
        let a = clip("a.mp4", "hevc", 2580, 1080, 3, 30.0);
        for other in [
            clip("b.mp4", "h264", 2580, 1080, 3, 5.0), // codec
            clip("b.mp4", "hevc", 1920, 1080, 3, 5.0), // size
            clip("b.mp4", "hevc", 2580, 1080, 1, 5.0), // audio tracks
        ] {
            assert_eq!(choose_strategy(&[a.clone(), other]), Strategy::Reencode { width: 2580, height: 1080 });
        }
        let odd = clip("o.mp4", "h264", 1081, 607, 1, 1.0);
        assert_eq!(choose_strategy(&[odd, a]), Strategy::Reencode { width: 1080, height: 606 });
    }

    #[test]
    fn concat_list_quotes_paths() {
        let list = concat_list(&[clip(r"C:\My clips\it's.mp4", "h264", 2, 2, 0, 1.0)]);
        assert_eq!(list, "ffconcat version 1.0\nfile 'C:\\My clips\\it'\\''s.mp4'\n");
    }

    #[test]
    fn reencode_graph_letterboxes_and_fills_missing_audio() {
        let srcs = [clip("a.mp4", "hevc", 1920, 1080, 2, 10.0), clip("b.mp4", "h264", 1080, 1920, 0, 4.0)];
        let args = reencode_args(&srcs, 1920, 1080, false, "out.mp4");
        let graph = &args[args.iter().position(|a| a == "-filter_complex").unwrap() + 1];
        assert!(graph.contains("[1:v:0]scale=1920:1080:force_original_aspect_ratio=decrease,pad=1920:1080"), "{graph}");
        assert!(graph.contains("[0:a:0]aresample=48000"), "{graph}");
        assert!(graph.contains("anullsrc=r=48000:cl=stereo,atrim=duration=4.000[a1]"), "{graph}");
        assert!(graph.ends_with("[v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]"), "{graph}");
        assert!(args.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert_eq!(args.last().unwrap(), "out.mp4");
    }

    #[test]
    fn stream_copy_keeps_all_audio_tracks() {
        let args = stream_copy_args("list.txt", "out.mp4");
        assert!(args.windows(2).any(|w| w == ["-map", "0:a?"]));
        assert!(args.windows(2).any(|w| w == ["-c", "copy"]));
    }

    /// Real FFmpeg run: `LAG_MERGE_TEST="a.mp4;b.mp4" cargo test -p lag-app real_merge -- --ignored`.
    #[test]
    #[ignore]
    fn real_merge() {
        let paths: Vec<String> = std::env::var("LAG_MERGE_TEST").unwrap().split(';').map(String::from).collect();
        let sources: Vec<MergeSource> =
            paths.iter().map(|p| MergeSource { path: p.clone(), info: ffmpeg::probe(p).unwrap() }).collect();
        let out = std::env::temp_dir().join("lag-merge-test.mp4");
        let _ = std::fs::remove_file(&out);
        let child = Arc::new(Mutex::new(None));
        run(&paths, &out, &AtomicBool::new(false), &child, &|_| {}).unwrap();
        let merged = ffmpeg::probe(&out.to_string_lossy()).unwrap();
        let expected = total_duration(&sources);
        assert!((merged.duration_sec - expected).abs() < 1.0, "{} vs {expected}", merged.duration_sec);
        println!("{:?} -> {merged:?}", choose_strategy(&sources));
    }
}
