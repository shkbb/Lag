//! The native video window (mpv `wid`) and the overlay window kept over it; keyboard focus while the
//! player is shown.

use super::{globals::*, session::*, sidebar::*};
use crate::shell::nofocus;
use crate::{app_ctx::AppCtx, AppWindow, LibraryVM, MainVM, Page, PlayerOverlayVM, PlayerOverlayWindow, PlayerVM, Tr};
use lag_player::NativeVideoWindow;
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    Graphics::Gdi::ClientToScreen,
    UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
    UI::WindowsAndMessaging::{
        GetCursorPos, GetWindowRect, IsIconic, IsWindowVisible, SetWindowPos, SWP_NOACTIVATE, SWP_NOZORDER,
        WM_DPICHANGED, WM_MOVE, WM_NCDESTROY, WM_SIZE, WM_WINDOWPOSCHANGED,
    },
};

/// Converts logical Slint rectangle to physical pixels, optionally insetting by `inset_logical`
/// (e.g. 1px windowed border).
pub(crate) fn logical_to_physical_rect(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    scale: f32,
    inset_logical: f32,
) -> (i32, i32, i32, i32) {
    let scale = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let inset = inset_logical.max(0.0);
    let lx = x + inset;
    let ly = y + inset;
    let lw = (w - inset * 2.0).max(0.0);
    let lh = (h - inset * 2.0).max(0.0);
    let px = (lx * scale).round() as i32;
    let py = (ly * scale).round() as i32;
    let pr = ((lx + lw) * scale).round() as i32;
    let pb = ((ly + lh) * scale).round() as i32;
    (px, py, (pr - px).max(0), (pb - py).max(0))
}

/// Owner, 2026-11-17: no player key worked (Space, Esc, M, arrows). While the player page is up, keyboard
/// focus could sit on one of our child/owned windows (the native video surface, mpv's own child window or the
/// non-activating overlay), none of which handle keys - only the main window does (Slint key handlers, Esc in
/// main_vm). If the foreground window is ours and focus is on any other window of our process, focus goes back
/// to the main window. Every focus owner change is logged once (class name) to see who takes it.
/// Only focus inside the player's own windows (child surfaces of the main window, mpv's child window, the overlay)
/// is pulled back. Owner, 2026-11-17: the themed tray menu (another window of ours) opened and closed at once
/// and the main window jumped to the front - this pulled the focus away from the menu.
pub(super) fn keep_keyboard_focus(main: HWND, overlay: Option<HWND>, last: &RefCell<String>) {
    use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentProcessId, GetCurrentThreadId};
    use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassNameW, GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO,
    };
    unsafe {
        let mut gti = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
        if GetGUIThreadInfo(0, &mut gti).is_err() {
            return;
        }
        let focus = gti.hwndFocus;
        if focus.0.is_null() {
            return;
        }
        let mut pid = 0u32;
        let thread = GetWindowThreadProcessId(focus, Some(&mut pid));
        if pid != GetCurrentProcessId() {
            return; // another app is in front: not ours to touch
        }
        let mut buf = [0u16; 128];
        let n = GetClassNameW(focus, &mut buf).max(0) as usize;
        let class = String::from_utf16_lossy(&buf[..n]);
        if *last.borrow() != class {
            log::info!("[Player] keyboard focus on window class '{class}'");
            *last.borrow_mut() = class;
        }
        if focus == main {
            return;
        }
        use windows::Win32::UI::WindowsAndMessaging::IsChild;
        let in_player =
            IsChild(main, focus).as_bool() || overlay.is_some_and(|ov| focus == ov || IsChild(ov, focus).as_bool());
        if !in_player {
            return; // tray menu, dialogs, splash...: leave them their focus
        }
        let me = GetCurrentThreadId();
        let attached = thread != me && AttachThreadInput(me, thread, true).as_bool();
        let _ = SetFocus(Some(main));
        if attached {
            let _ = AttachThreadInput(me, thread, false);
        }
    }
}

pub(super) fn sync_overlay_from_vm(ui: &AppWindow, ov: &PlayerOverlayWindow, session: &PlayerSessionState) {
    let vm = ui.global::<PlayerVM>();
    let mvm = ui.global::<MainVM>();
    let ovm = ov.global::<PlayerOverlayVM>();
    ovm.set_has_clip(vm.get_has_clip());
    ovm.set_is_playing(vm.get_is_playing());
    ovm.set_is_loading(vm.get_is_loading());
    ovm.set_is_image(vm.get_is_image());
    ovm.set_is_fullscreen(mvm.get_is_fullscreen());
    ovm.set_path(vm.get_path());
    ovm.set_time_text(vm.get_time_text());
    if !ovm.get_dragging() {
        ovm.set_position(vm.get_position());
    }
    ovm.set_volume(vm.get_volume());
    ovm.set_muted(vm.get_muted());
    ovm.set_speed(vm.get_speed());
    ovm.set_audio_track(vm.get_audio_track());
    ovm.set_audio_track_count(vm.get_audio_track_count());
    ovm.set_error(vm.get_error());
    ovm.set_library_loading(ui.global::<LibraryVM>().get_is_loading());
    let _ = session;
}

pub(super) struct NativeHost {
    pub(super) main_hwnd: HWND,
    pub(super) surface: NativeVideoWindow,
    pub(super) overlay: PlayerOverlayWindow,
    pub(super) last_overlay_rect: Cell<(i32, i32, i32, i32)>,
    pub(super) overlay_shown: Cell<bool>,
    /// Fullscreen while playing with no controls on screen: the full-screen transparent overlay window is hidden
    /// so DWM does not blend it with every 3440x1440 frame. Mouse movement over the video brings it back.
    pub(super) overlay_suppressed: Cell<bool>,
}

unsafe extern "system" fn main_host_proc(
    hwnd: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    let result = unsafe { DefSubclassProc(hwnd, message, wp, lp) };
    if matches!(message, WM_MOVE | WM_SIZE | WM_WINDOWPOSCHANGED | WM_DPICHANGED) {
        HOST_SYNC.with(|cb| {
            let cb = cb.borrow().clone();
            if let Some(cb) = cb {
                cb();
            }
        });
    } else if message == WM_NCDESTROY {
        HOST_SYNC.with(|cb| {
            *cb.borrow_mut() = None;
        });
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(main_host_proc), id);
        }
    }
    result
}

impl NativeHost {
    pub(super) fn new(ui: &AppWindow, main_hwnd: HWND, ctx: &Rc<AppCtx>) -> Result<Self, lag_player::Error> {
        let surface = NativeVideoWindow::new(main_hwnd.0 as isize)?;
        let overlay = PlayerOverlayWindow::new()
            .map_err(|e| lag_player::Error(format!("PlayerOverlayWindow init failed: {e}")))?;

        overlay.global::<Tr>().on_t(|key, _rev| lag_core::i18n::tr(&key).into());
        overlay.global::<Tr>().set_rev(ui.global::<Tr>().get_rev());
        overlay.set_theme_mode(ui.get_theme_mode());

        // Wire overlay callbacks -> AppWindow PlayerVM / MainVM / LibraryVM callbacks.
        let ovm = overlay.global::<PlayerOverlayVM>();
        ovm.on_synthetic_move(synthetic_pointer_move);
        {
            let weak = ui.as_weak();
            ovm.on_toggle(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_toggle();
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_stop(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_stop();
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_seek(move |pos, exact| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().set_position(pos);
                    ui.global::<PlayerVM>().invoke_seek(pos, exact);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_skip(move |delta| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_skip(delta);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_next(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_next();
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_play_clip(move |path| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_play_clip(path);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_edit(move |path| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_edit(path);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_show_in_folder(move |path| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<LibraryVM>().invoke_context_show_in_folder(path);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_delete_clip(move |path| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<LibraryVM>().invoke_context_delete(path);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_volume_changed(move |vol| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().set_volume(vol);
                    ui.global::<PlayerVM>().invoke_volume_changed(vol);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_mute(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_mute();
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_speed_changed(move |speed| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_speed_changed(speed);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_audio_changed(move |idx| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().invoke_audio_changed(idx);
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_toggle_fullscreen(move || {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<MainVM>().invoke_toggle_fullscreen();
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_exit_fullscreen(move || {
                if let Some(ui) = weak.upgrade() {
                    if ui.global::<MainVM>().get_is_fullscreen() {
                        ui.global::<MainVM>().invoke_toggle_fullscreen();
                    }
                }
            });
        }
        {
            let weak = ui.as_weak();
            ovm.on_dragging_changed(move |dragging| {
                if let Some(ui) = weak.upgrade() {
                    ui.global::<PlayerVM>().set_dragging(dragging);
                }
            });
        }

        // Sync initial sidebar items to the overlay.
        let clips = ctx.clips();
        let (seconds, bytes) = sidebar_totals(&clips);
        ovm.set_total_duration(format_total_duration(seconds).into());
        ovm.set_total_size(
            if bytes >= 1_073_741_824 {
                format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
            } else {
                format!("{:.0} MB", bytes as f64 / 1_048_576.0)
            }
            .into(),
        );
        ovm.set_clips(Rc::new(slint::VecModel::from(build_player_clips(&clips))).into());

        unsafe {
            let _ = SetWindowSubclass(main_hwnd, Some(main_host_proc), 0x4c4148, 0);
            install_cursor_proc(main_hwnd);
        }

        Ok(Self {
            main_hwnd,
            surface,
            overlay,
            last_overlay_rect: Cell::new((0, 0, 0, 0)),
            overlay_shown: Cell::new(false),
            overlay_suppressed: Cell::new(false),
        })
    }

    pub(super) fn sync(
        &self,
        ui: &AppWindow,
        logical_rect: (f32, f32, f32, f32),
        fs_transition: bool,
        video_ready: bool,
    ) {
        let occluder = OCCLUDER.with(|o| o.get());
        let vm = ui.global::<PlayerVM>();
        let mvm = ui.global::<MainVM>();
        let is_fullscreen = mvm.get_is_fullscreen();
        let scale = ui.window().scale_factor();

        let (nx, ny, nw, nh) = logical_to_physical_rect(
            logical_rect.0,
            logical_rect.1,
            logical_rect.2,
            logical_rect.3,
            scale,
            if is_fullscreen { 0.0 } else { 1.0 },
        );
        let (ox, oy, ow, oh) =
            logical_to_physical_rect(logical_rect.0, logical_rect.1, logical_rect.2, logical_rect.3, scale, 0.0);

        let minimized = ui.window().is_minimized() || unsafe { IsIconic(self.main_hwnd).as_bool() };
        let visible = ui.window().is_visible() && unsafe { IsWindowVisible(self.main_hwnd).as_bool() };
        let vis = SurfaceVisibility {
            page_is_player: mvm.get_current_page() == Page::Player,
            window_visible: visible,
            window_minimized: minimized,
            fs_transition,
            has_clip: vm.get_has_clip(),
            is_image: vm.get_is_image(),
            video_ready,
            width_px: ow,
            height_px: oh,
        };

        // Size the surface even while hidden: mpv builds its swapchain at the window's size when the first
        // frame arrives; a 1x1 hidden surface made it build tiny and visibly rescale on show.
        if nw > 0 && nh > 0 && !fs_transition {
            self.surface.set_bounds(nx, ny, nw, nh);
        }
        // Cut floating main-window UI (capture flyout) out of the video instead of hiding it under the surface.
        let hole = occluder.and_then(|(x, y, w, h)| {
            let (px, py, pw, ph) = logical_to_physical_rect(x, y, w, h, scale, 0.0);
            let (l, t) = ((px - nx).max(0), (py - ny).max(0));
            let (r, b) = ((px + pw - nx).min(nw), (py + ph - ny).min(nh));
            (r > l && b > t).then_some((l, t, r - l, b - t))
        });
        // the video frame (views/player.slint) is 12 px rounded with a 1 px border and the surface sits 1 px
        // inside it: clip to the inner 11 px radius so no square corner shows; fullscreen is square
        self.surface.set_corner_radius(if is_fullscreen { 0 } else { (11.0 * scale).round() as i32 });
        self.surface.set_exclusion(hole);
        if vis.show_native() && nw > 0 && nh > 0 {
            self.surface.show();
        } else {
            self.surface.hide();
        }

        if vis.show_overlay() && !(is_fullscreen && self.overlay_suppressed.get()) {
            let mut pt = POINT { x: ox, y: oy };
            unsafe {
                let _ = ClientToScreen(self.main_hwnd, &mut pt);
            }
            let target = (pt.x, pt.y, ow.max(1), oh.max(1));

            if !self.overlay_shown.get() {
                nofocus::apply_owned_no_focus(self.overlay.window(), self.main_hwnd);
                let _ = self.overlay.show();
                nofocus::apply_owned_no_focus(self.overlay.window(), self.main_hwnd);
                nofocus::show_owned_no_activate(self.overlay.window());
                self.overlay_shown.set(true);
                self.last_overlay_rect.set((0, 0, 0, 0));
            }
            if nofocus::hwnd(self.overlay.window()).is_some() && self.last_overlay_rect.get() != target {
                if let Some(ov_hwnd) = nofocus::hwnd(self.overlay.window()) {
                    nofocus::apply_owned_no_focus(self.overlay.window(), self.main_hwnd);
                    unsafe {
                        let _ = SetWindowPos(
                            ov_hwnd,
                            None,
                            target.0,
                            target.1,
                            target.2,
                            target.3,
                            SWP_NOACTIVATE | SWP_NOZORDER,
                        );
                    }
                    self.last_overlay_rect.set(target);
                }
            }
        } else if self.overlay_shown.get() {
            let _ = self.overlay.hide();
            self.overlay_shown.set(false);
        }
    }

    pub(super) fn check_cursor_auto_hide(&self, is_fullscreen: bool, pinned: bool) {
        if !self.overlay_shown.get() || pinned {
            return;
        }
        let ovm = self.overlay.global::<PlayerOverlayVM>();
        if !ovm.get_controls() && !(is_fullscreen && ovm.get_sidebar()) {
            return;
        }
        let Some(ov_hwnd) = nofocus::hwnd(self.overlay.window()) else {
            return;
        };
        let mut pt = POINT::default();
        let mut rect = RECT::default();
        unsafe {
            if GetCursorPos(&mut pt).is_ok()
                && GetWindowRect(ov_hwnd, &mut rect).is_ok()
                && (pt.x < rect.left || pt.x >= rect.right || pt.y < rect.top || pt.y >= rect.bottom)
            {
                ovm.set_controls(false);
                if is_fullscreen {
                    ovm.set_sidebar(false);
                }
            }
        }
    }
}

impl Drop for NativeHost {
    fn drop(&mut self) {
        HOST_SYNC.with(|cb| {
            *cb.borrow_mut() = None;
        });
        let _ = self.overlay.hide();
        self.surface.destroy();
    }
}
