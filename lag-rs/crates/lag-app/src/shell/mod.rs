//! Port of `App.axaml.cs`: tray, hotkey, HUD and startup service wiring.
use crate::app_ctx::AppCtx;
use crate::ComponentHandle;
use std::{cell::RefCell, rc::Rc};

pub mod autoscroll;
pub mod autostart;
pub mod hotkeys;
pub mod hud;
pub mod shortcut_icon;
pub use crate::services::nofocus;
pub mod fade_cover;
pub mod tray;
pub mod updater;
pub mod window_geometry;

thread_local! {
    static SERVICES: RefCell<Option<Services>> = const { RefCell::new(None) };
}
struct Services {
    _tray: Option<tray::Tray>,
    _hotkeys: hotkeys::Hotkeys,
    _hud: Rc<hud::Hud>,
    _update_timer: slint::Timer,
}

pub fn init(ui: &crate::AppWindow, ctx: &Rc<AppCtx>) {
    SERVICES.with(|slot| {
        if slot.borrow().is_some() {
            return;
        }
        let tray = match tray::Tray::new(ui, ctx) {
            Ok(tray) => Some(tray),
            Err(e) => {
                log::warn!("Tray menu localization failed: {e}");
                None
            }
        };
        let hotkeys = hotkeys::Hotkeys::new(ui, ctx);
        let hud = hud::Hud::new(ctx);
        autostart::init(ctx);
        let update_timer = slint::Timer::default();
        // download a newer Lag for the next start (the splash's own check, when it ran, is still going or done)
        update_timer.start(slint::TimerMode::SingleShot, std::time::Duration::from_secs(3), updater::background_fetch);
        if ctx.settings().start_minimized {
            start_minimized(ui);
        }
        *slot.borrow_mut() = Some(Services { _tray: tray, _hotkeys: hotkeys, _hud: hud, _update_timer: update_timer });
    });
}

// ui.run() is in the immutable shared main.rs. Guard its first native creation before it shows,
// matching C#'s minimized/no-taskbar startup, then hide on the first event-loop turn.
thread_local! {static STARTUP_HOOK: RefCell<Option<windows::Win32::UI::WindowsAndMessaging::HHOOK>> = const {RefCell::new(None)};}
unsafe extern "system" fn startup_hook(
    code: i32,
    wp: windows::Win32::Foundation::WPARAM,
    lp: windows::Win32::Foundation::LPARAM,
) -> windows::Win32::Foundation::LRESULT {
    use windows::Win32::{
        Foundation::{HWND, LRESULT},
        UI::WindowsAndMessaging::*,
    };
    if code == HCBT_CREATEWND as i32 {
        let create = unsafe { &mut *(lp.0 as *mut CBT_CREATEWNDW) };
        let cs = unsafe { &mut *create.lpcs };
        if cs.style as u32 & WS_CHILD.0 == 0
            && !cs.lpszName.is_null()
            && unsafe { cs.lpszName.to_string() }.is_ok_and(|s| s == "Lag")
        {
            cs.style |= WS_MINIMIZE.0 as i32;
            cs.dwExStyle = (cs.dwExStyle & !WS_EX_APPWINDOW) | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        }
    }
    if code == HCBT_ACTIVATE as i32 {
        let mut title = [0u16; 16];
        let n = unsafe { GetWindowTextW(HWND(wp.0 as *mut _), &mut title) };
        if n > 0 && String::from_utf16_lossy(&title[..n as usize]) == "Lag" {
            return LRESULT(1);
        }
    }
    unsafe { CallNextHookEx(None, code, wp, lp) }
}
fn start_minimized(ui: &crate::AppWindow) {
    use windows::Win32::UI::WindowsAndMessaging::*;
    STARTUP_HOOK.with(|h| {
        *h.borrow_mut() = unsafe {
            SetWindowsHookExW(WH_CBT, Some(startup_hook), None, windows::Win32::System::Threading::GetCurrentThreadId())
        }
        .ok()
    });
    ui.window().set_minimized(true);
    let weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::ZERO, move || {
        if let Some(ui) = weak.upgrade() {
            let _ = ui.hide();
            if let Some(hwnd) = nofocus::hwnd(ui.window()) {
                unsafe {
                    let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
                    SetWindowLongPtrW(
                        hwnd,
                        GWL_EXSTYLE,
                        (ex & !((WS_EX_TOOLWINDOW.0 | WS_EX_NOACTIVATE.0) as isize)) | WS_EX_APPWINDOW.0 as isize,
                    );
                }
            }
        }
        STARTUP_HOOK.with(|h| {
            if let Some(h) = h.borrow_mut().take() {
                unsafe {
                    let _ = UnhookWindowsHookEx(h);
                }
            }
        });
    });
}

/// Drops tray/hotkeys/HUD NOW, while their worker threads are alive. Must run before main returns: left in
/// the thread-local, they are dropped during process exit AFTER Windows has killed those threads, and
/// `Hotkeys::drop` joining a killed thread panics ("threads should not terminate unexpectedly") -> abort.
pub fn shutdown() {
    SERVICES.with(|s| {
        s.borrow_mut().take();
    });
}

/// Starts a new Lag (it waits for this one to close) and exits this one: a new interface theme takes effect.
pub fn restart() {
    match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).arg("--after-restart").spawn()) {
        Ok(_) => exit(),
        Err(e) => log::warn!("[Shutdown] restart failed: {e}"),
    }
}

/// C# TrayIcon_ExitClicked / ExitWatchdog. Also used after an updater handoff.
pub fn exit() {
    // Watchdog (C# ExitWatchdog): only for a hung teardown. Longer than the bounded recorder stop (3 s) in
    // main, so it never kills the encoder mid-stop; TerminateProcess skips thread-local destructors.
    std::thread::spawn(|| {
        std::thread::sleep(std::time::Duration::from_millis(6000));
        log::warn!("[Shutdown] watchdog: teardown hung, terminating.");
        log::logger().flush();
        unsafe {
            use windows::Win32::System::Threading::{GetCurrentProcess, TerminateProcess};
            let _ = TerminateProcess(GetCurrentProcess(), 0);
        }
    });
    tray::flush_settings();
    SERVICES.with(|s| {
        s.borrow_mut().take();
    });
    let _ = slint::quit_event_loop();
}
