//! Clip list, Import, Back, page activation.
use super::import::*;
use super::{runtime::*, worker::*};
use crate::app_ctx::AppCtx;
use crate::services::AppCtxPlaybackExt;
use crate::{AppWindow, EditorVM, MainVM, Page};
use slint::{ComponentHandle, Timer, TimerMode};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

pub(super) fn bind_navigation(ui: &AppWindow, state: &Rc<RefCell<EditorRuntime>>) {
    let vm = ui.global::<EditorVM>();
    // Open clip from sidebar
    {
        let st = state.clone();
        vm.on_open_clip(move |path| {
            let mut s = st.borrow_mut();
            let hint = s.sidebar_durations.get(path.as_str()).copied().unwrap_or(1.0);
            s.open_clip(&path, hint);
        });
    }

    // Import: any video file from disk into the editor (owner, 2026-11-18). The system file dialog runs on its
    // own thread (it pumps its own messages; Slint keeps rendering); the picked file opens like a sidebar clip.
    // Export then writes `<name>-edit.<ext>` next to that file.
    {
        let st = state.clone();
        let weak = ui.as_weak();
        let busy = Rc::new(Cell::new(false));
        vm.on_import_clip(move || {
            if busy.replace(true) {
                return;
            }
            let owner =
                weak.upgrade().and_then(|ui| crate::shell::nofocus::hwnd(ui.window())).map_or(0, |h| h.0 as isize);
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || {
                let _ = tx.send(pick_video_file(owner));
            });
            let (st, busy) = (st.clone(), busy.clone());
            let timer = Rc::new(Timer::default());
            let keep = timer.clone();
            timer.start(TimerMode::Repeated, Duration::from_millis(50), move || {
                let Ok(picked) = rx.try_recv() else { return };
                keep.stop();
                busy.set(false);
                if let Some(path) = picked {
                    log::info!("[Editor] import {path}");
                    st.borrow_mut().open_clip(&path, 1.0);
                }
            });
        });
    }

    // Back -> Player
    {
        let weak = ui.as_weak();
        vm.on_back(move || {
            if let Some(ui) = weak.upgrade() {
                ui.global::<MainVM>().invoke_navigate(Page::Player);
            }
        });
    }

    // Page active changed
    {
        let st = state.clone();
        vm.on_active_changed(move |active| {
            let mut s = st.borrow_mut();
            if !active {
                // Always pause on leaving: the flag can lag behind mpv (owner: video kept playing behind the
                // library/player pages).
                s.model.is_playing = false;
                let _ = s.commands.send(MpvCommand::Pause(true));
                if let Some(ui) = s.ui.upgrade() {
                    ui.global::<EditorVM>().set_is_playing(false);
                }
            } else if s.stopped_by_inactive {
                if let Some(path) = s.model.current_clip_path.clone() {
                    s.stopped_by_inactive = false;
                    let pos = s.model.position;
                    let _ = s.commands.send(MpvCommand::Load(path));
                    s.apply_all_preview();
                    s.queue_seek(&st, pos);
                }
            }
        });
    }
}

/// App-wide events: the library changed, settings (language) changed, "Edit" pressed elsewhere, playback stop.
pub(super) fn bind_app_events(ctx: &Rc<AppCtx>, state: &Rc<RefCell<EditorRuntime>>) {
    {
        let st = Rc::downgrade(&state);
        ctx.on_clips_changed(move |clips| {
            if let Some(st) = st.upgrade() {
                if let Ok(mut s) = st.try_borrow_mut() {
                    s.sync_clips_from_shared(clips);
                }
            }
        });
    }
    {
        let st = Rc::downgrade(&state);
        ctx.on_settings_changed(move |_| {
            if let Some(st) = st.upgrade() {
                if let Ok(mut s) = st.try_borrow_mut() {
                    s.refresh_localized_options();
                }
            }
        });
    }
    {
        let st = Rc::downgrade(&state);
        ctx.on_edit_clip(move |path| {
            if let Some(st) = st.upgrade() {
                let mut s = st.borrow_mut();
                let hint = s.sidebar_durations.get(path.as_str()).copied().unwrap_or(1.0);
                s.open_clip(path, hint);
            }
        });
    }
    {
        let st = Rc::downgrade(&state);
        ctx.on_stop_playback(move |include_editor| {
            if include_editor {
                if let Some(st) = st.upgrade() {
                    // Never skip a stop: if the state is busy right now, retry on the next event-loop turn
                    // (a skipped stop left the editor's mpv playing audio under another page).
                    let Ok(mut s) = st.try_borrow_mut() else {
                        let st2 = Rc::downgrade(&st);
                        slint::Timer::single_shot(std::time::Duration::ZERO, move || {
                            if let Some(st) = st2.upgrade() {
                                if let Ok(s) = st.try_borrow_mut() {
                                    let _ = s.commands.send(MpvCommand::Stop);
                                }
                            }
                        });
                        return;
                    };
                    {
                        s.stopped_by_inactive = true;
                        s.model.is_playing = false;
                        let _ = s.commands.send(MpvCommand::Stop);
                        if let Some(ui) = s.ui.upgrade() {
                            ui.global::<EditorVM>().set_is_playing(false);
                        }
                    }
                }
            }
        });
    }
}
