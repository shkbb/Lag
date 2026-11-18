//! Phase 1 of loading the library: enumerate the folder, sidecars, memory-cached thumbnails.

use super::clip::*;
use chrono::{Local, NaiveDateTime};
use lag_core::metadata::ClipMetadataStore;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::collections::HashMap;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// In-memory decoded thumbnail cache keyed by `thumbPath|LastWriteTimeUtcTicks`, cleared at 600 entries.
#[derive(Default)]
pub struct ThumbnailMemCache {
    pub(super) map: HashMap<String, SharedPixelBuffer<Rgba8Pixel>>,
}

impl ThumbnailMemCache {
    pub fn key_for(thumb_path: &Path) -> Option<String> {
        let meta = std::fs::metadata(thumb_path).ok()?;
        if !meta.is_file() {
            return None;
        }
        let ticks = meta.last_write_time() as i64 + DOTNET_WINDOWS_FILETIME_TICKS_OFFSET;
        Some(format!("{}|{ticks}", thumb_path.to_string_lossy()))
    }

    pub fn get(&self, thumb_path: &Path) -> Option<SharedPixelBuffer<Rgba8Pixel>> {
        let key = Self::key_for(thumb_path)?;
        self.map.get(&key).cloned()
    }

    pub fn insert(&mut self, thumb_path: &Path, buf: SharedPixelBuffer<Rgba8Pixel>) {
        let Some(key) = Self::key_for(thumb_path) else {
            return;
        };
        if self.map.len() >= THUMB_CACHE_LIMIT {
            self.map.clear();
        }
        self.map.insert(key, buf);
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.map.len()
    }
}

/// Phase 1 scan: top-level media enumeration, CreationTime descending sort, sidecars, and memory-cache lookup.
pub fn scan_library_phase1(
    library_dir: &Path,
    meta_store: Option<&ClipMetadataStore>,
    thumb_cache_dir: &Path,
    mem_cache: &Mutex<ThumbnailMemCache>,
    is_canceled: &dyn Fn() -> bool,
) -> std::io::Result<(Vec<ClipEntry>, String)> {
    if !library_dir.exists() {
        std::fs::create_dir_all(library_dir)?;
        return Ok((Vec::new(), String::new()));
    }

    struct ScannedFile {
        path: PathBuf,
        rank: usize,
        creation_ft: u64,
        created_local: NaiveDateTime,
        last_write_ticks: i64,
        len: i64,
    }

    let mut files = Vec::new();
    for entry in std::fs::read_dir(library_dir)? {
        if is_canceled() {
            return Ok((Vec::new(), String::new()));
        }
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        let Some(rank) = pattern_rank(&path) else {
            continue;
        };
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        let creation_ft = meta.creation_time();
        let created_local =
            meta.created().map(|st| chrono::DateTime::<Local>::from(st).naive_local()).unwrap_or_default();
        let last_write_ticks = meta.last_write_time() as i64 + DOTNET_WINDOWS_FILETIME_TICKS_OFFSET;
        files.push(ScannedFile { path, rank, creation_ft, created_local, last_write_ticks, len: meta.len() as i64 });
    }

    // OrderByDescending CreationTime, preserving pattern order on ties (LibraryViewModel.cs:197-200, 391).
    files.sort_by(|a, b| b.creation_ft.cmp(&a.creation_ft).then_with(|| a.rank.cmp(&b.rank)));

    let mut clips = Vec::with_capacity(files.len());
    let mut sig = String::with_capacity(files.len() * 64);

    for f in files {
        if is_canceled() {
            break;
        }
        let file_path = f.path.to_string_lossy().into_owned();
        let is_img = is_image_file(&f.path);

        sig.push_str(&file_path);
        sig.push('|');
        sig.push_str(&f.len.to_string());
        sig.push('|');
        sig.push_str(&f.last_write_ticks.to_string());
        sig.push('\n');

        let sidecar = meta_store.and_then(|s| s.read(&file_path));
        let stem = f.path.file_stem().and_then(|s| s.to_str()).unwrap_or_default();
        let edited = sidecar.as_ref().is_some_and(|m| m.edited) || contains_ignore_case(stem, "-edit");

        let thumb_path = if is_img {
            file_path.clone()
        } else {
            thumbnail_cache_path(thumb_cache_dir, &file_path).to_string_lossy().into_owned()
        };

        let cached_thumb = mem_cache.lock().ok().and_then(|c| c.get(Path::new(&thumb_path)));

        let duration_ticks = sidecar
            .as_ref()
            .and_then(|m| m.duration_seconds)
            .filter(|&s| s > 0.0 && s.is_finite())
            .map_or(0, |s| (s * 10_000_000.0).round() as i64);

        clips.push(ClipEntry {
            is_selected: false,
            is_favorite: sidecar.as_ref().is_some_and(|m| m.favorite),
            // normalized at read time too, so clips recorded before the fix join their game's group
            game: sidecar
                .as_ref()
                .and_then(|m| m.game.as_deref())
                .map(lag_core::metadata::normalize_game_name)
                .filter(|g| !g.is_empty()),
            exe: sidecar.and_then(|m| m.exe),
            is_edited: edited,
            is_image: is_img,
            file_path,
            thumbnail_path: thumb_path,
            thumbnail: cached_thumb,
            duration_ticks,
            created_date: f.created_local,
            file_size: f.len,
        });
    }

    Ok((clips, sig))
}
