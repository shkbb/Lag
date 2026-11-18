//! WaveFormat and sample helpers from WasapiAudioSource.cs and AudioMonitor.cs.
use lag_ffi::ffmpeg as ff;
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct PcmFormat {
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub is_float: bool,
}
impl PcmFormat {
    pub const SYSTEM: Self = Self { sample_rate: 48000, channels: 2, bits_per_sample: 32, is_float: true };
    pub fn block_align(self) -> usize {
        self.channels as usize * self.bits_per_sample as usize / 8
    }
    pub fn sample_format(self) -> ff::AVSampleFormat {
        if self.is_float {
            ff::AV_SAMPLE_FMT_FLT
        } else {
            match self.bits_per_sample {
                16 => ff::AV_SAMPLE_FMT_S16,
                32 => ff::AV_SAMPLE_FMT_S32,
                _ => ff::AV_SAMPLE_FMT_S16,
            }
        }
    }
    pub fn duration(self, bytes: usize) -> f64 {
        bytes as f64 / (self.sample_rate as f64 * self.block_align() as f64)
    }
}
pub fn peak(data: &[u8], f: PcmFormat) -> f32 {
    let mut p = 0f32;
    if f.is_float && f.bits_per_sample == 32 {
        for b in data.as_chunks::<4>().0.iter() {
            p = p.max(f32::from_le_bytes(*b).abs());
        }
    } else if f.bits_per_sample == 16 {
        for b in data.as_chunks::<2>().0.iter() {
            p = p.max((i16::from_le_bytes(*b) as f32 / 32768.).abs());
        }
    }
    p.min(1.)
}
pub fn curve(p: f32) -> f32 {
    (p as f64).powf(0.6) as f32
}
pub fn gain(data: &mut [u8], gain: f32) {
    floats_mut(data, |_, v| (v * gain).clamp(-1., 1.));
}
pub fn ramp(data: &mut [u8], from: f32, to: f32) {
    let n = data.len() / 4;
    floats_mut(data, |i, v| v * (from + (to - from) * (i as f32 / n as f32)));
}
pub fn floats_mut(data: &mut [u8], mut f: impl FnMut(usize, f32) -> f32) {
    for (i, b) in data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let v = f32::from_le_bytes(*b);
        b.copy_from_slice(&f(i, v).to_le_bytes());
    }
}
