using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using FFmpeg.AutoGen;

namespace Lag.Services.VfrCapture;

/// <summary>Everything the muxer needs to declare one output stream, decoupled from the encoder
/// that produced it. Timestamps on packets are in microseconds (1/1,000,000).</summary>
public sealed record StreamSpec(
    AVCodecID CodecId,
    bool IsVideo,
    int Width, int Height,           // video
    int SampleRate, int Channels,    // audio
    byte[] ExtraData);

/// <summary>
/// Writes buffered video + audio packets to an MP4 with their real, variable timestamps — a true
/// VFR file (not CFR with duplicate frames). Runs on the save (hotkey) thread off the capture path.
/// No <c>+faststart</c>: it forces a second whole-file rewrite that slows every save; local replays
/// don't need it (the player reads the trailing moov fine).
/// </summary>
public static unsafe class Mp4Muxer
{
    // Packets arrive timestamped in microseconds.
    private static readonly AVRational Us = new() { num = 1, den = 1_000_000 };

    /// <summary>One muxed audio track: its stream params, packets (re-based to the clip origin)
    /// and a display title the editor/player shows ("System", "Microphone"…).</summary>
    public sealed record AudioTrack(StreamSpec Spec, IReadOnlyList<EncodedPacket> Packets, string Title);

    /// <summary>
    /// Muxes the clip: one video stream plus zero or more named audio tracks. With two tracks the
    /// editor reads track 0 = system, track 1 = mic (its "separate audio tracks" contract). Returns
    /// true on success. Never throws to the caller.
    /// </summary>
    public static bool Write(string path, StreamSpec videoSpec, IReadOnlyList<EncodedPacket> videoPackets,
                             IReadOnlyList<AudioTrack>? audioTracks)
    {
        if (videoPackets.Count == 0) { Console.WriteLine("[Mp4Muxer] no video packets — nothing to save."); return false; }

        AVFormatContext* fmt = null;
        try
        {
            // Container chosen from the output extension (mp4 / mkv / mov) — all carry H.264/HEVC/AV1
            // + AAC via the same extradata we set per stream.
            string fmtName = path.EndsWith(".mkv", StringComparison.OrdinalIgnoreCase) ? "matroska"
                           : path.EndsWith(".mov", StringComparison.OrdinalIgnoreCase) ? "mov"
                           : "mp4";
            int r = ffmpeg.avformat_alloc_output_context2(&fmt, null, fmtName, path);
            if (r < 0 || fmt == null) { Console.WriteLine($"[Mp4Muxer] alloc ctx: {FfmpegInterop.Err(r)}"); return false; }

            int videoIdx = AddStream(fmt, videoSpec);
            var audioStreams = new List<(int idx, IReadOnlyList<EncodedPacket> packets)>();
            if (audioTracks != null)
                foreach (var t in audioTracks)
                {
                    if (t.Packets.Count == 0) continue;
                    int idx = AddStream(fmt, t.Spec);
                    SetStreamTitle(fmt, idx, t.Title);
                    // First audio track = the default one every player auto-plays (the mixed "All");
                    // clear default on the rest so they stay editor-only side tracks.
                    fmt->streams[idx]->disposition = audioStreams.Count == 0 ? ffmpeg.AV_DISPOSITION_DEFAULT : 0;
                    audioStreams.Add((idx, t.Packets));
                }

            r = ffmpeg.avio_open(&fmt->pb, path, ffmpeg.AVIO_FLAG_WRITE);
            if (r < 0) { Console.WriteLine($"[Mp4Muxer] avio_open: {FfmpegInterop.Err(r)}"); return false; }

            r = ffmpeg.avformat_write_header(fmt, null);
            if (r < 0) { Console.WriteLine($"[Mp4Muxer] write_header: {FfmpegInterop.Err(r)}"); return false; }

            // Merge all streams in non-decreasing pts order so the interleaver stays happy.
            foreach (var (pkt, streamIdx) in MergeAll(videoPackets, videoIdx, audioStreams))
                WritePacket(fmt, streamIdx, pkt);

            ffmpeg.av_write_trailer(fmt);
            Console.WriteLine($"[Mp4Muxer] wrote {videoPackets.Count} video + " +
                              $"{audioStreams.Count} audio track(s) → {path}");
            return true;
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[Mp4Muxer] error: {ex.Message}");
            return false;
        }
        finally
        {
            if (fmt != null)
            {
                if (fmt->pb != null) ffmpeg.avio_closep(&fmt->pb);
                ffmpeg.avformat_free_context(fmt);
            }
        }
    }

    private static void SetStreamTitle(AVFormatContext* fmt, int idx, string title)
    {
        if (string.IsNullOrEmpty(title)) return;
        AVDictionary* d = fmt->streams[idx]->metadata;
        ffmpeg.av_dict_set(&d, "title", title, 0);
        fmt->streams[idx]->metadata = d;
    }

    private static int AddStream(AVFormatContext* fmt, StreamSpec spec)
    {
        AVStream* st = ffmpeg.avformat_new_stream(fmt, null);
        st->time_base = Us;
        var par = st->codecpar;
        par->codec_id = spec.CodecId;
        if (spec.IsVideo)
        {
            par->codec_type = AVMediaType.AVMEDIA_TYPE_VIDEO;
            par->width = spec.Width;
            par->height = spec.Height;
        }
        else
        {
            par->codec_type = AVMediaType.AVMEDIA_TYPE_AUDIO;
            par->sample_rate = spec.SampleRate;
            ffmpeg.av_channel_layout_default(&par->ch_layout, spec.Channels);
        }
        // NOTE: we deliberately leave par->format unset (AV_PIX_FMT_NONE / AV_SAMPLE_FMT_NONE).
        // codecpar->format describes the DECODED sample/pixel format; a muxer fed compressed
        // packets doesn't need it, and this matches what a demuxer produces for a coded stream.
        // (The encoder must supply global-header extradata — avcC/hvcC/av1C — or matroska's
        // write_header rejects the file; see NvencVfrEncoder's AV_CODEC_FLAG_GLOBAL_HEADER.)

        // Codec extradata (SPS/PPS / AV1 config / AudioSpecificConfig) — required for playback.
        if (spec.ExtraData is { Length: > 0 })
        {
            par->extradata = (byte*)ffmpeg.av_malloc((ulong)(spec.ExtraData.Length + ffmpeg.AV_INPUT_BUFFER_PADDING_SIZE));
            Marshal.Copy(spec.ExtraData, 0, (IntPtr)par->extradata, spec.ExtraData.Length);
            par->extradata_size = spec.ExtraData.Length;
        }
        return st->index;
    }

    private static void WritePacket(AVFormatContext* fmt, int streamIndex, EncodedPacket p)
    {
        AVPacket* pkt = ffmpeg.av_packet_alloc();
        try
        {
            ffmpeg.av_new_packet(pkt, p.Data.Length);
            Marshal.Copy(p.Data, 0, (IntPtr)pkt->data, p.Data.Length);
            pkt->stream_index = streamIndex;
            pkt->pts = p.PtsUs;
            pkt->dts = p.DtsUs;
            if (p.IsKeyframe) pkt->flags |= ffmpeg.AV_PKT_FLAG_KEY;
            // Our timestamps are microseconds, but avformat_write_header may have re-assigned the
            // stream a different time_base (mp4 forces AAC audio to 1/sample_rate; video stays
            // 1/1e6). Rescale into the stream's actual time_base so both land at the right times.
            ffmpeg.av_packet_rescale_ts(pkt, Us, fmt->streams[streamIndex]->time_base);
            ffmpeg.av_interleaved_write_frame(fmt, pkt);
        }
        finally { ffmpeg.av_packet_free(&pkt); }
    }

    /// <summary>Merges video + N audio streams into one pts-ascending sequence for interleaving.</summary>
    private static List<(EncodedPacket pkt, int idx)> MergeAll(
        IReadOnlyList<EncodedPacket> video, int videoIdx,
        List<(int idx, IReadOnlyList<EncodedPacket> packets)> audioStreams)
    {
        var all = new List<(EncodedPacket pkt, int idx)>(video.Count);
        foreach (var p in video) all.Add((p, videoIdx));
        foreach (var (idx, packets) in audioStreams)
            foreach (var p in packets) all.Add((p, idx));
        all.Sort((a, b) => a.pkt.PtsUs.CompareTo(b.pkt.PtsUs));
        return all;
    }
}
