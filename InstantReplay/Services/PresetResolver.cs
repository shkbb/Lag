using System;
using System.Linq;

namespace Lag.Services;

/// <summary>What the user OPTIMISES FOR. The resolver maps this intent onto the machine's real
/// capabilities — the user picks a goal, not numbers.</summary>
public enum RecordingIntent
{
    /// <summary>Smoothest gameplay: high fps, lighter codec, modest resolution. fps over fidelity.</summary>
    Performance,
    /// <summary>The recommended middle: solid fps AND quality for this machine.</summary>
    Balanced,
    /// <summary>Sharpest clips: native-ish resolution, efficient codec, fat bitrate. Fidelity over fps.</summary>
    Quality,
}

/// <summary>Coarse capability class derived from the GPU (and the encode path). Drives the resolution,
/// fps and codec ceilings; RAM/disk/CPU then refine the rest.</summary>
public enum GpuTier { Entry = 0, Mid = 1, High = 2, Top = 3 }

/// <summary>Concrete recording settings the resolver recommends — primitives that map onto the
/// existing Settings dropdowns (TargetHeight/Fps/CodecId/BitrateKbps/BufferSeconds). The cleanup
/// fields are a RECOMMENDATION ONLY and must never be auto-applied (auto-cleanup deletes clips).</summary>
public sealed record RecommendedSettings(
    int TargetHeight,        // 0 = native; else 720/1080/1440/2160 (already capped to the monitor)
    int Fps,                 // capped to the monitor refresh
    string CodecId,          // "" = Auto, else "h264"/"hevc"/"av1" (only ones this box can encode)
    int BitrateKbps,         // -1 = Auto (constant quality); else a fixed kbps
    int BufferSeconds,       // snapped to an available buffer rung, fitted to the RAM budget
    bool RecommendCleanup,   // HINT ONLY — never auto-enabled
    int SuggestedStorageGb)  // for the hint, when RecommendCleanup is true
{
    private string ResText => TargetHeight == 0 ? "native" : $"{TargetHeight}p";
    private string CodecText => string.IsNullOrEmpty(CodecId) ? "Auto codec" : CodecId.ToUpperInvariant();
    private string RateText => BitrateKbps < 0 ? "Auto quality" : $"{BitrateKbps / 1000} Mbps";
    private string BufText => BufferSeconds < 60 ? $"{BufferSeconds}s" : $"{BufferSeconds / 60}min";

    /// <summary>One-line summary for a preset card subtitle / the log.</summary>
    public string Summary() => $"{ResText} · {Fps}fps · {CodecText} · {RateText} · {BufText} buffer";
}

/// <summary>
/// Maps <see cref="RecordingIntent"/> × <see cref="MachineProfile"/> → concrete <see cref="RecommendedSettings"/>.
/// The goal is presets that REALLY fit the box: a 4070 records native 1440p AV1, an Iris Xe records
/// 1080p H.264 — same intent, hardware-appropriate result. Pure/stateless; only reads the profile.
/// </summary>
public static class PresetResolver
{
    public static RecommendedSettings Resolve(RecordingIntent intent, MachineProfile p)
    {
        var tier = ClassifyGpu(p);
        // No hardware encoder at all → everything rides CPU x264; protect a weak CPU by capping hard.
        bool cpuOnly = p.HardwareEncoders.Count == 0;

        int monH = p.MonitorHeight > 0 ? p.MonitorHeight : 1080;
        int refresh = p.MonitorHz > 0 ? p.MonitorHz : 60;

        // ── Resolution (target encode height; 0 = native) ──
        int height = intent switch
        {
            RecordingIntent.Performance => tier >= GpuTier.Top ? 1440 : 1080,
            RecordingIntent.Balanced    => tier >= GpuTier.Top ? 0 : tier >= GpuTier.High ? 1440 : 1080,
            _ /* Quality */             => tier >= GpuTier.High ? 0 : tier == GpuTier.Mid ? 1440 : 1080,
        };
        if (cpuOnly) height = p.CpuLogicalCores >= 8 ? 1080 : 720;   // software x264: protect a weak CPU
        int targetHeight = SnapResolution(height, monH);

        // ── FPS (capped to the panel) ──
        int fps = intent switch
        {
            RecordingIntent.Performance => tier >= GpuTier.Mid ? 120 : 60,
            RecordingIntent.Balanced    => tier >= GpuTier.High ? 120 : 60,
            _ /* Quality */             => tier >= GpuTier.Top ? 120 : 60,
        };
        if (cpuOnly) fps = 60;
        fps = Math.Min(fps, refresh);

        // ── Codec (only what actually probed on this box) ──
        string codec = intent switch
        {
            // Performance: hardware H.264 — fastest encode (max fps) + rock-solid CBR.
            RecordingIntent.Performance => p.HasH264 ? "h264" : "",
            // Balanced: let the engine auto-pick the best available (it already prefers fast H.264).
            RecordingIntent.Balanced => "",
            // Quality: most efficient available — AV1 > HEVC > H.264 (smaller file at the same fidelity).
            _ => p.HasAv1 ? "av1" : p.HasHevc ? "hevc" : p.HasH264 ? "h264" : "",
        };

        // ── Bitrate ──
        // Performance/Balanced ride Auto (constant-quality RC) — great quality-per-bit, scales with the
        // scene, smaller files. Quality pins a fat fixed bitrate scaled by resolution so motion-heavy
        // scenes never starve — the visible "max fidelity" difference. Clamped to the RAM budget below.
        int bitrateKbps = intent == RecordingIntent.Quality ? QualityBitrateKbps(targetHeight, monH) : -1;

        // ── RAM budget → buffer (and a bitrate clamp for the fixed-rate Quality case) ──
        // The rolling buffer holds the COMPRESSED stream in RAM: bytes ≈ bitrate/8 × seconds. Keep it to
        // a safe slice of total RAM so a long buffer never eats the machine.
        long ramBytes = (long)(p.RamGiB * 1024 * 1024 * 1024);
        long ringBudget = (long)Math.Clamp(ramBytes * 0.15, 250L * 1024 * 1024, 1500L * 1024 * 1024);
        int effectiveKbps = bitrateKbps > 0 ? bitrateKbps : AutoEstimateKbps(targetHeight, monH);

        // If even a short buffer wouldn't fit the fixed Quality bitrate on a low-RAM box, trim the bitrate.
        if (bitrateKbps > 0)
        {
            long need30s = (long)bitrateKbps * 1000 / 8 * 30;
            if (need30s > ringBudget)
            {
                bitrateKbps = (int)(ringBudget * 8 / 30 / 1000);
                bitrateKbps = SnapBitrate(bitrateKbps);
                effectiveKbps = bitrateKbps;
            }
        }

        int bufferTarget = intent switch
        {
            RecordingIntent.Performance => 60,
            RecordingIntent.Balanced => 120,
            _ => 180,
        };
        long maxBufSec = ringBudget * 8 / Math.Max(1, (long)effectiveKbps * 1000);
        int bufferSeconds = SnapBuffer((int)Math.Min(bufferTarget, Math.Max(30, maxBufSec)));

        // ── Disk → RECOMMEND (never set) auto-cleanup ──
        bool recommendCleanup = p.DiskFreeGiB > 0 && p.DiskFreeGiB < 40;
        int suggestedGb = recommendCleanup ? SnapStorage((int)(p.DiskFreeGiB * 0.6)) : 0;

        return new RecommendedSettings(targetHeight, fps, codec, bitrateKbps, bufferSeconds,
                                       recommendCleanup, suggestedGb);
    }

    /// <summary>Classifies the primary GPU into a recording tier. iGPU = Entry. Discrete GPUs use
    /// VRAM as a prior, then a family/generation name parse corrects it (raw VRAM misleads — an 8 GB
    /// RX 580 from 2017 is Mid, not High).</summary>
    public static GpuTier ClassifyGpu(MachineProfile p)
    {
        var gpu = p.PrimaryGpu;
        if (gpu == null || gpu.IsIntegrated) return GpuTier.Entry;

        double vram = gpu.VramGiB;
        var tier = vram >= 12 ? GpuTier.Top : vram >= 8 ? GpuTier.High : vram >= 6 ? GpuTier.Mid
                 : vram >= 4 ? GpuTier.Mid : GpuTier.Entry;

        string n = gpu.Name.ToUpperInvariant();
        int nv = NvidiaClass(n);   // e.g. 4070 -> 70 ; 0 if not NVIDIA / unknown

        // NVIDIA modern (RTX 40/50): xx-class refines. Even an xx60 is comfortably High for recording.
        if (n.Contains("RTX 50") || n.Contains("RTX 40"))
            tier = nv >= 80 ? GpuTier.Top : nv >= 70 ? GpuTier.Top : nv >= 60 ? GpuTier.High : GpuTier.High;
        else if (n.Contains("RTX 30"))
            tier = nv >= 80 ? GpuTier.Top : nv >= 70 ? GpuTier.High : GpuTier.High;
        else if (n.Contains("RTX 20") || n.Contains("GTX 16") || n.Contains("GTX 10"))
            tier = Min(tier, GpuTier.Mid);                       // Turing/Pascal era → Mid at best
        // AMD
        else if (n.Contains("RX 9") || n.Contains("RX 7"))
            tier = (n.Contains("900") || n.Contains("800")) ? GpuTier.Top : GpuTier.High;
        else if (n.Contains("RX 6"))
            tier = (n.Contains("6900") || n.Contains("6800")) ? GpuTier.Top : (n.Contains("6700")) ? GpuTier.High : GpuTier.Mid;
        else if (n.Contains("RX 5") || n.Contains("RX 4") || n.Contains("RX 580") || n.Contains("RX 570"))
            tier = Min(tier, GpuTier.Mid);                       // Polaris/old → Mid even at 8 GB
        // Intel Arc discrete (has real VRAM) — keep the VRAM-based tier, AV1 capable.

        return tier;
    }

    /// <summary>Extracts the NVIDIA model class (e.g. "RTX 4070 Ti" → 70, "RTX 5060" → 60). 0 when
    /// not parseable. The class is the last two digits of the 4-digit model number.</summary>
    private static int NvidiaClass(string upperName)
    {
        foreach (var token in upperName.Split(' ', '(', ')'))
        {
            if (token.Length == 4 && token.All(char.IsDigit) && int.TryParse(token, out int m) && m >= 1000)
                return m % 100;   // 4070 -> 70, 5060 -> 60, 3090 -> 90
        }
        return 0;
    }

    private static GpuTier Min(GpuTier a, GpuTier b) => (GpuTier)Math.Min((int)a, (int)b);

    // Fixed Quality bitrate by output height (clamped to the monitor). Fat enough that motion never starves.
    private static int QualityBitrateKbps(int targetHeight, int monH)
    {
        int h = targetHeight == 0 ? monH : targetHeight;
        return h >= 2160 ? 100000 : h >= 1440 ? 50000 : h >= 1080 ? 30000 : 15000;
    }

    // Rough effective bitrate for Auto, used ONLY for the RAM/buffer budget math.
    private static int AutoEstimateKbps(int targetHeight, int monH)
    {
        int h = targetHeight == 0 ? monH : targetHeight;
        return h >= 2160 ? 70000 : h >= 1440 ? 38000 : h >= 1080 ? 20000 : 9000;
    }

    private static int SnapResolution(int wantedHeight, int monH)
    {
        if (wantedHeight <= 0 || wantedHeight >= monH) return 0; // native
        foreach (int rung in new[] { 2160, 1440, 1080, 720 })
            if (rung <= monH && wantedHeight >= rung) return rung;
        return 720;
    }

    private static readonly int[] BufferRungs = { 30, 60, 120, 300, 600, 900 };
    private static int SnapBuffer(int seconds) => Nearest(seconds, BufferRungs);

    private static readonly int[] BitrateRungs = { 3000, 5000, 7000, 10000, 15000, 20000, 25000, 30000, 50000, 70000, 100000 };
    private static int SnapBitrate(int kbps) => Nearest(kbps, BitrateRungs);

    private static readonly int[] StorageRungs = { 10, 25, 50, 100, 200, 500 };
    private static int SnapStorage(int gb) => Nearest(gb, StorageRungs);

    private static int Nearest(int value, int[] rungs)
    {
        int best = rungs[0]; long bestDiff = long.MaxValue;
        foreach (int r in rungs)
        {
            long d = Math.Abs((long)r - value);
            if (d < bestDiff) { bestDiff = d; best = r; }
        }
        return best;
    }

    /// <summary>Logs what each preset would recommend for this machine — for calibrating against
    /// real hardware before the UI is wired.</summary>
    public static void LogRecommendations(MachineProfile p)
    {
        try
        {
            Console.WriteLine($"===== Preset recommendations (GPU tier: {ClassifyGpu(p)}) =====");
            foreach (RecordingIntent intent in Enum.GetValues<RecordingIntent>())
            {
                var r = Resolve(intent, p);
                string hint = r.RecommendCleanup ? $"  [hint: suggest auto-cleanup ~{r.SuggestedStorageGb}GB]" : "";
                Console.WriteLine($"  {intent,-12}: {r.Summary()}{hint}");
            }
            Console.WriteLine("==========================================");
        }
        catch (Exception ex) { Console.WriteLine($"[PresetResolver] log failed: {ex.Message}"); }
    }
}
