using System;
using System.Collections.Generic;
using System.Linq;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Headless check of the cadence stabiliser (LAG_CADENCETEST=1). Feeds <see cref="CadenceGrid"/> a
/// synthetic presentation stream shaped like real capture — a high-fps game with per-frame wobble
/// plus the occasional long stall — and reports the same interval statistics used to compare our
/// output against the reference recorder, so a regression shows up as numbers rather than as
/// "it feels choppy".
///
/// Targets come from a measured Medal clip (CS2, 144fps cap): 3 distinct interval lengths, 92% of
/// frames within 10% of the median. Our pre-stabiliser capture measured 846 and 14%.
/// </summary>
public static class CadenceSelfTest
{
    public static void Run()
    {
        Console.WriteLine("[CadenceTest] stabiliser check — synthetic 165fps source, 120fps cap\n");

        var source = SyntheticPresentTimes(seed: 1234, frames: 3000, fps: 165.0,
                                           jitterUs: 1500, stallEveryN: 400, stallUs: 45_000);

        var raw = Intervals(source);
        var snapped = Intervals(Apply(new CadenceGrid(120), source, out long dropped));

        Report("RAW present times (what we wrote before)", raw);
        Report("SNAPPED to the 120fps grid", snapped);
        Console.WriteLine($"  frames dropped into filled slots: {dropped} of {source.Count}\n");

        int distinct = DistinctIntervals(snapped);
        double within10 = WithinPct(snapped, 0.10);
        bool ok = distinct <= 5 && within10 >= 85.0;

        Console.WriteLine($"[CadenceTest] distinct intervals {distinct} (target <= 5, Medal = 3)");
        Console.WriteLine($"[CadenceTest] within +/-10% of median {within10:F1}% (target >= 85%, Medal = 92.4%)");
        Console.WriteLine(ok ? "[CadenceTest] PASSED" : "[CadenceTest] FAILED");
    }

    // ── the synthetic source ────────────────────────────────────────────────

    /// <summary>Presentation times (µs) for a game running at <paramref name="fps"/> with per-frame
    /// jitter and a periodic long stall (the shape a GC pause or a loading hitch leaves).</summary>
    private static List<long> SyntheticPresentTimes(int seed, int frames, double fps,
                                                    int jitterUs, int stallEveryN, int stallUs)
    {
        var rng = new Random(seed);
        var list = new List<long>(frames);
        double step = 1_000_000.0 / fps;
        double t = 0;
        for (int i = 0; i < frames; i++)
        {
            t += step + (rng.NextDouble() * 2 - 1) * jitterUs;
            if (stallEveryN > 0 && i % stallEveryN == stallEveryN - 1) t += stallUs;
            list.Add((long)t);
        }
        return list;
    }

    private static List<long> Apply(CadenceGrid grid, List<long> source, out long dropped)
    {
        var outp = new List<long>(source.Count);
        dropped = 0;
        foreach (var p in source)
        {
            long s = grid.Snap(p);
            if (s == CadenceGrid.Drop) { dropped++; continue; }
            outp.Add(s);
        }
        return outp;
    }

    // ── statistics (mirrors how the Medal clip was measured) ────────────────

    private static List<double> Intervals(List<long> pts)
    {
        var d = new List<double>(Math.Max(0, pts.Count - 1));
        for (int i = 1; i < pts.Count; i++)
        {
            double ms = (pts[i] - pts[i - 1]) / 1000.0;
            if (ms > 0) d.Add(ms);
        }
        return d;
    }

    private static double Median(List<double> d)
    {
        var s = d.OrderBy(x => x).ToList();
        return s.Count == 0 ? 0 : s[s.Count / 2];
    }

    /// <summary>Distinct interval lengths at 0.01 ms resolution — a quantised cadence has very few.</summary>
    private static int DistinctIntervals(List<double> d) =>
        d.Select(x => Math.Round(x, 2)).Distinct().Count();

    private static double WithinPct(List<double> d, double frac)
    {
        if (d.Count == 0) return 0;
        double med = Median(d);
        return d.Count(x => Math.Abs(x - med) <= med * frac) * 100.0 / d.Count;
    }

    private static void Report(string label, List<double> d)
    {
        if (d.Count == 0) { Console.WriteLine($"  {label}: no intervals"); return; }
        var s = d.OrderBy(x => x).ToList();
        double med = Median(d);
        double mean = d.Average();
        double sd = Math.Sqrt(d.Sum(x => (x - mean) * (x - mean)) / d.Count);
        Console.WriteLine($"  {label}");
        Console.WriteLine($"    median {med:F2}ms (= {1000 / med:F1} fps) | stdev {sd:F2}ms | max {s[^1]:F2}ms");
        Console.WriteLine($"    distinct intervals {DistinctIntervals(d),5} | within +/-10% of median {WithinPct(d, 0.10):F1}%");
    }
}
