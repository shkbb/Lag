//! Theme and language applied to the window; language flag icons.

use super::rows::*;
use crate::{AppWindow, SettingsVM, Tr};
use lag_core::{i18n, settings::SettingsData};
use slint::ComponentHandle;

pub(super) fn flag_svg(code: &str) -> String {
    let mut svg =
        String::from("<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"30\" height=\"20\" viewBox=\"0 0 30 20\">");
    for l in i18n::flag(code).unwrap_or(&[]) {
        let fill = l.fill.as_deref().unwrap_or("none");
        let stroke = l
            .stroke
            .as_ref()
            .map_or(String::new(), |s| format!(" stroke=\"{}\" stroke-width=\"{}\"", s.color, s.thickness));
        if let Some(path) = &l.geometry {
            svg.push_str(&format!("<path d=\"{path}\" fill=\"{fill}\"{stroke}/>"));
        }
        if let Some(e) = &l.ellipse {
            let xy: Vec<_> = e.center.split(',').collect();
            if xy.len() == 2 {
                svg.push_str(&format!(
                    "<ellipse cx=\"{}\" cy=\"{}\" rx=\"{}\" ry=\"{}\" fill=\"{fill}\"{stroke}/>",
                    xy[0], xy[1], e.radius_x, e.radius_y
                ));
            }
        }
    }
    svg.push_str("</svg>");
    svg
}
pub(super) fn apply_appearance(ui: &AppWindow, s: &SettingsData) {
    let code = text(&s.language);
    if i18n::current_language().code != code {
        i18n::set_language(code);
        let tr = ui.global::<Tr>();
        tr.set_rev(tr.get_rev().wrapping_add(1));
    }
    ui.global::<SettingsVM>().set_theme_mode(text(&s.theme).into());
}
