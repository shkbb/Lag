using System;
using System.Runtime.InteropServices;
using SharpDX.Direct3D11;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Captures a game window as a single continuous stream of GAME frames, switching the underlying
/// source by where the game's frames actually live at each instant — the only cross-vendor,
/// hook-free way to get BOTH full frame rate AND game-follows-through-alt-tabs.
///
///   • Game in the foreground (borderless independent-flip): its frames go straight to the
///     monitor's flip chain at full rate, with no per-window surface to read. So we read the
///     monitor via DXGI Desktop Duplication — and since the game covers the monitor, that IS the
///     game, at full speed (measured 383 fps vs WGC's 60 on the same machine).
///   • Game in the background (alt-tabbed): it loses independent flip and is DWM-composed again,
///     so a per-window surface exists — WGC window capture reads the game's background frames
///     (never the desktop).
///
/// The engine sees one <see cref="FrameReady"/> event and never knows which source produced a
/// frame. Both timestamps are QPC-derived microseconds, so the stream stays monotonic and
/// seamless across a switch (a guard drops any out-of-order frame at the boundary).
/// </summary>
public sealed class HybridGameCaptureSource : IDisposable
{
    private readonly D3D11Context _d3d;
    private readonly IntPtr _hwnd;
    private readonly bool _captureCursor;
    private readonly bool _duplicationOnly; // testing/manual: skip foreground gating + WGC

    private DxgiDuplicationCaptureSource? _duplication; // foreground, full rate
    private WgcCaptureSource? _window;                  // background, follows the window
    private long _lastPtsUs = long.MinValue;
    private readonly object _emitGate = new();
    private volatile bool _disposed;
    private volatile bool _windowStarted;

    /// <summary>(texture valid only during the call, present time in microseconds).</summary>
    public event Action<Texture2D, long>? FrameReady;

    public HybridGameCaptureSource(D3D11Context d3d, IntPtr hwnd, bool captureCursor, bool duplicationOnly = false)
    {
        _d3d = d3d;
        _hwnd = hwnd;
        _captureCursor = captureCursor;
        _duplicationOnly = duplicationOnly;
    }

    public void Start()
    {
        // Duplication on the monitor the game window sits on.
        GetWindowRect(_hwnd, out var r);
        int cx = (r.Left + r.Right) / 2, cy = (r.Top + r.Bottom) / 2;
        int outputIndex = DxgiDuplicationCaptureSource.FindOutputForWindow(_d3d, cx, cy);

        _duplication = new DxgiDuplicationCaptureSource(_d3d, outputIndex);
        _duplication.FrameReady += OnDuplicationFrame;
        _duplication.Start();

        // WGC window is started lazily on the first alt-tab (creating it costs a ~0.5s frame-pool
        // spin-up; doing it while the user is still in-game would waste it). Until then duplication
        // covers the foreground game.
        Console.WriteLine("[HybridCapture] Started (duplication active; window capture armed for alt-tab).");
    }

    private void OnDuplicationFrame(Texture2D tex, long ptsUs)
    {
        // Foreground game → the monitor IS the game → accept at full rate.
        if (_disposed || !IsGameForeground()) return;
        Emit(tex, ptsUs);
    }

    private void OnWindowFrame(Texture2D tex, long ptsUs)
    {
        // Background game → duplication only sees the desktop, so the window source carries the
        // game. (When the game is foreground, duplication handles it and we ignore window frames.)
        if (_disposed || IsGameForeground()) return;
        Emit(tex, ptsUs);
    }

    private void Emit(Texture2D tex, long ptsUs)
    {
        lock (_emitGate)
        {
            if (ptsUs <= _lastPtsUs) return; // keep PTS strictly increasing across the source switch
            _lastPtsUs = ptsUs;
            FrameReady?.Invoke(tex, ptsUs);
        }
    }

    /// <summary>True when the captured game window is the foreground window — the moment its
    /// frames are on the monitor flip chain rather than in a DWM-composed window surface.</summary>
    private bool IsGameForeground()
    {
        if (_duplicationOnly) return true; // testing/manual: always treat as foreground
        bool fg = GetForegroundWindow() == _hwnd;
        if (!fg && !_windowStarted && !_disposed) StartWindowCaptureLazily();
        return fg;
    }

    private void StartWindowCaptureLazily()
    {
        lock (_emitGate)
        {
            if (_windowStarted || _disposed) return;
            _windowStarted = true;
        }
        try
        {
            var w = new WgcCaptureSource(_d3d, _hwnd);
            w.FrameReady += OnWindowFrame;
            w.Start(_captureCursor);
            _window = w;
            Console.WriteLine("[HybridCapture] Game backgrounded — window capture engaged (continuous game).");
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[HybridCapture] window capture start failed (background frames unavailable): {ex.Message}");
        }
    }

    public void Dispose()
    {
        _disposed = true;
        try { if (_duplication != null) _duplication.FrameReady -= OnDuplicationFrame; } catch { }
        try { if (_window != null) _window.FrameReady -= OnWindowFrame; } catch { }
        _duplication?.Dispose();
        _window?.Dispose();
        _duplication = null;
        _window = null;
    }

    [DllImport("user32.dll")] private static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] private static extern bool GetWindowRect(IntPtr hWnd, out Win32Rect rect);

    [StructLayout(LayoutKind.Sequential)]
    private struct Win32Rect { public int Left, Top, Right, Bottom; }
}
