//! Port of `Services/VfrCapture/OverlayCompositor.cs`: Direct2D/DirectWrite on a D3D11 texture.
pub mod gpu_usage;
pub mod keystroke;
pub mod stats;
pub mod webcam;

use crate::d3d::D3D11Context;
use keystroke::KeystrokeTracker;
use stats::SystemStatsSampler;
use std::mem::ManuallyDrop;
use webcam::WebcamCaptureSource;
use windows::core::{w, Error, Interface, Result};
use windows::Win32::Foundation::E_FAIL;
use windows::Win32::Graphics::Direct2D::Common::*;
use windows::Win32::Graphics::Direct2D::*;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::DirectWrite::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface};
use windows_numerics::{Matrix3x2, Vector2};

#[derive(Debug, Clone)]
pub struct OverlayOptions {
    pub webcam_enabled: bool,
    pub webcam_device_id: Option<String>,
    pub webcam_x: f64,
    pub webcam_y: f64,
    pub webcam_scale: f64,
    /// (width, height, fps) requested from the camera; zeros = Auto.
    pub webcam_format: (u32, u32, u32),
    pub keys_enabled: bool,
    pub keys_x: f64,
    pub keys_y: f64,
    pub keys_scale: f64,
    pub stats_enabled: bool,
    pub stats_x: f64,
    pub stats_y: f64,
    pub stats_scale: f64,
    pub stats_detail: i32,
}
impl Default for OverlayOptions {
    fn default() -> Self {
        Self {
            webcam_enabled: false,
            webcam_device_id: None,
            webcam_x: 0.97,
            webcam_y: 0.95,
            webcam_scale: 0.24,
            webcam_format: (0, 0, 0),
            keys_enabled: false,
            keys_x: 0.03,
            keys_y: 0.95,
            keys_scale: 0.20,
            stats_enabled: false,
            stats_x: 0.5,
            stats_y: 0.02,
            stats_scale: 0.045,
            stats_detail: 1,
        }
    }
}
impl OverlayOptions {
    pub fn any_enabled(&self) -> bool {
        self.webcam_enabled || self.keys_enabled || self.stats_enabled
    }
}
const KEY_ROWS: &[&[(&str, &str, f32)]] = &[
    &[("Esc", "Esc", 1.), ("1", "1", 1.), ("2", "2", 1.), ("3", "3", 1.), ("4", "4", 1.), ("5", "5", 1.)],
    &[("Tab", "Tab", 1.5), ("Q", "Q", 1.), ("W", "W", 1.), ("E", "E", 1.), ("R", "R", 1.), ("T", "T", 1.)],
    &[("Caps", "Caps", 1.75), ("A", "A", 1.), ("S", "S", 1.), ("D", "D", 1.), ("F", "F", 1.), ("G", "G", 1.)],
    &[("Shift", "Shift", 2.25), ("Z", "Z", 1.), ("X", "X", 1.), ("C", "C", 1.), ("V", "V", 1.), ("B", "B", 1.)],
    &[("Ctrl", "Ctrl", 1.25), ("Alt", "Alt", 1.25), ("Space", "", 3.5)],
];
struct Target {
    texture: ID3D11Texture2D,
    bitmap: ID2D1Bitmap1,
    width: u32,
    height: u32,
}
struct CameraBitmap {
    bitmap: ID2D1Bitmap1,
    width: u32,
    height: u32,
}
/// Capture-thread-only compositor. None means use the source unchanged (including error fallback).
/// Owns COM references to the shared D3D device; callers must serialize immediate-context use.
pub struct OverlayCompositor {
    dc: ID2D1DeviceContext,
    factory: ID2D1Factory1,
    dwrite: IDWriteFactory,
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    opts: OverlayOptions,
    webcam: Option<WebcamCaptureSource>,
    camera: Option<CameraBitmap>,
    cam_buffer: Vec<u8>,
    cam_seq: u64,
    cam_has_frame: bool,
    keys: Option<KeystrokeTracker>,
    stats: Option<SystemStatsSampler>,
    target: Option<Target>,
    idle: ID2D1SolidColorBrush,
    active: ID2D1SolidColorBrush,
    text: ID2D1SolidColorBrush,
    active_text: ID2D1SolidColorBrush,
    outline: ID2D1SolidColorBrush,
    border: ID2D1SolidColorBrush,
    key_format: Option<IDWriteTextFormat>,
    key_font: f32,
    stats_layout: Option<IDWriteTextLayout>,
    stats_text: String,
    stats_font: f32,
    error_logged: bool,
}
impl OverlayCompositor {
    pub fn new(d3d: &D3D11Context, opts: OverlayOptions) -> Result<Self> {
        let factory: ID2D1Factory1 = unsafe { D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)? };
        let dxgi: IDXGIDevice = d3d.device.cast()?;
        let d2d = unsafe { factory.CreateDevice(&dxgi)? };
        let dc = unsafe { d2d.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)? };
        let dwrite: IDWriteFactory = unsafe { DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)? };
        let brush = |r, g, b, a| unsafe { dc.CreateSolidColorBrush(&D2D1_COLOR_F { r, g, b, a }, None) };
        let idle = brush(0.06, 0.065, 0.085, 0.62)?;
        let active = brush(0.133, 0.827, 0.933, 1.)?;
        let text = brush(1., 1., 1., 0.85)?;
        let active_text = brush(0.04, 0.05, 0.06, 1.)?;
        let outline = brush(1., 1., 1., 0.28)?;
        let border = brush(1., 1., 1., 0.35)?;
        let webcam = if opts.webcam_enabled {
            let started = WebcamCaptureSource::new().and_then(|mut c| {
                c.start(opts.webcam_device_id.as_deref(), opts.webcam_format)?;
                Ok(c)
            });
            match started {
                Ok(c) => Some(c),
                Err(e) => {
                    log::info!("[Overlay] webcam unavailable ({}) — recording without it.", e.message());
                    None
                }
            }
        } else {
            None
        };
        let stats = if opts.stats_enabled { Some(SystemStatsSampler::new()?) } else { None };
        let keys = if opts.keys_enabled { Some(KeystrokeTracker::new()?) } else { None };
        log::info!(
            "[Overlay] compositor ready (webcam={} @{},{}×{}, keys={} @{},{}×{}).",
            cs_bool(webcam.is_some()),
            decimal(opts.webcam_x),
            decimal(opts.webcam_y),
            decimal(opts.webcam_scale),
            cs_bool(opts.keys_enabled),
            decimal(opts.keys_x),
            decimal(opts.keys_y),
            decimal(opts.keys_scale)
        );
        Ok(Self {
            dc,
            factory,
            dwrite,
            device: d3d.device.clone(),
            context: d3d.context.clone(),
            opts,
            webcam,
            camera: None,
            cam_buffer: Vec::new(),
            cam_seq: 0,
            cam_has_frame: false,
            keys,
            stats,
            target: None,
            idle,
            active,
            text,
            active_text,
            outline,
            border,
            key_format: None,
            key_font: 0.,
            stats_layout: None,
            stats_text: String::new(),
            stats_font: 0.,
            error_logged: false,
        })
    }
    pub fn compose(&mut self, bgra: &ID3D11Texture2D) -> Result<Option<ID3D11Texture2D>> {
        match self.compose_inner(bgra) {
            Ok(value) => Ok(value),
            Err(e) => {
                if !self.error_logged {
                    self.error_logged = true;
                    log::info!("[Overlay] compose failed, passing frames through: {}", e.message());
                }
                Ok(None)
            }
        }
    }
    fn compose_inner(&mut self, source: &ID3D11Texture2D) -> Result<Option<ID3D11Texture2D>> {
        self.poll_webcam()?;
        if let Some(stats) = &self.stats {
            stats.count_frame();
        }
        if !self.cam_has_frame && !self.opts.keys_enabled && self.stats.is_none() {
            return Ok(None);
        }
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            source.GetDesc(&mut desc);
        }
        self.ensure_target(desc.Width, desc.Height)?;
        let target = self.target.as_ref().ok_or_else(|| Error::new(E_FAIL, "overlay target unavailable"))?;
        let texture = target.texture.clone();
        unsafe {
            self.context.CopyResource(&texture, source);
            self.dc.SetTarget(&target.bitmap);
            self.dc.BeginDraw();
        }
        let draw = (|| -> Result<()> {
            if self.cam_has_frame {
                self.draw_webcam(desc.Width as f32, desc.Height as f32)?;
            }
            if self.opts.keys_enabled {
                self.draw_input(desc.Width as f32, desc.Height as f32)?;
            }
            if self.stats.is_some() {
                self.draw_stats(desc.Width as f32, desc.Height as f32)?;
            }
            Ok(())
        })();
        // Always finish the batch and release its target, even if layout/upload failed.
        let end = unsafe { self.dc.EndDraw(None, None) };
        unsafe {
            self.dc.SetTarget(None);
        }
        draw?;
        end?;
        Ok(Some(texture))
    }
    fn ensure_target(&mut self, width: u32, height: u32) -> Result<()> {
        if self.target.as_ref().is_some_and(|t| t.width == width && t.height == height) {
            return Ok(());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32,
            ..Default::default()
        };
        let mut texture = None;
        unsafe {
            self.device.CreateTexture2D(&desc, None, Some(&mut texture))?;
        }
        let texture = texture.ok_or_else(|| Error::new(E_FAIL, "CreateTexture2D returned no texture"))?;
        let surface: IDXGISurface = texture.cast()?;
        let props = bitmap_properties(D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW);
        let bitmap = unsafe { self.dc.CreateBitmapFromDxgiSurface(&surface, Some(&props))? };
        self.target = Some(Target { texture, bitmap, width, height });
        Ok(())
    }
    fn poll_webcam(&mut self) -> Result<()> {
        let Some(cam) = &self.webcam else {
            return Ok(());
        };
        let Some((width, height)) = cam.try_copy_latest(&mut self.cam_buffer, &mut self.cam_seq)? else {
            return Ok(());
        };
        if !self.camera.as_ref().is_some_and(|b| b.width == width && b.height == height) {
            let bitmap = unsafe {
                self.dc.CreateBitmap(
                    D2D_SIZE_U { width, height },
                    None,
                    0,
                    &bitmap_properties(D2D1_BITMAP_OPTIONS_NONE),
                )?
            };
            self.camera = Some(CameraBitmap { bitmap, width, height });
        }
        if let Some(cam) = &self.camera {
            unsafe {
                cam.bitmap.CopyFromMemory(None, self.cam_buffer.as_ptr().cast(), width * 4)?;
            }
        }
        self.cam_has_frame = true;
        Ok(())
    }
    fn push_clip(&self, rounded: &D2D1_ROUNDED_RECT) -> Result<()> {
        let geometry = unsafe { self.factory.CreateRoundedRectangleGeometry(rounded)? };
        let mut parameters = D2D1_LAYER_PARAMETERS1 {
            contentBounds: rounded.rect,
            geometricMask: ManuallyDrop::new(Some(geometry.cast()?)),
            maskAntialiasMode: D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            maskTransform: Matrix3x2::identity(),
            opacity: 1.,
            opacityBrush: ManuallyDrop::new(None),
            layerOptions: D2D1_LAYER_OPTIONS1_NONE,
        };
        unsafe {
            self.dc.PushLayer(&parameters, None);
            // The layer retains the geometry. windows-rs native parameter structs use ManuallyDrop.
            ManuallyDrop::drop(&mut parameters.geometricMask);
        }
        Ok(())
    }
    fn draw_webcam(&self, fw: f32, fh: f32) -> Result<()> {
        let Some(cam) = &self.camera else {
            return Ok(());
        };
        let unit = fh / 1080.;
        let bh = (self.opts.webcam_scale * fh as f64) as f32;
        let bw = bh * cam.width as f32 / cam.height.max(1) as f32;
        let x = (self.opts.webcam_x * (fw - bw).max(0.) as f64) as f32;
        let y = (self.opts.webcam_y * (fh - bh).max(0.) as f64) as f32;
        let rr = rounded(x, y, bw, bh, 14. * unit);
        self.push_clip(&rr)?;
        unsafe {
            // Use the render-target overload, matching SharpDX's BitmapInterpolationMode.Linear.
            let rt: &ID2D1RenderTarget = &self.dc;
            rt.DrawBitmap(&cam.bitmap, Some(&rr.rect), 1., D2D1_BITMAP_INTERPOLATION_MODE_LINEAR, None);
            self.dc.PopLayer();
            self.dc.DrawRoundedRectangle(&rr, &self.border, 2. * unit, None);
        }
        Ok(())
    }
    fn format(&self, size: f32) -> Result<IDWriteTextFormat> {
        unsafe {
            self.dwrite.CreateTextFormat(
                w!("Segoe UI"),
                None,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                DWRITE_FONT_STYLE_NORMAL,
                DWRITE_FONT_STRETCH_NORMAL,
                size,
                w!(""),
            )
        }
    }
    fn draw_input(&mut self, fw: f32, fh: f32) -> Result<()> {
        let pressed = self.keys.as_mut().map(|k| k.snapshot()).unwrap_or_default();
        let opacity = |key: &str| pressed.iter().find(|(s, _)| s == key).map(|(_, v)| *v).unwrap_or(0.);
        let unit = fh / 1080.;
        let ku = (self.opts.keys_scale * fh as f64) as f32 / 5.47;
        let gap = ku / 8.5;
        let font = (ku * 0.38).max(6.);
        if self.key_format.is_none() || (self.key_font - font).abs() > 0.5 {
            let f = self.format(font)?;
            unsafe {
                f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
            }
            self.key_format = Some(f);
            self.key_font = font;
        }
        let kbw = KEY_ROWS.iter().map(|r| r.iter().map(|k| k.2 * ku + gap).sum::<f32>() - gap).fold(0., f32::max);
        let kbh = 5. * (ku + gap) - gap;
        let (mw, mh, mg) = (1.82 * ku, 2.82 * ku, 0.41 * ku);
        let bw = kbw + mg + mw;
        let bh = kbh.max(mh);
        let bx = (self.opts.keys_x * (fw - bw).max(0.) as f64) as f32;
        let by = (self.opts.keys_y * (fh - bh).max(0.) as f64) as f32;
        let mut y = by + (bh - kbh) / 2.;
        for row in KEY_ROWS {
            let mut x = bx;
            for &(id, label, w) in *row {
                let rr = rounded(x, y, w * ku, ku, ku * 0.15);
                let act = opacity(id);
                unsafe {
                    self.dc.FillRoundedRectangle(&rr, &self.idle);
                    if act > 0. {
                        self.active.SetOpacity(act);
                        self.dc.FillRoundedRectangle(&rr, &self.active);
                    }
                    self.dc.DrawRoundedRectangle(&rr, &self.outline, unit, None);
                    if !label.is_empty() {
                        let text: Vec<u16> = label.encode_utf16().collect();
                        if let Some(format) = &self.key_format {
                            self.dc.DrawText(
                                &text,
                                format,
                                &rr.rect,
                                if act > 0.5 { &self.active_text } else { &self.text },
                                D2D1_DRAW_TEXT_OPTIONS_NONE,
                                DWRITE_MEASURING_MODE_NATURAL,
                            );
                        }
                    }
                }
                x += w * ku + gap;
            }
            y += ku + gap;
        }
        let mx = bx + kbw + mg;
        let my = by + (bh - mh) / 2.;
        let body = rounded(mx, my, mw, mh, mw * 0.42);
        let split = my + mh * 0.42;
        self.push_clip(&body)?;
        unsafe {
            self.dc.FillRoundedRectangle(&body, &self.idle);
            for (id, left, right) in [("LMB", mx, mx + mw / 2.), ("RMB", mx + mw / 2., mx + mw)] {
                let act = opacity(id);
                if act > 0. {
                    self.active.SetOpacity(act);
                    self.dc.FillRectangle(&rect(left, my, right, split), &self.active);
                }
            }
            self.dc.DrawLine(
                Vector2 { X: mx + mw / 2., Y: my },
                Vector2 { X: mx + mw / 2., Y: split },
                &self.outline,
                unit,
                None,
            );
            self.dc.DrawLine(Vector2 { X: mx, Y: split }, Vector2 { X: mx + mw, Y: split }, &self.outline, unit, None);
            self.dc.PopLayer();
            let ww = 9. * unit;
            let wheel = rounded(mx + mw / 2. - ww / 2., my + 12. * unit, ww, 24. * unit, ww / 2.);
            self.dc.FillRoundedRectangle(&wheel, &self.idle);
            let act = opacity("MMB");
            if act > 0. {
                self.active.SetOpacity(act);
                self.dc.FillRoundedRectangle(&wheel, &self.active);
            }
            self.dc.DrawRoundedRectangle(&wheel, &self.outline, unit, None);
            self.dc.DrawRoundedRectangle(&body, &self.outline, 1.5 * unit, None);
            self.active.SetOpacity(1.);
        }
        Ok(())
    }
    fn draw_stats(&mut self, fw: f32, fh: f32) -> Result<()> {
        let Some(s) = self.stats.as_ref().map(|s| s.current()) else {
            return Ok(());
        };
        let text = stats_text(s, self.opts.stats_detail);
        let ph = (self.opts.stats_scale * fh as f64) as f32;
        let font = (ph * 0.52).max(6.);
        if self.stats_layout.is_none() || self.stats_text != text || (self.stats_font - font).abs() > 0.5 {
            let f = self.format(font)?;
            let wide: Vec<u16> = text.encode_utf16().collect();
            self.stats_layout = Some(unsafe { self.dwrite.CreateTextLayout(&wide, &f, fw, ph)? });
            self.stats_text = text;
            self.stats_font = font;
        }
        if let Some(layout) = &self.stats_layout {
            let mut metrics = DWRITE_TEXT_METRICS::default();
            unsafe {
                layout.GetMetrics(&mut metrics)?;
            }
            let pad = ph * 0.45;
            let pw = metrics.width + pad * 2.;
            let x = (self.opts.stats_x * (fw - pw).max(0.) as f64) as f32;
            let y = (self.opts.stats_y * (fh - ph).max(0.) as f64) as f32;
            let rr = rounded(x, y, pw, ph, ph * 0.22);
            unsafe {
                self.dc.FillRoundedRectangle(&rr, &self.idle);
                self.dc.DrawRoundedRectangle(&rr, &self.outline, 1., None);
                self.dc.DrawTextLayout(
                    Vector2 { X: x + pad, Y: y + (ph - metrics.height) / 2. },
                    layout,
                    &self.text,
                    D2D1_DRAW_TEXT_OPTIONS_NONE,
                );
            }
        }
        Ok(())
    }
}
impl Drop for OverlayCompositor {
    fn drop(&mut self) {
        // Stop/join input and stats workers before releasing any drawing state.
        self.keys.take();
        self.stats.take();
        self.webcam.take();
        unsafe {
            self.dc.SetTarget(None);
        }
    }
}
fn bitmap_properties(options: D2D1_BITMAP_OPTIONS) -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT { format: DXGI_FORMAT_B8G8R8A8_UNORM, alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED },
        dpiX: 96.,
        dpiY: 96.,
        bitmapOptions: options,
        ..Default::default()
    }
}
fn rect(left: f32, top: f32, right: f32, bottom: f32) -> D2D_RECT_F {
    D2D_RECT_F { left, top, right, bottom }
}
fn rounded(x: f32, y: f32, w: f32, h: f32, r: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT { rect: rect(x, y, x + w, y + h), radiusX: r, radiusY: r }
}
fn cs_bool(v: bool) -> &'static str {
    if v {
        "True"
    } else {
        "False"
    }
}
fn decimal(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}
fn stats_text(s: stats::Snapshot, detail: i32) -> String {
    match detail {
        0 => format!("{} FPS", s.fps),
        1 => format!("{} FPS   CPU {}%   GPU {}%", s.fps, s.cpu_percent, s.gpu_percent),
        _ => format!(
            "{} FPS   CPU {}%   GPU {}%   RAM {:.1}/{:.1} GB",
            s.fps, s.cpu_percent, s.gpu_percent, s.ram_used_gb, s.ram_total_gb
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn texture(d3d: &D3D11Context, width: u32, height: u32) -> ID3D11Texture2D {
        let pixels: Vec<u8> = (0..width * height).flat_map(|_| [80, 100, 120, 255]).collect();
        let data =
            D3D11_SUBRESOURCE_DATA { pSysMem: pixels.as_ptr().cast(), SysMemPitch: width * 4, ..Default::default() };
        let desc = D3D11_TEXTURE2D_DESC {
            Width: width,
            Height: height,
            MipLevels: 1,
            ArraySize: 1,
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32,
            ..Default::default()
        };
        let mut texture = None;
        unsafe {
            d3d.device.CreateTexture2D(&desc, Some(&data), Some(&mut texture)).unwrap();
        }
        texture.unwrap()
    }
    fn read_pixels(d3d: &D3D11Context, texture: &ID3D11Texture2D) -> Vec<u8> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            texture.GetDesc(&mut desc);
        }
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        let mut staging = None;
        unsafe {
            d3d.device.CreateTexture2D(&desc, None, Some(&mut staging)).unwrap();
        }
        let staging = staging.unwrap();
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            d3d.context.CopyResource(&staging, texture);
            d3d.context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped)).unwrap();
            let mut pixels = vec![0u8; desc.Width as usize * desc.Height as usize * 4];
            for y in 0..desc.Height as usize {
                let row = std::slice::from_raw_parts(
                    (mapped.pData as *const u8).add(y * mapped.RowPitch as usize),
                    desc.Width as usize * 4,
                );
                pixels[y * desc.Width as usize * 4..(y + 1) * desc.Width as usize * 4].copy_from_slice(row);
            }
            d3d.context.Unmap(&staging, 0);
            pixels
        }
    }
    fn pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> &[u8] {
        &pixels[(y * width + x) * 4..(y * width + x) * 4 + 4]
    }
    #[test]
    fn offscreen_keys_stats_pixels_and_passthrough() {
        let d3d = D3D11Context::new(-1).unwrap();
        let source = texture(&d3d, 1920, 1080);
        let mut plain = OverlayCompositor::new(&d3d, OverlayOptions::default()).unwrap();
        assert!(plain.compose(&source).unwrap().is_none());
        drop(plain);
        let opts = OverlayOptions { keys_enabled: true, stats_enabled: true, ..Default::default() };
        assert!(opts.any_enabled());
        let mut overlay = OverlayCompositor::new(&d3d, opts).unwrap();
        let result = overlay.compose(&source).unwrap().expect("composed offscreen target");
        let pixels = read_pixels(&d3d, &result);
        let unchanged = &[80, 100, 120, 255];
        // At defaults: first key starts near (45,821); mouse body near (373,874).
        assert_ne!(pixel(&pixels, 1920, 55, 831), unchanged, "keyboard idle fill");
        assert_ne!(pixel(&pixels, 1920, 405, 950), unchanged, "mouse idle fill");
        assert_ne!(pixel(&pixels, 1920, 960, 25), unchanged, "stats panel fill");
        assert_eq!(pixel(&pixels, 1920, 1000, 500), unchanged, "source outside overlays");
        let original = read_pixels(&d3d, &source);
        assert_eq!(pixel(&original, 1920, 55, 831), unchanged, "input texture remains unchanged");
        // Reusing then resizing the render target must recreate its DXGI bitmap safely.
        assert!(overlay.compose(&source).unwrap().is_some());
        let smaller = texture(&d3d, 1280, 720);
        assert!(overlay.compose(&smaller).unwrap().is_some());
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            result.GetDesc(&mut desc);
        }
        assert_eq!((desc.Width, desc.Height), (1920, 1080));
    }
    #[test]
    fn failed_draw_passes_through_then_recovers() {
        let d3d = D3D11Context::new(-1).unwrap();
        let source = texture(&d3d, 1920, 1080);
        let mut overlay = OverlayCompositor::new(
            &d3d,
            OverlayOptions {
                stats_enabled: true,
                // DirectWrite rejects the negative layout height after BeginDraw.
                stats_scale: -0.045,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(overlay.compose(&source).unwrap().is_none());
        assert!(overlay.error_logged);
        overlay.opts.stats_scale = 0.045;
        assert!(overlay.compose(&source).unwrap().is_some(), "EndDraw and target cleanup permit retry");
    }
    #[test]
    fn synthetic_webcam_is_clipped_and_keeps_aspect_without_a_stream() {
        let d3d = D3D11Context::new(-1).unwrap();
        let source = texture(&d3d, 1920, 1080);
        // Supply a CPU fixture directly to D2D. No MediaCapture/reader is created.
        let mut overlay = OverlayCompositor::new(&d3d, OverlayOptions::default()).unwrap();
        let fixture: Vec<u8> = (0..640 * 480).flat_map(|_| [10, 20, 230, 255]).collect();
        let bitmap = unsafe {
            overlay
                .dc
                .CreateBitmap(
                    D2D_SIZE_U { width: 640, height: 480 },
                    Some(fixture.as_ptr().cast()),
                    640 * 4,
                    &bitmap_properties(D2D1_BITMAP_OPTIONS_NONE),
                )
                .unwrap()
        };
        overlay.camera = Some(CameraBitmap { bitmap, width: 640, height: 480 });
        overlay.cam_has_frame = true;
        let output = overlay.compose(&source).unwrap().unwrap();
        let pixels = read_pixels(&d3d, &output);
        // boxH=259.2, boxW=345.6, x=1527.168, y=779.76 at the C# defaults.
        assert_eq!(pixel(&pixels, 1920, 1700, 900), &[10, 20, 230, 255]);
        assert_eq!(pixel(&pixels, 1920, 1528, 781), &[80, 100, 120, 255], "rounded corner is clipped");
        assert_eq!(pixel(&pixels, 1920, 1878, 900), &[80, 100, 120, 255], "right of camera aspect bounds");
        assert_eq!(pixel(&pixels, 1920, 1700, 1045), &[80, 100, 120, 255], "below camera height");
    }
    #[test]
    fn stat_strings_and_options_match_csharp() {
        let s = stats::Snapshot { fps: 60, cpu_percent: 27, gpu_percent: 91, ram_used_gb: 12.25, ram_total_gb: 32. };
        assert_eq!(stats_text(s, 0), "60 FPS");
        assert_eq!(stats_text(s, 1), "60 FPS   CPU 27%   GPU 91%");
        assert_eq!(stats_text(s, 2), "60 FPS   CPU 27%   GPU 91%   RAM 12.2/32.0 GB");
        assert!(!OverlayOptions::default().any_enabled());
        assert_eq!(decimal(0.2), "0.2");
        assert_eq!(KEY_ROWS.len(), 5);
        assert_eq!(KEY_ROWS[4][2], ("Space", "", 3.5));
    }
}
