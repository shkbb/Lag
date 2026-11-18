//! Clips grouped by day with localized headers (Today / Yesterday / weekday / date).

use super::{clip::*, os::*};
use chrono::NaiveDate;
use std::collections::HashMap;

pub trait DateFormatProvider {
    fn format_d_mmm(&self, date: NaiveDate) -> String;
    fn format_d_mmmm(&self, date: NaiveDate) -> String;
    fn format_dddd(&self, date: NaiveDate) -> String;
}

pub(super) struct Win32DateFormatter;

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
