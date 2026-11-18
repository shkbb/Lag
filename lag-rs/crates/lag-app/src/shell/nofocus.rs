//! Mirrors Services/ToastService.cs and Views/StatsHudWindow.cs native non-activating overlay styles.
//! Call immediately before showing an overlay AND after its HWND exists. A thread-local CBT hook
//! stamps styles during CreateWindowEx, closing the C# first-show activation race. No GUI is created
//! by this helper. It also supports shell-B's click-through/capture-excluded HUD.
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::cell::RefCell;
use windows::Win32::UI::Shell::{DefSubclassProc, RemoveWindowSubclass, SetWindowSubclass};
use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::WindowsAndMessaging::*,
};

#[derive(Clone, Copy)]
struct Config {
    click_through: bool,
    exclude: bool,
    topmost: bool,
    owner: isize,
}
#[derive(Default)]
struct Hooks {
    hook: Option<HHOOK>,
    pending: Option<Config>,
    overlays: Vec<(HWND, Config)>,
}
thread_local! {
    static HOOKS: RefCell<Hooks> = RefCell::default();
}
pub fn hwnd(window: &slint::Window) -> Option<HWND> {
    let handle = window.window_handle();
    match handle.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut _)),
        _ => None,
    }
}
fn styles(config: Config) -> u32 {
    let mut style = WS_EX_NOACTIVATE.0 | WS_EX_TOOLWINDOW.0;
    if config.topmost {
        style |= WS_EX_TOPMOST.0;
    }
    if config.click_through {
        style |= WS_EX_TRANSPARENT.0 | WS_EX_LAYERED.0;
    }
    style
}
fn stamp(hwnd: HWND, config: Config) {
    unsafe {
        // Preserve styles when winit rewrites GWL_EXSTYLE during visibility/size changes.
        let bits =
            usize::from(config.click_through) | (usize::from(config.exclude) << 1) | (usize::from(config.topmost) << 2);
        let _ = SetWindowSubclass(hwnd, Some(overlay_proc), 0x4c4147, bits);
        if config.owner != 0 {
            let _ = SetWindowLongPtrW(hwnd, GWLP_HWNDPARENT, config.owner);
        }
        let ex = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32;
        let cleared = ex & !(WS_EX_APPWINDOW.0 | if config.topmost { 0 } else { WS_EX_TOPMOST.0 });
        SetWindowLongW(hwnd, GWL_EXSTYLE, (cleared | styles(config)) as i32);
        if config.exclude {
            let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        }
        let z = if config.topmost { HWND_TOPMOST } else { HWND_TOP };
        let _ = SetWindowPos(hwnd, Some(z), 0, 0, 0, 0, SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_FRAMECHANGED);
    }
}
unsafe extern "system" fn overlay_proc(
    hwnd: HWND,
    message: u32,
    wp: WPARAM,
    lp: LPARAM,
    id: usize,
    bits: usize,
) -> LRESULT {
    let config = Config { click_through: bits & 1 != 0, exclude: bits & 2 != 0, topmost: bits & 4 != 0, owner: 0 };
    if message == WM_MOUSEACTIVATE {
        return LRESULT(MA_NOACTIVATE as isize);
    }
    if message == WM_STYLECHANGING && wp.0 as i32 == GWL_EXSTYLE.0 {
        let style = unsafe { &mut *(lp.0 as *mut STYLESTRUCT) };
        let cleared = style.styleNew & !(WS_EX_APPWINDOW.0 | if config.topmost { 0 } else { WS_EX_TOPMOST.0 });
        style.styleNew = cleared | styles(config);
    }
    if message == WM_WINDOWPOSCHANGING {
        let position = unsafe { &mut *(lp.0 as *mut WINDOWPOS) };
        position.flags |= SWP_NOACTIVATE;
    }
    if message == WM_NCDESTROY {
        unsafe {
            let _ = RemoveWindowSubclass(hwnd, Some(overlay_proc), id);
        }
    }
    unsafe { DefSubclassProc(hwnd, message, wp, lp) }
}
unsafe extern "system" fn cbt(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if code == HCBT_CREATEWND as i32 {
        let create = unsafe { &mut *(lp.0 as *mut CBT_CREATEWNDW) };
        let cs = unsafe { &mut *create.lpcs };
        let hwnd = HWND(wp.0 as *mut _);
        let mut class = [0u16; 64];
        let len = unsafe { GetClassNameW(hwnd, &mut class) };
        // Renderer helper/dummy windows must not consume the Slint overlay's guard.
        if cs.style as u32 & WS_CHILD.0 == 0
            && String::from_utf16_lossy(&class[..len.max(0) as usize]) == "Window Class"
        {
            let config = HOOKS.with(|hooks| {
                let mut hooks = hooks.borrow_mut();
                if let Some(config) = hooks.pending.take() {
                    hooks.overlays.push((hwnd, config));
                    Some(config)
                } else {
                    None
                }
            });
            if let Some(config) = config {
                if config.owner != 0 {
                    cs.hwndParent = HWND(config.owner as *mut _);
                }
                let cleared = cs.dwExStyle.0 & !(WS_EX_APPWINDOW.0 | if config.topmost { 0 } else { WS_EX_TOPMOST.0 });
                cs.dwExStyle = windows::Win32::UI::WindowsAndMessaging::WINDOW_EX_STYLE(cleared | styles(config));
                unsafe {
                    SetWindowLongW(hwnd, GWL_EXSTYLE, cs.dwExStyle.0 as i32);
                }
            }
        }
    } else if code == HCBT_ACTIVATE as i32 {
        if HOOKS.with(|hooks| hooks.borrow().overlays.iter().any(|(hwnd, _)| hwnd.0 as usize == wp.0)) {
            return LRESULT(1);
        }
    } else if code == HCBT_DESTROYWND as i32 {
        HOOKS.with(|hooks| hooks.borrow_mut().overlays.retain(|(hwnd, _)| hwnd.0 as usize != wp.0));
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}
pub fn cancel_pending_creation() {
    HOOKS.with(|hooks| hooks.borrow_mut().pending = None);
}
fn apply_config(window: &slint::Window, config: Config) {
    if let Some(hwnd) = hwnd(window) {
        stamp(hwnd, config);
    } else {
        HOOKS.with(|hooks| {
            let mut hooks = hooks.borrow_mut();
            if hooks.hook.is_none() {
                hooks.hook = unsafe {
                    SetWindowsHookExW(WH_CBT, Some(cbt), None, windows::Win32::System::Threading::GetCurrentThreadId())
                }
                .ok();
            }
            // A failed creation hook must never fall back to a potentially activating first show.
            assert!(hooks.hook.is_some(), "Cannot guarantee non-activating overlay creation");
            hooks.pending = Some(config);
        });
    }
}
/// Reusable toast/HUD contract. On a not-yet-created window this arms the next top-level creation
/// on this UI thread: callers must immediately show that overlay, without creating another window.
pub fn apply_no_focus(window: &slint::Window, click_through: bool, exclude_from_capture: bool) {
    apply_config(window, Config { click_through, exclude: exclude_from_capture, topmost: true, owner: 0 });
}
/// Non-activating owned popup window (no WS_EX_TOPMOST; follows owner Z-order).
pub fn apply_owned_no_focus(window: &slint::Window, owner: HWND) {
    apply_config(window, Config { click_through: false, exclude: false, topmost: false, owner: owner.0 as isize });
}
pub fn show_no_activate(window: &slint::Window) {
    if let Some(hwnd) = hwnd(window) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
            );
        }
    }
}
pub fn show_owned_no_activate(window: &slint::Window) {
    if let Some(hwnd) = hwnd(window) {
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_SHOWWINDOW,
            );
        }
    }
}
pub fn exclusive_fullscreen() -> bool {
    unsafe { windows::Win32::UI::Shell::SHQueryUserNotificationState() }.is_ok_and(|state| state.0 == 3)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overlays_never_activate_and_hud_is_click_through() {
        for click_through in [false, true] {
            let s = styles(Config { click_through, exclude: true, topmost: true, owner: 0 });
            assert_eq!(s & WS_EX_NOACTIVATE.0, WS_EX_NOACTIVATE.0);
            assert_eq!(s & WS_EX_TOOLWINDOW.0, WS_EX_TOOLWINDOW.0);
            assert_eq!(s & WS_EX_TOPMOST.0, WS_EX_TOPMOST.0);
            assert_eq!(s & WS_EX_TRANSPARENT.0 != 0, click_through);
            assert_eq!(s & WS_EX_LAYERED.0 != 0, click_through);
        }
        let owned = styles(Config { click_through: false, exclude: false, topmost: false, owner: 123 });
        assert_eq!(owned & WS_EX_NOACTIVATE.0, WS_EX_NOACTIVATE.0);
        assert_eq!(owned & WS_EX_TOOLWINDOW.0, WS_EX_TOOLWINDOW.0);
        assert_eq!(owned & WS_EX_TOPMOST.0, 0);
        assert_eq!(owned & WS_EX_APPWINDOW.0, 0);
    }
}
