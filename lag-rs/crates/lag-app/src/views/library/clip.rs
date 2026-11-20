//! One library entry (`ClipEntry`), media file types, thumbnail / library paths.

use super::os::*;
use chrono::{NaiveDate, NaiveDateTime};
use lag_core::settings::SettingsData;
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::path::{Path, PathBuf};

/// Sentinel filter key for the "Desktop" chip (LibraryViewModel.cs:72).
pub const DESKTOP_FILTER_KEY: &str = "\u{0001}desktop";

/// Video container extensions in exact C# pattern order (LibraryViewModel.cs:184).
pub(super) const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "mov", "avi", "gif"];
/// Screenshot extensions in exact C# pattern order (LibraryViewModel.cs:187).
pub(super) const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg"];

pub(super) const THUMB_CACHE_LIMIT: usize = 600;
pub(super) const DOTNET_WINDOWS_FILETIME_TICKS_OFFSET: i64 = 504_911_232_000_000_000;

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
    /// The clip's cloud id (metadata `CloudId`), once it met the cloud.
    pub cloud_id: Option<lag_cloud::ClipId>,
    /// Where it lives, for the card's cloud badge.
    pub cloud: super::cloud::CloudMark,
}

impl ClipEntry {
    /// A card for a clip that is only in the cloud (its path is a `cloud:` key, not a file).
    pub fn is_cloud_only(&self) -> bool {
        crate::services::cloud::stream::is_cloud_key(&self.file_path)
    }

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

pub(super) fn dotnet_whitespace(c: char) -> bool {
    c.is_whitespace() && !matches!(c, '\u{1c}'..='\u{1f}')
}

pub(super) fn eq_ignore_case(a: &str, b: &str) -> bool {
    if a.is_ascii() && b.is_ascii() {
        a.eq_ignore_ascii_case(b)
    } else {
        a.to_lowercase() == b.to_lowercase()
    }
}

pub(super) fn contains_ignore_case(haystack: &str, needle: &str) -> bool {
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

pub(super) fn pattern_rank(path: &Path) -> Option<usize> {
    let ext = path.extension()?.to_str()?;
    VIDEO_EXTENSIONS.iter().chain(IMAGE_EXTENSIONS.iter()).position(|p| ext.eq_ignore_ascii_case(p))
}

pub(super) fn is_video_file(path: &Path) -> bool {
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
            cloud_id: None,
            cloud: Default::default(),
        }
    }
}
