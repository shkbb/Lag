//! Mirrors Services/VfrCapture/AacAudioEncoder.cs. Native state is serialized against teardown.
use crate::{check, AudioError, EncodedPacket, PcmFormat, Result, StreamSpec};
use lag_ffi::ffmpeg as ff;
use std::{
    ffi::CStr,
    ptr::{null, null_mut},
    sync::Mutex,
};
pub const OUT_SAMPLE_RATE: i32 = 48000;
pub const OUT_CHANNELS: i32 = 2;

pub(crate) struct Resampler {
    ptr: *mut ff::SwrContext,
    pub format: PcmFormat,
    planes: [Vec<f32>; 2],
}
// SwrContext has no thread affinity; every use requires exclusive access.
unsafe impl Send for Resampler {}
impl Resampler {
    pub fn new(format: PcmFormat) -> Result<Self> {
        if format.channels == 0
            || format.sample_rate == 0
            || format.sample_rate > i32::MAX as u32
            || !matches!((format.is_float, format.bits_per_sample), (true, 32) | (false, 16 | 32))
        {
            return Err(AudioError("Unsupported PCM format".into()));
        }
        let mut s = Self { ptr: null_mut(), format, planes: [Vec::new(), Vec::new()] };
        unsafe {
            let mut il = std::mem::zeroed();
            let mut ol = std::mem::zeroed();
            ff::av_channel_layout_default(&mut il, format.channels as i32);
            ff::av_channel_layout_default(&mut ol, 2);
            let r = ff::swr_alloc_set_opts2(
                &mut s.ptr,
                &ol,
                ff::AV_SAMPLE_FMT_FLTP,
                48000,
                &il,
                format.sample_format(),
                format.sample_rate as i32,
                0,
                null_mut(),
            );
            ff::av_channel_layout_uninit(&mut il);
            ff::av_channel_layout_uninit(&mut ol);
            check(r, "swr alloc")?;
            if s.ptr.is_null() {
                return Err(AudioError("swr alloc failed".into()));
            }
            check(ff::swr_init(s.ptr), "swr init")?;
        }
        Ok(s)
    }
    pub fn convert(&mut self, bytes: &[u8]) -> Result<usize> {
        let samples = bytes.len() / self.format.block_align();
        if samples == 0 {
            return Ok(0);
        }
        let samples = i32::try_from(samples).map_err(|_| AudioError("PCM buffer too large".into()))?;
        unsafe {
            let n = ff::swr_get_out_samples(self.ptr, samples);
            check(n, "swr capacity")?;
            for p in &mut self.planes {
                p.resize(n as usize, 0.);
            }
            let out = self.pointers();
            let r = ff::swr_convert(self.ptr, out.as_ptr(), n, [bytes.as_ptr()].as_ptr(), samples);
            check(r, "swr convert")?;
            Ok(r as usize)
        }
    }
    fn pointers(&mut self) -> [*mut u8; 2] {
        [self.planes[0].as_mut_ptr().cast(), self.planes[1].as_mut_ptr().cast()]
    }
    pub(crate) fn samples(&self, n: usize) -> impl Iterator<Item = [f32; 2]> + '_ {
        (0..n).map(|i| [self.planes[0][i], self.planes[1][i]])
    }
}
impl Drop for Resampler {
    fn drop(&mut self) {
        unsafe {
            ff::swr_free(&mut self.ptr);
        }
    }
}
struct State {
    ctx: *mut ff::AVCodecContext,
    fifo: *mut ff::AVAudioFifo,
    mic_fifo: *mut ff::AVAudioFifo,
    frame: *mut ff::AVFrame,
    packet: *mut ff::AVPacket,
    swr: Resampler,
    mic: Option<Resampler>,
    written: i64,
    read: i64,
    flushed: bool,
}
// All native pointers are owned, non-affine FFmpeg objects accessed under AacAudioEncoder's mutex.
unsafe impl Send for State {}
impl Drop for State {
    fn drop(&mut self) {
        unsafe {
            ff::av_packet_free(&mut self.packet);
            ff::av_frame_free(&mut self.frame);
            if !self.fifo.is_null() {
                ff::av_audio_fifo_free(self.fifo);
            }
            if !self.mic_fifo.is_null() {
                ff::av_audio_fifo_free(self.mic_fifo);
            }
            ff::avcodec_free_context(&mut self.ctx);
        }
    }
}
pub struct AacAudioEncoder {
    state: Mutex<State>,
}
impl AacAudioEncoder {
    pub fn new(input: PcmFormat, bitrate_kbps: i32) -> Result<Self> {
        let mut s = State {
            ctx: null_mut(),
            fifo: null_mut(),
            mic_fifo: null_mut(),
            frame: null_mut(),
            packet: null_mut(),
            swr: Resampler::new(input)?,
            mic: None,
            written: 0,
            read: 0,
            flushed: false,
        };
        unsafe {
            let codec = ff::avcodec_find_encoder(ff::AV_CODEC_ID_AAC);
            if codec.is_null() {
                return Err(AudioError("AAC encoder not found in avcodec.".into()));
            }
            s.ctx = ff::avcodec_alloc_context3(codec);
            if s.ctx.is_null() {
                return Err(AudioError("AAC allocation failed".into()));
            }
            (*s.ctx).sample_rate = OUT_SAMPLE_RATE;
            (*s.ctx).bit_rate = bitrate_kbps.max(64) as i64 * 1000;
            (*s.ctx).sample_fmt = ff::AV_SAMPLE_FMT_FLTP;
            ff::av_channel_layout_default(&mut (*s.ctx).ch_layout, 2);
            (*s.ctx).time_base = ff::AVRational { num: 1, den: OUT_SAMPLE_RATE };
            (*s.ctx).flags |= ff::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            check(ff::avcodec_open2(s.ctx, codec, null_mut()), "open aac")?;
            s.fifo = ff::av_audio_fifo_alloc(ff::AV_SAMPLE_FMT_FLTP, 2, 1);
            s.mic_fifo = ff::av_audio_fifo_alloc(ff::AV_SAMPLE_FMT_FLTP, 2, 1);
            s.frame = ff::av_frame_alloc();
            s.packet = ff::av_packet_alloc();
            if s.fifo.is_null() || s.mic_fifo.is_null() || s.frame.is_null() || s.packet.is_null() {
                return Err(AudioError("AAC allocation failed".into()));
            }
            let fmt = CStr::from_ptr(ff::av_get_sample_fmt_name(input.sample_format())).to_string_lossy();
            log::info!(
                "[AacAudioEncoder] AAC open: in {}Hz {}ch {} → 48kHz stereo, {}kbps.",
                input.sample_rate,
                input.channels,
                format_args!("AV_SAMPLE_FMT_{}", fmt.to_uppercase()),
                bitrate_kbps
            );
        }
        Ok(Self { state: Mutex::new(s) })
    }
    /// Input format must match construction. Returns owned compressed packets, safe to retain in rings.
    pub fn encode(&self, data: &[u8], chunk_engine_us: i64) -> Result<Vec<EncodedPacket>> {
        let mut s = self.state.lock().unwrap();
        let mut out = Vec::new();
        if data.is_empty() || s.flushed {
            return Ok(out);
        }
        let n = s.swr.convert(data)?;
        if n > 0 {
            unsafe {
                s.pad_to(chunk_engine_us)?;
                let p = s.swr.pointers();
                s.mix_mic(p, n)?;
                s.write(p, n)?;
                s.written += n as i64;
                s.drain(&mut out)?;
            }
        }
        Ok(out)
    }
    pub fn encode_mic(&self, data: &[u8], format: PcmFormat) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        if data.is_empty() || s.flushed {
            return Ok(());
        }
        if s.mic.as_ref().is_none_or(|m| m.format != format) {
            s.mic = Some(Resampler::new(format)?);
            let fmt = unsafe { CStr::from_ptr(ff::av_get_sample_fmt_name(format.sample_format())).to_string_lossy() };
            log::info!(
                "[AacAudioEncoder] mic mix on: {}Hz {}ch AV_SAMPLE_FMT_{} → 48kHz stereo.",
                format.sample_rate,
                format.channels,
                fmt.to_uppercase()
            );
        }
        let m = s.mic.as_mut().unwrap();
        let n = m.convert(data)?;
        let p = m.pointers();
        unsafe {
            let size = ff::av_audio_fifo_size(s.mic_fifo);
            if size + n as i32 > 48000 {
                check(ff::av_audio_fifo_drain(s.mic_fifo, size.min(size + n as i32 - 48000)), "mic fifo drain")?;
            }
            check(ff::av_audio_fifo_write(s.mic_fifo, p.as_ptr().cast(), n as i32), "mic fifo write")?;
        }
        Ok(())
    }
    pub fn pump_silence_to(&self, engine_us: i64) -> Result<Vec<EncodedPacket>> {
        let mut s = self.state.lock().unwrap();
        let mut out = Vec::new();
        if !s.flushed {
            unsafe {
                s.pad_to(engine_us)?;
                s.drain(&mut out)?;
            }
        }
        Ok(out)
    }
    /// Like C#: submits codec EOF; does not pad a partial FIFO frame or drain resampler latency.
    pub fn flush(&self) -> Result<Vec<EncodedPacket>> {
        let mut s = self.state.lock().unwrap();
        let mut out = Vec::new();
        if !s.flushed {
            unsafe {
                s.submit(null(), &mut out)?;
            }
            s.flushed = true;
        }
        Ok(out)
    }
    pub fn get_stream_spec(&self) -> StreamSpec {
        let s = self.state.lock().unwrap();
        let extra_data = unsafe {
            if (*s.ctx).extradata_size > 0 {
                std::slice::from_raw_parts((*s.ctx).extradata, (*s.ctx).extradata_size as usize).to_vec()
            } else {
                Vec::new()
            }
        };
        StreamSpec {
            codec_id: ff::AV_CODEC_ID_AAC,
            is_video: false,
            width: 0,
            height: 0,
            sample_rate: 48000,
            channels: 2,
            extra_data,
        }
    }
    pub fn written_samples(&self) -> i64 {
        self.state.lock().unwrap().written
    }
}
impl State {
    unsafe fn write(&mut self, p: [*mut u8; 2], n: usize) -> Result<()> {
        check(ff::av_audio_fifo_write(self.fifo, p.as_ptr().cast(), n as i32), "audio fifo write")
    }
    unsafe fn pad_to(&mut self, target: i64) -> Result<()> {
        let gap = target * 48000 / 1_000_000 - self.written;
        if gap <= 0 || gap > 48000 * 5 {
            return Ok(());
        }
        let mut zero = [[0f32; 4096]; 2];
        let mut left = gap;
        while left > 0 {
            let n = left.min(4096) as usize;
            zero[0][..n].fill(0.);
            zero[1][..n].fill(0.);
            let p = [zero[0].as_mut_ptr().cast(), zero[1].as_mut_ptr().cast()];
            self.mix_mic(p, n)?;
            self.write(p, n)?;
            left -= n as i64;
        }
        self.written += gap;
        Ok(())
    }
    unsafe fn mix_mic(&mut self, p: [*mut u8; 2], n: usize) -> Result<()> {
        let n = n.min(ff::av_audio_fifo_size(self.mic_fifo) as usize);
        if n == 0 {
            return Ok(());
        }
        let mut m = [vec![0f32; n], vec![0f32; n]];
        let mp = [m[0].as_mut_ptr().cast(), m[1].as_mut_ptr().cast()];
        check(ff::av_audio_fifo_read(self.mic_fifo, mp.as_ptr(), n as i32), "mic fifo read")?;
        for c in 0..2 {
            let dst = std::slice::from_raw_parts_mut(p[c].cast::<f32>(), n);
            for (d, v) in dst.iter_mut().zip(&m[c]) {
                *d = (*d + v).clamp(-1., 1.);
            }
        }
        Ok(())
    }
    unsafe fn drain(&mut self, out: &mut Vec<EncodedPacket>) -> Result<()> {
        let n = if (*self.ctx).frame_size > 0 { (*self.ctx).frame_size } else { 1024 };
        while ff::av_audio_fifo_size(self.fifo) >= n {
            ff::av_frame_unref(self.frame);
            (*self.frame).nb_samples = n;
            (*self.frame).format = ff::AV_SAMPLE_FMT_FLTP;
            ff::av_channel_layout_copy(&mut (*self.frame).ch_layout, &(*self.ctx).ch_layout);
            (*self.frame).sample_rate = 48000;
            check(ff::av_frame_get_buffer(self.frame, 0), "audio frame buffer")?;
            check(ff::av_audio_fifo_read(self.fifo, (*self.frame).data.as_ptr().cast(), n), "audio fifo read")?;
            (*self.frame).pts = self.read;
            self.read += n as i64;
            self.submit(self.frame, out)?;
        }
        Ok(())
    }
    unsafe fn submit(&mut self, frame: *const ff::AVFrame, out: &mut Vec<EncodedPacket>) -> Result<()> {
        check(ff::avcodec_send_frame(self.ctx, frame), "aac send frame")?;
        loop {
            let r = ff::avcodec_receive_packet(self.ctx, self.packet);
            if r == lag_ffi::AVERROR_EAGAIN || r == -541478725 {
                break;
            }
            check(r, "aac receive packet")?;
            let pts = if (*self.packet).pts == i64::MIN { 0 } else { (*self.packet).pts * 1_000_000 / 48000 };
            let data = std::slice::from_raw_parts((*self.packet).data, (*self.packet).size as usize);
            out.push(EncodedPacket::new(data, pts, pts, true));
            ff::av_packet_unref(self.packet);
        }
        Ok(())
    }
}
