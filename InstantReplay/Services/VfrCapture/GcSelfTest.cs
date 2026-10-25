using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.Linq;
using System.Runtime;
using System.Threading;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Headless GC stress check of the replay buffer (LAG_GCTEST=1). Drives the REAL
/// <see cref="PacketRingBuffer"/> with the same allocation pattern capture produces — one byte[] per
/// encoded packet, allocated on the capture thread — and measures how long each "packet arrives"
/// step takes, so a GC pause that would stall capture shows up as a latency spike with a cause.
///
/// Feed it a real clip's packet sizes for a faithful workload:
///   ffprobe -select_streams v:0 -show_entries packet=size,flags -of csv=p=0 clip.mp4 > sizes.csv
///   LAG_GCTEST=1 LAG_GCTEST_PACKETS=sizes.csv Lag.exe
/// Without it, a synthetic distribution measured from a real CS2 clip is used.
///
/// Mirrors the engine's capture conditions: SustainedLowLatency GC mode, a 120s buffer, three audio
/// tracks, and a periodic save (Snapshot), which allocates a record per buffered packet.
/// </summary>
public static class GcSelfTest
{
    private const int Fps = 165;
    private const double BufferSeconds = 120;
    private const double SimSeconds = 240;      // fill the buffer, then as long again at steady state
    private const double Speed = 2.0;           // real time runs at 2x — same heap, twice the churn
    private const int AudioTracks = 3;
    private const double AudioPacketsPerSec = 47; // AAC 1024 samples @ 48kHz
    private const int AudioPacketBytes = 434;

    public static void Run()
    {
        var seq = Load(Environment.GetEnvironmentVariable("LAG_GCTEST_PACKETS")) ?? Synthetic();
        Console.WriteLine($"[GcTest] {seq.Count} packet sizes loaded; simulating {SimSeconds}s at {Fps}fps " +
                          $"({Speed}x speed), buffer {BufferSeconds}s, {AudioTracks} audio tracks.");

        // LAG_GCTEST_MODE=interactive runs the same workload under the default GC mode, to measure what
        // the engine's SustainedLowLatency setting is actually buying.
        bool interactive = string.Equals(Environment.GetEnvironmentVariable("LAG_GCTEST_MODE"), "interactive",
                                         StringComparison.OrdinalIgnoreCase);
        var oldMode = GCSettings.LatencyMode;
        GCSettings.LatencyMode = interactive ? GCLatencyMode.Interactive
                                             : GCLatencyMode.SustainedLowLatency;   // what the engine sets while capturing
        Console.WriteLine($"[GcTest] GC latency mode: {GCSettings.LatencyMode}");
        try { Measure(seq); }
        finally { GCSettings.LatencyMode = oldMode; }
    }

    private static void Measure(List<(int Size, bool Key)> seq)
    {
        var video = new PacketRingBuffer(BufferSeconds);
        var audio = Enumerable.Range(0, AudioTracks).Select(_ => new PacketRingBuffer(BufferSeconds)).ToArray();
        var source = new byte[1 << 20];           // stands in for the AVPacket the encoder copies out of

        int total = (int)(SimSeconds * Fps);
        var lat = new double[total];
        var gcDuringSpike = new List<(int Frame, double Ms, bool Gc)>();

        int g0 = GC.CollectionCount(0), g1 = GC.CollectionCount(1), g2 = GC.CollectionCount(2);
        TimeSpan pause0 = GC.GetTotalPauseDuration();
        double audioAcc = 0;
        var wall = Stopwatch.StartNew();
        double freq = Stopwatch.Frequency;

        for (int i = 0; i < total; i++)
        {
            var (size, key) = seq[i % seq.Count];
            long pts = i * 1_000_000L / Fps;

            // ── the step capture actually performs per encoded frame ──
            TimeSpan p0 = GC.GetTotalPauseDuration();
            long t0 = Stopwatch.GetTimestamp();
            var data = new byte[size];
            Buffer.BlockCopy(source, 0, data, 0, Math.Min(size, source.Length));
            video.Add(new EncodedPacket(data, pts, pts, key));
            long t1 = Stopwatch.GetTimestamp();
            TimeSpan p1 = GC.GetTotalPauseDuration();

            double ms = (t1 - t0) * 1000.0 / freq;
            lat[i] = ms;
            if (ms > 5) gcDuringSpike.Add((i, ms, p1 > p0));

            // audio packets arrive on their own cadence
            audioAcc += AudioPacketsPerSec / Fps;
            while (audioAcc >= 1)
            {
                audioAcc -= 1;
                foreach (var a in audio) a.Add(new EncodedPacket(new byte[AudioPacketBytes], pts, pts, true));
            }

            // a save every 30 simulated seconds (Snapshot allocates a record per buffered packet)
            if (i > 0 && i % (Fps * 30) == 0) _ = video.Snapshot(out _);

            // pacing
            double targetMs = (i + 1) * 1000.0 / Fps / Speed;
            int wait = (int)(targetMs - wall.Elapsed.TotalMilliseconds);
            if (wait > 0) Thread.Sleep(wait);
        }

        TimeSpan pauseTotal = GC.GetTotalPauseDuration() - pause0;
        var info = GC.GetGCMemoryInfo();
        var sorted = lat.OrderBy(x => x).ToArray();
        double P(double q) => sorted[Math.Min(sorted.Length - 1, (int)(sorted.Length * q))];

        Console.WriteLine();
        Console.WriteLine($"[GcTest] ran {wall.Elapsed.TotalSeconds:F0}s real for {SimSeconds}s simulated, {total} video packets");
        Console.WriteLine($"[GcTest] per-packet cost on the capture thread (alloc + copy + buffer add):");
        Console.WriteLine($"           p50 {P(.50):F3}ms | p99 {P(.99):F3}ms | p99.9 {P(.999):F2}ms | max {sorted[^1]:F2}ms");
        Console.WriteLine($"           spikes >5ms: {lat.Count(x => x > 5)} | >16ms: {lat.Count(x => x > 16)} | >30ms: {lat.Count(x => x > 30)}");
        Console.WriteLine($"[GcTest] GC: gen0 {GC.CollectionCount(0) - g0} | gen1 {GC.CollectionCount(1) - g1} | gen2 {GC.CollectionCount(2) - g2} " +
                          $"| total pause {pauseTotal.TotalMilliseconds:F0}ms ({pauseTotal.TotalMilliseconds / (SimSeconds / Speed * 10):F2}% of runtime)");
        Console.WriteLine($"[GcTest] heap: {GC.GetTotalMemory(false) / 1048576} MB live, " +
                          $"{info.FragmentedBytes / 1048576} MB fragmented, promoted last GC {info.PromotedBytes / 1048576} MB");

        var worst = gcDuringSpike.OrderByDescending(s => s.Ms).Take(8).ToList();
        if (worst.Count > 0)
        {
            Console.WriteLine("[GcTest] worst spikes (was a GC pause inside them?):");
            foreach (var s in worst)
                Console.WriteLine($"           {s.Ms,7:F2}ms @ {s.Frame / (double)Fps,6:F1}s sim  -> {(s.Gc ? "GC PAUSE" : "not GC")}");
        }
        int gcSpikes = gcDuringSpike.Count(s => s.Gc);
        Console.WriteLine($"[GcTest] spikes >5ms caused by GC: {gcSpikes} of {gcDuringSpike.Count}");
    }

    private static List<(int, bool)>? Load(string? path)
    {
        if (string.IsNullOrEmpty(path) || !File.Exists(path)) return null;
        var list = new List<(int, bool)>();
        foreach (var line in File.ReadLines(path))
        {
            var parts = line.Split(',');
            if (parts.Length == 0 || !int.TryParse(parts[0], out int size) || size <= 0) continue;
            list.Add((size, parts.Length > 1 && parts[1].Contains('K')));
        }
        return list.Count > 0 ? list : null;
    }

    /// <summary>Distribution measured from a real CS2 capture: P-frames ~12KB median with a heavy
    /// tail, a ~520KB keyframe every 2s.</summary>
    private static List<(int, bool)> Synthetic()
    {
        var rng = new Random(7);
        var list = new List<(int, bool)>();
        for (int i = 0; i < Fps * 60; i++)
        {
            bool key = i % (Fps * 2) == 0;
            int size = key ? 520_000 + rng.Next(200_000)
                           : (int)(12_000 * Math.Exp(rng.NextDouble() * 2.9));   // ~12KB..~220KB
            list.Add((size, key));
        }
        return list;
    }
}
