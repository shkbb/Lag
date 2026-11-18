//! Mirrors Views/SettingsView.axaml(.cs), ViewModels/SettingsViewModel.cs,
//! SettingsViewModel.Presets.cs, SettingsViewModel.AudioMeter.cs and Controls/SegmentedListBox.cs.
//! All persisted mutations go through AppCtx::update_settings. Hardware and disk work stay off UI.
use crate::{
    app_ctx::AppCtx, AppWindow, ComboBoxItemData, MainVM, OverlayBoxData, Page, PresetData, SegmentedOption,
    SettingsGroup, SettingsRowData, SettingsVM, Tr,
};
use lag_core::{
    hardware::{self, CodecTier, EncoderCapability, MachineProfile, MicrophoneInfo, MonitorInfo},
    i18n,
    presets::{self, RecommendedSettings, RecordingIntent},
    settings::{AppAudioSetting, SettingsData},
};
use slint::{ComponentHandle, Model, ModelRc, VecModel};
use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    rc::Rc,
    sync::{mpsc, Arc, Mutex},
    time::{Duration, Instant},
};

const INTENTS: [RecordingIntent; 3] =
    [RecordingIntent::Performance, RecordingIntent::Balanced, RecordingIntent::Quality];
fn model<T: Clone + 'static>(rows: Vec<T>) -> ModelRc<T> {
    Rc::new(VecModel::from(rows)).into()
}
fn tr(key: &str) -> String {
    i18n::tr(key).into()
}
fn text(s: &Option<String>) -> &str {
    s.as_deref().unwrap_or("")
}
fn refresh_cap(hz: i32) -> i32 {
    if hz < 24 {
        return 360;
    }
    [60, 75, 90, 100, 120, 144, 160, 165, 180, 200, 240, 280, 300, 360, 390, 480, 500]
        .into_iter()
        .find(|r| (hz - r).abs() <= 2)
        .unwrap_or(hz)
        .max(60)
}
fn fps_options(hz: i32) -> Vec<i32> {
    let cap = refresh_cap(hz);
    let mut values: Vec<_> = [24, 30, 60, 120, 240, 360].into_iter().filter(|v| *v <= cap).collect();
    if values.last().copied().unwrap_or(0) < cap {
        values.push(cap);
    }
    values.push(0);
    values
}
fn resolutions(height: i32) -> Vec<i32> {
    std::iter::once(0).chain([2160, 1440, 1080, 720].into_iter().filter(|v| *v <= height)).collect()
}
fn device_index<'a>(ids: impl IntoIterator<Item = &'a str>, saved: &str) -> usize {
    ids.into_iter().position(|id| id == saved).unwrap_or(0)
}
fn monitor_index(monitors: &[MonitorInfo], s: &SettingsData) -> usize {
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
fn finite(value: f64, min: f64, max: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.clamp(min, max)
    } else {
        fallback
    }
}
fn default_library() -> String {
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
fn normalize(s: &mut SettingsData, h: &Hardware) {
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
    if !["dark", "light", "system"].contains(&text(&s.theme)) {
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
struct Hardware {
    monitors: Vec<MonitorInfo>,
    mics: Vec<MicrophoneInfo>,
    outputs: Vec<MicrophoneInfo>,
    webcams: Vec<lag_capture::overlay::webcam::DeviceInfo>,
    gpus: Vec<hardware::GpuInfo>,
    codecs: Vec<(String, String)>,
    profile: MachineProfile,
}
impl Hardware {
    fn probe(s: &SettingsData) -> Self {
        let caps: Vec<_> = lag_capture::encoder::available()
            .iter()
            .map(|e| EncoderCapability {
                friendly_name: e.friendly_name.into(),
                is_hardware: e.is_hardware,
                tier: match e.tier {
                    lag_capture::encoder::CodecTier::H264 => CodecTier::H264,
                    lag_capture::encoder::CodecTier::Hevc => CodecTier::Hevc,
                    lag_capture::encoder::CodecTier::Av1 => CodecTier::Av1,
                },
            })
            .collect();
        let mut codecs = vec![(String::new(), tr("Option_Auto"))];
        for (tier, id, name) in [
            (CodecTier::H264, "h264", "H.264"),
            (CodecTier::Hevc, "hevc", "H.265 (HEVC)"),
            (CodecTier::Av1, "av1", "AV1"),
        ] {
            if caps.iter().any(|e| e.tier == tier) {
                codecs.push((id.into(), name.into()));
            }
        }
        if codecs.len() == 1 {
            codecs.push(("h264".into(), "H.264".into()));
        }
        let mut outputs = vec![MicrophoneInfo { id: String::new(), name: tr("Option_DefaultDevice") }];
        outputs.extend(hardware::get_output_devices());
        // Mic and webcam: an empty id = follow the Windows default device (owner decision 2026-11-16:
        // a saved device that is not connected falls back to the Windows default, not to the first one).
        let mut mics = vec![MicrophoneInfo { id: String::new(), name: tr("Option_DefaultDevice") }];
        mics.extend(hardware::get_microphones().into_iter().filter(|m| m.id != "default"));
        let mut webcams =
            vec![lag_capture::overlay::webcam::DeviceInfo { id: String::new(), name: tr("Option_DefaultDevice") }];
        webcams.extend(lag_capture::overlay::webcam::WebcamCaptureSource::list_devices());
        Self {
            monitors: hardware::get_available_monitors(),
            mics,
            outputs,
            webcams,
            gpus: hardware::get_gpu_adapters(),
            codecs,
            profile: MachineProfile::capture(s.library_path.as_deref().map(Path::new), &caps),
        }
    }
}
struct State {
    hardware: Option<Hardware>,
    settings: SettingsData,
    groups: Rc<VecModel<SettingsGroup>>,
    tab: i32,
    custom_fps: bool,
    custom_bitrate: bool,
    acknowledged: bool,
    applying_preset: bool,
    pending: bool,
    capture: String,
    suppressed_until: Instant,
    update_status: String,
    apps: Vec<(String, String)>,
    /// App icons by lower-case exe name, decoded on the UI thread (slint::Image is not Send).
    app_icons: std::collections::HashMap<String, slint::Image>,
}
fn row(id: &str, label: &str, kind: i32) -> SettingsRowData {
    SettingsRowData {
        id: id.into(),
        label: label.into(),
        kind,
        enabled: true,
        visible: true,
        divider: true,
        maximum: 100.,
        ..Default::default()
    }
}
fn choice(id: &str, label: &str, kind: i32, options: Vec<(String, i32)>, selected: usize) -> SettingsRowData {
    let mut r = row(id, label, kind);
    r.selected = selected as i32;
    r.options = model(options.iter().map(|(t, tier)| ComboBoxItemData { text: t.into(), tier: *tier }).collect());
    r.segments = model(options.into_iter().map(|(t, tier)| SegmentedOption { text: t.into(), tier }).collect());
    r
}
fn simple_options(items: &[&str]) -> Vec<(String, i32)> {
    items.iter().map(|s| (tr(s), 0)).collect()
}
fn toggle(id: &str, label: &str, desc: &str, checked: bool) -> SettingsRowData {
    let mut r = row(id, label, 2);
    r.checked = checked;
    r.description = desc.into();
    r
}
/// PNG bytes (lag_audio icon extractor, 32x32 RGBA) -> slint image.
fn decode_icon(png: &[u8]) -> Option<slint::Image> {
    let img = image::load_from_memory(png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(img.as_raw(), w, h);
    Some(slint::Image::from_rgba8(buf))
}
fn volume(id: &str, label: &str, enabled: bool, value: i32, max: i32) -> SettingsRowData {
    let mut r = row(id, label, 4);
    r.checked = enabled;
    r.value = value as f32;
    r.maximum = max as f32;
    r.divider = false;
    r
}
fn pos<T: PartialEq>(values: &[T], value: &T) -> usize {
    values.iter().position(|v| v == value).unwrap_or(0)
}
fn hotkey_display(key: &Option<String>, mods: &Option<String>) -> String {
    let k = text(key);
    if k.is_empty() || k == "VcUndefined" {
        return "—".into();
    }
    let m = text(mods);
    let mut parts = Vec::new();
    for (s, label) in [("Ctrl", "Ctrl"), ("Control", "Ctrl"), ("Alt", "Alt"), ("Shift", "Shift")] {
        if m.contains(s) && !parts.contains(&label) {
            parts.push(label);
        }
    }
    parts.push(k.strip_prefix("Vc").unwrap_or(k));
    parts.join(" + ")
}
impl State {
    fn groups(&self) -> Vec<(String, Vec<SettingsRowData>)> {
        let Some(h) = &self.hardware else {
            return vec![];
        };
        let s = &self.settings;
        let mon = h.monitors.get(monitor_index(&h.monitors, s));
        let hz = mon.map_or(0, |m| m.refresh_rate as i32);
        let height = mon.map_or(1080, |m| m.height as i32);
        let combo = |id, label, items: Vec<String>, index| {
            choice(id, label, 0, items.into_iter().map(|t| (t, 0)).collect(), index)
        };
        match self.tab {
            0 => {
                let fps = fps_options(hz);
                let res = resolutions(height);
                let mut rate = choice(
                    "fps",
                    "Settings_FrameRate",
                    1,
                    fps.iter()
                        .map(|v| {
                            (
                                if *v == 0 { tr("Option_Custom") } else { v.to_string() },
                                if *v > 120 {
                                    2
                                } else if *v > 60 {
                                    1
                                } else {
                                    0
                                },
                            )
                        })
                        .collect(),
                    if self.custom_fps { fps.len() - 1 } else { pos(&fps, &s.frame_rate) },
                );
                rate.custom = self.custom_fps;
                rate.display = s.frame_rate.to_string().into();
                let mut br = vec![(tr("Option_Auto"), 0)];
                br.extend(presets::BITRATE_RUNGS.iter().map(|v| {
                    (
                        format!("{} Mbps", v / 1000),
                        if *v > 50000 {
                            2
                        } else if *v > 30000 {
                            1
                        } else {
                            0
                        },
                    )
                }));
                br.push((tr("Option_Custom"), 0));
                let mut bitrate = choice(
                    "bitrate",
                    "Settings_Bitrate",
                    0,
                    br,
                    if self.custom_bitrate {
                        presets::BITRATE_RUNGS.len() + 1
                    } else if s.bitrate_kbps < 0 {
                        0
                    } else {
                        pos(presets::BITRATE_RUNGS, &s.bitrate_kbps) + 1
                    },
                );
                bitrate.custom = self.custom_bitrate;
                bitrate.value = (s.bitrate_kbps.max(3000) / 1000) as f32;
                let buffers = presets::BUFFER_RUNGS
                    .iter()
                    .map(|v| {
                        i18n::format(
                            if *v < 60 { "Time_SecShort" } else { "Time_MinShort" },
                            &[&if *v < 60 { *v } else { v / 60 }.to_string()],
                        )
                        .unwrap_or_default()
                    })
                    .collect();
                let mut gpus = vec![format!("{} ({})", tr("Option_Auto"), h.gpus.first().map_or("", |g| &g.name))];
                gpus.extend(h.gpus.iter().map(|g| format!("GPU {}: {}", g.index, g.name)));
                vec![
                    (
                        "Settings_Group_Recording".into(),
                        vec![
                            combo(
                                "buffer",
                                "Settings_BufferLength",
                                buffers,
                                pos(presets::BUFFER_RUNGS, &s.buffer_seconds),
                            ),
                            combo(
                                "monitor",
                                "Settings_Display",
                                h.monitors.iter().map(ToString::to_string).collect(),
                                monitor_index(&h.monitors, s),
                            ),
                            choice(
                                "game-focus",
                                "Settings_GameFocusMode",
                                0,
                                simple_options(&["GameFocus_KeepGame", "GameFocus_Desktop"]),
                                usize::from(text(&s.game_focus_mode) == "desktop"),
                            ),
                            rate,
                            choice(
                                "resolution",
                                "Settings_OutputResolution",
                                0,
                                res.iter()
                                    .map(|v| {
                                        let h = if *v == 0 { height } else { *v };
                                        (
                                            match *v {
                                                0 => tr("Option_Native"),
                                                2160 => "2160p (4K)".into(),
                                                1440 => "1440p (QHD)".into(),
                                                _ => format!("{v}p"),
                                            },
                                            if h > 1440 {
                                                2
                                            } else if h > 1080 {
                                                1
                                            } else {
                                                0
                                            },
                                        )
                                    })
                                    .collect(),
                                pos(&res, &s.output_resolution_height),
                            ),
                        ],
                    ),
                    (
                        "Settings_Group_Encoding".into(),
                        vec![
                            choice(
                                "codec",
                                "Settings_Codec",
                                1,
                                h.codecs
                                    .iter()
                                    .map(|(id, n)| (if id.is_empty() { tr("Option_Auto") } else { n.clone() }, 0))
                                    .collect(),
                                h.codecs.iter().position(|(id, _)| id == text(&s.codec_name)).unwrap_or(0),
                            ),
                            bitrate,
                            combo(
                                "gpu",
                                "Settings_Gpu",
                                gpus,
                                if s.gpu_index < 0 {
                                    0
                                } else {
                                    h.gpus.iter().position(|g| g.index == s.gpu_index).map_or(0, |i| i + 1)
                                },
                            ),
                            combo(
                                "format",
                                "Settings_Format",
                                vec!["MP4".into(), "MOV".into(), "MKV".into()],
                                pos(&["mp4", "mov", "mkv"], &text(&s.file_format)),
                            ),
                        ],
                    ),
                ]
            }
            1 => {
                let apps = text(&s.audio_capture_mode) == "apps" || s.audio_mode_by_target;
                let mut sources = vec![
                    choice(
                        "audio-mode",
                        "Settings_AudioMode",
                        1,
                        simple_options(&["AudioMode_All", "AudioMode_Apps"]),
                        usize::from(text(&s.audio_capture_mode) == "apps"),
                    ),
                    toggle(
                        "audio-target",
                        "Settings_AudioModeByTarget",
                        "Settings_AudioModeByTarget_Desc",
                        s.audio_mode_by_target,
                    ),
                ];
                let mut sys =
                    volume("system-volume", "Editor_TrackSystem", s.system_audio_enabled, s.system_audio_volume, 100);
                sys.visible = text(&s.audio_capture_mode) != "apps" || s.audio_mode_by_target;
                sources.push(sys);
                sources.push(volume("mic-volume", "Settings_Microphone", s.mic_enabled, s.mic_volume, 200));
                let mut game =
                    volume("game-volume", "Settings_GameSound", s.game_audio_enabled, s.game_audio_volume, 100);
                game.visible = apps;
                sources.push(game);
                if apps {
                    let saved = s.audio_apps.as_deref().unwrap_or(&[]);
                    for (exe, name) in &self.apps {
                        let a = saved.iter().find(|a| text(&a.exe).eq_ignore_ascii_case(exe));
                        let mut r = volume(
                            &format!("app:{exe}"),
                            name,
                            a.is_some_and(|a| a.enabled),
                            a.map_or(100, |a| a.volume),
                            100,
                        );
                        if let Some(icon) = self.app_icons.get(&exe.to_lowercase()) {
                            r.icon = icon.clone();
                        }
                        sources.push(r);
                    }
                    let mut hint = row("apps-hint", "Settings_AppsHint", 14);
                    hint.divider = false;
                    sources.push(hint);
                }
                let mut level = row("output-level", "Settings_OutputLevel", 13);
                level.divider = false;
                let mut bar = row("threshold", "", 5);
                bar.auto_mode = s.input_sensitivity_auto;
                bar.value = s.input_sensitivity_threshold as f32;
                bar.divider = false;
                let mut ptt = toggle("ptt", "Settings_Ptt", "Settings_Ptt_Desc", s.ptt_enabled);
                ptt.kind = 12;
                ptt.display = hotkey_display(&s.ptt_key, &None).into();
                vec![
                    ("Settings_Group_SystemAudio".into(), sources),
                    (
                        "Settings_Group_Devices".into(),
                        vec![
                            combo(
                                "output",
                                "Settings_OutputDevice",
                                h.outputs
                                    .iter()
                                    .map(|d| if d.id.is_empty() { tr("Option_DefaultDevice") } else { d.name.clone() })
                                    .collect(),
                                device_index(h.outputs.iter().map(|d| d.id.as_str()), text(&s.system_audio_device_id)),
                            ),
                            level,
                            combo(
                                "microphone",
                                "Settings_Microphone",
                                h.mics
                                    .iter()
                                    .map(|d| if d.id.is_empty() { tr("Option_DefaultDevice") } else { d.name.clone() })
                                    .collect(),
                                device_index(h.mics.iter().map(|d| d.id.as_str()), text(&s.microphone_id)),
                            ),
                            toggle(
                                "sensitivity",
                                "Settings_InputSensitivity",
                                "Settings_InputSensitivityHint",
                                s.input_sensitivity_auto,
                            ),
                            bar,
                            toggle(
                                "denoise",
                                "Settings_NoiseSuppression",
                                "Settings_NoiseSuppressionHint",
                                s.noise_suppression,
                            ),
                            {
                                let mut r = row("listen", "Settings_HearYourself", 6);
                                r.description = "Settings_HearYourselfHint".into();
                                r
                            },
                            choice(
                                "channels",
                                "Settings_MicChannels",
                                1,
                                simple_options(&["Channels_Mono", "Channels_Stereo"]),
                                usize::from(!s.mic_mono),
                            ),
                        ],
                    ),
                    (
                        "Settings_Group_Advanced".into(),
                        vec![
                            ptt,
                            toggle(
                                "tracks",
                                "Settings_SeparateTracks",
                                "Settings_SeparateTracks_Desc",
                                s.separate_audio_tracks,
                            ),
                        ],
                    ),
                ]
            }
            2 => {
                let mut camera = combo(
                    "webcam",
                    "Overlay_Camera",
                    h.webcams
                        .iter()
                        .map(|d| if d.id.is_empty() { tr("Option_DefaultDevice") } else { d.name.clone() })
                        .collect(),
                    device_index(h.webcams.iter().map(|d| d.id.as_str()), text(&s.webcam_device_id)),
                );
                camera.enabled = s.webcam_shown;
                camera.visible = s.overlay_enabled;
                camera.with_toggle = true;
                camera.toggle_id = "webcam-shown".into();
                camera.checked = s.webcam_shown;
                // Owner, 2026-11-18: camera modes like a webcam app's (resolution + 30/60 fps presets).
                let mut camera_mode = combo(
                    "webcam-mode",
                    "Overlay_CameraMode",
                    lag_core::presets::WEBCAM_MODES
                        .iter()
                        .map(
                            |&(w, h, f)| {
                                if w == 0 {
                                    tr("Option_Auto")
                                } else {
                                    format!("{w}\u{d7}{h} \u{b7} {f} FPS")
                                }
                            },
                        )
                        .collect(),
                    usize::try_from(s.webcam_mode).unwrap_or(0).min(lag_core::presets::WEBCAM_MODES.len() - 1),
                );
                camera_mode.enabled = s.webcam_shown;
                let mut stats = combo(
                    "stats-detail",
                    "Overlay_Stats",
                    vec!["FPS".into(), "FPS + CPU/GPU".into(), "FPS + CPU/GPU + RAM".into()],
                    s.stats_detail as usize,
                );
                stats.enabled = s.stats_shown;
                stats.visible = s.overlay_enabled;
                stats.with_toggle = true;
                stats.toggle_id = "stats-shown".into();
                stats.checked = s.stats_shown;
                let mut rows = vec![
                    toggle("overlay", "Overlay_Enable", "Overlay_Enable_Desc", s.overlay_enabled),
                    camera,
                    camera_mode,
                    toggle("keys-shown", "Overlay_Keys", "", s.keys_shown),
                    stats,
                ];
                for r in rows.iter_mut().skip(1) {
                    r.visible = s.overlay_enabled;
                }
                vec![("Settings_Group_Overlay".into(), rows)]
            }
            _ => {
                let mut keys = Vec::new();
                for (id, label, key, mods) in [
                    ("save-key", "Settings_Hotkey_SaveReplay", &s.hotkey_key, &s.hotkey_modifiers),
                    ("screenshot-key", "Settings_Hotkey_Screenshot", &s.screenshot_key, &s.screenshot_modifiers),
                    ("pause-key", "Settings_Hotkey_Pause", &s.pause_key, &s.pause_modifiers),
                    ("record-key", "Settings_Hotkey_Record", &s.record_key, &s.record_modifiers),
                ] {
                    let mut r = row(id, label, 3);
                    let display = hotkey_display(key, mods);
                    r.parts = model(display.split(" + ").map(Into::into).collect());
                    r.display = display.into();
                    keys.push(r);
                }
                let mut storage = combo(
                    "storage",
                    "Settings_StorageLimit",
                    presets::STORAGE_RUNGS.iter().map(|v| format!("{v} GB")).collect(),
                    pos(presets::STORAGE_RUNGS, &s.max_library_size_gb),
                );
                storage.enabled = s.auto_cleanup_enabled;
                let mut folder = row("folder", "Settings_LibraryFolder", 7);
                folder.display = text(&s.library_path).into();
                let mut about = row("updates", "About_Group", 8);
                about.enabled = self.update_status != tr("Update_Checking");
                about.display = if self.update_status.is_empty() {
                    format!("{} {}", tr("About_Version"), env!("CARGO_PKG_VERSION"))
                } else {
                    self.update_status.clone()
                }
                .into();
                let mut theme = choice(
                    "theme",
                    "Settings_Theme",
                    1,
                    simple_options(&["Theme_Light", "Theme_Dark", "Theme_System"]),
                    pos(&["light", "dark", "system"], &text(&s.theme)),
                );
                theme.description = "Settings_Theme_Desc".into();
                vec![
                    (
                        "".into(),
                        vec![
                            theme,
                            combo(
                                "language",
                                "Settings_Language",
                                i18n::LANGUAGES.iter().map(|l| l.display.into()).collect(),
                                i18n::LANGUAGES.iter().position(|l| l.code == text(&s.language)).unwrap_or(0),
                            ),
                        ],
                    ),
                    ("Settings_Group_Hotkey".into(), keys),
                    (
                        "Settings_Group_Startup".into(),
                        vec![
                            toggle("startup", "Settings_StartWithWindows", "", s.start_with_windows),
                            toggle(
                                "minimized",
                                "Settings_StartMinimized",
                                "Settings_StartMinimized_Desc",
                                s.start_minimized,
                            ),
                            toggle("autostart", "Settings_AutoStart", "", s.auto_start_recording),
                            toggle(
                                "game-mode",
                                "Settings_DisableGameMode",
                                "Settings_DisableGameMode_Desc",
                                s.disable_game_mode,
                            ),
                        ],
                    ),
                    (
                        "Settings_Group_Storage".into(),
                        vec![
                            toggle(
                                "cleanup",
                                "Settings_AutoCleanup",
                                "Settings_AutoCleanup_Desc",
                                s.auto_cleanup_enabled,
                            ),
                            storage,
                            folder,
                            about,
                            row("logs", "Settings_Logs", 9),
                        ],
                    ),
                ]
            }
        }
    }
    fn refresh(&self, ui: &AppWindow) {
        let vm = ui.global::<SettingsVM>();
        let groups = self.groups();
        // Retain delegate identity while dragging sliders; only update each row's data.
        if self.groups.row_count() == groups.len()
            && self.groups.iter().zip(&groups).all(|(a, b)| a.title.as_str() == b.0 && a.rows.row_count() == b.1.len())
        {
            for (old, (_, rows)) in self.groups.iter().zip(groups) {
                for (i, r) in rows.into_iter().enumerate() {
                    if let Some(m) = old.rows.as_any().downcast_ref::<VecModel<SettingsRowData>>() {
                        m.set_row_data(i, r);
                    }
                }
            }
        } else {
            self.groups.set_vec(
                groups
                    .into_iter()
                    .map(|(title, rows)| SettingsGroup { title: title.into(), rows: model(rows) })
                    .collect::<Vec<_>>(),
            );
        }
        vm.set_pending(self.pending);
        vm.set_overlay_enabled(self.settings.overlay_enabled);
        vm.set_overlay_boxes(model(vec![
            OverlayBoxData {
                xf: self.settings.webcam_x as f32,
                yf: self.settings.webcam_y as f32,
                scale: self.settings.webcam_scale as f32,
                shown: self.settings.webcam_shown,
            },
            OverlayBoxData {
                xf: self.settings.keys_x as f32,
                yf: self.settings.keys_y as f32,
                scale: self.settings.keys_scale as f32,
                shown: self.settings.keys_shown,
            },
            OverlayBoxData {
                xf: self.settings.stats_x as f32,
                yf: self.settings.stats_y as f32,
                scale: self.settings.stats_scale as f32,
                shown: self.settings.stats_shown,
            },
        ]));
        let Some(h) = &self.hardware else {
            return;
        };
        let presets: Vec<_> = INTENTS
            .iter()
            .enumerate()
            .map(|(i, intent)| {
                let p = presets::resolve(*intent, &h.profile);
                let names = ["Performance", "Balanced", "Quality"];
                PresetData {
                    title: format!("Preset_{}", names[i]).into(),
                    description: format!("Preset_{}Desc", names[i]).into(),
                    summary: preset_summary(&p).into(),
                    active: matches_preset(&self.settings, &p),
                }
            })
            .collect();
        vm.set_presets(model(presets));
        let b = presets::resolve(RecordingIntent::Balanced, &h.profile);
        vm.set_cleanup_hint(b.recommend_cleanup && !self.settings.auto_cleanup_enabled);
        vm.set_cleanup_text(
            i18n::format(
                "Preset_CleanupHint",
                &[&(h.profile.disk_free_gib as i32).to_string(), &b.suggested_storage_gb.to_string()],
            )
            .unwrap_or_default()
            .into(),
        );
        let ceil = presets::resolve(RecordingIntent::Quality, &h.profile);
        let mon = h.monitors.get(monitor_index(&h.monitors, &self.settings)).map_or(1080, |m| m.height as i32);
        let ceil_h = if ceil.target_height == 0 { mon } else { ceil.target_height };
        let lead = tr("Settings_IntensiveSummary");
        vm.set_warning_summary(
            format!(
                "{lead} {} FPS / {} / {ceil_h}p",
                ceil.fps,
                if ceil.bitrate_kbps > 0 { format!("{} Mbps", ceil.bitrate_kbps / 1000) } else { tr("Option_Auto") }
            )
            .into(),
        );
    }
    fn maybe_warn(&self, ui: &AppWindow) {
        if self.acknowledged || self.applying_preset {
            return;
        }
        if let Some(h) = &self.hardware {
            let ceil = presets::resolve(RecordingIntent::Quality, &h.profile);
            let native = h.monitors.get(monitor_index(&h.monitors, &self.settings)).map_or(1080, |m| m.height as i32);
            let height = if self.settings.output_resolution_height == 0 {
                native
            } else {
                self.settings.output_resolution_height
            };
            if self.settings.frame_rate > ceil.fps
                || (ceil.bitrate_kbps > 0 && self.settings.bitrate_kbps > ceil.bitrate_kbps)
                || height > if ceil.target_height == 0 { native } else { ceil.target_height }
            {
                ui.global::<SettingsVM>().set_warning(true);
            }
        }
    }
}
fn matches_preset(s: &SettingsData, p: &RecommendedSettings) -> bool {
    s.frame_rate == p.fps
        && s.output_resolution_height == p.target_height
        && text(&s.codec_name) == p.codec_id
        && s.bitrate_kbps == p.bitrate_kbps
        && s.buffer_seconds == p.buffer_seconds
}
/// C# ApplyPreset selects from the current monitor's options. A primary-monitor
/// recommendation need not exist on the selected secondary monitor.
fn apply_preset_choices(st: &mut State, p: RecommendedSettings) {
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
fn preset_summary(p: &RecommendedSettings) -> String {
    let res = if p.target_height == 0 { tr("Option_Native") } else { format!("{}p", p.target_height) };
    let codec = if p.codec_id.is_empty() { tr("Option_Auto") } else { p.codec_id.to_uppercase() };
    let rate = if p.bitrate_kbps < 0 { tr("Option_Auto") } else { format!("{} Mbps", p.bitrate_kbps / 1000) };
    let seconds = if p.buffer_seconds < 60 { p.buffer_seconds } else { p.buffer_seconds / 60 }.to_string();
    let buf = i18n::format(if p.buffer_seconds < 60 { "Time_SecShort" } else { "Time_MinShort" }, &[&seconds])
        .unwrap_or_default();
    format!("{res} · {}fps · {codec} · {rate} · {buf}", p.fps)
}

/// Capture-pipeline edits need a recording restart; appearance, hotkeys, cleanup and live HUD do not.
fn restart_required(id: &str) -> bool {
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
fn save(ui: &AppWindow, ctx: &Rc<AppCtx>, state: &Rc<RefCell<State>>, id: &str, edit: impl FnOnce(&mut State)) {
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
fn choose(st: &mut State, id: &str, index: usize) {
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
        "theme" => s.theme = Some(["light", "dark", "system"].get(index).unwrap_or(&"dark").to_string()),
        "language" => s.language = Some(i18n::LANGUAGES.get(index).unwrap_or(&i18n::LANGUAGES[0]).code.into()),
        "storage" => s.max_library_size_gb = presets::STORAGE_RUNGS.get(index).copied().unwrap_or(50),
        _ => {}
    }
}
fn set_toggle(s: &mut SettingsData, id: &str, value: bool) {
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
fn app_edit(s: &mut SettingsData, exe: &str, f: impl FnOnce(&mut AppAudioSetting)) {
    let apps = s.audio_apps.get_or_insert_with(Vec::new);
    let i = apps.iter().position(|a| text(&a.exe).eq_ignore_ascii_case(exe)).unwrap_or_else(|| {
        apps.push(AppAudioSetting { exe: Some(exe.into()), ..Default::default() });
        apps.len() - 1
    });
    f(&mut apps[i]);
    apps.retain(|a| a.enabled || a.volume != 100);
}
fn set_number(st: &mut State, id: &str, value: f32) {
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
fn flag_svg(code: &str) -> String {
    let mut svg =
        String::from("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"30\" height=\"20\" viewBox=\"0 0 30 20\">");
    for l in i18n::flag(code).unwrap_or(&[]) {
        let fill = l.fill.as_deref().unwrap_or("none");
        let stroke = l
            .stroke
            .as_ref()
            .map_or(String::new(), |s| format!(" stroke=\"{}\" stroke-width=\"{}\"", s.color, s.thickness));
        if let Some(path) = &l.geometry {
            svg.push_str(&format!("<path d=\"{path}\" fill=\"{fill}\"{stroke}/>"));
        }
        if let Some(e) = &l.ellipse {
            let xy: Vec<_> = e.center.split(',').collect();
            if xy.len() == 2 {
                svg.push_str(&format!(
                    "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" fill=\"{fill}\"{stroke}/>",
                    xy[0], xy[1], e.radius_x, e.radius_y
                ));
            }
        }
    }
    svg.push_str("</svg>");
    svg
}
fn apply_appearance(ui: &AppWindow, s: &SettingsData) {
    let code = text(&s.language);
    if i18n::current_language().code != code {
        i18n::set_language(code);
        let tr = ui.global::<Tr>();
        tr.set_rev(tr.get_rev().wrapping_add(1));
    }
    ui.global::<SettingsVM>().set_theme_mode(text(&s.theme).into());
}

/// Retained by the settings listener; all native audio operations run on this worker.
#[derive(Clone, PartialEq)]
struct MonitorConfig {
    active: bool,
    mic: Option<String>,
    output: Option<String>,
    auto: bool,
    threshold: i32,
    denoise: bool,
    listen: bool,
}
#[derive(Default)]
struct Peaks {
    mic: f32,
    output: f32,
    threshold: f32,
    open: bool,
}
struct AudioWorker {
    send: mpsc::Sender<MonitorConfig>,
    peaks: Arc<Mutex<Peaks>>,
}
impl AudioWorker {
    fn new() -> Self {
        let (send, recv) = mpsc::channel::<MonitorConfig>();
        let peaks = Arc::new(Mutex::new(Peaks::default()));
        let target = peaks.clone();
        std::thread::spawn(move || {
            let mut meter = lag_audio::monitor::AudioMonitor::new();
            let mut previous: Option<MonitorConfig> = None;
            loop {
                match recv.recv_timeout(Duration::from_millis(33)) {
                    Ok(mut cfg) => {
                        while let Ok(next) = recv.try_recv() {
                            cfg = next;
                        }
                        let restart = previous
                            .as_ref()
                            .is_none_or(|p| p.active != cfg.active || p.mic != cfg.mic || p.output != cfg.output);
                        if restart {
                            if cfg.active {
                                meter.start(
                                    cfg.mic.clone().filter(|s| !s.is_empty() && s != "default"),
                                    cfg.output.clone().filter(|s| !s.is_empty()),
                                );
                            } else {
                                meter.stop();
                            }
                        }
                        if cfg.active {
                            meter.set_input_gate(cfg.auto, cfg.threshold as f32 / 100.);
                            meter.set_noise_suppression(cfg.denoise);
                            if restart || previous.as_ref().is_none_or(|p| p.listen != cfg.listen) {
                                meter.set_monitoring(cfg.listen, cfg.output.clone().filter(|s| !s.is_empty()));
                            }
                        }
                        previous = Some(cfg);
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                if previous.as_ref().is_some_and(|p| p.active) {
                    *target.lock().unwrap() = Peaks {
                        mic: meter.read_mic_peak(),
                        output: meter.read_output_peak(),
                        threshold: meter.gate_threshold(),
                        open: meter.gate_open(),
                    };
                } else {
                    *target.lock().unwrap() = Peaks::default();
                }
            }
        });
        Self { send, peaks }
    }
}
fn smooth(current: f32, raw: f32) -> f32 {
    let target = raw.clamp(0., 1.).powf(0.6);
    current + (target - current) * if target > current { 0.5 } else { 0.16 }
}
fn window_active(ui: &AppWindow) -> bool {
    ui.window().is_visible()
        && crate::shell::nofocus::hwnd(ui.window())
            .is_some_and(|hwnd| unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() == hwnd })
}

pub fn init(ui: &AppWindow, ctx: &Rc<AppCtx>) {
    let initial = ctx.settings();
    let groups = Rc::new(VecModel::default());
    ui.global::<SettingsVM>().set_groups(groups.clone().into());
    ui.global::<SettingsVM>().set_language_flags(model(
        i18n::LANGUAGES
            .iter()
            .map(|l| slint::Image::load_from_svg_data(flag_svg(l.code).as_bytes()).unwrap_or_default())
            .collect(),
    ));
    apply_appearance(ui, &initial);
    let state = Rc::new(RefCell::new(State {
        hardware: None,
        settings: initial.clone(),
        groups,
        tab: 0,
        custom_fps: false,
        custom_bitrate: false,
        acknowledged: false,
        applying_preset: false,
        pending: false,
        capture: String::new(),
        suppressed_until: Instant::now(),
        update_status: String::new(),
        apps: Vec::new(),
        app_icons: Default::default(),
    }));
    // Hardware probing (including encoder-open tests and webcam enumeration) never blocks Slint.
    let (hardware_send, hardware_recv) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = hardware_send.send(Hardware::probe(&initial));
    });
    let (st, weak) = (state.clone(), ui.as_weak());
    ctx.on_settings_changed(move |s| {
        if let Some(ui) = weak.upgrade() {
            let mut st = st.borrow_mut();
            st.settings = s.clone();
            apply_appearance(&ui, s);
            st.refresh(&ui);
        }
    });
    let (st, c, weak) = (state.clone(), ctx.clone(), ui.as_weak());
    ui.global::<SettingsVM>().on_select_tab(move |tab| {
        if let Some(ui) = weak.upgrade() {
            let mut st = st.borrow_mut();
            st.tab = tab.clamp(0, 3);
            ui.global::<SettingsVM>().set_tab(st.tab);
            ui.global::<SettingsVM>().set_hear_yourself(false);
            crate::shell::hotkeys::cancel();
            st.capture.clear();
            ui.global::<SettingsVM>().set_capture_id("".into());
            st.refresh(&ui);
            let _ = &c;
        }
    });
    let (st, c, weak) = (state.clone(), ctx.clone(), ui.as_weak());
    ui.global::<SettingsVM>().on_choose(move |id, index| {
        if let Some(ui) = weak.upgrade() {
            if index >= 0 {
                save(&ui, &c, &st, &id, |s| choose(s, &id, index as usize));
            }
        }
    });
    let (st, c, weak) = (state.clone(), ctx.clone(), ui.as_weak());
    ui.global::<SettingsVM>().on_toggle(move |id, value| {
        if let Some(ui) = weak.upgrade() {
            save(&ui, &c, &st, &id, |s| set_toggle(&mut s.settings, &id, value));
        }
    });
    let (st, c, weak) = (state.clone(), ctx.clone(), ui.as_weak());
    ui.global::<SettingsVM>().on_number(move |id, value| {
        if let Some(ui) = weak.upgrade() {
            save(&ui, &c, &st, &id, |s| set_number(s, &id, value));
        }
    });
    let (st, c, weak) = (state.clone(), ctx.clone(), ui.as_weak());
    ui.global::<SettingsVM>().on_text(move |id, value| {
        if let (Some(ui), Ok(v)) = (weak.upgrade(), value.parse::<f32>()) {
            save(&ui, &c, &st, &id, |s| set_number(s, &id, v));
        }
    });
    let (st, c, weak) = (state.clone(), ctx.clone(), ui.as_weak());
    ui.global::<SettingsVM>().on_commit_overlay(move |i, x, y, scale| {
        if let Some(ui) = weak.upgrade() {
            save(&ui, &c, &st, "overlay-layout", |st| {
                let s = &mut st.settings;
                let (xx, yy, ss, min, max) = match i {
                    0 => (&mut s.webcam_x, &mut s.webcam_y, &mut s.webcam_scale, 0.06, 0.45),
                    1 => (&mut s.keys_x, &mut s.keys_y, &mut s.keys_scale, 0.06, 0.42),
                    _ => (&mut s.stats_x, &mut s.stats_y, &mut s.stats_scale, 0.02, 0.10),
                };
                *xx = finite(x as f64, 0., 1., 0.5);
                *yy = finite(y as f64, 0., 1., 0.5);
                *ss = finite(scale as f64, min, max, min);
            });
        }
    });
    let (st, c, weak) = (state.clone(), ctx.clone(), ui.as_weak());
    ui.global::<SettingsVM>().on_apply_preset(move |i| {
        if let Some(ui) = weak.upgrade() {
            let preset = {
                let st = st.borrow();
                st.hardware
                    .as_ref()
                    .and_then(|h| INTENTS.get(i as usize).map(|intent| presets::resolve(*intent, &h.profile)))
            };
            if let Some(p) = preset {
                save(&ui, &c, &st, "preset", |st| {
                    st.applying_preset = true;
                    apply_preset_choices(st, p);
                    st.applying_preset = false;
                });
            }
        }
    });
    let (st, weak) = (state.clone(), ui.as_weak());
    ui.global::<SettingsVM>().on_acknowledge(move || {
        st.borrow_mut().acknowledged = true;
        if let Some(ui) = weak.upgrade() {
            ui.global::<SettingsVM>().set_warning(false);
        }
    });
    let (st, c, weak) = (state.clone(), ctx.clone(), ui.as_weak());
    ui.global::<SettingsVM>().on_captured(move |key, mods| {
        if let Some(ui) = weak.upgrade() {
            finish_capture(&ui, &c, &st, key.to_string(), mods.to_string());
        }
    });
    let (st, c, weak) = (state.clone(), ctx.clone(), ui.as_weak());
    ui.global::<SettingsVM>().on_action(move |id| {
        let Some(ui) = weak.upgrade() else {
            return;
        };
        match id.as_str() {
            "listen" => {
                let vm = ui.global::<SettingsVM>();
                vm.set_hear_yourself(!vm.get_hear_yourself());
            }
            "folder" => {
                let weak = ui.as_weak();
                let old = text(&st.borrow().settings.library_path).to_string();
                // Native dialog is created only by an explicit user click, never during initialization/tests.
                let (sender, receiver) = mpsc::channel();
                std::thread::spawn(move || {
                    let _ = sender.send(browse_folder(&old));
                });
                let st = st.clone();
                let c = c.clone();
                let timer = Rc::new(slint::Timer::default());
                let keep = timer.clone();
                timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
                    if let Ok(result) = receiver.try_recv() {
                        keep.stop();
                        if let (Some(path), Some(ui)) = (result, weak.upgrade()) {
                            save(&ui, &c, &st, "folder", |st| st.settings.library_path = Some(path));
                            c.request_library_refresh();
                        }
                    }
                });
            }
            "logs" => {
                std::thread::spawn(|| {
                    if let Some(path) = lag_core::filelog::log_directory() {
                        open_in_explorer(&path, false);
                    }
                });
            }
            "diagnostics" => {
                std::thread::spawn(|| {
                    if let Err(e) = export_diagnostics() {
                        log::warn!("[Settings] diagnostics export failed: {e}");
                    }
                });
            }
            "updates" => {
                st.borrow_mut().update_status = tr("Update_Checking");
                st.borrow().refresh(&ui);
                let weak = ui.as_weak();
                crate::shell::updater::check_for_updates(false, move |status| {
                    if let Some(ui) = weak.upgrade() {
                        ui.global::<SettingsVM>().invoke_update_status(status.unwrap_or("").into());
                    }
                });
            }
            "save-key" | "screenshot-key" | "pause-key" | "record-key" | "ptt" => {
                crate::shell::hotkeys::cancel();
                st.borrow_mut().capture = id.to_string();
                ui.global::<SettingsVM>().set_capture_id(id.clone());
                let st = st.clone();
                let c = c.clone();
                let weak = ui.as_weak();
                crate::shell::hotkeys::start_capture(move |captured| {
                    if let Some(ui) = weak.upgrade() {
                        finish_capture(
                            &ui,
                            &c,
                            &st,
                            if captured.cleared { "VcUndefined".into() } else { captured.key },
                            captured.modifiers,
                        );
                    }
                });
            }
            _ => {}
        }
    });
    let (st, weak) = (state.clone(), ui.as_weak());
    ui.global::<SettingsVM>().on_update_status(move |key| {
        if let Some(ui) = weak.upgrade() {
            st.borrow_mut().update_status = if key.is_empty() { String::new() } else { tr(&key) };
            st.borrow().refresh(&ui);
        }
    });
    let audio = AudioWorker::new();
    let mut last_config = None;
    let mut was_active = false;
    let (device_send, device_recv) = mpsc::channel();
    let watcher = lag_audio::devices::AudioDeviceWatcher::new(move || {
        let _ = device_send.send(Instant::now());
    })
    .ok();
    let mut device_due = None;
    let mut devices_pending = false;
    let (devices_send, devices_recv) = mpsc::channel();
    let (apps_send, apps_recv) = mpsc::channel();
    let mut apps_pending = false;
    let mut next_apps = Instant::now();
    let mut startup_check = Some(Instant::now() + Duration::from_secs(3));
    let timer = Rc::new(slint::Timer::default());
    let keep_timer = timer.clone();
    let weak = ui.as_weak();
    let st = state.clone();
    let c = ctx.clone();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(33), move || {
        let _keep_watcher = &watcher;
        let Some(ui) = weak.upgrade() else {
            keep_timer.stop();
            return;
        };
        if let Ok(h) = hardware_recv.try_recv() {
            let normalized = {
                let mut st = st.borrow_mut();
                let mut settings = c.settings();
                normalize(&mut settings, &h);
                st.custom_fps = !fps_options(
                    h.monitors.get(monitor_index(&h.monitors, &settings)).map_or(0, |m| m.refresh_rate as i32),
                )
                .contains(&settings.frame_rate);
                st.custom_bitrate =
                    settings.bitrate_kbps > 0 && !presets::BITRATE_RUNGS.contains(&settings.bitrate_kbps);
                st.apps = settings
                    .audio_apps
                    .as_deref()
                    .unwrap_or(&[])
                    .iter()
                    .map(|a| {
                        let exe = text(&a.exe);
                        (exe.into(), exe.strip_suffix(".exe").unwrap_or(exe).into())
                    })
                    .collect();
                st.settings = settings.clone();
                st.hardware = Some(h);
                settings
            };
            // Existing raw settings are normalized through the sole persistence contract.
            // Initialization never dirties the store. The normalized snapshot is persisted on the
            // first actual user edit; shared AppCtx has no non-dirty initialization API.
            apply_appearance(&ui, &normalized);
            st.borrow().refresh(&ui);
        }
        let focused = window_active(&ui);
        let active = ui.global::<MainVM>().get_current_page() == Page::Settings && st.borrow().tab == 1 && focused;
        let vm = ui.global::<SettingsVM>();
        if !active {
            vm.set_hear_yourself(false);
            vm.set_mic_level(0.);
            vm.set_output_level(0.);
        }
        if !ui.global::<MainVM>().get_is_recording() && st.borrow().pending {
            st.borrow_mut().pending = false;
            vm.set_pending(false);
        }
        if (!focused || ui.global::<MainVM>().get_current_page() != Page::Settings) && !st.borrow().capture.is_empty() {
            crate::shell::hotkeys::cancel();
            st.borrow_mut().capture.clear();
            vm.set_capture_id("".into());
        }
        vm.set_hotkeys_suppressed(crate::shell::hotkeys::are_suppressed());
        let cfg = {
            let s = &st.borrow().settings;
            MonitorConfig {
                active,
                mic: s.microphone_id.clone().filter(|id| !id.is_empty()),
                output: s.system_audio_device_id.clone(),
                auto: s.input_sensitivity_auto,
                threshold: s.input_sensitivity_threshold,
                denoise: s.noise_suppression,
                listen: vm.get_hear_yourself(),
            }
        };
        if last_config.as_ref() != Some(&cfg) {
            let _ = audio.send.send(cfg.clone());
            last_config = Some(cfg);
        }
        if active {
            let p = audio.peaks.lock().unwrap();
            vm.set_mic_level(smooth(vm.get_mic_level(), p.mic));
            vm.set_output_level(smooth(vm.get_output_level(), p.output));
            vm.set_gate_threshold(p.threshold);
            vm.set_gate_open(p.open);
        }
        while let Ok(when) = device_recv.try_recv() {
            device_due = Some(when + Duration::from_millis(300));
        }
        if !devices_pending && device_due.is_some_and(|due| Instant::now() >= due) {
            device_due = None;
            devices_pending = true;
            let sender = devices_send.clone();
            std::thread::spawn(move || {
                let mut out = vec![MicrophoneInfo { id: String::new(), name: tr("Option_DefaultDevice") }];
                out.extend(hardware::get_output_devices());
                let _ = sender.send((hardware::get_microphones(), out));
            });
        }
        if let Ok((mics, outputs)) = devices_recv.try_recv() {
            devices_pending = false;
            {
                let mut st = st.borrow_mut();
                if let Some(h) = &mut st.hardware {
                    h.mics = mics;
                    h.outputs = outputs;
                }
                if let Some(h) = &st.hardware {
                    let mut settings = st.settings.clone();
                    normalize(&mut settings, h);
                    st.settings = settings;
                }
                st.refresh(&ui);
            }
            last_config = None; // Reopen even if only the default endpoint changed.
            let _ = audio.send.send(MonitorConfig {
                active: false,
                mic: None,
                output: None,
                auto: true,
                threshold: 8,
                denoise: false,
                listen: false,
            });
        }
        let show_apps = {
            let s = &st.borrow().settings;
            text(&s.audio_capture_mode) == "apps" || s.audio_mode_by_target
        };
        if active && show_apps && (!was_active || Instant::now() >= next_apps) && !apps_pending {
            apps_pending = true;
            next_apps = Instant::now() + Duration::from_secs(4);
            let sender = apps_send.clone();
            let monitor = st.borrow().settings.monitor_device_name.clone();
            std::thread::spawn(move || {
                let games = lag_capture::detect::GameDetector::new().running_game_exes(monitor.as_deref());
                let apps = lag_audio::sessions::get_active_audio_apps()
                    .into_iter()
                    .filter(|a| !games.contains(&a.exe_name.to_lowercase()))
                    .map(|a| (a.exe_name, a.display_name, a.icon_png))
                    .collect::<Vec<_>>();
                let _ = sender.send(apps);
            });
        }
        if let Ok(apps) = apps_recv.try_recv() {
            apps_pending = false;
            let mut st = st.borrow_mut();
            let saved = st.settings.audio_apps.clone().unwrap_or_default();
            for (exe, _, png) in &apps {
                let key = exe.to_lowercase();
                if !st.app_icons.contains_key(&key) {
                    if let Some(img) = png.as_deref().and_then(decode_icon) {
                        st.app_icons.insert(key, img);
                    }
                }
            }
            st.apps.retain(|(exe, _)| {
                apps.iter().any(|(e, _, _)| e.eq_ignore_ascii_case(exe))
                    || saved.iter().any(|a| a.enabled && text(&a.exe).eq_ignore_ascii_case(exe))
            });
            for (exe, name, _) in apps {
                if let Some(a) = st.apps.iter_mut().find(|(e, _)| e.eq_ignore_ascii_case(&exe)) {
                    a.1 = name;
                } else {
                    st.apps.push((exe, name));
                }
            }
            st.refresh(&ui);
        }
        was_active = active && show_apps;
        if startup_check.is_some_and(|due| Instant::now() >= due) {
            startup_check = None;
            crate::shell::updater::check_for_updates(true, |_| {});
        }
    });
}
fn finish_capture(ui: &AppWindow, ctx: &Rc<AppCtx>, state: &Rc<RefCell<State>>, key: String, mods: String) {
    let id = state.borrow().capture.clone();
    if id.is_empty() {
        return;
    }
    let key = if key == "VcEscape" { "VcUndefined".into() } else { key };
    let mods = if key == "VcUndefined" { "None".into() } else { mods };
    save(ui, ctx, state, &id, |st| {
        let s = &mut st.settings;
        match id.as_str() {
            "save-key" => {
                s.hotkey_key = Some(key);
                s.hotkey_modifiers = Some(mods);
            }
            "screenshot-key" => {
                s.screenshot_key = Some(key);
                s.screenshot_modifiers = Some(mods);
            }
            "pause-key" => {
                s.pause_key = Some(key);
                s.pause_modifiers = Some(mods);
            }
            "record-key" => {
                s.record_key = Some(key);
                s.record_modifiers = Some(mods);
            }
            "ptt" => s.ptt_key = Some(key),
            _ => {}
        }
        st.capture.clear();
        st.suppressed_until = Instant::now() + Duration::from_millis(800);
    });
    ui.global::<SettingsVM>().set_capture_id("".into());
    crate::shell::hotkeys::cancel();
}

fn browse_folder(current: &str) -> Option<String> {
    use windows::{
        core::PCWSTR,
        Win32::{System::Com::*, UI::Shell::*},
    };
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().ok()?;
        struct Apartment;
        impl Drop for Apartment {
            fn drop(&mut self) {
                unsafe {
                    CoUninitialize();
                }
            }
        }
        let _com = Apartment;
        let dialog: IFileOpenDialog = CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        dialog.SetOptions(FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM | FOS_PATHMUSTEXIST | FOS_NOCHANGEDIR).ok()?;
        let title: Vec<u16> = tr("Browse_Title").encode_utf16().chain(Some(0)).collect();
        let _ = dialog.SetTitle(PCWSTR(title.as_ptr()));
        let folder: Vec<u16> = current.encode_utf16().chain(Some(0)).collect();
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(PCWSTR(folder.as_ptr()), None) {
            let _ = dialog.SetFolder(&item);
        }
        dialog.Show(None).ok()?;
        let item = dialog.GetResult().ok()?;
        let name = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let result = name.to_string().ok();
        CoTaskMemFree(Some(name.0.cast()));
        result
    }
}
fn open_in_explorer(path: &Path, select: bool) {
    use std::os::windows::process::CommandExt;
    let mut command = std::process::Command::new("explorer.exe");
    if select {
        command.arg(format!("/select,{}", path.display()));
    } else {
        command.arg(path);
    }
    let _ = command.creation_flags(0x08000000).spawn();
}
/// Minimal ZIP STORE writer: avoids staging/copying logs and streams each file only once.
/// Records UTF-8 names and CRC32; no new compression dependency is needed by this package.
fn diagnostics_zip(directory: &Path, destination: &Path) -> std::io::Result<()> {
    use std::io::{Read, Seek, Write};
    fn u16le(w: &mut impl Write, n: u16) -> std::io::Result<()> {
        w.write_all(&n.to_le_bytes())
    }
    fn u32le(w: &mut impl Write, n: u32) -> std::io::Result<()> {
        w.write_all(&n.to_le_bytes())
    }
    let mut zip = std::fs::File::create(destination)?;
    let mut central = Vec::new();
    let mut count = 0u16;
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned().into_bytes();
        let name_len = u16::try_from(name.len()).map_err(|_| std::io::Error::other("diagnostic filename too long"))?;
        let offset = u32::try_from(zip.stream_position()?)
            .map_err(|_| std::io::Error::other("diagnostics exceed ZIP32 size"))?;
        let mut input = std::fs::File::open(entry.path())?;
        let mut bytes = Vec::new();
        input.read_to_end(&mut bytes)?;
        let size = u32::try_from(bytes.len()).map_err(|_| std::io::Error::other("log exceeds ZIP32 size"))?;
        let mut crc = !0u32;
        for b in &bytes {
            crc ^= u32::from(*b);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb88320u32.wrapping_mul(crc & 1));
            }
        }
        crc = !crc;
        u32le(&mut zip, 0x04034b50)?;
        for v in [20, 0x800, 0, 0, 0x21] {
            u16le(&mut zip, v)?;
        }
        u32le(&mut zip, crc)?;
        u32le(&mut zip, size)?;
        u32le(&mut zip, size)?;
        u16le(&mut zip, name_len)?;
        u16le(&mut zip, 0)?;
        zip.write_all(&name)?;
        zip.write_all(&bytes)?;
        u32le(&mut central, 0x02014b50)?;
        for v in [20, 20, 0x800, 0, 0, 0x21] {
            u16le(&mut central, v)?;
        }
        u32le(&mut central, crc)?;
        u32le(&mut central, size)?;
        u32le(&mut central, size)?;
        for v in [name_len, 0, 0, 0, 0] {
            u16le(&mut central, v)?;
        }
        u32le(&mut central, 0)?;
        u32le(&mut central, offset)?;
        central.write_all(&name)?;
        count = count.checked_add(1).ok_or_else(|| std::io::Error::other("too many diagnostic files"))?;
    }
    let offset =
        u32::try_from(zip.stream_position()?).map_err(|_| std::io::Error::other("diagnostics exceed ZIP32 size"))?;
    zip.write_all(&central)?;
    u32le(&mut zip, 0x06054b50)?;
    u16le(&mut zip, 0)?;
    u16le(&mut zip, 0)?;
    u16le(&mut zip, count)?;
    u16le(&mut zip, count)?;
    u32le(&mut zip, central.len() as u32)?;
    u32le(&mut zip, offset)?;
    u16le(&mut zip, 0)?;
    zip.flush()
}
fn export_diagnostics() -> std::io::Result<()> {
    let logs =
        lag_core::filelog::log_directory().ok_or_else(|| std::io::Error::other("Documents directory unavailable"))?;
    let parent = logs.parent().ok_or_else(|| std::io::Error::other("Invalid logs path"))?;
    std::fs::create_dir_all(&logs)?;
    let target = parent.join(format!("Lag-diagnostics-{}.zip", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S")));
    diagnostics_zip(&logs, &target)?;
    open_in_explorer(&target, true);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hardware() -> Hardware {
        Hardware {
            monitors: vec![
                MonitorInfo {
                    index: 0,
                    device_name: "DISPLAY1".into(),
                    stable_id: "panel-A".into(),
                    width: 2560,
                    height: 1440,
                    is_primary: true,
                    refresh_rate: 144,
                },
                MonitorInfo {
                    index: 1,
                    device_name: "DISPLAY2".into(),
                    stable_id: "panel-B".into(),
                    width: 1920,
                    height: 1080,
                    is_primary: false,
                    refresh_rate: 60,
                },
            ],
            mics: vec![
                MicrophoneInfo { id: "".into(), name: "Default".into() },
                MicrophoneInfo { id: "mic-first".into(), name: "Mic".into() },
            ],
            outputs: vec![
                MicrophoneInfo { id: "".into(), name: "Default".into() },
                MicrophoneInfo { id: "speakers".into(), name: "Speakers".into() },
            ],
            webcams: vec![
                lag_capture::overlay::webcam::DeviceInfo { id: "".into(), name: "Default".into() },
                lag_capture::overlay::webcam::DeviceInfo { id: "camera-first".into(), name: "Camera".into() },
            ],
            gpus: vec![],
            codecs: vec![("".into(), "Auto".into()), ("h264".into(), "H.264".into())],
            profile: MachineProfile {
                gpus: vec![],
                primary_gpu: None,
                hardware_encoders: vec![],
                has_av1: false,
                has_hevc: false,
                has_h264: true,
                cpu_name: "Test CPU".into(),
                cpu_logical_cores: 4,
                ram_gib: 8.,
                library_drive: "C:".into(),
                disk_free_gib: 20.,
                disk_total_gib: 100.,
                monitor_width: 2560,
                monitor_height: 1440,
                monitor_hz: 144,
            },
        }
    }
    #[test]
    fn option_lists_match_caps_and_rungs() {
        assert_eq!(fps_options(143), vec![24, 30, 60, 120, 144, 0]);
        assert_eq!(fps_options(23), vec![24, 30, 60, 120, 240, 360, 0]);
        assert_eq!(fps_options(59), vec![24, 30, 60, 0]);
        assert_eq!(fps_options(240), vec![24, 30, 60, 120, 240, 0]);
        assert_eq!(fps_options(500), vec![24, 30, 60, 120, 240, 360, 500, 0]);
        assert_eq!(resolutions(1080), vec![0, 1080, 720]);
        assert_eq!(resolutions(600), vec![0]);
        assert_eq!(presets::BUFFER_RUNGS, &[30, 60, 120, 300, 600, 900]);
        assert_eq!(presets::BITRATE_RUNGS.len(), 11);
        assert_eq!(presets::STORAGE_RUNGS, &[10, 25, 50, 100, 200, 500]);
    }
    #[test]
    fn migration_and_clamping_do_not_need_gui() {
        let mut s = SettingsData {
            buffer_seconds: 0,
            buffer_minutes: 10,
            frame_rate: 999,
            mic_volume: 999,
            system_audio_volume: -2,
            game_audio_volume: 105,
            input_sensitivity_threshold: -8,
            webcam_x: f64::NAN,
            keys_scale: 8.,
            stats_scale: 0.,
            stats_detail: 9,
            max_library_size_gb: 42,
            library_path: Some(" ".into()),
            monitor_device_name: Some("DISPLAY2".into()),
            monitor_stable_id: None,
            bitrate_kbps: 4321,
            codec_name: Some("missing".into()),
            game_focus_mode: Some("invalid".into()),
            ..Default::default()
        };
        normalize(&mut s, &hardware());
        assert_eq!(s.buffer_seconds, 600);
        assert_eq!(s.monitor_stable_id.as_deref(), Some("panel-B"));
        assert_eq!(s.frame_rate, 60);
        assert_eq!(s.mic_volume, 200);
        assert_eq!(s.system_audio_volume, 0);
        assert_eq!(s.game_audio_volume, 100);
        assert_eq!(s.input_sensitivity_threshold, 0);
        assert!(s.webcam_x.is_finite());
        assert_eq!(s.keys_scale, 0.42);
        assert_eq!(s.stats_scale, 0.02);
        assert_eq!(s.stats_detail, 2);
        assert_eq!(s.max_library_size_gb, 50);
        assert!(!text(&s.library_path).trim().is_empty());
        assert_eq!(s.bitrate_kbps, 4000);
        assert_eq!(text(&s.codec_name), "");
        assert_eq!(text(&s.game_focus_mode), "game");
    }
    #[test]
    fn missing_devices_use_first_not_default_microphone() {
        let h = hardware();
        let mut s = SettingsData {
            microphone_id: Some("gone".into()),
            system_audio_device_id: Some("gone".into()),
            webcam_device_id: Some("gone".into()),
            ..Default::default()
        };
        normalize(&mut s, &h);
        // Absent saved devices fall back to the Windows default item (empty id), not the first device.
        assert_eq!(text(&s.microphone_id), "");
        assert_eq!(text(&s.system_audio_device_id), "");
        assert_eq!(text(&s.webcam_device_id), "");
        s.monitor_stable_id = Some("gone".into());
        s.monitor_device_name = Some("DISPLAY2".into());
        assert_eq!(monitor_index(&h.monitors, &s), 0);
        s.monitor_stable_id = Some("panel-B".into());
        assert_eq!(monitor_index(&h.monitors, &s), 1);
    }
    #[test]
    fn app_rules_deduplicate_clamp_and_filter() {
        let mut s = SettingsData {
            audio_apps: Some(vec![
                AppAudioSetting { exe: Some("Game.exe".into()), enabled: true, volume: 999 },
                AppAudioSetting { exe: Some("GAME.EXE".into()), enabled: false, volume: 100 },
                AppAudioSetting { exe: Some(" ".into()), ..Default::default() },
            ]),
            ..Default::default()
        };
        normalize(&mut s, &hardware());
        assert_eq!(s.audio_apps.as_ref().unwrap().len(), 1);
        assert_eq!(s.audio_apps.as_ref().unwrap()[0].volume, 100);
        app_edit(&mut s, "Game.exe", |a| a.enabled = false);
        assert!(s.audio_apps.as_ref().unwrap().is_empty());
        app_edit(&mut s, "Music.exe", |a| a.volume = 42);
        assert_eq!(s.audio_apps.as_ref().unwrap()[0].volume, 42);
    }
    #[test]
    fn preset_maths_and_matching() {
        let h = hardware();
        for intent in INTENTS {
            let p = presets::resolve(intent, &h.profile);
            assert!(p.fps <= 144);
            assert_eq!(p.target_height, 720);
            assert!(presets::BUFFER_RUNGS.contains(&p.buffer_seconds));
            assert!(p.recommend_cleanup);
            let s = SettingsData {
                frame_rate: p.fps,
                output_resolution_height: p.target_height,
                codec_name: Some(p.codec_id.clone()),
                bitrate_kbps: p.bitrate_kbps,
                buffer_seconds: p.buffer_seconds,
                ..Default::default()
            };
            assert!(matches_preset(&s, &p));
            let mut wrong = s;
            wrong.frame_rate += 1;
            assert!(!matches_preset(&wrong, &p));
        }
    }
    fn state_with_hardware(settings: SettingsData) -> State {
        State {
            hardware: Some(hardware()),
            settings,
            groups: Rc::new(VecModel::default()),
            tab: 0,
            custom_fps: false,
            custom_bitrate: false,
            acknowledged: false,
            applying_preset: false,
            pending: false,
            capture: String::new(),
            suppressed_until: Instant::now(),
            update_status: String::new(),
            apps: Vec::new(),
            app_icons: Default::default(),
        }
    }
    #[test]
    fn preset_falls_back_to_selected_monitor_options() {
        let mut st =
            state_with_hardware(SettingsData { monitor_stable_id: Some("panel-B".into()), ..Default::default() });
        apply_preset_choices(
            &mut st,
            RecommendedSettings {
                target_height: 1440,
                fps: 120,
                codec_id: "av1".into(),
                bitrate_kbps: 70000,
                buffer_seconds: 120,
                recommend_cleanup: false,
                suggested_storage_gb: 0,
            },
        );
        assert_eq!(st.settings.output_resolution_height, 0);
        assert_eq!(st.settings.frame_rate, 24); // C# first positive rung fallback.
        assert_eq!(text(&st.settings.codec_name), "");
        assert_eq!(st.settings.bitrate_kbps, 70000);
        assert_eq!(st.settings.buffer_seconds, 120);
    }
    #[test]
    fn custom_bitrate_stays_visible_when_crossing_fixed_rung() {
        let mut st = state_with_hardware(SettingsData::default());
        st.custom_bitrate = true;
        set_number(&mut st, "bitrate", 30.0);
        assert_eq!(st.settings.bitrate_kbps, 30000);
        assert!(st.custom_bitrate);
        set_number(&mut st, "bitrate", 999.0);
        assert_eq!(st.settings.bitrate_kbps, 100000);
        assert!(st.custom_bitrate);
    }
    #[test]
    fn hotkey_format_and_restart_flags() {
        assert_eq!(hotkey_display(&Some("VcF10".into()), &Some("LeftAlt, RightCtrl".into())), "Ctrl + Alt + F10");
        assert_eq!(hotkey_display(&Some("VcUndefined".into()), &None), "—");
        assert!(!restart_required("language"));
        assert!(!restart_required("stats-shown"));
        assert!(restart_required("game-focus"));
        assert!(restart_required("overlay-layout"));
        assert_eq!(smooth(0., 1.), 0.5);
        assert!((smooth(1., 0.) - 0.84).abs() < 0.0001);
    }
    #[test]
    fn all_new_strings_and_flags_are_present() {
        for l in i18n::LANGUAGES {
            for key in [
                "Settings_GameFocusMode",
                "GameFocus_KeepGame",
                "GameFocus_Desktop",
                "Settings_GameSound",
                "Settings_IntensiveTitle",
                "Settings_IntensiveBody",
                "Settings_IntensiveOk",
            ] {
                assert_ne!(i18n::tr_in(l.code, key), key, "{} {key}", l.code);
            }
            assert!(flag_svg(l.code).contains("<path"));
        }
        assert_eq!(i18n::tr_in("uk", "Settings_GameFocusMode"), "Коли гра не у фокусі");
        assert_eq!(i18n::tr_in("en", "GameFocus_KeepGame"), "Keep recording the game");
    }
}
