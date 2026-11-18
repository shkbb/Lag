//! `LibraryState`: the page's state on the UI thread and everything that pushes it into `LibraryVM`.

use super::{clip::*, filter::*, grouping::*, layout::*, scan::*, storage::*, thumbnails::*};
use crate::app_ctx::AppCtx;
use crate::{GameChipData, LibraryCardData, LibraryRowData, LibraryVM};
use chrono::Local;
use lag_core::metadata::ClipMetadataStore;
use slint::{ComponentHandle, Image, Model, ModelRc, SharedString, VecModel};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

thread_local! {
    static STATE_REGISTRY: RefCell<HashMap<u64, std::rc::Weak<RefCell<LibraryState>>>> = RefCell::default();
    static NEXT_STATE_ID: std::cell::Cell<u64> = const { std::cell::Cell::new(1) };
}

pub(super) fn register_state(state: &Rc<RefCell<LibraryState>>) -> u64 {
    let id = NEXT_STATE_ID.with(|n| {
        let id = n.get();
        n.set(id + 1);
        id
    });
    state.borrow_mut().registry_id = id;
    STATE_REGISTRY.with(|r| {
        r.borrow_mut().insert(id, Rc::downgrade(state));
    });
    id
}

pub(super) fn with_registered_state(id: u64, f: impl FnOnce(&mut LibraryState)) {
    let weak = STATE_REGISTRY.with(|r| r.borrow().get(&id).cloned());
    if let Some(st) = weak.and_then(|w| w.upgrade()) {
        if let Ok(mut guard) = st.try_borrow_mut() {
            f(&mut guard);
        }
    }
}

pub(super) struct LibraryState {
    pub(super) registry_id: u64,
    pub(super) ctx: Rc<AppCtx>,
    pub(super) ui: slint::Weak<crate::AppWindow>,
    pub(super) meta_store: Option<ClipMetadataStore>,
    pub(super) thumb_cache_dir: PathBuf,
    pub(super) mem_cache: Arc<Mutex<ThumbnailMemCache>>,
    pub(super) refresh_gen: Arc<AtomicU64>,
    pub(super) published_signature: Option<String>,

    pub(super) clips: Vec<ClipEntry>,
    pub(super) clip_path_to_idx: HashMap<String, usize>,
    pub(super) search_text: String,
    pub(super) favorites_only: bool,
    pub(super) game_filter: Option<String>,
    pub(super) selection_anchor: Option<String>,

    pub(super) groups: Vec<DayGroup>,
    pub(super) virtual_rows: Vec<VirtualRow>,
    pub(super) total_content_height: f32,

    pub(super) scroll_y: f32,
    pub(super) viewport_w: f32,
    pub(super) viewport_h: f32,
    pub(super) cols: usize,

    pub(super) visible_rows_model: Rc<VecModel<LibraryRowData>>,
    pub(super) visible_window: (usize, usize),
    pub(super) visible_card_models: HashMap<usize, Rc<VecModel<LibraryCardData>>>,
    pub(super) clip_to_visible_pos: HashMap<usize, (usize, usize)>,
}

impl LibraryState {
    pub(super) fn new(ui: &crate::AppWindow, ctx: Rc<AppCtx>) -> Self {
        let thumb_cache_dir = default_thumbnail_cache_dir().unwrap_or_else(|| PathBuf::from("Lag").join("thumbnails"));
        let visible_rows_model = Rc::new(VecModel::default());
        ui.global::<LibraryVM>().set_visible_rows(ModelRc::from(visible_rows_model.clone()));

        Self {
            registry_id: 0,
            ctx,
            ui: ui.as_weak(),
            meta_store: ClipMetadataStore::for_user(),
            thumb_cache_dir,
            mem_cache: Arc::new(Mutex::new(ThumbnailMemCache::default())),
            refresh_gen: Arc::new(AtomicU64::new(0)),
            published_signature: None,
            clips: Vec::new(),
            clip_path_to_idx: HashMap::new(),
            search_text: String::new(),
            favorites_only: false,
            game_filter: None,
            selection_anchor: None,
            groups: Vec::new(),
            virtual_rows: Vec::new(),
            total_content_height: 0.0,
            scroll_y: 0.0,
            viewport_w: 1200.0,
            viewport_h: 700.0,
            cols: columns_for_viewport(1200.0),
            visible_rows_model,
            visible_window: (0, 0),
            visible_card_models: HashMap::new(),
            clip_to_visible_pos: HashMap::new(),
        }
    }

    pub(super) fn publish_clips(&self) {
        let shared: Vec<crate::app_ctx::SharedClip> = self.clips.iter().map(Into::into).collect();
        self.ctx.set_clips(shared);
    }

    pub(super) fn rebuild_path_index(&mut self) {
        self.clip_path_to_idx.clear();
        for (i, c) in self.clips.iter().enumerate() {
            self.clip_path_to_idx.insert(c.file_path.clone(), i);
        }
    }

    pub(super) fn to_card_data(&self, clip_idx: usize) -> LibraryCardData {
        let c = &self.clips[clip_idx];
        let (thumbnail, has_thumbnail) = match &c.thumbnail {
            Some(buf) => (Image::from_rgba8(buf.clone()), true),
            None => (Image::default(), false),
        };
        LibraryCardData {
            clip_index: clip_idx as i32,
            file_path: SharedString::from(c.file_path.as_str()),
            title: SharedString::from(c.display_name()),
            time_text: SharedString::from(c.time_hh_mm()),
            game_name: SharedString::from(c.game.as_deref().unwrap_or("")),
            duration_text: SharedString::from(if c.is_image { String::new() } else { c.duration_display() }),
            thumbnail,
            has_thumbnail,
            is_image: c.is_image,
            is_favorite: c.is_favorite,
            is_edited: c.is_edited,
            is_selected: c.is_selected,
        }
    }

    pub(super) fn build_slint_row(&mut self, vrow_idx: usize) -> LibraryRowData {
        let row = &self.virtual_rows[vrow_idx];
        match &row.kind {
            VirtualRowKind::Header { label, sub, count } => LibraryRowData {
                kind: 0,
                y: row.y,
                height: row.height,
                header_label: SharedString::from(label.as_str()),
                header_sub: SharedString::from(sub.as_str()),
                header_count: SharedString::from(count.as_str()),
                cards: ModelRc::default(),
            },
            VirtualRowKind::Cards { clip_indices } => {
                let cards_vec: Vec<LibraryCardData> = clip_indices.iter().map(|&idx| self.to_card_data(idx)).collect();
                let cards_model = Rc::new(VecModel::from(cards_vec));
                self.visible_card_models.insert(vrow_idx, cards_model.clone());
                LibraryRowData {
                    kind: 1,
                    y: row.y,
                    height: row.height,
                    header_label: SharedString::default(),
                    header_sub: SharedString::default(),
                    header_count: SharedString::default(),
                    cards: ModelRc::from(cards_model),
                }
            }
        }
    }

    pub(super) fn rebuild_clip_to_visible_pos(&mut self) {
        self.clip_to_visible_pos.clear();
        let (first, last) = self.visible_window;
        for vrow_idx in first..last {
            if let Some(VirtualRow { kind: VirtualRowKind::Cards { clip_indices }, .. }) =
                self.virtual_rows.get(vrow_idx)
            {
                for (col_idx, &clip_idx) in clip_indices.iter().enumerate() {
                    self.clip_to_visible_pos.insert(clip_idx, (vrow_idx, col_idx));
                }
            }
        }
    }

    pub(super) fn sync_visible_window(&mut self, force_rebuild: bool) {
        let (new_first, new_last) = visible_row_range(&self.virtual_rows, self.scroll_y, self.viewport_h);
        let (mut cur_first, mut cur_last) = self.visible_window;

        if !force_rebuild && (new_first, new_last) == (cur_first, cur_last) {
            return;
        }

        if force_rebuild || new_last <= cur_first || new_first >= cur_last || cur_first >= cur_last {
            self.visible_card_models.clear();
            let mut items = Vec::with_capacity(new_last.saturating_sub(new_first));
            for vrow_idx in new_first..new_last {
                items.push(self.build_slint_row(vrow_idx));
            }
            self.visible_rows_model.set_vec(items);
            self.visible_window = (new_first, new_last);
            self.rebuild_clip_to_visible_pos();
            return;
        }

        // Diff overlapping range so remaining rows stay intact in Slint without flicker.
        while cur_first < new_first && cur_first < cur_last {
            self.visible_rows_model.remove(0);
            self.visible_card_models.remove(&cur_first);
            cur_first += 1;
        }
        while cur_last > new_last && cur_last > cur_first {
            cur_last -= 1;
            self.visible_rows_model.remove(cur_last - cur_first);
            self.visible_card_models.remove(&cur_last);
        }
        if new_first < cur_first {
            for vrow_idx in (new_first..cur_first).rev() {
                let row_data = self.build_slint_row(vrow_idx);
                self.visible_rows_model.insert(0, row_data);
            }
        }
        if new_last > cur_last {
            for vrow_idx in cur_last..new_last {
                let row_data = self.build_slint_row(vrow_idx);
                self.visible_rows_model.push(row_data);
            }
        }
        self.visible_window = (new_first, new_last);
        self.rebuild_clip_to_visible_pos();
    }

    pub(super) fn update_visible_card(&self, clip_idx: usize) {
        if let Some(&(vrow_idx, col_idx)) = self.clip_to_visible_pos.get(&clip_idx) {
            if let Some(model) = self.visible_card_models.get(&vrow_idx) {
                model.set_row_data(col_idx, self.to_card_data(clip_idx));
            }
        }
    }

    pub(super) fn refresh_all_visible_cards(&self) {
        for (&clip_idx, &(vrow_idx, col_idx)) in &self.clip_to_visible_pos {
            if let Some(model) = self.visible_card_models.get(&vrow_idx) {
                model.set_row_data(col_idx, self.to_card_data(clip_idx));
            }
        }
    }

    pub(super) fn rebuild_game_chips(&mut self, ui: &crate::AppWindow) {
        let desktop_label = lag_core::i18n::tr("Library_Desktop");
        let chips = build_game_chips(&self.clips, &mut self.game_filter, desktop_label);
        let slint_chips: Vec<GameChipData> = chips
            .into_iter()
            .map(|c| GameChipData {
                label: SharedString::from(c.label),
                filter_key: SharedString::from(c.filter_key),
                is_active: c.is_active,
            })
            .collect();
        ui.global::<LibraryVM>().set_game_chips(ModelRc::from(Rc::new(VecModel::from(slint_chips))));
    }

    pub(super) fn rebuild_groups(&mut self, ui: &crate::AppWindow) {
        let filtered = filter_clips(&self.clips, self.favorites_only, self.game_filter.as_deref(), &self.search_text);
        let today = Local::now().date_naive();
        let fmt = Win32DateFormatter;
        self.groups = group_clips_by_day(&filtered, today, &fmt);
        let (vrows, total_h) = layout_virtual_rows(&self.groups, self.cols);
        self.virtual_rows = vrows;
        self.total_content_height = total_h;
        self.scroll_y = self.scroll_y.clamp(0.0, (total_h - self.viewport_h).max(0.0));

        let vm = ui.global::<LibraryVM>();
        vm.set_total_content_height(total_h);
        vm.set_has_clips(!self.clips.is_empty());
        vm.set_has_no_results(!self.clips.is_empty() && self.groups.is_empty());

        self.sync_visible_window(true);
    }

    pub(super) fn rebuild_view(&mut self, ui: &crate::AppWindow) {
        self.rebuild_game_chips(ui);
        self.rebuild_groups(ui);
        let count_str = self.clips.len().to_string();
        let label =
            lag_core::i18n::format("Library_ClipCount", &[&count_str]).unwrap_or_else(|_| format!("{count_str} CLIPS"));
        ui.global::<LibraryVM>().set_clip_count_label(label.into());
    }

    pub(super) fn notify_selection_changed(&self, ui: &crate::AppWindow) {
        let selected_count = self.clips.iter().filter(|c| c.is_selected).count();
        let has_sel = selected_count > 0;
        let vm = ui.global::<LibraryVM>();
        vm.set_has_selection(has_sel);
        let del_word = lag_core::i18n::tr("Library_Delete");
        vm.set_delete_selected_label(format!("{del_word} ({selected_count})").into());
    }

    pub(super) fn clear_selection(&mut self, ui: &crate::AppWindow) {
        let mut any = false;
        for c in &mut self.clips {
            if c.is_selected {
                c.is_selected = false;
                any = true;
            }
        }
        self.selection_anchor = None;
        if any {
            self.refresh_all_visible_cards();
        }
        self.notify_selection_changed(ui);
    }

    pub(super) fn toggle_select_clip(&mut self, ui: &crate::AppWindow, clip_idx: usize) {
        let Some(c) = self.clips.get_mut(clip_idx) else {
            return;
        };
        c.is_selected = !c.is_selected;
        if c.is_selected {
            self.selection_anchor = Some(c.file_path.clone());
        }
        self.update_visible_card(clip_idx);
        self.notify_selection_changed(ui);
    }

    pub(super) fn visible_clip_order(&self) -> Vec<usize> {
        let mut order = Vec::new();
        for g in &self.groups {
            order.extend_from_slice(&g.clip_indices);
        }
        order
    }

    pub(super) fn select_range(&mut self, ui: &crate::AppWindow, target_idx: usize) {
        let order = self.visible_clip_order();
        let Some(to) = order.iter().position(|&idx| idx == target_idx) else {
            return;
        };
        let from = self
            .selection_anchor
            .as_deref()
            .and_then(|anchor_path| self.clip_path_to_idx.get(anchor_path).copied())
            .and_then(|anchor_idx| order.iter().position(|&idx| idx == anchor_idx));

        let Some(from) = from else {
            if let Some(c) = self.clips.get_mut(target_idx) {
                c.is_selected = true;
                self.selection_anchor = Some(c.file_path.clone());
            }
            self.update_visible_card(target_idx);
            self.notify_selection_changed(ui);
            return;
        };

        let (lo, hi) = (from.min(to), from.max(to));
        for (pos, &idx) in order.iter().enumerate() {
            if let Some(c) = self.clips.get_mut(idx) {
                c.is_selected = pos >= lo && pos <= hi;
            }
        }
        self.refresh_all_visible_cards();
        self.notify_selection_changed(ui);
    }

    pub(super) fn select_all(&mut self, ui: &crate::AppWindow) {
        for idx in self.visible_clip_order() {
            if let Some(c) = self.clips.get_mut(idx) {
                c.is_selected = true;
            }
        }
        self.refresh_all_visible_cards();
        self.notify_selection_changed(ui);
    }

    pub(super) fn toggle_favorite(&mut self, ui: &crate::AppWindow, clip_idx: usize) {
        let Some(c) = self.clips.get_mut(clip_idx) else {
            return;
        };
        c.is_favorite = !c.is_favorite;
        let path = c.file_path.clone();
        let fav = c.is_favorite;
        if let Some(ref store) = self.meta_store {
            store.set_favorite(&path, fav);
        }
        if self.favorites_only {
            self.rebuild_groups(ui);
        } else {
            self.update_visible_card(clip_idx);
        }
    }

    pub(super) fn delete_clip_by_path(&mut self, file_path: &str) -> bool {
        let Some(&idx) = self.clip_path_to_idx.get(file_path) else {
            return false;
        };
        let thumb_path = self.clips[idx].thumbnail_path.clone();
        let res: std::io::Result<()> = (|| {
            if Path::new(file_path).exists() {
                std::fs::remove_file(file_path)?;
            }
            if Path::new(&thumb_path).exists() {
                std::fs::remove_file(&thumb_path)?;
            }
            if let Some(ref store) = self.meta_store {
                store.delete(file_path);
            }
            Ok(())
        })();
        if let Err(e) = res {
            log::debug!("Failed to delete clip: {e}");
            return false;
        }
        self.clips.remove(idx);
        self.rebuild_path_index();
        true
    }

    pub(super) fn delete_single(&mut self, ui: &crate::AppWindow, file_path: &str) {
        if self.delete_clip_by_path(file_path) {
            self.rebuild_view(ui);
            self.publish_clips();
        }
        self.notify_selection_changed(ui);
    }

    pub(super) fn delete_selected(&mut self, ui: &crate::AppWindow) {
        let targets: Vec<String> = self.clips.iter().filter(|c| c.is_selected).map(|c| c.file_path.clone()).collect();
        let mut deleted_any = false;
        for path in targets {
            if self.delete_clip_by_path(&path) {
                deleted_any = true;
            }
        }
        self.selection_anchor = None;
        if deleted_any {
            self.rebuild_view(ui);
            self.publish_clips();
        }
        self.notify_selection_changed(ui);
    }

    pub(super) fn apply_hydration_batch(&mut self, ui: &crate::AppWindow, batch: Vec<HydrateResult>) {
        let _ = ui;
        let mut any_changed = false;
        for item in batch {
            let Some(&idx) = self.clip_path_to_idx.get(&item.file_path) else {
                continue;
            };
            let c = &mut self.clips[idx];
            let mut changed = false;
            if let Some(buf) = item.buffer {
                c.thumbnail = Some(buf);
                changed = true;
            }
            if item.duration_ticks > 0 && c.duration_ticks != item.duration_ticks {
                c.duration_ticks = item.duration_ticks;
                changed = true;
            }
            if changed {
                any_changed = true;
                self.update_visible_card(idx);
            }
        }
        if any_changed {
            self.publish_clips();
        }
    }

    pub(super) fn on_viewport_changed(&mut self, scroll_y: f32, viewport_w: f32, viewport_h: f32) {
        let new_w = viewport_w.max(300.0);
        let new_h = viewport_h.max(200.0);
        let new_cols = columns_for_viewport(new_w);
        self.scroll_y = scroll_y.max(0.0);
        self.viewport_w = new_w;
        self.viewport_h = new_h;

        if new_cols != self.cols {
            self.cols = new_cols;
            let (vrows, total_h) = layout_virtual_rows(&self.groups, self.cols);
            self.virtual_rows = vrows;
            self.total_content_height = total_h;
            if let Some(ui) = self.ui.upgrade() {
                ui.global::<LibraryVM>().set_total_content_height(total_h);
            }
            self.sync_visible_window(true);
        } else {
            self.sync_visible_window(false);
        }
    }

    pub(super) fn update_free_space(ui: &crate::AppWindow, library_dir: &Path) {
        if let Some((used_display, bar_width)) = query_drive_usage_display(library_dir) {
            let vm = ui.global::<LibraryVM>();
            vm.set_used_space_display(used_display.into());
            vm.set_used_space_bar_width(bar_width);
        }
    }

    pub(super) fn start_refresh(state_rc: &Rc<RefCell<Self>>) {
        let (my_gen, refresh_gen, settings, meta_store, thumb_cache_dir, mem_cache, ui_weak, state_id) = {
            let st = state_rc.borrow();
            let my_gen = st.refresh_gen.fetch_add(1, Ordering::AcqRel) + 1;
            (
                my_gen,
                Arc::clone(&st.refresh_gen),
                st.ctx.settings(),
                st.meta_store.clone(),
                st.thumb_cache_dir.clone(),
                Arc::clone(&st.mem_cache),
                st.ui.clone(),
                st.registry_id,
            )
        };

        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<LibraryVM>().set_is_loading(true);
        }

        let library_dir = resolve_library_dir(&settings);
        let auto_cleanup = settings.auto_cleanup_enabled;
        let max_gb = settings.max_library_size_gb;

        let _ = std::thread::Builder::new().name("lag-lib-scan".into()).spawn(move || {
            let canceled = || refresh_gen.load(Ordering::Acquire) != my_gen;
            let scan_res =
                scan_library_phase1(&library_dir, meta_store.as_ref(), &thumb_cache_dir, &mem_cache, &canceled);
            if canceled() {
                return;
            }
            let (built, signature) = match scan_res {
                Ok(v) => v,
                Err(e) => {
                    log::debug!("[Library] scan failed: {e}");
                    let _ = slint::invoke_from_event_loop(move || {
                        if refresh_gen.load(Ordering::Acquire) == my_gen {
                            if let Some(ui) = ui_weak.upgrade() {
                                LibraryState::update_free_space(&ui, &library_dir);
                                ui.global::<LibraryVM>().set_is_loading(false);
                            }
                        }
                    });
                    return;
                }
            };

            let _ = slint::invoke_from_event_loop(move || {
                if refresh_gen.load(Ordering::Acquire) != my_gen {
                    return;
                }
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                let weak_st = STATE_REGISTRY.with(|r| r.borrow().get(&state_id).cloned());
                let Some(st_rc) = weak_st.and_then(|w| w.upgrade()) else {
                    return;
                };

                let tasks_and_hk: Option<(Vec<HydrateTask>, bool)> = {
                    let mut st = st_rc.borrow_mut();
                    let unchanged = st.published_signature.as_deref().is_some_and(|prev| prev == signature)
                        && st.clips.len() == built.len();

                    if unchanged {
                        let pending: Vec<HydrateTask> = st
                            .clips
                            .iter()
                            .filter(|c| c.thumbnail.is_none() || (!c.is_image && c.duration_ticks <= 0))
                            .map(|c| HydrateTask {
                                file_path: c.file_path.clone(),
                                thumbnail_path: c.thumbnail_path.clone(),
                                is_image: c.is_image,
                                duration_ticks: c.duration_ticks,
                                has_thumbnail: c.thumbnail.is_some(),
                            })
                            .collect();
                        LibraryState::update_free_space(&ui, &library_dir);
                        ui.global::<LibraryVM>().set_is_loading(false);
                        if pending.is_empty() {
                            None
                        } else {
                            Some((pending, false))
                        }
                    } else {
                        let tasks: Vec<HydrateTask> = built
                            .iter()
                            .map(|c| HydrateTask {
                                file_path: c.file_path.clone(),
                                thumbnail_path: c.thumbnail_path.clone(),
                                is_image: c.is_image,
                                duration_ticks: c.duration_ticks,
                                has_thumbnail: c.thumbnail.is_some(),
                            })
                            .collect();
                        st.clips = built;
                        st.rebuild_path_index();
                        st.selection_anchor = None;
                        st.published_signature = Some(signature);
                        st.rebuild_view(&ui);
                        st.publish_clips();
                        LibraryState::update_free_space(&ui, &library_dir);
                        st.notify_selection_changed(&ui);
                        ui.global::<LibraryVM>().set_is_loading(false);
                        Some((tasks, true))
                    }
                };

                if let Some((tasks, run_housekeeping)) = tasks_and_hk {
                    start_phase2_hydration(
                        HydrationJob {
                            my_gen,
                            refresh_gen,
                            library_dir,
                            thumb_cache_dir,
                            meta_store,
                            auto_cleanup,
                            max_gb,
                            run_housekeeping,
                            mem_cache,
                            tasks,
                        },
                        ui_weak,
                        st_rc,
                    );
                }
            });
        });
    }
}
