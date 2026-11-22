//! LagUpdate.exe: installs a Lag update while showing it. Lag downloads the installer (with its progress on the
//! startup splash), copies this helper out of its install folder (the installer replaces that folder) and exits;
//! the helper shows the same splash with "Installing the update...", waits for Lag to be gone, runs the installer
//! silently, starts the updated Lag and stays until Lag is up (its own splash takes over), so the user sees the
//! whole update instead of Lag silently vanishing (owner, 2026-11-22).
//!
//! `LagUpdate.exe --installer <setup.exe> --app <Lag.exe> --wait-pid <pid> [--mode dark|light|system|neon]
//!   [--installing <text>] [--starting <text>] [--failed <text>] [--installer-args <args>]`
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use slint::ComponentHandle;
use std::path::PathBuf;
use std::time::{Duration, Instant};

slint::include_modules!();

/// What the installer gets unless told otherwise: no wizard, no message boxes, no reboot, no relaunch of its own
/// (this helper starts Lag, so it can wait for it).
const INSTALLER_ARGS: &str = "/VERYSILENT /SUPPRESSMSGBOXES /NORESTART";

struct Args {
    installer: PathBuf,
    app: PathBuf,
    wait_pid: Option<u32>,
    mode: String,
    installing: String,
    starting: String,
    failed: String,
    installer_args: String,
}

fn args() -> Option<Args> {
    let a: Vec<String> = std::env::args().collect();
    let get = |name: &str| a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned();
    Some(Args {
        installer: get("--installer")?.into(),
        app: get("--app")?.into(),
        wait_pid: get("--wait-pid").and_then(|p| p.parse().ok()),
        mode: get("--mode").unwrap_or_else(|| "dark".into()),
        installing: get("--installing").unwrap_or_else(|| "Installing the update...".into()),
        starting: get("--starting").unwrap_or_else(|| "Starting...".into()),
        failed: get("--failed").unwrap_or_else(|| "The update did not install".into()),
        installer_args: get("--installer-args").unwrap_or_else(|| INSTALLER_ARGS.into()),
    })
}

/// Centres the window on the monitor under the cursor (where Lag's splash was).
fn centre(window: &SplashWindow) {
    unsafe {
        use windows::Win32::Foundation::POINT;
        use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, MONITORINFO, MONITOR_DEFAULTTOPRIMARY};
        use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
        use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let mon = MonitorFromPoint(pt, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(mon, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        if GetMonitorInfoW(mon, &mut info).as_bool() {
            let w = (300.0 * dx as f32 / 96.0) as i32;
            let h = (340.0 * dx as f32 / 96.0) as i32;
            let r = info.rcWork;
            window.window().set_position(slint::PhysicalPosition::new(r.left + (r.right - r.left - w) / 2, r.top + (r.bottom - r.top - h) / 2));
        }
    }
}

/// Waits (up to `limit`) for a process to end.
fn wait_for_exit(pid: u32, limit: Duration) {
    unsafe {
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE};
        if let Ok(h) = OpenProcess(PROCESS_SYNCHRONIZE, false, pid) {
            let _ = WaitForSingleObject(h, limit.as_millis() as u32);
            let _ = CloseHandle(h);
        }
    }
}

/// Runs the installer and waits for it. Ok(exit code) / Err(why it could not start).
fn run_installer(installer: &std::path::Path, params: &str) -> Result<u32, String> {
    use std::os::windows::ffi::OsStrExt;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject};
    use windows::Win32::UI::Shell::{ShellExecuteExW, SEE_MASK_FLAG_NO_UI, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW};
    use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
    let file: Vec<u16> = installer.as_os_str().encode_wide().chain(Some(0)).collect();
    let params: Vec<u16> = params.encode_utf16().chain(Some(0)).collect();
    // ShellExecute (not CreateProcess): an installer that needs elevation can still ask for it
    let mut info = SHELLEXECUTEINFOW {
        cbSize: std::mem::size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_FLAG_NO_UI,
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(params.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    unsafe {
        ShellExecuteExW(&mut info).map_err(|e| e.to_string())?;
        if info.hProcess.is_invalid() {
            return Ok(0);
        }
        // an installer never takes this long; past it, start Lag anyway
        let _ = WaitForSingleObject(info.hProcess, 10 * 60 * 1000);
        let mut code = 0u32;
        let _ = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        Ok(code)
    }
}

/// Lag holds this mutex while it runs (lag-app startup.rs).
fn lag_is_up() -> bool {
    unsafe {
        use windows::core::w;
        use windows::Win32::Foundation::CloseHandle;
        use windows::Win32::System::Threading::{OpenMutexW, SYNCHRONIZATION_SYNCHRONIZE};
        match OpenMutexW(SYNCHRONIZATION_SYNCHRONIZE, false, w!("LagAppSingleInstanceMutex")) {
            Ok(h) => {
                let _ = CloseHandle(h);
                true
            }
            Err(_) => false,
        }
    }
}

fn main() {
    let Some(a) = args() else { return };
    let Ok(window) = SplashWindow::new() else {
        // no window: still update and start Lag
        if let Some(pid) = a.wait_pid {
            wait_for_exit(pid, Duration::from_secs(15));
        }
        let _ = run_installer(&a.installer, &a.installer_args);
        let _ = std::process::Command::new(&a.app).spawn();
        return;
    };
    window.set_theme_mode(a.mode.clone().into());
    window.set_text(a.installing.clone().into());
    window.set_show_progress(true);
    window.set_busy(true);
    centre(&window);
    let _ = window.show();
    let weak = window.as_weak();
    std::thread::spawn(move || {
        if let Some(pid) = a.wait_pid {
            wait_for_exit(pid, Duration::from_secs(15));
        }
        let result = run_installer(&a.installer, &a.installer_args);
        let ok = matches!(result, Ok(0));
        if !ok {
            let why = match &result {
                Ok(code) => format!("exit code {code}"),
                Err(e) => e.clone(),
            };
            eprintln!("LagUpdate: installer failed: {why}");
        }
        let text = if ok { a.starting.clone() } else { a.failed.clone() };
        let w = weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = w.upgrade() {
                w.set_text(text.into());
            }
        });
        if !ok {
            std::thread::sleep(Duration::from_millis(2500));
        }
        // the updated Lag (or the old one, still there when the install failed)
        let started = std::process::Command::new(&a.app).spawn().is_ok();
        // stay until Lag runs (its own splash appears), at most 20 s
        let since = Instant::now();
        while started && !lag_is_up() && since.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(500));
        let _ = slint::invoke_from_event_loop(|| {
            let _ = slint::quit_event_loop();
        });
    });
    let _ = slint::run_event_loop_until_quit();
}
