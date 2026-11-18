//! Builders for the settings rows (`SettingsRowData`) and small display helpers.

use super::*;
use crate::{ComboBoxItemData, SegmentedOption, SettingsRowData};
use slint::{ModelRc, VecModel};
use std::rc::Rc;

pub(super) fn model<T: Clone + 'static>(rows: Vec<T>) -> ModelRc<T> {
    Rc::new(VecModel::from(rows)).into()
}

pub(super) fn text(s: &Option<String>) -> &str {
    s.as_deref().unwrap_or("")
}

pub(super) fn row(id: &str, label: &str, kind: i32) -> SettingsRowData {
    SettingsRowData {
        id: id.into(),
        label: label.into(),
        kind,
        enabled: true,
        visible: true,
        divider: true,
        maximum: 100.,
        ..Default::default()
    }
}
pub(super) fn choice(
    id: &str,
    label: &str,
    kind: i32,
    options: Vec<(String, i32)>,
    selected: usize,
) -> SettingsRowData {
    let mut r = row(id, label, kind);
    r.selected = selected as i32;
    r.options = model(options.iter().map(|(t, tier)| ComboBoxItemData { text: t.into(), tier: *tier }).collect());
    r.segments = model(options.into_iter().map(|(t, tier)| SegmentedOption { text: t.into(), tier }).collect());
    r
}
pub(super) fn simple_options(items: &[&str]) -> Vec<(String, i32)> {
    items.iter().map(|s| (tr(s), 0)).collect()
}
pub(super) fn toggle(id: &str, label: &str, desc: &str, checked: bool) -> SettingsRowData {
    let mut r = row(id, label, 2);
    r.checked = checked;
    r.description = desc.into();
    r
}
/// PNG bytes (lag_audio icon extractor, 32x32 RGBA) -> slint image.
pub(super) fn decode_icon(png: &[u8]) -> Option<slint::Image> {
    let img = image::load_from_memory(png).ok()?.to_rgba8();
    let (w, h) = img.dimensions();
    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(img.as_raw(), w, h);
    Some(slint::Image::from_rgba8(buf))
}
pub(super) fn volume(id: &str, label: &str, enabled: bool, value: i32, max: i32) -> SettingsRowData {
    let mut r = row(id, label, 4);
    r.checked = enabled;
    r.value = value as f32;
    r.maximum = max as f32;
    r.divider = false;
    r
}
pub(super) fn pos<T: PartialEq>(values: &[T], value: &T) -> usize {
    values.iter().position(|v| v == value).unwrap_or(0)
}
pub(super) fn hotkey_display(key: &Option<String>, mods: &Option<String>) -> String {
    let k = text(key);
    if k.is_empty() || k == "VcUndefined" {
        return "—".into();
    }
    let m = text(mods);
    let mut parts = Vec::new();
    for (s, label) in [("Ctrl", "Ctrl"), ("Control", "Ctrl"), ("Alt", "Alt"), ("Shift", "Shift")] {
        if m.contains(s) && !parts.contains(&label) {
            parts.push(label);
        }
    }
    parts.push(k.strip_prefix("Vc").unwrap_or(k));
    parts.join(" + ")
}
