//! Frame shape, crop/reframe, rotation/flips, speed, filters and colour.
use super::{export::*, geometry::*, runtime::*};
use crate::{AppWindow, EditorVM};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

pub(super) fn bind_frame(ui: &AppWindow, state: &Rc<RefCell<EditorRuntime>>) {
    let vm = ui.global::<EditorVM>();
    // Aspect, Crop, Reframe, Rotate/Flip, Speed
    {
        let st = state.clone();
        vm.on_select_aspect(move |idx| {
            let mut s = st.borrow_mut();
            let idx = (idx as usize).min(s.model.aspect_options.len().saturating_sub(1));
            if s.model.selected_aspect_index != idx {
                s.model.push_undo();
                s.model.selected_aspect_index = idx;
                s.model.reset_reframe();
            }
            if let Some(ui) = s.ui.upgrade() {
                s.sync_overlay_to_ui(&ui.global::<EditorVM>());
            }
        });
    }
    {
        let st = state.clone();
        vm.on_toggle_crop_tool(move || {
            let mut s = st.borrow_mut();
            s.model.crop_tool_active = !s.model.crop_tool_active;
            s.apply_preview_vf();
            if let Some(ui) = s.ui.upgrade() {
                let vm = ui.global::<EditorVM>();
                vm.set_crop_tool_active(s.model.crop_tool_active);
                s.sync_overlay_to_ui(&vm);
            }
        });
    }
    {
        let st = state.clone();
        vm.on_right_click_reset(move || {
            let mut s = st.borrow_mut();
            if s.model.crop_tool_active {
                s.model.reset_crop();
            } else {
                s.model.reset_reframe();
            }
            s.apply_preview_vf();
            if let Some(ui) = s.ui.upgrade() {
                s.sync_overlay_to_ui(&ui.global::<EditorVM>());
            }
        });
    }
    {
        let st = state.clone();
        vm.on_begin_overlay_drag(move || {
            let mut s = st.borrow_mut();
            s.drag_start_disp = s.model.display_edit_norm();
        });
    }
    {
        let st = state.clone();
        vm.on_drag_overlay(move |mode, dx, dy, rx, ry, rw, rh, sw, sh| {
            let mut s = st.borrow_mut();
            let start_disp = s.drag_start_disp;
            apply_overlay_drag(
                &mut s.model,
                start_disp,
                mode,
                dx as f64,
                dy as f64,
                (rx as f64, ry as f64, rw as f64, rh as f64),
                (sw as f64, sh as f64),
            );
            if let Some(ui) = s.ui.upgrade() {
                s.sync_overlay_to_ui(&ui.global::<EditorVM>());
            }
        });
    }
    {
        let st = state.clone();
        vm.on_rotate_left(move || {
            let mut s = st.borrow_mut();
            s.model.push_undo();
            s.model.rotation = (s.model.rotation + 270) % 360;
            s.apply_preview_vf();
            if let Some(ui) = s.ui.upgrade() {
                s.sync_overlay_to_ui(&ui.global::<EditorVM>());
            }
        });
    }
    {
        let st = state.clone();
        vm.on_rotate_right(move || {
            let mut s = st.borrow_mut();
            s.model.push_undo();
            s.model.rotation = (s.model.rotation + 90) % 360;
            s.apply_preview_vf();
            if let Some(ui) = s.ui.upgrade() {
                s.sync_overlay_to_ui(&ui.global::<EditorVM>());
            }
        });
    }
    {
        let st = state.clone();
        vm.on_toggle_flip_h(move || {
            let mut s = st.borrow_mut();
            s.model.push_undo();
            s.model.flip_h = !s.model.flip_h;
            s.apply_preview_vf();
            if let Some(ui) = s.ui.upgrade() {
                s.sync_overlay_to_ui(&ui.global::<EditorVM>());
            }
        });
    }
    {
        let st = state.clone();
        vm.on_toggle_flip_v(move || {
            let mut s = st.borrow_mut();
            s.model.push_undo();
            s.model.flip_v = !s.model.flip_v;
            s.apply_preview_vf();
            if let Some(ui) = s.ui.upgrade() {
                s.sync_overlay_to_ui(&ui.global::<EditorVM>());
            }
        });
    }
    {
        let st = state.clone();
        vm.on_select_speed_preset(move |idx| {
            let mut s = st.borrow_mut();
            let idx = idx as usize;
            if let Some(opt) = s.model.speed_options.get(idx).cloned() {
                s.model.push_undo();
                let txt = fmt_dec(opt.value, 2);
                s.model.set_speed_text(txt);
                s.apply_preview_rate();
                if let Some(ui) = s.ui.upgrade() {
                    let vm = ui.global::<EditorVM>();
                    vm.set_selected_speed_index(idx as i32);
                    vm.set_speed_text(s.model.speed_text.as_str().into());
                }
            }
        });
    }
    {
        let st = state.clone();
        vm.on_speed_text_edited(move |txt| {
            let mut s = st.borrow_mut();
            s.model.set_speed_text(txt.to_string());
            s.apply_preview_rate();
            if let Some(ui) = s.ui.upgrade() {
                ui.global::<EditorVM>()
                    .set_selected_speed_index(s.model.selected_speed_index.map(|i| i as i32).unwrap_or(-1));
            }
        });
    }

    // Filters & Color
    {
        let st = state.clone();
        vm.on_select_filter(move |idx| {
            let mut s = st.borrow_mut();
            let idx = (idx as usize).min(s.model.filter_options.len().saturating_sub(1));
            if s.model.selected_filter_index != idx {
                s.model.push_undo();
                s.model.selected_filter_index = idx;
                s.apply_preview_vf();
                if let Some(ui) = s.ui.upgrade() {
                    ui.global::<EditorVM>().set_selected_filter_index(idx as i32);
                }
            }
        });
    }
    {
        let st = state.clone();
        vm.on_color_changed(move |b, c, sat| {
            let mut s = st.borrow_mut();
            let now = Instant::now();
            let need_snap =
                s.last_color_undo_at.is_none_or(|t| now.saturating_duration_since(t) > Duration::from_millis(500));
            if need_snap {
                s.model.push_undo();
            }
            s.last_color_undo_at = Some(now);
            s.model.color_brightness = b.clamp(0, 200);
            s.model.color_contrast = c.clamp(0, 200);
            s.model.color_saturation = sat.clamp(0, 200);
            s.apply_preview_vf();
            if let Some(ui) = s.ui.upgrade() {
                let vm = ui.global::<EditorVM>();
                vm.set_color_brightness(s.model.color_brightness);
                vm.set_color_contrast(s.model.color_contrast);
                vm.set_color_saturation(s.model.color_saturation);
            }
        });
    }
    {
        let st = state.clone();
        vm.on_reset_color(move || {
            let mut s = st.borrow_mut();
            s.model.push_undo();
            s.last_color_undo_at = None;
            s.model.color_brightness = 100;
            s.model.color_contrast = 100;
            s.model.color_saturation = 100;
            s.apply_preview_vf();
            if let Some(ui) = s.ui.upgrade() {
                let vm = ui.global::<EditorVM>();
                vm.set_color_brightness(100);
                vm.set_color_contrast(100);
                vm.set_color_saturation(100);
            }
        });
    }
}
