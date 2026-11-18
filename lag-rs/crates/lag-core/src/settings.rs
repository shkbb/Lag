//! `%AppData%\Lag\settings.json` — byte-compatible with the C# `SettingsData`
//! (`InstantReplay/ViewModels/SettingsViewModel.cs:2071-2207`, spec: `docs/spec/settings.md` §5.1).
//!
//! Compatibility rules mirrored from System.Text.Json defaults as used by the C# app:
//! * exact PascalCase keys, case-SENSITIVE on read, declaration order on write;
//! * missing key → C# initializer default; explicit `null` on a string → `None` (C# null);
//! * unknown keys are ignored on read and dropped on write;
//! * output: 2-space indent, CRLF line breaks (the file is written on Windows by `File.WriteAllText`),
//!   no BOM, no trailing newline, non-ASCII and HTML-sensitive characters escaped as `\uXXXX` (uppercase hex),
//!   integral doubles written without a fraction (`1`, not `1.0`).
//!
//! Load-time clamping/migration (BufferMinutes → BufferSeconds, MonitorDeviceName fallback, …) is NOT done
//! here: it belongs to the settings view-model, exactly like in C#. This type is the raw on-disk record.

use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

fn s(v: &str) -> Option<String> {
    Some(v.to_owned())
}

macro_rules! str_default {
    ($($fn_name:ident = $val:expr;)*) => { $(fn $fn_name() -> Option<String> { s($val) })* };
}
str_default! {
    d_empty = "";
    d_vcf10 = "VcF10";
    d_vcf9 = "VcF9";
    d_vcf8 = "VcF8";
    d_vcf7 = "VcF7";
    d_leftalt = "LeftAlt";
    d_ffmpeg = "ffmpeg";
    d_en = "en";
    d_dark = "dark";
    d_all = "all";
    d_vcv = "VcV";
    d_mp4 = "mp4";
}
fn d_apps() -> Option<Vec<AppAudioSetting>> {
    Some(Vec::new())
}

/// One per-application audio rule (`AppAudioSetting`, SettingsViewModel.cs:2209-2214).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AppAudioSetting {
    #[serde(default = "d_empty")]
    pub exe: Option<String>,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "d_100")]
    pub volume: i32,
}

impl Default for AppAudioSetting {
    fn default() -> Self {
        Self { exe: s(""), enabled: false, volume: 100 }
    }
}

fn d_100() -> i32 {
    100
}
fn d_5() -> i32 {
    5
}
fn d_30() -> i32 {
    30
}
fn d_50() -> i32 {
    50
}
fn d_8() -> i32 {
    8
}
fn d_1() -> i32 {
    1
}
fn d_m1() -> i32 {
    -1
}
fn d_true() -> bool {
    true
}
fn d_097() -> f64 {
    0.97
}
fn d_095() -> f64 {
    0.95
}
fn d_024() -> f64 {
    0.24
}
fn d_003() -> f64 {
    0.03
}
fn d_020() -> f64 {
    0.20
}
fn d_05() -> f64 {
    0.5
}
fn d_002() -> f64 {
    0.02
}
fn d_0045() -> f64 {
    0.045
}

/// Raw on-disk settings record. Field order == C# declaration order == JSON key order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SettingsData {
    #[serde(default)]
    pub buffer_seconds: i32,
    #[serde(default = "d_5")]
    pub buffer_minutes: i32,
    #[serde(default)]
    pub monitor_index: i32,
    #[serde(default = "d_vcf10")]
    pub hotkey_key: Option<String>,
    #[serde(default = "d_leftalt")]
    pub hotkey_modifiers: Option<String>,
    #[serde(default = "d_vcf9")]
    pub screenshot_key: Option<String>,
    #[serde(default = "d_leftalt")]
    pub screenshot_modifiers: Option<String>,
    #[serde(default = "d_vcf8")]
    pub pause_key: Option<String>,
    #[serde(default = "d_leftalt")]
    pub pause_modifiers: Option<String>,
    #[serde(default = "d_vcf7")]
    pub record_key: Option<String>,
    #[serde(default = "d_leftalt")]
    pub record_modifiers: Option<String>,
    #[serde(rename = "FFmpegPath", default = "d_ffmpeg")]
    pub ffmpeg_path: Option<String>,
    #[serde(default = "d_empty")]
    pub library_path: Option<String>,
    #[serde(default = "d_30")]
    pub frame_rate: i32,
    #[serde(default = "d_empty")]
    pub monitor_device_name: Option<String>,
    #[serde(default = "d_empty")]
    pub monitor_stable_id: Option<String>,
    #[serde(default)]
    pub overlay_enabled: bool,
    #[serde(default = "d_empty")]
    pub webcam_device_id: Option<String>,
    #[serde(default = "d_true")]
    pub webcam_shown: bool,
    #[serde(default = "d_097", rename = "WebcamX")]
    pub webcam_x: f64,
    #[serde(default = "d_095", rename = "WebcamY")]
    pub webcam_y: f64,
    #[serde(default = "d_024")]
    pub webcam_scale: f64,
    #[serde(default)]
    pub keys_shown: bool,
    #[serde(default = "d_003", rename = "KeysX")]
    pub keys_x: f64,
    #[serde(default = "d_095", rename = "KeysY")]
    pub keys_y: f64,
    #[serde(default = "d_020")]
    pub keys_scale: f64,
    #[serde(default)]
    pub stats_shown: bool,
    #[serde(default = "d_05", rename = "StatsX")]
    pub stats_x: f64,
    #[serde(default = "d_002", rename = "StatsY")]
    pub stats_y: f64,
    #[serde(default = "d_0045")]
    pub stats_scale: f64,
    #[serde(default = "d_1")]
    pub stats_detail: i32,
    #[serde(default = "d_empty")]
    pub microphone_id: Option<String>,
    #[serde(default)]
    pub start_with_windows: bool,
    #[serde(default)]
    pub auto_start_recording: bool,
    #[serde(default)]
    pub start_minimized: bool,
    #[serde(default = "d_true")]
    pub disable_game_mode: bool,
    #[serde(default)]
    pub game_capture_enabled: bool,
    #[serde(default = "d_en")]
    pub language: Option<String>,
    #[serde(default = "d_dark")]
    pub theme: Option<String>,
    #[serde(default = "d_100")]
    pub mic_volume: i32,
    #[serde(default)]
    pub output_resolution_height: i32,
    #[serde(default = "d_empty")]
    pub codec_name: Option<String>,
    #[serde(default)]
    pub auto_cleanup_enabled: bool,
    #[serde(default = "d_50")]
    pub max_library_size_gb: i32,
    #[serde(default = "d_m1")]
    pub bitrate_kbps: i32,
    #[serde(default = "d_m1")]
    pub gpu_index: i32,
    #[serde(default = "d_all")]
    pub audio_capture_mode: Option<String>,
    #[serde(default)]
    pub audio_mode_by_target: bool,
    #[serde(default = "d_empty")]
    pub system_audio_device_id: Option<String>,
    #[serde(default = "d_apps")]
    pub audio_apps: Option<Vec<AppAudioSetting>>,
    #[serde(default)]
    pub ptt_enabled: bool,
    #[serde(default = "d_vcv")]
    pub ptt_key: Option<String>,
    #[serde(default)]
    pub mic_mono: bool,
    #[serde(default = "d_true")]
    pub system_audio_enabled: bool,
    #[serde(default = "d_100")]
    pub system_audio_volume: i32,
    #[serde(default = "d_true")]
    pub mic_enabled: bool,
    #[serde(default = "d_true")]
    pub game_audio_enabled: bool,
    #[serde(default = "d_100")]
    pub game_audio_volume: i32,
    #[serde(default = "d_true")]
    pub input_sensitivity_auto: bool,
    #[serde(default = "d_8")]
    pub input_sensitivity_threshold: i32,
    #[serde(default)]
    pub noise_suppression: bool,
    #[serde(default)]
    pub separate_audio_tracks: bool,
    #[serde(default = "d_mp4")]
    pub file_format: Option<String>,
    /// NEW in the Rust version (not in C#'s SettingsData): what to record while the game is out of focus.
    /// "game" = keep recording the game window (C# behaviour, default); "desktop" = record the desktop.
    /// Old C# builds ignore the key, so the file stays compatible both ways.
    #[serde(default = "d_game_focus")]
    pub game_focus_mode: Option<String>,
    /// NEW in Rust: player slow motion (< 1x) blends in-between frames (mpv interpolation). Default on.
    #[serde(default = "d_true")]
    pub slowmo_interpolation: bool,
    /// NEW in Rust: webcam overlay mode, index into `presets::WEBCAM_MODES` (0 = Auto).
    #[serde(default)]
    pub webcam_mode: i32,
    /// NEW in Rust: the main window's normal size (logical px, 0 = default) and maximized state, remembered
    /// across hiding to the tray and restarts (owner, 2026-11-18).
    #[serde(default)]
    pub window_width: i32,
    #[serde(default)]
    pub window_height: i32,
    #[serde(default)]
    pub window_maximized: bool,
}

fn d_game_focus() -> Option<String> {
    Some("game".to_owned())
}

/// Keys the Rust version adds after the C# ones (written last, in this order).
pub const RUST_ONLY_KEYS: &[&str] =
    &["GameFocusMode", "SlowmoInterpolation", "WebcamMode", "WindowWidth", "WindowHeight", "WindowMaximized"];

impl Default for SettingsData {
    fn default() -> Self {
        // An empty JSON object yields exactly the C# initializer defaults.
        serde_json::from_str("{}").expect("defaults")
    }
}

/// `%AppData%\Lag\settings.json` (SettingsViewModel.cs:21-23).
pub fn default_path() -> Option<PathBuf> {
    dirs_roaming_appdata().map(|d| d.join("Lag").join("settings.json"))
}

fn dirs_roaming_appdata() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from)
}

impl SettingsData {
    /// Parses settings text the way System.Text.Json does (tolerates a UTF-8 BOM).
    pub fn from_json(text: &str) -> serde_json::Result<Self> {
        serde_json::from_str(text.trim_start_matches('\u{FEFF}'))
    }

    /// Serialises exactly like `JsonSerializer.Serialize(settings, new { WriteIndented = true })`
    /// written by `File.WriteAllText` on Windows.
    pub fn to_json(&self) -> String {
        crate::netjson::to_string(self).expect("settings serialise")
    }

    /// Reads the file; a missing file is `Ok(None)` (C# keeps view-model defaults then).
    pub fn load(path: &Path) -> io::Result<Option<Self>> {
        match std::fs::read_to_string(path) {
            Ok(t) => Self::from_json(&t).map(Some).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Writes the file. Unlike C#'s plain `File.WriteAllText`, goes through a temp file + rename so a crash
    /// mid-write cannot leave a truncated settings.json (output bytes are identical).
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_csharp_initializers() {
        let d = SettingsData::default();
        assert_eq!(d.buffer_seconds, 0);
        assert_eq!(d.buffer_minutes, 5);
        assert_eq!(d.hotkey_key.as_deref(), Some("VcF10"));
        assert_eq!(d.frame_rate, 30);
        assert!(d.webcam_shown);
        assert_eq!(d.webcam_x, 0.97);
        assert_eq!(d.stats_scale, 0.045);
        assert_eq!(d.bitrate_kbps, -1);
        assert_eq!(d.gpu_index, -1);
        assert_eq!(d.ptt_key.as_deref(), Some("VcV"));
        assert_eq!(d.file_format.as_deref(), Some("mp4"));
        assert_eq!(d.game_focus_mode.as_deref(), Some("game"));
        assert_eq!(d.library_path.as_deref(), Some(""));
        assert_eq!(d.audio_apps.as_deref(), Some(&[][..]));
    }

    #[test]
    fn null_string_is_none_like_csharp() {
        let d = SettingsData::from_json(r#"{"LibraryPath": null}"#).unwrap();
        assert_eq!(d.library_path, None);
        assert!(d.to_json().contains("\"LibraryPath\": null"));
    }

    #[test]
    fn keys_are_case_sensitive() {
        let d = SettingsData::from_json(r#"{"framerate": 144}"#).unwrap();
        assert_eq!(d.frame_rate, 30);
    }

    #[test]
    fn escaping_and_numbers_match_system_text_json() {
        let d = SettingsData {
            library_path: s("E:\\Мої кліпи & <x>\\"),
            keys_x: 0.0,
            keys_y: 1.0,
            webcam_scale: 0.19130434782608696,
            ..Default::default()
        };
        let j = d.to_json();
        assert!(
            j.contains(
                r#""LibraryPath": "E:\\\u041C\u043E\u0457 \u043A\u043B\u0456\u043F\u0438 \u0026 \u003Cx\u003E\\""#
            ),
            "{j}"
        );
        assert!(j.contains("\"KeysX\": 0,\r\n"));
        assert!(j.contains("\"KeysY\": 1,\r\n"));
        assert!(j.contains("\"WebcamScale\": 0.19130434782608696,"));
        assert!(j.starts_with("{\r\n  \"BufferSeconds\": 0,\r\n"));
        assert!(j.ends_with("\"FileFormat\": \"mp4\",\r\n  \"GameFocusMode\": \"game\",\r\n  \"SlowmoInterpolation\": true,\r\n  \"WebcamMode\": 0,\r\n  \"WindowWidth\": 0,\r\n  \"WindowHeight\": 0,\r\n  \"WindowMaximized\": false\r\n}"));
        assert!(j.contains("\"AudioApps\": [],"));
    }

    /// The real acceptance test: the user's own settings.json must round-trip byte-for-byte.
    #[test]
    fn real_settings_file_round_trips_byte_for_byte() {
        let Some(path) = default_path() else { return };
        let Ok(original) = std::fs::read_to_string(&path) else {
            eprintln!("no {path:?}, skipping");
            return;
        };
        let parsed = SettingsData::from_json(&original).expect("parse real settings.json");
        let mut written = parsed.to_json();
        // A file last written by C# lacks the Rust-only keys (appended at the end): everything else must match.
        // last key first: each removal also takes the comma in front of its own line
        for key in RUST_ONLY_KEYS.iter().rev() {
            if !original.contains(&format!("\"{key}\"")) {
                let needle = format!(",\r\n  \"{key}\"");
                let start = written.find(&needle).expect("rust-only key written");
                let after = start + needle.len();
                let end = written[after..].find("\r\n").map_or(written.len(), |e| after + e);
                written.replace_range(start..end, "");
            }
        }
        if written != original {
            let (a, b): (Vec<_>, Vec<_>) = (original.lines().collect(), written.lines().collect());
            for (i, (x, y)) in a.iter().zip(b.iter()).enumerate() {
                if x != y {
                    panic!("first difference at line {}:\n  C#:   {x}\n  Rust: {y}", i + 1);
                }
            }
            panic!("length differs: C# {} lines, Rust {} lines", a.len(), b.len());
        }
    }
}
