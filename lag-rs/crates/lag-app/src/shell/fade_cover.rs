//! Full-monitor black cover for the player's fullscreen switch (owner, 2026-11-16: "fade everything to black
//! and back, like other players, so the window rescale is never visible"). A plain Win32 layered popup:
//! topmost, click-through, never activated, no taskbar entry. Only its alpha changes.
use std::cell::Cell;
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, GetStockObject, MonitorFromWindow, BLACK_BRUSH, HBRUSH, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::*;

unsafe extern "system" fn proc_(h: HWND, m: u32, w: WPARAM, l: LPARAM) -> LRESULT {
    if m == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    if m == WM_NCHITTEST {
        return LRESULT(HTTRANSPARENT as isize);
    }
    unsafe { DefWindowProcW(h, m, w, l) }
}

thread_local! { static COVER: Cell<isize> = const { Cell::new(0) }; }

fn hwnd() -> Option<HWND> {
    let existing = COVER.with(|c| c.get());
    if existing != 0 {
        return Some(HWND(existing as *mut _));
    }
    unsafe {
        let hinst = GetModuleHandleW(None).ok()?;
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(proc_),
            hInstance: hinst.into(),
            hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
            lpszClassName: w!("LagFadeCover"),
            ..Default::default()
        };
        RegisterClassExW(&wc);
        let h = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_TRANSPARENT,
            w!("LagFadeCover"),
            w!(""),
            WS_POPUP,
            0,
            0,
            1,
            1,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .ok()?;
        COVER.with(|c| c.set(h.0 as isize));
        Some(h)
    }
}

fn monitor_rect(main: HWND) -> RECT {
    unsafe {
        let m = MonitorFromWindow(main, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let _ = GetMonitorInfoW(m, &mut info);
        info.rcMonitor
    }
}

/// Shows the cover over `main`'s monitor at `alpha` (0 = invisible, 255 = black).
pub fn show(main: HWND, alpha: u8) {
    let Some(h) = hwnd() else { return };
    let r = monitor_rect(main);
    unsafe {
        let _ = SetLayeredWindowAttributes(h, windows::Win32::Foundation::COLORREF(0), alpha, LWA_ALPHA);
        let _ = SetWindowPos(
            h,
            Some(HWND_TOPMOST),
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
}

pub fn set_alpha(alpha: u8) {
    let Some(h) = hwnd() else { return };
    unsafe {
        let _ = SetLayeredWindowAttributes(h, windows::Win32::Foundation::COLORREF(0), alpha, LWA_ALPHA);
    }
}

pub fn hide() {
    let existing = COVER.with(|c| c.get());
    if existing != 0 {
        unsafe {
            let _ = ShowWindow(HWND(existing as *mut _), SW_HIDE);
        }
    }
}
