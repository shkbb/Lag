//! Export options, export and cancel.
use super::montage_export::{montage_output_path, spawn_montage_export};
use super::{export::*, model::*, runtime::*, tr_fmt};
use crate::{AppWindow, EditorVM};
use crate::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub(super) fn bind_export(ui: &AppWindow, state: &Rc<RefCell<EditorRuntime>>) {
    let vm = ui.global::<EditorVM>();
    // Export
    {
        let st = state.clone();
        vm.on_select_export_resolution(move |idx| {
            let mut s = st.borrow_mut();
            s.model.selected_export_res_index = (idx as usize).min(s.model.export_resolutions.len().saturating_sub(1));
        });
    }
    {
        let st = state.clone();
        vm.on_select_export_format(move |idx| {
            let mut s = st.borrow_mut();
            s.model.selected_export_format_index = (idx as usize).min(EXPORT_FORMATS.len().saturating_sub(1));
        });
    }
    {
        let st = state.clone();
        vm.on_set_export_gif(move |gif| {
            st.borrow_mut().model.export_as_gif = gif;
        });
    }
    {
        let st = state.clone();
        vm.on_export_clip(move || {
            let mut s = st.borrow_mut();
            if s.is_exporting {
                return;
            }
            let Some(clip_path) = s.model.current_clip_path.clone() else {
                return;
            };
            s.is_exporting = true;
            s.export_gen = s.export_gen.wrapping_add(1);
            let cancel = Arc::new(AtomicBool::new(false));
            s.export_cancel = Some(cancel.clone());
            let montage = s.montage.is_montage();
            // a montage is one video (no GIF: the export menu hides it while a montage is open)
            let ext = if s.model.export_as_gif && !montage { "gif" } else { s.model.selected_export_format() };
            if let Some(ui) = s.ui.upgrade() {
                let vm = ui.global::<EditorVM>();
                vm.set_is_exporting(true);
                vm.set_export_progress(0);
                vm.set_export_status(tr_fmt("Editor_Exporting", "0").into());
            }
            if montage {
                let live = s.model.clone();
                s.montage.commit(&live);
                let out_path = montage_output_path(&s.montage, ext);
                let res = s.model.selected_export_res().height;
                spawn_montage_export(
                    s.export_gen,
                    s.montage.clone(),
                    res,
                    out_path,
                    cancel,
                    s.export_child.clone(),
                    s.messages_tx.clone(),
                );
            } else {
                let out_path = build_output_path(&clip_path, ext);
                spawn_export(
                    s.export_gen,
                    s.model.clone(),
                    clip_path,
                    out_path,
                    cancel,
                    s.export_child.clone(),
                    s.messages_tx.clone(),
                );
            }
        });
    }
    {
        let st = state.clone();
        vm.on_cancel_export(move || {
            let s = st.borrow();
            if let Some(c) = &s.export_cancel {
                c.store(true, Ordering::Release);
            }
            if let Ok(mut g) = s.export_child.lock() {
                if let Some(ch) = g.as_mut() {
                    let _ = ch.kill();
                }
            };
        });
    }
}
