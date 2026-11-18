//! Library page logic (ViewModels/LibraryViewModel.cs, Views/LibraryView.axaml.cs, Models/ReplayClip.cs).
//! Spec: docs/spec/library-player.md.

use crate::app_ctx::AppCtx;
use crate::{GameChipData, LibraryCardData, LibraryRowData, LibraryVM};
use chrono::{Datelike, Local, NaiveDate, NaiveDateTime};
use image::imageops::FilterType;
use lag_core::metadata::ClipMetadataStore;
use lag_core::settings::SettingsData;
use slint::{ComponentHandle, Image, Model, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::os::windows::fs::MetadataExt;
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use windows::core::{w, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::Globalization::{GetDateFormatEx, GetLocaleInfoEx, ENUM_DATE_FORMATS_FLAGS, LOCALE_SDECIMAL};
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Videos, SHGetKnownFolderPath, ShellExecuteW, KF_FLAG_DEFAULT};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

/// Sentinel filter key for the "Desktop" chip (LibraryViewModel.cs:72).
pub const DESKTOP_FILTER_KEY: &str = "\u{0001}desktop";

/// Video container extensions in exact C# pattern order (LibraryViewModel.cs:184).
const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "mov", "avi", "gif"];
/// Screenshot extensions in exact C# pattern order (LibraryViewModel.cs:187).
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

const THUMB_CACHE_LIMIT: usize = 600;
const DOTNET_WINDOWS_FILETIME_TICKS_OFFSET: i64 = 504_911_232_000_000_000;

// Layout constants matching Views/LibraryView.axaml:288-336
const BODY_PAD_X: f32 = 38.0;
const BODY_PAD_TOP: f32 = 16.0;
const CARD_WIDTH: f32 = 248.0;
const CARD_GAP: f32 = 20.0;
const CARD_PITCH_X: f32 = CARD_WIDTH + CARD_GAP; // 268
const CARD_HEIGHT: f32 = 186.0;
const CARD_PITCH_Y: f32 = CARD_HEIGHT + CARD_GAP; // 206
const GROUP_HEADER_HEIGHT: f32 = 22.0;
const GROUP_HEADER_STEP: f32 = GROUP_HEADER_HEIGHT + 16.0; // 38
const GROUP_MARGIN_BOTTOM: f32 = 34.0;
const BODY_MARGIN_BOTTOM: f32 = 56.0;
const OVERSCAN_PX: f32 = 450.0;

#[derive(Debug, Clone)]
pub struct ClipEntry {
    pub is_selected: bool,
    pub is_favorite: bool,
    pub game: Option<String>,
    pub exe: Option<String>,
    pub is_edited: bool,
    pub is_image: bool,
    pub file_path: String,
    pub thumbnail_path: String,
    pub thumbnail: Option<SharedPixelBuffer<Rgba8Pixel>>,
    /// Signed .NET TimeSpan ticks (100 ns).
    pub duration_ticks: i64,
    pub created_date: NaiveDateTime,
    pub file_size: i64,
}

impl ClipEntry {
    pub fn has_game(&self) -> bool {
        self.game.as_ref().is_some_and(|s| !s.trim_matches(dotnet_whitespace).is_empty())
    }

    pub fn display_name(&self) -> &str {
        let name = self.file_path.rsplit(['\\', '/']).next().unwrap_or("");
        name.rsplit_once('.').map_or(name, |(stem, _)| stem)
    }

    pub fn duration_display(&self) -> String {
        let seconds = self.duration_ticks.unsigned_abs() / 10_000_000;
        if self.duration_ticks >= 36_000_000_000 {
            format!("{}:{:02}:{:02}", seconds / 3600 % 24, seconds / 60 % 60, seconds % 60)
        } else {
            format!("{}:{:02}", seconds / 60 % 60, seconds % 60)
        }
    }

    pub fn time_hh_mm(&self) -> String {
        self.created_date.format("%H:%M").to_string()
    }
}

impl From<&ClipEntry> for crate::app_ctx::SharedClip {
    fn from(c: &ClipEntry) -> Self {
        Self {
            is_selected: c.is_selected,
            is_favorite: c.is_favorite,
            game: c.game.clone(),
            exe: c.exe.clone(),
            is_edited: c.is_edited,
            is_image: c.is_image,
            file_path: c.file_path.clone(),
            thumbnail_path: c.thumbnail_path.clone(),
            thumbnail: c.thumbnail.clone(),
            duration_ticks: c.duration_ticks,
            created_date: c.created_date,
            file_size: c.file_size,
        }
    }
}

fn dotnet_whitespace(c: char) -> bool {
    c.is_whitespace() && !matches!(c, '\u{1c}'..='\u{1f}')
}

fn eq_ignore_case(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        a.eq_ignore_ascii_case(b)
    } else {
        a.to_lowercase() == b.to_lowercase()
    }
}

fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    if haystack.is_ascii() && needle.is_ascii() {
        let h = haystack.as_bytes();
        let n = needle.as_bytes();
        h.windows(n.len()).any(|w| w.eq_ignore_ascii_case(n))
    } else {
        haystack.to_lowercase().contains(&needle.to_lowercase())
    }
}

pub fn is_image_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| IMAGE_EXTENSIONS.iter().any(|p| ext.eq_ignore_ascii_case(p)))
}

fn pattern_rank(path: &Path) -> Option<usize> {
    let ext = path.extension()?.to_str()?;
    VIDEO_EXTENSIONS.iter().chain(IMAGE_EXTENSIONS.iter()).position(|p| ext.eq_ignore_ascii_case(p))
}

fn is_video_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| VIDEO_EXTENSIONS.iter().any(|p| ext.eq_ignore_ascii_case(p)))
}

/// Deterministic disk thumbnail path: `%LocalAppData%\Lag\thumbnails\{UPPER_MD5(lower_utf8_path)}.jpg`.
pub fn thumbnail_cache_path(thumb_cache_dir: &Path, video_path: &str) -> PathBuf {
    thumb_cache_dir.join(format!("{}.jpg", lag_core::metadata::path_hash(video_path)))
}

pub fn default_thumbnail_cache_dir() -> Option<PathBuf> {
    lag_core::hardware::local_appdata().map(|p| p.join("Lag").join("thumbnails"))
}

pub fn resolve_library_dir(settings: &SettingsData) -> PathBuf {
    if let Some(p) = settings.library_path.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        return PathBuf::from(p);
    }
    videos_folder().unwrap_or_else(|| PathBuf::from(".")).join("Lag")
}

/// In-memory decoded thumbnail cache keyed by `thumbPath|LastWriteTimeUtcTicks`, cleared at 600 entries.
#[derive(Default)]
pub struct ThumbnailMemCache {
    map: HashMap<String, SharedPixelBuffer<Rgba8Pixel>>,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChipModel {
    pub label: String,
    pub filter_key: String,
    pub is_active: bool,
}

/// Rebuilds category chips from ALL clips and clears stale active filter (LibraryViewModel.cs:76-94).
pub fn build_game_chips(
    clips: &[ClipEntry],
    active_filter: &mut Option<String>,
    desktop_label: &str,
) -> Vec<ChipModel> {
    let mut seen = HashSet::<String>::new();
    let mut games = Vec::<String>::new();
    for clip in clips {
        if clip.has_game() {
            if let Some(ref g) = clip.game {
                let lower = g.to_lowercase();
                if seen.insert(lower) {
                    games.push(g.clone());
                }
            }
        }
    }
    games.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()));
    let desktop_present = clips.iter().any(|c| !c.has_game());

    if let Some(ref current) = *active_filter {
        if current == DESKTOP_FILTER_KEY {
            if !desktop_present {
                *active_filter = None;
            }
        } else if !games.iter().any(|g| eq_ignore_case(g, current)) {
            *active_filter = None;
        }
    }

    let mut chips = Vec::with_capacity(games.len() + usize::from(desktop_present));
    for g in games {
        let active = active_filter.as_deref().is_some_and(|f| eq_ignore_case(&g, f));
        chips.push(ChipModel { label: g.clone(), filter_key: g, is_active: active });
    }
    if desktop_present {
        chips.push(ChipModel {
            label: desktop_label.to_string(),
            filter_key: DESKTOP_FILTER_KEY.to_string(),
            is_active: active_filter.as_deref() == Some(DESKTOP_FILTER_KEY),
        });
    }
    chips
}

/// Filters clips with AND of favorites, game/Desktop category, and trimmed search (LibraryViewModel.cs:97-112).
pub fn filter_clips<'a>(
    clips: &'a [ClipEntry],
    favorites_only: bool,
    game_filter: Option<&str>,
    search_text: &str,
) -> Vec<(usize, &'a ClipEntry)> {
    let q = search_text.trim();
    clips
        .iter()
        .enumerate()
        .filter(|(_, c)| {
            if favorites_only && !c.is_favorite {
                return false;
            }
            match game_filter {
                Some(DESKTOP_FILTER_KEY) => {
                    if c.has_game() {
                        return false;
                    }
                }
                Some(game) => {
                    if !c.game.as_deref().is_some_and(|g| eq_ignore_case(g, game)) {
                        return false;
                    }
                }
                None => {}
            }
            if !q.is_empty() {
                let name_match = contains_ignore_case(c.display_name(), q);
                let game_match = c.game.as_deref().is_some_and(|g| contains_ignore_case(g, q));
                if !name_match && !game_match {
                    return false;
                }
            }
            true
        })
        .collect()
}

pub trait DateFormatProvider {
    fn format_d_mmm(&self, date: NaiveDate) -> String;
    fn format_d_mmmm(&self, date: NaiveDate) -> String;
    fn format_dddd(&self, date: NaiveDate) -> String;
}

struct Win32DateFormatter;

impl DateFormatProvider for Win32DateFormatter {
    fn format_d_mmm(&self, date: NaiveDate) -> String {
        win32_date_format(date, "d MMM").unwrap_or_else(|| date.format("%-d %b").to_string())
    }
    fn format_d_mmmm(&self, date: NaiveDate) -> String {
        win32_date_format(date, "d MMMM").unwrap_or_else(|| date.format("%-d %B").to_string())
    }
    fn format_dddd(&self, date: NaiveDate) -> String {
        win32_date_format(date, "dddd").unwrap_or_else(|| date.format("%A").to_string())
    }
}

#[derive(Debug, Clone)]
pub struct DayGroup {
    pub label: String,
    pub sub: String,
    pub count_label: String,
    pub clip_indices: Vec<usize>,
}

/// Buckets filtered clips by local creation date descending (LibraryViewModel.cs:113-147).
pub fn group_clips_by_day(
    filtered: &[(usize, &ClipEntry)],
    today: NaiveDate,
    fmt: &dyn DateFormatProvider,
) -> Vec<DayGroup> {
    let mut by_day: HashMap<NaiveDate, Vec<usize>> = HashMap::new();
    for &(idx, clip) in filtered {
        by_day.entry(clip.created_date.date()).or_default().push(idx);
    }

    let mut dates: Vec<NaiveDate> = by_day.keys().copied().collect();
    dates.sort_by(|a, b| b.cmp(a));

    let yesterday = today.pred_opt().unwrap_or(today);
    dates
        .into_iter()
        .map(|date| {
            let clip_indices = by_day.remove(&date).unwrap_or_default();
            let (label, sub) = if date == today {
                (lag_core::i18n::tr("Library_Today").to_uppercase(), fmt.format_d_mmm(date))
            } else if date == yesterday {
                (lag_core::i18n::tr("Library_Yesterday").to_uppercase(), fmt.format_d_mmm(date))
            } else {
                (fmt.format_d_mmmm(date).to_uppercase(), fmt.format_dddd(date))
            };
            let count_str = clip_indices.len().to_string();
            let count_label = lag_core::i18n::format("Library_GroupCount", &[&count_str])
                .unwrap_or_else(|_| format!("{count_str} clips"));
            DayGroup { label, sub, count_label, clip_indices }
        })
        .collect()
}

#[derive(Debug, Clone)]
enum VirtualRowKind {
    Header { label: String, sub: String, count: String },
    Cards { clip_indices: Vec<usize> },
}

#[derive(Debug, Clone)]
struct VirtualRow {
    y: f32,
    height: f32,
    kind: VirtualRowKind,
}

fn columns_for_viewport(viewport_width: f32) -> usize {
    let avail = (viewport_width - BODY_PAD_X * 2.0).max(CARD_PITCH_X);
    ((avail / CARD_PITCH_X).floor() as usize).max(1)
}

fn layout_virtual_rows(groups: &[DayGroup], cols: usize) -> (Vec<VirtualRow>, f32) {
    if groups.is_empty() {
        return (Vec::new(), 0.0);
    }
    let cols = cols.max(1);
    let mut rows = Vec::new();
    let mut y = BODY_PAD_TOP;

    for g in groups {
        rows.push(VirtualRow {
            y,
            height: GROUP_HEADER_HEIGHT,
            kind: VirtualRowKind::Header { label: g.label.clone(), sub: g.sub.clone(), count: g.count_label.clone() },
        });
        y += GROUP_HEADER_STEP;

        for chunk in g.clip_indices.chunks(cols) {
            rows.push(VirtualRow {
                y,
                height: CARD_HEIGHT,
                kind: VirtualRowKind::Cards { clip_indices: chunk.to_vec() },
            });
            y += CARD_PITCH_Y;
        }

        y += GROUP_MARGIN_BOTTOM;
    }

    y += BODY_MARGIN_BOTTOM;
    (rows, y)
}

fn visible_row_range(rows: &[VirtualRow], scroll_y: f32, viewport_h: f32) -> (usize, usize) {
    if rows.is_empty() {
        return (0, 0);
    }
    let min_y = (scroll_y - OVERSCAN_PX).max(0.0);
    let max_y = scroll_y + viewport_h.max(600.0) + OVERSCAN_PX;

    let first = rows.partition_point(|r| r.y + r.height < min_y);
    let last = rows.partition_point(|r| r.y <= max_y);
    let first = first.min(rows.len());
    let last = last.max(first).min(rows.len());
    (first, last)
}

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

    for f in files {
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

/// Splits the configured save-replay hotkey into keycap strings (SettingsViewModel.cs:151-152, 1682-1700).
pub fn hotkey_parts_from_settings(settings: &SettingsData) -> Vec<String> {
    let key = settings.hotkey_key.as_deref().unwrap_or("VcF10");
    if key == "VcUndefined" || key.is_empty() {
        return vec!["—".to_string()];
    }
    let mods = settings.hotkey_modifiers.as_deref().unwrap_or("LeftAlt");
    let mut parts = Vec::new();
    if contains_ignore_case(mods, "Ctrl") {
        parts.push("Ctrl".to_string());
    }
    if contains_ignore_case(mods, "Alt") {
        parts.push("Alt".to_string());
    }
    if contains_ignore_case(mods, "Shift") {
        parts.push("Shift".to_string());
    }
    parts.push(key.replace("Vc", ""));
    parts
}

fn probe_duration_secs(file_path: &str) -> Option<f64> {
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
const THUMB_BOX: (u32, u32) = (384, 216);
/// The old forced size: a thumbnail of exactly this size may be squashed, so it is made again once.
const LEGACY_THUMB_SIZE: (u32, u32) = (320, 180);

fn thumbnail_filter() -> String {
    let (w, h) = THUMB_BOX;
    format!("scale={w}:{h}:force_original_aspect_ratio=increase:flags=bicubic")
}

/// A thumbnail written by the old code (squashed into 320x180 whatever the clip's shape).
fn is_legacy_thumbnail(thumb_path: &Path) -> bool {
    image::image_dimensions(thumb_path).is_ok_and(|d| d == LEGACY_THUMB_SIZE)
}

fn generate_video_thumbnail(file_path: &str, thumb_path: &Path, duration_secs: f64) -> Result<(), String> {
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

fn decode_image_to_buffer(path: &Path, max_width: Option<u32>) -> Result<SharedPixelBuffer<Rgba8Pixel>, String> {
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
struct HydrateTask {
    file_path: String,
    thumbnail_path: String,
    is_image: bool,
    duration_ticks: i64,
    has_thumbnail: bool,
}

struct HydrateResult {
    file_path: String,
    buffer: Option<SharedPixelBuffer<Rgba8Pixel>>,
    duration_ticks: i64,
}

fn hydrate_single_clip(
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

fn start_phase2_hydration(
    my_gen: u64,
    refresh_gen: Arc<AtomicU64>,
    library_dir: PathBuf,
    thumb_cache_dir: PathBuf,
    meta_store: Option<ClipMetadataStore>,
    auto_cleanup: bool,
    max_gb: i32,
    run_housekeeping: bool,
    mem_cache: Arc<Mutex<ThumbnailMemCache>>,
    tasks: Vec<HydrateTask>,
    ui_weak: slint::Weak<crate::AppWindow>,
    state: Rc<RefCell<LibraryState>>,
) {
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
thread_local! {
    static STATE_REGISTRY: RefCell<HashMap<u64, std::rc::Weak<RefCell<LibraryState>>>> = RefCell::default();
    static NEXT_STATE_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

fn register_state(state: &Rc<RefCell<LibraryState>>) -> u64 {
    let id = NEXT_STATE_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    state.borrow_mut().registry_id = id;
    STATE_REGISTRY.with(|r| {
        r.borrow_mut().insert(id, Rc::downgrade(state));
    });
    id
}

fn with_registered_state(id: u64, f: impl FnOnce(&mut LibraryState)) {
    let weak = STATE_REGISTRY.with(|r| r.borrow().get(&id).cloned());
    if let Some(st) = weak.and_then(|w| w.upgrade()) {
        if let Ok(mut guard) = st.try_borrow_mut() {
            f(&mut guard);
        }
    }
}

struct LibraryState {
    registry_id: u64,
    ctx: Rc<AppCtx>,
    ui: slint::Weak<crate::AppWindow>,
    meta_store: Option<ClipMetadataStore>,
    thumb_cache_dir: PathBuf,
    mem_cache: Arc<Mutex<ThumbnailMemCache>>,
    refresh_gen: Arc<AtomicU64>,
    published_signature: Option<String>,

    clips: Vec<ClipEntry>,
    clip_path_to_idx: HashMap<String, usize>,
    search_text: String,
    favorites_only: bool,
    game_filter: Option<String>,
    selection_anchor: Option<String>,

    groups: Vec<DayGroup>,
    virtual_rows: Vec<VirtualRow>,
    total_content_height: f32,

    scroll_y: f32,
    viewport_w: f32,
    viewport_h: f32,
    cols: usize,

    visible_rows_model: Rc<VecModel<LibraryRowData>>,
    visible_window: (usize, usize),
    visible_card_models: HashMap<usize, Rc<VecModel<LibraryCardData>>>,
    clip_to_visible_pos: HashMap<usize, (usize, usize)>,
}

impl LibraryState {
    fn new(ui: &crate::AppWindow, ctx: Rc<AppCtx>) -> Self {
        let thumb_cache_dir = default_thumbnail_cache_dir().unwrap_or_else(|| PathBuf::from("Lag").join("thumbnails"));
        let visible_rows_model = Rc::new(VecModel::default());
        ui.global::<LibraryVM>().set_visible_rows(ModelRc::from(visible_rows_model.clone()));

        Self {
            registry_id: 0,
            ctx,
            ui: ui.as_weak(),
            meta_store: ClipMetadataStore::for_user(),
            thumb_cache_dir,
            mem_cache: Arc::new(Mutex::new(ThumbnailMemCache::default())),
            refresh_gen: Arc::new(AtomicU64::new(0)),
            published_signature: None,
            clips: Vec::new(),
            clip_path_to_idx: HashMap::new(),
            search_text: String::new(),
            favorites_only: false,
            game_filter: None,
            selection_anchor: None,
            groups: Vec::new(),
            virtual_rows: Vec::new(),
            total_content_height: 0.0,
            scroll_y: 0.0,
            viewport_w: 1200.0,
            viewport_h: 700.0,
            cols: columns_for_viewport(1200.0),
            visible_rows_model,
            visible_window: (0, 0),
            visible_card_models: HashMap::new(),
            clip_to_visible_pos: HashMap::new(),
        }
    }

    fn publish_clips(&self) {
        let shared: Vec<crate::app_ctx::SharedClip> = self.clips.iter().map(Into::into).collect();
        self.ctx.set_clips(shared);
    }

    fn rebuild_path_index(&mut self) {
        self.clip_path_to_idx.clear();
        for (i, c) in self.clips.iter().enumerate() {
            self.clip_path_to_idx.insert(c.file_path.clone(), i);
        }
    }

    fn to_card_data(&self, clip_idx: usize) -> LibraryCardData {
        let c = &self.clips[clip_idx];
        let (thumbnail, has_thumbnail) = match &c.thumbnail {
            Some(buf) => (Image::from_rgba8(buf.clone()), true),
            None => (Image::default(), false),
        };
        LibraryCardData {
            clip_index: clip_idx as i32,
            file_path: SharedString::from(c.file_path.as_str()),
            title: SharedString::from(c.display_name()),
            time_text: SharedString::from(c.time_hh_mm()),
            game_name: SharedString::from(c.game.as_deref().unwrap_or("")),
            duration_text: SharedString::from(if c.is_image { String::new() } else { c.duration_display() }),
            thumbnail,
            has_thumbnail,
            is_image: c.is_image,
            is_favorite: c.is_favorite,
            is_edited: c.is_edited,
            is_selected: c.is_selected,
        }
    }

    fn build_slint_row(&mut self, vrow_idx: usize) -> LibraryRowData {
        let row = &self.virtual_rows[vrow_idx];
        match &row.kind {
            VirtualRowKind::Header { label, sub, count } => LibraryRowData {
                kind: 0,
                y: row.y,
                height: row.height,
                header_label: SharedString::from(label.as_str()),
                header_sub: SharedString::from(sub.as_str()),
                header_count: SharedString::from(count.as_str()),
                cards: ModelRc::default(),
            },
            VirtualRowKind::Cards { clip_indices } => {
                let cards_vec: Vec<LibraryCardData> = clip_indices.iter().map(|&idx| self.to_card_data(idx)).collect();
                let cards_model = Rc::new(VecModel::from(cards_vec));
                self.visible_card_models.insert(vrow_idx, cards_model.clone());
                LibraryRowData {
                    kind: 1,
                    y: row.y,
                    height: row.height,
                    header_label: SharedString::default(),
                    header_sub: SharedString::default(),
                    header_count: SharedString::default(),
                    cards: ModelRc::from(cards_model),
                }
            }
        }
    }

    fn rebuild_clip_to_visible_pos(&mut self) {
        self.clip_to_visible_pos.clear();
        let (first, last) = self.visible_window;
        for vrow_idx in first..last {
            if let Some(VirtualRow { kind: VirtualRowKind::Cards { clip_indices }, .. }) =
                self.virtual_rows.get(vrow_idx)
            {
                for (col_idx, &clip_idx) in clip_indices.iter().enumerate() {
                    self.clip_to_visible_pos.insert(clip_idx, (vrow_idx, col_idx));
                }
            }
        }
    }

    fn sync_visible_window(&mut self, force_rebuild: bool) {
        let (new_first, new_last) = visible_row_range(&self.virtual_rows, self.scroll_y, self.viewport_h);
        let (mut cur_first, mut cur_last) = self.visible_window;

        if !force_rebuild && (new_first, new_last) == (cur_first, cur_last) {
            return;
        }

        if force_rebuild || new_last <= cur_first || new_first >= cur_last || cur_first >= cur_last {
            self.visible_card_models.clear();
            let mut items = Vec::with_capacity(new_last.saturating_sub(new_first));
            for vrow_idx in new_first..new_last {
                items.push(self.build_slint_row(vrow_idx));
            }
            self.visible_rows_model.set_vec(items);
            self.visible_window = (new_first, new_last);
            self.rebuild_clip_to_visible_pos();
            return;
        }

        // Diff overlapping range so remaining rows stay intact in Slint without flicker.
        while cur_first < new_first && cur_first < cur_last {
            self.visible_rows_model.remove(0);
            self.visible_card_models.remove(&cur_first);
            cur_first += 1;
        }
        while cur_last > new_last && cur_last > cur_first {
            cur_last -= 1;
            self.visible_rows_model.remove(cur_last - cur_first);
            self.visible_card_models.remove(&cur_last);
        }
        if new_first < cur_first {
            for vrow_idx in (new_first..cur_first).rev() {
                let row_data = self.build_slint_row(vrow_idx);
                self.visible_rows_model.insert(0, row_data);
            }
        }
        if new_last > cur_last {
            for vrow_idx in cur_last..new_last {
                let row_data = self.build_slint_row(vrow_idx);
                self.visible_rows_model.push(row_data);
            }
        }
        self.visible_window = (new_first, new_last);
        self.rebuild_clip_to_visible_pos();
    }

    fn update_visible_card(&self, clip_idx: usize) {
        if let Some(&(vrow_idx, col_idx)) = self.clip_to_visible_pos.get(&clip_idx) {
            if let Some(model) = self.visible_card_models.get(&vrow_idx) {
                model.set_row_data(col_idx, self.to_card_data(clip_idx));
            }
        }
    }

    fn refresh_all_visible_cards(&self) {
        for (&clip_idx, &(vrow_idx, col_idx)) in &self.clip_to_visible_pos {
            if let Some(model) = self.visible_card_models.get(&vrow_idx) {
                model.set_row_data(col_idx, self.to_card_data(clip_idx));
            }
        }
    }

    fn rebuild_game_chips(&mut self, ui: &crate::AppWindow) {
        let desktop_label = lag_core::i18n::tr("Library_Desktop");
        let chips = build_game_chips(&self.clips, &mut self.game_filter, desktop_label);
        let slint_chips: Vec<GameChipData> = chips
            .into_iter()
            .map(|c| GameChipData {
                label: SharedString::from(c.label),
                filter_key: SharedString::from(c.filter_key),
                is_active: c.is_active,
            })
            .collect();
        ui.global::<LibraryVM>().set_game_chips(ModelRc::from(Rc::new(VecModel::from(slint_chips))));
    }

    fn rebuild_groups(&mut self, ui: &crate::AppWindow) {
        let filtered = filter_clips(&self.clips, self.favorites_only, self.game_filter.as_deref(), &self.search_text);
        let today = Local::now().date_naive();
        let fmt = Win32DateFormatter;
        self.groups = group_clips_by_day(&filtered, today, &fmt);
        let (vrows, total_h) = layout_virtual_rows(&self.groups, self.cols);
        self.virtual_rows = vrows;
        self.total_content_height = total_h;
        self.scroll_y = self.scroll_y.clamp(0.0, (total_h - self.viewport_h).max(0.0));

        let vm = ui.global::<LibraryVM>();
        vm.set_total_content_height(total_h);
        vm.set_has_clips(!self.clips.is_empty());
        vm.set_has_no_results(!self.clips.is_empty() && self.groups.is_empty());

        self.sync_visible_window(true);
    }

    fn rebuild_view(&mut self, ui: &crate::AppWindow) {
        self.rebuild_game_chips(ui);
        self.rebuild_groups(ui);
        let count_str = self.clips.len().to_string();
        let label =
            lag_core::i18n::format("Library_ClipCount", &[&count_str]).unwrap_or_else(|_| format!("{count_str} CLIPS"));
        ui.global::<LibraryVM>().set_clip_count_label(label.into());
    }

    fn notify_selection_changed(&self, ui: &crate::AppWindow) {
        let selected_count = self.clips.iter().filter(|c| c.is_selected).count();
        let has_sel = selected_count > 0;
        let vm = ui.global::<LibraryVM>();
        vm.set_has_selection(has_sel);
        let del_word = lag_core::i18n::tr("Library_Delete");
        vm.set_delete_selected_label(format!("{del_word} ({selected_count})").into());
    }

    fn clear_selection(&mut self, ui: &crate::AppWindow) {
        let mut any = false;
        for c in &mut self.clips {
            if c.is_selected {
                c.is_selected = false;
                any = true;
            }
        }
        self.selection_anchor = None;
        if any {
            self.refresh_all_visible_cards();
        }
        self.notify_selection_changed(ui);
    }

    fn toggle_select_clip(&mut self, ui: &crate::AppWindow, clip_idx: usize) {
        let Some(c) = self.clips.get_mut(clip_idx) else {
            return;
        };
        c.is_selected = !c.is_selected;
        if c.is_selected {
            self.selection_anchor = Some(c.file_path.clone());
        }
        self.update_visible_card(clip_idx);
        self.notify_selection_changed(ui);
    }

    fn visible_clip_order(&self) -> Vec<usize> {
        let mut order = Vec::new();
        for g in &self.groups {
            order.extend_from_slice(&g.clip_indices);
        }
        order
    }

    fn select_range(&mut self, ui: &crate::AppWindow, target_idx: usize) {
        let order = self.visible_clip_order();
        let Some(to) = order.iter().position(|&idx| idx == target_idx) else {
            return;
        };
        let from = self
            .selection_anchor
            .as_deref()
            .and_then(|anchor_path| self.clip_path_to_idx.get(anchor_path).copied())
            .and_then(|anchor_idx| order.iter().position(|&idx| idx == anchor_idx));

        let Some(from) = from else {
            if let Some(c) = self.clips.get_mut(target_idx) {
                c.is_selected = true;
                self.selection_anchor = Some(c.file_path.clone());
            }
            self.update_visible_card(target_idx);
            self.notify_selection_changed(ui);
            return;
        };

        let (lo, hi) = (from.min(to), from.max(to));
        for (pos, &idx) in order.iter().enumerate() {
            if let Some(c) = self.clips.get_mut(idx) {
                c.is_selected = pos >= lo && pos <= hi;
            }
        }
        self.refresh_all_visible_cards();
        self.notify_selection_changed(ui);
    }

    fn select_all(&mut self, ui: &crate::AppWindow) {
        for idx in self.visible_clip_order() {
            if let Some(c) = self.clips.get_mut(idx) {
                c.is_selected = true;
            }
        }
        self.refresh_all_visible_cards();
        self.notify_selection_changed(ui);
    }

    fn toggle_favorite(&mut self, ui: &crate::AppWindow, clip_idx: usize) {
        let Some(c) = self.clips.get_mut(clip_idx) else {
            return;
        };
        c.is_favorite = !c.is_favorite;
        let path = c.file_path.clone();
        let fav = c.is_favorite;
        if let Some(ref store) = self.meta_store {
            store.set_favorite(&path, fav);
        }
        if self.favorites_only {
            self.rebuild_groups(ui);
        } else {
            self.update_visible_card(clip_idx);
        }
    }

    fn delete_clip_by_path(&mut self, file_path: &str) -> bool {
        let Some(&idx) = self.clip_path_to_idx.get(file_path) else {
            return false;
        };
        let thumb_path = self.clips[idx].thumbnail_path.clone();
        let res: std::io::Result<()> = (|| {
            if Path::new(file_path).exists() {
                std::fs::remove_file(file_path)?;
            }
            if Path::new(&thumb_path).exists() {
                std::fs::remove_file(&thumb_path)?;
            }
            if let Some(ref store) = self.meta_store {
                store.delete(file_path);
            }
            Ok(())
        })();
        if let Err(e) = res {
            log::debug!("Failed to delete clip: {e}");
            return false;
        }
        self.clips.remove(idx);
        self.rebuild_path_index();
        true
    }

    fn delete_single(&mut self, ui: &crate::AppWindow, file_path: &str) {
        if self.delete_clip_by_path(file_path) {
            self.rebuild_view(ui);
            self.publish_clips();
        }
        self.notify_selection_changed(ui);
    }

    fn delete_selected(&mut self, ui: &crate::AppWindow) {
        let targets: Vec<String> = self.clips.iter().filter(|c| c.is_selected).map(|c| c.file_path.clone()).collect();
        let mut deleted_any = false;
        for path in targets {
            if self.delete_clip_by_path(&path) {
                deleted_any = true;
            }
        }
        self.selection_anchor = None;
        if deleted_any {
            self.rebuild_view(ui);
            self.publish_clips();
        }
        self.notify_selection_changed(ui);
    }

    fn apply_hydration_batch(&mut self, ui: &crate::AppWindow, batch: Vec<HydrateResult>) {
        let _ = ui;
        let mut any_changed = false;
        for item in batch {
            let Some(&idx) = self.clip_path_to_idx.get(&item.file_path) else {
                continue;
            };
            let c = &mut self.clips[idx];
            let mut changed = false;
            if let Some(buf) = item.buffer {
                c.thumbnail = Some(buf);
                changed = true;
            }
            if item.duration_ticks > 0 && c.duration_ticks != item.duration_ticks {
                c.duration_ticks = item.duration_ticks;
                changed = true;
            }
            if changed {
                any_changed = true;
                self.update_visible_card(idx);
            }
        }
        if any_changed {
            self.publish_clips();
        }
    }

    fn on_viewport_changed(&mut self, scroll_y: f32, viewport_w: f32, viewport_h: f32) {
        let new_w = viewport_w.max(300.0);
        let new_h = viewport_h.max(200.0);
        let new_cols = columns_for_viewport(new_w);
        self.scroll_y = scroll_y.max(0.0);
        self.viewport_w = new_w;
        self.viewport_h = new_h;

        if new_cols != self.cols {
            self.cols = new_cols;
            let (vrows, total_h) = layout_virtual_rows(&self.groups, self.cols);
            self.virtual_rows = vrows;
            self.total_content_height = total_h;
            if let Some(ui) = self.ui.upgrade() {
                ui.global::<LibraryVM>().set_total_content_height(total_h);
            }
            self.sync_visible_window(true);
        } else {
            self.sync_visible_window(false);
        }
    }

    fn update_free_space(ui: &crate::AppWindow, library_dir: &Path) {
        if let Some((used_display, bar_width)) = query_drive_usage_display(library_dir) {
            let vm = ui.global::<LibraryVM>();
            vm.set_used_space_display(used_display.into());
            vm.set_used_space_bar_width(bar_width);
        }
    }

    fn start_refresh(state_rc: &Rc<RefCell<Self>>) {
        let (my_gen, refresh_gen, settings, meta_store, thumb_cache_dir, mem_cache, ui_weak, state_id) = {
            let st = state_rc.borrow();
            let my_gen = st.refresh_gen.fetch_add(1, Ordering::AcqRel) + 1;
            (
                my_gen,
                Arc::clone(&st.refresh_gen),
                st.ctx.settings(),
                st.meta_store.clone(),
                st.thumb_cache_dir.clone(),
                Arc::clone(&st.mem_cache),
                st.ui.clone(),
                st.registry_id,
            )
        };

        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<LibraryVM>().set_is_loading(true);
        }

        let library_dir = resolve_library_dir(&settings);
        let auto_cleanup = settings.auto_cleanup_enabled;
        let max_gb = settings.max_library_size_gb;

        let _ = std::thread::Builder::new().name("lag-lib-scan".into()).spawn(move || {
            let canceled = || refresh_gen.load(Ordering::Acquire) != my_gen;
            let scan_res =
                scan_library_phase1(&library_dir, meta_store.as_ref(), &thumb_cache_dir, &mem_cache, &canceled);
            if canceled() {
                return;
            }
            let (built, signature) = match scan_res {
                Ok(v) => v,
                Err(e) => {
                    log::debug!("[Library] scan failed: {e}");
                    let _ = slint::invoke_from_event_loop(move || {
                        if refresh_gen.load(Ordering::Acquire) == my_gen {
                            if let Some(ui) = ui_weak.upgrade() {
                                LibraryState::update_free_space(&ui, &library_dir);
                                ui.global::<LibraryVM>().set_is_loading(false);
                            }
                        }
                    });
                    return;
                }
            };

            let _ = slint::invoke_from_event_loop(move || {
                if refresh_gen.load(Ordering::Acquire) != my_gen {
                    return;
                }
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                let weak_st = STATE_REGISTRY.with(|r| r.borrow().get(&state_id).cloned());
                let Some(st_rc) = weak_st.and_then(|w| w.upgrade()) else {
                    return;
                };

                let tasks_and_hk: Option<(Vec<HydrateTask>, bool)> = {
                    let mut st = st_rc.borrow_mut();
                    let unchanged = st.published_signature.as_deref().is_some_and(|prev| prev == signature)
                        && st.clips.len() == built.len();

                    if unchanged {
                        let pending: Vec<HydrateTask> = st
                            .clips
                            .iter()
                            .filter(|c| c.thumbnail.is_none() || (!c.is_image && c.duration_ticks <= 0))
                            .map(|c| HydrateTask {
                                file_path: c.file_path.clone(),
                                thumbnail_path: c.thumbnail_path.clone(),
                                is_image: c.is_image,
                                duration_ticks: c.duration_ticks,
                                has_thumbnail: c.thumbnail.is_some(),
                            })
                            .collect();
                        LibraryState::update_free_space(&ui, &library_dir);
                        ui.global::<LibraryVM>().set_is_loading(false);
                        if pending.is_empty() {
                            None
                        } else {
                            Some((pending, false))
                        }
                    } else {
                        let tasks: Vec<HydrateTask> = built
                            .iter()
                            .map(|c| HydrateTask {
                                file_path: c.file_path.clone(),
                                thumbnail_path: c.thumbnail_path.clone(),
                                is_image: c.is_image,
                                duration_ticks: c.duration_ticks,
                                has_thumbnail: c.thumbnail.is_some(),
                            })
                            .collect();
                        st.clips = built;
                        st.rebuild_path_index();
                        st.selection_anchor = None;
                        st.published_signature = Some(signature);
                        st.rebuild_view(&ui);
                        st.publish_clips();
                        LibraryState::update_free_space(&ui, &library_dir);
                        st.notify_selection_changed(&ui);
                        ui.global::<LibraryVM>().set_is_loading(false);
                        Some((tasks, true))
                    }
                };

                if let Some((tasks, run_housekeeping)) = tasks_and_hk {
                    start_phase2_hydration(
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
                        ui_weak,
                        st_rc,
                    );
                }
            });
        });
    }
}

pub fn init(ui: &crate::AppWindow, ctx: &Rc<AppCtx>) {
    let state = Rc::new(RefCell::new(LibraryState::new(ui, Rc::clone(ctx))));
    register_state(&state);

    // Sync initial hotkey keycaps
    let initial_settings = ctx.settings();
    sync_hotkey_parts(ui, &initial_settings);

    // Startup background quota check + initial free-space read (LibraryViewModel.cs:340-344)
    {
        let lib_dir = resolve_library_dir(&initial_settings);
        let auto_cleanup = initial_settings.auto_cleanup_enabled;
        let max_gb = initial_settings.max_library_size_gb;
        let thumb_dir = state.borrow().thumb_cache_dir.clone();
        let meta_store = state.borrow().meta_store.clone();
        let ui_weak = ui.as_weak();
        let _ = std::thread::Builder::new().name("lag-lib-startup".into()).spawn(move || {
            enforce_storage_quota(&lib_dir, auto_cleanup, max_gb, meta_store.as_ref(), &thumb_dir);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    LibraryState::update_free_space(&ui, &lib_dir);
                }
            });
        });
    }

    let vm = ui.global::<LibraryVM>();

    // Search text changed (immediate, no debounce)
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_search_changed(move |text| {
            let Some(ui) = ui_w.upgrade() else { return };
            let mut st = st.borrow_mut();
            st.search_text = text.to_string();
            st.clear_selection(&ui);
            st.rebuild_groups(&ui);
        });
    }

    // Category chip toggle
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_set_game_filter(move |key| {
            let Some(ui) = ui_w.upgrade() else { return };
            let mut st = st.borrow_mut();
            let key_str = key.as_str();
            let same = st.game_filter.as_deref().is_some_and(|cur| eq_ignore_case(cur, key_str));
            st.game_filter = if same { None } else { Some(key_str.to_string()) };
            st.rebuild_game_chips(&ui);
            st.clear_selection(&ui);
            st.rebuild_groups(&ui);
        });
    }

    // Favorites toggle
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_toggle_favorites_only(move || {
            let Some(ui) = ui_w.upgrade() else { return };
            let mut st = st.borrow_mut();
            st.favorites_only = !st.favorites_only;
            ui.global::<LibraryVM>().set_favorites_only(st.favorites_only);
            st.clear_selection(&ui);
            st.rebuild_groups(&ui);
        });
    }

    // Open library folder
    {
        let ctx = Rc::clone(ctx);
        vm.on_open_library_folder(move || {
            let dir = resolve_library_dir(&ctx.settings());
            open_folder_shell(&dir);
        });
    }

    // Bulk delete & clear/select-all
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_delete_selected(move || {
            let Some(ui) = ui_w.upgrade() else { return };
            st.borrow_mut().delete_selected(&ui);
        });
    }
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_clear_selection(move || {
            let Some(ui) = ui_w.upgrade() else { return };
            st.borrow_mut().clear_selection(&ui);
        });
    }
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_select_all(move || {
            let Some(ui) = ui_w.upgrade() else { return };
            st.borrow_mut().select_all(&ui);
        });
    }

    // Card interactions
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_card_pressed(move |clip_idx, shift, ctrl| {
            let Some(ui) = ui_w.upgrade() else { return };
            let idx = clip_idx as usize;
            let play_path: Option<String> = {
                let mut st = st.borrow_mut();
                if idx >= st.clips.len() {
                    return;
                }
                let has_selection = st.clips.iter().any(|c| c.is_selected);
                if shift {
                    st.select_range(&ui, idx);
                    None
                } else if ctrl || has_selection {
                    st.toggle_select_clip(&ui, idx);
                    None
                } else {
                    Some(st.clips[idx].file_path.clone())
                }
            };
            if let Some(path) = play_path {
                st.borrow().ctx.play_clip(&path);
            }
        });
    }
    {
        let st = Rc::clone(&state);
        vm.on_card_play(move |clip_idx| {
            let path = st.borrow().clips.get(clip_idx as usize).map(|c| c.file_path.clone());
            if let Some(p) = path {
                st.borrow().ctx.play_clip(&p);
            }
        });
    }
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_card_favorite(move |clip_idx| {
            let Some(ui) = ui_w.upgrade() else { return };
            st.borrow_mut().toggle_favorite(&ui, clip_idx as usize);
        });
    }
    {
        let st = Rc::clone(&state);
        vm.on_card_edit(move |clip_idx| {
            let target = st.borrow().clips.get(clip_idx as usize).and_then(|c| {
                if c.is_image {
                    None
                } else {
                    Some(c.file_path.clone())
                }
            });
            if let Some(path) = target {
                st.borrow().ctx.edit_clip(&path);
            }
        });
    }
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_card_select_circle(move |clip_idx| {
            let Some(ui) = ui_w.upgrade() else { return };
            st.borrow_mut().toggle_select_clip(&ui, clip_idx as usize);
        });
    }

    // Context menu actions
    {
        let st = Rc::clone(&state);
        vm.on_context_edit(move |file_path| {
            let target = {
                let st = st.borrow();
                st.clip_path_to_idx
                    .get(file_path.as_str())
                    .and_then(|&idx| st.clips.get(idx))
                    .filter(|c| !c.is_image)
                    .map(|c| c.file_path.clone())
            };
            if let Some(path) = target {
                st.borrow().ctx.edit_clip(&path);
            }
        });
    }
    {
        vm.on_context_show_in_folder(move |file_path| {
            show_in_folder(file_path.as_str());
        });
    }
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_context_delete(move |file_path| {
            let Some(ui) = ui_w.upgrade() else { return };
            st.borrow_mut().delete_single(&ui, file_path.as_str());
        });
    }

    // Virtualization viewport updates
    {
        let st = Rc::clone(&state);
        vm.on_viewport_changed(move |scroll_y, w, h| {
            st.borrow_mut().on_viewport_changed(scroll_y, w, h);
        });
    }

    // Refresh hook
    {
        let st = Rc::clone(&state);
        ctx.on_library_refresh(move || {
            LibraryState::start_refresh(&st);
        });
    }

    // Settings changes (language, hotkeys, library path, quota)
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        let prev_settings = RefCell::new(initial_settings);
        ctx.on_settings_changed(move |new_s| {
            let Some(ui) = ui_w.upgrade() else { return };
            let mut prev = prev_settings.borrow_mut();
            let path_changed = prev.library_path != new_s.library_path;
            let quota_changed = prev.auto_cleanup_enabled != new_s.auto_cleanup_enabled
                || prev.max_library_size_gb != new_s.max_library_size_gb;
            let lang_changed = prev.language != new_s.language;
            let hotkey_changed = prev.hotkey_key != new_s.hotkey_key || prev.hotkey_modifiers != new_s.hotkey_modifiers;
            *prev = new_s.clone();
            drop(prev);

            if hotkey_changed {
                sync_hotkey_parts(&ui, new_s);
            }
            if path_changed || quota_changed {
                LibraryState::start_refresh(&st);
            } else if lang_changed {
                let mut guard = st.borrow_mut();
                guard.rebuild_view(&ui);
                guard.notify_selection_changed(&ui);
            }
        });
    }

    // Initial background scan at startup (MainViewModel.cs:231)
    LibraryState::start_refresh(&state);
}

fn sync_hotkey_parts(ui: &crate::AppWindow, settings: &SettingsData) {
    let parts: Vec<SharedString> = hotkey_parts_from_settings(settings).into_iter().map(SharedString::from).collect();
    ui.global::<LibraryVM>().set_hotkey_parts(ModelRc::from(Rc::new(VecModel::from(parts))));
}

fn show_in_folder(file_path: &str) {
    if !Path::new(file_path).exists() {
        return;
    }
    if let Err(e) = Command::new("explorer.exe").raw_arg(format!("/select,\"{file_path}\"")).spawn() {
        log::debug!("Failed to show clip in folder: {e}");
    }
}

fn open_folder_shell(dir: &Path) {
    let wide_path = to_wide(&dir.to_string_lossy());
    let res = unsafe { ShellExecuteW(None, w!("open"), PCWSTR(wide_path.as_ptr()), None, None, SW_SHOWNORMAL) };
    if (res.0 as isize) <= 32 {
        log::debug!("Failed to open folder: ShellExecuteW returned {}", res.0 as isize);
    }
}

fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

fn utf16_slice_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn videos_folder() -> Option<PathBuf> {
    known_folder(&FOLDERID_Videos)
}

fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok().map(|p: PWSTR| {
            let text = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0.cast()));
            PathBuf::from(text)
        })
    }
}

fn locale_fixed_one(value: f64) -> String {
    let mut buffer = [0u16; 16];
    let count = unsafe { GetLocaleInfoEx(PCWSTR::null(), LOCALE_SDECIMAL, Some(&mut buffer)) };
    let decimal = if count > 1 { utf16_slice_to_string(&buffer) } else { ".".into() };
    format!("{value:.1}").replace('.', &decimal)
}

fn win32_date_format(date: NaiveDate, pattern: &str) -> Option<String> {
    let st = SYSTEMTIME {
        wYear: date.year() as u16,
        wMonth: date.month() as u16,
        wDayOfWeek: date.weekday().num_days_from_sunday() as u16,
        wDay: date.day() as u16,
        wHour: 0,
        wMinute: 0,
        wSecond: 0,
        wMilliseconds: 0,
    };
    // Month/day names follow the APP language, not the Windows locale (C# used CurrentCulture, so a
    // German UI on a Ukrainian Windows showed "25 ВЕРЕСНЯ"). Neutral locale names are valid here.
    let code = lag_core::i18n::current_language().code;
    let locale = to_wide(if code == "no" { "nb-NO" } else { code });
    // Japanese writes month before day with counters: 9月25日, not "25 9月".
    let pattern = match (code, pattern) {
        ("ja", "d MMM" | "d MMMM") => "M'月'd'日'",
        _ => pattern,
    };
    let pat = to_wide(pattern);
    let mut buf = [0u16; 128];
    let len = unsafe {
        GetDateFormatEx(
            PCWSTR(locale.as_ptr()),
            ENUM_DATE_FORMATS_FLAGS(0),
            Some(&st),
            PCWSTR(pat.as_ptr()),
            Some(&mut buf),
            None,
        )
    };
    if len <= 0 {
        return None;
    }
    Some(utf16_slice_to_string(&buf))
}

fn query_drive_usage_display(library_dir: &Path) -> Option<(String, f32)> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use lag_core::metadata::ClipMetadata;
    use std::sync::atomic::AtomicUsize;

    #[test]
    fn squashed_legacy_thumbnails_are_made_again() {
        let dir = std::env::temp_dir().join(format!("lag-thumb-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let write = |name: &str, w: u32, h: u32| {
            let p = dir.join(name);
            image::RgbImage::new(w, h).save(&p).unwrap();
            p
        };
        assert!(is_legacy_thumbnail(&write("old.jpg", 320, 180)));
        assert!(!is_legacy_thumbnail(&write("wide.jpg", 516, 216)));
        assert!(!is_legacy_thumbnail(&write("16x9.jpg", 384, 216)));
        assert!(!is_legacy_thumbnail(&dir.join("missing.jpg")));
        assert!(thumbnail_filter().contains("force_original_aspect_ratio=increase"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn test_temp_dir(name: &str) -> PathBuf {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target/test-data/library").join(format!(
            "{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    struct FixedFormatter;
    impl DateFormatProvider for FixedFormatter {
        fn format_d_mmm(&self, date: NaiveDate) -> String {
            date.format("%-d %b").to_string()
        }
        fn format_d_mmmm(&self, date: NaiveDate) -> String {
            date.format("%-d %B").to_string()
        }
        fn format_dddd(&self, date: NaiveDate) -> String {
            date.format("%A").to_string()
        }
    }

    #[test]
    fn thumbnail_cache_key_matches_csharp_md5_and_mtime() {
        let thumb_root = Path::new(r"C:\Users\Test\AppData\Local\Lag\thumbnails");
        // Verified against .NET 8 MD5.HashData(UTF8(@"C:\Clips\Replay.mp4".ToLowerInvariant()))
        assert_eq!(
            thumbnail_cache_path(thumb_root, r"C:\Clips\Replay.mp4"),
            thumb_root.join("E70506024595A2FBC13F5ECC2858A8C3.jpg")
        );
        assert_eq!(
            thumbnail_cache_path(thumb_root, r"C:\Clips\Replay.mp4"),
            thumbnail_cache_path(thumb_root, r"c:\clips\replay.mp4")
        );

        let dir = test_temp_dir("thumb-key");
        let jpg = dir.join("E70506024595A2FBC13F5ECC2858A8C3.jpg");
        assert!(ThumbnailMemCache::key_for(&jpg).is_none());
        std::fs::write(&jpg, b"fakejpg").unwrap();
        let k1 = ThumbnailMemCache::key_for(&jpg).unwrap();
        assert!(k1.starts_with(&format!("{}|", jpg.to_string_lossy())));
        let ticks_part: i64 = k1.rsplit('|').next().unwrap().parse().unwrap();
        assert!(ticks_part > 630_000_000_000_000_000); // > year 1997 in .NET ticks

        let mut cache = ThumbnailMemCache::default();
        let buf = SharedPixelBuffer::<Rgba8Pixel>::new(2, 2);
        cache.insert(&jpg, buf.clone());
        assert!(cache.get(&jpg).is_some());
        assert_eq!(cache.len(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn scan_discovers_all_media_ignores_other_and_subfolders_and_builds_signature() {
        let dir = test_temp_dir("scan");
        let lib = dir.join("lib");
        let meta_dir = dir.join("meta");
        let thumb_dir = dir.join("thumbs");
        let sub = lib.join("subfolder");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&sub).unwrap();

        // Create files across all 8 patterns plus ignored files and a subfolder clip
        let names = [
            "clip1.mp4",
            "clip2.MKV",
            "clip3.mov",
            "clip4.avi",
            "anim.gif",
            "shot1.png",
            "shot2.jpg",
            "shot3.JPEG",
            "clip1-edit.mp4",
        ];
        for (i, n) in names.iter().enumerate() {
            std::fs::write(lib.join(n), vec![b'x'; 10 + i]).unwrap();
            std::thread::sleep(std::time::Duration::from_millis(18));
        }
        std::fs::write(lib.join("notes.txt"), b"ignore").unwrap();
        std::fs::write(lib.join("clip1.mp4.json"), b"{}").unwrap();
        std::fs::write(sub.join("nested.mp4"), b"ignore").unwrap();

        let store = ClipMetadataStore::new(&meta_dir);
        store.write(
            &lib.join("clip1.mp4").to_string_lossy(),
            &ClipMetadata {
                game: Some("Valorant".into()),
                exe: Some("VALORANT.exe".into()),
                favorite: true,
                edited: false,
                duration_seconds: Some(12.5),
            },
        );
        store.write(
            &lib.join("clip3.mov").to_string_lossy(),
            &ClipMetadata { game: None, exe: None, favorite: false, edited: true, duration_seconds: None },
        );

        let mem = Mutex::new(ThumbnailMemCache::default());
        let (clips, sig) = scan_library_phase1(&lib, Some(&store), &thumb_dir, &mem, &|| false).unwrap();

        assert_eq!(clips.len(), names.len());
        // Newest-first (clip1-edit.mp4 created last -> first in list)
        assert_eq!(clips[0].display_name(), "clip1-edit");
        assert!(clips[0].is_edited); // fallback "-edit" in filename

        // Check image vs gif classification
        let gif = clips.iter().find(|c| c.display_name() == "anim").unwrap();
        assert!(!gif.is_image);
        assert_ne!(gif.thumbnail_path, gif.file_path);

        let png = clips.iter().find(|c| c.display_name() == "shot1").unwrap();
        assert!(png.is_image);
        assert_eq!(png.thumbnail_path, png.file_path);

        let jpeg = clips.iter().find(|c| c.display_name() == "shot3").unwrap();
        assert!(jpeg.is_image);

        // Check sidecar metadata fields
        let c1 = clips.iter().find(|c| c.display_name() == "clip1").unwrap();
        assert!(c1.is_favorite);
        assert_eq!(c1.game.as_deref(), Some("Valorant"));
        assert_eq!(c1.duration_ticks, 125_000_000);
        assert_eq!(c1.duration_display(), "0:12");

        let c3 = clips.iter().find(|c| c.display_name() == "clip3").unwrap();
        assert!(c3.is_edited);
        assert!(!c3.has_game());

        // Signature has 1 line per clip: path|len|ticks\n
        let lines: Vec<&str> = sig.lines().collect();
        assert_eq!(lines.len(), names.len());
        assert!(lines[0].contains("|18|")); // 10 + 8

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn filter_chips_and_day_grouping_match_csharp() {
        lag_core::i18n::set_language("en");
        let today = NaiveDate::from_ymd_opt(2026, 11, 16).unwrap();
        let yesterday = NaiveDate::from_ymd_opt(2026, 11, 15).unwrap();
        let older = NaiveDate::from_ymd_opt(2026, 11, 10).unwrap();

        let clips = vec![
            ClipEntry {
                file_path: r"C:\Lib\Alpha.mp4".into(),
                game: Some("Valorant".into()),
                is_favorite: true,
                created_date: today.and_hms_opt(14, 5, 0).unwrap(),
                ..ClipEntry::default()
            },
            ClipEntry {
                file_path: r"C:\Lib\Beta.mp4".into(),
                game: Some("valorant".into()), // distinct OrdinalIgnoreCase preserves first casing
                is_favorite: false,
                created_date: today.and_hms_opt(10, 0, 0).unwrap(),
                ..ClipEntry::default()
            },
            ClipEntry {
                file_path: r"C:\Lib\Gamma.png".into(),
                game: None,
                is_image: true,
                is_favorite: true,
                created_date: yesterday.and_hms_opt(22, 30, 0).unwrap(),
                ..ClipEntry::default()
            },
            ClipEntry {
                file_path: r"C:\Lib\Delta.mp4".into(),
                game: Some("Apex Legends".into()),
                is_favorite: false,
                created_date: older.and_hms_opt(18, 0, 0).unwrap(),
                ..ClipEntry::default()
            },
        ];

        let mut filter = Some("NONEXISTENT".to_string());
        let chips = build_game_chips(&clips, &mut filter, "Desktop");
        assert!(filter.is_none()); // stale filter dropped
        assert_eq!(chips.len(), 3);
        assert_eq!(chips[0].label, "Apex Legends");
        assert_eq!(chips[1].label, "Valorant");
        assert_eq!(chips[2].label, "Desktop");
        assert_eq!(chips[2].filter_key, DESKTOP_FILTER_KEY);

        // Filter: favorites + Desktop
        let filtered = filter_clips(&clips, true, Some(DESKTOP_FILTER_KEY), "");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].0, 2);

        // Filter: search matches DisplayName or Game case-insensitively
        let filtered_q = filter_clips(&clips, false, None, "  VALO ");
        assert_eq!(filtered_q.len(), 2);

        // Grouping: Today, Yesterday, older date descending
        let all: Vec<(usize, &ClipEntry)> = clips.iter().enumerate().collect();
        let groups = group_clips_by_day(&all, today, &FixedFormatter);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[0].label, "TODAY");
        assert_eq!(groups[0].count_label, "2 clips");
        assert_eq!(groups[0].clip_indices, vec![0, 1]);
        assert_eq!(groups[1].label, "YESTERDAY");
        assert_eq!(groups[1].count_label, "1 clips");
        assert_eq!(groups[2].label, "10 NOVEMBER");
        assert_eq!(groups[2].sub, "Tuesday");
    }

    #[test]
    fn quota_cleanup_deletes_oldest_videos_only_and_virtual_layout_rows() {
        let dir = test_temp_dir("quota");
        let lib = dir.join("lib");
        let meta_dir = dir.join("meta");
        let thumb_dir = dir.join("thumbs");
        std::fs::create_dir_all(&lib).unwrap();
        std::fs::create_dir_all(&thumb_dir).unwrap();

        let store = ClipMetadataStore::new(&meta_dir);
        let old_vid = lib.join("old.mp4");
        let shot = lib.join("shot.png");
        let new_vid = lib.join("new.mp4");

        // Total videos: 2 * 600MB? We can't write 1.2GB in a fast unit test, so test quota non-positive
        // and test delete_clip + virtual row layout on synthetic data.
        std::fs::write(&old_vid, vec![1u8; 128]).unwrap();
        std::fs::write(&shot, vec![2u8; 256]).unwrap();
        std::fs::write(&new_vid, vec![3u8; 128]).unwrap();
        // Files written in the same instant share a creation time; make "old" unambiguously older
        // (cleanup orders by CreationTime, like C# LibraryViewModel.cs:300).
        {
            use std::os::windows::fs::FileTimesExt;
            let hour_ago = std::time::SystemTime::now() - std::time::Duration::from_secs(3600);
            let f = std::fs::OpenOptions::new().write(true).open(&old_vid).unwrap();
            f.set_times(std::fs::FileTimes::new().set_created(hour_ago)).unwrap();
        }

        let old_thumb = thumbnail_cache_path(&thumb_dir, &old_vid.to_string_lossy());
        std::fs::write(&old_thumb, b"thumb").unwrap();
        store.set_favorite(&old_vid.to_string_lossy(), true);

        // Non-positive quota does nothing
        enforce_storage_quota(&lib, true, 0, Some(&store), &thumb_dir);
        assert!(old_vid.exists());

        // Disabled quota does nothing
        enforce_storage_quota(&lib, false, 1, Some(&store), &thumb_dir);
        assert!(old_vid.exists());

        // Enforce 200-byte limit: total video bytes = 256 (128 + 128), screenshot is 256 (excluded).
        // Should delete oldest video (old.mp4, even though favorited!) along with its metadata & thumb,
        // leaving new.mp4 (128 <= 200) and shot.png intact.
        enforce_storage_quota_bytes(&lib, 200, Some(&store), &thumb_dir);
        assert!(!old_vid.exists());
        assert!(!old_thumb.exists());
        assert!(store.read(&old_vid.to_string_lossy()).is_none());
        assert!(new_vid.exists());
        assert!(shot.exists());

        // Virtual row layout test: 2 groups (5 clips, 2 clips) at 3 columns -> 2 + 1 card rows + 2 headers = 5 rows
        let groups = vec![
            DayGroup {
                label: "TODAY".into(),
                sub: "16 Nov".into(),
                count_label: "5 clips".into(),
                clip_indices: vec![0, 1, 2, 3, 4],
            },
            DayGroup {
                label: "YESTERDAY".into(),
                sub: "15 Nov".into(),
                count_label: "2 clips".into(),
                clip_indices: vec![5, 6],
            },
        ];
        assert_eq!(columns_for_viewport(900.0), 3); // (900 - 76) / 268 = 3.07 -> 3
        let (rows, total_h) = layout_virtual_rows(&groups, 3);
        assert_eq!(rows.len(), 5);
        assert!(total_h > 700.0);
        let (first, last) = visible_row_range(&rows, 0.0, 300.0);
        assert_eq!(first, 0);
        assert!(last <= rows.len());

        // Hotkey parts
        let s = SettingsData::default();
        assert_eq!(hotkey_parts_from_settings(&s), vec!["Alt", "F10"]);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}

impl Default for ClipEntry {
    fn default() -> Self {
        Self {
            is_selected: false,
            is_favorite: false,
            game: None,
            exe: None,
            is_edited: false,
            is_image: false,
            file_path: String::new(),
            thumbnail_path: String::new(),
            thumbnail: None,
            duration_ticks: 0,
            created_date: NaiveDate::from_ymd_opt(1, 1, 1).unwrap().and_hms_opt(0, 0, 0).unwrap(),
            file_size: 0,
        }
    }
}
