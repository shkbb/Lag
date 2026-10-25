using System;
using System.Threading;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Headless check of <see cref="CaptureStallMonitor"/>'s attribution (LAG_STALLTEST=1). Replays the
/// two shapes a stall takes in a real capture and checks each is blamed on the right side:
///   • the game stops presenting while we sit idle            → SOURCE
///   • we block on one frame; the 3-buffer pool drains, then the
///     gap left by the frames it had to drop appears            → OURS, with the slow stage named
/// A misattribution here would send the next investigation after the wrong culprit, so it is worth a
/// few lines of test.
/// </summary>
public static class StallMonitorSelfTest
{
    private static int _failed;

    public static void Run()
    {
        Console.WriteLine("[StallTest] stall attribution checks");
        SourceGapIsBlamedOnTheGame();
        SlowEncodeIsBlamedOnUsEvenWhenTheGapShowsLater();
        SlowOverlayIsNamed();
        NormalCadenceReportsNothing();
        ResetForgetsTheTimeline();
        Console.WriteLine(_failed == 0 ? "[StallTest] ALL PASSED" : $"[StallTest] {_failed} CHECK(S) FAILED");
    }

    private const long Step = 6_061;   // 165fps

    private static void SourceGapIsBlamedOnTheGame()
    {
        var m = new CaptureStallMonitor();
        long t = 0;
        for (int i = 0; i < 10; i++) { Frame(m, t, work: (0, 0, 300)); t += Step; }
        t += 48_000;                                   // the game presents nothing for 48ms
        Frame(m, t, work: (0, 0, 300));
        string? r = m.DrainReport();
        Check("a game hitch is reported", r != null);
        Check("...as SOURCE", r != null && r.Contains("1 SOURCE") && r.Contains("0 OURS"));
    }

    private static void SlowEncodeIsBlamedOnUsEvenWhenTheGapShowsLater()
    {
        var m = new CaptureStallMonitor();
        long t = 0;
        for (int i = 0; i < 10; i++) { Frame(m, t, work: (0, 0, 300)); t += Step; }
        Frame(m, t, work: (0, 0, 45_000), sleepMs: 45);   // we block for 45ms inside encode
        t += Step;
        Frame(m, t, work: (0, 0, 300)); t += Step;        // the two frames the pool held arrive...
        Frame(m, t, work: (0, 0, 300)); t += Step;
        t += 40_000;                                      // ...then the hole of the dropped frames
        Frame(m, t, work: (0, 0, 300));
        string? r = m.DrainReport();
        Check("our stall is reported", r != null);
        Check("...as OURS", r != null && r.Contains("1 OURS"));
        Check("...naming the encoder", r != null && r.Contains("OURS: encode"));
    }

    private static void SlowOverlayIsNamed()
    {
        var m = new CaptureStallMonitor();
        long t = 0;
        for (int i = 0; i < 5; i++) { Frame(m, t, work: (0, 0, 300)); t += Step; }
        Frame(m, t, work: (40_000, 0, 300), sleepMs: 40);  // overlay draw blocks
        t += 46_000;
        Frame(m, t, work: (0, 0, 300));
        string? r = m.DrainReport();
        Check("a slow overlay is named", r != null && r.Contains("OURS: overlay"));
    }

    private static void NormalCadenceReportsNothing()
    {
        var m = new CaptureStallMonitor();
        long t = 0;
        for (int i = 0; i < 500; i++) { Frame(m, t, work: (0, 0, 300)); t += Step * ((i % 7 == 0) ? 2 : 1); }
        Check("ordinary 1-2 step cadence is not a stall", m.DrainReport() == null);
    }

    private static void ResetForgetsTheTimeline()
    {
        var m = new CaptureStallMonitor();
        Frame(m, 0, work: (0, 0, 300));
        m.Reset();
        Frame(m, 5_000_000, work: (0, 0, 300));        // new session, clock restarted
        Check("a gap across a reset is not a stall", m.DrainReport() == null);
    }

    /// <summary>Feeds one frame: arrives at <paramref name="presentUs"/>, "works" for the given
    /// stages (overlay, convert, encode µs), optionally really blocking so the measured frame time is
    /// genuine rather than only the declared stage times.</summary>
    private static void Frame(CaptureStallMonitor m, long presentUs, (long Overlay, long Convert, long Encode) work,
                              int sleepMs = 0)
    {
        m.FrameArrived(presentUs, presentUs + 1_000);
        if (sleepMs > 0) Thread.Sleep(sleepMs);
        m.FrameDone(work.Overlay, work.Convert, work.Encode, 0);
    }

    private static void Check(string what, bool ok)
    {
        if (!ok) _failed++;
        Console.WriteLine($"  [{(ok ? "PASS" : "FAIL")}] {what}");
    }
}
