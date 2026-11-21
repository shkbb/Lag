//! The library's cloud actions (docs/spec/cloud.md §11, §12): the cloud telling the library what changed, the
//! context-menu items (upload, save to this PC, open in the cloud, retry) and Delete with its where-from
//! question.
use super::clip::resolve_library_dir;
use super::cloud::{DeleteScope, CLOUD_ONLY, SYNCED};
use super::state::LibraryState;
use crate::app_ctx::AppCtx;
use crate::services::cloud::{stream, LibraryEvent};
use crate::LibraryVM;
use lag_cloud::settings::DeleteMode;
use crate::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;

pub(super) fn bind(ui: &crate::AppWindow, ctx: &Rc<AppCtx>, state: &Rc<RefCell<LibraryState>>) {
    state.borrow_mut().cloud = ctx.cloud().library();
    ui.global::<LibraryVM>().set_cloud_connected(state.borrow().cloud.connected);

    // the cloud changed: re-join; new or gone cloud-only cards need a rescan (layout + thumbnails)
    {
        let (st, weak, c) = (state.clone(), ui.as_weak(), ctx.clone());
        ctx.cloud().on_library(move |event| {
            let Some(ui) = weak.upgrade() else { return };
            let rescan = match event {
                LibraryEvent::Changed => {
                    let Ok(mut s) = st.try_borrow_mut() else { return };
                    s.cloud = c.cloud().library();
                    s.apply_cloud(&ui)
                }
                LibraryEvent::IdAssigned { path, id } => {
                    let Ok(mut s) = st.try_borrow_mut() else { return };
                    if let Some(&i) = s.clip_path_to_idx.get(path.as_str()) {
                        s.clips[i].cloud_id = Some(*id);
                    }
                    s.apply_cloud(&ui)
                }
                LibraryEvent::Rescan => true,
            };
            if rescan {
                LibraryState::start_refresh(&st);
            }
        });
    }

    let vm = ui.global::<LibraryVM>();
    {
        let c = ctx.clone();
        vm.on_context_upload(move |path| c.cloud().upload_now(path.as_str()));
    }
    {
        let c = ctx.clone();
        vm.on_context_save_local(move |path| {
            if let Some(id) = stream::parse_key(path.as_str()) {
                c.cloud().download(id, resolve_library_dir(&c.settings()));
            }
        });
    }
    {
        let (c, st) = (ctx.clone(), state.clone());
        vm.on_context_open_cloud(move |path| {
            let id = {
                let s = st.borrow();
                s.clip_path_to_idx.get(path.as_str()).and_then(|&i| s.clips.get(i)).and_then(|c| c.cloud_id)
            };
            match id.and_then(|id| c.cloud().web_link(id)) {
                Some(link) if link.starts_with("http") => {
                    std::thread::spawn(move || {
                        let _ = lag_cloud_drive::win::open_browser(&link);
                    });
                }
                // a NAS folder: the file in Explorer
                Some(path) => super::os::show_in_folder(&path),
                None => {}
            }
        });
    }
    {
        let c = ctx.clone();
        vm.on_context_retry(move || c.cloud().retry());
    }

    // Delete: a clip that is also (or only) in the cloud asks where from, unless the user chose for good
    {
        let (st, weak, c) = (state.clone(), ui.as_weak(), ctx.clone());
        vm.on_context_delete(move |path| {
            let Some(ui) = weak.upgrade() else { return };
            ask_or_delete(&ui, &c, &st, vec![path.to_string()]);
        });
    }
    {
        let (st, weak, c) = (state.clone(), ui.as_weak(), ctx.clone());
        vm.on_delete_selected(move || {
            let Some(ui) = weak.upgrade() else { return };
            let paths: Vec<String> = st.borrow().clips.iter().filter(|c| c.is_selected).map(|c| c.file_path.clone()).collect();
            ask_or_delete(&ui, &c, &st, paths);
        });
    }
    {
        let (st, weak, c) = (state.clone(), ui.as_weak(), ctx.clone());
        vm.on_delete_choose(move |choice, remember| {
            let Some(ui) = weak.upgrade() else { return };
            ui.global::<LibraryVM>().set_delete_dialog_open(false);
            if remember {
                c.cloud().edit(|s| s.delete_mode = if choice == 0 { DeleteMode::ThisPcOnly } else { DeleteMode::Everywhere });
            }
            let paths = std::mem::take(&mut st.borrow_mut().pending_delete);
            delete(&ui, &c, &st, &paths, choice);
        });
    }
    {
        let (st, weak) = (state.clone(), ui.as_weak());
        vm.on_delete_cancel(move || {
            st.borrow_mut().pending_delete.clear();
            if let Some(ui) = weak.upgrade() {
                ui.global::<LibraryVM>().set_delete_dialog_open(false);
            }
        });
    }
}

fn ask_or_delete(ui: &crate::AppWindow, ctx: &Rc<AppCtx>, st: &Rc<RefCell<LibraryState>>, paths: Vec<String>) {
    let scope = {
        let s = st.borrow();
        DeleteScope::of(paths.iter().filter_map(|p| s.clip_path_to_idx.get(p).and_then(|&i| s.clips.get(i))))
    };
    if scope.local() {
        // nothing in the cloud: as before the cloud
        delete(ui, ctx, st, &paths, 0);
        return;
    }
    let only_cloud = scope.both == 0;
    let mode = ctx.cloud().view().settings.delete_mode;
    // deleting what exists only in the cloud is for good: always asked
    if !only_cloud && mode != DeleteMode::Ask {
        delete(ui, ctx, st, &paths, if mode == DeleteMode::ThisPcOnly { 0 } else { 2 });
        return;
    }
    st.borrow_mut().pending_delete = paths;
    let vm = ui.global::<LibraryVM>();
    vm.set_delete_dialog_mixed(!only_cloud);
    vm.set_delete_dialog_cloud_only(only_cloud && scope.local_only == 0);
    vm.set_delete_dialog_open(true);
}

/// 0 = from this PC, 1 = from the cloud, 2 = everywhere. Clips only on this PC go with 0 and 2.
fn delete(ui: &crate::AppWindow, ctx: &Rc<AppCtx>, st: &Rc<RefCell<LibraryState>>, paths: &[String], choice: i32) {
    let mut s = st.borrow_mut();
    let mut changed = false;
    for path in paths {
        let Some(clip) = s.clip_path_to_idx.get(path).and_then(|&i| s.clips.get(i)).cloned() else { continue };
        let in_cloud = matches!(clip.cloud.state, SYNCED | CLOUD_ONLY);
        if choice != 0 && in_cloud {
            if let Some(id) = clip.cloud_id {
                ctx.cloud().delete_from_cloud(id);
                s.deleting.insert(id);
                changed = true;
            }
        }
        if choice != 1 && !clip.is_cloud_only() && s.delete_clip_by_path(path) {
            changed = true;
        }
    }
    s.selection_anchor = None;
    for c in s.clips.iter_mut() {
        c.is_selected = false;
    }
    if changed {
        // the local deletes are gone from the list; a clip still in the cloud comes back as a cloud card
        s.apply_cloud(ui);
        s.rebuild_view(ui);
        s.publish_clips();
    }
    s.notify_selection_changed(ui);
}
