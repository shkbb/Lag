//! Mirrors Models/ReplayClip.cs; UI state is plain data and thumbnail ownership is generic.

use chrono::{NaiveDate, NaiveDateTime};

#[derive(Debug, Clone)]
pub struct ReplayClip<T = Vec<u8>> {
    pub is_selected: bool,
    pub is_favorite: bool,
    pub game: Option<String>,
    pub exe: Option<String>,
    pub is_edited: bool,
    pub is_image: bool,
    pub file_path: String,
    pub thumbnail_path: String,
    pub thumbnail: Option<T>,
    /// Signed .NET TimeSpan ticks (100 ns), preserving the complete data domain.
    pub duration_ticks: i64,
    pub created_date: NaiveDateTime,
    pub file_size: i64,
}

impl<T> Default for ReplayClip<T> {
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
            created_date: NaiveDate::from_ymd_opt(1, 1, 1)
                .expect("DateTime.MinValue")
                .and_hms_opt(0, 0, 0)
                .expect("midnight"),
            file_size: 0,
        }
    }
}
impl<T> ReplayClip<T> {
    pub fn has_game(&self) -> bool {
        self.game.as_ref().is_some_and(|s| !s.trim_matches(dotnet_whitespace).is_empty())
    }
    pub fn display_name(&self) -> &str {
        let name = self.file_path.rsplit(['\\', '/']).next().unwrap_or("");
        name.rsplit_once('.').map_or(name, |(stem, _)| stem)
    }
    pub fn file_size_display(&self) -> String {
        for (threshold, unit) in [(1_073_741_824, "GB"), (1_048_576, "MB"), (1024, "KB")] {
            if self.file_size >= threshold {
                return format!("{} {unit}", crate::hardware::fixed_one(self.file_size as f64 / threshold as f64));
            }
        }
        format!("{} B", self.file_size)
    }
    pub fn duration_display(&self) -> String {
        let seconds = self.duration_ticks.unsigned_abs() / 10_000_000;
        if self.duration_ticks >= 36_000_000_000 {
            format!("{}:{:02}:{:02}", seconds / 3600 % 24, seconds / 60 % 60, seconds % 60)
        } else {
            format!("{}:{:02}", seconds / 60 % 60, seconds % 60)
        }
    }
    pub fn short_meta(&self) -> String {
        format!("{} · {}", self.duration_display(), self.created_date.format("%d.%m"))
    }
}

fn dotnet_whitespace(c: char) -> bool {
    c.is_whitespace() && !matches!(c, '\u{1c}'..='\u{1f}')
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn derived_labels() {
        let mut c = ReplayClip::<()> {
            file_path: r"C:\Clips\foo.edit.mp4".into(),
            file_size: 1_048_576,
            duration_ticks: 25 * 3600 * 10_000_000,
            ..Default::default()
        };
        assert_eq!(c.display_name(), "foo.edit");
        assert_eq!(c.file_size_display(), format!("{} MB", crate::hardware::fixed_one(1.0)));
        assert_eq!(c.duration_display(), "1:00:00"); // h component, not total hours
        c.duration_ticks = -65 * 10_000_000;
        assert_eq!(c.duration_display(), "1:05");
        c.game = Some(" \u{a0}\t".into());
        assert!(!c.has_game());
        c.game = Some("Game".into());
        assert!(c.has_game());
    }
}
