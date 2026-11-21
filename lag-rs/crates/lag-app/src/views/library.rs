//! Library page logic (ViewModels/LibraryViewModel.cs, Views/LibraryView.axaml.cs, Models/ReplayClip.cs).
//! Spec: docs/spec/library-player.md.
//!
//! Modules: `clip` (a clip entry, file types, paths), `scan` (folder scan + thumbnail memory cache), `thumbnails`
//! (FFmpeg thumbnails, background hydration), `filter` (search + game chips), `grouping` (day groups), `layout`
//! (virtualised card grid), `storage` (quota + disk usage), `os` (Explorer, locale, dates), `state`
//! (`LibraryState`, UI-thread). This file wires the page (`init`).

use crate::app_ctx::AppCtx;
use crate::LibraryVM;
use lag_core::settings::SettingsData;
use crate::ComponentHandle;
use slint::{ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::rc::Rc;

mod clip;
mod cloud;
mod cloud_actions;
mod filter;
mod grouping;
mod layout;
mod os;
mod scan;
mod state;
mod storage;
#[cfg(test)]
mod tests;
mod thumbnails;

use clip::*;
use os::*;
use state::*;
use storage::*;

/// Splits the configured save-replay hotkey into keycap strings (SettingsViewModel.cs:151-152, 1682-1700).
pub fn hotkey_parts_from_settings(settings: &SettingsData) -> Vec<String> {
    let key = settings.hotkey_key.as_deref().unwrap_or("VcF10");
    if key == "VcUndefined" || key.is_empty() {
        return vec!["—".to_string()];
    }
    let mods = settings.hotkey_modifiers.as_deref().unwrap_or("LeftAlt");
    let mut parts = Vec::new();
    if contains_ignore_case(mods, "Ctrl") {
        parts.push("Ctrl".to_string());
    }
    if contains_ignore_case(mods, "Alt") {
        parts.push("Alt".to_string());
    }
    if contains_ignore_case(mods, "Shift") {
        parts.push("Shift".to_string());
    }
    parts.push(key.replace("Vc", ""));
    parts
}

pub fn init(ui: &crate::AppWindow, ctx: &Rc<AppCtx>) {
    let state = Rc::new(RefCell::new(LibraryState::new(ui, Rc::clone(ctx))));
    register_state(&state);

    // Sync initial hotkey keycaps
    let initial_settings = ctx.settings();
    sync_hotkey_parts(ui, &initial_settings);

    // Startup background quota check + initial free-space read (LibraryViewModel.cs:340-344)
    {
        let lib_dir = resolve_library_dir(&initial_settings);
        let auto_cleanup = initial_settings.auto_cleanup_enabled;
        let max_gb = initial_settings.max_library_size_gb;
        let thumb_dir = state.borrow().thumb_cache_dir.clone();
        let meta_store = state.borrow().meta_store.clone();
        let ui_weak = ui.as_weak();
        let _ = std::thread::Builder::new().name("lag-lib-startup".into()).spawn(move || {
            enforce_storage_quota(&lib_dir, auto_cleanup, max_gb, meta_store.as_ref(), &thumb_dir);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(ui) = ui_weak.upgrade() {
                    LibraryState::update_free_space(&ui, &lib_dir);
                }
            });
        });
    }

    let vm = ui.global::<LibraryVM>();

    // Search text changed (immediate, no debounce)
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_search_changed(move |text| {
            let Some(ui) = ui_w.upgrade() else { return };
            let mut st = st.borrow_mut();
            st.search_text = text.to_string();
            st.clear_selection(&ui);
            st.rebuild_groups(&ui);
        });
    }

    // Category chip toggle
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_set_game_filter(move |key| {
            let Some(ui) = ui_w.upgrade() else { return };
            let mut st = st.borrow_mut();
            let key_str = key.as_str();
            let same = st.game_filter.as_deref().is_some_and(|cur| eq_ignore_case(cur, key_str));
            st.game_filter = if same { None } else { Some(key_str.to_string()) };
            st.rebuild_game_chips(&ui);
            st.clear_selection(&ui);
            st.rebuild_groups(&ui);
        });
    }

    // Favorites toggle
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_toggle_favorites_only(move || {
            let Some(ui) = ui_w.upgrade() else { return };
            let mut st = st.borrow_mut();
            st.favorites_only = !st.favorites_only;
            ui.global::<LibraryVM>().set_favorites_only(st.favorites_only);
            st.clear_selection(&ui);
            st.rebuild_groups(&ui);
        });
    }

    // Open library folder
    {
        let ctx = Rc::clone(ctx);
        vm.on_open_library_folder(move || {
            let dir = resolve_library_dir(&ctx.settings());
            open_folder_shell(&dir);
        });
    }

    // Clear/select-all (Delete and the cloud actions: cloud_actions.rs)
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_clear_selection(move || {
            let Some(ui) = ui_w.upgrade() else { return };
            st.borrow_mut().clear_selection(&ui);
        });
    }
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_select_all(move || {
            let Some(ui) = ui_w.upgrade() else { return };
            st.borrow_mut().select_all(&ui);
        });
    }

    // Card interactions
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_card_pressed(move |clip_idx, shift, ctrl| {
            let Some(ui) = ui_w.upgrade() else { return };
            let idx = clip_idx as usize;
            let play_path: Option<String> = {
                let mut st = st.borrow_mut();
                if idx >= st.clips.len() {
                    return;
                }
                let has_selection = st.clips.iter().any(|c| c.is_selected);
                if shift {
                    st.select_range(&ui, idx);
                    None
                } else if ctrl || has_selection {
                    st.toggle_select_clip(&ui, idx);
                    None
                } else {
                    Some(st.clips[idx].file_path.clone())
                }
            };
            if let Some(path) = play_path {
                st.borrow().ctx.play_clip(&path);
            }
        });
    }
    {
        let st = Rc::clone(&state);
        vm.on_card_play(move |clip_idx| {
            let path = st.borrow().clips.get(clip_idx as usize).map(|c| c.file_path.clone());
            if let Some(p) = path {
                st.borrow().ctx.play_clip(&p);
            }
        });
    }
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_card_favorite(move |clip_idx| {
            let Some(ui) = ui_w.upgrade() else { return };
            st.borrow_mut().toggle_favorite(&ui, clip_idx as usize);
        });
    }
    {
        let st = Rc::clone(&state);
        vm.on_card_edit(move |clip_idx| {
            let target = st.borrow().clips.get(clip_idx as usize).and_then(|c| {
                // a clip only in the cloud is edited once it is saved to this PC
                if c.is_image || c.is_cloud_only() {
                    None
                } else {
                    Some(c.file_path.clone())
                }
            });
            if let Some(path) = target {
                st.borrow().ctx.edit_clip(&path);
            }
        });
    }
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        vm.on_card_select_circle(move |clip_idx| {
            let Some(ui) = ui_w.upgrade() else { return };
            st.borrow_mut().toggle_select_clip(&ui, clip_idx as usize);
        });
    }

    // Context menu actions
    {
        let st = Rc::clone(&state);
        vm.on_context_edit(move |file_path| {
            let target = {
                let st = st.borrow();
                st.clip_path_to_idx
                    .get(file_path.as_str())
                    .and_then(|&idx| st.clips.get(idx))
                    .filter(|c| !c.is_image)
                    .map(|c| c.file_path.clone())
            };
            if let Some(path) = target {
                st.borrow().ctx.edit_clip(&path);
            }
        });
    }
    {
        vm.on_context_show_in_folder(move |file_path| {
            show_in_folder(file_path.as_str());
        });
    }

    // Virtualization viewport updates
    {
        let st = Rc::clone(&state);
        vm.on_viewport_changed(move |scroll_y, w, h| {
            st.borrow_mut().on_viewport_changed(scroll_y, w, h);
        });
    }

    // Refresh hook
    {
        let st = Rc::clone(&state);
        ctx.on_library_refresh(move || {
            LibraryState::start_refresh(&st);
        });
    }

    // Settings changes (language, hotkeys, library path, quota)
    {
        let st = Rc::clone(&state);
        let ui_w = ui.as_weak();
        let prev_settings = RefCell::new(initial_settings);
        ctx.on_settings_changed(move |new_s| {
            let Some(ui) = ui_w.upgrade() else { return };
            let mut prev = prev_settings.borrow_mut();
            let path_changed = prev.library_path != new_s.library_path;
            let quota_changed = prev.auto_cleanup_enabled != new_s.auto_cleanup_enabled
                || prev.max_library_size_gb != new_s.max_library_size_gb;
            let lang_changed = prev.language != new_s.language;
            let hotkey_changed = prev.hotkey_key != new_s.hotkey_key || prev.hotkey_modifiers != new_s.hotkey_modifiers;
            *prev = new_s.clone();
            drop(prev);

            if hotkey_changed {
                sync_hotkey_parts(&ui, new_s);
            }
            if path_changed || quota_changed {
                LibraryState::start_refresh(&st);
            } else if lang_changed {
                let mut guard = st.borrow_mut();
                guard.rebuild_view(&ui);
                guard.notify_selection_changed(&ui);
            }
        });
    }

    cloud_actions::bind(ui, ctx, &state);

    // Initial background scan at startup (MainViewModel.cs:231)
    LibraryState::start_refresh(&state);
}

fn sync_hotkey_parts(ui: &crate::AppWindow, settings: &SettingsData) {
    let parts: Vec<SharedString> = hotkey_parts_from_settings(settings).into_iter().map(SharedString::from).collect();
    ui.global::<LibraryVM>().set_hotkey_parts(ModelRc::from(Rc::new(VecModel::from(parts))));
}
