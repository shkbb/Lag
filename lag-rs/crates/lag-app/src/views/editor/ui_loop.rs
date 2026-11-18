//! The native video host (editor/native.rs) and the 33 ms UI tick: drains worker messages, keeps cuts and the
//! trim end, interpolates the playhead.

use super::merge;
use super::{model::*, runtime::*, tr, tr_fmt, worker::*};
use crate::{AppWindow, EditorVM, MainVM, Page};
use lag_player::{MpvPlayer, PlayerEvent};
use slint::{ComponentHandle, Model, Timer, TimerMode};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

pub(super) fn run(
    ui: &AppWindow,
    state: &Rc<RefCell<EditorRuntime>>,
    events: mpsc::Receiver<UiMessage>,
    mpv_player: Rc<RefCell<Option<Arc<MpvPlayer>>>>,
) {
    // ── Native video surface + overlay window (player v2 engine, see editor/native.rs) ──
    let host: Rc<RefCell<Option<super::native::EditorHost>>> = Rc::new(RefCell::new(None));
    let host_failed = Rc::new(Cell::new(false));
    let stage_rect = Rc::new(Cell::new((0.0f32, 0.0f32, 0.0f32, 0.0f32)));
    let resync: Rc<dyn Fn()> = {
        let host = Rc::downgrade(&host);
        let st = Rc::downgrade(state);
        let stage_rect = stage_rect.clone();
        let weak = ui.as_weak();
        Rc::new(move || {
            let (Some(host), Some(st), Some(ui)) = (host.upgrade(), st.upgrade(), weak.upgrade()) else { return };
            let (Ok(h), Ok(s)) = (host.try_borrow(), st.try_borrow()) else { return };
            if let Some(h) = h.as_ref() {
                h.sync(&ui, &s.model, stage_rect.get());
            }
        })
    };
    {
        let stage_rect = stage_rect.clone();
        let resync = resync.clone();
        ui.global::<EditorVM>().on_stage_rect_changed(move |x, y, w, h| {
            stage_rect.set((x, y, w, h));
            resync();
        });
    }

    // 33ms UI loop: drains worker messages, enforces cut skipping & trim end-stop, interpolates playhead.
    let timer = Rc::new(Timer::default());
    let keep_timer = timer.clone();
    let st_timer = state.clone();
    let mut was_visible = true;
    timer.start(TimerMode::Repeated, Duration::from_millis(33), move || {
        let _ = &keep_timer;
        let mut s = st_timer.borrow_mut();
        let Some(ui) = s.ui.upgrade() else {
            return;
        };
        let vm = ui.global::<EditorVM>();
        let visible = ui.window().is_visible() && ui.global::<MainVM>().get_current_page() == Page::Editor;
        if was_visible && !visible {
            s.model.is_playing = false;
            let _ = s.commands.send(MpvCommand::Pause(true));
            vm.set_is_playing(false);
        }
        was_visible = visible;

        // native video surface: created the first time the editor page is on screen with a real window
        // (headless checks have no HWND and keep the in-scene preview)
        if visible && host.borrow().is_none() && !host_failed.get() {
            if let Some(hwnd) = crate::shell::nofocus::hwnd(ui.window()) {
                match super::native::EditorHost::new(&ui, hwnd) {
                    Ok(h) => {
                        let _ = s.commands.send(MpvCommand::InitNative(h.wid()));
                        vm.set_native_video(true);
                        *host.borrow_mut() = Some(h);
                        super::native::set_resync(Some(resync.clone()));
                    }
                    Err(e) => {
                        host_failed.set(true);
                        log::error!("[Editor] native video surface failed: {e}");
                    }
                }
            }
        }
        if let Some(h) = host.borrow().as_ref() {
            h.sync(&ui, &s.model, stage_rect.get());
        }

        for msg in events.try_iter() {
            match msg {
                UiMessage::MpvReady(p) => {
                    *mpv_player.borrow_mut() = Some(p);
                    ui.window().request_redraw();
                }
                UiMessage::MpvEvent(ev) => match ev {
                    PlayerEvent::Position(sec) => {
                        let dur = s.model.duration_sec;
                        if dur > 0.0 {
                            if s.model.is_playing {
                                if let Some(jump_sec) = should_skip_cut(sec, &s.model.cuts, s.model.trim_end_sec) {
                                    let target = (jump_sec / dur).clamp(0.0, 1.0);
                                    s.model.position = target;
                                    s.last_mpv_pos = target;
                                    s.last_mpv_pos_at = Instant::now();
                                    let _ = s.commands.send(MpvCommand::Seek(target, true));
                                    vm.set_playhead_fraction(target as f32);
                                    vm.set_time_display(
                                        format!("{} / {}", format_time(jump_sec), format_time(dur)).into(),
                                    );
                                    continue;
                                }
                                if sec >= s.model.trim_end_sec {
                                    s.model.is_playing = false;
                                    let target = (s.model.trim_end_sec / dur).clamp(0.0, 1.0);
                                    s.model.position = target;
                                    s.last_mpv_pos = target;
                                    s.last_mpv_pos_at = Instant::now();
                                    let _ = s.commands.send(MpvCommand::Pause(true));
                                    let _ = s.commands.send(MpvCommand::Seek(target, true));
                                    vm.set_is_playing(false);
                                    vm.set_playhead_fraction(target as f32);
                                    vm.set_time_display(
                                        format!("{} / {}", format_time(s.model.trim_end_sec), format_time(dur)).into(),
                                    );
                                    continue;
                                }
                            }
                            let frac = (sec / dur).clamp(0.0, 1.0);
                            s.model.position = frac;
                            s.last_mpv_pos = frac;
                            s.last_mpv_pos_at = Instant::now();
                            if !s.model.is_playing || s.is_scrubbing {
                                vm.set_playhead_fraction(frac as f32);
                            }
                            if s.model.pending_cut_start.is_some() {
                                s.sync_cuts_to_ui(&vm);
                            }
                            vm.set_time_display(format!("{} / {}", format_time(sec), format_time(dur)).into());
                        }
                    }
                    PlayerEvent::Duration(d) if d > 0.0 => {
                        if s.model.duration_sec <= 1.0 && d > 1.0 {
                            let end_at_max = (s.model.trim_end_sec - s.model.duration_sec).abs() < 0.05;
                            s.model.duration_sec = d;
                            if end_at_max || s.model.trim_end_sec > d {
                                s.model.trim_end_sec = d;
                            }
                            for t in &mut s.model.audio_tracks {
                                t.clip_duration_sec = d;
                                if (t.end_sec - 1.0).abs() < 0.05 {
                                    t.end_sec = d;
                                }
                            }
                            s.sync_trim_displays(&vm);
                            s.sync_tracks_to_ui(&vm);
                        }
                    }
                    PlayerEvent::Paused(paused) => {
                        log::info!("[Editor] mpv paused={paused}");
                        let playing = !paused && s.model.current_clip_path.is_some();
                        s.model.is_playing = playing;
                        s.last_mpv_pos_at = Instant::now();
                        vm.set_is_playing(playing);
                        if playing {
                            s.apply_all_preview();
                            s.schedule_reassert(&st_timer);
                        }
                    }
                    PlayerEvent::Eof(true) => {
                        log::info!("[Editor] mpv eof");
                        s.model.is_playing = false;
                        vm.set_is_playing(false);
                    }
                    // END_FILE of the PREVIOUS clip arrives after the new one already started playing (log,
                    // 2026-11-17 20:04: load -> paused=false -> stopped): it flipped the UI to "not playing" while
                    // mpv played, so the next press sent "play" to a playing mpv and nothing changed (owner: "the
                    // button does not work after picking another clip, again"). Our own Stop sets the state itself.
                    PlayerEvent::Stopped => log::info!("[Editor] mpv stopped (previous file)"),
                    PlayerEvent::VideoSize(..) => {}
                    PlayerEvent::AudioTracks(_) | PlayerEvent::FileLoaded => {
                        s.apply_preview_audio();
                        s.apply_preview_vf();
                    }
                    PlayerEvent::Error(e) => log::warn!("[Editor] mpv error: {e}"),
                    _ => {}
                },
                UiMessage::ProbeDone(gen, clip_path, res)
                    if gen == s.clip_gen && s.model.current_clip_path.as_deref() == Some(&clip_path) =>
                {
                    log::info!(
                        "[Editor] probe: {}x{} {:.2}s audio streams={}",
                        res.video_width,
                        res.video_height,
                        res.duration_sec,
                        res.audio_streams
                    );
                    s.model.source_width = res.video_width;
                    s.model.source_height = res.video_height;
                    s.model.reset_reframe();

                    let end_at_max = (s.model.trim_end_sec - s.model.duration_sec).abs() < 0.05;
                    if res.duration_sec > 0.0 {
                        s.model.duration_sec = res.duration_sec.max(1.0);
                    }
                    if end_at_max || s.model.trim_end_sec > s.model.duration_sec {
                        s.model.trim_end_sec = s.model.duration_sec;
                    }
                    s.model.trim_start_sec = s.model.trim_start_sec.min(s.model.trim_end_sec);

                    let dur = s.model.duration_sec;
                    s.model.audio_tracks.clear();
                    if res.audio_streams >= 3 {
                        s.model.audio_tracks.push(AudioTrack::new(1, tr("Editor_TrackSystem"), dur));
                        s.model.audio_tracks.push(AudioTrack::new(2, tr("Editor_TrackMic"), dur));
                    } else if res.audio_streams == 2 {
                        s.model.audio_tracks.push(AudioTrack::new(0, tr("Editor_TrackSystem"), dur));
                        s.model.audio_tracks.push(AudioTrack::new(1, tr("Editor_TrackMic"), dur));
                    } else if res.audio_streams == 1 {
                        s.model.audio_tracks.push(AudioTrack::new(0, tr("Editor_TrackAudio"), dur));
                    }

                    s.model.link_audio_to_trim();
                    s.apply_preview_audio();
                    s.sync_all_to_ui();
                }
                UiMessage::ProbeDone(gen, clip_path, _) => {
                    log::info!(
                        "[Editor] probe result dropped: gen {gen} vs {} / {clip_path} vs {:?}",
                        s.clip_gen,
                        s.model.current_clip_path
                    );
                }
                UiMessage::TimelineThumb(gen, idx, px) if gen == s.clip_gen => {
                    let thumbs = vm.get_timeline_thumbs();
                    if idx < thumbs.row_count() {
                        thumbs.set_row_data(idx, px.into_image());
                    }
                }
                UiMessage::TimelineThumb(..) => {}
                UiMessage::FilterThumb(gen, idx, px) if gen == s.clip_gen => {
                    let filters = vm.get_filter_options();
                    if let Some(mut item) = filters.row_data(idx) {
                        item.preview = px.into_image();
                        item.has_preview = true;
                        filters.set_row_data(idx, item);
                    }
                }
                UiMessage::FilterThumb(..) => {}
                UiMessage::ExportProgress(gen, pct) if gen == s.export_gen => {
                    vm.set_export_progress(pct);
                    let text = match s.job {
                        Job::Export => tr_fmt("Editor_Exporting", &pct.to_string()),
                        Job::Merge { .. } => merge::progress_text(pct),
                    };
                    vm.set_export_status(text.into());
                }
                UiMessage::ExportProgress(..) => {}
                UiMessage::ExportFinished(gen, res) if gen == s.export_gen => {
                    let job = std::mem::replace(&mut s.job, Job::Export);
                    s.is_exporting = false;
                    vm.set_is_exporting(false);
                    vm.set_export_progress(0);
                    match res {
                        Ok(fname) => {
                            vm.set_export_status(tr_fmt("Editor_ExportDone", &fname).into());
                            s.ctx.request_library_refresh();
                            if let Job::Merge { out } = job {
                                merge::finished(&mut s, &vm, &out);
                            }
                        }
                        Err(None) => vm.set_export_status("".into()),
                        Err(Some(err)) => {
                            vm.set_export_status(tr_fmt("Editor_ExportError", &err).into());
                        }
                    }
                }
                UiMessage::ExportFinished(..) => {}
            }
        }

        // 30fps playhead interpolation while playing and not scrubbing
        if s.model.is_playing && !s.is_scrubbing && s.model.duration_sec > 0.0 {
            let pred = interpolate_playhead(
                s.last_mpv_pos,
                s.last_mpv_pos_at.elapsed().as_secs_f64(),
                s.model.effective_speed(),
                s.model.duration_sec,
                &s.model.cuts,
                s.model.trim_start_sec,
                s.model.trim_end_sec,
            );
            vm.set_playhead_fraction(pred as f32);
        }
    });
}
