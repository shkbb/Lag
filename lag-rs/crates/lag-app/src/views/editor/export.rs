//! Export: FFmpeg filter graphs and arguments built from an `EditModel`, and the export worker
//! (segments, NVENC with an x264 fallback, progress, cancel).

use super::{model::*, unique_hex_id, worker::*};
use crate::services::ffmpeg::{self};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};

pub(super) fn fmt_dec(v: f64, max_decimals: usize) -> String {
    let mut s = format!("{v:.max_decimals$}");
    if s.contains('.') {
        while s.ends_with('0') {
            s.pop();
        }
        if s.ends_with('.') {
            s.pop();
        }
    }
    if s == "-0" {
        "0".to_string()
    } else {
        s
    }
}

pub fn even_round(v: f64) -> i32 {
    let mut i = v.round() as i32;
    if i < 2 {
        i = 2;
    }
    i - (i % 2)
}

pub fn color_to_ffmpeg(hex: &str) -> String {
    let h = hex.trim().trim_start_matches('#');
    if h.len() == 6 || h.len() == 8 {
        format!("0x{h}")
    } else {
        "white".to_string()
    }
}

pub fn build_atempo_chain(speed: f64) -> String {
    let mut parts = Vec::new();
    let mut s = speed;
    while s < 0.5 {
        parts.push("atempo=0.5".to_string());
        s /= 0.5;
    }
    while s > 2.0 {
        parts.push("atempo=2.0".to_string());
        s /= 2.0;
    }
    parts.push(format!("atempo={}", fmt_dec(s, 4)));
    parts.join(",")
}

pub fn build_reframe_filters(model: &EditModel) -> Vec<String> {
    let mut vparts = Vec::new();
    let needs_aspect = model.selected_aspect().ratio.is_some();
    if !needs_aspect && !model.is_reframed() && !model.is_cropped() {
        return vparts;
    }
    if model.source_width <= 0 || model.source_height <= 0 {
        return vparts;
    }

    let base_w = even_round(model.source_width as f64 * model.crop_w);
    let base_h = even_round(model.source_height as f64 * model.crop_h);

    let (ow, oh) = match model.selected_aspect().ratio {
        Some(r) if r > 0.0 => {
            (even_round((base_w as f64).min(base_h as f64 * r)), even_round((base_h as f64).min(base_w as f64 / r)))
        }
        _ => (base_w, base_h),
    };

    let vw = even_round(model.vid_w * ow as f64);
    let vh = even_round(model.vid_h * oh as f64);
    let ox = (model.vid_x * ow as f64).round() as i32;
    let oy = (model.vid_y * oh as f64).round() as i32;

    let mut crop_x = (-ox).max(0);
    let mut crop_y = (-oy).max(0);
    let mut px = ox.max(0);
    let mut py = oy.max(0);
    let cw = even_round((vw - crop_x).min(ow - px) as f64);
    let ch = even_round((vh - crop_y).min(oh - py) as f64);
    if cw < 2 || ch < 2 {
        return vparts;
    }
    crop_x -= crop_x % 2;
    crop_y -= crop_y % 2;
    px -= px % 2;
    py -= py % 2;

    vparts.push(format!("scale={vw}:{vh}"));
    vparts.push(format!("crop={cw}:{ch}:{crop_x}:{crop_y}"));
    vparts.push(format!("pad={ow}:{oh}:{px}:{py}:black"));
    vparts
}

/// Scale filter for an export resolution preset (`res` = 720, 1080, ...; 0 = original size).
///
/// The preset names the SHORT side, the way TikTok / Shorts / Reels count it: "1080p" vertical is 1080x1920.
/// Scaling a portrait frame by height (C# did `scale=-2:1080` for every shape) turned a 9:16 export into a
/// blurry 608x1080 (owner, 2026-11-18: "9:16 works like a cripple").
pub fn export_scale_filter(res: i32, portrait: bool) -> Option<String> {
    (res > 0).then(|| if portrait { format!("scale={res}:-2") } else { format!("scale=-2:{res}") })
}

pub fn build_rotate_flip_filters(model: &EditModel) -> Vec<String> {
    let mut vparts = Vec::new();
    if model.flip_h {
        vparts.push("hflip".to_string());
    }
    if model.flip_v {
        vparts.push("vflip".to_string());
    }
    match model.rotation {
        90 => vparts.push("transpose=1".to_string()),
        180 => {
            vparts.push("transpose=1".to_string());
            vparts.push("transpose=1".to_string());
        }
        270 => vparts.push("transpose=2".to_string()),
        _ => {}
    }
    vparts
}

pub fn escape_drawtext_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/").replace(':', "\\:").replace('\'', "\\'")
}

pub fn build_video_filter_chain(model: &EditModel, speed: f64, text_file_paths: &[(&TextItem, &Path)]) -> Vec<String> {
    let mut vparts = Vec::new();
    let speed_changed = (speed - 1.0).abs() > 0.001;
    if speed_changed {
        vparts.push(format!("setpts=PTS/{speed}"));
    }
    if let Some(graph) = model.selected_filter().graph {
        vparts.push(graph.to_string());
    }
    if model.has_color_adjust() {
        let c = model.color_contrast as f64 / 100.0;
        let b = model.color_brightness as f64 / 100.0 - 1.0;
        let s = model.color_saturation as f64 / 100.0;
        vparts.push(format!("eq=contrast={}:brightness={}:saturation={}", fmt_dec(c, 3), fmt_dec(b, 3), fmt_dec(s, 3)));
    }
    if model.is_cropped() {
        vparts.push(format!(
            "crop=iw*{}:ih*{}:iw*{}:ih*{}",
            fmt_dec(model.crop_w, 5),
            fmt_dec(model.crop_h, 5),
            fmt_dec(model.crop_x, 5),
            fmt_dec(model.crop_y, 5)
        ));
    }
    vparts.extend(build_reframe_filters(model));
    vparts.extend(build_rotate_flip_filters(model));
    if let Some(scale) = export_scale_filter(model.selected_export_res().height, model.is_portrait_output()) {
        vparts.push(scale);
    }
    for (item, path) in text_file_paths {
        if item.text.trim().is_empty() {
            continue;
        }
        let esc = escape_drawtext_path(path);
        let nx = fmt_dec(item.nx.clamp(0.0, 1.0), 3);
        let ny = fmt_dec(item.ny.clamp(0.0, 1.0), 3);
        let frac = fmt_dec(item.font_percent.clamp(1, 50) as f64 / 100.0, 4);
        let col = color_to_ffmpeg(&item.color);
        vparts.push(format!(
            "drawtext=fontfile='C\\:/Windows/Fonts/arialbd.ttf':textfile='{esc}':\
             x=(w-text_w)*{nx}:y=(h-text_h)*{ny}:fontsize=h*{frac}:\
             fontcolor={col}:borderw=2:bordercolor=black@0.7"
        ));
    }
    vparts
}

/// Audio of one exported span of input `input`: one filter per audible track (its volume, silence outside the
/// track's own range, tempo for the speed) and the output labels `[a{tag}{stream}]`. Tracks that are muted or
/// lie outside the span are left out.
pub fn audio_track_filters(
    model: &EditModel,
    input: usize,
    tag: &str,
    span: (f64, f64),
    speed: f64,
) -> (Vec<String>, Vec<String>) {
    let (start, duration) = span;
    let (mut filters, mut labels) = (Vec::new(), Vec::new());
    for track in model.audio_tracks.iter().filter(|t| !t.is_muted && t.volume > 0) {
        let a = (track.start_sec - start).max(0.0);
        let b = (track.end_sec - start).min(duration);
        if b <= 0.01 || a >= duration - 0.01 {
            continue;
        }
        let mut aparts = vec![format!("volume={}", fmt_dec(track.volume as f64 / 100.0, 3))];
        if a > 0.01 || b < duration - 0.01 {
            aparts.push(format!("volume=0:enable='lt(t\\,{})+gt(t\\,{})'", fmt_dec(a, 3), fmt_dec(b, 3)));
        }
        if (speed - 1.0).abs() > 0.001 {
            aparts.push(build_atempo_chain(speed));
        }
        let label = format!("[a{tag}{}]", track.stream_index);
        filters.push(format!("[{input}:a:{}]{}{label}", track.stream_index, aparts.join(",")));
        labels.push(label);
    }
    (filters, labels)
}

/// `span` = (start, duration) of the source to export, in seconds.
pub fn build_export_args(
    model: &EditModel,
    clip_path: &str,
    out_path: &str,
    span: (f64, f64),
    speed: f64,
    use_nvenc: bool,
    text_file_paths: &[(&TextItem, &Path)],
) -> Vec<String> {
    let (start, duration) = span;
    let mut args = vec![
        "-y".to_string(),
        "-ss".to_string(),
        format!("{start}"),
        "-t".to_string(),
        format!("{duration}"),
        "-i".to_string(),
        clip_path.to_string(),
    ];

    let vparts = build_video_filter_chain(model, speed, text_file_paths);

    if model.export_as_gif {
        let vchain = if vparts.is_empty() { String::new() } else { format!("{},", vparts.join(",")) };
        args.push("-filter_complex".to_string());
        args.push(format!(
            "[0:v:0]{vchain}fps=15,split[ga][gb];[ga]palettegen=max_colors=256[gp];[gb][gp]paletteuse=dither=sierra2_4a[v]"
        ));
        args.extend([
            "-map".to_string(),
            "[v]".to_string(),
            "-an".to_string(),
            "-loop".to_string(),
            "0".to_string(),
            "-progress".to_string(),
            "pipe:1".to_string(),
            out_path.to_string(),
        ]);
        return args;
    }

    let mut filters = Vec::new();
    let mut maps: Vec<String> = Vec::new();
    if !vparts.is_empty() {
        filters.push(format!("[0:v:0]{}[v]", vparts.join(",")));
        maps.push("-map".to_string());
        maps.push("[v]".to_string());
    } else {
        maps.push("-map".to_string());
        maps.push("0:v:0".to_string());
    }

    let (audio_filters, audio_labels) = audio_track_filters(model, 0, "", span, speed);
    filters.extend(audio_filters);

    let has_audio = !audio_labels.is_empty();
    if audio_labels.len() == 1 {
        maps.push("-map".to_string());
        maps.push(audio_labels[0].clone());
    } else if audio_labels.len() > 1 {
        filters.push(format!(
            "{}amix=inputs={}:duration=longest:normalize=0[aout]",
            audio_labels.concat(),
            audio_labels.len()
        ));
        maps.push("-map".to_string());
        maps.push("[aout]".to_string());
    }

    if !filters.is_empty() {
        args.push("-filter_complex".to_string());
        args.push(filters.join(";"));
    }
    args.extend(maps);

    args.extend(ffmpeg::h264_args(use_nvenc));

    if has_audio {
        args.extend(["-c:a", "aac", "-b:a", "192k"].into_iter().map(str::to_string));
    } else {
        args.push("-an".to_string());
    }

    args.extend(["-movflags", "+faststart", "-progress", "pipe:1"].into_iter().map(str::to_string));
    args.push(out_path.to_string());
    args
}

pub fn build_concat_args(list_file: &str, out_path: &str, faststart: bool) -> Vec<String> {
    let mut args = vec![
        "-y".to_string(),
        "-f".to_string(),
        "concat".to_string(),
        "-safe".to_string(),
        "0".to_string(),
        "-i".to_string(),
        list_file.to_string(),
        "-c".to_string(),
        "copy".to_string(),
    ];
    if faststart {
        args.push("-movflags".to_string());
        args.push("+faststart".to_string());
    }
    args.push(out_path.to_string());
    args
}

pub fn build_output_path(clip_path: &str, ext: &str) -> PathBuf {
    ffmpeg::unique_sibling_path(clip_path, "edit", ext)
}

pub(super) fn spawn_export(
    export_gen: u64,
    model: EditModel,
    clip_path: String,
    out_path: PathBuf,
    cancel: Arc<AtomicBool>,
    active_child: Arc<Mutex<Option<Child>>>,
    tx: mpsc::Sender<UiMessage>,
) {
    let _ = std::thread::Builder::new().name("Lag.EditorExport".into()).spawn(move || {
        let speed = model.effective_speed();
        let start = model.trim_start_sec.clamp(0.0, model.duration_sec);
        let end = model.trim_end_sec.min(model.duration_sec);

        let mut segments = if model.export_as_gif || model.cuts.is_empty() {
            vec![(start, end.max(start + 0.2))]
        } else {
            model.kept_segments(start, end)
        };
        if segments.is_empty() {
            segments.push((start, end.max(start + 0.2)));
        }

        let out_str = out_path.to_string_lossy().into_owned();
        let result = export_segments(
            &ExportJob { model: &model, clip_path: &clip_path, speed, cancel: &cancel, active_child: &active_child },
            export_gen,
            &out_str,
            &segments,
            &tx,
        );

        match result {
            Ok(0) if out_path.exists() => {
                let fname = out_path.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string();
                let _ = tx.send(UiMessage::ExportFinished(export_gen, Ok(fname)));
            }
            Ok(code) => {
                let _ = std::fs::remove_file(&out_path);
                if cancel.load(Ordering::Acquire) {
                    let _ = tx.send(UiMessage::ExportFinished(export_gen, Err(None)));
                } else {
                    let _ = tx.send(UiMessage::ExportFinished(export_gen, Err(Some(format!("ffmpeg exit {code}")))));
                }
            }
            Err(e) => {
                let _ = std::fs::remove_file(&out_path);
                if cancel.load(Ordering::Acquire) || e == "cancelled" {
                    let _ = tx.send(UiMessage::ExportFinished(export_gen, Err(None)));
                } else {
                    let _ = tx.send(UiMessage::ExportFinished(export_gen, Err(Some(e))));
                }
            }
        }
    });
}

/// One export run: the edit, the source clip, the playback speed, and how the run is stopped.
pub(super) struct ExportJob<'a> {
    pub model: &'a EditModel,
    pub clip_path: &'a str,
    pub speed: f64,
    pub cancel: &'a AtomicBool,
    /// The running FFmpeg process (killed by Cancel).
    pub active_child: &'a Arc<Mutex<Option<Child>>>,
}

pub(super) fn export_segments(
    job: &ExportJob,
    export_gen: u64,
    out_path: &str,
    segments: &[(f64, f64)],
    tx: &mpsc::Sender<UiMessage>,
) -> Result<i32, String> {
    let ExportJob { cancel, active_child, .. } = *job;
    if segments.len() == 1 {
        let (s, e) = segments[0];
        let seg_dur = (e - s).max(0.05);
        return run_one_segment(job, out_path, s, seg_dur, &|p| {
            let _ = tx.send(UiMessage::ExportProgress(export_gen, p.clamp(0.0, 100.0) as i32));
        });
    }

    let ext = Path::new(out_path).extension().and_then(|s| s.to_str()).unwrap_or("mp4");
    let mut temps = Vec::new();
    let res = (|| -> Result<i32, String> {
        for (i, &(s, e)) in segments.iter().enumerate() {
            if cancel.load(Ordering::Acquire) {
                return Err("cancelled".into());
            }
            let tmp = std::env::temp_dir().join(format!("lag-seg-{}.{ext}", unique_hex_id()));
            let tmp_str = tmp.to_string_lossy().into_owned();
            temps.push(tmp);
            let seg_dur = (e - s).max(0.05);
            let total_segs = segments.len() as f64;
            let code = run_one_segment(job, &tmp_str, s, seg_dur, &|p| {
                let overall = (i as f64 * 100.0 + p.clamp(0.0, 100.0)) / total_segs;
                let _ = tx.send(UiMessage::ExportProgress(export_gen, overall as i32));
            })?;
            if code != 0 {
                return Ok(code);
            }
        }

        let list_file = std::env::temp_dir().join(format!("lag-concat-{}.txt", unique_hex_id()));
        let mut content = String::new();
        for t in &temps {
            let escaped = t.to_string_lossy().replace('\'', "'\\''");
            content.push_str(&format!("file '{escaped}'\n"));
        }
        std::fs::write(&list_file, content).map_err(|e| e.to_string())?;
        let faststart = matches!(ext, "mp4" | "mov");
        let list_str = list_file.to_string_lossy().into_owned();
        let cargs = build_concat_args(&list_str, out_path, faststart);
        let cexit = ffmpeg::run_with_progress(&cargs, cancel, Some(active_child), 0.0, None);
        let _ = std::fs::remove_file(&list_file);
        cexit
    })();

    for t in temps {
        let _ = std::fs::remove_file(t);
    }
    res
}

pub(super) fn run_one_segment(
    job: &ExportJob,
    out_path: &str,
    start: f64,
    duration: f64,
    on_progress: &dyn Fn(f64),
) -> Result<i32, String> {
    let ExportJob { model, clip_path, speed, cancel, active_child } = *job;
    let eff_dur = duration / speed;
    let run_with_codec = |use_nvenc: bool| -> Result<i32, String> {
        let mut temp_text_paths: Vec<(TextItem, PathBuf)> = Vec::new();
        for item in &model.texts {
            if item.text.trim().is_empty() {
                continue;
            }
            let tf = std::env::temp_dir().join(format!("lag-txt-{}.txt", unique_hex_id()));
            std::fs::write(&tf, &item.text).map_err(|e| e.to_string())?;
            temp_text_paths.push((item.clone(), tf));
        }
        let pairs: Vec<(&TextItem, &Path)> = temp_text_paths.iter().map(|(it, p)| (it, p.as_path())).collect();
        let args = build_export_args(model, clip_path, out_path, (start, duration), speed, use_nvenc, &pairs);
        let res = ffmpeg::run_with_progress(&args, cancel, Some(active_child), eff_dur, Some(on_progress));
        for (_, p) in temp_text_paths {
            let _ = std::fs::remove_file(p);
        }
        res
    };

    if model.export_as_gif {
        return run_with_codec(false);
    }
    let exit = run_with_codec(true)?;
    if exit != 0 && !cancel.load(Ordering::Acquire) {
        return run_with_codec(false);
    }
    Ok(exit)
}
