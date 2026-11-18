//! Editor state on the UI thread (`EditorRuntime`): the model plus everything that talks to the worker,
//! and the methods that push the model into `EditorVM`.

use super::merge;
use super::montage::Montage;
use super::{model::*, preview::*, tr, worker::*};
use crate::app_ctx::AppCtx;
use crate::{
    AppWindow, ComboBoxItemData, EditorAudioTrackData, EditorClipItem, EditorCutBand, EditorFilterData,
    EditorTextOverlayData, EditorVM, SegmentedOption,
};
use slint::{Color, ComponentHandle, Image, Model, ModelRc, Timer, TimerMode, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::process::Child;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

pub(super) fn parse_hex_color(hex: &str) -> Color {
    let h = hex.trim().trim_start_matches('#');
    if h.len() == 6 {
        if let (Ok(r), Ok(g), Ok(b)) =
            (u8::from_str_radix(&h[0..2], 16), u8::from_str_radix(&h[2..4], 16), u8::from_str_radix(&h[4..6], 16))
        {
            return Color::from_rgb_u8(r, g, b);
        }
    }
    Color::from_rgb_u8(255, 255, 255)
}

pub(super) struct EditorRuntime {
    pub(super) ui: slint::Weak<AppWindow>,
    pub(super) ctx: Rc<AppCtx>,
    pub(super) model: EditModel,
    pub(super) commands: mpsc::Sender<MpvCommand>,
    pub(super) messages_tx: mpsc::Sender<UiMessage>,
    pub(super) clip_gen: u64,
    pub(super) thumb_cancel: Option<Arc<AtomicBool>>,
    pub(super) export_gen: u64,
    pub(super) export_cancel: Option<Arc<AtomicBool>>,
    pub(super) export_child: Arc<Mutex<Option<Child>>>,
    pub(super) is_exporting: bool,
    pub(super) merge: merge::MergePicks,
    /// All segments of the edit; `model` is the selected one's live copy (see montage.rs).
    pub(super) montage: Montage,
    /// Playback runs on into the next segment (set by Play while a montage is open, cleared by Pause/Stop).
    pub(super) play_through: bool,
    /// Generation of the last "add clips" probe (a newer request makes older results stale).
    pub(super) montage_gen: u64,
    /// The selected segment's length as last shown in the strip (refresh the strip when a trim changes it).
    pub(super) strip_live_duration: f64,
    /// The strip's width (logical px) as last reported by the UI: the blocks are laid out for it.
    pub(super) strip_width: f64,
    pub(super) stopped_by_inactive: bool,
    pub(super) sidebar_durations: HashMap<String, f64>,

    // Seek coalescer (80ms)
    pub(super) pending_seek: Option<f64>,
    pub(super) seek_timer: Timer,

    // Playhead interpolation & position sync
    pub(super) last_mpv_pos: f64,
    pub(super) last_mpv_pos_at: Instant,
    pub(super) is_scrubbing: bool,
    pub(super) drag_start_disp: (f64, f64, f64, f64),
    pub(super) last_color_undo_at: Option<Instant>,
    pub(super) reapply_timer: Timer,
}

pub(super) fn editable_clips(clips: &[crate::app_ctx::SharedClip]) -> Vec<&crate::app_ctx::SharedClip> {
    clips.iter().filter(|c| !c.is_image).collect()
}

impl EditorRuntime {
    pub(super) fn new(
        ui: &AppWindow,
        ctx: &Rc<AppCtx>,
        model: EditModel,
        commands: mpsc::Sender<MpvCommand>,
        messages_tx: mpsc::Sender<UiMessage>,
    ) -> Self {
        Self {
            ui: ui.as_weak(),
            ctx: ctx.clone(),
            montage: Montage::single(model.clone()),
            play_through: false,
            montage_gen: 0,
            strip_live_duration: 0.0,
            strip_width: 0.0,
            model,
            commands,
            messages_tx,
            clip_gen: 0,
            thumb_cancel: None,
            export_gen: 0,
            export_cancel: None,
            export_child: Arc::new(Mutex::new(None)),
            is_exporting: false,
            merge: merge::MergePicks::default(),
            stopped_by_inactive: false,
            sidebar_durations: HashMap::new(),
            pending_seek: None,
            seek_timer: Timer::default(),
            last_mpv_pos: 0.0,
            last_mpv_pos_at: Instant::now(),
            is_scrubbing: false,
            drag_start_disp: (0.0, 0.0, 1.0, 1.0),
            last_color_undo_at: None,
            reapply_timer: Timer::default(),
        }
    }

    pub(super) fn sync_all_to_ui(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let vm = ui.global::<EditorVM>();
        let m = &self.model;
        vm.set_has_clip(m.current_clip_path.is_some());
        vm.set_current_clip_path(m.current_clip_path.as_deref().unwrap_or("").into());
        vm.set_clip_name(m.clip_name.as_str().into());
        vm.set_is_playing(m.is_playing);
        vm.set_duration_sec(m.duration_sec as f32);
        vm.set_trim_start_sec(m.trim_start_sec as f32);
        vm.set_trim_end_sec(m.trim_end_sec as f32);
        vm.set_position(m.position as f32);
        vm.set_playhead_fraction(m.position as f32);
        vm.set_is_video_selected(m.is_video_selected);
        vm.set_is_marking_cut(m.pending_cut_start.is_some());
        vm.set_audio_follows_video(m.audio_follows_video);

        vm.set_trim_start_display(format_time(m.trim_start_sec).into());
        vm.set_trim_end_display(format_time(m.trim_end_sec).into());
        vm.set_trim_duration_display(format_time((m.trim_end_sec - m.trim_start_sec).max(0.0)).into());

        vm.set_selected_aspect_index(m.selected_aspect_index as i32);
        vm.set_crop_tool_active(m.crop_tool_active);
        vm.set_selected_speed_index(m.selected_speed_index.map(|i| i as i32).unwrap_or(-1));
        vm.set_speed_text(m.speed_text.as_str().into());

        vm.set_selected_filter_index(m.selected_filter_index as i32);
        vm.set_color_brightness(m.color_brightness);
        vm.set_color_contrast(m.color_contrast);
        vm.set_color_saturation(m.color_saturation);

        self.sync_overlay_to_ui(&vm);
        self.sync_cuts_to_ui(&vm);
        self.sync_tracks_to_ui(&vm);
        self.sync_texts_to_ui(&vm);
    }

    pub(super) fn sync_overlay_to_ui(&self, vm: &EditorVM) {
        let m = &self.model;
        vm.set_display_source_aspect_raw(m.display_source_aspect_raw() as f32);
        vm.set_display_frame_aspect(m.display_frame_aspect() as f32);
        let (ex, ey, ew, eh) = m.display_edit_norm();
        vm.set_edit_nx(ex as f32);
        vm.set_edit_ny(ey as f32);
        vm.set_edit_nw(ew as f32);
        vm.set_edit_nh(eh as f32);
    }

    pub(super) fn sync_cuts_to_ui(&self, vm: &EditorVM) {
        let mut bands: Vec<EditorCutBand> = self
            .model
            .cuts
            .iter()
            .enumerate()
            .map(|(i, c)| EditorCutBand {
                start_sec: c.start_sec as f32,
                end_sec: c.end_sec as f32,
                is_pending: false,
                cut_index: i as i32,
            })
            .collect();
        if let Some(p) = self.model.pending_cut_start {
            let play_sec = self.model.position.clamp(0.0, 1.0) * self.model.duration_sec;
            bands.push(EditorCutBand {
                start_sec: p.min(play_sec) as f32,
                end_sec: p.max(play_sec) as f32,
                is_pending: true,
                cut_index: -1,
            });
        }
        vm.set_cut_bands(ModelRc::from(Rc::new(VecModel::from(bands))));
        vm.set_is_marking_cut(self.model.pending_cut_start.is_some());
    }

    pub(super) fn sync_tracks_to_ui(&self, vm: &EditorVM) {
        let list: Vec<EditorAudioTrackData> = self
            .model
            .audio_tracks
            .iter()
            .map(|t| EditorAudioTrackData {
                stream_index: t.stream_index as i32,
                name: t.name.as_str().into(),
                volume: t.volume,
                is_muted: t.is_muted,
                is_selected: t.is_selected,
                start_sec: t.start_sec as f32,
                end_sec: t.end_sec as f32,
                clip_duration_sec: t.clip_duration_sec as f32,
            })
            .collect();
        vm.set_audio_tracks(ModelRc::from(Rc::new(VecModel::from(list))));
    }

    pub(super) fn sync_texts_to_ui(&self, vm: &EditorVM) {
        let sel = self.model.selected_text_index;
        let items: Vec<EditorTextOverlayData> = self
            .model
            .texts
            .iter()
            .enumerate()
            .map(|(i, t)| EditorTextOverlayData {
                text: t.text.as_str().into(),
                nx: t.nx as f32,
                ny: t.ny as f32,
                font_percent: t.font_percent,
                text_color: parse_hex_color(&t.color),
                hex: t.color.as_str().into(),
                selected: sel == Some(i),
            })
            .collect();
        // rows updated in place: a fresh model rebuilt every caption (and the in-place edit field) per keystroke
        let current = vm.get_texts();
        match current.as_any().downcast_ref::<VecModel<EditorTextOverlayData>>() {
            Some(rows) if rows.row_count() == items.len() => {
                for (i, it) in items.into_iter().enumerate() {
                    if rows.row_data(i).as_ref() != Some(&it) {
                        rows.set_row_data(i, it);
                    }
                }
            }
            _ => vm.set_texts(ModelRc::from(Rc::new(VecModel::from(items)))),
        }
        vm.set_has_texts(!self.model.texts.is_empty());
        if let Some(t) = sel.and_then(|i| self.model.texts.get(i)) {
            vm.set_has_selected_text(true);
            vm.set_selected_text_content(t.text.as_str().into());
            vm.set_selected_text_font_percent(t.font_percent);
        } else {
            vm.set_has_selected_text(false);
            vm.set_selected_text_content("".into());
        }
    }

    pub(super) fn sync_trim_displays(&self, vm: &EditorVM) {
        if self.model.audio_follows_video {
            self.sync_tracks_to_ui(vm); // the lanes moved with the trim
        }
        vm.set_trim_start_sec(self.model.trim_start_sec as f32);
        vm.set_trim_end_sec(self.model.trim_end_sec as f32);
        vm.set_trim_start_display(format_time(self.model.trim_start_sec).into());
        vm.set_trim_end_display(format_time(self.model.trim_end_sec).into());
        vm.set_trim_duration_display(
            format_time((self.model.trim_end_sec - self.model.trim_start_sec).max(0.0)).into(),
        );
    }

    pub(super) fn queue_seek(&mut self, me: &Rc<RefCell<Self>>, pos: f64) {
        self.pending_seek = Some(pos.clamp(0.0, 1.0));
        if !self.seek_timer.running() {
            self.flush_pending_seek();
            let weak = Rc::downgrade(me);
            self.seek_timer.start(TimerMode::Repeated, Duration::from_millis(80), move || {
                if let Some(rc) = weak.upgrade() {
                    if let Ok(mut st) = rc.try_borrow_mut() {
                        if !st.flush_pending_seek() {
                            st.seek_timer.stop();
                        }
                    }
                }
            });
        }
    }

    pub(super) fn flush_pending_seek(&mut self) -> bool {
        if let Some(pos) = self.pending_seek.take() {
            let _ = self.commands.send(MpvCommand::Seek(pos, true));
            true
        } else {
            false
        }
    }

    pub(super) fn apply_preview_rate(&self) {
        let rate = self.model.effective_speed().clamp(0.25, 4.0);
        let _ = self.commands.send(MpvCommand::Speed(rate));
    }

    pub(super) fn apply_preview_vf(&self) {
        let look = build_preview_look(&self.model);
        let _ = self.commands.send(MpvCommand::Vf(look.vf));
        let _ = self.commands.send(MpvCommand::Look(look.shader_opts));
        let _ = self.commands.send(MpvCommand::Crop(look.crop));
        let _ = self.commands.send(MpvCommand::Rotate(look.rotate));
    }

    pub(super) fn apply_preview_audio(&self) {
        let (mute, stream_index, volume) = select_preview_audio(&self.model.audio_tracks);
        let _ = self.commands.send(MpvCommand::Audio(mute, stream_index, volume));
    }

    pub(super) fn apply_all_preview(&self) {
        self.apply_preview_vf();
        self.apply_preview_rate();
        self.apply_preview_audio();
    }

    pub(super) fn schedule_reassert(&self, me: &Rc<RefCell<Self>>) {
        let weak = Rc::downgrade(me);
        self.reapply_timer.start(TimerMode::SingleShot, Duration::from_millis(300), move || {
            if let Some(rc) = weak.upgrade() {
                if let Ok(st) = rc.try_borrow() {
                    st.apply_preview_vf();
                    st.apply_preview_rate();
                    st.apply_preview_audio();
                }
            }
        });
    }

    /// Opens one clip as a new edit (a montage of one segment) and plays it.
    pub(super) fn open_clip(&mut self, path: &str, duration_hint: f64) {
        let default_audio = tr("Editor_TrackAudio");
        self.model.reset_for_clip(path, duration_hint, &default_audio);
        self.montage = Montage::single(self.model.clone());
        self.play_through = false;
        if let Some(ui) = self.ui.upgrade() {
            let vm = ui.global::<EditorVM>();
            vm.set_export_status("".into());
            vm.set_export_progress(0);
        }
        self.load_live_edit(true);
    }

    pub(super) fn refresh_localized_options(&mut self) {
        let native = tr("Option_Native");
        self.model.aspect_options[0].display = native.clone();
        self.model.export_resolutions[0].display = native;
        match self.model.audio_tracks.len() {
            1 => self.model.audio_tracks[0].name = tr("Editor_TrackAudio"),
            2 => {
                self.model.audio_tracks[0].name = tr("Editor_TrackSystem");
                self.model.audio_tracks[1].name = tr("Editor_TrackMic");
            }
            _ => {}
        }
        if let Some(ui) = self.ui.upgrade() {
            let vm = ui.global::<EditorVM>();
            let aspect_items: Vec<ComboBoxItemData> = self
                .model
                .aspect_options
                .iter()
                .map(|o| ComboBoxItemData { text: o.display.as_str().into(), tier: 0 })
                .collect();
            vm.set_aspect_options(ModelRc::from(Rc::new(VecModel::from(aspect_items))));
            let res_items: Vec<ComboBoxItemData> = self
                .model
                .export_resolutions
                .iter()
                .map(|o| ComboBoxItemData { text: o.display.as_str().into(), tier: 0 })
                .collect();
            vm.set_export_resolution_options(ModelRc::from(Rc::new(VecModel::from(res_items))));
            self.sync_tracks_to_ui(&vm);
        }
    }

    pub(super) fn sync_clips_from_shared(&mut self, clips: &[crate::app_ctx::SharedClip]) {
        self.sidebar_durations.clear();
        let items: Vec<EditorClipItem> = editable_clips(clips)
            .into_iter()
            .map(|c| {
                let dur = (c.duration_ticks.max(0) as f64) / 10_000_000.0;
                if dur > 0.0 {
                    self.sidebar_durations.insert(c.file_path.clone(), dur);
                }
                let has_thumbnail = c.thumbnail.is_some();
                EditorClipItem {
                    path: c.file_path.as_str().into(),
                    title: c.display_name().into(),
                    meta: c.short_meta().into(),
                    thumbnail: c.thumbnail.as_ref().map(|b| Image::from_rgba8(b.clone())).unwrap_or_default(),
                    has_thumbnail,
                    merge_number: self.merge.number_of(&c.file_path),
                }
            })
            .collect();
        if let Some(ui) = self.ui.upgrade() {
            ui.global::<EditorVM>().set_clips(ModelRc::from(Rc::new(VecModel::from(items))));
        }
    }
}

/// Options that never change while the app runs (frame shapes, speeds, filters, export sizes and formats).
pub(super) fn fill_static_options(vm: &EditorVM, model: &EditModel) {
    let aspect_items: Vec<ComboBoxItemData> =
        model.aspect_options.iter().map(|o| ComboBoxItemData { text: o.display.as_str().into(), tier: 0 }).collect();
    vm.set_aspect_options(ModelRc::from(Rc::new(VecModel::from(aspect_items))));

    let speed_items: Vec<SegmentedOption> =
        model.speed_options.iter().map(|o| SegmentedOption { text: o.display.into(), tier: 0 }).collect();
    vm.set_speed_options(ModelRc::from(Rc::new(VecModel::from(speed_items))));

    let filter_items: Vec<EditorFilterData> = model
        .filter_options
        .iter()
        .map(|f| EditorFilterData { name: f.name.into(), preview: Image::default(), has_preview: false })
        .collect();
    vm.set_filter_options(ModelRc::from(Rc::new(VecModel::from(filter_items))));

    let res_items: Vec<ComboBoxItemData> = model
        .export_resolutions
        .iter()
        .map(|r| ComboBoxItemData { text: r.display.as_str().into(), tier: 0 })
        .collect();
    vm.set_export_resolution_options(ModelRc::from(Rc::new(VecModel::from(res_items))));

    let fmt_items: Vec<ComboBoxItemData> =
        EXPORT_FORMATS.iter().map(|&f| ComboBoxItemData { text: f.into(), tier: 0 }).collect();
    vm.set_export_format_options(ModelRc::from(Rc::new(VecModel::from(fmt_items))));
}
