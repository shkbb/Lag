//! FFmpeg / FFprobe as child processes: locating the bundled tools, running with progress + cancel, probing
//! files, picking the video encoder arguments. Shared by the editor (export, merge, thumbnails) and the library.
//! Before 2026-11-18 these lived inside views/editor.rs, with a second copy of the tool lookup in library.rs.

use std::io::{BufRead, BufReader};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// No console window for the child process.
pub const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn resolve_tool(exe_name: &str) -> PathBuf {
    if let Some(dir) = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.join("ffmpeg"))) {
        let candidate = dir.join(exe_name);
        if candidate.exists() {
            return candidate;
        }
    }
    let dev_candidate = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../InstantReplay/ffmpeg").join(exe_name);
    if dev_candidate.exists() {
        return dev_candidate;
    }
    PathBuf::from(exe_name)
}

pub fn parse_progress_line(line: &str, expected_duration_sec: f64) -> Option<f64> {
    if expected_duration_sec <= 0.0 {
        return None;
    }
    let rest = line.trim().strip_prefix("out_time_ms=")?;
    let us: i64 = rest.parse().ok()?;
    if us < 0 {
        return None;
    }
    Some((us as f64 / 1_000_000.0 / expected_duration_sec * 100.0).clamp(0.0, 100.0))
}

/// One audio stream's shape: two files can be joined without re-encoding only when these match.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AudioFormat {
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u32,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct ProbeResult {
    pub duration_sec: f64,
    pub audio_streams: usize,
    pub video_width: i32,
    pub video_height: i32,
    /// FFmpeg codec name of the first video stream ("h264", "hevc", ...).
    pub video_codec: String,
    /// Every audio stream in file order.
    pub audio_formats: Vec<AudioFormat>,
}

pub fn parse_ffprobe_json(json_str: &str) -> Option<ProbeResult> {
    let v: serde_json::Value = serde_json::from_str(json_str).ok()?;
    let format_dur = v
        .get("format")
        .and_then(|f| f.get("duration"))
        .and_then(|d| d.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|d| d.is_finite() && *d > 0.0)
        .unwrap_or(0.0);

    let streams = v.get("streams").and_then(|s| s.as_array())?;
    let mut audio_streams = 0usize;
    let mut audio_formats = Vec::new();
    let mut video_codec = String::new();
    let mut video_width = 0i32;
    let mut video_height = 0i32;
    let mut stream_dur = 0.0f64;

    for st in streams {
        let codec_type = st.get("codec_type").and_then(|c| c.as_str()).unwrap_or("");
        let text = |k: &str| st.get(k).and_then(|c| c.as_str()).unwrap_or("").to_string();
        if codec_type == "audio" {
            audio_streams += 1;
            audio_formats.push(AudioFormat {
                codec: text("codec_name"),
                sample_rate: text("sample_rate").parse().unwrap_or(0),
                channels: st.get("channels").and_then(|c| c.as_u64()).unwrap_or(0) as u32,
            });
        } else if codec_type == "video" && video_width == 0 {
            video_codec = text("codec_name");
            video_width = st.get("width").and_then(|w| w.as_i64()).unwrap_or(0) as i32;
            video_height = st.get("height").and_then(|h| h.as_i64()).unwrap_or(0) as i32;
            if stream_dur <= 0.0 {
                stream_dur = st
                    .get("duration")
                    .and_then(|d| d.as_str())
                    .and_then(|s| s.parse::<f64>().ok())
                    .filter(|d| d.is_finite() && *d > 0.0)
                    .unwrap_or(0.0);
            }
        }
    }

    Some(ProbeResult {
        duration_sec: if format_dur > 0.0 { format_dur } else { stream_dur },
        audio_streams,
        video_width,
        video_height,
        video_codec,
        audio_formats,
    })
}

pub fn run_simple(args: &[String], cancel: &AtomicBool) -> Result<i32, String> {
    run_with_progress(args, cancel, None, 0.0, None)
}

pub fn run_with_progress(
    args: &[String],
    cancel: &AtomicBool,
    active_child: Option<&Arc<Mutex<Option<Child>>>>,
    expected_duration_sec: f64,
    on_progress: Option<&dyn Fn(f64)>,
) -> Result<i32, String> {
    let ffmpeg = resolve_tool("ffmpeg.exe");
    let mut child = Command::new(&ffmpeg)
        .creation_flags(CREATE_NO_WINDOW)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();

    if let Some(slot) = active_child {
        if let Ok(mut g) = slot.lock() {
            *g = Some(child);
        }
    } else {
        // Poll-wait with cancel
        let err_drain = stderr.map(|s| {
            std::thread::spawn(move || {
                let mut r = BufReader::new(s);
                let mut buf = Vec::new();
                let _ = std::io::Read::read_to_end(&mut r, &mut buf);
            })
        });
        if let Some(out) = stdout {
            let reader = BufReader::new(out);
            for line in reader.lines().map_while(Result::ok) {
                if cancel.load(Ordering::Acquire) {
                    let _ = child.kill();
                    let _ = child.wait();
                    if let Some(h) = err_drain {
                        let _ = h.join();
                    }
                    return Err("cancelled".into());
                }
                if let Some(cb) = on_progress {
                    if let Some(p) = parse_progress_line(&line, expected_duration_sec) {
                        cb(p);
                    }
                }
            }
        }
        let status = child.wait().map_err(|e| e.to_string())?;
        if let Some(h) = err_drain {
            let _ = h.join();
        }
        return Ok(status.code().unwrap_or(-1));
    }

    // Draining stderr on background thread so ffmpeg never blocks on a full pipe.
    let err_drain = stderr.map(|s| {
        std::thread::spawn(move || {
            let mut r = BufReader::new(s);
            let mut buf = Vec::new();
            let _ = std::io::Read::read_to_end(&mut r, &mut buf);
        })
    });

    if let Some(out) = stdout {
        let reader = BufReader::new(out);
        for line in reader.lines().map_while(Result::ok) {
            if cancel.load(Ordering::Acquire) {
                break;
            }
            if let Some(cb) = on_progress {
                if let Some(p) = parse_progress_line(&line, expected_duration_sec) {
                    cb(p);
                }
            }
        }
    }

    let slot = active_child.unwrap();
    let code = {
        let mut g = slot.lock().map_err(|_| "lock poisoned".to_string())?;
        if let Some(mut ch) = g.take() {
            if cancel.load(Ordering::Acquire) {
                let _ = ch.kill();
            }
            ch.wait().map_err(|e| e.to_string())?.code().unwrap_or(-1)
        } else {
            -1
        }
    };
    if let Some(h) = err_drain {
        let _ = h.join();
    }
    if cancel.load(Ordering::Acquire) {
        return Err("cancelled".into());
    }
    Ok(code)
}

/// `<dir of source>/<stem>-<suffix>.<ext>`, or `-<suffix>2`, `-<suffix>3`, ... when that file exists:
/// outputs land next to their source and never overwrite anything.
pub fn unique_sibling_path(source: &str, suffix: &str, ext: &str) -> PathBuf {
    let p = Path::new(source);
    let dir = p.parent().unwrap_or_else(|| Path::new("."));
    let base = p.file_stem().and_then(|s| s.to_str()).unwrap_or("clip");
    let mut candidate = dir.join(format!("{base}-{suffix}.{ext}"));
    let mut i = 2usize;
    while candidate.exists() {
        candidate = dir.join(format!("{base}-{suffix}{i}.{ext}"));
        i += 1;
    }
    candidate
}

/// Runs ffprobe on `path` (blocking; call it off the UI thread). None = not a readable media file.
pub fn probe(path: &str) -> Option<ProbeResult> {
    let out = Command::new(resolve_tool("ffprobe.exe"))
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-v", "quiet", "-print_format", "json", "-show_format", "-show_streams", path])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_ffprobe_json(&String::from_utf8_lossy(&out.stdout))
}

/// H.264 encoder arguments: NVENC (constant quality 19) or x264 CRF 19. Callers retry with `nvenc = false`
/// when the NVENC run fails (no NVIDIA GPU, driver too old, session limit).
pub fn h264_args(nvenc: bool) -> Vec<String> {
    let args: &[&str] = if nvenc {
        &["-c:v", "h264_nvenc", "-preset", "p5", "-rc", "vbr", "-cq", "19", "-b:v", "0"]
    } else {
        &["-c:v", "libx264", "-preset", "veryfast", "-crf", "19"]
    };
    args.iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_lines() {
        assert_eq!(parse_progress_line("out_time_ms=2500000", 10.0), Some(25.0));
        assert_eq!(parse_progress_line("out_time_ms=15000000", 10.0), Some(100.0));
        assert_eq!(parse_progress_line("frame=120", 10.0), None);
        assert_eq!(parse_progress_line("out_time_ms=1", 0.0), None);
    }

    #[test]
    fn probe_reads_codecs_and_audio_formats() {
        let json = r#"{"format":{"duration":"12.5"},"streams":[
            {"codec_type":"video","codec_name":"hevc","width":2580,"height":1080},
            {"codec_type":"audio","codec_name":"aac","sample_rate":"48000","channels":2},
            {"codec_type":"audio","codec_name":"aac","sample_rate":"44100","channels":1}]}"#;
        let p = parse_ffprobe_json(json).unwrap();
        assert_eq!((p.video_codec.as_str(), p.video_width, p.video_height), ("hevc", 2580, 1080));
        assert_eq!(p.audio_streams, 2);
        assert_eq!(p.audio_formats[1], AudioFormat { codec: "aac".into(), sample_rate: 44100, channels: 1 });
        assert_eq!(p.duration_sec, 12.5);
    }

    #[test]
    fn encoder_args() {
        assert_eq!(h264_args(true)[1], "h264_nvenc");
        assert_eq!(h264_args(false)[1], "libx264");
    }
}
