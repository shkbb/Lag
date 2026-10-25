using System;
using System.Diagnostics;
using System.Threading;
using SharpDX.DXGI;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Measures how many frames actually reach a monitor's output, via DXGI Desktop Duplication
/// (LAG_DUPPROBE=1). Nothing is encoded or saved: it only counts.
///
/// Why: WGC window capture was measured delivering only a game's RENDERED frames (~72/s in Dying
/// Light: The Beast) while frame generation put roughly twice that on screen. Generated frames are
/// visible, so they must pass through the display output — and duplication captures the output
/// rather than the window. This probe answers "does duplication see them?" before anything is built
/// on that assumption.
///
///   LAG_DUPPROBE=1 [LAG_DUPPROBE_OUTPUT=n] [LAG_DUPPROBE_SECONDS=s] Lag.exe
///
/// Reports per second: frames acquired, presents accumulated (AccumulatedFrames — every present to
/// the output, even ones the probe was too slow to acquire individually), and distinct present
/// timestamps.
/// </summary>
public static class DuplicationProbe
{
    public static void Run()
    {
        int seconds = int.TryParse(Environment.GetEnvironmentVariable("LAG_DUPPROBE_SECONDS"), out var s) ? s : 8;
        int wantOutput = int.TryParse(Environment.GetEnvironmentVariable("LAG_DUPPROBE_OUTPUT"), out var o) ? o : 0;

        using var d3d = new D3D11Context();
        using var dxgiDev = d3d.Device.QueryInterface<SharpDX.DXGI.Device>();
        using var adapter = dxgiDev.Adapter;
        Console.WriteLine($"[DupProbe] adapter: {adapter.Description.Description}");

        for (int i = 0; ; i++)
        {
            Output? oi;
            try { oi = adapter.GetOutput(i); } catch { break; }
            using (oi)
            {
                var b = oi.Description.DesktopBounds;
                Console.WriteLine($"[DupProbe] output {i}: {oi.Description.DeviceName} {b.Right - b.Left}x{b.Bottom - b.Top}" +
                                  $" at ({b.Left},{b.Top}){(i == wantOutput ? "  <- probing" : "")}");
            }
        }

        using var output = adapter.GetOutput(wantOutput);
        using var output1 = output.QueryInterface<Output1>();
        using var dup = output1.DuplicateOutput(d3d.Device);

        long acquired = 0, accumulated = 0, distinct = 0, lastPresent = -1;
        long secAcquired = 0, secAccumulated = 0, secDistinct = 0;
        var sw = Stopwatch.StartNew();
        double nextMark = 1.0;
        Console.WriteLine($"[DupProbe] counting for {seconds}s — keep the game in the foreground...");

        while (sw.Elapsed.TotalSeconds < seconds)
        {
            var res = dup.TryAcquireNextFrame(100, out var info, out var desktop);
            if (res.Failure || desktop == null)
            {
                if (res.Code == SharpDX.DXGI.ResultCode.AccessLost.Result.Code)
                {
                    Console.WriteLine("[DupProbe] access lost (mode change / secure desktop) — stopping.");
                    break;
                }
            }
            else
            {
                try
                {
                    acquired++; secAcquired++;
                    accumulated += info.AccumulatedFrames; secAccumulated += info.AccumulatedFrames;
                    if (info.LastPresentTime != 0 && info.LastPresentTime != lastPresent)
                    {
                        lastPresent = info.LastPresentTime;
                        distinct++; secDistinct++;
                    }
                }
                finally
                {
                    desktop.Dispose();
                    dup.ReleaseFrame();
                }
            }

            if (sw.Elapsed.TotalSeconds >= nextMark)
            {
                Console.WriteLine($"[DupProbe]   {nextMark,3:F0}s: acquired {secAcquired,4}/s | presents {secAccumulated,4}/s | distinct {secDistinct,4}/s");
                secAcquired = secAccumulated = secDistinct = 0;
                nextMark += 1.0;
            }
        }

        double t = Math.Max(0.001, sw.Elapsed.TotalSeconds);
        Console.WriteLine($"[DupProbe] TOTAL over {t:F1}s: acquired {acquired / t:F1}/s | presents to output {accumulated / t:F1}/s | distinct frames {distinct / t:F1}/s");
    }
}
