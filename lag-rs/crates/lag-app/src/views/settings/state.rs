//! `State`: the page's state and the rows of every tab, rebuilt from `SettingsData`.

use super::*;
use super::{devices::*, edit::*, normalize::*, rows::*};
use crate::{AppWindow, OverlayBoxData, PresetData, SettingsGroup, SettingsRowData, SettingsVM};
use lag_core::{
    i18n,
    presets::{self, RecordingIntent},
    settings::SettingsData,
};
use crate::ComponentHandle;
use slint::{Model, VecModel};
use std::{rc::Rc, time::Instant};

pub(super) struct State {
    pub(super) hardware: Option<Hardware>,
    pub(super) settings: SettingsData,
    pub(super) groups: Rc<VecModel<SettingsGroup>>,
    pub(super) tab: i32,
    pub(super) custom_fps: bool,
    pub(super) custom_bitrate: bool,
    pub(super) acknowledged: bool,
    pub(super) applying_preset: bool,
    pub(super) pending: bool,
    pub(super) capture: String,
    pub(super) suppressed_until: Instant,
    pub(super) update_status: String,
    pub(super) apps: Vec<(String, String)>,
    /// App icons by lower-case exe name, decoded on the UI thread (slint::Image is not Send).
    pub(super) app_icons: std::collections::HashMap<String, slint::Image>,
    /// The cloud part of the Library tab (services/cloud.rs keeps it current).
    pub(super) cloud: crate::services::cloud::CloudView,
    /// The result of the last theme import / export, shown under the theme buttons.
    pub(super) theme_note: String,
    /// which buttons row the note belongs to ("theme-own" / "theme-share")
    pub(super) theme_note_at: &'static str,
    /// the theme (id) the note of the last per-theme action is about (shown in that theme's row)
    pub(super) theme_note_for: String,
    /// what is typed in the New theme field
    pub(super) theme_new_name: String,
    /// "Delete theme" was pressed once for this theme (a second press within a few seconds moves it to the bin)
    pub(super) theme_delete: Option<(String, std::time::Instant)>,
}

impl State {
    /// A fresh page state for `settings`; `groups` is the model the page shows (rebuilt by `refresh`).
    pub(super) fn new(settings: SettingsData, groups: Rc<VecModel<SettingsGroup>>) -> Self {
        Self {
            hardware: None,
            settings,
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
            cloud: Default::default(),
            theme_note: String::new(),
            theme_note_at: "",
            theme_note_for: String::new(),
            theme_new_name: String::new(),
            theme_delete: None,
        }
    }

    pub(super) fn groups(&self) -> Vec<(String, Vec<SettingsRowData>)> {
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
            3 => {
                // Library (owner, 2026-11-20): everything about the clips in one place - the folder and its size
                // limit on this PC, and the cloud next to it
                let mut storage = combo(
                    "storage",
                    "Settings_StorageLimit",
                    presets::STORAGE_RUNGS.iter().map(|v| format!("{v} GB")).collect(),
                    pos(presets::STORAGE_RUNGS, &s.max_library_size_gb),
                );
                storage.enabled = s.auto_cleanup_enabled;
                let mut folder = row("folder", "Settings_LibraryFolder", 7);
                folder.display = text(&s.library_path).into();
                vec![
                    (
                        "Settings_Group_LibraryDisk".into(),
                        vec![
                            folder,
                            toggle(
                                "cleanup",
                                "Settings_AutoCleanup",
                                "Settings_AutoCleanup_Desc",
                                s.auto_cleanup_enabled,
                            ),
                            storage,
                        ],
                    ),
                    ("Settings_Group_Cloud".into(), super::cloud::rows(&self.cloud)),
                ]
            }
            5 => {
                let mut theme = choice(
                    "theme",
                    "Settings_ColorTheme",
                    1,
                    simple_options(&["Theme_Light", "Theme_Dark", "Theme_System", "Theme_Neon"]),
                    pos(&["light", "dark", "system", "neon"], &text(&s.theme)),
                );
                theme.description = "Settings_Theme_Desc".into();
                // the interface theme (docs/spec/themes.md): Lag's own + the installed ones
                let themes = crate::themes::list();
                let chosen = match text(&s.ui_theme) {
                    "" => crate::themes::BUILTIN,
                    id => id,
                };
                // two themes of the same name (imported twice, made before 2.2 named them apart) show their folders
                let same = |t: &crate::themes::ThemeInfo| themes.iter().filter(|o| o.name.eq_ignore_ascii_case(&t.name)).count() > 1;
                let names = themes
                    .iter()
                    .map(|t| {
                        let name = if same(t) && !t.builtin { format!("{} ({})", t.name, t.id) } else { t.name.clone() };
                        if t.builtin {
                            tr("Theme_Builtin")
                        } else if t.author.is_empty() {
                            name
                        } else {
                            format!("{name} · {}", t.author)
                        }
                    })
                    .collect();
                let mut ui_theme = combo("ui-theme", "Settings_UiThemeRow", names, themes.iter().position(|t| t.id == chosen).unwrap_or(0));
                let pending = chosen != crate::themes::active();
                ui_theme.description = match crate::themes::last_error() {
                    Some(e) if pending => i18n::format("Theme_Failed", &[&e]).unwrap_or(e),
                    _ if pending => "Settings_UiTheme_Restart".into(),
                    _ => "Settings_UiTheme_Desc".into(),
                }
                .into();
                ui_theme.auto_mode = pending;
                // a chosen theme waits for a restart: the button sits next to the list
                ui_theme.custom = pending;
                // "Your themes" (owner, 2026-11-23: which theme a button acts on must be plain): a new theme with
                // its name typed in; every theme of the user its own row with its own buttons (kind 19); sharing
                let note = |r: &mut SettingsRowData, id: &str| {
                    if self.theme_note_at == id {
                        r.display = self.theme_note.clone().into();
                    }
                };
                let mut new_row = row("theme-new", "Theme_NewRow", 20);
                new_row.description = "Theme_OwnRow_Desc".into();
                new_row.display = self.theme_new_name.clone().into();
                // the note of the last action on a theme that is gone shows here
                if self.theme_note_at == "theme-own" {
                    new_row.parts = model(vec![self.theme_note.clone().into()]);
                }
                let mut own = vec![new_row];
                for t in themes.iter().filter(|t| !t.builtin) {
                    let id = format!("theme:{}", t.id);
                    let mut r = row(&id, "", 19);
                    let file = t.location().file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                    // label: the name; description: the file it is (or the folder of a theme made before 2.2)
                    r.label = if same(t) { format!("{} ({})", t.name, t.id) } else { t.name.clone() }.into();
                    let place = if t.file.is_some() { file } else { i18n::format("Theme_OldFolder", &[&file]).unwrap_or(file) };
                    r.checked = t.id == chosen;
                    r.auto_mode = t.id == crate::themes::active();
                    // in use now / chosen, waiting for a restart
                    let state = if r.auto_mode {
                        tr("Theme_InUse")
                    } else if r.checked {
                        tr("Theme_AfterRestart")
                    } else {
                        String::new()
                    };
                    r.description = if state.is_empty() { place } else { format!("{place} · {state}") }.into();
                    // a theme made before 2.2 offers to become one file
                    r.custom = t.file.is_none();
                    // the note of the last action on this theme (theme_note_at "theme", theme_note_for its id)
                    if self.theme_note_at == "theme" && self.theme_note_for == t.id {
                        r.display = self.theme_note.clone().into();
                    }
                    own.push(r);
                }
                let mut share = row("theme-share", "Theme_ShareRow", 18);
                share.description = "Theme_ShareRow_Desc".into();
                note(&mut share, "theme-share");
                vec![("".into(), vec![theme, ui_theme]), ("Settings_Group_OwnThemes".into(), own), ("".into(), vec![share])]
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
                // the group is already called "About": the row is the app itself
                let mut about = row("updates", "Lag", 8);
                about.enabled = self.update_status != tr("Update_Checking");
                about.display = if self.update_status.is_empty() {
                    format!("{} {}", tr("About_Version"), env!("CARGO_PKG_VERSION"))
                } else {
                    self.update_status.clone()
                }
                .into();
                vec![
                    (
                        "".into(),
                        vec![
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
                    ("Settings_Group_About".into(), vec![about, row("logs", "Settings_Logs", 9)]),
                ]
            }
        }
    }
    pub(super) fn refresh(&self, ui: &AppWindow) {
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
    pub(super) fn maybe_warn(&self, ui: &AppWindow) {
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
