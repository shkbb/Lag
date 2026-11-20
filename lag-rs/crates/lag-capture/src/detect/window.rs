//! Facts about one top-level window, read from Win32: its process, class, title, whether it really fills the
//! recorded monitor, whether it has focus. Nothing here decides anything (verdict.rs does).
use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::WindowsAndMessaging::*;

/// Smaller windows are splash screens, tooltips and helpers, not something to record.
const MIN_SIDE: i32 = 200;

pub(super) struct WindowFacts {
    pub hwnd: isize,
    pub pid: u32,
    /// Full path of the process image.
    pub path: String,
    pub exe: String,
    pub class: String,
    pub title: String,
    /// Fills the recorded monitor with no title bar (borderless or exclusive fullscreen).
    pub fullscreen: bool,
    pub focused: bool,
}

impl WindowFacts {
    /// None for windows that cannot be a game: hidden, minimized, tool windows, tiny ones, Lag's own.
    pub(super) fn read(hwnd: isize, monitor_device_id: Option<&str>, foreground: isize) -> Option<Self> {
        let window = HWND(hwnd as *mut _);
        unsafe {
            if hwnd == 0 || !IsWindowVisible(window).as_bool() || IsIconic(window).as_bool() {
                return None;
            }
            if GetWindowLongW(window, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0 {
                return None;
            }
            let mut rect = RECT::default();
            GetWindowRect(window, &mut rect).ok()?;
            if rect.right - rect.left < MIN_SIDE || rect.bottom - rect.top < MIN_SIDE {
                return None;
            }
            let mut pid = 0;
            GetWindowThreadProcessId(window, Some(&mut pid));
            if pid == 0 || pid == std::process::id() {
                return None;
            }
            let path = process_path(pid)?;
            let exe = path.rsplit(['\\', '/']).next()?.to_owned();
            let mut class = [0u16; 256];
            let n = GetClassNameW(window, &mut class).max(0) as usize;
            let class = String::from_utf16_lossy(&class[..n]);
            let mut title = [0u16; 256];
            let n = GetWindowTextW(window, &mut title).max(0) as usize;
            let title = String::from_utf16_lossy(&title[..n]);
            Some(Self {
                hwnd,
                pid,
                path,
                exe,
                class,
                title,
                fullscreen: is_fullscreen(window, rect, monitor_device_id),
                focused: hwnd == foreground,
            })
        }
    }
}

/// Full image path of a process.
pub(super) fn process_path(pid: u32) -> Option<String> {
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut path = [0u16; 32768];
        let mut len = path.len() as u32;
        let queried = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(path.as_mut_ptr()), &mut len);
        let _ = CloseHandle(process);
        queried.ok()?;
        Some(String::from_utf16_lossy(&path[..len as usize]))
    }
}

/// `rect` covers the whole `monitor`, and that monitor is the recorded one (`selected` device, or the primary).
pub(super) fn covers(rect: RECT, monitor: RECT, flags: u32, device: &str, selected: Option<&str>) -> bool {
    let on_recorded = match selected.filter(|s| !s.is_empty()) {
        Some(s) => s.eq_ignore_ascii_case(device),
        None => flags & 1 != 0,
    };
    on_recorded
        && rect.left <= monitor.left
        && rect.top <= monitor.top
        && rect.right >= monitor.right
        && rect.bottom >= monitor.bottom
}

fn is_fullscreen(window: HWND, rect: RECT, selected: Option<&str>) -> bool {
    unsafe {
        // A maximized window with a title bar is an app, not a fullscreen game: maximized windows overhang the
        // monitor by their borders, so the rectangle test alone counted every maximized window (owner,
        // 2026-11-20: the maximized ChatGPT window was recorded instead of CS2).
        let style = GetWindowLongW(window, GWL_STYLE) as u32;
        if IsZoomed(window).as_bool() && style & WS_CAPTION.0 == WS_CAPTION.0 {
            return false;
        }
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if !GetMonitorInfoW(MonitorFromWindow(window, MONITOR_DEFAULTTONEAREST), &mut info.monitorInfo).as_bool() {
            return false;
        }
        let n = info.szDevice.iter().position(|&c| c == 0).unwrap_or(info.szDevice.len());
        covers(
            rect,
            info.monitorInfo.rcMonitor,
            info.monitorInfo.dwFlags,
            &String::from_utf16_lossy(&info.szDevice[..n]),
            selected,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn monitor_filter_and_exact_edges() {
        let monitor = RECT { left: -1920, top: 0, right: 0, bottom: 1080 };
        assert!(covers(monitor, monitor, 1, "DISPLAY1", None));
        assert!(covers(monitor, monitor, 0, "DISPLAY2", Some("display2")));
        assert!(!covers(monitor, monitor, 0, "DISPLAY2", None));
        assert!(!covers(monitor, monitor, 1, "DISPLAY1", Some("DISPLAY2")));
        assert!(!covers(RECT { left: -1919, ..monitor }, monitor, 1, "DISPLAY1", None));
    }
}
