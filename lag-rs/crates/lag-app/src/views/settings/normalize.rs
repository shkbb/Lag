//! Settings loaded from disk, repaired against this PC: frame rates the monitor can show, devices that
//! still exist, values in range, a default library folder.

use super::{devices::*, rows::*};
use lag_core::{
    hardware::MonitorInfo,
    i18n,
    presets::{self},
    settings::SettingsData,
};
use std::path::{Path, PathBuf};

pub(super) fn refresh_cap(hz: i32) -> i32 {
    if hz < 24 {
        return 360;
    }
    [60, 75, 90, 100, 120, 144, 160, 165, 180, 200, 240, 280, 300, 360, 390, 480, 500]
        .into_iter()
        .find(|r| (hz - r).abs() <= 2)
        .unwrap_or(hz)
        .max(60)
}
pub(super) fn fps_options(hz: i32) -> Vec<i32> {
    let cap = refresh_cap(hz);
    let mut values: Vec<_> = [24, 30, 60, 120, 240, 360].into_iter().filter(|v| *v <= cap).collect();
    if values.last().copied().unwrap_or(0) < cap {
        values.push(cap);
    }
    values.push(0);
    values
}
pub(super) fn resolutions(height: i32) -> Vec<i32> {
    std::iter::once(0).chain([2160, 1440, 1080, 720].into_iter().filter(|v| *v <= height)).collect()
}
pub(super) fn device_index<'a>(ids: impl IntoIterator<Item = &'a str>, saved: &str) -> usize {
    ids.into_iter().position(|id| id == saved).unwrap_or(0)
}
pub(super) fn monitor_index(monitors: &[MonitorInfo], s: &SettingsData) -> usize {
    let saved = text(&s.monitor_stable_id);
    monitors
        .iter()
        .position(|m| {
            if !saved.is_empty() {
                m.stable_id == saved
            } else {
                !text(&s.monitor_device_name).is_empty() && m.device_name == text(&s.monitor_device_name)
            }
        })
        .unwrap_or_else(|| monitors.iter().position(|m| m.is_primary).unwrap_or(0))
}
pub(super) fn finite(value: f64, min: f64, max: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}
pub(super) fn default_library() -> String {
    use windows::Win32::UI::Shell::{FOLDERID_Videos, SHGetKnownFolderPath, KF_FLAG_DEFAULT};
    unsafe {
        if let Ok(p) = SHGetKnownFolderPath(&FOLDERID_Videos, KF_FLAG_DEFAULT, None) {
            let path = p.to_string().unwrap_or_default();
            windows::Win32::System::Com::CoTaskMemFree(Some(p.0.cast()));
            return Path::new(&path).join("Lag").to_string_lossy().into();
        }
    }
    PathBuf::from(std::env::var_os("USERPROFILE").unwrap_or_default()).join("Videos/Lag").to_string_lossy().into()
}
/// C# load validation, including legacy minutes and stable-monitor migration. Does not save.
pub(super) fn normalize(s: &mut SettingsData, h: &Hardware) {
    let target = if s.buffer_seconds > 0 { s.buffer_seconds } else { s.buffer_minutes.saturating_mul(60) };
    s.buffer_seconds = if presets::BUFFER_RUNGS.contains(&target) { target } else { 300 };
    if let Some(m) = h.monitors.get(monitor_index(&h.monitors, s)) {
        s.monitor_stable_id = Some(m.stable_id.clone());
        s.monitor_device_name = Some(m.device_name.clone());
        s.frame_rate = if s.frame_rate > 0 { s.frame_rate.clamp(1, refresh_cap(m.refresh_rate as i32)) } else { 30 };
        if !resolutions(m.height as i32).contains(&s.output_resolution_height) {
            s.output_resolution_height = 0;
        }
    }
    if text(&s.library_path).trim().is_empty() {
        s.library_path = Some(default_library());
    }
    if !["mp4", "mov", "mkv"].contains(&text(&s.file_format)) {
        s.file_format = Some("mp4".into());
    }
    if !h.codecs.iter().any(|(id, _)| id == text(&s.codec_name)) {
        s.codec_name = Some(String::new());
    }
    if !h.gpus.iter().any(|g| g.index == s.gpu_index) {
        s.gpu_index = -1;
    }
    if !i18n::LANGUAGES.iter().any(|l| l.code == text(&s.language)) {
        s.language = Some("en".into());
    }
    if !["dark", "light", "system", "neon"].contains(&text(&s.theme)) {
        s.theme = Some("dark".into());
    }
    if !presets::STORAGE_RUNGS.contains(&s.max_library_size_gb) {
        s.max_library_size_gb = 50;
    }
    s.audio_capture_mode = Some(if text(&s.audio_capture_mode) == "apps" { "apps" } else { "all" }.into());
    s.game_focus_mode = Some(if text(&s.game_focus_mode) == "desktop" { "desktop" } else { "game" }.into());
    s.mic_volume = s.mic_volume.clamp(0, 200);
    s.system_audio_volume = s.system_audio_volume.clamp(0, 100);
    s.game_audio_volume = s.game_audio_volume.clamp(0, 100);
    s.input_sensitivity_threshold = s.input_sensitivity_threshold.clamp(0, 100);
    s.stats_detail = s.stats_detail.clamp(0, 2);
    s.bitrate_kbps = if s.bitrate_kbps <= 0 {
        -1
    } else if presets::BITRATE_RUNGS.contains(&s.bitrate_kbps) {
        s.bitrate_kbps
    } else {
        (s.bitrate_kbps / 1000).clamp(3, 100) * 1000
    };
    for (x, y, scale, min, max, default) in [
        (&mut s.webcam_x, &mut s.webcam_y, &mut s.webcam_scale, 0.06, 0.45, 0.24),
        (&mut s.keys_x, &mut s.keys_y, &mut s.keys_scale, 0.06, 0.42, 0.20),
        (&mut s.stats_x, &mut s.stats_y, &mut s.stats_scale, 0.02, 0.10, 0.045),
    ] {
        *x = finite(*x, 0., 1., 0.5);
        *y = finite(*y, 0., 1., 0.5);
        *scale = finite(*scale, min, max, default);
    }
    s.microphone_id =
        h.mics.get(device_index(h.mics.iter().map(|d| d.id.as_str()), text(&s.microphone_id))).map(|d| d.id.clone());
    s.system_audio_device_id = h
        .outputs
        .get(device_index(h.outputs.iter().map(|d| d.id.as_str()), text(&s.system_audio_device_id)))
        .map(|d| d.id.clone());
    s.webcam_device_id = h
        .webcams
        .get(device_index(h.webcams.iter().map(|d| d.id.as_str()), text(&s.webcam_device_id)))
        .map(|d| d.id.clone());
    let mut seen = std::collections::HashSet::new();
    let apps = s.audio_apps.get_or_insert_with(Vec::new);
    apps.retain(|a| !text(&a.exe).trim().is_empty() && seen.insert(text(&a.exe).to_lowercase()));
    for app in apps {
        app.volume = app.volume.clamp(0, 100);
    }
}
