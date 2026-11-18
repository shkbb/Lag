//! Port of `Bgra2Nv12Converter.cs`: GPU colour conversion + scaling BGRA -> NV12 with the D3D11
//! VideoProcessor.
//!
//! Kept from C# (each was a measured fix):
//! * blit into our OWN single (non-array) NV12 scratch texture, then `CopySubresourceRegion` into the
//!   encoder's pool slice: blitting straight into a Texture2D *array slice* silently skipped ~half the
//!   frames on NVIDIA (solid green frames);
//! * no per-frame CPU fence (the encoder reads on the same immediate context);
//! * colour space: input full-range RGB (WGC always delivers DWM-composited full-range sRGB), output
//!   BT.709 LIMITED range (16-235), and the encoder is tagged to match.
//!
//! Optimisations vs C#:
//! * the VideoProcessorInputView on the fixed input-copy texture is created once per pipeline instead of
//!   once per frame (C# created and released a COM view for every captured frame);
//! * the CPU staging texture exists only when the system-memory (x264/cross-vendor) path asks for it.

use crate::d3d::D3D11Context;
use std::mem::ManuallyDrop;
use windows::core::{Interface, Result};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_FORMAT, DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL, DXGI_SAMPLE_DESC,
};

/// D3D11_VIDEO_PROCESSOR_COLOR_SPACE bit layout: Usage:1, RGB_Range:1, YCbCr_Matrix:1, YCbCr_xvYCC:1,
/// Nominal_Range:2.
const CS_INPUT_FULL_RANGE_RGB: u32 = 0; // RGB_Range = 0 -> 0-255
const CS_OUTPUT_BT709_LIMITED: u32 = (1 << 2) | (1 << 4); // YCbCr_Matrix = 1 (BT.709), Nominal_Range = 1 (16-235)

struct Pipeline {
    in_w: u32,
    in_h: u32,
    processor: ID3D11VideoProcessor,
    // Kept alive for the lifetime of the views created from it.
    _enumerator: ID3D11VideoProcessorEnumerator,
    input_copy: ID3D11Texture2D,
    input_view: ID3D11VideoProcessorInputView,
    scratch: ID3D11Texture2D,
    scratch_view: ID3D11VideoProcessorOutputView,
    staging: Option<ID3D11Texture2D>,
}

pub struct Bgra2Nv12Converter {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    out_w: u32,
    out_h: u32,
    pipe: Option<Pipeline>,
}

/// A mapped CPU view of the converted NV12 frame (system-memory path). Unmaps on drop.
pub struct MappedNv12<'a> {
    conv: &'a Bgra2Nv12Converter,
    pub data: *const u8,
    pub row_pitch: usize,
}

impl Drop for MappedNv12<'_> {
    fn drop(&mut self) {
        if let Some(st) = self.conv.pipe.as_ref().and_then(|p| p.staging.as_ref()) {
            unsafe { self.conv.context.Unmap(st, 0) };
        }
    }
}

fn tex_desc(w: u32, h: u32, format: DXGI_FORMAT, usage: D3D11_USAGE, bind: u32, cpu: u32) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: w,
        Height: h,
        MipLevels: 1,
        ArraySize: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Usage: usage,
        BindFlags: bind,
        CPUAccessFlags: cpu,
        MiscFlags: 0,
    }
}

/// Largest rect with the input's aspect ratio centred in the output (even coordinates for NV12). When the
/// aspects match within 1% the whole output is used (identical to C# for every same-aspect source).
fn fit_rect(in_w: u32, in_h: u32, out_w: u32, out_h: u32) -> RECT {
    let full = RECT { left: 0, top: 0, right: out_w as i32, bottom: out_h as i32 };
    if in_w == 0 || in_h == 0 {
        return full;
    }
    let (ia, oa) = (in_w as f64 / in_h as f64, out_w as f64 / out_h as f64);
    if (ia / oa - 1.0).abs() < 0.01 {
        return full;
    }
    let (w, h) = if ia > oa { (out_w as f64, out_w as f64 / ia) } else { (out_h as f64 * ia, out_h as f64) };
    let (w, h) = ((w.round() as i32) & !1, (h.round() as i32) & !1);
    let left = ((out_w as i32 - w) / 2) & !1;
    let top = ((out_h as i32 - h) / 2) & !1;
    RECT { left, top, right: left + w, bottom: top + h }
}

impl Bgra2Nv12Converter {
    pub fn new(d3d: &D3D11Context, output_width: u32, output_height: u32) -> Result<Self> {
        Ok(Self {
            device: d3d.device.clone(),
            context: d3d.context.clone(),
            video_device: d3d.device.cast()?,
            video_context: d3d.context.cast()?,
            out_w: output_width & !1, // NV12 needs even dimensions
            out_h: output_height & !1,
            pipe: None,
        })
    }

    pub fn out_width(&self) -> u32 {
        self.out_w
    }
    pub fn out_height(&self) -> u32 {
        self.out_h
    }

    fn create_texture(&self, desc: &D3D11_TEXTURE2D_DESC) -> Result<ID3D11Texture2D> {
        let mut t = None;
        unsafe { self.device.CreateTexture2D(desc, None, Some(&mut t))? };
        Ok(t.expect("CreateTexture2D returned no texture"))
    }

    fn ensure_pipeline(&mut self, in_w: u32, in_h: u32) -> Result<()> {
        if matches!(&self.pipe, Some(p) if p.in_w == in_w && p.in_h == in_h) {
            return Ok(());
        }
        self.pipe = None;
        let (out_w, out_h) = (self.out_w, self.out_h);
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL { Numerator: 120, Denominator: 1 },
            InputWidth: in_w,
            InputHeight: in_h,
            OutputFrameRate: DXGI_RATIONAL { Numerator: 120, Denominator: 1 },
            OutputWidth: out_w,
            OutputHeight: out_h,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        unsafe {
            let enumerator = self.video_device.CreateVideoProcessorEnumerator(&content)?;
            let processor = self.video_device.CreateVideoProcessor(&enumerator, 0)?;
            let vc = &self.video_context;
            let src = RECT { left: 0, top: 0, right: in_w as i32, bottom: in_h as i32 };
            let target = RECT { left: 0, top: 0, right: out_w as i32, bottom: out_h as i32 };
            // A source whose aspect differs from the session's output (e.g. the desktop after a windowed
            // game, see engine source switching) is fitted with black bars instead of being stretched.
            let dst = fit_rect(in_w, in_h, out_w, out_h);
            vc.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&src));
            vc.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&dst));
            vc.VideoProcessorSetOutputTargetRect(&processor, true, Some(&target));
            // Bars = video black (Y=16, Cb=Cr=128 in limited range).
            vc.VideoProcessorSetOutputBackgroundColor(
                &processor,
                true,
                &D3D11_VIDEO_COLOR {
                    Anonymous: D3D11_VIDEO_COLOR_0 {
                        YCbCr: D3D11_VIDEO_COLOR_YCbCrA { Y: 16.0 / 255.0, Cb: 0.5, Cr: 0.5, A: 1.0 },
                    },
                },
            );
            vc.VideoProcessorSetStreamColorSpace(
                &processor,
                0,
                &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: CS_INPUT_FULL_RANGE_RGB },
            );
            vc.VideoProcessorSetOutputColorSpace(
                &processor,
                &D3D11_VIDEO_PROCESSOR_COLOR_SPACE { _bitfield: CS_OUTPUT_BT709_LIMITED },
            );

            let input_copy = self.create_texture(&tex_desc(
                in_w,
                in_h,
                DXGI_FORMAT_B8G8R8A8_UNORM,
                D3D11_USAGE_DEFAULT,
                (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                0,
            ))?;
            let ivd = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                FourCC: 0,
                ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                    Texture2D: D3D11_TEX2D_VPIV { MipSlice: 0, ArraySlice: 0 },
                },
            };
            let mut input_view = None;
            self.video_device.CreateVideoProcessorInputView(&input_copy, &enumerator, &ivd, Some(&mut input_view))?;

            let scratch = self.create_texture(&tex_desc(
                out_w,
                out_h,
                DXGI_FORMAT_NV12,
                D3D11_USAGE_DEFAULT,
                D3D11_BIND_RENDER_TARGET.0 as u32,
                0,
            ))?;
            let ovd = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
                ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
                Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 { Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 } },
            };
            let mut scratch_view = None;
            self.video_device.CreateVideoProcessorOutputView(&scratch, &enumerator, &ovd, Some(&mut scratch_view))?;

            self.pipe = Some(Pipeline {
                in_w,
                in_h,
                processor,
                _enumerator: enumerator,
                input_copy,
                input_view: input_view.expect("input view"),
                scratch,
                scratch_view: scratch_view.expect("output view"),
                staging: None,
            });
        }
        log::info!("[Bgra2Nv12] Pipeline {in_w}x{in_h} \u{2192} {out_w}x{out_h} NV12.");
        Ok(())
    }

    fn blit_to_scratch(&mut self, src_bgra: &ID3D11Texture2D) -> Result<()> {
        let mut d = D3D11_TEXTURE2D_DESC::default();
        unsafe { src_bgra.GetDesc(&mut d) };
        self.ensure_pipeline(d.Width, d.Height)?;
        let p = self.pipe.as_ref().expect("pipeline");
        unsafe {
            // Copy out of the WGC pool so the capture frame can be released immediately.
            self.context.CopyResource(&p.input_copy, src_bgra);
            let stream = D3D11_VIDEO_PROCESSOR_STREAM {
                Enable: true.into(),
                OutputIndex: 0,
                InputFrameOrField: 0,
                PastFrames: 0,
                FutureFrames: 0,
                ppPastSurfaces: std::ptr::null_mut(),
                pInputSurface: ManuallyDrop::new(Some(p.input_view.clone())),
                ppFutureSurfaces: std::ptr::null_mut(),
                ppPastSurfacesRight: std::ptr::null_mut(),
                pInputSurfaceRight: ManuallyDrop::new(None),
                ppFutureSurfacesRight: std::ptr::null_mut(),
            };
            let mut streams = [stream];
            let r = self.video_context.VideoProcessorBlt(&p.processor, &p.scratch_view, 0, &streams);
            // Release the view reference we placed in the ManuallyDrop field.
            ManuallyDrop::drop(&mut streams[0].pInputSurface);
            r
        }
    }

    /// Hardware path: convert `src_bgra` into slice `slice` of the encoder's NV12 texture array.
    pub fn convert_into(
        &mut self,
        src_bgra: &ID3D11Texture2D,
        dst_nv12_array: &ID3D11Texture2D,
        slice: u32,
    ) -> Result<()> {
        self.blit_to_scratch(src_bgra)?;
        let p = self.pipe.as_ref().expect("pipeline");
        unsafe { self.context.CopySubresourceRegion(dst_nv12_array, slice, 0, 0, 0, &p.scratch, 0, None) };
        Ok(())
    }

    /// System-memory path: convert and map the NV12 result for CPU reading (Y plane, then UV plane at
    /// `row_pitch * height`). The mapping lives until the returned guard drops.
    pub fn map_system_nv12(&mut self, src_bgra: &ID3D11Texture2D) -> Result<MappedNv12<'_>> {
        self.blit_to_scratch(src_bgra)?;
        let (out_w, out_h) = (self.out_w, self.out_h);
        if self.pipe.as_ref().is_some_and(|p| p.staging.is_none()) {
            let st = self.create_texture(&tex_desc(
                out_w,
                out_h,
                DXGI_FORMAT_NV12,
                D3D11_USAGE_STAGING,
                0,
                D3D11_CPU_ACCESS_READ.0 as u32,
            ))?;
            self.pipe.as_mut().expect("pipeline").staging = Some(st);
        }
        let p = self.pipe.as_ref().expect("pipeline");
        let staging = p.staging.as_ref().expect("staging");
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        unsafe {
            self.context.CopyResource(staging, &p.scratch);
            self.context.Map(staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        }
        Ok(MappedNv12 { conv: self, data: mapped.pData as *const u8, row_pitch: mapped.RowPitch as usize })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fills a BGRA texture with one colour and checks the NV12 output luma/chroma match BT.709 limited range.
    fn convert_solid(b: u8, g: u8, r: u8) -> (u8, u8, u8) {
        let d3d = D3D11Context::new(-1).unwrap();
        let (w, h) = (64u32, 32u32);
        let px: Vec<u8> = (0..w * h).flat_map(|_| [b, g, r, 255]).collect();
        let init = D3D11_SUBRESOURCE_DATA { pSysMem: px.as_ptr() as _, SysMemPitch: w * 4, SysMemSlicePitch: 0 };
        let mut src = None;
        unsafe {
            d3d.device
                .CreateTexture2D(
                    &tex_desc(
                        w,
                        h,
                        DXGI_FORMAT_B8G8R8A8_UNORM,
                        D3D11_USAGE_DEFAULT,
                        D3D11_BIND_SHADER_RESOURCE.0 as u32,
                        0,
                    ),
                    Some(&init),
                    Some(&mut src),
                )
                .unwrap();
        }
        let src = src.unwrap();
        let mut conv = Bgra2Nv12Converter::new(&d3d, w, h).unwrap();
        let m = conv.map_system_nv12(&src).unwrap();
        unsafe {
            let y = *m.data.add(m.row_pitch * 8 + 8);
            let uv = m.data.add(m.row_pitch * h as usize + m.row_pitch * 4 + 8);
            (y, *uv, *uv.add(1))
        }
    }

    #[test]
    fn colour_space_is_bt709_limited() {
        let (y, u, v) = convert_solid(0, 0, 0);
        assert!((15..=17).contains(&y), "black Y={y}");
        assert!((126..=130).contains(&u) && (126..=130).contains(&v), "black UV={u},{v}");
        let (y, _, _) = convert_solid(255, 255, 255);
        assert!((233..=236).contains(&y), "white Y={y}");
        // Pure red, BT.709 limited: Y = 16 + 219*0.2126 = 62.6, Cr = 128 + 112 = 240.
        let (y, u, v) = convert_solid(0, 0, 255);
        assert!((60..=65).contains(&y), "red Y={y}");
        assert!(v >= 235, "red V={v}");
        assert!(u < 110, "red U={u}");
    }

    #[test]
    fn pipeline_rebuilds_on_resize_and_scales() {
        let d3d = D3D11Context::new(-1).unwrap();
        let mut conv = Bgra2Nv12Converter::new(&d3d, 1281, 721).unwrap();
        assert_eq!((conv.out_width(), conv.out_height()), (1280, 720));
        for (w, h) in [(3440u32, 1440u32), (1920, 1080)] {
            let mut t = None;
            unsafe {
                d3d.device
                    .CreateTexture2D(
                        &tex_desc(
                            w,
                            h,
                            DXGI_FORMAT_B8G8R8A8_UNORM,
                            D3D11_USAGE_DEFAULT,
                            D3D11_BIND_SHADER_RESOURCE.0 as u32,
                            0,
                        ),
                        None,
                        Some(&mut t),
                    )
                    .unwrap();
            }
            let m = conv.map_system_nv12(&t.unwrap()).unwrap();
            assert!(m.row_pitch >= 1280);
        }
    }
}
