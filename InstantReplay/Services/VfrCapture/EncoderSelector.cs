using System;
using System.Collections.Generic;
using System.Threading;
using FFmpeg.AutoGen;

namespace Lag.Services.VfrCapture;

/// <summary>The video codec families we can record in, best-quality-per-bit first.</summary>
public enum CodecTier { Av1, Hevc, H264 }

/// <summary>A concrete encoder the local machine actually supports (probed, not assumed).</summary>
public sealed record EncoderChoice(string FfmpegName, AVCodecID CodecId, CodecTier Tier, string FriendlyName, bool IsHardware);

/// <summary>
/// Hardware-agnostic encoder discovery. The app ships to wildly different machines (every GPU
/// vendor and generation, plus CPU-only boxes), so we never assume a codec is usable — we
/// actually try to OPEN each candidate against the real driver and keep only the ones that
/// succeed. The caller then either takes the best (Auto) or the user's explicit preference.
///
/// Ordering reflects quality-per-bit (AV1 ≈ HEVC &gt; H.264) and falls all the way back to CPU
/// x264 so recording works even with no hardware encoder at all. Only the encoders for the GPU
/// actually present will open, so cross-vendor candidates self-select naturally.
/// </summary>
public static class EncoderSelector
{
    // Candidate encoders in descending preference. Each opens only on hardware that supports it.
    private static readonly (string Name, AVCodecID Id, CodecTier Tier, string Friendly, bool Hw)[] Candidates =
    {
        ("av1_nvenc",  AVCodecID.AV_CODEC_ID_AV1,  CodecTier.Av1,  "AV1 (NVIDIA)",       true),
        ("av1_amf",    AVCodecID.AV_CODEC_ID_AV1,  CodecTier.Av1,  "AV1 (AMD)",          true),
        ("av1_qsv",    AVCodecID.AV_CODEC_ID_AV1,  CodecTier.Av1,  "AV1 (Intel)",        true),
        ("hevc_nvenc", AVCodecID.AV_CODEC_ID_HEVC, CodecTier.Hevc, "HEVC (NVIDIA)",      true),
        ("hevc_amf",   AVCodecID.AV_CODEC_ID_HEVC, CodecTier.Hevc, "HEVC (AMD)",         true),
        ("hevc_qsv",   AVCodecID.AV_CODEC_ID_HEVC, CodecTier.Hevc, "HEVC (Intel)",       true),
        ("h264_nvenc", AVCodecID.AV_CODEC_ID_H264, CodecTier.H264, "H.264 (NVIDIA)",     true),
        ("h264_amf",   AVCodecID.AV_CODEC_ID_H264, CodecTier.H264, "H.264 (AMD)",        true),
        ("h264_qsv",   AVCodecID.AV_CODEC_ID_H264, CodecTier.H264, "H.264 (Intel)",      true),
        ("libx264",    AVCodecID.AV_CODEC_ID_H264, CodecTier.H264, "H.264 (CPU / x264)", false),
    };

    /// <summary>
    /// Returns every encoder that opens successfully on this machine, best first. Result is
    /// cached after the first probe (opening encoders is not free and the hardware doesn't
    /// change mid-session). Never throws.
    /// </summary>
    public static IReadOnlyList<EncoderChoice> Available => _available ??= ProbeMta();
    private static IReadOnlyList<EncoderChoice>? _available;

    /// <summary>
    /// Runs the probe on a dedicated MTA thread. Intel QSV / MFX session init fails with ENOMEM when
    /// called from an STA thread (the app's UI thread, where this is first triggered), but succeeds
    /// in a multithreaded apartment — the same constraint the per-app audio capture hit. NVENC/AMF/
    /// x264 are apartment-agnostic, so probing them here too is harmless. Result is plain data.
    /// </summary>
    private static IReadOnlyList<EncoderChoice> ProbeMta()
    {
        IReadOnlyList<EncoderChoice> result = Array.Empty<EncoderChoice>();
        var t = new Thread(() => result = Probe()) { IsBackground = true, Name = "EncoderProbe" };
        try { t.SetApartmentState(ApartmentState.MTA); } catch { /* already set / unsupported */ }
        t.Start();
        t.Join();
        return result;
    }

    /// <summary>
    /// Picks the encoder to use. <paramref name="preference"/> is the user's Settings choice:
    /// "" / "auto" → best available; "av1"/"hevc"/"h264" → best of that family, falling back to
    /// the next-best family if the requested one isn't supported here. Returns null only if not
    /// even libx264 opened (extremely unlikely) — caller then falls back to the OBS engine.
    /// </summary>
    public static EncoderChoice? Select(string? preference)
    {
        var avail = Available;
        if (avail.Count == 0) return null;

        string pref = (preference ?? "").Trim().ToLowerInvariant();

        // Explicit CPU x264: pick libx264 by name (forces the system-memory path).
        if (pref is "x264" or "libx264" or "cpu")
        {
            foreach (var c in avail) if (c.FfmpegName == "libx264") return c;
        }

        // Explicit vendor/backend (the UI codec dropdown: NVENC / AMF / QSV). Pick that vendor's
        // encoder, preferring its H.264 (fastest, rock-solid CBR); fall back to its best codec, then
        // to auto. A vendor that doesn't match the capture GPU runs the system-memory path in
        // NvencVfrEncoder — universal, never garbage.
        string? vendor = pref switch { "nvenc" => "nvenc", "amf" => "amf", "qsv" => "qsv", _ => null };
        if (vendor != null)
        {
            foreach (var c in avail) if (c.FfmpegName.Contains(vendor) && c.Tier == CodecTier.H264) return c;
            foreach (var c in avail) if (c.FfmpegName.Contains(vendor)) return c;
            Console.WriteLine($"[EncoderSelector] backend '{vendor}' unavailable here; using best available.");
            return avail[0];
        }

        CodecTier? wanted = pref switch
        {
            "av1" => CodecTier.Av1,
            "hevc" or "h265" => CodecTier.Hevc,
            "h264" or "avc" => CodecTier.H264,
            _ => null, // auto
        };

        if (wanted is { } tier)
        {
            foreach (var c in avail)
                if (c.Tier == tier) return c;
            // Requested family unsupported here — fall through to best available, but log why.
            Console.WriteLine($"[EncoderSelector] Preferred codec '{pref}' unavailable on this system; using best available.");
            return avail[0];
        }

        // AUTO: prefer hardware H.264 — fastest encode (max fps) and rock-solid CBR. AV1/HEVC are
        // more efficient but heavier, and av1_nvenc in these libs needs the fps-hint to fill rate;
        // they stay available when the user picks them explicitly.
        foreach (var c in avail)
            if (c.Tier == CodecTier.H264 && c.IsHardware) return c;

        return avail[0]; // no hardware H.264 — take the best that opened
    }

    private static unsafe IReadOnlyList<EncoderChoice> Probe()
    {
        var found = new List<EncoderChoice>();
        if (!FfmpegInterop.Available) return found;

        foreach (var cand in Candidates)
        {
            try
            {
                if (CanOpen(cand.Name))
                {
                    found.Add(new EncoderChoice(cand.Name, cand.Id, cand.Tier, cand.Friendly, cand.Hw));
                    Console.WriteLine($"[EncoderSelector] Usable: {cand.Friendly} ({cand.Name})");
                }
            }
            catch (Exception ex)
            {
                // A driver that crashes on probe must never take the app down — just skip it.
                Console.WriteLine($"[EncoderSelector] {cand.Name} probe threw, skipping: {ex.Message}");
            }
        }

        if (found.Count == 0)
            Console.WriteLine("[EncoderSelector] No usable video encoder found (not even libx264).");
        return found;
    }

    /// <summary>
    /// Truly tests an encoder by allocating a context with a minimal valid config and calling
    /// avcodec_open2. For hardware encoders this succeeds only when the GPU + driver support the
    /// codec — exactly the "does it work HERE" answer we need. QSV self-initialises its MFX session
    /// straight from the NV12 system input (no explicit hw_device_ctx — creating one ourselves
    /// actually FAILED on Intel iGPUs).
    /// </summary>
    private static unsafe bool CanOpen(string name)
    {
        AVCodec* codec = ffmpeg.avcodec_find_encoder_by_name(name);
        if (codec == null) return false;

        AVCodecContext* ctx = ffmpeg.avcodec_alloc_context3(codec);
        if (ctx == null) return false;

        bool ok = false;
        try
        {
            ctx->width = 1280;
            ctx->height = 720;
            ctx->time_base = new AVRational { num = 1, den = 1000 };
            ctx->framerate = new AVRational { num = 60, den = 1 };
            ctx->pix_fmt = AVPixelFormat.AV_PIX_FMT_NV12;
            ctx->bit_rate = 4_000_000;
            ctx->gop_size = 120;

            int r = ffmpeg.avcodec_open2(ctx, codec, null);
            ok = r >= 0;
        }
        finally
        {
            ffmpeg.avcodec_free_context(&ctx);
        }
        return ok;
    }
}
