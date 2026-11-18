//! Windows helpers: Explorer, known folders, the user's locale (decimal separator, date formats).

use chrono::{Datelike, NaiveDate};
use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use windows::core::{w, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::SYSTEMTIME;
use windows::Win32::Globalization::{GetDateFormatEx, GetLocaleInfoEx, ENUM_DATE_FORMATS_FLAGS, LOCALE_SDECIMAL};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Videos, SHGetKnownFolderPath, ShellExecuteW, KF_FLAG_DEFAULT};
use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

pub(super) fn show_in_folder(file_path: &str) {
    if !Path::new(file_path).exists() {
        return;
    }
    if let Err(e) = Command::new("explorer.exe").raw_arg(format!("/select,\"{file_path}\"")).spawn() {
        log::debug!("Failed to show clip in folder: {e}");
    }
}

pub(super) fn open_folder_shell(dir: &Path) {
    let wide_path = to_wide(&dir.to_string_lossy());
    let res = unsafe { ShellExecuteW(None, w!("open"), PCWSTR(wide_path.as_ptr()), None, None, SW_SHOWNORMAL) };
    if (res.0 as isize) <= 32 {
        log::debug!("Failed to open folder: ShellExecuteW returned {}", res.0 as isize);
    }
}

pub(super) fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(Some(0)).collect()
}

pub(super) fn utf16_slice_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub(super) fn videos_folder() -> Option<PathBuf> {
    known_folder(&FOLDERID_Videos)
}

pub(super) fn known_folder(id: &GUID) -> Option<PathBuf> {
    unsafe {
        SHGetKnownFolderPath(id, KF_FLAG_DEFAULT, None).ok().map(|p: PWSTR| {
            let text = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0.cast()));
            PathBuf::from(text)
        })
    }
}

pub(super) fn locale_fixed_one(value: f64) -> String {
    let mut buffer = [0u16; 16];
    let count = unsafe { GetLocaleInfoEx(PCWSTR::null(), LOCALE_SDECIMAL, Some(&mut buffer)) };
    let decimal = if count > 1 { utf16_slice_to_string(&buffer) } else { ".".into() };
    format!("{value:.1}").replace('.', &decimal)
}

pub(super) fn win32_date_format(date: NaiveDate, pattern: &str) -> Option<String> {
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
