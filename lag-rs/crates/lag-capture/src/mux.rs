//! Port of `Mp4Muxer.cs`: writes buffered video + N audio tracks with their real (variable) timestamps.
//!
//! Same as C#: container from the extension (mp4 / mov / matroska), µs time base rescaled to whatever
//! the muxer assigns, first audio track = default disposition (the mixed "All audio"), track titles set
//! (also as the mp4 handler name, which is what players show), no `+faststart`.
//!
//! Fixes vs C#:
//! * `av_interleaved_write_frame` / `av_write_trailer` failures were ignored and the save reported success
//!   on a broken file; now they fail the write;
//! * packets are merged with a STABLE sort (C# `List.Sort` is unstable: equal-pts packets could swap);
//! * one AVPacket is reused instead of allocating one per packet.

use lag_ffi::av_err_string;
use lag_ffi::ffmpeg as ff;
use lag_ffi::media::{EncodedPacket, StreamSpec};
use std::ffi::CString;
use std::ptr;

const US: ff::AVRational = ff::AVRational { num: 1, den: 1_000_000 };

pub struct AudioTrack<'a> {
    pub spec: StreamSpec,
    pub packets: &'a [EncodedPacket],
    pub title: String,
}

/// Owns the format context; frees it (and closes the file) on drop, on every path.
struct Output(*mut ff::AVFormatContext);

impl Drop for Output {
    fn drop(&mut self) {
        unsafe {
            if !self.0.is_null() {
                if !(*self.0).pb.is_null() {
                    ff::avio_closep(&mut (*self.0).pb);
                }
                ff::avformat_free_context(self.0);
            }
        }
    }
}

pub fn write(path: &str, video_spec: &StreamSpec, video: &[EncodedPacket], audio: &[AudioTrack<'_>]) -> bool {
    if video.is_empty() {
        log::info!("[Mp4Muxer] no video packets \u{2014} nothing to save.");
        return false;
    }
    match unsafe { write_inner(path, video_spec, video, audio) } {
        Ok(n_audio) => {
            log::info!("[Mp4Muxer] wrote {} video + {} audio track(s) \u{2192} {}", video.len(), n_audio, path);
            true
        }
        Err(e) => {
            log::info!("[Mp4Muxer] {e}");
            false
        }
    }
}

unsafe fn write_inner(
    path: &str,
    video_spec: &StreamSpec,
    video: &[EncodedPacket],
    audio: &[AudioTrack<'_>],
) -> Result<usize, String> {
    let lower = path.to_ascii_lowercase();
    let fmt_name = if lower.ends_with(".mkv") {
        "matroska"
    } else if lower.ends_with(".mov") {
        "mov"
    } else {
        "mp4"
    };
    let c_fmt = CString::new(fmt_name).unwrap();
    let c_path = CString::new(path).map_err(|_| "path contains NUL".to_string())?;
    let mut out = Output(ptr::null_mut());
    let r = ff::avformat_alloc_output_context2(&mut out.0, ptr::null(), c_fmt.as_ptr(), c_path.as_ptr());
    if r < 0 || out.0.is_null() {
        return Err(format!("alloc ctx: {}", av_err_string(r)));
    }
    let fmt = out.0;
    let video_idx = add_stream(fmt, video_spec)?;
    // mp4 flags the first video track default by itself, matroska does not (settings audit 2026-11-17: MKV
    // clips had the video track `default=0`)
    (**(*fmt).streams.add(video_idx as usize)).disposition = ff::AV_DISPOSITION_DEFAULT as i32;
    let mut audio_streams: Vec<(i32, &[EncodedPacket])> = Vec::new();
    for t in audio {
        if t.packets.is_empty() {
            continue;
        }
        let idx = add_stream(fmt, &t.spec)?;
        set_title(fmt, idx, &t.title);
        let st = *(*fmt).streams.add(idx as usize);
        (*st).disposition = if audio_streams.is_empty() { ff::AV_DISPOSITION_DEFAULT as i32 } else { 0 };
        audio_streams.push((idx, t.packets));
    }
    let r = ff::avio_open(&mut (*fmt).pb, c_path.as_ptr(), ff::AVIO_FLAG_WRITE as i32);
    if r < 0 {
        return Err(format!("avio_open: {}", av_err_string(r)));
    }
    let r = ff::avformat_write_header(fmt, ptr::null_mut());
    if r < 0 {
        return Err(format!("write_header: {}", av_err_string(r)));
    }

    // Merge all streams in non-decreasing pts order (stable: ties keep stream order). Each packet carries
    // an explicit duration (gap to the next packet of its stream; the last repeats the previous gap).
    // C# left duration 0: the mp4 edit list then ended AT the last frame's pts and players discarded that
    // frame (flag D). Real clips hid it because the audio track extends the edit list; a video-only clip
    // lost its last frame.
    let mut all: Vec<(&EncodedPacket, i32, i64)> =
        Vec::with_capacity(video.len() + audio_streams.iter().map(|a| a.1.len()).sum::<usize>());
    let with_durations = |packets: &'_ [EncodedPacket]| -> Vec<i64> {
        let mut d: Vec<i64> = packets.windows(2).map(|w| (w[1].pts_us - w[0].pts_us).max(0)).collect();
        d.push(d.last().copied().unwrap_or(0));
        d
    };
    let vd = with_durations(video);
    all.extend(video.iter().zip(vd).map(|(p, d)| (p, video_idx, d)));
    for (idx, packets) in &audio_streams {
        let ad = with_durations(packets);
        all.extend(packets.iter().zip(ad).map(|(p, d)| (p, *idx, d)));
    }
    all.sort_by_key(|(p, _, _)| p.pts_us);

    let mut pkt = ff::av_packet_alloc();
    if pkt.is_null() {
        return Err("av_packet_alloc failed".into());
    }
    let mut first_err = None;
    for (p, idx, duration) in all {
        if ff::av_new_packet(pkt, p.data.len() as i32) < 0 {
            first_err.get_or_insert_with(|| "av_new_packet failed".to_string());
            break;
        }
        ptr::copy_nonoverlapping(p.data.as_ptr(), (*pkt).data, p.data.len());
        (*pkt).stream_index = idx;
        (*pkt).pts = p.pts_us;
        (*pkt).dts = p.dts_us;
        (*pkt).duration = duration;
        if p.is_keyframe {
            (*pkt).flags |= ff::AV_PKT_FLAG_KEY as i32;
        }
        let st = *(*fmt).streams.add(idx as usize);
        ff::av_packet_rescale_ts(pkt, US, (*st).time_base);
        // Takes ownership of the packet's reference and resets it, so `pkt` is reusable.
        let r = ff::av_interleaved_write_frame(fmt, pkt);
        if r < 0 && first_err.is_none() {
            first_err = Some(format!("write_frame: {}", av_err_string(r)));
        }
    }
    ff::av_packet_free(&mut pkt);
    let r = ff::av_write_trailer(fmt);
    if let Some(e) = first_err {
        return Err(e);
    }
    if r < 0 {
        return Err(format!("write_trailer: {}", av_err_string(r)));
    }
    Ok(audio_streams.len())
}

unsafe fn set_title(fmt: *mut ff::AVFormatContext, idx: i32, title: &str) {
    if title.is_empty() {
        return;
    }
    let st = *(*fmt).streams.add(idx as usize);
    let v = CString::new(title).unwrap_or_default();
    // `title` lands in matroska's track name and mp4's udta `name`; most players (Windows Media Player,
    // VLC, mpv, Premiere) read the mp4 `hdlr` name instead, which FFmpeg writes from `handler_name`
    // (default "SoundHandler"). Settings audit 2026-11-17: the tracks showed as unnamed - set both.
    for key in ["title", "handler_name"] {
        let k = CString::new(key).unwrap();
        ff::av_dict_set(&mut (*st).metadata, k.as_ptr(), v.as_ptr(), 0);
    }
}

unsafe fn add_stream(fmt: *mut ff::AVFormatContext, spec: &StreamSpec) -> Result<i32, String> {
    let st = ff::avformat_new_stream(fmt, ptr::null());
    if st.is_null() {
        return Err("avformat_new_stream failed".into());
    }
    (*st).time_base = US;
    let par = &mut *(*st).codecpar;
    par.codec_id = spec.codec_id;
    if spec.is_video {
        par.codec_type = ff::AVMEDIA_TYPE_VIDEO;
        par.width = spec.width;
        par.height = spec.height;
    } else {
        par.codec_type = ff::AVMEDIA_TYPE_AUDIO;
        par.sample_rate = spec.sample_rate;
        // AAC-LC frames are always 1024 samples; without it the mp4 muxer warns "codec frame size is not
        // set" (C# had the same warning) and has to guess sample durations.
        if spec.codec_id == ff::AV_CODEC_ID_AAC {
            par.frame_size = 1024;
        }
        ff::av_channel_layout_default(&mut par.ch_layout, spec.channels);
    }
    if !spec.extra_data.is_empty() {
        let n = spec.extra_data.len();
        let buf = ff::av_mallocz(n + ff::AV_INPUT_BUFFER_PADDING_SIZE as usize) as *mut u8;
        if buf.is_null() {
            return Err("av_mallocz extradata failed".into());
        }
        ptr::copy_nonoverlapping(spec.extra_data.as_ptr(), buf, n);
        par.extradata = buf;
        par.extradata_size = n as i32;
    }
    Ok((*st).index)
}
