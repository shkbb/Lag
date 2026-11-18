//! Ports of `EncoderSelector.cs` and `NvencVfrEncoder.cs`: pick a working video encoder and run it
//! variable-frame-rate on the shared D3D11 device.
//!
//! Paths (unchanged from C#):
//! * NVENC/AMF on the capture GPU -> D3D11 hwframe pool, zero-copy (`AV_PIX_FMT_D3D11`);
//! * Intel QSV -> NV12 from system memory (qsv opens its own MFX session);
//! * x264 / a hardware encoder of a different vendor than the capture GPU -> NV12 from system memory.
//!
//! Fixes vs C#:
//! * a conversion failure after `av_hwframe_get_buffer` returned without `av_frame_unref`, leaking one
//!   of the 16 pool slots each time (enough failures and the encoder runs out of surfaces); the frame is
//!   now always unreferenced;
//! * packets are produced into a reusable buffer and handed out as `Arc<[u8]>` (one copy, as before),
//!   via a returned Vec instead of a multicast event.

use crate::convert::Bgra2Nv12Converter;
use crate::d3d::{D3D11Context, VENDOR_AMD, VENDOR_INTEL, VENDOR_NVIDIA};
use lag_ffi::d3d11va::{AVD3D11VADeviceContext, AVD3D11VAFramesContext};
use lag_ffi::ffmpeg as ff;
use lag_ffi::media::{EncodedPacket, StreamSpec};
use lag_ffi::{av_err_string, AVERROR_EAGAIN, AVERROR_EOF, AV_NOPTS_VALUE};
use std::ffi::{c_void, CString};
use std::ptr;
use std::sync::OnceLock;
use std::time::Instant;
use windows::core::Interface;
use windows::Win32::Graphics::Direct3D11::{ID3D11Texture2D, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CodecTier {
    Av1,
    Hevc,
    H264,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncoderChoice {
    pub ffmpeg_name: &'static str,
    pub codec_id: ff::AVCodecID,
    pub tier: CodecTier,
    pub friendly_name: &'static str,
    pub is_hardware: bool,
}

/// Candidates in descending preference (EncoderSelector.cs Candidates).
const CANDIDATES: &[(&str, ff::AVCodecID, CodecTier, &str, bool)] = &[
    ("av1_nvenc", ff::AV_CODEC_ID_AV1, CodecTier::Av1, "AV1 (NVIDIA)", true),
    ("av1_amf", ff::AV_CODEC_ID_AV1, CodecTier::Av1, "AV1 (AMD)", true),
    ("av1_qsv", ff::AV_CODEC_ID_AV1, CodecTier::Av1, "AV1 (Intel)", true),
    ("hevc_nvenc", ff::AV_CODEC_ID_HEVC, CodecTier::Hevc, "HEVC (NVIDIA)", true),
    ("hevc_amf", ff::AV_CODEC_ID_HEVC, CodecTier::Hevc, "HEVC (AMD)", true),
    ("hevc_qsv", ff::AV_CODEC_ID_HEVC, CodecTier::Hevc, "HEVC (Intel)", true),
    ("h264_nvenc", ff::AV_CODEC_ID_H264, CodecTier::H264, "H.264 (NVIDIA)", true),
    ("h264_amf", ff::AV_CODEC_ID_H264, CodecTier::H264, "H.264 (AMD)", true),
    ("h264_qsv", ff::AV_CODEC_ID_H264, CodecTier::H264, "H.264 (Intel)", true),
    ("libx264", ff::AV_CODEC_ID_H264, CodecTier::H264, "H.264 (CPU / x264)", false),
];

/// Encoders that actually open on this machine (probed once, lazily).
pub fn available() -> &'static [EncoderChoice] {
    static AVAILABLE: OnceLock<Vec<EncoderChoice>> = OnceLock::new();
    AVAILABLE.get_or_init(|| {
        // C# probed on a dedicated MTA thread; drivers may require it for COM init.
        std::thread::Builder::new()
            .name("EncoderProbe".into())
            .spawn(probe)
            .and_then(|h| h.join().map_err(|_| std::io::Error::other("probe panicked")))
            .unwrap_or_default()
    })
}

fn probe() -> Vec<EncoderChoice> {
    let mut found = Vec::new();
    for &(name, id, tier, friendly, hw) in CANDIDATES {
        if can_open(name) {
            log::info!("[EncoderSelector] Usable: {friendly} ({name})");
            found.push(EncoderChoice {
                ffmpeg_name: name,
                codec_id: id,
                tier,
                friendly_name: friendly,
                is_hardware: hw,
            });
        }
    }
    if found.is_empty() {
        log::info!("[EncoderSelector] No usable video encoder found (not even libx264).");
    }
    found
}

fn can_open(name: &str) -> bool {
    let cname = CString::new(name).expect("encoder name");
    unsafe {
        let codec = ff::avcodec_find_encoder_by_name(cname.as_ptr());
        if codec.is_null() {
            return false;
        }
        let mut ctx = ff::avcodec_alloc_context3(codec);
        if ctx.is_null() {
            return false;
        }
        (*ctx).width = 1280;
        (*ctx).height = 720;
        (*ctx).time_base = ff::AVRational { num: 1, den: 1000 };
        (*ctx).framerate = ff::AVRational { num: 60, den: 1 };
        (*ctx).pix_fmt = ff::AV_PIX_FMT_NV12;
        (*ctx).bit_rate = 4_000_000;
        (*ctx).gop_size = 120;
        let ok = ff::avcodec_open2(ctx, codec, ptr::null_mut()) >= 0;
        ff::avcodec_free_context(&mut ctx);
        ok
    }
}

/// EncoderSelector.Select over an explicit list (testable without hardware).
pub fn select_from(avail: &[EncoderChoice], preference: Option<&str>) -> Option<EncoderChoice> {
    let first = avail.first()?.clone();
    let pref = preference.unwrap_or("").trim().to_lowercase();
    if matches!(pref.as_str(), "x264" | "libx264" | "cpu") {
        if let Some(c) = avail.iter().find(|c| c.ffmpeg_name == "libx264") {
            return Some(c.clone());
        }
    }
    let vendor = match pref.as_str() {
        "nvenc" => Some("nvenc"),
        "amf" => Some("amf"),
        "qsv" => Some("qsv"),
        _ => None,
    };
    if let Some(v) = vendor {
        if let Some(c) = avail.iter().find(|c| c.ffmpeg_name.contains(v) && c.tier == CodecTier::H264) {
            return Some(c.clone());
        }
        if let Some(c) = avail.iter().find(|c| c.ffmpeg_name.contains(v)) {
            return Some(c.clone());
        }
        log::info!("[EncoderSelector] backend '{v}' unavailable here; using best available.");
        return Some(first);
    }
    let wanted = match pref.as_str() {
        "av1" => Some(CodecTier::Av1),
        "hevc" | "h265" => Some(CodecTier::Hevc),
        "h264" | "avc" => Some(CodecTier::H264),
        _ => None,
    };
    if let Some(tier) = wanted {
        if let Some(c) = avail.iter().find(|c| c.tier == tier) {
            return Some(c.clone());
        }
        log::info!("[EncoderSelector] Preferred codec '{pref}' unavailable on this system; using best available.");
        return Some(first);
    }
    // AUTO: hardware H.264 (fastest encode, solid CBR); else the best that opened.
    avail.iter().find(|c| c.tier == CodecTier::H264 && c.is_hardware).cloned().or(Some(first))
}

pub fn select(preference: Option<&str>) -> Option<EncoderChoice> {
    select_from(available(), preference)
}

const FORCED_KEYFRAME_US: i64 = 2_000_000;
pub const TIME_BASE_DEN: i32 = 1_000_000;
const QUALITY_X264: i32 = 21;
const QUALITY_QSV: i32 = 23;
const QUALITY_AMF: i32 = 23;
const QP_NVENC_H264: i32 = 20;
const QP_NVENC_HEVC: i32 = 22;

fn is_vendor_matched(name: &str, vendor: u32) -> bool {
    (name.contains("nvenc") && vendor == VENDOR_NVIDIA)
        || (name.contains("amf") && vendor == VENDOR_AMD)
        || (name.contains("qsv") && vendor == VENDOR_INTEL)
}

/// Per-frame stage timings accumulated for the periodic capture log.
#[derive(Debug, Default, Clone, Copy)]
pub struct EncodeTimings {
    pub convert_us: i64,
    pub encode_us: i64,
    pub frames: i64,
}

pub struct VideoEncoder {
    choice: EncoderChoice,
    converter: Bgra2Nv12Converter,
    hw_device: *mut ff::AVBufferRef,
    hw_frames: *mut ff::AVBufferRef,
    ctx: *mut ff::AVCodecContext,
    pkt: *mut ff::AVPacket,
    frame: *mut ff::AVFrame,
    width: i32,
    height: i32,
    fps_hint: i32,
    hw_mode: bool,
    last_forced_kf_us: Option<i64>,
    pub timings: EncodeTimings,
}

// SAFETY: all FFmpeg objects are owned exclusively by this struct; the engine guards it with a mutex
// (C#'s `_opLock`) so encode/flush/drop never run concurrently.
unsafe impl Send for VideoEncoder {}

impl VideoEncoder {
    /// `bitrate_kbps <= 0` = "Auto" (constant quality). `preset` 1 = fastest .. 7 = slowest.
    pub fn new(
        d3d: &D3D11Context,
        choice: EncoderChoice,
        width: i32,
        height: i32,
        bitrate_kbps: i32,
        preset: i32,
        fps_hint: i32,
    ) -> Result<Self, String> {
        let qsv_mode = choice.ffmpeg_name.contains("qsv");
        let hw_mode = is_vendor_matched(choice.ffmpeg_name, d3d.vendor_id) && !qsv_mode;
        log::info!(
            "[NvencVfrEncoder] {}: {} path (capture GPU vendor 0x{:04X}).",
            choice.friendly_name,
            if hw_mode { "D3D11 zero-copy" } else { "system-memory feed" },
            d3d.vendor_id
        );
        let (width, height) = (width & !1, height & !1);
        let converter = Bgra2Nv12Converter::new(d3d, width as u32, height as u32).map_err(|e| e.to_string())?;
        let mut enc = Self {
            choice,
            converter,
            hw_device: ptr::null_mut(),
            hw_frames: ptr::null_mut(),
            ctx: ptr::null_mut(),
            pkt: ptr::null_mut(),
            frame: ptr::null_mut(),
            width,
            height,
            fps_hint: if fps_hint > 0 { fps_hint } else { 120 },
            hw_mode,
            last_forced_kf_us: None,
            timings: EncodeTimings::default(),
        };
        // On error `enc` drops and frees whatever was allocated (C#: catch { Dispose(); throw; }).
        unsafe {
            if hw_mode {
                enc.setup_hardware(d3d)?;
            }
            enc.open(bitrate_kbps, preset, qsv_mode)?;
            enc.pkt = ff::av_packet_alloc();
            enc.frame = ff::av_frame_alloc();
            if enc.pkt.is_null() || enc.frame.is_null() {
                return Err("av_packet_alloc/av_frame_alloc failed".into());
            }
        }
        Ok(enc)
    }

    pub fn choice(&self) -> &EncoderChoice {
        &self.choice
    }
    pub fn width(&self) -> i32 {
        self.width
    }
    pub fn height(&self) -> i32 {
        self.height
    }
    pub fn fps_hint(&self) -> i32 {
        self.fps_hint
    }

    unsafe fn setup_hardware(&mut self, d3d: &D3D11Context) -> Result<(), String> {
        self.hw_device = ff::av_hwdevice_ctx_alloc(ff::AV_HWDEVICE_TYPE_D3D11VA);
        if self.hw_device.is_null() {
            return Err("av_hwdevice_ctx_alloc failed.".into());
        }
        let dev_ctx = (*self.hw_device).data as *mut ff::AVHWDeviceContext;
        let d3d_ctx = (*dev_ctx).hwctx as *mut AVD3D11VADeviceContext;
        // FFmpeg releases both on teardown, so hand it an owned (AddRef'd) reference of each.
        (*d3d_ctx).device = d3d.device.clone().into_raw();
        // Share OUR immediate context explicitly: ffmpeg's work runs GPU-ordered with the converter.
        (*d3d_ctx).device_context = d3d.context.clone().into_raw();
        let r = ff::av_hwdevice_ctx_init(self.hw_device);
        if r < 0 {
            return Err(format!("hwdevice init: {}", av_err_string(r)));
        }
        self.hw_frames = ff::av_hwframe_ctx_alloc(self.hw_device);
        if self.hw_frames.is_null() {
            return Err("av_hwframe_ctx_alloc failed.".into());
        }
        let fc = (*self.hw_frames).data as *mut ff::AVHWFramesContext;
        (*fc).format = ff::AV_PIX_FMT_D3D11;
        (*fc).sw_format = ff::AV_PIX_FMT_NV12;
        (*fc).width = self.width;
        (*fc).height = self.height;
        (*fc).initial_pool_size = 16;
        let d3d_frames = (*fc).hwctx as *mut AVD3D11VAFramesContext;
        (*d3d_frames).BindFlags = (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32;
        let r = ff::av_hwframe_ctx_init(self.hw_frames);
        if r < 0 {
            return Err(format!("hwframes init: {}", av_err_string(r)));
        }
        Ok(())
    }

    unsafe fn open(&mut self, bitrate_kbps: i32, preset: i32, qsv_mode: bool) -> Result<(), String> {
        let name = CString::new(self.choice.ffmpeg_name).expect("name");
        let codec = ff::avcodec_find_encoder_by_name(name.as_ptr());
        if codec.is_null() {
            return Err(format!("encoder {} vanished.", self.choice.ffmpeg_name));
        }
        self.ctx = ff::avcodec_alloc_context3(codec);
        if self.ctx.is_null() {
            return Err("avcodec_alloc_context3 failed".into());
        }
        let c = &mut *self.ctx;
        let auto_quality = bitrate_kbps <= 0;
        c.width = self.width;
        c.height = self.height;
        c.pix_fmt = if self.hw_mode { ff::AV_PIX_FMT_D3D11 } else { ff::AV_PIX_FMT_NV12 };
        c.colorspace = ff::AVCOL_SPC_BT709;
        c.color_range = ff::AVCOL_RANGE_MPEG; // limited/tv, matches the converter
        c.color_primaries = ff::AVCOL_PRI_BT709;
        c.color_trc = ff::AVCOL_TRC_BT709;
        c.time_base = ff::AVRational { num: 1, den: TIME_BASE_DEN };
        // Rate-control HINT only; timing stays VFR via per-frame microsecond PTS.
        c.framerate = ff::AVRational { num: self.fps_hint, den: 1 };
        if auto_quality {
            c.bit_rate = 0;
            if qsv_mode {
                c.global_quality = QUALITY_QSV;
            }
        } else {
            let bps = bitrate_kbps.max(1) as i64 * 1000;
            c.bit_rate = bps;
            c.rc_max_rate = bps;
            c.rc_buffer_size = (bps * 2) as i32;
        }
        c.gop_size = 240;
        c.max_b_frames = 0;
        c.flags |= ff::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
        if self.hw_mode {
            c.hw_frames_ctx = ff::av_buffer_ref(self.hw_frames);
        }
        self.apply_codec_options(preset, auto_quality);
        let r = ff::avcodec_open2(self.ctx, codec, ptr::null_mut());
        if r < 0 {
            return Err(format!("open {}: {}", self.choice.ffmpeg_name, av_err_string(r)));
        }
        let rc = if auto_quality { "Auto (constant-quality)".to_string() } else { format!("CBR {bitrate_kbps}kbps") };
        log::info!(
            "[NvencVfrEncoder] {} open @ {}x{}, {rc}, fpsHint={}, VFR.",
            self.choice.friendly_name,
            self.width,
            self.height,
            self.fps_hint
        );
        Ok(())
    }

    unsafe fn apply_codec_options(&mut self, preset: i32, auto_quality: bool) {
        let priv_data = (*self.ctx).priv_data;
        let opt = |k: &str, v: &str| {
            let (ck, cv) = (CString::new(k).unwrap(), CString::new(v).unwrap());
            let r = ff::av_opt_set(priv_data, ck.as_ptr(), cv.as_ptr(), 0);
            if r < 0 {
                log::info!("[NvencVfrEncoder] opt '{k}={v}' rejected: {}", av_err_string(r));
            }
        };
        let name = self.choice.ffmpeg_name;
        if name.contains("nvenc") {
            opt("preset", &format!("p{}", preset.clamp(1, 7)));
            opt("tune", "hq");
            if auto_quality {
                opt("rc", "constqp");
                opt("qp", &(if name.contains("hevc") { QP_NVENC_HEVC } else { QP_NVENC_H264 }).to_string());
            } else {
                opt("rc", "cbr");
                opt("multipass", "qres");
            }
            opt("rc-lookahead", "0");
            opt("spatial-aq", "1");
            opt("forced-idr", "1");
        } else if name.contains("amf") {
            opt("quality", "speed");
            if auto_quality {
                opt("rc", "cqp");
                opt("qp_i", &QUALITY_AMF.to_string());
                opt("qp_p", &QUALITY_AMF.to_string());
            } else {
                opt("rc", "vbr_peak");
            }
            opt("preanalysis", "false");
            opt("forced_idr", "1");
        } else if name.contains("qsv") {
            opt("preset", "veryfast");
            opt("forced_idr", "1");
        } else if name == "libx264" {
            opt("preset", "veryfast");
            opt("tune", "zerolatency");
            if auto_quality {
                opt("crf", &QUALITY_X264.to_string());
            }
            opt("forced-idr", "1");
        }
    }

    /// Converts + submits one captured BGRA frame at `pts_us`; returns the packets that came out.
    /// `Err` only for a failed submit (C# returned false); a conversion hiccup skips the frame.
    pub fn encode(&mut self, bgra: &ID3D11Texture2D, pts_us: i64) -> Result<Vec<EncodedPacket>, String> {
        let t0 = Instant::now();
        let converted = unsafe {
            if self.hw_mode {
                self.convert_hardware(bgra)
            } else {
                self.convert_system(bgra)
            }
        };
        if let Err(e) = converted {
            unsafe { ff::av_frame_unref(self.frame) }; // never leak a pool slot (C# bug)
            log::info!("[NvencVfrEncoder] Encode error: {e}");
            return Ok(Vec::new());
        }
        let t1 = Instant::now();
        unsafe {
            (*self.frame).pts = pts_us;
            // VFR makes a frame-count GOP unbounded in TIME; force an IDR every 2 s of presentation time.
            match self.last_forced_kf_us {
                None => self.last_forced_kf_us = Some(pts_us), // frame 0 is an IDR already
                Some(last) if pts_us - last >= FORCED_KEYFRAME_US => {
                    (*self.frame).pict_type = ff::AV_PICTURE_TYPE_I;
                    self.last_forced_kf_us = Some(pts_us);
                }
                _ => {}
            }
            let sr = ff::avcodec_send_frame(self.ctx, self.frame);
            ff::av_frame_unref(self.frame);
            if sr < 0 && sr != AVERROR_EAGAIN {
                let msg = format!("[NvencVfrEncoder] send: {}", av_err_string(sr));
                log::info!("{msg}");
                return Err(msg);
            }
        }
        let out = self.drain_ready();
        let t2 = Instant::now();
        self.timings.convert_us += (t1 - t0).as_micros() as i64;
        self.timings.encode_us += (t2 - t1).as_micros() as i64;
        self.timings.frames += 1;
        Ok(out)
    }

    unsafe fn convert_hardware(&mut self, bgra: &ID3D11Texture2D) -> Result<(), String> {
        let gb = ff::av_hwframe_get_buffer(self.hw_frames, self.frame, 0);
        if gb < 0 {
            return Err(format!("get_buffer: {}", av_err_string(gb)));
        }
        let tex_ptr = (*self.frame).data[0] as *mut c_void;
        let slice = (*self.frame).data[1] as usize as u32;
        let tex = ID3D11Texture2D::from_raw_borrowed(&tex_ptr).ok_or("hwframe without texture")?;
        self.converter.convert_into(bgra, tex, slice).map_err(|e| e.to_string())
    }

    unsafe fn convert_system(&mut self, bgra: &ID3D11Texture2D) -> Result<(), String> {
        ff::av_frame_unref(self.frame);
        let f = &mut *self.frame;
        f.format = ff::AV_PIX_FMT_NV12;
        f.width = self.width;
        f.height = self.height;
        if ff::av_frame_get_buffer(self.frame, 32) < 0 {
            return Err("av_frame_get_buffer failed".into());
        }
        let (w, h) = (self.width as usize, self.height as usize);
        let (y_dst, uv_dst) = (f.data[0], f.data[1]);
        let (y_pitch, uv_pitch) = (f.linesize[0] as usize, f.linesize[1] as usize);
        let m = self.converter.map_system_nv12(bgra).map_err(|e| e.to_string())?;
        for r in 0..h {
            ptr::copy_nonoverlapping(m.data.add(r * m.row_pitch), y_dst.add(r * y_pitch), w);
        }
        let uv_src = m.data.add(m.row_pitch * h); // UV plane follows Y in the NV12 staging
        for r in 0..h / 2 {
            ptr::copy_nonoverlapping(uv_src.add(r * m.row_pitch), uv_dst.add(r * uv_pitch), w);
        }
        Ok(())
    }

    fn drain_ready(&mut self) -> Vec<EncodedPacket> {
        let mut out = Vec::new();
        unsafe {
            loop {
                let rr = ff::avcodec_receive_packet(self.ctx, self.pkt);
                if rr == AVERROR_EAGAIN || rr == AVERROR_EOF {
                    break;
                }
                if rr < 0 {
                    log::info!("[NvencVfrEncoder] receive: {}", av_err_string(rr));
                    break;
                }
                let p = &*self.pkt;
                let data: std::sync::Arc<[u8]> = std::slice::from_raw_parts(p.data, p.size as usize).into();
                // nvenc flags only the FIRST IDR with AV_PKT_FLAG_KEY; detect periodic IDRs from the bitstream.
                let key =
                    (p.flags & ff::AV_PKT_FLAG_KEY as i32) != 0 || is_keyframe_bitstream(&data, self.choice.codec_id);
                let dts = if p.dts == AV_NOPTS_VALUE { p.pts } else { p.dts };
                out.push(EncodedPacket { data, pts_us: p.pts, dts_us: dts, is_keyframe: key });
                ff::av_packet_unref(self.pkt);
            }
        }
        out
    }

    pub fn stream_spec(&self) -> StreamSpec {
        let extra = unsafe {
            let c = &*self.ctx;
            if c.extradata.is_null() || c.extradata_size <= 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(c.extradata, c.extradata_size as usize).to_vec()
            }
        };
        StreamSpec {
            codec_id: self.choice.codec_id,
            is_video: true,
            width: self.width,
            height: self.height,
            sample_rate: 0,
            channels: 0,
            extra_data: extra,
        }
    }

    /// Drains the encoder at end of stream.
    pub fn flush(&mut self) -> Vec<EncodedPacket> {
        unsafe { ff::avcodec_send_frame(self.ctx, ptr::null()) };
        self.drain_ready()
    }
}

impl Drop for VideoEncoder {
    fn drop(&mut self) {
        unsafe {
            if !self.pkt.is_null() {
                ff::av_packet_free(&mut self.pkt);
            }
            if !self.frame.is_null() {
                ff::av_frame_free(&mut self.frame);
            }
            if !self.ctx.is_null() {
                ff::avcodec_free_context(&mut self.ctx);
            }
            if !self.hw_frames.is_null() {
                ff::av_buffer_unref(&mut self.hw_frames);
            }
            if !self.hw_device.is_null() {
                ff::av_buffer_unref(&mut self.hw_device);
            }
        }
    }
}

/// IDR detection straight from the H.264/HEVC bitstream (Annex-B or 4-byte length-prefixed).
pub fn is_keyframe_bitstream(d: &[u8], codec: ff::AVCodecID) -> bool {
    let h264 = codec == ff::AV_CODEC_ID_H264;
    let hevc = codec == ff::AV_CODEC_ID_HEVC;
    if (!h264 && !hevc) || d.len() < 5 {
        return false;
    }
    let is_key_nal = |hdr: u8| if h264 { hdr & 0x1F == 5 } else { (16..=21).contains(&((hdr >> 1) & 0x3F)) };
    let n = d.len();
    if d[0] == 0 && d[1] == 0 && (d[2] == 1 || (d[2] == 0 && d[3] == 1)) {
        let mut i = 0;
        while i + 4 < n {
            if d[i] == 0 && d[i + 1] == 0 && d[i + 2] == 1 {
                if is_key_nal(d[i + 3]) {
                    return true;
                }
                i += 3;
            } else {
                i += 1;
            }
        }
        return false;
    }
    let mut pos = 0;
    while pos + 5 <= n {
        let len = u32::from_be_bytes([d[pos], d[pos + 1], d[pos + 2], d[pos + 3]]) as usize;
        if len == 0 || pos + 4 + len > n {
            break;
        }
        if is_key_nal(d[pos + 4]) {
            return true;
        }
        pos += 4 + len;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(name: &'static str, tier: CodecTier, hw: bool) -> EncoderChoice {
        EncoderChoice { ffmpeg_name: name, codec_id: ff::AV_CODEC_ID_H264, tier, friendly_name: name, is_hardware: hw }
    }

    #[test]
    fn selection_rules_match_csharp() {
        let avail = vec![
            choice("hevc_nvenc", CodecTier::Hevc, true),
            choice("h264_nvenc", CodecTier::H264, true),
            choice("libx264", CodecTier::H264, false),
        ];
        let pick = |p: Option<&str>| select_from(&avail, p).unwrap().ffmpeg_name;
        assert_eq!(pick(None), "h264_nvenc"); // auto -> hardware H.264
        assert_eq!(pick(Some("")), "h264_nvenc");
        assert_eq!(pick(Some("HEVC")), "hevc_nvenc");
        assert_eq!(pick(Some("h265")), "hevc_nvenc");
        assert_eq!(pick(Some("cpu")), "libx264");
        assert_eq!(pick(Some("nvenc")), "h264_nvenc");
        assert_eq!(pick(Some("amf")), "hevc_nvenc"); // vendor missing -> best available
        assert_eq!(pick(Some("av1")), "hevc_nvenc"); // tier missing -> best available
        assert!(select_from(&[], None).is_none());
    }

    #[test]
    fn keyframe_detection() {
        // Annex-B H.264 IDR (nal type 5) after SPS/PPS.
        let annexb = [0, 0, 0, 1, 0x67, 1, 2, 0, 0, 1, 0x68, 3, 0, 0, 1, 0x65, 9, 9];
        assert!(is_keyframe_bitstream(&annexb, ff::AV_CODEC_ID_H264));
        let p_frame = [0, 0, 0, 1, 0x41, 1, 2, 3, 4];
        assert!(!is_keyframe_bitstream(&p_frame, ff::AV_CODEC_ID_H264));
        // Length-prefixed HEVC IDR_W_RADL (type 19 -> header byte 19<<1 = 0x26).
        let hevc = [0, 0, 0, 3, 0x26, 1, 0xAA];
        assert!(is_keyframe_bitstream(&hevc, ff::AV_CODEC_ID_HEVC));
        assert!(!is_keyframe_bitstream(&hevc, ff::AV_CODEC_ID_AV1));
    }

    /// Real GPU encode: 2 s of a moving BGRA pattern through the zero-copy (or system) path.
    #[test]
    fn encodes_frames_on_this_gpu() {
        let d3d = D3D11Context::new(-1).unwrap();
        let Some(choice) = select(None) else { panic!("no encoder") };
        let mut enc = VideoEncoder::new(&d3d, choice.clone(), 1280, 720, 20_000, 4, 120).unwrap();
        use windows::Win32::Graphics::Direct3D11::*;
        use windows::Win32::Graphics::Dxgi::Common::*;
        let (w, h) = (1920u32, 1080u32);
        let mut packets = Vec::new();
        for i in 0..240i64 {
            let px: Vec<u8> = (0..w * h).flat_map(|p| [(p as i64 + i * 7) as u8, (i * 3) as u8, 128, 255]).collect();
            let init = D3D11_SUBRESOURCE_DATA { pSysMem: px.as_ptr() as _, SysMemPitch: w * 4, SysMemSlicePitch: 0 };
            let desc = D3D11_TEXTURE2D_DESC {
                Width: w,
                Height: h,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };
            let mut t = None;
            unsafe { d3d.device.CreateTexture2D(&desc, Some(&init), Some(&mut t)).unwrap() };
            packets.extend(enc.encode(&t.unwrap(), i * 8_333).unwrap()); // 120 fps for 2 s
        }
        packets.extend(enc.flush());
        let keys = packets.iter().filter(|p| p.is_keyframe).count();
        eprintln!(
            "{}: {} packets, {} keyframes, avg convert {} us, encode {} us",
            choice.friendly_name,
            packets.len(),
            keys,
            enc.timings.convert_us / enc.timings.frames,
            enc.timings.encode_us / enc.timings.frames
        );
        assert_eq!(packets.len(), 240);
        assert!(keys >= 1, "first frame must be a keyframe");
        assert!(packets.windows(2).all(|w| w[1].pts_us > w[0].pts_us));
        assert!(!enc.stream_spec().extra_data.is_empty(), "global header extradata (avcC) required");
    }
}
