using System;
using System.Threading;
using SharpDX.DXGI;
using SharpDX.Direct3D11;
using Texture2D = SharpDX.Direct3D11.Texture2D;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Captures a monitor via DXGI Desktop Duplication — the high-rate path. Unlike WGC window
/// capture (capped to the DWM composition rate, ~60 fps on a mixed-refresh multi-monitor setup),
/// duplication delivers frames at the monitor's real present rate: measured 383 fps for a
/// borderless CS2 pushing 320+, versus 60 from WGC on the very same machine. For a borderless
/// fullscreen game the monitor shows the game, so this captures the game at full speed.
///
/// It hands out the desktop BGRA texture and the present time (QPC) per frame, the same contract
/// the VFR encoder already consumes. Runs its own acquire loop on a dedicated thread; the device
/// is multithread-protected so encoding on this thread is safe. On a transient duplication loss
/// (resolution change, full-screen transition, secure desktop) it transparently re-creates the
/// duplication.
/// </summary>
public sealed class DxgiDuplicationCaptureSource : IDisposable
{
    private readonly D3D11Context _d3d;
    private readonly int _outputIndex;
    private OutputDuplication? _dup;
    private Output1? _output1;
    private Thread? _thread;
    private volatile bool _running;
    private readonly long _qpcFreq;

    /// <summary>(texture valid only during the call, present time in microseconds).</summary>
    public event Action<Texture2D, long>? FrameReady;

    public int MonitorWidth { get; private set; }
    public int MonitorHeight { get; private set; }

    public DxgiDuplicationCaptureSource(D3D11Context d3d, int outputIndex)
    {
        _d3d = d3d;
        _outputIndex = outputIndex;
        System.Diagnostics.Stopwatch.GetTimestamp();
        _qpcFreq = System.Diagnostics.Stopwatch.Frequency;
    }

    /// <summary>Finds the DXGI output index whose desktop bounds contain the given point (the game
    /// window's monitor). Returns 0 (primary) if none matched.</summary>
    public static int FindOutputForWindow(D3D11Context d3d, int px, int py)
    {
        try
        {
            using var dxgiDev = d3d.Device.QueryInterface<SharpDX.DXGI.Device>();
            using var adapter = dxgiDev.Adapter;
            for (int i = 0; ; i++)
            {
                Output o;
                try { o = adapter.GetOutput(i); } catch { break; }
                using (o)
                {
                    var b = o.Description.DesktopBounds;
                    if (px >= b.Left && px < b.Right && py >= b.Top && py < b.Bottom) return i;
                }
            }
        }
        catch { }
        return 0;
    }

    public void Start()
    {
        CreateDuplication();
        _running = true;
        _thread = new Thread(AcquireLoop) { IsBackground = true, Name = "DxgiDuplication", Priority = ThreadPriority.AboveNormal };
        _thread.Start();
    }

    private void CreateDuplication()
    {
        using var dxgiDev = _d3d.Device.QueryInterface<SharpDX.DXGI.Device>();
        using var adapter = dxgiDev.Adapter;
        using var output = adapter.GetOutput(_outputIndex);
        var b = output.Description.DesktopBounds;
        MonitorWidth = b.Right - b.Left;
        MonitorHeight = b.Bottom - b.Top;
        _output1 = output.QueryInterface<Output1>();
        _dup = _output1.DuplicateOutput(_d3d.Device);
        Console.WriteLine($"[DxgiDuplication] Output {_outputIndex} {MonitorWidth}x{MonitorHeight} duplicated.");
    }

    private void AcquireLoop()
    {
        long lastPresent = -1;
        while (_running)
        {
            OutputDuplication? dup = _dup;
            if (dup == null) { Thread.Sleep(5); continue; }
            try
            {
                var res = dup.TryAcquireNextFrame(100, out var info, out var desktopResource);
                if (res.Failure || desktopResource == null)
                {
                    if (res.Code == SharpDX.DXGI.ResultCode.AccessLost.Result.Code) RecreateDuplication();
                    continue;
                }
                try
                {
                    // Only emit on a real present (the screen actually changed) — keeps the VFR
                    // buffer to genuine new frames, no idle-desktop repeats.
                    if (info.LastPresentTime != 0 && info.LastPresentTime != lastPresent)
                    {
                        lastPresent = info.LastPresentTime;
                        long ptsUs = info.LastPresentTime * 1_000_000L / _qpcFreq;
                        using var tex = desktopResource.QueryInterface<Texture2D>();
                        FrameReady?.Invoke(tex, ptsUs);
                    }
                }
                finally
                {
                    desktopResource.Dispose();
                    dup.ReleaseFrame();
                }
            }
            catch (SharpDX.SharpDXException ex)
            {
                if (ex.ResultCode == SharpDX.DXGI.ResultCode.AccessLost) RecreateDuplication();
                else Thread.Sleep(2);
            }
            catch { Thread.Sleep(2); }
        }
    }

    private void RecreateDuplication()
    {
        try { _dup?.Dispose(); } catch { }
        try { _output1?.Dispose(); } catch { }
        _dup = null; _output1 = null;
        Thread.Sleep(50); // let the mode/flip transition settle
        try { CreateDuplication(); }
        catch (Exception ex) { Console.WriteLine($"[DxgiDuplication] recreate failed: {ex.Message}"); }
    }

    public void Dispose()
    {
        _running = false;
        try { _thread?.Join(500); } catch { }
        try { _dup?.Dispose(); } catch { }
        try { _output1?.Dispose(); } catch { }
        _dup = null; _output1 = null;
    }
}
