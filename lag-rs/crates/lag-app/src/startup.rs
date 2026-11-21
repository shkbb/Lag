//! Startup helpers (Program.cs): logging, timer resolution, single instance, settings.

use lag_core::settings::SettingsData;
use windows::core::w;
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE};
use windows::Win32::Media::timeBeginPeriod;
use windows::Win32::System::Threading::CreateMutexW;

pub fn init_logging() -> Option<lag_core::filelog::LogGuard> {
    let opts = lag_core::filelog::Options::for_user()?;
    lag_core::filelog::FileLog::install(opts).ok()
}

pub fn raise_timer_resolution() {
    unsafe {
        let _ = timeBeginPeriod(1);
    }
}

/// Holds the named mutex for the process lifetime.
pub struct SingleInstance(HANDLE);

impl Drop for SingleInstance {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

/// Same mutex name as the C# app, so the C# and Rust builds also exclude each other.
pub fn acquire_single_instance() -> Option<SingleInstance> {
    unsafe {
        let h = CreateMutexW(None, true, w!("LagAppSingleInstanceMutex")).ok()?;
        if GetLastError() == ERROR_ALREADY_EXISTS {
            let _ = CloseHandle(h);
            return None;
        }
        Some(SingleInstance(h))
    }
}

pub fn load_settings() -> SettingsData {
    let path = lag_core::settings::default_path();
    let loaded = path.as_deref().and_then(|p| SettingsData::load(p).ok().flatten());
    let existed = loaded.is_some();
    let mut settings = loaded.unwrap_or_default();
    // 2.1.2: System becomes the main theme once (new users, and users 2.0 put on GTA VI); later choices stick
    if settings.introduce_system_theme(!existed) && existed {
        if let Some(p) = path.as_deref() {
            if let Err(e) = settings.save(p) {
                log::warn!("[Settings] could not save the System theme switch: {e}");
            }
        }
    }
    settings
}

/// Startup splash (ui/splash.slint), centred on the monitor under the cursor. None if it cannot be created.
pub fn show_splash(settings: &lag_core::settings::SettingsData) -> Option<crate::SplashWindow> {
    use slint::ComponentHandle;
    let splash = crate::SplashWindow::new().ok()?;
    splash.set_theme_mode(settings.theme.as_deref().unwrap_or("dark").into());
    splash.set_text(lag_core::i18n::tr("Splash_Starting").into());
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
            let x = r.left + (r.right - r.left - w) / 2;
            let y = r.top + (r.bottom - r.top - h) / 2;
            splash.window().set_position(slint::PhysicalPosition::new(x, y));
        }
    }
    if let Err(e) = splash.show() {
        log::warn!("[Startup] splash failed: {e}");
        return None;
    }
    Some(splash)
}
