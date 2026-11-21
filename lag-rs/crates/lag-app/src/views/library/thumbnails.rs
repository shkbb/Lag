//! Phase 2: durations and FFmpeg thumbnails filled in on worker threads behind the visible library.

use super::{scan::*, state::*, storage::*};
use image::imageops::FilterType;
use lag_core::metadata::ClipMetadataStore;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::cell::RefCell;
use std::os::windows::fs::MetadataExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

pub(super) fn probe_duration_secs(file_path: &str) -> Option<f64> {
    let ffprobe = crate::services::ffmpeg::resolve_tool("ffprobe.exe");
    let out = Command::new(&ffprobe)
        .creation_flags(crate::services::ffmpeg::CREATE_NO_WINDOW)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            file_path,
        ])
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    stdout.lines().find_map(|line| line.trim().parse::<f64>().ok()).filter(|&d| d > 0.0 && d.is_finite())
}

/// Library thumbnails fill this box keeping the video's shape (a 2580x1080 clip -> 516x216, 9:16 -> 384x683); the
/// cards crop them to their own shape (`image-fit: cover`). Owner, 2026-11-18: "if the clip is not 16:9 the
/// preview is deformed" - C# (and this port until now) forced `-s 320x180`, squashing every other shape.
pub(super) const THUMB_BOX: (u32, u32) = (384, 216);
/// The old forced size: a thumbnail of exactly this size may be squashed, so it is made again once.
pub(super) const LEGACY_THUMB_SIZE: (u32, u32) = (320, 180);

pub(super) fn thumbnail_filter() -> String {
    let (w, h) = THUMB_BOX;
    format!("scale={w}:{h}:force_original_aspect_ratio=increase:flags=bicubic")
}

/// A thumbnail written by the old code (squashed into 320x180 whatever the clip's shape).
pub(super) fn is_legacy_thumbnail(thumb_path: &Path) -> bool {
    image::image_dimensions(thumb_path).is_ok_and(|d| d == LEGACY_THUMB_SIZE)
}

pub(super) fn generate_video_thumbnail(file_path: &str, thumb_path: &Path, duration_secs: f64) -> Result<(), String> {
    if let Some(parent) = thumb_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let ffmpeg = crate::services::ffmpeg::resolve_tool("ffmpeg.exe");
    let ss = if duration_secs > 3.0 { "00:00:02" } else { "00:00:00" };
    let thumb_str = thumb_path.to_string_lossy();
    let out = Command::new(&ffmpeg)
        .creation_flags(crate::services::ffmpeg::CREATE_NO_WINDOW)
        .args([
            "-ss",
            ss,
            "-i",
            file_path,
            "-map",
            "0:v:0",
            "-c:v",
            "mjpeg",
            "-vframes",
            "1",
            "-vf",
            &thumbnail_filter(),
            "-q:v",
            "3",
            "-y",
            thumb_str.as_ref(),
        ])
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.trim().to_string());
    }
    Ok(())
}

pub(super) fn decode_image_to_buffer(
    path: &Path,
    max_width: Option<u32>,
) -> Result<SharedPixelBuffer<Rgba8Pixel>, String> {
    let reader =
        image::ImageReader::open(path).map_err(|e| e.to_string())?.with_guessed_format().map_err(|e| e.to_string())?;
    let mut img = reader.decode().map_err(|e| e.to_string())?;
    if let Some(w) = max_width {
        if img.width() > w && img.width() > 0 {
            let h = ((img.height() as u64 * w as u64) / img.width() as u64).max(1) as u32;
            img = img.resize(w, h, FilterType::Triangle);
        }
    }
    let rgba = img.to_rgba8();
    Ok(SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(rgba.as_raw(), rgba.width(), rgba.height()))
}

#[derive(Clone)]
pub(super) struct HydrateTask {
    pub(super) file_path: String,
    pub(super) thumbnail_path: String,
    pub(super) is_image: bool,
    pub(super) duration_ticks: i64,
    pub(super) has_thumbnail: bool,
}

pub(super) struct HydrateResult {
    pub(super) file_path: String,
    pub(super) buffer: Option<SharedPixelBuffer<Rgba8Pixel>>,
    pub(super) duration_ticks: i64,
}

pub(super) fn hydrate_single_clip(
    task: &HydrateTask,
    mem_cache: &Mutex<ThumbnailMemCache>,
    meta_store: Option<&ClipMetadataStore>,
    is_canceled: &dyn Fn() -> bool,
) -> Option<(Option<SharedPixelBuffer<Rgba8Pixel>>, i64)> {
    if task.has_thumbnail && (task.duration_ticks > 0 || task.is_image) {
        return None;
    }

    let mut buffer: Option<SharedPixelBuffer<Rgba8Pixel>> = None;
    let mut duration_ticks = task.duration_ticks;

    // a clip only in the cloud: nothing to probe or render here, only the thumbnail the cloud worker fetched
    if crate::services::cloud::stream::is_cloud_key(&task.file_path) {
        let thumb_path = Path::new(&task.thumbnail_path);
        if task.has_thumbnail || !thumb_path.exists() {
            return None;
        }
        let buf = decode_image_to_buffer(thumb_path, None).ok()?;
        if let Ok(mut cache) = mem_cache.lock() {
            cache.insert(thumb_path, buf.clone());
        }
        return Some((Some(buf), duration_ticks));
    }

    if task.is_image {
        let p = Path::new(&task.file_path);
        match decode_image_to_buffer(p, Some(640)) {
            Ok(buf) => {
                if let Ok(mut cache) = mem_cache.lock() {
                    cache.insert(p, buf.clone());
                }
                buffer = Some(buf);
            }
            Err(e) => {
                log::debug!("[Library] Failed to load screenshot {}: {e}", task.file_path);
            }
        }
    } else {
        let src_path = Path::new(&task.file_path);
        let thumb_path = Path::new(&task.thumbnail_path);

        if duration_ticks <= 0 {
            if let Some(secs) = probe_duration_secs(&task.file_path) {
                duration_ticks = (secs * 10_000_000.0).round() as i64;
                if duration_ticks > 0 {
                    if let Some(store) = meta_store {
                        store.set_duration(&task.file_path, secs);
                    }
                }
            } else {
                log::debug!("[FFProbe] Failed to get duration for {}", task.file_path);
            }
        }

        if is_canceled() {
            return None;
        }

        let needs_regen = match (std::fs::metadata(thumb_path), std::fs::metadata(src_path)) {
            (Ok(tm), Ok(sm)) => tm.last_write_time() < sm.last_write_time() || is_legacy_thumbnail(thumb_path),
            (Err(_), _) => true,
            _ => false,
        };
        if needs_regen {
            let secs = duration_ticks as f64 / 10_000_000.0;
            if let Err(e) = generate_video_thumbnail(&task.file_path, thumb_path, secs) {
                log::debug!("[FFMpeg] Failed to generate thumbnail for {}: {e}", task.file_path);
            }
        }

        if thumb_path.exists() {
            match decode_image_to_buffer(thumb_path, None) {
                Ok(buf) => {
                    if let Ok(mut cache) = mem_cache.lock() {
                        cache.insert(thumb_path, buf.clone());
                    }
                    buffer = Some(buf);
                }
                Err(e) => {
                    log::debug!("[FFMpeg] Failed to generate thumbnail for {}: {e}", task.file_path);
                }
            }
        }
    }

    if is_canceled() || (buffer.is_none() && duration_ticks == task.duration_ticks) {
        return None;
    }

    Some((buffer, duration_ticks))
}

/// Phase 2 of one library refresh: what to hydrate and where, plus the housekeeping that runs first.
pub(super) struct HydrationJob {
    /// This refresh's generation; a newer refresh (`refresh_gen` moved on) cancels the job.
    pub my_gen: u64,
    pub refresh_gen: Arc<AtomicU64>,
    pub library_dir: PathBuf,
    pub thumb_cache_dir: PathBuf,
    pub meta_store: Option<ClipMetadataStore>,
    /// Storage limit (oldest clips removed first) and whether to apply it before hydrating.
    pub auto_cleanup: bool,
    pub max_gb: i32,
    pub run_housekeeping: bool,
    pub mem_cache: Arc<Mutex<ThumbnailMemCache>>,
    pub tasks: Vec<HydrateTask>,
}

pub(super) fn start_phase2_hydration(
    job: HydrationJob,
    ui_weak: crate::Weak<crate::AppWindow>,
    state: Rc<RefCell<LibraryState>>,
) {
    let HydrationJob {
        my_gen,
        refresh_gen,
        library_dir,
        thumb_cache_dir,
        meta_store,
        auto_cleanup,
        max_gb,
        run_housekeeping,
        mem_cache,
        tasks,
    } = job;
    // Keep a thread-safe bridge for batched UI posting via invoke_from_event_loop.
    // LibraryState lives on the UI thread inside Rc<RefCell<_>>, so we pass a Weak
    // token via a UI-thread-only handle or look it up via a thread_local? Wait:
    // invoke_from_event_loop closure must be Send! Rc<RefCell<LibraryState>> is !Send.
    // Instead, we store a thread_local UI-thread callback/state registry keyed by u64 id!
    let state_id = state.borrow().registry_id;
    let _ = std::thread::Builder::new().name("lag-lib-hydrate".into()).spawn(move || {
        let canceled = || refresh_gen.load(Ordering::Acquire) != my_gen;
        if canceled() {
            return;
        }

        if run_housekeeping {
            let _ = std::fs::create_dir_all(&thumb_cache_dir);
            cleanup_legacy_thumbnail_folder(&library_dir);
            enforce_storage_quota(&library_dir, auto_cleanup, max_gb, meta_store.as_ref(), &thumb_cache_dir);
        }

        if tasks.is_empty() || canceled() {
            return;
        }

        let cpus = std::thread::available_parallelism().map_or(2, |n| n.get());
        let parallel = (cpus / 2).clamp(2, 6).min(tasks.len());
        let next_idx = AtomicUsize::new(0);
        let pending: Arc<Mutex<Vec<HydrateResult>>> = Arc::new(Mutex::new(Vec::new()));
        let flush_scheduled = Arc::new(AtomicBool::new(false));

        std::thread::scope(|scope| {
            for _ in 0..parallel {
                scope.spawn(|| loop {
                    if canceled() {
                        break;
                    }
                    let idx = next_idx.fetch_add(1, Ordering::Relaxed);
                    if idx >= tasks.len() {
                        break;
                    }
                    let task = &tasks[idx];
                    if let Some((buffer, duration_ticks)) =
                        hydrate_single_clip(task, &mem_cache, meta_store.as_ref(), &canceled)
                    {
                        if let Ok(mut q) = pending.lock() {
                            q.push(HydrateResult { file_path: task.file_path.clone(), buffer, duration_ticks });
                        }
                        if !flush_scheduled.swap(true, Ordering::AcqRel) {
                            let pending_c = Arc::clone(&pending);
                            let flush_c = Arc::clone(&flush_scheduled);
                            let gen_c = Arc::clone(&refresh_gen);
                            let ui_c = ui_weak.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                flush_c.store(false, Ordering::Release);
                                let batch =
                                    if let Ok(mut q) = pending_c.lock() { std::mem::take(&mut *q) } else { Vec::new() };
                                if batch.is_empty() || gen_c.load(Ordering::Acquire) != my_gen {
                                    return;
                                }
                                if let Some(ui) = ui_c.upgrade() {
                                    with_registered_state(state_id, |st| {
                                        st.apply_hydration_batch(&ui, batch);
                                    });
                                }
                            });
                        }
                    }
                });
            }
        });

        // Final drain in case a worker pushed right after flush_scheduled cleared.
        if !canceled() {
            let has_more = pending.lock().is_ok_and(|q| !q.is_empty());
            if has_more {
                let pending_c = Arc::clone(&pending);
                let gen_c = Arc::clone(&refresh_gen);
                let ui_c = ui_weak;
                let _ = slint::invoke_from_event_loop(move || {
                    let batch = if let Ok(mut q) = pending_c.lock() { std::mem::take(&mut *q) } else { Vec::new() };
                    if batch.is_empty() || gen_c.load(Ordering::Acquire) != my_gen {
                        return;
                    }
                    if let Some(ui) = ui_c.upgrade() {
                        with_registered_state(state_id, |st| {
                            st.apply_hydration_batch(&ui, batch);
                        });
                    }
                });
            }
        }
    });
}

// UI-thread registry so Send closures inside invoke_from_event_loop can access Rc<RefCell<LibraryState>> safely.
