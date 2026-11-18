//! Editor preview on the player's v2 engine (owner, 2026-11-17: "the editor is dead").
//!
//! The video is a native child window inside the main window (mpv `wid`, vo=gpu-next, d3d11, hwdec) placed at the
//! video rect of the preview stage and clipped to the stage. Slint cannot draw over a child window, so everything
//! that sits on top of the video (crop shades, frame outline, crop box + handles, captions, the deselect chip) is
//! drawn by `EditorOverlayWindow`: a transparent, non-activating window owned by the main window and kept exactly
//! over the stage. Its callbacks go straight back to `EditorVM`, so the editor logic did not change.

use super::model::EditModel;
use crate::shell::nofocus;
use crate::{AppWindow, EditorOverlayVM, EditorOverlayWindow, EditorVM, MainVM, Page, Tr};
use lag_player::NativeVideoWindow;
use slint::ComponentHandle;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, WPARAM},
    Graphics::Gdi::ClientToScreen,
    UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
    UI::WindowsAndMessaging::{
        IsIconic, IsWindowVisible, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER, WM_DPICHANGED, WM_MOVE, WM_NCDESTROY,
        WM_SIZE, WM_WINDOWPOSCHANGED,
    },
};

/// Stage card: 14 px corners with a 1 px border; the video is clipped 1 px inside it.
const STAGE_RADIUS: f32 = 13.0;
const STAGE_BORDER: f32 = 1.0;
const SUBCLASS_ID: usize = 0x4c4145;

thread_local! {
    /// Re-sync right away when the main window moves/resizes (the 33 ms tick would lag one frame behind).
    static EDITOR_SYNC: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
    /// The overlay window, for `--gpu-check` scripts (events dispatched to it, never to the OS).
    static OVERLAY: RefCell<Option<slint::Weak<EditorOverlayWindow>>> = const { RefCell::new(None) };
}

/// The live overlay window (gpu-check only).
pub(crate) fn overlay_for_checks() -> Option<EditorOverlayWindow> {
    OVERLAY.with(|o| o.borrow().as_ref().and_then(|w| w.upgrade()))
}

unsafe extern "system" fn editor_host_proc(
    hwnd: HWND,
    msg: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    _d: usize,
) -> LRESULT {
    let result = unsafe { DefSubclassProc(hwnd, msg, wp, lp) };
    if matches!(msg, WM_MOVE | WM_SIZE | WM_WINDOWPOSCHANGED | WM_DPICHANGED) {
        let cb = EDITOR_SYNC.with(|c| c.borrow().clone());
        if let Some(cb) = cb {
            cb();
        }
    } else if msg == WM_NCDESTROY {
        EDITOR_SYNC.with(|c| *c.borrow_mut() = None);
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(editor_host_proc), id);
        }
    }
    result
}

pub(crate) fn set_resync(cb: Option<Rc<dyn Fn()>>) {
    EDITOR_SYNC.with(|c| *c.borrow_mut() = cb);
}

/// Aspect-fit of `aspect` inside `sw` x `sh` (same as EditorStageOverlay's fit-x/y/w/h).
pub fn fit(sw: f32, sh: f32, aspect: f32) -> (f32, f32, f32, f32) {
    if sw <= 0.0 || sh <= 0.0 || aspect <= 0.0 || !aspect.is_finite() {
        return (0.0, 0.0, sw.max(0.0), sh.max(0.0));
    }
    if sw / aspect > sh {
        let w = sh * aspect;
        ((sw - w) / 2.0, 0.0, w, sh)
    } else {
        let h = sw / aspect;
        (0.0, (sh - h) / 2.0, sw, h)
    }
}

/// The video rect inside a `sw` x `sh` stage (logical px), exactly where the old Slint Image was drawn:
/// the reference box while cropping, else the reframe box.
pub fn video_rect_in_stage(model: &EditModel, sw: f32, sh: f32) -> (f32, f32, f32, f32) {
    let ref_aspect = if model.crop_tool_active {
        model.display_source_aspect_raw() as f32
    } else {
        model.display_frame_aspect() as f32
    };
    let (rx, ry, rw, rh) = fit(sw, sh, ref_aspect);
    if model.crop_tool_active {
        return (rx, ry, rw, rh);
    }
    let (nx, ny, nw, nh) = model.display_edit_norm();
    (rx + nx as f32 * rw, ry + ny as f32 * rh, nw as f32 * rw, nh as f32 * rh)
}

/// Part of the stage (logical px, stage-relative) where the preview video may show, and whether that part keeps
/// the stage card's rounded top corners.
///
/// While the frame is being edited (`editing`: crop tool, or the reframe box selected) the whole stage shows the
/// video and the overlay dims what lies outside the frame. Otherwise only the export frame shows, so the preview
/// is exactly what the export contains. Owner, 2026-11-18, "9:16 works like a cripple": the native surface was
/// clipped to the stage, so a 9:16 frame over a 21:9 clip still showed the whole wide picture.
pub fn visible_rect(sw: f32, sh: f32, frame: (f32, f32, f32, f32), editing: bool) -> ((f32, f32, f32, f32), bool) {
    // the video sits 1 px inside the card border (top, left, right; the bottom joins the controls row)
    let inset = (STAGE_BORDER, STAGE_BORDER, (sw - 2.0 * STAGE_BORDER).max(0.0), (sh - STAGE_BORDER).max(0.0));
    if editing {
        return (inset, true);
    }
    let (fx, fy, fw, fh) = frame;
    let left = fx.max(inset.0);
    let top = fy.max(inset.1);
    let right = (fx + fw).min(inset.0 + inset.2);
    let bottom = (fy + fh).min(inset.1 + inset.3);
    let rect = (left, top, (right - left).max(0.0), (bottom - top).max(0.0));
    // a frame narrower or lower than the card has square corners: rounding them would cut the picture
    let fills = (rect.2 - inset.2).abs() < 1.0 && (rect.3 - inset.3).abs() < 1.0;
    (rect, fills)
}

pub struct EditorHost {
    pub main_hwnd: HWND,
    surface: NativeVideoWindow,
    overlay: EditorOverlayWindow,
    overlay_shown: Cell<bool>,
    overlay_rect: Cell<(i32, i32, i32, i32)>,
}

impl EditorHost {
    pub fn new(ui: &AppWindow, main_hwnd: HWND) -> Result<Self, String> {
        let surface = NativeVideoWindow::new(main_hwnd.0 as isize).map_err(|e| e.to_string())?;
        surface.set_round_bottom(false);
        let overlay = EditorOverlayWindow::new().map_err(|e| e.to_string())?;
        overlay.global::<Tr>().on_t(|key, _rev| lag_core::i18n::tr(&key).into());
        overlay.global::<Tr>().set_rev(ui.global::<Tr>().get_rev());
        overlay.set_theme_mode(ui.get_theme_mode());

        // every overlay callback goes back to the main window's EditorVM (one source of truth)
        let ovm = overlay.global::<EditorOverlayVM>();
        macro_rules! forward {
            // rustfmt writes an empty argument list `| |` as `||`
            ($on:ident, || $body:expr) => { forward!($on, | | $body) };
            ($on:ident, | $($a:ident),* | $body:expr) => {{
                let weak = ui.as_weak();
                ovm.$on(move |$($a),*| {
                    if let Some(ui) = weak.upgrade() {
                        let vm = ui.global::<EditorVM>();
                        let f = $body;
                        f(&vm);
                    }
                });
            }};
        }
        forward!(on_focus_keys, || |vm: &EditorVM| vm.set_focus_serial(vm.get_focus_serial().wrapping_add(1)));
        forward!(on_play_pause, || |vm: &EditorVM| vm.invoke_play_pause());
        forward!(on_deselect_reframe, || |vm: &EditorVM| vm.set_reframe_selected(false));
        forward!(on_select_reframe, || |vm: &EditorVM| vm.set_reframe_selected(true));
        forward!(on_right_click_reset, || |vm: &EditorVM| vm.invoke_right_click_reset());
        forward!(on_begin_overlay_drag, || |vm: &EditorVM| vm.invoke_begin_overlay_drag());
        forward!(on_push_undo, || |vm: &EditorVM| vm.invoke_push_undo());
        forward!(on_select_text, |i| move |vm: &EditorVM| vm.invoke_select_text(i));
        forward!(on_move_text, |i, nx, ny| move |vm: &EditorVM| vm.invoke_move_text(i, nx, ny));
        // in-place caption editing: the main window's Text-tab field takes the keyboard (no overlay activation)
        forward!(on_text_edit_started, |i| move |vm: &EditorVM| {
            vm.set_editing_text_index(i);
            vm.set_selected_tools_tab(2);
            vm.set_text_focus_serial(vm.get_text_focus_serial().wrapping_add(1));
        });
        forward!(on_text_edit_finished, || |vm: &EditorVM| {
            vm.set_editing_text_index(-1);
            vm.set_focus_serial(vm.get_focus_serial().wrapping_add(1));
        });
        forward!(on_deselect_text, || |vm: &EditorVM| vm.invoke_select_text(-1));
        {
            let weak = ui.as_weak();
            ovm.on_drag_overlay(move |m, dx, dy, rx, ry, rw, rh, sw, sh| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<EditorVM>().invoke_drag_overlay(m, dx, dy, rx, ry, rw, rh, sw, sh);
                }
            });
        }

        unsafe {
            let _ = SetWindowSubclass(main_hwnd, Some(editor_host_proc), SUBCLASS_ID, 0);
        }
        OVERLAY.with(|o| *o.borrow_mut() = Some(overlay.as_weak()));
        log::info!("[Editor] native video surface ready (player v2 engine)");
        Ok(Self { main_hwnd, surface, overlay, overlay_shown: Cell::new(false), overlay_rect: Cell::new((0, 0, 0, 0)) })
    }

    pub fn wid(&self) -> isize {
        self.surface.wid()
    }

    /// Place/show or hide the surface and the overlay. `stage` = the stage rect in the window (logical px).
    pub fn sync(&self, ui: &AppWindow, model: &EditModel, stage: (f32, f32, f32, f32)) {
        let mvm = ui.global::<MainVM>();
        let vm = ui.global::<EditorVM>();
        let (sx, sy, sw, sh) = stage;
        let minimized = ui.window().is_minimized() || unsafe { IsIconic(self.main_hwnd).as_bool() };
        let visible = mvm.get_current_page() == Page::Editor
            && ui.window().is_visible()
            && unsafe { IsWindowVisible(self.main_hwnd).as_bool() }
            && !minimized
            && model.current_clip_path.is_some()
            && sw > 2.0
            && sh > 2.0;
        if !visible {
            self.surface.hide();
            if self.overlay_shown.get() {
                let _ = self.overlay.hide();
                self.overlay_shown.set(false);
            }
            return;
        }
        let scale = ui.window().scale_factor();
        let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
        let px = |v: f32| (v * scale).round() as i32;

        // video surface: the video rect, clipped to the export frame (or the whole stage while editing the frame)
        let (vx, vy, vw, vh) = video_rect_in_stage(model, sw, sh);
        let (bx, by) = (px(sx + vx), px(sy + vy));
        let (bw, bh) = (px(vw).max(1), px(vh).max(1));
        self.surface.set_bounds(bx, by, bw, bh);
        let editing = model.crop_tool_active || vm.get_reframe_selected();
        let frame = fit(sw, sh, model.display_frame_aspect() as f32);
        let ((lx, ly, lw, lh), rounded) = visible_rect(sw, sh, frame, editing);
        let (cx, cy) = (px(sx + lx) - bx, px(sy + ly) - by);
        self.surface.set_clip_rect(Some((cx, cy, px(lw).max(1), px(lh).max(1))));
        self.surface.set_corner_radius(if rounded { px(STAGE_RADIUS) } else { 0 });
        // floating main-window UI over the stage (capture flyout): cut out of the video
        let hole = crate::views::player::occluder_rect().and_then(|(x, y, w, h)| {
            let (hx, hy, hw, hh) = (px(x) - bx, px(y) - by, px(w), px(h));
            let (l, t) = (hx.max(0), hy.max(0));
            let (r, b) = ((hx + hw).min(bw), (hy + hh).min(bh));
            (r > l && b > t).then_some((l, t, r - l, b - t))
        });
        self.surface.set_exclusion(hole);
        self.surface.show();

        // overlay window: exactly over the stage
        let ovm = self.overlay.global::<EditorOverlayVM>();
        ovm.set_has_clip(true);
        ovm.set_display_source_aspect_raw(vm.get_display_source_aspect_raw());
        ovm.set_display_frame_aspect(vm.get_display_frame_aspect());
        ovm.set_crop_tool_active(vm.get_crop_tool_active());
        ovm.set_reframe_selected(vm.get_reframe_selected());
        ovm.set_edit_nx(vm.get_edit_nx());
        ovm.set_edit_ny(vm.get_edit_ny());
        ovm.set_edit_nw(vm.get_edit_nw());
        ovm.set_edit_nh(vm.get_edit_nh());
        ovm.set_texts(vm.get_texts());
        ovm.set_editing_index(vm.get_editing_text_index());
        self.overlay.set_theme_mode(ui.get_theme_mode());
        let tr_rev = ui.global::<Tr>().get_rev();
        if self.overlay.global::<Tr>().get_rev() != tr_rev {
            self.overlay.global::<Tr>().set_rev(tr_rev);
        }

        let mut pt = POINT { x: px(sx), y: px(sy) };
        unsafe {
            let _ = ClientToScreen(self.main_hwnd, &mut pt);
        }
        let target = (pt.x, pt.y, px(sw).max(1), px(sh).max(1));
        if !self.overlay_shown.get() {
            nofocus::apply_owned_no_focus(self.overlay.window(), self.main_hwnd);
            let _ = self.overlay.show();
            nofocus::apply_owned_no_focus(self.overlay.window(), self.main_hwnd);
            nofocus::show_owned_no_activate(self.overlay.window());
            self.overlay_shown.set(true);
            self.overlay_rect.set((0, 0, 0, 0));
        }
        if self.overlay_rect.get() != target {
            if let Some(ov) = nofocus::hwnd(self.overlay.window()) {
                nofocus::apply_owned_no_focus(self.overlay.window(), self.main_hwnd);
                unsafe {
                    let _ =
                        SetWindowPos(ov, None, target.0, target.1, target.2, target.3, SWP_NOACTIVATE | SWP_NOZORDER);
                }
                // NO SetWindowRgn here: a region on this frameless winit window brought back a classic title bar
                // ("Lag Editor Overlay" + X, owner 2026-11-17). The card shape is clipped in Slint instead.
                self.overlay_rect.set(target);
            }
        }
    }
}

impl Drop for EditorHost {
    fn drop(&mut self) {
        set_resync(None);
        let _ = self.overlay.hide();
        self.surface.destroy();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_letterboxes_and_pillarboxes() {
        assert_eq!(fit(1000.0, 500.0, 2.0), (0.0, 0.0, 1000.0, 500.0));
        assert_eq!(fit(1000.0, 1000.0, 2.0), (0.0, 250.0, 1000.0, 500.0));
        assert_eq!(fit(1000.0, 250.0, 2.0), (250.0, 0.0, 500.0, 250.0));
        assert_eq!(fit(0.0, 100.0, 2.0), (0.0, 0.0, 0.0, 100.0));
    }

    #[test]
    fn preview_shows_only_the_export_frame() {
        let (sw, sh) = (801.0, 451.0);
        // 9:16 frame in a wide stage: only the narrow frame is visible, square corners
        let frame = fit(sw, sh, 9.0 / 16.0);
        let (r, rounded) = visible_rect(sw, sh, frame, false);
        assert!((r.2 - frame.2).abs() < 0.01 && (r.0 - frame.0).abs() < 0.01, "{r:?} vs {frame:?}");
        assert!(!rounded);
        // editing the frame: the whole stage, rounded like the card
        assert_eq!(visible_rect(sw, sh, frame, true), ((1.0, 1.0, 799.0, 450.0), true));
        // a frame that fills the stage keeps the rounded card corners
        let full = fit(sw, sh, sw / sh);
        assert!(visible_rect(sw, sh, full, false).1);
    }
}
