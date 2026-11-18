//! Exporting a montage in ONE FFmpeg run: every piece (a segment's kept span) is its own input with its own
//! filter chain (speed, look, crop/reframe, rotation, captions, track volumes), fitted into the montage frame
//! (letterboxed, never stretched), and the pieces are joined by the concat filter. One encode, no temp videos.

use super::export::{audio_track_filters, build_video_filter_chain};
use super::model::{EditModel, TextItem};
use super::montage::Montage;
use super::unique_hex_id;
use crate::services::ffmpeg;
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// One input of the montage export: a span of one segment's source played with that segment's edit.
pub struct MontagePiece<'a> {
    pub edit: &'a EditModel,
    /// (start, duration) in source seconds.
    pub span: (f64, f64),
    pub texts: Vec<(&'a TextItem, &'a Path)>,
}

fn even(v: f64) -> i32 {
    ((v.round() as i32).max(2) / 2) * 2
}

/// The montage's frame size: the first segment's output shape (frame option, crop, rotation) with the export
/// resolution preset as the short side (`res` = 0: the first segment's own size).
pub fn montage_frame(first: &EditModel, res: i32) -> (i32, i32) {
    let (sw, sh) = if first.source_width > 0 && first.source_height > 0 {
        (first.source_width as f64, first.source_height as f64)
    } else {
        (1920.0, 1080.0)
    };
    let (bw, bh) = (sw * first.crop_w, sh * first.crop_h);
    let (mut w, mut h) = match first.selected_aspect().ratio {
        Some(r) if r > 0.0 => (bw.min(bh * r), bh.min(bw / r)),
        _ => (bw, bh),
    };
    if first.rotation == 90 || first.rotation == 270 {
        std::mem::swap(&mut w, &mut h);
    }
    if res > 0 {
        let k = res as f64 / w.min(h);
        (w, h) = (w * k, h * k);
    }
    (even(w), even(h))
}

/// FFmpeg arguments for the whole montage: inputs, one filter graph, H.264 (NVENC or x264) + AAC.
pub fn build_montage_args(
    pieces: &[MontagePiece],
    frame: (i32, i32),
    nvenc: bool,
    clip_paths: &[&str],
    out: &str,
) -> Vec<String> {
    let (w, h) = frame;
    let mut args = vec!["-y".to_string()];
    for (piece, path) in pieces.iter().zip(clip_paths) {
        let (start, duration) = piece.span;
        args.extend([
            "-ss".into(),
            format!("{start}"),
            "-t".into(),
            format!("{duration}"),
            "-i".into(),
            path.to_string(),
        ]);
    }
    let mut graph = Vec::new();
    let mut pairs = String::new();
    for (i, piece) in pieces.iter().enumerate() {
        let speed = piece.edit.effective_speed();
        let mut vchain = build_video_filter_chain(piece.edit, speed, &piece.texts);
        vchain.push(format!("scale={w}:{h}:force_original_aspect_ratio=decrease"));
        vchain.push(format!("pad={w}:{h}:(ow-iw)/2:(oh-ih)/2:black"));
        vchain.push("setsar=1".into());
        vchain.push("format=yuv420p".into());
        graph.push(format!("[{i}:v:0]{}[v{i}]", vchain.join(",")));

        let (filters, labels) = audio_track_filters(piece.edit, i, &format!("{i}_"), piece.span, speed);
        graph.extend(filters);
        let to_stereo = "aresample=48000,aformat=sample_fmts=fltp:channel_layouts=stereo";
        match labels.len() {
            0 => {
                let d = piece.span.1 / speed.max(0.01);
                graph.push(format!("anullsrc=r=48000:cl=stereo,atrim=duration={d:.3}[a{i}]"));
            }
            1 => graph.push(format!("{}{to_stereo}[a{i}]", labels[0])),
            n => {
                graph.push(format!("{}amix=inputs={n}:duration=longest:normalize=0,{to_stereo}[a{i}]", labels.concat()))
            }
        }
        pairs.push_str(&format!("[v{i}][a{i}]"));
    }
    graph.push(format!("{pairs}concat=n={}:v=1:a=1[v][a]", pieces.len()));
    args.extend(["-filter_complex".to_string(), graph.join(";")]);
    args.extend(["-map", "[v]", "-map", "[a]", "-fps_mode", "vfr"].map(String::from));
    args.extend(ffmpeg::h264_args(nvenc));
    args.extend(["-c:a", "aac", "-b:a", "192k", "-movflags", "+faststart", "-progress", "pipe:1"].map(String::from));
    args.push(out.to_string());
    args
}

/// Where the montage goes: next to its first clip, `<first>-montage.<ext>`.
pub fn montage_output_path(montage: &Montage, ext: &str) -> PathBuf {
    let first = montage.segments()[0].edit.current_clip_path.as_deref().unwrap_or("montage.mp4");
    ffmpeg::unique_sibling_path(first, "montage", ext)
}

/// Renders the montage into `out` (blocking; a worker thread). `res` = export resolution preset (short side).
/// NVENC first, x264 when that fails. Returns FFmpeg's exit code; `Err("cancelled")` after a cancel.
pub fn export_montage(
    montage: &Montage,
    res: i32,
    out: &Path,
    cancel: &AtomicBool,
    child: &Arc<Mutex<Option<Child>>>,
    on_progress: &dyn Fn(f64),
) -> Result<i32, String> {
    // caption texts go through files (drawtext textfile=) so any character survives
    let mut text_files: Vec<(usize, usize, PathBuf)> = Vec::new();
    for (si, seg) in montage.segments().iter().enumerate() {
        for (ti, t) in seg.edit.texts.iter().enumerate() {
            if t.text.trim().is_empty() {
                continue;
            }
            let p = std::env::temp_dir().join(format!("lag-txt-{}.txt", unique_hex_id()));
            std::fs::write(&p, &t.text).map_err(|e| e.to_string())?;
            text_files.push((si, ti, p));
        }
    }
    let mut pieces = Vec::new();
    let mut paths = Vec::new();
    for (si, seg) in montage.segments().iter().enumerate() {
        let Some(path) = seg.edit.current_clip_path.as_deref() else { continue };
        let texts: Vec<(&TextItem, &Path)> = text_files
            .iter()
            .filter(|(s, _, _)| *s == si)
            .map(|(_, ti, p)| (&seg.edit.texts[*ti], p.as_path()))
            .collect();
        for (a, b) in seg.pieces() {
            pieces.push(MontagePiece { edit: &seg.edit, span: (a, (b - a).max(0.05)), texts: texts.clone() });
            paths.push(path);
        }
    }
    let result = if pieces.is_empty() {
        Err("nothing to export".to_string())
    } else {
        let frame = montage_frame(&montage.segments()[0].edit, res);
        let total = montage.duration();
        let out_str = out.to_string_lossy().into_owned();
        log::info!(
            "[Montage] export {} piece(s), {total:.1} s, frame {}x{} -> {out_str}",
            pieces.len(),
            frame.0,
            frame.1
        );
        let run = |nvenc| {
            let args = build_montage_args(&pieces, frame, nvenc, &paths, &out_str);
            ffmpeg::run_with_progress(&args, cancel, Some(child), total, Some(on_progress))
        };
        match run(true) {
            Ok(0) => Ok(0),
            Ok(code) if !cancel.load(Ordering::Acquire) => {
                log::info!("[Montage] NVENC run failed (exit {code}), retrying with x264");
                run(false)
            }
            other => other,
        }
    };
    for (_, _, p) in text_files {
        let _ = std::fs::remove_file(p);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip(path: &str, w: i32, h: i32, dur: f64) -> EditModel {
        let mut m = EditModel::new("Native");
        m.reset_for_clip(path, dur, "Audio");
        m.source_width = w;
        m.source_height = h;
        m
    }

    #[test]
    fn frame_follows_the_first_segment_and_the_short_side() {
        let a = clip("a.mp4", 2580, 1080, 10.0);
        assert_eq!(montage_frame(&a, 0), (2580, 1080));
        assert_eq!(montage_frame(&a, 720), (1720, 720));
        let mut v = a.clone();
        v.selected_aspect_index = 2; // 9:16
        v.reset_reframe();
        assert_eq!(montage_frame(&v, 1080), (1080, 1920));
        let mut r = a.clone();
        r.rotation = 90;
        assert_eq!(montage_frame(&r, 0), (1080, 2580));
    }

    #[test]
    fn one_graph_joins_every_piece_in_order() {
        let mut a = clip("a.mp4", 1920, 1080, 10.0);
        a.audio_tracks[0].volume = 50;
        let mut b = clip("b.mp4", 1280, 720, 8.0);
        b.set_speed_text("2".into());
        b.audio_tracks.clear(); // no sound -> silence of the right length
        let pieces = [
            MontagePiece { edit: &a, span: (1.0, 3.0), texts: vec![] },
            MontagePiece { edit: &b, span: (2.0, 4.0), texts: vec![] },
        ];
        let args = build_montage_args(&pieces, (1920, 1080), false, &["a.mp4", "b.mp4"], "out.mp4");
        assert_eq!(&args[1..7], &["-ss", "1", "-t", "3", "-i", "a.mp4"]);
        assert_eq!(&args[7..13], &["-ss", "2", "-t", "4", "-i", "b.mp4"]);
        let graph = &args[args.iter().position(|x| x == "-filter_complex").unwrap() + 1];
        assert!(graph.contains("[1:v:0]setpts=PTS/2,"), "{graph}");
        assert!(graph.contains("scale=1920:1080:force_original_aspect_ratio=decrease,pad=1920:1080"), "{graph}");
        assert!(graph.contains("[0:a:0]volume=0.5[a0_0]"), "{graph}");
        assert!(graph.contains("anullsrc=r=48000:cl=stereo,atrim=duration=2.000[a1]"), "{graph}");
        assert!(graph.ends_with("[v0][a0][v1][a1]concat=n=2:v=1:a=1[v][a]"), "{graph}");
        assert!(args.windows(2).any(|w| w == ["-c:v", "libx264"]));
        assert_eq!(args.last().unwrap(), "out.mp4");
    }

    /// Real FFmpeg run: `LAG_MONTAGE_TEST="a.mp4;b.mp4" cargo test -p lag-app real_montage -- --ignored`.
    #[test]
    #[ignore]
    fn real_montage() {
        let paths: Vec<String> = std::env::var("LAG_MONTAGE_TEST").unwrap().split(';').map(String::from).collect();
        let mut edits = Vec::new();
        for (i, p) in paths.iter().enumerate() {
            let info = ffmpeg::probe(p).unwrap();
            let mut e = clip(p, info.video_width, info.video_height, info.duration_sec);
            e.audio_tracks = (0..info.audio_streams)
                .map(|k| super::super::model::AudioTrack::new(k, "t", info.duration_sec))
                .collect();
            e.trim_start_sec = 1.0;
            e.trim_end_sec = 4.0 + i as f64;
            if i == 1 {
                e.selected_filter_index = 3;
                e.set_speed_text("2".into());
            }
            edits.push(e);
        }
        let mut m = Montage::from_edits(edits);
        m.set_aspect_everywhere(2); // 9:16 vertical
        let out = std::env::temp_dir().join("lag-montage-test.mp4");
        let _ = std::fs::remove_file(&out);
        let code =
            export_montage(&m, 1080, &out, &AtomicBool::new(false), &Arc::new(Mutex::new(None)), &|_| {}).unwrap();
        assert_eq!(code, 0);
        let got = ffmpeg::probe(&out.to_string_lossy()).unwrap();
        println!("expected {:.2}s -> {got:?}", m.duration());
        assert_eq!((got.video_width, got.video_height), (1080, 1920));
        assert!((got.duration_sec - m.duration()).abs() < 0.5);
    }
}
