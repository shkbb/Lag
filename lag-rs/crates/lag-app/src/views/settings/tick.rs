//! The settings page's 33 ms tick: hardware probe result, audio devices (hot-plug), the microphone meter,
//! running apps for per-app audio, the startup update check.

use super::*;

pub(super) fn run(
    ui: &AppWindow,
    ctx: &Rc<AppCtx>,
    state: &Rc<RefCell<State>>,
    hardware_recv: mpsc::Receiver<Hardware>,
) {
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
