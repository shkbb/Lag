using System;
using System.Diagnostics;
using System.Reflection;
using System.Threading;
using Avalonia.Rendering;

namespace Lag.Services;

/// <summary>
/// Background render timer with a RUNTIME-adjustable tick rate. Replaces Avalonia's fixed-rate
/// DefaultRenderTimer: the app retunes it to the refresh rate of the monitor the window is
/// currently on (fullscreen playback on a 165 Hz panel composes at 165 Hz; drag the window to a
/// 180 Hz panel and it follows). A fixed rate that mismatches the active panel's Hz beats
/// against vsync — frames repeat/skip on an irregular cadence and playback looks juddery.
///
/// IRenderTimer is fenced off in Avalonia's reference assemblies ("not implementable by user
/// code"), so the interface surface (<see cref="Interface"/>) is generated at runtime with
/// DispatchProxy — the runtime assembly's interface has no such fence — and forwarded here.
/// </summary>
public sealed class DynamicRenderTimer
{
    private readonly object _gate = new();
    private Action<TimeSpan>? _tick;
    private Thread? _thread;
    private volatile int _hz;
    private long _wantMonitor;   // HMONITOR of the window's display (0 = none → clock pacing)

    /// <summary>The IRenderTimer facade to bind into AvaloniaLocator.</summary>
    public IRenderTimer Interface { get; }

    public DynamicRenderTimer(int hz)
    {
        _hz = ClampHz(hz);
        var proxy = DispatchProxy.Create<IRenderTimer, RenderTimerFacade>();
        ((RenderTimerFacade)(object)proxy).Owner = this;
        Interface = proxy;
    }

    private static int ClampHz(int hz) => Math.Clamp(hz, 30, 360);

    /// <summary>Current tick rate for the CLOCK fallback. When vsync-locked the display's
    /// vblank IS the cadence and this value is informational only.</summary>
    public int Hz
    {
        get => _hz;
        set
        {
            int v = ClampHz(value);
            if (_hz == v) return;
            _hz = v;
            Console.WriteLine($"[RenderTimer] retuned to {v} Hz (window's monitor).");
        }
    }

    /// <summary>Points the timer at the display the window is on. The loop then ticks on that
    /// output's REAL vblank (DXGI WaitForVBlank) — phase-locked composition, no beat against
    /// the panel — and falls back to clock pacing at <see cref="Hz"/> when DXGI can't help.</summary>
    public void SetMonitor(IntPtr hmonitor) => Interlocked.Exchange(ref _wantMonitor, hmonitor.ToInt64());

    public void AddTick(Action<TimeSpan> handler)
    {
        lock (_gate)
        {
            _tick += handler;
            EnsureThread();
        }
    }

    public void RemoveTick(Action<TimeSpan> handler)
    {
        lock (_gate) _tick -= handler;
    }

    private void EnsureThread()
    {
        if (_thread != null) return;
        _thread = new Thread(Loop) { IsBackground = true, Name = "Lag render timer", Priority = ThreadPriority.AboveNormal };
        _thread.Start();
    }

    private void Loop()
    {
        var sw = Stopwatch.StartNew();
        double next = sw.ElapsedTicks;
        SharpDX.DXGI.Output? output = null;
        long boundMon = -1;

        while (true)
        {
            // (Re)bind the DXGI output when the window moved to another monitor.
            long want = Interlocked.Read(ref _wantMonitor);
            if (want != boundMon)
            {
                output?.Dispose();
                output = TryGetOutput(new IntPtr(want));
                boundMon = want;
                next = sw.ElapsedTicks;
                Console.WriteLine(output != null
                    ? "[RenderTimer] vsync-locked to the window's monitor."
                    : "[RenderTimer] no DXGI output resolved — clock pacing.");
            }

            if (output != null)
            {
                bool ticked = false;
                try
                {
                    output.WaitForVerticalBlank();   // blocks until the panel's real vblank
                    ticked = true;
                }
                catch (Exception ex)
                {
                    // Device lost / display off — drop to clock pacing until the monitor changes.
                    Console.WriteLine($"[RenderTimer] WaitForVBlank failed ({ex.Message}) — clock pacing.");
                    output.Dispose();
                    output = null;
                    next = sw.ElapsedTicks;
                }
                if (ticked)
                {
                    _tick?.Invoke(sw.Elapsed);
                    continue;
                }
            }

            // Clock fallback: fixed-rate pacing at Hz. timeBeginPeriod(1) is held in Main, so
            // Sleep() has ~1 ms granularity — same pacing quality as Avalonia's DefaultRenderTimer.
            double interval = (double)Stopwatch.Frequency / _hz;
            next += interval;
            if (sw.ElapsedTicks - next > interval * 4) next = sw.ElapsedTicks; // resync after a stall
            int sleepMs = (int)((next - sw.ElapsedTicks) * 1000 / Stopwatch.Frequency);
            if (sleepMs > 0) Thread.Sleep(sleepMs);
            _tick?.Invoke(sw.Elapsed);
        }
    }

    /// <summary>The DXGI output whose attached monitor is <paramref name="hmon"/>, or null.
    /// COM refcounting keeps the output usable after the factory/adapter are disposed.</summary>
    private static SharpDX.DXGI.Output? TryGetOutput(IntPtr hmon)
    {
        if (hmon == IntPtr.Zero) return null;
        try
        {
            using var factory = new SharpDX.DXGI.Factory1();
            int adapters = factory.GetAdapterCount1();
            for (int a = 0; a < adapters; a++)
            {
                using var adapter = factory.GetAdapter1(a);
                for (int o = 0; ; o++)
                {
                    SharpDX.DXGI.Output output;
                    try { output = adapter.GetOutput(o); }
                    catch { break; }   // out of outputs on this adapter
                    if (output.Description.MonitorHandle == hmon) return output;
                    output.Dispose();
                }
            }
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[RenderTimer] DXGI output lookup failed: {ex.Message}");
        }
        return null;
    }
}

/// <summary>Runtime-generated IRenderTimer implementation forwarding to <see cref="DynamicRenderTimer"/>.
/// Must be public non-sealed with a parameterless ctor (DispatchProxy requirements).</summary>
public class RenderTimerFacade : DispatchProxy
{
    internal DynamicRenderTimer Owner = null!;

    protected override object? Invoke(MethodInfo? targetMethod, object?[]? args)
    {
        switch (targetMethod?.Name)
        {
            case "add_Tick" when args is [Action<TimeSpan> h]:
                Owner.AddTick(h);
                return null;
            case "remove_Tick" when args is [Action<TimeSpan> h]:
                Owner.RemoveTick(h);
                return null;
            case "get_RunsInBackground":
                return true;
            default:
                return null;
        }
    }
}
