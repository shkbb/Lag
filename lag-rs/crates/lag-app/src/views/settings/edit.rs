//! What each control does to the settings: choices, toggles, numbers, presets; which changes need a
//! recording restart.

use super::*;
use super::{normalize::*, rows::*, state::*};
use crate::{app_ctx::AppCtx, AppWindow, MainVM};
use lag_core::{
    i18n,
    presets::{self, RecommendedSettings, RecordingIntent},
    settings::{AppAudioSetting, SettingsData},
};
use slint::ComponentHandle;
use std::{cell::RefCell, rc::Rc};

pub(super) const INTENTS: [RecordingIntent; 3] =
    [RecordingIntent::Performance, RecordingIntent::Balanced, RecordingIntent::Quality];

pub(super) fn matches_preset(s: &SettingsData, p: &RecommendedSettings) -> bool {
    s.frame_rate == p.fps
        && s.output_resolution_height == p.target_height
        && text(&s.codec_name) == p.codec_id
        && s.bitrate_kbps == p.bitrate_kbps
        && s.buffer_seconds == p.buffer_seconds
}
/// C# ApplyPreset selects from the current monitor's options. A primary-monitor
/// recommendation need not exist on the selected secondary monitor.
pub(super) fn apply_preset_choices(st: &mut State, p: RecommendedSettings) {
    let Some(h) = &st.hardware else {
        return;
    };
    let s = &mut st.settings;
    let monitor = h.monitors.get(monitor_index(&h.monitors, s));
    let heights = resolutions(monitor.map_or(1080, |m| m.height as i32));
    let rates = fps_options(monitor.map_or(0, |m| m.refresh_rate as i32));
    s.output_resolution_height = if heights.contains(&p.target_height) { p.target_height } else { 0 };
    s.frame_rate = if rates.contains(&p.fps) { p.fps } else { rates.iter().copied().find(|f| *f > 0).unwrap_or(30) };
    s.codec_name = Some(if h.codecs.iter().any(|(id, _)| *id == p.codec_id) { p.codec_id } else { String::new() });
    s.bitrate_kbps =
        if p.bitrate_kbps == -1 || presets::BITRATE_RUNGS.contains(&p.bitrate_kbps) { p.bitrate_kbps } else { -1 };
    s.buffer_seconds =
        if presets::BUFFER_RUNGS.contains(&p.buffer_seconds) { p.buffer_seconds } else { presets::BUFFER_RUNGS[0] };
    st.custom_fps = false;
    st.custom_bitrate = false;
}
pub(super) fn preset_summary(p: &RecommendedSettings) -> String {
    let res = if p.target_height == 0 { tr("Option_Native") } else { format!("{}p", p.target_height) };
    let codec = if p.codec_id.is_empty() { tr("Option_Auto") } else { p.codec_id.to_uppercase() };
    let rate = if p.bitrate_kbps < 0 { tr("Option_Auto") } else { format!("{} Mbps", p.bitrate_kbps / 1000) };
    let seconds = if p.buffer_seconds < 60 { p.buffer_seconds } else { p.buffer_seconds / 60 }.to_string();
    let buf = i18n::format(if p.buffer_seconds < 60 { "Time_SecShort" } else { "Time_MinShort" }, &[&seconds])
        .unwrap_or_default();
    format!("{res} · {}fps · {codec} · {rate} · {buf}", p.fps)
}

/// Capture-pipeline edits need a recording restart; appearance, hotkeys, cleanup and live HUD do not.
pub(super) fn restart_required(id: &str) -> bool {
    !matches!(
        id,
        "theme"
            | "language"
            | "storage"
            | "cleanup"
            // NOT "folder": the running session saves into the folder it started with (C#: restart required)
            | "startup"
            | "minimized"
            | "autostart"
            | "game-mode"
            | "save-key"
            | "screenshot-key"
            | "pause-key"
            | "record-key"
            | "ptt"
            | "stats-shown"
            | "stats-detail"
    )
}
pub(super) fn save(
    ui: &AppWindow,
    ctx: &Rc<AppCtx>,
    state: &Rc<RefCell<State>>,
    id: &str,
    edit: impl FnOnce(&mut State),
) {
    let next = {
        let mut st = state.borrow_mut();
        edit(&mut st);
        if restart_required(id) && ui.global::<MainVM>().get_is_recording() {
            st.pending = true;
        }
        st.settings.clone()
    };
    ctx.update_settings(|s| *s = next);
    if matches!(id, "fps" | "resolution" | "bitrate" | "monitor") {
        state.borrow().maybe_warn(ui);
    }
}
pub(super) fn choose(st: &mut State, id: &str, index: usize) {
    let Some(h) = &st.hardware else {
        return;
    };
    let s = &mut st.settings;
    let mon = h.monitors.get(monitor_index(&h.monitors, s));
    match id {
        "buffer" => {
            if let Some(v) = presets::BUFFER_RUNGS.get(index) {
                s.buffer_seconds = *v;
            }
        }
        "monitor" => {
            if let Some(m) = h.monitors.get(index) {
                let fps = fps_options(m.refresh_rate as i32);
                s.monitor_stable_id = Some(m.stable_id.clone());
                s.monitor_device_name = Some(m.device_name.clone());
                let cap = refresh_cap(m.refresh_rate as i32);
                if st.custom_fps {
                    s.frame_rate = s.frame_rate.clamp(1, cap);
                } else if !fps.contains(&s.frame_rate) {
                    s.frame_rate = fps
                        .iter()
                        .copied()
                        .filter(|v| *v > 0)
                        .min_by_key(|v| (*v - s.frame_rate.min(cap)).abs())
                        .unwrap_or(30);
                }
                if !resolutions(m.height as i32).contains(&s.output_resolution_height) {
                    s.output_resolution_height = 0;
                }
            }
        }
        "fps" => {
            if let Some(v) = fps_options(mon.map_or(0, |m| m.refresh_rate as i32)).get(index) {
                st.custom_fps = *v == 0;
                if *v > 0 {
                    s.frame_rate = *v;
                } else {
                    s.frame_rate = s.frame_rate.clamp(1, refresh_cap(mon.map_or(0, |m| m.refresh_rate as i32)));
                }
            }
        }
        "resolution" => {
            if let Some(v) = resolutions(mon.map_or(1080, |m| m.height as i32)).get(index) {
                s.output_resolution_height = *v;
            }
        }
        "codec" => {
            if let Some((id, _)) = h.codecs.get(index) {
                s.codec_name = Some(id.clone());
            }
        }
        "bitrate" => {
            st.custom_bitrate = index == presets::BITRATE_RUNGS.len() + 1;
            s.bitrate_kbps = if index == 0 {
                -1
            } else if st.custom_bitrate {
                (s.bitrate_kbps / 1000).clamp(3, 100) * 1000
            } else {
                presets::BITRATE_RUNGS.get(index - 1).copied().unwrap_or(-1)
            };
        }
        "gpu" => s.gpu_index = if index == 0 { -1 } else { h.gpus.get(index - 1).map_or(-1, |g| g.index) },
        "format" => s.file_format = Some(["mp4", "mov", "mkv"].get(index).unwrap_or(&"mp4").to_string()),
        "game-focus" => s.game_focus_mode = Some(if index == 1 { "desktop" } else { "game" }.into()),
        "audio-mode" => s.audio_capture_mode = Some(if index == 1 { "apps" } else { "all" }.into()),
        "microphone" => {
            if let Some(d) = h.mics.get(index) {
                s.microphone_id = Some(d.id.clone());
            }
        }
        "output" => {
            if let Some(d) = h.outputs.get(index) {
                s.system_audio_device_id = Some(d.id.clone());
            }
        }
        "webcam" => {
            if let Some(d) = h.webcams.get(index) {
                s.webcam_device_id = Some(d.id.clone());
            }
        }
        "stats-detail" => s.stats_detail = (index as i32).clamp(0, 2),
        "webcam-mode" => s.webcam_mode = (index as i32).clamp(0, lag_core::presets::WEBCAM_MODES.len() as i32 - 1),
        "channels" => s.mic_mono = index == 0,
        "theme" => s.theme = Some(["light", "dark", "system", "neon"].get(index).unwrap_or(&"dark").to_string()),
        "language" => s.language = Some(i18n::LANGUAGES.get(index).unwrap_or(&i18n::LANGUAGES[0]).code.into()),
        "storage" => s.max_library_size_gb = presets::STORAGE_RUNGS.get(index).copied().unwrap_or(50),
        _ => {}
    }
}
pub(super) fn set_toggle(s: &mut SettingsData, id: &str, value: bool) {
    match id {
        "audio-target" => s.audio_mode_by_target = value,
        "system-volume" => s.system_audio_enabled = value,
        "mic-volume" => s.mic_enabled = value,
        "game-volume" => s.game_audio_enabled = value,
        "sensitivity" => s.input_sensitivity_auto = value,
        "denoise" => s.noise_suppression = value,
        "tracks" => s.separate_audio_tracks = value,
        "ptt" => s.ptt_enabled = value,
        "overlay" => s.overlay_enabled = value,
        "webcam-shown" => s.webcam_shown = value,
        "keys-shown" => s.keys_shown = value,
        "stats-shown" => s.stats_shown = value,
        "startup" => s.start_with_windows = value,
        "minimized" => s.start_minimized = value,
        "autostart" => s.auto_start_recording = value,
        "game-mode" => s.disable_game_mode = value,
        "cleanup" => s.auto_cleanup_enabled = value,
        _ => {
            if let Some(exe) = id.strip_prefix("app:") {
                app_edit(s, exe, |a| a.enabled = value);
            }
        }
    }
}
pub(super) fn app_edit(s: &mut SettingsData, exe: &str, f: impl FnOnce(&mut AppAudioSetting)) {
    let apps = s.audio_apps.get_or_insert_with(Vec::new);
    let i = apps.iter().position(|a| text(&a.exe).eq_ignore_ascii_case(exe)).unwrap_or_else(|| {
        apps.push(AppAudioSetting { exe: Some(exe.into()), ..Default::default() });
        apps.len() - 1
    });
    f(&mut apps[i]);
    apps.retain(|a| a.enabled || a.volume != 100);
}
pub(super) fn set_number(st: &mut State, id: &str, value: f32) {
    if !value.is_finite() {
        return;
    }
    let v = value.round() as i32;
    let s = &mut st.settings;
    match id {
        "bitrate" => {
            if st.custom_bitrate {
                s.bitrate_kbps = v.clamp(3, 100) * 1000;
                // SettingsViewModel.cs:722-723 collapses this slider halfway through a drag.
                // Keep the user's explicit Custom selection until they choose another option.
            }
        }
        "system-volume" => s.system_audio_volume = v.clamp(0, 100),
        "mic-volume" => s.mic_volume = v.clamp(0, 200),
        "game-volume" => s.game_audio_volume = v.clamp(0, 100),
        "threshold" => s.input_sensitivity_threshold = v.clamp(0, 100),
        "fps" => {
            let cap = st
                .hardware
                .as_ref()
                .and_then(|h| h.monitors.get(monitor_index(&h.monitors, s)))
                .map_or(360, |m| refresh_cap(m.refresh_rate as i32));
            s.frame_rate = v.clamp(1, cap);
        }
        _ => {
            if let Some(exe) = id.strip_prefix("app:") {
                app_edit(s, exe, |a| a.volume = v.clamp(0, 100));
            }
        }
    }
}
