//! Port of `WgcCaptureSource.cs`: Windows Graphics Capture of a window (HWND) or a monitor (HMONITOR)
//! onto the shared D3D11 device.
//!
//! Same knobs as C# (each one measured): free-threaded frame pool of 3 B8G8R8A8 buffers; borderless
//! access requested, then `IsBorderRequired = false`; `MinUpdateInterval = 1/(2*maxFps)` on Windows 11
//! 24H2+ (switches WGC from vsync-throttled ~60 fps to present-driven delivery); cursor capture as asked.
//!
//! Fixes vs C#:
//! * the pool is recreated with 3 buffers after a resolution change (C# dropped to 2, losing the slack
//!   that keeps WGC from skipping presents);
//! * the activation-factory class-name HSTRING is freed (C# leaked one per capture start);
//! * the frame handler never logs on the WGC thread except on error (C# rule: logging there cost fps).

use crate::d3d::D3D11Context;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use windows::core::{factory, IInspectable, Interface, Result};
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Graphics::Capture::{
    Direct3D11CaptureFramePool, GraphicsCaptureAccess, GraphicsCaptureAccessKind, GraphicsCaptureItem,
    GraphicsCaptureSession,
};
use windows::Graphics::DirectX::Direct3D11::IDirect3DDevice;
use windows::Graphics::DirectX::DirectXPixelFormat;
use windows::Graphics::SizeInt32;
use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Direct3D11::ID3D11Texture2D;
use windows::Win32::Graphics::Dxgi::IDXGIDevice;
use windows::Win32::Graphics::Gdi::HMONITOR;
use windows::Win32::System::WinRT::Direct3D11::{CreateDirect3D11DeviceFromDXGIDevice, IDirect3DDxgiInterfaceAccess};
use windows::Win32::System::WinRT::Graphics::Capture::IGraphicsCaptureItemInterop;

const POOL_BUFFERS: i32 = 3;

/// What to capture.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureTarget {
    Window(isize),
    Monitor(isize),
}

/// Frame callback: the captured texture (valid only during the call) and WGC's SystemRelativeTime
/// (100 ns QPC ticks). Runs on the WGC free-threaded callback thread.
pub type FrameCallback = Box<dyn FnMut(&ID3D11Texture2D, i64) + Send>;

pub struct WgcCapture {
    target: CaptureTarget,
    pool: Option<Direct3D11CaptureFramePool>,
    session: Option<GraphicsCaptureSession>,
    item: Option<GraphicsCaptureItem>,
    frame_token: Option<i64>,
    closed_token: Option<i64>,
    disposed: Arc<AtomicBool>,
    size: Arc<Mutex<SizeInt32>>,
}

/// The WinRT wrapper of our D3D11 device, moved into the free-threaded FrameArrived handler.
struct AgileDevice(IDirect3DDevice);
// SAFETY: the object returned by CreateDirect3D11DeviceFromDXGIDevice is agile (free-threaded marshaler),
// and the underlying D3D11 device is multithread-protected (see D3D11Context).
unsafe impl Send for AgileDevice {}
unsafe impl Sync for AgileDevice {}
impl AgileDevice {
    // A method (not `.0`) so closures capture the whole Send wrapper, not the inner field.
    fn get(&self) -> &IDirect3DDevice {
        &self.0
    }
}

fn winrt_device(d3d: &D3D11Context) -> Result<IDirect3DDevice> {
    let dxgi: IDXGIDevice = d3d.device.cast()?;
    unsafe { CreateDirect3D11DeviceFromDXGIDevice(&dxgi)? }.cast()
}

fn create_item(target: CaptureTarget) -> Result<GraphicsCaptureItem> {
    // windows-rs owns (and frees) the class-name HSTRING inside `factory`.
    let interop = factory::<GraphicsCaptureItem, IGraphicsCaptureItemInterop>()?;
    unsafe {
        match target {
            CaptureTarget::Window(h) => interop.CreateForWindow(HWND(h as *mut _)),
            CaptureTarget::Monitor(h) => interop.CreateForMonitor(HMONITOR(h as *mut _)),
        }
    }
}

fn texture_of(frame: &windows::Graphics::Capture::Direct3D11CaptureFrame) -> Result<ID3D11Texture2D> {
    let access: IDirect3DDxgiInterfaceAccess = frame.Surface()?.cast()?;
    unsafe { access.GetInterface::<ID3D11Texture2D>() }
}

impl WgcCapture {
    pub fn is_supported() -> bool {
        GraphicsCaptureSession::IsSupported().unwrap_or(false)
    }

    pub fn new(target: CaptureTarget) -> Self {
        Self {
            target,
            pool: None,
            session: None,
            item: None,
            frame_token: None,
            closed_token: None,
            disposed: Arc::new(AtomicBool::new(false)),
            size: Arc::new(Mutex::new(SizeInt32::default())),
        }
    }

    pub fn width(&self) -> i32 {
        self.size.lock().map(|s| s.Width).unwrap_or(0)
    }
    pub fn height(&self) -> i32 {
        self.size.lock().map(|s| s.Height).unwrap_or(0)
    }

    /// Starts capture. `on_frame` gets every frame; `on_closed` fires when the target window/monitor goes away.
    pub fn start(
        &mut self,
        d3d: &D3D11Context,
        capture_cursor: bool,
        max_fps: i32,
        on_frame: FrameCallback,
        on_closed: Box<dyn Fn() + Send + Sync>,
    ) -> Result<()> {
        let device = winrt_device(d3d)?;
        let item = create_item(self.target)?;
        let size0 = item.Size()?;
        *self.size.lock().unwrap_or_else(|e| e.into_inner()) = size0;

        let disposed = self.disposed.clone();
        self.closed_token =
            Some(item.Closed(&TypedEventHandler::<GraphicsCaptureItem, IInspectable>::new(move |_, _| {
                if !disposed.load(Ordering::Acquire) {
                    on_closed();
                }
                Ok(())
            }))?);

        let pool = Direct3D11CaptureFramePool::CreateFreeThreaded(
            &device,
            DirectXPixelFormat::B8G8R8A8UIntNormalized,
            POOL_BUFFERS,
            size0,
        )?;

        let disposed = self.disposed.clone();
        let size = self.size.clone();
        let handler_device = AgileDevice(device.clone());
        // WinRT handlers must be `Fn`; WGC raises FrameArrived for a pool one at a time, so this lock is
        // never contended.
        let on_frame = Mutex::new(on_frame);
        self.frame_token = Some(pool.FrameArrived(
            &TypedEventHandler::<Direct3D11CaptureFramePool, IInspectable>::new(move |sender, _| {
                if disposed.load(Ordering::Acquire) {
                    return Ok(());
                }
                let Some(pool) = sender.as_ref() else { return Ok(()) };
                let frame = match pool.TryGetNextFrame() {
                    Ok(f) => f,
                    Err(_) => return Ok(()), // no frame ready
                };
                let result = (|| -> Result<()> {
                    // The target changed resolution: resize the pool so frames keep coming full-size.
                    let cs = frame.ContentSize()?;
                    let mut last = size.lock().unwrap_or_else(|e| e.into_inner());
                    if cs.Width != last.Width || cs.Height != last.Height {
                        *last = cs;
                        drop(last);
                        pool.Recreate(
                            handler_device.get(),
                            DirectXPixelFormat::B8G8R8A8UIntNormalized,
                            POOL_BUFFERS,
                            cs,
                        )?;
                    } else {
                        drop(last);
                    }
                    let tex = texture_of(&frame)?;
                    let ticks = frame.SystemRelativeTime()?.Duration;
                    (on_frame.lock().unwrap_or_else(|e| e.into_inner()))(&tex, ticks);
                    Ok(())
                })();
                if let Err(e) = result {
                    // A bad frame must not kill the capture loop.
                    log::info!("[WgcCaptureSource] frame error: {}", e.message());
                }
                let _ = frame.Close();
                Ok(())
            }),
        )?);

        let session = pool.CreateCaptureSession(&item)?;
        let _ = session.SetIsCursorCaptureEnabled(capture_cursor);
        // Borderless needs the access REQUEST first (Windows otherwise keeps drawing the yellow border).
        match GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless).and_then(|op| op.join())
        {
            Ok(_) => {}
            Err(e) => log::info!("[WgcCaptureSource] borderless access request unavailable: {}", e.message()),
        }
        if session.SetIsBorderRequired(false).is_ok() {
            log::info!("[WgcCaptureSource] disabled WinRT capture border.");
        } else {
            log::info!("[WgcCaptureSource] capture-border toggle not available on this OS build.");
        }
        if max_fps > 0 {
            let ms = 1000.0 / (max_fps as f64 * 2.0);
            if session.SetMinUpdateInterval(TimeSpan { Duration: (ms * 10_000.0) as i64 }).is_ok() {
                log::info!("[WgcCaptureSource] MinUpdateInterval set to {ms:.2}ms (floor {}fps).", max_fps * 2);
            }
        }
        session.StartCapture()?;
        let (kind, h) = match self.target {
            CaptureTarget::Window(h) => ("hwnd", h),
            CaptureTarget::Monitor(h) => ("monitor", h),
        };
        log::info!("[WgcCaptureSource] Capturing {kind} 0x{h:X} at {}x{}.", size0.Width, size0.Height);
        self.pool = Some(pool);
        self.session = Some(session);
        self.item = Some(item);
        Ok(())
    }

    pub fn stop(&mut self) {
        self.disposed.store(true, Ordering::Release);
        if let (Some(pool), Some(t)) = (&self.pool, self.frame_token.take()) {
            let _ = pool.RemoveFrameArrived(t);
        }
        if let (Some(item), Some(t)) = (&self.item, self.closed_token.take()) {
            let _ = item.RemoveClosed(t);
        }
        if let Some(s) = self.session.take() {
            let _ = s.Close();
        }
        if let Some(p) = self.pool.take() {
            let _ = p.Close();
        }
        self.item = None;
    }
}

impl Drop for WgcCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use windows::core::BOOL;
    use windows::Win32::Foundation::{LPARAM, RECT};
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, MONITORINFO};

    /// Captures a NON-primary monitor for 1 s (no window is opened; never touches the game display).
    /// Ignored by default: run explicitly with `cargo test -p lag-capture wgc -- --ignored`.
    #[test]
    #[ignore]
    fn captures_a_secondary_monitor() {
        unsafe extern "system" fn cb(m: HMONITOR, _: HDC, _: *mut RECT, out: LPARAM) -> BOOL {
            let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
            unsafe {
                let _ = GetMonitorInfoW(m, &mut info);
                if info.dwFlags & 1 == 0 {
                    *(out.0 as *mut isize) = m.0 as isize; // MONITORINFOF_PRIMARY not set
                }
            }
            true.into()
        }
        let mut secondary: isize = 0;
        unsafe {
            let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&mut secondary as *mut isize as isize));
        }
        if secondary == 0 {
            eprintln!("no secondary monitor; skipping");
            return;
        }
        let d3d = D3D11Context::new(-1).unwrap();
        let frames = Arc::new(AtomicUsize::new(0));
        let f2 = frames.clone();
        let mut cap = WgcCapture::new(CaptureTarget::Monitor(secondary));
        cap.start(
            &d3d,
            false,
            120,
            Box::new(move |_t, _ticks| {
                f2.fetch_add(1, Ordering::Relaxed);
            }),
            Box::new(|| {}),
        )
        .unwrap();
        std::thread::sleep(std::time::Duration::from_secs(1));
        cap.stop();
        let n = frames.load(Ordering::Relaxed);
        eprintln!("secondary monitor {}x{}: {n} frames in 1 s", cap.width(), cap.height());
        assert!(n >= 1, "WGC delivered no frames");
    }
}
