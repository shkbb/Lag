//! Play/pause, undo, trim and cuts, timeline drags, audio tracks.
use super::{model::*, runtime::*, worker::*};
use crate::{AppWindow, EditorVM};
use slint::ComponentHandle;
use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

pub(super) fn bind_timeline(ui: &AppWindow, state: &Rc<RefCell<EditorRuntime>>) {
    let vm = ui.global::<EditorVM>();
    // Play / Pause
    {
        let st = state.clone();
        vm.on_play_pause(move || {
            let mut s = st.borrow_mut();
            // owner, 2026-11-17: "the play button does not work again" (worked after leaving/re-entering) -> logged
            log::info!(
                "[Editor] play/pause pressed: clip={} playing={} stopped_by_inactive={}",
                s.model.current_clip_path.is_some(),
                s.model.is_playing,
                s.stopped_by_inactive
            );
            if s.model.current_clip_path.is_none() {
                return;
            }
            if s.model.is_playing {
                let _ = s.commands.send(MpvCommand::Pause(true));
                s.model.is_playing = false;
                if let Some(ui) = s.ui.upgrade() {
                    ui.global::<EditorVM>().set_is_playing(false);
                }
                return;
            }
            if s.stopped_by_inactive {
                if let Some(path) = s.model.current_clip_path.clone() {
                    s.stopped_by_inactive = false;
                    let _ = s.commands.send(MpvCommand::Load(path));
                    s.apply_all_preview();
                }
            }
            let pos_sec = s.model.position * s.model.duration_sec;
            if pos_sec < s.model.trim_start_sec - 0.05 || pos_sec >= s.model.trim_end_sec - 0.05 {
                let target = (s.model.trim_start_sec / s.model.duration_sec.max(0.1)).clamp(0.0, 1.0);
                s.model.position = target;
                s.last_mpv_pos = target;
                s.last_mpv_pos_at = Instant::now();
                if let Some(ui) = s.ui.upgrade() {
                    ui.global::<EditorVM>().set_playhead_fraction(target as f32);
                }
                s.queue_seek(&st, target);
            }
            let _ = s.commands.send(MpvCommand::Play);
        });
    }

    // Undo / PushUndo
    {
        let st = state.clone();
        vm.on_push_undo(move || {
            st.borrow_mut().model.push_undo();
        });
    }
    {
        let st = state.clone();
        vm.on_undo(move || {
            let mut s = st.borrow_mut();
            let prev_pos = s.model.position;
            if s.model.undo() {
                let new_pos = s.model.position;
                if (new_pos - prev_pos).abs() > 1e-6 {
                    s.queue_seek(&st, new_pos);
                }
                s.apply_all_preview();
                s.sync_all_to_ui();
            }
        });
    }

    // Set start / end / cut / reset trim
    {
        let st = state.clone();
        vm.on_set_trim_start_here(move || {
            let mut s = st.borrow_mut();
            if s.model.current_clip_path.is_none() {
                return;
            }
            s.model.push_undo();
            let dur = s.model.duration_sec;
            let t = (s.model.position * dur).clamp(0.0, dur);
            s.model.trim_start_sec = t.min((s.model.trim_end_sec - MIN_TRIM_GAP_SEC).max(0.0));
            s.model.link_audio_to_trim();
            let pos = (s.model.trim_start_sec / dur.max(0.1)).clamp(0.0, 1.0);
            s.model.position = pos;
            s.queue_seek(&st, pos);
            if let Some(ui) = s.ui.upgrade() {
                let vm = ui.global::<EditorVM>();
                s.sync_trim_displays(&vm);
                vm.set_playhead_fraction(pos as f32);
            }
        });
    }
    {
        let st = state.clone();
        vm.on_set_trim_end_here(move || {
            let mut s = st.borrow_mut();
            if s.model.current_clip_path.is_none() {
                return;
            }
            s.model.push_undo();
            let dur = s.model.duration_sec;
            let t = (s.model.position * dur).clamp(0.0, dur);
            s.model.trim_end_sec = t.max((s.model.trim_start_sec + MIN_TRIM_GAP_SEC).min(dur));
            s.model.link_audio_to_trim();
            if let Some(ui) = s.ui.upgrade() {
                s.sync_trim_displays(&ui.global::<EditorVM>());
            }
        });
    }
    {
        let st = state.clone();
        vm.on_toggle_cut(move || {
            let mut s = st.borrow_mut();
            if s.model.current_clip_path.is_none() {
                return;
            }
            let t = (s.model.position * s.model.duration_sec).clamp(s.model.trim_start_sec, s.model.trim_end_sec);
            match s.model.pending_cut_start {
                None => {
                    s.model.pending_cut_start = Some(t);
                }
                Some(p) => {
                    let a = p.min(t);
                    let b = p.max(t);
                    s.model.pending_cut_start = None;
                    if b - a >= MIN_CUT_LEN_SEC {
                        s.model.push_undo();
                        s.model.add_cut(a, b);
                    }
                }
            }
            if let Some(ui) = s.ui.upgrade() {
                s.sync_cuts_to_ui(&ui.global::<EditorVM>());
            }
        });
    }
    {
        let st = state.clone();
        vm.on_remove_cut_at(move |frac| {
            let mut s = st.borrow_mut();
            let sec = (frac as f64).clamp(0.0, 1.0) * s.model.duration_sec;
            if let Some(idx) = s.model.cuts.iter().position(|c| sec >= c.start_sec && sec <= c.end_sec) {
                if s.model.remove_cut(idx) {
                    if let Some(ui) = s.ui.upgrade() {
                        s.sync_cuts_to_ui(&ui.global::<EditorVM>());
                    }
                }
            }
        });
    }
    {
        let st = state.clone();
        vm.on_reset_trim(move || {
            let mut s = st.borrow_mut();
            if s.model.current_clip_path.is_none() {
                return;
            }
            s.model.push_undo();
            s.model.trim_start_sec = 0.0;
            s.model.trim_end_sec = s.model.duration_sec;
            s.model.link_audio_to_trim();
            s.model.clear_cuts();
            s.model.position = 0.0;
            s.queue_seek(&st, 0.0);
            if let Some(ui) = s.ui.upgrade() {
                let vm = ui.global::<EditorVM>();
                s.sync_trim_displays(&vm);
                s.sync_cuts_to_ui(&vm);
                vm.set_playhead_fraction(0.0);
            }
        });
    }

    // Timeline drag & selection
    {
        let st = state.clone();
        vm.on_toggle_video_selected(move || {
            let mut s = st.borrow_mut();
            s.model.is_video_selected = !s.model.is_video_selected;
            if let Some(ui) = s.ui.upgrade() {
                ui.global::<EditorVM>().set_is_video_selected(s.model.is_video_selected);
            }
        });
    }
    {
        let st = state.clone();
        vm.on_timeline_drag(move |mode, frac, width_px| {
            let mut s = st.borrow_mut();
            s.is_scrubbing = mode == 3;
            let seek_pos = s.model.apply_timeline_drag(mode, frac as f64, width_px as f64);
            if let Some(pos) = seek_pos {
                s.last_mpv_pos = pos;
                s.last_mpv_pos_at = Instant::now();
                s.queue_seek(&st, pos);
            }
            if let Some(ui) = s.ui.upgrade() {
                let vm = ui.global::<EditorVM>();
                s.sync_trim_displays(&vm);
                if mode == 1 || mode == 3 {
                    vm.set_playhead_fraction(s.model.position as f32);
                }
                if mode == 3 && s.model.pending_cut_start.is_some() {
                    s.sync_cuts_to_ui(&vm);
                }
                if s.model.is_video_selected && (mode == 1 || mode == 2) {
                    s.sync_tracks_to_ui(&vm);
                }
            }
        });
    }
    {
        let st = state.clone();
        vm.on_timeline_drag_end(move || {
            st.borrow_mut().is_scrubbing = false;
        });
    }

    // Audio follows the video trim (toggle in the timeline header)
    {
        let st = state.clone();
        vm.on_set_audio_follows_video(move |on| {
            let mut s = st.borrow_mut();
            s.model.push_undo();
            s.model.audio_follows_video = on;
            s.model.link_audio_to_trim();
            if let Some(ui) = s.ui.upgrade() {
                let vm = ui.global::<EditorVM>();
                vm.set_audio_follows_video(on);
                s.sync_tracks_to_ui(&vm);
            }
        });
    }

    // Audio tracks
    {
        let st = state.clone();
        vm.on_toggle_audio_selected(move |idx| {
            let mut s = st.borrow_mut();
            if let Some(t) = s.model.audio_tracks.get_mut(idx as usize) {
                t.is_selected = !t.is_selected;
            }
            if let Some(ui) = s.ui.upgrade() {
                s.sync_tracks_to_ui(&ui.global::<EditorVM>());
            }
        });
    }
    {
        let st = state.clone();
        vm.on_set_audio_muted(move |idx, muted| {
            let mut s = st.borrow_mut();
            if let Some(t) = s.model.audio_tracks.get_mut(idx as usize) {
                t.is_muted = muted;
            }
            s.apply_preview_audio();
            if let Some(ui) = s.ui.upgrade() {
                s.sync_tracks_to_ui(&ui.global::<EditorVM>());
            }
        });
    }
    {
        let st = state.clone();
        vm.on_set_audio_volume(move |idx, vol| {
            let mut s = st.borrow_mut();
            if let Some(t) = s.model.audio_tracks.get_mut(idx as usize) {
                t.volume = vol.clamp(0, 100);
            }
            s.apply_preview_audio();
            if let Some(ui) = s.ui.upgrade() {
                s.sync_tracks_to_ui(&ui.global::<EditorVM>());
            }
        });
    }
    {
        let st = state.clone();
        vm.on_set_audio_range(move |idx, start, end| {
            let mut s = st.borrow_mut();
            if let Some(t) = s.model.audio_tracks.get_mut(idx as usize) {
                t.start_sec = start as f64;
                t.end_sec = end as f64;
            }
            if let Some(ui) = s.ui.upgrade() {
                s.sync_tracks_to_ui(&ui.global::<EditorVM>());
            }
        });
    }
}
