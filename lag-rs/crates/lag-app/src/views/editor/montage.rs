//! Montage: an ordered list of segments, each one clip with its own complete edit (trim, cuts, speed, crop/reframe,
//! rotation, look, captions, audio). Owner, 2026-11-18: "edit and trim N videos at once, then join everything into
//! one - take highlights apart and cut them together, with the freedom of Premiere".
//!
//! The editor always edits ONE `EditModel`: the selected segment's, checked out into the runtime. `commit` writes it
//! back before any operation that reads the other segments. A single clip is simply a montage of one segment.
//! Pure data, no UI and no FFmpeg (unit-tested).

use super::model::{EditModel, MIN_TRIM_GAP_SEC};

/// One clip of the montage with its own edit.
#[derive(Debug, Clone)]
pub struct Segment {
    /// Stable identity (the strip keeps selection and drag state by it, not by position).
    pub id: u64,
    pub edit: EditModel,
}

impl Segment {
    /// The parts of the source this segment plays, in source seconds: the trim range minus the cuts.
    pub fn pieces(&self) -> Vec<(f64, f64)> {
        let e = &self.edit;
        let start = e.trim_start_sec.clamp(0.0, e.duration_sec.max(0.0));
        let end = e.trim_end_sec.min(e.duration_sec).max(start);
        let kept = e.kept_segments(start, end);
        if kept.is_empty() {
            vec![(start, end.max(start + MIN_TRIM_GAP_SEC))]
        } else {
            kept
        }
    }

    /// How long the segment lasts in the montage (after cuts and speed).
    pub fn duration(&self) -> f64 {
        let source: f64 = self.pieces().iter().map(|(a, b)| b - a).sum();
        source / self.edit.effective_speed().max(0.01)
    }
}

/// One step back for montage-level operations (add, remove, move, split).
#[derive(Debug, Clone)]
struct MontageSnap {
    segments: Vec<Segment>,
    selected: usize,
    /// Edits made inside a segment after this snapshot are undone first (see `undo_is_montage`).
    edit_undo_len: usize,
}

#[derive(Debug, Clone)]
pub struct Montage {
    segments: Vec<Segment>,
    selected: usize,
    next_id: u64,
    undo: Vec<MontageSnap>,
}

impl Montage {
    /// A montage of one clip (what the editor shows for a single clip).
    pub fn single(edit: EditModel) -> Self {
        Self::from_edits(vec![edit])
    }

    /// Clips in this order; the first is selected. Never empty (an empty list gives one default segment).
    pub fn from_edits(edits: Vec<EditModel>) -> Self {
        let mut m = Self { segments: Vec::new(), selected: 0, next_id: 1, undo: Vec::new() };
        for edit in edits {
            let id = m.take_id();
            m.segments.push(Segment { id, edit });
        }
        if m.segments.is_empty() {
            let id = m.take_id();
            m.segments.push(Segment { id, edit: EditModel::new("") });
        }
        m
    }

    fn take_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn segments(&self) -> &[Segment] {
        &self.segments
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }

    /// More than one segment: the strip is shown and playback runs on into the next segment.
    pub fn is_montage(&self) -> bool {
        self.segments.len() > 1
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn selected_edit(&self) -> &EditModel {
        &self.segments[self.selected].edit
    }

    /// Writes the editor's live model back into the selected segment.
    pub fn commit(&mut self, live: &EditModel) {
        self.segments[self.selected].edit = live.clone();
    }

    /// Selects segment `index` (after `commit`); returns its edit for the editor.
    pub fn select(&mut self, index: usize) -> Option<&EditModel> {
        let seg = self.segments.get(index)?;
        self.selected = index;
        Some(&seg.edit)
    }

    /// Total length of the montage.
    pub fn duration(&self) -> f64 {
        self.segments.iter().map(Segment::duration).sum()
    }

    /// Where segment `index` starts in the montage.
    pub fn start_of(&self, index: usize) -> f64 {
        self.segments[..index.min(self.segments.len())].iter().map(Segment::duration).sum()
    }

    fn snapshot(&mut self, live_undo_len: usize) {
        self.undo.push(MontageSnap {
            segments: self.segments.clone(),
            selected: self.selected,
            edit_undo_len: live_undo_len,
        });
    }

    /// Adds a clip right after the selected segment and selects it. `live_undo_len` = the selected segment's undo
    /// depth now (so Ctrl+Z order stays chronological).
    pub fn add_after_selected(&mut self, edit: EditModel, live_undo_len: usize) -> usize {
        self.snapshot(live_undo_len);
        let id = self.take_id();
        let at = (self.selected + 1).min(self.segments.len());
        self.segments.insert(at, Segment { id, edit });
        self.selected = at;
        at
    }

    /// Removes segment `index` (the last remaining one stays). The neighbour that takes its place is selected.
    pub fn remove(&mut self, index: usize, live_undo_len: usize) -> bool {
        if self.segments.len() <= 1 || index >= self.segments.len() {
            return false;
        }
        self.snapshot(live_undo_len);
        self.segments.remove(index);
        self.selected = index.min(self.segments.len() - 1);
        true
    }

    /// Moves segment `from` to position `to` (positions after the removal); the moved segment stays selected.
    pub fn move_segment(&mut self, from: usize, to: usize, live_undo_len: usize) -> bool {
        let n = self.segments.len();
        if from >= n || to >= n || from == to {
            return false;
        }
        self.snapshot(live_undo_len);
        let seg = self.segments.remove(from);
        self.segments.insert(to, seg);
        self.selected = to;
        true
    }

    /// Splits segment `index` at source time `at` into two segments with the same edit: the left one ends at `at`,
    /// the right one starts there; cuts go to the side they are on (a cut across `at` is split too). Both halves
    /// start with an empty edit history. The right half is selected (the playhead stays on it).
    pub fn split(&mut self, index: usize, at: f64, live_undo_len: usize) -> bool {
        let Some(seg) = self.segments.get(index) else { return false };
        let e = seg.edit.clone();
        if at <= e.trim_start_sec + MIN_TRIM_GAP_SEC || at >= e.trim_end_sec - MIN_TRIM_GAP_SEC {
            return false;
        }
        self.snapshot(live_undo_len);
        let mut left = e.clone();
        let mut right = e.clone();
        left.trim_end_sec = at;
        right.trim_start_sec = at;
        left.cuts = e
            .cuts
            .iter()
            .filter(|c| c.start_sec < at)
            .map(|c| {
                let mut c = *c;
                c.end_sec = c.end_sec.min(at);
                c
            })
            .collect();
        right.cuts = e
            .cuts
            .iter()
            .filter(|c| c.end_sec > at)
            .map(|c| {
                let mut c = *c;
                c.start_sec = c.start_sec.max(at);
                c
            })
            .collect();
        for half in [&mut left, &mut right] {
            half.undo_stack.clear();
            half.pending_cut_start = None;
            half.link_audio_to_trim();
        }
        let id = self.take_id();
        self.segments[index].edit = left;
        self.segments.insert(index + 1, Segment { id, edit: right });
        self.selected = index + 1;
        true
    }

    /// Ctrl+Z belongs to the montage when its last operation is newer than every edit made inside the selected
    /// segment since (`live_undo_len` = that segment's undo depth now).
    pub fn undo_is_montage(&self, live_undo_len: usize) -> bool {
        self.undo.last().is_some_and(|s| live_undo_len <= s.edit_undo_len)
    }

    /// Restores the montage before its last operation.
    pub fn undo(&mut self) -> bool {
        let Some(s) = self.undo.pop() else { return false };
        self.segments = s.segments;
        self.selected = s.selected.min(self.segments.len() - 1);
        true
    }

    /// Applies one frame shape (aspect option) to every segment: the montage is one video, its shape is shared
    /// (like a sequence setting in an NLE). Each segment re-centres its picture in the new frame.
    pub fn set_aspect_everywhere(&mut self, aspect_index: usize) {
        for seg in &mut self.segments {
            let e = &mut seg.edit;
            e.selected_aspect_index = aspect_index.min(e.aspect_options.len().saturating_sub(1));
            e.reset_reframe();
        }
    }
}

/// The clip is used whole and unchanged (no trim, cuts, speed, frame, crop, rotation, look, captions or volume
/// changes): a montage of such clips can be joined without re-encoding.
pub fn is_untouched(e: &EditModel) -> bool {
    e.trim_start_sec <= 0.05
        && e.trim_end_sec >= e.duration_sec - 0.05
        && e.cuts.is_empty()
        && (e.effective_speed() - 1.0).abs() < 1e-6
        && e.selected_aspect_index == 0
        && !e.is_cropped()
        && !e.is_reframed()
        && e.rotation == 0
        && !e.flip_h
        && !e.flip_v
        && e.selected_filter_index == 0
        && !e.has_color_adjust()
        && e.texts.iter().all(|t| t.text.trim().is_empty())
        && e.audio_tracks.iter().all(|t| t.volume == 100 && !t.is_muted)
}

/// Width of each block in the montage strip: proportional to its duration, at least `min_w`. The strip draws
/// the same formula (montage_strip.slint), so a drag offset maps to the same positions here.
pub fn block_widths(durations: &[f64], strip_w: f64, min_w: f64, gap: f64) -> Vec<f64> {
    let total: f64 = durations.iter().sum();
    let avail = (strip_w - gap * durations.len().saturating_sub(1) as f64).max(0.0);
    durations.iter().map(|d| if total > 0.0 { (d / total * avail).max(min_w) } else { min_w }).collect()
}

/// Where a block dragged by `dx` pixels lands: the index it takes after the move (`Montage::move_segment` `to`).
pub fn drop_index(widths: &[f64], from: usize, dx: f64, gap: f64) -> usize {
    let mut x = 0.0;
    let centers: Vec<f64> = widths
        .iter()
        .map(|w| {
            let c = x + w / 2.0;
            x += w + gap;
            c
        })
        .collect();
    let Some(&c) = centers.get(from) else { return from };
    let moved = c + dx;
    centers.iter().enumerate().filter(|&(i, &other)| i != from && other < moved).count()
}

impl Montage {
    /// Every segment is an untouched clip: the export can join the files without re-encoding.
    pub fn is_plain_join(&self) -> bool {
        self.segments.iter().all(|s| is_untouched(&s.edit))
    }

    /// Durations of the segments in order (the strip's block sizes).
    pub fn durations(&self) -> Vec<f64> {
        self.segments.iter().map(Segment::duration).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::views::editor::model::Cut;

    #[test]
    fn plain_join_only_for_untouched_clips() {
        let mut m = Montage::from_edits(vec![clip("a.mp4", 10.0), clip("b.mp4", 10.0)]);
        assert!(m.is_plain_join());
        let mut b = m.segments()[1].edit.clone();
        b.trim_end_sec = 8.0;
        m.select(1);
        m.commit(&b);
        assert!(!m.is_plain_join());
    }

    #[test]
    fn dragging_a_block_lands_where_its_centre_goes() {
        let w = block_widths(&[10.0, 10.0, 20.0], 412.0, 72.0, 6.0);
        assert_eq!(w, vec![100.0, 100.0, 200.0]);
        assert_eq!(drop_index(&w, 0, 0.0, 6.0), 0);
        // centres 50 / 156 / 312: block 0 moved by 60 (centre 110) stays, by 120 (centre 170) passes block 1
        assert_eq!(drop_index(&w, 0, 60.0, 6.0), 0);
        assert_eq!(drop_index(&w, 0, 120.0, 6.0), 1);
        assert_eq!(drop_index(&w, 0, 400.0, 6.0), 2);
        assert_eq!(drop_index(&w, 2, -400.0, 6.0), 0);
        // tiny clips keep a usable width
        assert_eq!(block_widths(&[1.0, 99.0], 300.0, 72.0, 0.0)[0], 72.0);
    }

    fn clip(path: &str, dur: f64) -> EditModel {
        let mut m = EditModel::new("Native");
        m.reset_for_clip(path, dur, "Audio");
        m.source_width = 1920;
        m.source_height = 1080;
        m
    }

    #[test]
    fn durations_follow_trim_cuts_and_speed() {
        let mut a = clip("a.mp4", 30.0);
        a.trim_start_sec = 5.0;
        a.trim_end_sec = 25.0;
        a.cuts = vec![Cut { start_sec: 10.0, end_sec: 12.0 }];
        a.set_speed_text("2".into());
        let m = Montage::from_edits(vec![a, clip("b.mp4", 10.0)]);
        assert!((m.segments()[0].duration() - 9.0).abs() < 1e-9); // (20 - 2) / 2
        assert_eq!(m.segments()[0].pieces(), vec![(5.0, 10.0), (12.0, 25.0)]);
        assert!((m.duration() - 19.0).abs() < 1e-9);
        assert!((m.start_of(1) - 9.0).abs() < 1e-9);
        assert!(m.is_montage());
        assert!(!Montage::single(clip("c.mp4", 3.0)).is_montage());
    }

    #[test]
    fn split_divides_trim_and_cuts_and_keeps_the_look() {
        let mut a = clip("a.mp4", 30.0);
        a.trim_end_sec = 30.0;
        a.cuts = vec![Cut { start_sec: 8.0, end_sec: 12.0 }, Cut { start_sec: 20.0, end_sec: 22.0 }];
        a.selected_filter_index = 3;
        a.push_undo();
        let mut m = Montage::single(a);
        assert!(!m.split(0, 0.1, 0)); // too close to the start
        assert!(m.split(0, 10.0, 1));
        let (l, r) = (&m.segments()[0].edit, &m.segments()[1].edit);
        assert_eq!((l.trim_start_sec, l.trim_end_sec, r.trim_start_sec, r.trim_end_sec), (0.0, 10.0, 10.0, 30.0));
        assert_eq!(l.cuts, vec![Cut { start_sec: 8.0, end_sec: 10.0 }]);
        assert_eq!(r.cuts, vec![Cut { start_sec: 10.0, end_sec: 12.0 }, Cut { start_sec: 20.0, end_sec: 22.0 }]);
        assert_eq!((l.selected_filter_index, r.selected_filter_index), (3, 3));
        assert!(l.undo_stack.is_empty() && r.undo_stack.is_empty());
        assert_eq!(m.selected(), 1);
        assert_ne!(m.segments()[0].id, m.segments()[1].id);
        // total length is unchanged by a split
        assert!((m.duration() - 24.0).abs() < 1e-9);
    }

    #[test]
    fn add_move_remove_and_undo_in_order() {
        let mut m = Montage::from_edits(vec![clip("a.mp4", 10.0), clip("b.mp4", 10.0)]);
        let ids = |m: &Montage| m.segments().iter().map(|s| s.edit.clip_name.clone()).collect::<Vec<_>>();
        assert_eq!(m.add_after_selected(clip("c.mp4", 5.0), 0), 1); // after a
        assert_eq!(ids(&m), ["a", "c", "b"]);
        assert!(m.move_segment(1, 2, 0));
        assert_eq!(ids(&m), ["a", "b", "c"]);
        assert_eq!(m.selected(), 2);
        assert!(m.remove(0, 0));
        assert_eq!(ids(&m), ["b", "c"]);
        // an edit made inside the selected segment after the remove is undone first
        assert!(!m.undo_is_montage(1));
        assert!(m.undo_is_montage(0));
        assert!(m.undo());
        assert_eq!(ids(&m), ["a", "b", "c"]);
        assert!(m.undo());
        assert!(m.undo());
        assert_eq!(ids(&m), ["a", "b"]);
        assert!(!m.undo());
        // the last segment cannot be removed
        let mut one = Montage::single(clip("x.mp4", 1.0));
        assert!(!one.remove(0, 0));
    }

    #[test]
    fn frame_shape_is_shared_by_all_segments() {
        let mut m = Montage::from_edits(vec![clip("a.mp4", 10.0), clip("b.mp4", 10.0)]);
        m.set_aspect_everywhere(2); // 9:16
        for s in m.segments() {
            assert_eq!(s.edit.selected_aspect_index, 2);
            assert!(s.edit.is_portrait_output());
            assert!(!s.edit.is_reframed());
        }
    }

    #[test]
    fn commit_and_select_swap_the_live_edit() {
        let mut m = Montage::from_edits(vec![clip("a.mp4", 10.0), clip("b.mp4", 10.0)]);
        let mut live = m.selected_edit().clone();
        live.trim_end_sec = 4.0;
        m.commit(&live);
        let b = m.select(1).unwrap().clone();
        assert_eq!(b.clip_name, "b");
        assert_eq!(m.segments()[0].edit.trim_end_sec, 4.0);
        assert!(m.select(9).is_none());
    }
}
