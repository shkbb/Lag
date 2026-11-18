//! Native Win32 child video host window for zero-copy libmpv (`vo=gpu-next`, `wid=<hwnd>`).
use crate::{Error, Result};
use std::cell::Cell;
use std::sync::Once;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, GetStockObject, SetWindowRgn, BLACK_BRUSH, HBRUSH,
    HGDIOBJ, RGN_AND, RGN_DIFF,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, IsWindow, RegisterClassExW, SetWindowPos, ShowWindow, CS_HREDRAW,
    CS_OWNDC, CS_VREDRAW, HTTRANSPARENT, MA_NOACTIVATE, SWP_NOACTIVATE, SWP_NOOWNERZORDER, SWP_NOZORDER, SW_HIDE,
    SW_SHOWNA, WM_ERASEBKGND, WM_MOUSEACTIVATE, WM_NCHITTEST, WNDCLASSEXW, WS_CHILD, WS_CLIPCHILDREN, WS_CLIPSIBLINGS,
    WS_EX_NOPARENTNOTIFY,
};

static CLASS_INIT: Once = Once::new();

unsafe extern "system" fn wnd_proc(hwnd: HWND, message: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    match message {
        // Pass all pointer hit-testing to the parent Slint window (or owned overlay above).
        WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_ERASEBKGND => unsafe { DefWindowProcW(hwnd, message, wp, lp) },
        _ => unsafe { DefWindowProcW(hwnd, message, wp, lp) },
    }
}

fn ensure_class_registered() -> Result<windows::Win32::Foundation::HINSTANCE> {
    let hinst = unsafe { GetModuleHandleW(None) }
        .map(|h| windows::Win32::Foundation::HINSTANCE(h.0))
        .map_err(|e| Error(format!("GetModuleHandleW failed: {e}")))?;

    let mut reg_err: Option<Error> = None;
    CLASS_INIT.call_once(|| {
        let brush = unsafe { HBRUSH(GetStockObject(BLACK_BRUSH).0) };
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_OWNDC | CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinst,
            hbrBackground: brush,
            lpszClassName: w!("LagNativeVideoSurface"),
            ..Default::default()
        };
        if unsafe { RegisterClassExW(&wc) } == 0 {
            let err = windows::core::Error::from_thread();
            // ERROR_CLASS_ALREADY_EXISTS (1410) is benign across modules/tests.
            if err.code().0 as u32 & 0xFFFF != 1410 {
                reg_err = Some(Error(format!("RegisterClassExW failed: {err}")));
            }
        }
    });
    if let Some(err) = reg_err {
        return Err(err);
    }
    Ok(hinst)
}

/// Child HWND owned by the parent Slint window (`WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN`).
/// Must be created, moved/resized, shown/hidden, and dropped on the parent window's UI thread.
pub struct NativeVideoWindow {
    hwnd: HWND,
    visible: Cell<bool>,
    bounds: Cell<(i32, i32, i32, i32)>,
    exclusion: Cell<Option<(i32, i32, i32, i32)>>,
    /// Corner radius (physical px) the surface is clipped to; 0 = square (fullscreen).
    corner_radius: Cell<i32>,
    /// Only this part of the surface (surface-local physical px) is shown; None = all of it. The editor moves
    /// and zooms the video past its preview area (reframe), the area clips it like the old Slint stage did.
    clip_rect: Cell<Option<(i32, i32, i32, i32)>>,
    /// false = only the top corners are rounded (editor: the transport bar sits right under the video)
    round_bottom: Cell<bool>,
}

impl NativeVideoWindow {
    pub fn new(parent_hwnd: isize) -> Result<Self> {
        if parent_hwnd == 0 {
            return Err(Error("null parent HWND".into()));
        }
        let parent = HWND(parent_hwnd as *mut _);
        if !unsafe { IsWindow(Some(parent)).as_bool() } {
            return Err(Error("invalid parent HWND".into()));
        }
        let hinst = ensure_class_registered()?;
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOPARENTNOTIFY,
                w!("LagNativeVideoSurface"),
                w!(""),
                WS_CHILD | WS_CLIPSIBLINGS | WS_CLIPCHILDREN,
                0,
                0,
                1,
                1,
                Some(parent),
                None,
                Some(hinst),
                None,
            )
        }
        .map_err(|e| Error(format!("CreateWindowExW failed: {e}")))?;

        Ok(Self {
            hwnd,
            visible: Cell::new(false),
            bounds: Cell::new((0, 0, 1, 1)),
            exclusion: Cell::new(None),
            corner_radius: Cell::new(0),
            clip_rect: Cell::new(None),
            round_bottom: Cell::new(true),
        })
    }

    pub fn wid(&self) -> isize {
        self.hwnd.0 as isize
    }

    /// Position and resize the child surface in physical pixels relative to parent client origin.
    pub fn set_bounds(&self, x: i32, y: i32, width: i32, height: i32) {
        if self.hwnd.0.is_null() {
            return;
        }
        let w = width.max(1);
        let h = height.max(1);
        let next = (x, y, w, h);
        if self.bounds.get() == next {
            return;
        }
        self.bounds.set(next);
        unsafe {
            let _ = SetWindowPos(self.hwnd, None, x, y, w, h, SWP_NOACTIVATE | SWP_NOZORDER | SWP_NOOWNERZORDER);
        }
        // the region was built for the old size
        if self.exclusion.get().is_some() || self.corner_radius.get() > 0 || self.clip_rect.get().is_some() {
            self.apply_region();
        }
    }

    pub fn set_round_bottom(&self, round: bool) {
        if self.hwnd.0.is_null() || self.round_bottom.get() == round {
            return;
        }
        self.round_bottom.set(round);
        self.apply_region();
    }

    /// Show only `rect` (surface-local physical px) of the surface; the corner radius rounds this rect.
    pub fn set_clip_rect(&self, rect: Option<(i32, i32, i32, i32)>) {
        if self.hwnd.0.is_null() || self.clip_rect.get() == rect {
            return;
        }
        self.clip_rect.set(rect);
        self.apply_region();
    }

    /// Clip the surface to rounded corners (physical px) so the square child window does not stick out of the
    /// rounded video frame drawn by the UI (owner, 2026-11-17: visible corners on the light theme).
    pub fn set_corner_radius(&self, radius: i32) {
        let radius = radius.max(0);
        if self.hwnd.0.is_null() || self.corner_radius.get() == radius {
            return;
        }
        self.corner_radius.set(radius);
        self.apply_region();
    }

    /// Cut a rectangle (surface-local physical px) out of the video so main-window UI drawn there (flyouts,
    /// tooltips) stays visible instead of being covered by the child window. `None` = full surface.
    pub fn set_exclusion(&self, rect: Option<(i32, i32, i32, i32)>) {
        if self.hwnd.0.is_null() || self.exclusion.get() == rect {
            return;
        }
        self.exclusion.set(rect);
        self.apply_region();
    }

    /// Window region = (rounded) surface minus the cut-out; no region at all when neither applies.
    fn apply_region(&self) {
        let (_, _, w, h) = self.bounds.get();
        let radius = self.corner_radius.get();
        let rect = self.exclusion.get();
        let clip = self.clip_rect.get();
        unsafe {
            if radius == 0 && rect.is_none() && clip.is_none() {
                let _ = SetWindowRgn(self.hwnd, None, true);
                return;
            }
            let (cx, cy, cw, ch) = clip.unwrap_or((0, 0, w, h));
            // CreateRoundRectRgn excludes the right/bottom edge: +1 to cover the whole area
            let full = if radius > 0 && !self.round_bottom.get() {
                // round rect reaching past the bottom edge, cut back to the area: square bottom corners
                let r = CreateRoundRectRgn(cx, cy, cx + cw + 1, cy + ch + 1 + radius * 2, radius * 2, radius * 2);
                let area = CreateRectRgn(cx, cy, cx + cw, cy + ch);
                let _ = CombineRgn(Some(r), Some(r), Some(area), RGN_AND);
                let _ = DeleteObject(HGDIOBJ(area.0));
                r
            } else if radius > 0 {
                CreateRoundRectRgn(cx, cy, cx + cw + 1, cy + ch + 1, radius * 2, radius * 2)
            } else {
                CreateRectRgn(cx, cy, cx + cw, cy + ch)
            };
            if let Some((x, y, rw, rh)) = rect {
                let hole = CreateRectRgn(x, y, x + rw, y + rh);
                let _ = CombineRgn(Some(full), Some(full), Some(hole), RGN_DIFF);
                let _ = DeleteObject(HGDIOBJ(hole.0));
            }
            // the system owns `full` after a successful SetWindowRgn
            if SetWindowRgn(self.hwnd, Some(full), true) == 0 {
                let _ = DeleteObject(HGDIOBJ(full.0));
            }
        }
    }

    pub fn show(&self) {
        self.set_visible(true);
    }

    pub fn hide(&self) {
        self.set_visible(false);
    }

    pub fn set_visible(&self, visible: bool) {
        if self.hwnd.0.is_null() || self.visible.get() == visible {
            return;
        }
        self.visible.set(visible);
        unsafe {
            let _ = ShowWindow(self.hwnd, if visible { SW_SHOWNA } else { SW_HIDE });
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible.get()
    }

    pub fn bounds(&self) -> (i32, i32, i32, i32) {
        self.bounds.get()
    }

    pub fn destroy(&mut self) {
        if !self.hwnd.0.is_null() {
            unsafe {
                if IsWindow(Some(self.hwnd)).as_bool() {
                    let _ = DestroyWindow(self.hwnd);
                }
            }
            self.hwnd = HWND(std::ptr::null_mut());
            self.visible.set(false);
        }
    }
}

impl Drop for NativeVideoWindow {
    fn drop(&mut self) {
        self.destroy();
    }
}
