//! The virtualised card grid: rows of day headers and cards, visible range for a scroll position.

use super::grouping::*;

// Layout constants matching Views/LibraryView.axaml:288-336
pub(super) const BODY_PAD_X: f32 = 38.0;
pub(super) const BODY_PAD_TOP: f32 = 16.0;
pub(super) const CARD_WIDTH: f32 = 248.0;
pub(super) const CARD_GAP: f32 = 20.0;
pub(super) const CARD_PITCH_X: f32 = CARD_WIDTH + CARD_GAP; // 268
pub(super) const CARD_HEIGHT: f32 = 186.0;
pub(super) const CARD_PITCH_Y: f32 = CARD_HEIGHT + CARD_GAP; // 206
pub(super) const GROUP_HEADER_HEIGHT: f32 = 22.0;
pub(super) const GROUP_HEADER_STEP: f32 = GROUP_HEADER_HEIGHT + 16.0; // 38
pub(super) const GROUP_MARGIN_BOTTOM: f32 = 34.0;
pub(super) const BODY_MARGIN_BOTTOM: f32 = 56.0;
pub(super) const OVERSCAN_PX: f32 = 450.0;

#[derive(Debug, Clone)]
pub(super) enum VirtualRowKind {
    Header { label: String, sub: String, count: String },
    Cards { clip_indices: Vec<usize> },
}

#[derive(Debug, Clone)]
pub(super) struct VirtualRow {
    pub(super) y: f32,
    pub(super) height: f32,
    pub(super) kind: VirtualRowKind,
}

pub(super) fn columns_for_viewport(viewport_width: f32) -> usize {
    let avail = (viewport_width - BODY_PAD_X * 2.0).max(CARD_PITCH_X);
    ((avail / CARD_PITCH_X).floor() as usize).max(1)
}

pub(super) fn layout_virtual_rows(groups: &[DayGroup], cols: usize) -> (Vec<VirtualRow>, f32) {
    if groups.is_empty() {
        return (Vec::new(), 0.0);
    }
    let cols = cols.max(1);
    let mut rows = Vec::new();
    let mut y = BODY_PAD_TOP;

    for g in groups {
        rows.push(VirtualRow {
            y,
            height: GROUP_HEADER_HEIGHT,
            kind: VirtualRowKind::Header { label: g.label.clone(), sub: g.sub.clone(), count: g.count_label.clone() },
        });
        y += GROUP_HEADER_STEP;

        for chunk in g.clip_indices.chunks(cols) {
            rows.push(VirtualRow {
                y,
                height: CARD_HEIGHT,
                kind: VirtualRowKind::Cards { clip_indices: chunk.to_vec() },
            });
            y += CARD_PITCH_Y;
        }

        y += GROUP_MARGIN_BOTTOM;
    }

    y += BODY_MARGIN_BOTTOM;
    (rows, y)
}

pub(super) fn visible_row_range(rows: &[VirtualRow], scroll_y: f32, viewport_h: f32) -> (usize, usize) {
    if rows.is_empty() {
        return (0, 0);
    }
    let min_y = (scroll_y - OVERSCAN_PX).max(0.0);
    let max_y = scroll_y + viewport_h.max(600.0) + OVERSCAN_PX;

    let first = rows.partition_point(|r| r.y + r.height < min_y);
    let last = rows.partition_point(|r| r.y <= max_y);
    let first = first.min(rows.len());
    let last = last.max(first).min(rows.len());
    (first, last)
}
