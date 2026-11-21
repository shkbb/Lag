//! Captions.
use super::{model::*, runtime::*, tr};
use crate::{AppWindow, EditorVM};
use crate::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn bind_text(ui: &AppWindow, state: &Rc<RefCell<EditorRuntime>>) {
    let vm = ui.global::<EditorVM>();
    // Text / Captions
    {
        let st = state.clone();
        vm.on_add_text(move || {
            let mut s = st.borrow_mut();
            s.model.push_undo();
            // each new caption a step lower than the last, so several never sit on top of each other
            // (owner, 2026-11-17: "you cannot add several texts" - they were all stacked at one spot)
            let mut item = TextItem::new(tr("Editor_TextDefault"));
            item.ny = (0.12 + 0.14 * s.model.texts.len() as f64) % 0.98;
            s.model.texts.push(item);
            s.model.selected_text_index = Some(s.model.texts.len() - 1);
            if let Some(ui) = s.ui.upgrade() {
                s.sync_texts_to_ui(&ui.global::<EditorVM>());
            }
        });
    }
    {
        let st = state.clone();
        vm.on_select_text(move |idx| {
            let mut s = st.borrow_mut();
            if idx < 0 {
                // click on the video: the caption selection goes away
                if s.model.selected_text_index.take().is_some() {
                    if let Some(ui) = s.ui.upgrade() {
                        let vm = ui.global::<EditorVM>();
                        vm.set_editing_text_index(-1);
                        s.sync_texts_to_ui(&vm);
                    }
                }
                return;
            }
            if (idx as usize) < s.model.texts.len() {
                // picking a caption ends the crop/reframe selection (owner: "the previous action is not cancelled")
                if s.model.crop_tool_active {
                    s.model.crop_tool_active = false;
                    s.apply_preview_vf();
                }
                s.model.selected_text_index = Some(idx as usize);
                if let Some(ui) = s.ui.upgrade() {
                    let vm = ui.global::<EditorVM>();
                    vm.set_reframe_selected(false);
                    vm.set_crop_tool_active(false);
                    s.sync_overlay_to_ui(&vm);
                    vm.set_selected_tools_tab(2);
                    s.sync_texts_to_ui(&vm);
                }
            }
        });
    }
    {
        let st = state.clone();
        vm.on_update_selected_text(move |txt| {
            let mut s = st.borrow_mut();
            if let Some(i) = s.model.selected_text_index {
                if let Some(item) = s.model.texts.get_mut(i) {
                    item.text = txt.to_string();
                }
                if let Some(ui) = s.ui.upgrade() {
                    s.sync_texts_to_ui(&ui.global::<EditorVM>());
                }
            }
        });
    }
    {
        let st = state.clone();
        vm.on_update_selected_text_size(move |pct| {
            let mut s = st.borrow_mut();
            if let Some(i) = s.model.selected_text_index {
                if let Some(item) = s.model.texts.get_mut(i) {
                    item.font_percent = pct.clamp(3, 25);
                }
                if let Some(ui) = s.ui.upgrade() {
                    s.sync_texts_to_ui(&ui.global::<EditorVM>());
                }
            }
        });
    }
    {
        let st = state.clone();
        vm.on_set_text_color(move |hex| {
            let mut s = st.borrow_mut();
            if let Some(i) = s.model.selected_text_index {
                if let Some(item) = s.model.texts.get_mut(i) {
                    item.color = hex.to_string();
                }
                if let Some(ui) = s.ui.upgrade() {
                    s.sync_texts_to_ui(&ui.global::<EditorVM>());
                }
            }
        });
    }
    {
        let st = state.clone();
        vm.on_move_text(move |idx, nx, ny| {
            let mut s = st.borrow_mut();
            if let Some(item) = s.model.texts.get_mut(idx as usize) {
                item.nx = (nx as f64).clamp(0.0, 1.0);
                item.ny = (ny as f64).clamp(0.0, 1.0);
            }
            if let Some(ui) = s.ui.upgrade() {
                s.sync_texts_to_ui(&ui.global::<EditorVM>());
            }
        });
    }
    {
        let st = state.clone();
        vm.on_remove_selected_text(move || {
            let mut s = st.borrow_mut();
            if let Some(i) = s.model.selected_text_index.take() {
                if i < s.model.texts.len() {
                    s.model.push_undo();
                    s.model.texts.remove(i);
                }
                if let Some(ui) = s.ui.upgrade() {
                    s.sync_texts_to_ui(&ui.global::<EditorVM>());
                }
            }
        });
    }
}
