//! Editor side of clip merging (owner, 2026-11-18): "Merge" in the clip sidebar switches to picking mode, clips
//! are picked in order (their number shows on the card), "Merge (N)" joins them via `services::clip_merge`
//! on a worker thread, reusing the export's progress / cancel plumbing, and the result opens in the editor so it
//! can be trimmed, reframed to 9:16, captioned and exported like any clip.

use super::{tr_fmt, EditorRuntime, Job, UiMessage};
use crate::services::clip_merge;
use crate::{AppWindow, EditorClipItem, EditorVM};
use slint::{ComponentHandle, Model};
use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Clips picked for the merge, in the order they were picked.
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

fn start(s: &mut EditorRuntime) {
    if s.is_exporting || !s.merge.ready() {
        return;
    }
    let paths = s.merge.paths().to_vec();
    let out = clip_merge::output_path(&paths[0]);
    s.is_exporting = true;
    s.export_gen = s.export_gen.wrapping_add(1);
    s.job = Job::Merge { out: out.clone() };
    let cancel = Arc::new(AtomicBool::new(false));
    s.export_cancel = Some(cancel.clone());
    if let Some(ui) = s.ui.upgrade() {
        let vm = ui.global::<EditorVM>();
        vm.set_is_exporting(true);
        vm.set_export_progress(0);
        vm.set_export_status(tr_fmt("Editor_Merging", "0").into());
    }
    let (gen, child, tx) = (s.export_gen, s.export_child.clone(), s.messages_tx.clone());
    let _ = std::thread::Builder::new().name("Lag.ClipMerge".into()).spawn(move || {
        let progress = |p: f64| {
            let _ = tx.send(UiMessage::ExportProgress(gen, p.clamp(0.0, 100.0) as i32));
        };
        let result = clip_merge::run(&paths, &out, &cancel, &child, &progress);
        let message = match result {
            Ok(()) => Ok(file_name(&out)),
            Err(_) if cancel.load(Ordering::Acquire) => Err(None),
            Err(e) => {
                log::warn!("[ClipMerge] failed: {e}");
                Err(Some(e))
            }
        };
        let _ = tx.send(UiMessage::ExportFinished(gen, message));
    });
}

/// The merged file exists: leave picking mode and open it for editing.
pub(super) fn finished(s: &mut EditorRuntime, vm: &EditorVM, out: &Path) {
    vm.set_merge_mode(false);
    s.merge.clear();
    sync_to_ui(vm, &s.merge);
    let path = out.to_string_lossy().into_owned();
    log::info!("[ClipMerge] done -> {path}");
    s.open_clip(&path, 1.0);
}

/// Progress text of the running merge.
pub(super) fn progress_text(pct: i32) -> String {
    tr_fmt("Editor_Merging", &pct.to_string())
}

fn file_name(p: &Path) -> String {
    p.file_name().and_then(|s| s.to_str()).unwrap_or("").to_string()
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
