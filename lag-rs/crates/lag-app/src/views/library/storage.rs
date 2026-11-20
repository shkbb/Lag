//! Library size limit (oldest clips removed first) and the disk usage bar.

use super::{clip::*, os::*};
use lag_core::metadata::ClipMetadataStore;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

/// Removes legacy `{library_dir}\.thumbnails` recursively (LibraryViewModel.cs:218-230).
pub fn cleanup_legacy_thumbnail_folder(library_dir: &Path) {
    let legacy = library_dir.join(".thumbnails");
    if legacy.exists() {
        if let Err(e) = std::fs::remove_dir_all(&legacy) {
            log::debug!("[Library] Legacy thumbnail cleanup failed: {e}");
        }
    }
}

/// Deletes oldest video files (excluding screenshots) until total video size <= MaxLibrarySizeGb (LibraryViewModel.cs:285-332).
pub fn enforce_storage_quota(
    library_dir: &Path,
    auto_cleanup_enabled: bool,
    max_library_size_gb: i32,
    meta_store: Option<&ClipMetadataStore>,
    thumb_cache_dir: &Path,
) {
    if !auto_cleanup_enabled {
        return;
    }
    let max_bytes = (max_library_size_gb as i64).saturating_mul(1024 * 1024 * 1024);
    enforce_storage_quota_bytes(library_dir, max_bytes, meta_store, thumb_cache_dir);
}

/// Byte-limit quota helper (testable without multi-GB test files).
pub fn enforce_storage_quota_bytes(
    library_dir: &Path,
    max_bytes: i64,
    meta_store: Option<&ClipMetadataStore>,
    thumb_cache_dir: &Path,
) {
    if max_bytes <= 0 || !library_dir.exists() {
        return;
    }

    let Ok(entries) = std::fs::read_dir(library_dir) else {
        return;
    };

    struct VideoFile {
        path: PathBuf,
        name: String,
        creation_ft: u64,
        len: i64,
    }

    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !is_video_file(&path) {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        files.push(VideoFile { path, name, creation_ft: meta.creation_time(), len: meta.len() as i64 });
    }

    files.sort_by_key(|f| f.creation_ft);
    let mut total_bytes: i64 = files.iter().map(|f| f.len).sum();
    if total_bytes <= max_bytes {
        return;
    }

    // With a cloud (docs/spec/cloud.md §11, lag_cloud::quota::plan_local_limit): first the local copies of
    // clips that are safely in a cloud (nothing is lost, they stay in the library as cloud clips), then the
    // oldest local-only clips as before; a clip being uploaded or downloaded is never touched.
    let guard = crate::services::cloud::local_guard();
    let key = |f: &VideoFile| f.path.to_string_lossy().to_lowercase();
    let (in_cloud, local_only): (Vec<VideoFile>, Vec<VideoFile>) =
        files.into_iter().filter(|f| !guard.busy.contains(&key(f))).partition(|f| guard.in_cloud.contains(&key(f)));

    for f in in_cloud.into_iter().chain(local_only) {
        if total_bytes <= max_bytes {
            break;
        }
        let size = f.len;
        let path_str = f.path.to_string_lossy().into_owned();
        if let Err(e) = std::fs::remove_file(&f.path) {
            log::debug!("[Storage] Failed to delete '{}': {e}", f.name);
            continue;
        }
        if let Some(store) = meta_store {
            store.delete(&path_str);
        }
        let thumb = thumbnail_cache_path(thumb_cache_dir, &path_str);
        if thumb.exists() {
            if let Err(e) = std::fs::remove_file(&thumb) {
                log::debug!("[Storage] Failed to delete '{}': {e}", f.name);
                continue;
            }
        }
        total_bytes -= size;
        log::info!("[Storage] Quota cleanup: deleted oldest clip '{}' ({} MB).", f.name, size / 1_048_576);
    }
}

pub(super) fn query_drive_usage_display(library_dir: &Path) -> Option<(String, f32)> {
    let abs = std::path::absolute(library_dir).ok()?;
    let mut root = PathBuf::new();
    for part in abs.components() {
        match part {
            std::path::Component::Prefix(_) | std::path::Component::RootDir => root.push(part),
            _ => break,
        }
    }
    if root.as_os_str().is_empty() {
        return None;
    }
    let root_wide = to_wide(&root.to_string_lossy());
    let (mut avail, mut total) = (0u64, 0u64);
    unsafe { GetDiskFreeSpaceExW(PCWSTR(root_wide.as_ptr()), Some(&mut avail), Some(&mut total), None) }.ok()?;
    if total == 0 {
        return None;
    }
    const GIB: f64 = 1_073_741_824.0;
    let free_gb = avail as f64 / GIB;
    let total_gb = total as f64 / GIB;
    let used_gb = total_gb - free_gb;
    let percent = 100.0 * (total.saturating_sub(avail)) as f64 / total as f64;
    let used_display = format!("{} / {:.0} GB", locale_fixed_one(used_gb), total_gb);
    let bar_width = (percent.clamp(0.0, 100.0) / 100.0 * 120.0) as f32;
    Some((used_display, bar_width))
}
