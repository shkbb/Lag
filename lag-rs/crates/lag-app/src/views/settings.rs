//! Mirrors Views/SettingsView.axaml(.cs), ViewModels/SettingsViewModel.cs,
//! SettingsViewModel.Presets.cs, SettingsViewModel.AudioMeter.cs and Controls/SegmentedListBox.cs.
//! All persisted mutations go through AppCtx::update_settings. Hardware and disk work stay off UI.
//!
//! Modules: `devices` (monitors, encoders, devices probed off the UI thread), `normalize` (repairing a loaded
//! settings file against the hardware), `rows` (row builders), `state` (the page state and the rows of every
//! tab), `edit` (what each control changes, presets, restart-required), `appearance` (theme, language flags),
//! `audio_monitor` (input level meter / hear yourself), `os` (folder dialog, Explorer, diagnostics zip).
//! This file wires the page (`init`) and hotkey capture.

use crate::{app_ctx::AppCtx, AppWindow, MainVM, Page, SettingsVM};
use lag_core::{
    hardware::{self, MicrophoneInfo},
    i18n,
    presets::{self},
};
use slint::{ComponentHandle, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::mpsc,
    time::{Duration, Instant},
};

mod appearance;
mod audio_monitor;
mod devices;
mod edit;
mod normalize;
mod os;
mod rows;
mod state;
#[cfg(test)]
mod tests;

use appearance::*;
use audio_monitor::*;
use devices::*;
use edit::*;
use normalize::*;
use os::*;
use rows::*;
use state::*;

fn tr(key: &str) -> String {
    i18n::tr(key).into()
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
