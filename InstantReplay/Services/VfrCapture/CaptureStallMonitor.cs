using System;
using System.Collections.Concurrent;
using System.Diagnostics;
using System.Text;
using System.Threading;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Attributes gaps in the captured frame stream to their cause, so a stall in a saved clip can be
/// traced to "the game presented nothing" versus "we were busy doing X" instead of being guessed at.
///
/// How a stall of OURS shows up: WGC keeps presenting into a 3-buffer frame pool while the capture
/// thread is busy. When the thread comes back it drains the frames the pool held, and only then hits
/// the hole left by the frames the full pool had to drop. So the gap in presentation times appears a
/// frame or two AFTER the slow frame — which is why the most recent frames' work is kept and the
/// heaviest of them is what gets compared against the gap.
///
/// Classification per gap longer than the threshold:
///   • OURS   — some recent frame's work (plus any GC pause that hit the thread while it sat idle) is at
///              least half the gap; the cause is that frame's dominant component.
///   • SOURCE — nothing we did comes close; the game (or DWM) simply did not present.
///
/// Threading: <see cref="FrameArrived"/>/<see cref="FrameDone"/>/<see cref="Reset"/> run on the
/// single capture thread; <see cref="DrainReport"/> runs on the reporting thread. The only shared
/// state is the event queue and two Interlocked counters.
/// </summary>
internal sealed class CaptureStallMonitor
{
    /// <summary>Gaps longer than this are reported. 25ms is ~4 frames at 165fps — well past normal
    /// cadence variation, short enough to catch a visible hitch.</summary>
    public const long DefaultThresholdUs = 25_000;

    private const int RecentFrames = 8;          // covers the 3-buffer pool plus a margin
    private const int MaxQueuedEvents = 64;      // bound memory if nobody drains for a while
    private const int MaxReportedEvents = 6;     // detail lines per report; the rest are counted

    private readonly long _thresholdUs;
    private static readonly double UsPerTick = 1_000_000.0 / Stopwatch.Frequency;

    // ── capture thread only ────────────────────────────────────────────────────────────────────
    private readonly Work[] _recent = new Work[RecentFrames];
    private int _recentNext;
    private long _prevPresentUs = long.MinValue;
    private long _gcAtPrevEndTicks;              // GC total-pause ticks (100ns) when the last frame finished
    private bool _haveGcBase;
    private long _curStartTs, _curGcStartTicks, _curIdleGcUs;

    // ── shared with the reporting thread ───────────────────────────────────────────────────────
    private readonly ConcurrentQueue<StallEvent> _events = new();
    private long _sourceStalls, _ourStalls;

    public CaptureStallMonitor(long thresholdUs = DefaultThresholdUs) => _thresholdUs = thresholdUs;

    /// <summary>Time spent on one frame, split by stage. All microseconds.</summary>
    public struct Work
    {
        public long TotalUs, OverlayUs, ConvertUs, EncodeUs, RetuneUs, GcUs, IdleGcUs;

        /// <summary>Whatever the named stages do not account for (bookkeeping, logging, locks).</summary>
        public readonly long OtherUs => Math.Max(0, TotalUs - OverlayUs - ConvertUs - EncodeUs - RetuneUs);

        /// <summary>How much of the capture thread's time this frame held up, idle GC included.</summary>
        public readonly long BlockingUs => TotalUs + IdleGcUs;
    }

    private readonly record struct StallEvent(long GapUs, long AtUs, long WallTicksUtc, bool Ours,
                                              string Cause, Work Worst);

    /// <summary>Forget history — a new session or a resume starts a fresh timeline, and a gap across
    /// it is not a stall.</summary>
    public void Reset()
    {
        Array.Clear(_recent);
        _recentNext = 0;
        _prevPresentUs = long.MinValue;
        _haveGcBase = false;
    }

    /// <summary>Call first thing for every frame WGC delivers (including ones the cadence grid will
    /// drop): checks the presentation gap to the previous frame and starts timing this one.</summary>
    public void FrameArrived(long presentUs, long arrivalUs)
    {
        long gcNow = GC.GetTotalPauseDuration().Ticks;
        _curIdleGcUs = _haveGcBase ? Math.Max(0, (gcNow - _gcAtPrevEndTicks) / 10) : 0;
        _curGcStartTicks = gcNow;
        _curStartTs = Stopwatch.GetTimestamp();

        if (_prevPresentUs != long.MinValue)
        {
            long gap = presentUs - _prevPresentUs;
            if (gap > _thresholdUs) Classify(gap, arrivalUs);
        }
        _prevPresentUs = presentUs;
    }

    /// <summary>Call when the frame's processing ends (every exit path). Stage times are the ones
    /// measured for this frame; zeros for a frame that was skipped.</summary>
    public void FrameDone(long overlayUs, long convertUs, long encodeUs, long retuneUs)
    {
        long endTs = Stopwatch.GetTimestamp();
        long gcEnd = GC.GetTotalPauseDuration().Ticks;

        _recent[_recentNext] = new Work
        {
            TotalUs = (long)((endTs - _curStartTs) * UsPerTick),
            OverlayUs = overlayUs,
            ConvertUs = convertUs,
            EncodeUs = encodeUs,
            RetuneUs = retuneUs,
            GcUs = Math.Max(0, (gcEnd - _curGcStartTicks) / 10),
            IdleGcUs = _curIdleGcUs,
        };
        _recentNext = (_recentNext + 1) % RecentFrames;

        _gcAtPrevEndTicks = gcEnd;
        _haveGcBase = true;
    }

    private void Classify(long gapUs, long atUs)
    {
        // The heaviest recent frame — and the GC pause, if any, that hit us while waiting for THIS
        // frame (it has no work record yet).
        Work worst = default;
        foreach (var w in _recent)
            if (w.BlockingUs > worst.BlockingUs) worst = w;
        if (_curIdleGcUs > worst.BlockingUs) worst = new Work { IdleGcUs = _curIdleGcUs };

        bool ours = worst.BlockingUs * 2 >= gapUs;
        string cause = ours ? Dominant(worst) : "source";

        if (ours) Interlocked.Increment(ref _ourStalls);
        else Interlocked.Increment(ref _sourceStalls);

        if (_events.Count < MaxQueuedEvents)
            _events.Enqueue(new StallEvent(gapUs, atUs, DateTime.UtcNow.Ticks, ours, cause, worst));
    }

    /// <summary>Names the stage responsible. GC is checked first: a pause that lands mid-encode
    /// inflates the encode time, and would otherwise be misread as the encoder being slow.</summary>
    private static string Dominant(Work w)
    {
        long gc = w.GcUs + w.IdleGcUs;
        if (gc * 2 >= w.BlockingUs) return "gc";

        (string Name, long Us) best = ("other", w.OtherUs);
        if (w.OverlayUs > best.Us) best = ("overlay", w.OverlayUs);
        if (w.ConvertUs > best.Us) best = ("convert", w.ConvertUs);
        if (w.EncodeUs > best.Us) best = ("encode", w.EncodeUs);
        if (w.RetuneUs > best.Us) best = ("retune", w.RetuneUs);
        return best.Name;
    }

    /// <summary>Returns the stalls seen since the last call (summary plus the first few in detail),
    /// or null when there were none. Safe to call from any thread.</summary>
    public string? DrainReport()
    {
        long src = Interlocked.Exchange(ref _sourceStalls, 0);
        long ours = Interlocked.Exchange(ref _ourStalls, 0);
        if (src + ours == 0)
        {
            while (_events.TryDequeue(out _)) { }   // keep the queue from holding stale events
            return null;
        }

        var sb = new StringBuilder();
        sb.Append($"[stall] {src + ours} gap(s) >{_thresholdUs / 1000}ms: {src} SOURCE (game/DWM presented nothing), {ours} OURS");
        int shown = 0, more = 0;
        while (_events.TryDequeue(out var e))
        {
            if (shown++ >= MaxReportedEvents) { more++; continue; }
            var w = e.Worst;
            string when = new DateTime(e.WallTicksUtc, DateTimeKind.Utc).ToLocalTime().ToString("HH:mm:ss.fff");
            sb.Append($"\n    {e.GapUs / 1000.0,6:F1}ms gap at {when} -> ");
            if (e.Ours)
                sb.Append($"OURS: {e.Cause} | frame {w.TotalUs / 1000.0:F1}ms = overlay {w.OverlayUs / 1000.0:F1} " +
                          $"+ convert {w.ConvertUs / 1000.0:F1} + encode {w.EncodeUs / 1000.0:F1} " +
                          $"+ retune {w.RetuneUs / 1000.0:F1} + other {w.OtherUs / 1000.0:F1} " +
                          $"| gc in-frame {w.GcUs / 1000.0:F1} idle {w.IdleGcUs / 1000.0:F1}");
            else
                sb.Append($"SOURCE (heaviest recent frame of ours: {w.BlockingUs / 1000.0:F1}ms)");
        }
        if (more > 0) sb.Append($"\n    ... and {more} more");
        return sb.ToString();
    }
}
