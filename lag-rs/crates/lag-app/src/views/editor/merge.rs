//! Picking clips for a montage in the clip sidebar (owner, 2026-11-18): "Montage" switches the sidebar to picking
//! mode, clips are picked in order (their number shows on the card), and "Montage (N)" opens them as a montage
//! (montage.rs) - every clip then trimmed, cut, reframed and captioned on its own before one export.

use super::EditorRuntime;
use crate::services::clip_merge;
use crate::{AppWindow, EditorClipItem, EditorVM};
use crate::ComponentHandle;
use slint::Model;
use std::cell::RefCell;
use std::rc::Rc;

/// Clips picked for the montage, in the order they were picked.
#[derive(Debug, Default)]
pub(super) struct MergePicks {
    paths: Vec<String>,
}

impl MergePicks {
    /// Adds the clip at the end, or removes it when it was already picked (the rest keep their order).
    pub fn toggle(&mut self, path: &str) {
        match self.paths.iter().position(|p| p == path) {
            Some(i) => {
                self.paths.remove(i);
            }
            None => self.paths.push(path.to_string()),
        }
    }

    pub fn clear(&mut self) {
        self.paths.clear();
    }

    /// 1-based position for the card badge, 0 = not picked.
    pub fn number_of(&self, path: &str) -> i32 {
        self.paths.iter().position(|p| p == path).map_or(0, |i| i as i32 + 1)
    }

    pub fn len(&self) -> usize {
        self.paths.len()
    }

    pub fn ready(&self) -> bool {
        self.paths.len() >= clip_merge::MIN_CLIPS
    }

    pub fn paths(&self) -> &[String] {
        &self.paths
    }
}

/// Card badges + the "Merge (N)" count. Rows are updated in place (a fresh model would recreate every card and
/// drop the thumbnails mid-scroll).
pub(super) fn sync_to_ui(vm: &EditorVM, picks: &MergePicks) {
    let clips = vm.get_clips();
    for i in 0..clips.row_count() {
        let Some(mut row): Option<EditorClipItem> = clips.row_data(i) else { continue };
        let n = picks.number_of(&row.path);
        if row.merge_number != n {
            row.merge_number = n;
            clips.set_row_data(i, row);
        }
    }
    vm.set_merge_count(picks.len() as i32);
}

pub(super) fn wire(ui: &AppWindow, state: &Rc<RefCell<EditorRuntime>>) {
    let vm = ui.global::<EditorVM>();
    {
        let st = state.clone();
        vm.on_toggle_merge_mode(move || {
            let mut s = st.borrow_mut();
            let Some(ui) = s.ui.upgrade() else { return };
            let vm = ui.global::<EditorVM>();
            vm.set_merge_mode(!vm.get_merge_mode());
            s.merge.clear();
            sync_to_ui(&vm, &s.merge);
        });
    }
    {
        let st = state.clone();
        vm.on_merge_pick(move |path| {
            let mut s = st.borrow_mut();
            s.merge.toggle(&path);
            if let Some(ui) = s.ui.upgrade() {
                sync_to_ui(&ui.global::<EditorVM>(), &s.merge);
            }
        });
    }
    {
        let st = state.clone();
        vm.on_merge_clips(move || start(&mut st.borrow_mut()));
    }
}

/// "Montage (N)": the picked clips become a new montage (probed on a worker thread first).
fn start(s: &mut EditorRuntime) {
    if s.is_exporting || !s.merge.ready() {
        return;
    }
    let paths = s.merge.paths().to_vec();
    s.add_clips(paths, true);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn picks_keep_their_order_and_toggle_off() {
        let mut m = MergePicks::default();
        m.toggle("a");
        m.toggle("b");
        m.toggle("c");
        assert_eq!((m.number_of("a"), m.number_of("b"), m.number_of("c"), m.number_of("x")), (1, 2, 3, 0));
        m.toggle("b"); // un-pick: the others close the gap, order kept
        assert_eq!(m.paths(), ["a", "c"]);
        assert_eq!(m.number_of("c"), 2);
        assert!(m.ready());
        m.toggle("a");
        assert!(!m.ready());
    }
}
