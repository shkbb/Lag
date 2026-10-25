using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Text;
using FFmpeg.AutoGen;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Joins the tail of a finished capture session (see <see cref="CarryOverSegment"/>) to the live
/// buffer so one clip spans both — the footage from just before you quit a game plus what came
/// after, saved as a single file.
///
/// Two paths:
///   • <b>Direct</b> — the sessions produced identical stream parameters (same codec, size and
///     extradata, i.e. the usual fullscreen game → desktop switch on one monitor). The packets are
///     simply concatenated with a time offset. Instant, lossless, no re-encode.
///   • <b>Transcode</b> — the sizes differ (a windowed game). A video track cannot change resolution
///     mid-stream, so both halves are muxed to temp files and re-encoded into one via the bundled
///     FFmpeg, the tail scaled and letterboxed into the live frame. Costs a few seconds.
/// </summary>
internal static class ReplayStitcher
{
    /// <summary>Gap inserted between the two halves so timestamps stay strictly increasing across
    /// the seam. One frame at 60fps — invisible, and it collapses the real dead time spent tearing
    /// down one capture and starting the next (which would otherwise freeze on the last game frame).</summary>
    public const long SeamGapUs = 16_000;

    /// <summary>True when both halves can live in one track untouched: same codec, same size and
    /// identical extradata. Extradata must match byte-for-byte — it carries the SPS/PPS the decoder
    /// applies to the WHOLE track, so a mismatch would corrupt the second half.</summary>
    public static bool CanConcatDirect(StreamSpec a, StreamSpec b)
        => a.CodecId == b.CodecId && a.Width == b.Width && a.Height == b.Height
           && BytesEqual(a.ExtraData, b.ExtraData);

    /// <summary>Audio halves can be concatenated when the decoder config is identical.</summary>
    public static bool CanConcatAudio(StreamSpec a, StreamSpec b)
        => a.CodecId == b.CodecId && a.SampleRate == b.SampleRate && a.Channels == b.Channels
           && BytesEqual(a.ExtraData, b.ExtraData);

    private static bool BytesEqual(byte[]? a, byte[]? b)
    {
        if (ReferenceEquals(a, b)) return true;
        if (a == null || b == null) return a == null && b == null;
        if (a.Length != b.Length) return false;
        for (int i = 0; i < a.Length; i++) if (a[i] != b[i]) return false;
        return true;
    }

    /// <summary>Shifts a packet list later by <paramref name="offsetUs"/>.</summary>
    public static List<EncodedPacket> Shift(IReadOnlyList<EncodedPacket> packets, long offsetUs)
    {
        var list = new List<EncodedPacket>(packets.Count);
        foreach (var p in packets) list.Add(p with { PtsUs = p.PtsUs + offsetUs, DtsUs = p.DtsUs + offsetUs });
        return list;
    }

    /// <summary>
    /// Re-encodes head + live into ONE file when their frame sizes differ. Both halves are muxed to
    /// temp files first (they are already valid clips), then a single FFmpeg pass scales the head
    /// into the live frame and concatenates. Returns false if FFmpeg is unavailable or fails — the
    /// caller then falls back to writing the two halves side by side rather than losing the footage.
    /// </summary>
    public static bool TranscodeStitch(string outPath,
                                       StreamSpec headSpec, IReadOnlyList<EncodedPacket> headVideo,
                                       IReadOnlyList<Mp4Muxer.AudioTrack> headAudio,
                                       StreamSpec liveSpec, IReadOnlyList<EncodedPacket> liveVideo,
                                       IReadOnlyList<Mp4Muxer.AudioTrack> liveAudio,
                                       int bitrateKbps, int preset = 4)
    {
        string ffmpeg = Path.Combine(AppContext.BaseDirectory, "ffmpeg", "ffmpeg.exe");
        if (!File.Exists(ffmpeg))
        {
            Console.WriteLine($"[Stitch] bundled ffmpeg.exe missing ({ffmpeg}) — cannot re-encode.");
            return false;
        }

        string tmpDir = Path.Combine(Path.GetTempPath(), "Lag-stitch-" + Guid.NewGuid().ToString("N").Substring(0, 8));
        Directory.CreateDirectory(tmpDir);
        string headTmp = Path.Combine(tmpDir, "head.mp4");
        string liveTmp = Path.Combine(tmpDir, "live.mp4");
        try
        {
            // Only the tracks present in BOTH halves can survive the concat filter (it needs the same
            // stream count per segment), so trim to the common count — the mixed/default track first.
            int aCount = Math.Min(headAudio.Count, liveAudio.Count);
            var headA = Take(headAudio, aCount);
            var liveA = Take(liveAudio, aCount);
            if (headAudio.Count != liveAudio.Count)
                Console.WriteLine($"[Stitch] audio track count differs ({headAudio.Count} vs {liveAudio.Count}) — keeping {aCount}.");

            if (!Mp4Muxer.Write(headTmp, headSpec, headVideo, headA)) return false;
            if (!Mp4Muxer.Write(liveTmp, liveSpec, liveVideo, liveA)) return false;

            int w = liveSpec.Width, h = liveSpec.Height;
            var fc = new StringBuilder();
            // Fit the head into the live frame, preserving aspect (letterbox rather than distort),
            // and normalise SAR/pixel format so concat accepts both inputs.
            fc.Append($"[0:v]scale={w}:{h}:force_original_aspect_ratio=decrease,")
              .Append($"pad={w}:{h}:(ow-iw)/2:(oh-ih)/2,setsar=1,format=yuv420p[v0];")
              .Append($"[1:v]scale={w}:{h},setsar=1,format=yuv420p[v1];");
            fc.Append("[v0]");
            for (int i = 0; i < aCount; i++) fc.Append($"[0:a:{i}]");
            fc.Append("[v1]");
            for (int i = 0; i < aCount; i++) fc.Append($"[1:a:{i}]");
            fc.Append($"concat=n=2:v=1:a={aCount}[v]");
            for (int i = 0; i < aCount; i++) fc.Append($"[a{i}]");

            var args = new StringBuilder();
            args.Append($"-y -hide_banner -loglevel error -i \"{headTmp}\" -i \"{liveTmp}\" ");
            args.Append($"-filter_complex \"{fc}\" -map \"[v]\" ");
            for (int i = 0; i < aCount; i++) args.Append($"-map \"[a{i}]\" ");
            args.Append($"-c:v {VideoEncoderFor(liveSpec.CodecId)} -preset p{Math.Clamp(preset, 1, 7)} ")
                .Append($"-b:v {Math.Max(1000, bitrateKbps)}k ");
            // The first track is the "All" mix: keep it above the others, because Windows plays the
            // audio track with the highest bitrate rather than the one flagged default (see
            // VfrReplayEngine.AllTrackKbps).
            if (aCount > 0)
                args.Append($"-c:a aac -b:a {VfrReplayEngine.SourceTrackKbps}k -b:a:0 {VfrReplayEngine.AllTrackKbps}k ");
            for (int i = 0; i < aCount; i++)
            {
                string title = Escape(liveA[i].Title);
                if (!string.IsNullOrEmpty(title)) args.Append($"-metadata:s:a:{i} title=\"{title}\" ");
            }
            args.Append($"\"{outPath}\"");

            if (Run(ffmpeg, args.ToString())) return true;

            // Hardware encoders fail on some drivers / session limits — retry once on the CPU so the
            // save still succeeds, just slower.
            Console.WriteLine("[Stitch] hardware re-encode failed — retrying with libx264.");
            string cpuArgs = args.ToString()
                .Replace($"-c:v {VideoEncoderFor(liveSpec.CodecId)} -preset p{Math.Clamp(preset, 1, 7)} ",
                         "-c:v libx264 -preset veryfast ");
            return Run(ffmpeg, cpuArgs);
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[Stitch] transcode error: {ex.Message}");
            return false;
        }
        finally
        {
            try { Directory.Delete(tmpDir, recursive: true); } catch { }
        }
    }

    private static IReadOnlyList<Mp4Muxer.AudioTrack> Take(IReadOnlyList<Mp4Muxer.AudioTrack> src, int n)
    {
        var list = new List<Mp4Muxer.AudioTrack>(n);
        for (int i = 0; i < n; i++) list.Add(src[i]);
        return list;
    }

    private static string Escape(string s) => (s ?? "").Replace("\"", "");

    /// <summary>Hardware encoder matching the live stream's codec (the output keeps that codec).</summary>
    private static string VideoEncoderFor(AVCodecID codec) => codec switch
    {
        AVCodecID.AV_CODEC_ID_AV1  => "av1_nvenc",
        AVCodecID.AV_CODEC_ID_HEVC => "hevc_nvenc",
        _                          => "h264_nvenc",
    };

    private static bool Run(string exe, string args)
    {
        var psi = new ProcessStartInfo
        {
            FileName = exe,
            Arguments = args,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardError = true,
            RedirectStandardOutput = true,
        };
        var sw = Stopwatch.StartNew();
        using var proc = Process.Start(psi);
        if (proc == null) { Console.WriteLine("[Stitch] failed to start ffmpeg."); return false; }
        string err = proc.StandardError.ReadToEnd();
        proc.WaitForExit();
        if (proc.ExitCode != 0)
        {
            Console.WriteLine($"[Stitch] ffmpeg exit {proc.ExitCode}: {Tail(err)}");
            return false;
        }
        Console.WriteLine($"[Stitch] re-encoded and joined in {sw.ElapsedMilliseconds}ms.");
        return true;
    }

    private static string Tail(string s)
    {
        s = (s ?? "").Trim();
        return s.Length <= 400 ? s : s.Substring(s.Length - 400);
    }
}
