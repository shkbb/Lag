//! The clip list next to the player: rows, totals, header text.

use crate::{AppWindow, PlayerClip, PlayerOverlayVM, PlayerOverlayWindow, PlayerVM};
use crate::ComponentHandle;
use std::rc::Rc;

pub(super) fn sidebar_totals(clips: &[crate::app_ctx::SharedClip]) -> (u64, u64) {
    let total_ticks: i64 = clips.iter().map(|c| c.duration_ticks.max(0)).sum();
    let seconds = (total_ticks / 10_000_000) as u64;
    let bytes: u64 = clips.iter().map(|c| c.file_size.max(0) as u64).sum();
    (seconds, bytes)
}

pub(super) fn format_total_duration(seconds: u64) -> String {
    let total = if seconds >= 3600 {
        lag_core::i18n::format("Time_HourMinShort", &[&(seconds / 3600).to_string(), &(seconds / 60 % 60).to_string()])
    } else if seconds >= 60 {
        lag_core::i18n::format("Time_MinShort", &[&(seconds / 60).to_string()])
    } else {
        lag_core::i18n::format("Time_SecShort", &[&seconds.to_string()])
    };
    total.unwrap_or_default()
}

pub(super) fn header_meta_for(c: &crate::app_ctx::SharedClip) -> String {
    format!("{} · {}", c.created_date.format("%-d %b, %H:%M"), c.duration_display())
}

pub(super) fn build_player_clips(clips: &[crate::app_ctx::SharedClip]) -> Vec<PlayerClip> {
    clips
        .iter()
        .map(|c| PlayerClip {
            path: c.file_path.as_str().into(),
            title: c.display_name().into(),
            meta: c.short_meta().into(),
            header_meta: header_meta_for(c).into(),
            is_image: c.is_image,
            thumbnail: c.thumbnail.as_ref().map(|b| slint::Image::from_rgba8(b.clone())).unwrap_or_default(),
        })
        .collect()
}

pub(super) fn sync_sidebar_from_shared(
    ui: &AppWindow,
    overlay: Option<&PlayerOverlayWindow>,
    clips: &[crate::app_ctx::SharedClip],
) {
    let vm = ui.global::<PlayerVM>();
    let (seconds, bytes) = sidebar_totals(clips);
    let total_duration = format_total_duration(seconds);
    let total_size = if bytes >= 1_073_741_824 {
        format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
    } else {
        format!("{:.0} MB", bytes as f64 / 1_048_576.0)
    };
    let items = build_player_clips(clips);
    vm.set_total_duration(total_duration.clone().into());
    vm.set_total_size(total_size.clone().into());
    vm.set_clips(Rc::new(slint::VecModel::from(items.clone())).into());
    if vm.get_has_clip() {
        let cur = vm.get_path();
        if let Some(active) = clips.iter().find(|c| c.file_path == cur.as_str()) {
            vm.set_title(active.display_name().into());
            vm.set_meta(header_meta_for(active).into());
        }
    }
    if let Some(ov) = overlay {
        let ovm = ov.global::<PlayerOverlayVM>();
        ovm.set_total_duration(total_duration.into());
        ovm.set_total_size(total_size.into());
        ovm.set_clips(Rc::new(slint::VecModel::from(items)).into());
    }
}
