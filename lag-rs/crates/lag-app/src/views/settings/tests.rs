use super::{appearance::*, audio_monitor::*, devices::*, edit::*, normalize::*, rows::*, state::*};
use lag_core::{
    hardware::{MachineProfile, MicrophoneInfo, MonitorInfo},
    i18n,
    presets::{self, RecommendedSettings},
    settings::{AppAudioSetting, SettingsData},
};
use slint::VecModel;
use std::{rc::Rc, time::Instant};

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
        cloud: Default::default(),
        theme_note: String::new(),
        theme_note_at: "",
    }
}
#[test]
fn preset_falls_back_to_selected_monitor_options() {
    let mut st = state_with_hardware(SettingsData { monitor_stable_id: Some("panel-B".into()), ..Default::default() });
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
