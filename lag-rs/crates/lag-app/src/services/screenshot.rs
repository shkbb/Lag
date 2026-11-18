//! Mirrors Services/ScreenshotService.cs: GDI CopyFromScreen to Screenshot_yyyy-MM-dd_HH-mm-ss.png.
use std::{mem::size_of, path::PathBuf};
use windows::{
    core::BOOL,
    Win32::{
        Foundation::{LPARAM, RECT},
        Graphics::Gdi::*,
        System::SystemInformation::GetLocalTime,
    },
};

#[derive(Default)]
struct Search {
    requested: Option<String>,
    matched: Option<RECT>,
    primary: Option<RECT>,
}
unsafe extern "system" fn monitor(mon: HMONITOR, _: HDC, _: *mut RECT, data: LPARAM) -> BOOL {
    // The callback is synchronous; data points to Search for the full enumeration.
    let search = unsafe { &mut *(data.0 as *mut Search) };
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if unsafe { GetMonitorInfoW(mon, &mut info.monitorInfo) }.as_bool() {
        if info.monitorInfo.dwFlags & 1 != 0 {
            search.primary = Some(info.monitorInfo.rcMonitor);
        }
        let name = String::from_utf16_lossy(&info.szDevice[..info.szDevice.iter().position(|c| *c == 0).unwrap_or(32)]);
        if search.requested.as_deref() == Some(&name) {
            search.matched = Some(info.monitorInfo.rcMonitor);
        }
    }
    true.into()
}
pub fn capture(device: Option<String>, output_dir: String) -> Option<PathBuf> {
    match capture_inner(device, output_dir) {
        Ok(path) => Some(path),
        Err(e) => {
            log::debug!("[Screenshot] capture failed: {e}");
            None
        }
    }
}
fn capture_inner(device: Option<String>, output_dir: String) -> Result<PathBuf, String> {
    let mut search = Search { requested: device, ..Search::default() };
    unsafe { EnumDisplayMonitors(None, None, Some(monitor), LPARAM((&mut search as *mut Search) as isize)) }
        .ok()
        .map_err(|e| e.to_string())?;
    let bounds = search.matched.or(search.primary).ok_or("No monitor")?;
    let (w, h) = (bounds.right - bounds.left, bounds.bottom - bounds.top);
    if w <= 0 || h <= 0 {
        return Err("Invalid monitor bounds".into());
    }
    struct Gdi {
        screen: HDC,
        memory: HDC,
        bitmap: HBITMAP,
        previous: HGDIOBJ,
    }
    impl Drop for Gdi {
        fn drop(&mut self) {
            unsafe {
                if !self.previous.is_invalid() {
                    SelectObject(self.memory, self.previous);
                }
                if !self.bitmap.is_invalid() {
                    let _ = DeleteObject(self.bitmap.into());
                }
                if !self.memory.is_invalid() {
                    let _ = DeleteDC(self.memory);
                }
                if !self.screen.is_invalid() {
                    ReleaseDC(None, self.screen);
                }
            }
        }
    }
    let mut g = Gdi {
        screen: unsafe { GetDC(None) },
        memory: HDC::default(),
        bitmap: HBITMAP::default(),
        previous: HGDIOBJ::default(),
    };
    if g.screen.is_invalid() {
        return Err("GetDC failed".into());
    }
    g.memory = unsafe { CreateCompatibleDC(Some(g.screen)) };
    g.bitmap = unsafe { CreateCompatibleBitmap(g.screen, w, h) };
    if g.memory.is_invalid() || g.bitmap.is_invalid() {
        return Err("CreateCompatibleBitmap failed".into());
    }
    g.previous = unsafe { SelectObject(g.memory, g.bitmap.into()) };
    unsafe { BitBlt(g.memory, 0, 0, w, h, Some(g.screen), bounds.left, bounds.top, SRCCOPY) }
        .map_err(|e| e.to_string())?;
    unsafe {
        SelectObject(g.memory, g.previous);
    }
    g.previous = HGDIOBJ::default();
    let mut info = BITMAPINFO::default();
    info.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB.0,
        ..Default::default()
    };
    let bytes = (w as usize).checked_mul(h as usize).and_then(|n| n.checked_mul(4)).ok_or("Screenshot too large")?;
    let mut pixels = vec![0u8; bytes];
    if unsafe {
        GetDIBits(g.screen, g.bitmap, 0, h as u32, Some(pixels.as_mut_ptr().cast()), &mut info, DIB_RGB_COLORS)
    } != h
    {
        return Err("GetDIBits failed".into());
    }
    for pixel in pixels.chunks_exact_mut(4) {
        pixel.swap(0, 2);
        pixel[3] = 255;
    }
    drop(g);
    std::fs::create_dir_all(&output_dir).map_err(|e| e.to_string())?;
    let t = unsafe { GetLocalTime() };
    let path = PathBuf::from(output_dir).join(format!(
        "Screenshot_{:04}-{:02}-{:02}_{:02}-{:02}-{:02}.png",
        t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
    ));
    image::save_buffer_with_format(
        &path,
        &pixels,
        w as u32,
        h as u32,
        image::ColorType::Rgba8,
        image::ImageFormat::Png,
    )
    .map_err(|e| e.to_string())?;
    Ok(path)
}
