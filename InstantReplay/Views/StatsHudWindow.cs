using System;
using System.Runtime.InteropServices;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Layout;
using Avalonia.Media;
using Avalonia.Threading;
using Lag.Services.VfrCapture;

namespace Lag.Views;

/// <summary>
/// The LIVE Steam-style performance HUD: a tiny always-on-top, click-through window showing
/// FPS / CPU / GPU / RAM in real time while playing (borderless/windowed games; exclusive
/// fullscreen bypasses the desktop compositor and can't show it). Excluded from screen
/// capture (WDA_EXCLUDEFROMCAPTURE) so it never leaks into the user's recordings.
/// FPS comes from the capture engine's frame counter — while the replay buffer runs, that IS
/// the game's present rate; with recording off it shows "--".
/// </summary>
public sealed class StatsHudWindow : Window
{
    private readonly TextBlock _text;
    private readonly SystemStatsSampler _sampler = new();
    private readonly DispatcherTimer _timer;
    private readonly Func<long> _frames;
    private long _lastFrames = -1;
    private DateTime _lastAt = DateTime.UtcNow;
    private int _detail = 1;
    private double _xf = 0.5, _yf = 0.02;

    public StatsHudWindow(Func<long> engineFrameCounter)
    {
        _frames = engineFrameCounter;

        SystemDecorations = SystemDecorations.None;
        TransparencyLevelHint = new[] { WindowTransparencyLevel.Transparent };
        Background = null;
        Topmost = true;
        ShowInTaskbar = false;
        CanResize = false;
        ShowActivated = false;
        Focusable = false;
        SizeToContent = SizeToContent.WidthAndHeight;

        _text = new TextBlock
        {
            Foreground = Brushes.White,
            FontWeight = FontWeight.SemiBold,
            FontSize = 15,
            VerticalAlignment = VerticalAlignment.Center,
        };
        Content = new Border
        {
            Background = new SolidColorBrush(Color.Parse("#D10E1116")),
            BorderBrush = new SolidColorBrush(Color.Parse("#33FFFFFF")),
            BorderThickness = new Thickness(1),
            CornerRadius = new CornerRadius(8),
            Padding = new Thickness(12, 5),
            Child = _text,
        };

        _timer = new DispatcherTimer { Interval = TimeSpan.FromSeconds(1) };
        _timer.Tick += (_, _) =>
        {
            // Exclusive-fullscreen games bypass the compositor: the HUD can't be seen there,
            // and a topmost window (re)appearing over them MINIMIZES the game (the old overlay
            // pain). Same policy as the toasts: hide while exclusive fullscreen is running.
            if (IsExclusiveFullscreen())
            {
                if (IsVisible) Hide();
                return;
            }
            if (!IsVisible) Show();
            UpdateText();
            Reposition();
        };

        Opened += (_, _) =>
        {
            MakeClickThroughAndUncapturable();
            UpdateText();
            Reposition();
            _timer.Start();
        };
        Closed += (_, _) => { _timer.Stop(); _sampler.Dispose(); };
    }

    /// <summary>Live-applies the settings (detail level, size, position fractions).</summary>
    public void Apply(int detail, double scale, double xf, double yf)
    {
        _detail = Math.Clamp(detail, 0, 2);
        _text.FontSize = 10 + Math.Clamp(scale, 0.03, 0.10) * 140;   // 0.03..0.10 → ~14..24 px
        _xf = xf; _yf = yf;
        UpdateText();
        Reposition();
    }

    private void UpdateText()
    {
        // FPS from the engine counter delta (handles session resets via the max-0 clamp).
        long frames = _frames();
        var now = DateTime.UtcNow;
        double sec = Math.Max(0.2, (now - _lastAt).TotalSeconds);
        string fps = _lastFrames >= 0 && frames >= _lastFrames
            ? Math.Round((frames - _lastFrames) / sec).ToString()
            : "--";
        if (fps == "0") fps = "--";   // buffer idle → no meaningful rate
        _lastFrames = frames; _lastAt = now;

        var s = _sampler.Current;
        var inv = System.Globalization.CultureInfo.InvariantCulture;
        _text.Text = _detail switch
        {
            0 => $"{fps} FPS",
            1 => $"{fps} FPS   CPU {s.CpuPercent}%   GPU {s.GpuPercent}%",
            _ => string.Format(inv, "{0} FPS   CPU {1}%   GPU {2}%   RAM {3:0.0}/{4:0.0} GB",
                               fps, s.CpuPercent, s.GpuPercent, s.RamUsedGb, s.RamTotalGb),
        };
    }

    private void Reposition()
    {
        var screen = Screens.Primary ?? (Screens.All.Count > 0 ? Screens.All[0] : null);
        if (screen == null) return;
        var b = screen.Bounds;
        var size = PixelSize.FromSize(Bounds.Size, screen.Scaling);
        Position = new PixelPoint(
            b.X + (int)(_xf * Math.Max(0, b.Width - size.Width)),
            b.Y + (int)(_yf * Math.Max(0, b.Height - size.Height)));
    }

    /// <summary>Click-through + no-activate (never steals game input) + invisible to capture.</summary>
    private void MakeClickThroughAndUncapturable()
    {
        try
        {
            var hwnd = TryGetPlatformHandle()?.Handle ?? IntPtr.Zero;
            if (hwnd == IntPtr.Zero) return;
            const int GWL_EXSTYLE = -20;
            const long WS_EX_TRANSPARENT = 0x20, WS_EX_LAYERED = 0x80000, WS_EX_NOACTIVATE = 0x08000000, WS_EX_TOOLWINDOW = 0x80;
            long ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE).ToInt64();
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new IntPtr(ex | WS_EX_TRANSPARENT | WS_EX_LAYERED | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW));
            SetWindowDisplayAffinity(hwnd, 0x11);   // WDA_EXCLUDEFROMCAPTURE — never in recordings
        }
        catch (Exception ex) { Console.WriteLine($"[StatsHud] window styling failed: {ex.Message}"); }
    }

    /// <summary>True while any exclusive-fullscreen D3D app runs (the toasts use the same signal).</summary>
    private static bool IsExclusiveFullscreen()
    {
        try
        {
            return SHQueryUserNotificationState(out int state) == 0
                   && state == 3;   // QUNS_RUNNING_D3D_FULL_SCREEN
        }
        catch { return false; }
    }

    [DllImport("shell32.dll")]
    private static extern int SHQueryUserNotificationState(out int pquns);

    [DllImport("user32.dll", EntryPoint = "GetWindowLongPtrW")]
    private static extern IntPtr GetWindowLongPtrW(IntPtr hWnd, int nIndex);
    [DllImport("user32.dll", EntryPoint = "SetWindowLongPtrW")]
    private static extern IntPtr SetWindowLongPtrW(IntPtr hWnd, int nIndex, IntPtr dwNewLong);
    [DllImport("user32.dll")]
    private static extern bool SetWindowDisplayAffinity(IntPtr hWnd, uint dwAffinity);
}
