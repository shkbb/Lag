//! Media types shared by lag-audio (producer of AAC packets), lag-capture (video encoder, packet rings,
//! muxer). Mirrors `EncodedPacket` (NvencVfrEncoder.cs:10) and `StreamSpec` (Mp4Muxer.cs:10).
use std::sync::Arc;

/// One encoded packet. Timestamps are microseconds on the ENGINE clock (QPC-based, shared by audio and
/// video so they stay in sync). `data` is reference-counted: ring snapshots for save/carry-over share the
/// bytes instead of copying them (C# shared the byte[] references the same way).
#[derive(Debug, Clone)]
pub struct EncodedPacket {
    pub data: Arc<[u8]>,
    pub pts_us: i64,
    pub dts_us: i64,
    pub is_keyframe: bool,
}

impl EncodedPacket {
    pub fn new(data: impl Into<Arc<[u8]>>, pts_us: i64, dts_us: i64, is_keyframe: bool) -> Self {
        Self { data: data.into(), pts_us, dts_us, is_keyframe }
    }
    /// Same packet shifted by `delta_us` (used by snapshot re-basing).
    pub fn shifted(&self, delta_us: i64) -> Self {
        Self {
            data: self.data.clone(),
            pts_us: self.pts_us + delta_us,
            dts_us: self.dts_us + delta_us,
            is_keyframe: self.is_keyframe,
        }
    }
}

/// Codec parameters a muxer needs to create one output stream.
#[derive(Debug, Clone)]
pub struct StreamSpec {
    pub codec_id: crate::ffmpeg::AVCodecID,
    pub is_video: bool,
    pub width: i32,
    pub height: i32,
    pub sample_rate: i32,
    pub channels: i32,
    pub extra_data: Vec<u8>,
}
