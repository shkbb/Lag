//! The settings page's controls: tabs, choices, toggles, numbers, overlay boxes, presets, hotkey capture,
//! buttons (folder, logs, diagnostics, updates).

use super::*;

pub(super) fn bind_controls(ui: &AppWindow, ctx: &Rc<AppCtx>, state: &Rc<RefCell<State>>) {
    let (st, c, weak) = (state.clone(), ctx.clone(), ui.as_weak());
    ui.global::<SettingsVM>().on_select_tab(move |tab| {
        if let Some(ui) = weak.upgrade() {
            let mut st = st.borrow_mut();
            st.tab = tab.clamp(0, 4);
            ui.global::<SettingsVM>().set_tab(st.tab);
            ui.global::<SettingsVM>().set_hear_yourself(false);
            crate::shell::hotkeys::cancel();
            st.capture.clear();
            ui.global::<SettingsVM>().set_capture_id("".into());
            st.refresh(&ui);
            if st.tab == 3 {
                c.cloud().refresh();
            }
        }
    });
    let (st, c, weak) = (state.clone(), ctx.clone(), ui.as_weak());
    ui.global::<SettingsVM>().on_choose(move |id, index| {
        if let Some(ui) = weak.upgrade() {
            if index >= 0 && id.starts_with("cloud-") {
                c.cloud().edit(|cs| super::cloud::choose(cs, &id, index as usize));
            } else if index >= 0 {
                save(&ui, &c, &st, &id, |s| choose(s, &id, index as usize));
            }
        }
    });
    let (st, c, weak) = (state.clone(), ctx.clone(), ui.as_weak());
    ui.global::<SettingsVM>().on_toggle(move |id, value| {
        if id.starts_with("cloud-") {
            c.cloud().edit(|cs| super::cloud::toggle(cs, &id, value));
        } else if let Some(ui) = weak.upgrade() {
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
            "cloud-add-google" => c.cloud().connect(),
            "cloud-add-dropbox" => c.cloud().connect_oauth(lag_cloud::ProviderKind::Dropbox),
            "cloud-add-onedrive" => c.cloud().connect_oauth(lag_cloud::ProviderKind::OneDrive),
            "cloud-cancel" => c.cloud().cancel_connect(),
            "cloud-add-folder" => {
                // a folder picker (a NAS share, a network drive, a synced folder), off the UI thread
                let (sender, receiver) = mpsc::channel();
                std::thread::spawn(move || {
                    let _ = sender.send(browse_folder(""));
                });
                let c = c.clone();
                let timer = Rc::new(slint::Timer::default());
                let keep = timer.clone();
                timer.start(slint::TimerMode::Repeated, Duration::from_millis(50), move || {
                    if let Ok(result) = receiver.try_recv() {
                        keep.stop();
                        if let Some(path) = result {
                            c.cloud().connect_folder(&path);
                        }
                    }
                });
            }
            "cloud-add-webdav" => {
                let vm = ui.global::<SettingsVM>();
                vm.set_webdav_error("".into());
                vm.set_webdav_open(true);
            }
            other if other.starts_with("cloud-disconnect:") => {
                // the page's state is borrowed again by the change listener: take the id out first
                let index = other.rsplit(':').next().and_then(|i| i.parse::<usize>().ok());
                let id = index.and_then(|i| st.borrow().cloud.accounts.get(i).map(|a| a.settings.id.clone()));
                if let Some(id) = id {
                    c.cloud().disconnect(&id);
                }
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
            // "Update failed: {0}" has a slot for the C# exception text; there is none here, so it is dropped
            // (2.0.0 showed the "{0}" itself).
            st.borrow_mut().update_status = if key.is_empty() {
                String::new()
            } else {
                tr(&key).replace("{0}", "").trim_end_matches([':', ' ', '\u{a0}']).to_string()
            };
            st.borrow().refresh(&ui);
        }
    });
}
