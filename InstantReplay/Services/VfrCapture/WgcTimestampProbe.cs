using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Measures what WGC's per-frame timestamp (<c>SystemRelativeTime</c>) actually means, against the moment
/// the frame reaches us (LAG_WGCPROBE=1). Captures the FOREGROUND window with the same
/// <see cref="WgcCaptureSource"/> the engine uses, records (arrival QPC, frame timestamp) per frame, and
/// encodes nothing.
///
/// Why: the engine converts SystemRelativeTime straight into its QPC timeline and trusts it only when
/// arrival − timestamp lies in [−5ms, +500ms], falling back to the arrival time otherwise. After that
/// change the "present-lag" diagnostic never printed, i.e. no frame ever arrived AFTER its timestamp —
/// which suggests the timestamp is a target display time in the near future. If part of the frames
/// then fall outside −5ms, a clip mixes two clocks. This probe shows the real distribution.
///
///   LAG_WGCPROBE=1 [LAG_WGCPROBE_SECONDS=10] [LAG_WGCPROBE_FPS=165] Lag.exe   (game in the foreground)
/// </summary>
public static class WgcTimestampProbe
{
    [DllImport("user32.dll")] private static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern int GetWindowText(IntPtr h, StringBuilder s, int n);

    public static void Run()
    {
        int seconds = int.TryParse(Environment.GetEnvironmentVariable("LAG_WGCPROBE_SECONDS"), out var s) ? s : 10;
        int fps = int.TryParse(Environment.GetEnvironmentVariable("LAG_WGCPROBE_FPS"), out var f) ? f : 165;

        IntPtr hwnd = GetForegroundWindow();
        var title = new StringBuilder(256);
        GetWindowText(hwnd, title, 256);
        GetWindowThreadProcessId(hwnd, out uint pid);
        string proc = "?";
        try { proc = Process.GetProcessById((int)pid).ProcessName; } catch { }
        Console.WriteLine($"[WgcProbe] target: '{title}' ({proc}, pid {pid}), {seconds}s, MinUpdateInterval for {fps}fps");

        // Preallocated so the capture callback never allocates or locks.
        int cap = Math.Max(1000, seconds * 600);
        var arrival = new long[cap];
        var source = new long[cap];
        int n = 0;

        using var d3d = new D3D11Context();
        using var wgc = new WgcCaptureSource(d3d, hwnd);
        wgc.FrameReady += (_, ticks) =>
        {
            long now = Stopwatch.GetTimestamp();
            int i = n;
            if (i >= cap) return;
            arrival[i] = now;
            source[i] = ticks;
            n = i + 1;
        };
        wgc.Start(captureCursor: false, maxFps: fps);
        Thread.Sleep(seconds * 1000);
        wgc.Dispose();

        int count = Math.Min(n, cap);
        Console.WriteLine($"[WgcProbe] frames: {count} ({count / (double)seconds:F1}/s)");
        if (count < 20) { Console.WriteLine("[WgcProbe] too few frames — is the game in the foreground and presenting?"); return; }

        long freq = Stopwatch.Frequency;
        double UsFromQpc(long q) => q * 1_000_000.0 / freq;
        // SystemRelativeTime is 100ns units; Stopwatch ticks are 1/freq (also 100ns when freq = 10MHz).
        long ToQpc(long srt) => freq == 10_000_000L ? srt : (long)(srt * (freq / 10_000_000.0));

        var lag = new double[count];                        // arrival − timestamp, µs (+ = arrived after)
        for (int i = 0; i < count; i++) lag[i] = UsFromQpc(arrival[i] - ToQpc(source[i]));
        var sorted = lag.OrderBy(x => x).ToArray();
        double P(double q) => sorted[Math.Min(sorted.Length - 1, (int)(sorted.Length * q))];

        int future = lag.Count(x => x <= 0);
        int fallback = lag.Count(x => x < -5_000 || x > 500_000);
        int nonMono = 0;
        for (int i = 1; i < count; i++) if (source[i] <= source[i - 1]) nonMono++;

        Console.WriteLine();
        Console.WriteLine($"[WgcProbe] QPC frequency {freq} Hz (100ns units: {freq == 10_000_000L})");
        Console.WriteLine("[WgcProbe] arrival − frame timestamp (ms; negative = timestamp is AHEAD of arrival):");
        Console.WriteLine($"             min {P(0) / 1000,7:F2} | p1 {P(.01) / 1000,7:F2} | p10 {P(.10) / 1000,7:F2} | median {P(.5) / 1000,7:F2}" +
                          $" | p90 {P(.90) / 1000,7:F2} | p99 {P(.99) / 1000,7:F2} | max {P(1) / 1000,7:F2}");
        Console.WriteLine($"[WgcProbe] timestamp at/after arrival: {future} of {count} ({future * 100.0 / count:F1}%)");
        Console.WriteLine($"[WgcProbe] frames the engine would REJECT (lag < −5ms or > 500ms -> arrival-time fallback): " +
                          $"{fallback} ({fallback * 100.0 / count:F1}%)");
        Console.WriteLine($"[WgcProbe] non-increasing frame timestamps: {nonMono}");

        // Which clock gives the smoother cadence?
        Report("frame timestamps (SystemRelativeTime)", Intervals(source, count, t => UsFromQpc(ToQpc(t))));
        Report("arrival times (when our callback ran)", Intervals(arrival, count, UsFromQpc));

        // What would the RECORDED clip look like? Run each timestamp strategy through the real CadenceGrid
        // at the same cap the engine uses, and measure the output cadence the way clips are measured.
        Console.WriteLine();
        Console.WriteLine($"[WgcProbe] output cadence after CadenceGrid({fps}) — what each strategy would record:");
        var arrUs = new long[count]; var srcUs = new long[count]; var mixUs = new long[count];
        for (int i = 0; i < count; i++)
        {
            arrUs[i] = (long)UsFromQpc(arrival[i]);
            srcUs[i] = (long)UsFromQpc(ToQpc(source[i]));
            double l = arrUs[i] - srcUs[i];
            mixUs[i] = (l < -5_000 || l > 500_000) ? arrUs[i] : srcUs[i];   // today's engine rule
        }
        Simulate("A  arrival time for every frame", arrUs, fps);
        Simulate("B  WGC timestamp for every frame", srcUs, fps);
        Simulate("C  today's engine (mixed per frame)", mixUs, fps);
    }

    private static void Simulate(string label, long[] ptsUs, int fps)
    {
        var grid = new CadenceGrid(fps);
        var kept = new List<long>(ptsUs.Length);
        long last = long.MinValue;
        int dropped = 0, backwards = 0;
        foreach (var p in ptsUs)
        {
            long s = grid.Snap(p);
            if (s == CadenceGrid.Drop) { dropped++; continue; }
            if (last != long.MinValue && s <= last) backwards++;
            last = s;
            kept.Add(s);
        }
        var d = new List<double>();
        for (int i = 1; i < kept.Count; i++) d.Add((kept[i] - kept[i - 1]) / 1000.0);
        if (d.Count == 0) return;
        var s2 = d.OrderBy(x => x).ToList();
        double med = s2[s2.Count / 2], mean = d.Average();
        double sd = Math.Sqrt(d.Sum(x => (x - mean) * (x - mean)) / d.Count);
        double w10 = d.Count(x => Math.Abs(x - med) <= med * 0.10) * 100.0 / d.Count;
        int distinct = d.Select(x => Math.Round(x, 2)).Distinct().Count();
        Console.WriteLine($"   {label,-38} kept {kept.Count,4} dropped {dropped,3} | within +/-10% {w10,5:F1}% | " +
                          $"distinct {distinct,2} | stdev {sd:F2}ms | max {s2[^1]:F2}ms");
    }

    private static List<double> Intervals(long[] t, int count, Func<long, double> toUs)
    {
        var d = new List<double>(count);
        for (int i = 1; i < count; i++)
        {
            double ms = (toUs(t[i]) - toUs(t[i - 1])) / 1000.0;
            if (ms > 0) d.Add(ms);
        }
        return d;
    }

    private static void Report(string label, List<double> d)
    {
        if (d.Count == 0) return;
        var s = d.OrderBy(x => x).ToList();
        double med = s[s.Count / 2], mean = d.Average();
        double sd = Math.Sqrt(d.Sum(x => (x - mean) * (x - mean)) / d.Count);
        double w10 = d.Count(x => Math.Abs(x - med) <= med * 0.10) * 100.0 / d.Count;
        Console.WriteLine($"[WgcProbe] {label}:");
        Console.WriteLine($"             median {med:F2}ms | stdev {sd:F2}ms | within +/-10% of median {w10:F1}% | max {s[^1]:F2}ms");

        // Are the intervals whole multiples of a display refresh period? (vblank-locked timestamps)
        foreach (double hz in new[] { 165.0, 144.0, 60.0 })
        {
            double period = 1000.0 / hz;
            double onGrid = d.Count(x => { double k = Math.Round(x / period); return k >= 1 && Math.Abs(x - k * period) <= 0.25; })
                            * 100.0 / d.Count;
            Console.WriteLine($"             multiples of {hz}Hz ({period:F2}ms, +/-0.25ms): {onGrid:F1}%");
        }
        var top = d.GroupBy(x => Math.Round(x * 4) / 4).OrderByDescending(g => g.Count()).Take(8)
                   .Select(g => $"{g.Key:F2}ms x{g.Count()}");
        Console.WriteLine($"             most common: {string.Join(", ", top)}");
    }
}
