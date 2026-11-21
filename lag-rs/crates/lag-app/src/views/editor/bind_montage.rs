//! The montage strip's controls: select a segment, drag it to a new place, razor (split at the playhead), remove,
//! play the whole montage, add a clip from the sidebar or from a file.
use super::import::ask_for_video;
use super::runtime::EditorRuntime;
use crate::{AppWindow, EditorVM};
use crate::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn bind_montage(ui: &AppWindow, state: &Rc<RefCell<EditorRuntime>>) {
    let vm = ui.global::<EditorVM>();
    {
        let st = state.clone();
        vm.on_select_segment(move |i| {
            let mut s = st.borrow_mut();
            if i >= 0 && i as usize != s.montage.selected() {
                s.play_through = false;
                s.select_segment(i as usize, false);
            }
        });
    }
    {
        let st = state.clone();
        vm.on_segment_dropped(move |i, dx, strip_w| {
            if i >= 0 {
                st.borrow_mut().drop_segment(i as usize, dx as f64, strip_w as f64);
            }
        });
    }
    {
        let st = state.clone();
        vm.on_strip_resized(move |w| {
            if let Ok(mut s) = st.try_borrow_mut() {
                s.strip_resized(w as f64);
            }
        });
    }
    {
        let st = state.clone();
        vm.on_split_segment(move || st.borrow_mut().split_at_playhead());
    }
    {
        let st = state.clone();
        vm.on_remove_segment(move || st.borrow_mut().remove_selected_segment());
    }
    {
        let st = state.clone();
        vm.on_play_montage(move || st.borrow_mut().play_montage_from_start());
    }
    {
        let st = state.clone();
        vm.on_add_to_montage(move |path| {
            if !path.is_empty() {
                st.borrow_mut().add_clips(vec![path.to_string()], false);
            }
        });
    }
    {
        let st = state.clone();
        let weak = ui.as_weak();
        vm.on_add_montage_file(move || {
            let Some(ui) = weak.upgrade() else { return };
            let st = st.clone();
            ask_for_video(&ui, move |path| st.borrow_mut().add_clips(vec![path], false));
        });
    }
}
