//! The editor side of the montage: switching the live edit between segments, the strip in `EditorVM`, segment
//! operations (add, split, remove, move, undo), playing on into the next segment, and adding probed clips.

use super::model::{EditModel, TrackNames};
use super::montage::{block_widths, drop_index, Montage, Segment};
use super::runtime::EditorRuntime;
use super::worker::{spawn_probe_and_thumbs, MpvCommand, UiMessage};
use super::{model::format_time, tr};
use crate::services::ffmpeg::{self, ProbeResult};
use crate::{EditorFilterData, EditorSegmentData, EditorVM};
use crate::ComponentHandle;
use slint::{Image, Model, ModelRc, VecModel};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Strip geometry shared with montage_strip.slint (block minimum width and gap, logical px).
pub(super) const STRIP_MIN_BLOCK: f64 = 72.0;
pub(super) const STRIP_GAP: f64 = 6.0;

/// Localized audio lane names for `EditModel::apply_probe`.
pub(super) fn track_names() -> TrackNames {
    TrackNames { single: tr("Editor_TrackAudio"), system: tr("Editor_TrackSystem"), mic: tr("Editor_TrackMic") }
}

impl EditorRuntime {
    /// Shows the live edit: the preview loads its clip at the trim start (playing or paused), the timeline
    /// thumbnails and the probe run again for that clip. The edit itself is kept as it is.
    pub(super) fn load_live_edit(&mut self, play: bool) {
        let Some(path) = self.model.current_clip_path.clone() else { return };
        if let Some(c) = self.thumb_cancel.take() {
            c.store(true, Ordering::Release);
        }
        self.clip_gen = self.clip_gen.wrapping_add(1);
        self.stopped_by_inactive = false;
        let dur = self.model.duration_sec.max(0.1);
        let start = self.model.trim_start_sec.clamp(0.0, dur);
        self.model.position = (start / dur).clamp(0.0, 1.0);
        self.model.is_playing = play;
        self.last_mpv_pos = self.model.position;
        self.last_mpv_pos_at = Instant::now();

        if let Some(ui) = self.ui.upgrade() {
            let vm = ui.global::<EditorVM>();
            vm.set_reframe_selected(false);
            vm.set_video_image(Image::default());
            vm.set_is_playing(play);
            vm.set_playhead_fraction(self.model.position as f32);
            vm.set_time_display(format!("{} / {}", format_time(start), format_time(dur)).into());
            vm.set_timeline_thumbs(ModelRc::from(Rc::new(VecModel::from(vec![Image::default(); 10]))));
            let filters: Vec<EditorFilterData> = self
                .model
                .filter_options
                .iter()
                .map(|f| EditorFilterData { name: f.name.into(), preview: Image::default(), has_preview: false })
                .collect();
            vm.set_filter_options(ModelRc::from(Rc::new(VecModel::from(filters))));
            self.sync_all_to_ui();
            self.sync_montage_to_ui(&vm);
        }
        // hidden and black until the new clip's first frame + size are in (see `reveal_video_if_ready`)
        self.video_ready = false;
        self.first_frame = false;
        self.reveal_gen.set(self.reveal_gen.get().wrapping_add(1));
        let _ = self.commands.send(MpvCommand::Brightness(-100));
        let _ = self.commands.send(MpvCommand::LoadAt { path: path.clone(), start, play });
        self.apply_all_preview();
        let cancel = Arc::new(AtomicBool::new(false));
        self.thumb_cancel = Some(cancel.clone());
        spawn_probe_and_thumbs(self.clip_gen, path, self.model.duration_sec, cancel, self.messages_tx.clone());
    }

    /// Shows the preview once the clip's size is known and its first frame is up: the surface appears (still
    /// black) at its final shape and fades in over ~180 ms.
    pub(super) fn reveal_video_if_ready(&mut self) {
        if self.video_ready || !self.first_frame || !self.model.is_probed() {
            return;
        }
        self.video_ready = true;
        const STEPS: i64 = 12; // x 15 ms
        let gen = self.reveal_gen.get();
        for n in 1..=STEPS {
            let (tx, current) = (self.commands.clone(), self.reveal_gen.clone());
            slint::Timer::single_shot(std::time::Duration::from_millis(15 * n as u64), move || {
                if current.get() == gen {
                    let _ = tx.send(MpvCommand::Brightness(-100 + 100 * n / STEPS));
                }
            });
        }
    }

    /// Makes segment `index` the live edit (the current one is written back first).
    pub(super) fn select_segment(&mut self, index: usize, play: bool) {
        self.montage.commit(&self.model);
        let Some(edit) = self.montage.select(index) else { return };
        self.model = edit.clone();
        self.load_live_edit(play);
    }

    /// The strip: one block per segment (title, length, thumbnail from the clip list, selected), the total.
    pub(super) fn sync_montage_to_ui(&mut self, vm: &EditorVM) {
        self.montage.commit(&self.model);
        let clips = vm.get_clips();
        let thumb = |path: &str| clips.iter().find(|c| c.path == path).filter(|c| c.has_thumbnail).map(|c| c.thumbnail);
        let widths = block_widths(&self.montage.durations(), self.strip_width.max(1.0), STRIP_MIN_BLOCK, STRIP_GAP);
        let mut x = 0.0;
        let rows: Vec<EditorSegmentData> = self
            .montage
            .segments()
            .iter()
            .enumerate()
            .map(|(i, s): (usize, &Segment)| {
                let path = s.edit.current_clip_path.as_deref().unwrap_or("");
                let image = thumb(path);
                let (bx, bw) = (x, widths[i]);
                x += bw + STRIP_GAP;
                EditorSegmentData {
                    id: s.id as i32,
                    title: s.edit.clip_name.as_str().into(),
                    duration: format_time(s.duration()).into(),
                    x: bx as f32,
                    w: bw as f32,
                    has_thumbnail: image.is_some(),
                    thumbnail: image.unwrap_or_default(),
                    selected: i == self.montage.selected(),
                }
            })
            .collect();
        vm.set_montage_segments(ModelRc::from(Rc::new(VecModel::from(rows))));
        vm.set_montage_active(self.montage.is_montage());
        vm.set_montage_total(format_time(self.montage.duration()).into());
        self.strip_live_duration = Segment { id: 0, edit: self.model.clone() }.duration();
    }

    /// Refreshes the strip when the live segment's length changed (trim, cuts, speed) - called every tick.
    pub(super) fn refresh_strip_if_resized(&mut self, vm: &EditorVM) {
        if !self.montage.is_montage() {
            return;
        }
        let now = Segment { id: 0, edit: self.model.clone() }.duration();
        if (now - self.strip_live_duration).abs() > 0.01 {
            self.sync_montage_to_ui(vm);
        }
    }

    fn after_montage_change(&mut self) {
        self.model = self.montage.selected_edit().clone();
        self.load_live_edit(false);
    }

    /// Razor: splits the selected segment at the playhead.
    pub(super) fn split_at_playhead(&mut self) {
        let at = self.model.position * self.model.duration_sec;
        self.montage.commit(&self.model);
        let undo_len = self.model.undo_stack.len();
        let sel = self.montage.selected();
        if self.montage.split(sel, at, undo_len) {
            log::info!("[Montage] split segment {sel} at {at:.2}s");
            // the right half continues where the playhead is: no reload jump to its trim start
            self.model = self.montage.selected_edit().clone();
            self.model.position = at / self.model.duration_sec.max(0.1);
            if let Some(ui) = self.ui.upgrade() {
                let vm = ui.global::<EditorVM>();
                self.sync_all_to_ui();
                self.sync_montage_to_ui(&vm);
            }
        }
    }

    pub(super) fn remove_selected_segment(&mut self) {
        self.montage.commit(&self.model);
        let sel = self.montage.selected();
        if self.montage.remove(sel, self.model.undo_stack.len()) {
            self.after_montage_change();
        }
    }

    /// The strip changed size: lay its blocks out again.
    pub(super) fn strip_resized(&mut self, width: f64) {
        if (width - self.strip_width).abs() > 0.5 && width > 1.0 {
            self.strip_width = width;
            if let Some(ui) = self.ui.upgrade() {
                self.sync_montage_to_ui(&ui.global::<EditorVM>());
            }
        }
    }

    /// Strip drag: the block `from` was dropped `dx` px away in a strip `strip_w` px wide.
    pub(super) fn drop_segment(&mut self, from: usize, dx: f64, strip_w: f64) {
        let widths = block_widths(&self.montage.durations(), strip_w, STRIP_MIN_BLOCK, STRIP_GAP);
        let to = drop_index(&widths, from, dx, STRIP_GAP);
        self.move_segment(from, to);
    }

    pub(super) fn move_segment(&mut self, from: usize, to: usize) {
        self.montage.commit(&self.model);
        if self.montage.move_segment(from, to, self.model.undo_stack.len()) {
            // the live edit is the moved segment: nothing to reload
            if let Some(ui) = self.ui.upgrade() {
                self.sync_montage_to_ui(&ui.global::<EditorVM>());
            }
        }
    }

    /// Ctrl+Z: the newer of the last montage operation and the last edit inside the selected segment.
    pub(super) fn undo_any(&mut self) -> bool {
        if self.montage.undo_is_montage(self.model.undo_stack.len()) && self.montage.undo() {
            self.after_montage_change();
            return true;
        }
        false
    }

    /// End of the selected segment while playing a montage: on into the next one. False = it was the last.
    pub(super) fn advance_to_next_segment(&mut self) -> bool {
        let next = self.montage.selected() + 1;
        if !self.play_through || next >= self.montage.len() {
            self.play_through = false;
            return false;
        }
        self.select_segment(next, true);
        true
    }

    /// "Play the whole montage": from the first segment, playing on through all of them.
    pub(super) fn play_montage_from_start(&mut self) {
        self.play_through = true;
        self.select_segment(0, true);
    }

    /// Probes `paths` on a worker thread; the result comes back as `UiMessage::MontageProbed`.
    pub(super) fn add_clips(&mut self, paths: Vec<String>, replace: bool) {
        if paths.is_empty() {
            return;
        }
        self.montage_gen = self.montage_gen.wrapping_add(1);
        let (gen, tx) = (self.montage_gen, self.messages_tx.clone());
        let _ = std::thread::Builder::new().name("Lag.MontageProbe".into()).spawn(move || {
            let probed: Vec<(String, ProbeResult)> = paths
                .into_iter()
                .filter_map(|p| match ffmpeg::probe(&p) {
                    Some(r) if r.video_width > 0 => Some((p, r)),
                    _ => {
                        log::warn!("[Montage] cannot read {p} - skipped");
                        None
                    }
                })
                .collect();
            let _ = tx.send(UiMessage::MontageProbed(gen, probed, replace));
        });
    }

    /// The probe of `add_clips` is back: new segments (or a new montage) with size, length and tracks known.
    pub(super) fn apply_probed_clips(&mut self, gen: u64, probed: Vec<(String, ProbeResult)>, replace: bool) {
        if gen != self.montage_gen || probed.is_empty() {
            return;
        }
        let names = track_names();
        let frame_shape = self.model.selected_aspect_index;
        let edits: Vec<EditModel> = probed
            .iter()
            .map(|(path, res)| {
                let mut e = EditModel::new(&tr("Option_Native"));
                e.reset_for_clip(path, res.duration_sec, &names.single);
                e.apply_probe(res, &names);
                // export settings are the editor's, not the clip's
                e.selected_export_res_index = self.model.selected_export_res_index;
                e.selected_export_format_index = self.model.selected_export_format_index;
                if !replace {
                    e.selected_aspect_index = frame_shape; // one frame shape for the whole montage
                    e.reset_reframe();
                }
                e
            })
            .collect();
        let fresh = replace || self.model.current_clip_path.is_none();
        if fresh {
            log::info!("[Montage] new montage of {} clip(s)", edits.len());
            self.montage = Montage::from_edits(edits);
        } else {
            self.montage.commit(&self.model);
            for e in edits {
                let undo_len = self.model.undo_stack.len();
                self.montage.add_after_selected(e, undo_len);
            }
            log::info!("[Montage] added clip(s): {} segment(s) now", self.montage.len());
        }
        self.play_through = false;
        self.after_montage_change();
    }
}

/// The selected segment's position on the montage's own clock (for the strip's time readout).
pub(super) fn montage_clock(m: &Montage, live: &EditModel) -> f64 {
    let pos = live.position * live.duration_sec;
    let in_seg: f64 =
        Segment { id: 0, edit: live.clone() }.pieces().iter().map(|&(a, b)| (pos.min(b) - a).max(0.0)).sum::<f64>()
            / live.effective_speed().max(0.01);
    m.start_of(m.selected()) + in_seg
}
