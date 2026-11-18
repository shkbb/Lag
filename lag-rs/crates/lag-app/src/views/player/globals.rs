//! UI-thread state the player shares with other pages: hiding the cursor over the video, the rect of floating
//! UI that must be cut out of the video, the fullscreen fade, scripted input in self-checks.

use super::host::*;
use crate::PlayerOverlayVM;
use slint::ComponentHandle;
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM},
    UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass},
    UI::WindowsAndMessaging::{GetCursorPos, WM_NCDESTROY},
};

/// Runs a fullscreen switch (the boxed closure) behind a fade to black.
pub(super) type FullscreenFade = Rc<dyn Fn(Box<dyn FnOnce()>)>;

thread_local! {
    pub(super) static HOST_SYNC: RefCell<Option<Rc<dyn Fn()>>> = const { RefCell::new(None) };
    /// Visible main-window UI floating over the video area (window coords, logical px).
    pub(super) static OCCLUDER: Cell<Option<(f32, f32, f32, f32)>> = const { Cell::new(None) };
    /// Set by `init`: runs a fullscreen toggle behind a fade to black when video is on screen.
    pub(super) static FULLSCREEN_FADE: RefCell<Option<FullscreenFade>> = const { RefCell::new(None) };
    /// Screen position of the cursor at the last pointer event the player UI saw (see `synthetic_pointer_move`).
    pub(super) static LAST_POINTER: Cell<(i32, i32)> = const { Cell::new((i32::MIN, i32::MIN)) };
    /// `--ui-check` / `--gpu-check` feed pointer events without moving the OS cursor: every one is real.
    pub(super) static SCRIPTED_INPUT: Cell<bool> = const { Cell::new(false) };
    /// Screen position where the player hid the cursor; None = not hidden. See `cursor_proc`.
    pub(super) static CURSOR_HIDDEN_AT: Cell<Option<(i32, i32)>> = const { Cell::new(None) };
}

pub(super) const CURSOR_SUBCLASS_ID: usize = 0x4c4143;

/// Owner, 2026-11-17: the hidden cursor flickered in fullscreen. Windows asks the window under the cursor for
/// its cursor (WM_SETCURSOR) not only on movement but also on repaints and window changes, and winit answered
/// with the arrow every time; the tick hid it again ~30 ms later. While the cursor is hidden and has not moved
/// since, our player windows now answer "no cursor" themselves. Any movement releases it at once.
unsafe extern "system" fn cursor_proc(
    hwnd: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    _data: usize,
) -> LRESULT {
    use windows::Win32::UI::WindowsAndMessaging::{SetCursor, HTCLIENT, WM_SETCURSOR};
    if message == WM_SETCURSOR && (lp.0 as u32 & 0xFFFF) == HTCLIENT {
        if let Some(at) = CURSOR_HIDDEN_AT.with(|c| c.get()) {
            let mut pt = POINT::default();
            if unsafe { GetCursorPos(&mut pt) }.is_ok() && (pt.x, pt.y) == at {
                unsafe {
                    let _ = SetCursor(None);
                }
                return LRESULT(1);
            }
            CURSOR_HIDDEN_AT.with(|c| c.set(None)); // moved: show it again (the window sets its own below)
        }
    } else if message == WM_NCDESTROY {
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(cursor_proc), id);
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wp, lp) }
}

pub(super) fn install_cursor_proc(hwnd: HWND) {
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(cursor_proc), CURSOR_SUBCLASS_ID, 0);
    }
}

pub(super) fn release_cursor_hide() {
    CURSOR_HIDDEN_AT.with(|c| c.set(None));
}

/// Visible main-window UI floating over a video area (window coords, logical px), shared with the editor.
pub(crate) fn occluder_rect() -> Option<(f32, f32, f32, f32)> {
    OCCLUDER.with(|o| o.get())
}

pub(crate) fn mark_scripted_input() {
    SCRIPTED_INPUT.with(|s| s.set(true));
}

/// True when the OS cursor has not moved since the previous call. Windows sends a fake WM_MOUSEMOVE whenever
/// a window under a still cursor is shown, hidden or moved (the overlay window going away in fullscreen);
/// those must not count as "the user moved the mouse" or the controls/cursor never stay hidden.
pub(super) fn synthetic_pointer_move() -> bool {
    if SCRIPTED_INPUT.with(|s| s.get()) {
        return false;
    }
    let mut pt = POINT::default();
    if unsafe { GetCursorPos(&mut pt) }.is_err() {
        return false;
    }
    LAST_POINTER.with(|last| {
        let same = last.get() == (pt.x, pt.y);
        last.set((pt.x, pt.y));
        same
    })
}

/// Owner, 2026-11-17: "the cursor does not hide, neither with the buttons nor by itself". Slint only applies
/// `mouse-cursor` while it processes a pointer event, so switching it to `none` on an idle timer (no mouse
/// movement, by definition) never reached the screen. While the player wants it hidden and the cursor is
/// inside `area` over one of our windows, the cursor is cleared directly; the next real mouse move makes the
/// window set its cursor again and wakes the controls.
pub(super) fn hide_cursor_over(area: RECT) {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, SetCursor, WindowFromPoint};
    unsafe {
        let mut pt = POINT::default();
        if GetCursorPos(&mut pt).is_err()
            || pt.x < area.left
            || pt.x >= area.right
            || pt.y < area.top
            || pt.y >= area.bottom
        {
            release_cursor_hide();
            return;
        }
        let under = WindowFromPoint(pt);
        let mut pid = 0u32;
        if under.0.is_null() {
            return;
        }
        GetWindowThreadProcessId(under, Some(&mut pid));
        if pid != GetCurrentProcessId() {
            release_cursor_hide();
            return; // another app's window is under the cursor: not ours to change
        }
        if CURSOR_HIDDEN_AT.with(|c| c.replace(Some((pt.x, pt.y)))) != Some((pt.x, pt.y)) {
            let _ = SetCursor(None);
        }
    }
}

pub(super) fn h_ref_smooth(host: &Rc<RefCell<Option<NativeHost>>>, on: bool) {
    if let Some(h) = host.borrow().as_ref() {
        h.overlay.global::<PlayerOverlayVM>().set_smooth_slowmo(on);
    }
}

/// Fullscreen switch with the video faded to black first (owner: the resize/rescale moment must not be
/// visible; "fade to black and back like other players"). Without visible video it switches at once.
pub fn toggle_fullscreen_with_fade(apply: Box<dyn FnOnce()>) {
    let fade = FULLSCREEN_FADE.with(|f| f.borrow().clone());
    match fade {
        Some(fade) => fade(apply),
        None => apply(),
    }
}
