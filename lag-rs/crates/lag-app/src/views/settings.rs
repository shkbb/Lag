//! Mirrors Views/SettingsView.axaml(.cs), ViewModels/SettingsViewModel.cs,
//! SettingsViewModel.Presets.cs, SettingsViewModel.AudioMeter.cs and Controls/SegmentedListBox.cs.
//! All persisted mutations go through AppCtx::update_settings. Hardware and disk work stay off UI.
//!
//! Modules: `devices` (monitors, encoders, devices probed off the UI thread), `normalize` (repairing a loaded
//! settings file against the hardware), `rows` (row builders), `state` (the page state and the rows of every
//! tab), `edit` (what each control changes, presets, restart-required), `appearance` (theme, language flags),
//! `audio_monitor` (input level meter / hear yourself), `os` (folder dialog, Explorer, diagnostics zip),
//! `cloud` (the cloud rows of the Library tab and what they change).
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
mod bind;
mod cloud;
mod devices;
mod edit;
mod normalize;
mod os;
mod rows;
mod state;
#[cfg(test)]
mod tests;
mod tick;

use appearance::*;
use audio_monitor::*;
use bind::*;
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
    let state = Rc::new(RefCell::new(State::new(initial.clone(), groups)));
    state.borrow_mut().cloud = ctx.cloud().view();
    let (st, weak) = (state.clone(), ui.as_weak());
    ctx.cloud().on_changed(move |view| {
        if let Some(ui) = weak.upgrade() {
            let mut st = st.borrow_mut();
            st.cloud = view.clone();
            st.refresh(&ui);
        }
    });
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
    bind_controls(ui, ctx, &state);
    tick::run(ui, ctx, &state, hardware_recv);
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
