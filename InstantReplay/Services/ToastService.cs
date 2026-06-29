using System;
using System.Runtime.InteropServices;
using Avalonia;
using Avalonia.Animation;
using Avalonia.Controls;
using Avalonia.Layout;
using Avalonia.Media;
using Avalonia.Styling;
using Avalonia.Threading;
using Material.Icons;

namespace Lag.Services;

/// <summary>
/// On-screen toast: a small topmost card in the top-right corner of the primary display — used for
/// "Replay saved" / "Screenshot saved", recording start/stop, and capture-target (game↔desktop)
/// changes. Shown even while the main window is hidden in the tray; never steals focus from the
/// foreground game. Colours follow the active Light/Dark theme. ⚠️ Over a TRUE exclusive-fullscreen
/// game it is suppressed (sound-only) — a topmost window would yank the game out of fullscreen;
/// borderless / windowed games and the desktop still get the visual toast.
/// </summary>
public static class ToastService
{
    private readonly record struct ToastReq(string Title, string? Subtitle, MaterialIconKind Icon, string AccentKey);

    // One-at-a-time queue: toasts play sequentially (one fully shown + faded out before the next),
    // never stacked on top of each other. All access is on the UI thread, so no lock is needed.
    private static readonly System.Collections.Generic.Queue<ToastReq> _queue = new();
    private static bool _active;

    /// <summary>Queues a toast. <paramref name="icon"/> + <paramref name="accentKey"/> (a Palette
    /// brush resource key) colour the leading glyph so each event reads at a glance.</summary>
    public static void Show(string title, string? subtitle = null,
                            MaterialIconKind icon = MaterialIconKind.CheckCircle, string accentKey = "Accent")
    {
        Dispatcher.UIThread.Post(() =>
        {
            if (_queue.Count >= 6) _queue.Dequeue();   // bound a burst; drop the oldest pending
            _queue.Enqueue(new ToastReq(title, subtitle, icon, accentKey));
            Pump();
        });
    }

    /// <summary>Shows the next queued toast if none is currently on screen.</summary>
    private static void Pump()
    {
        if (_active || _queue.Count == 0) return;
        _active = true;
        ShowCore(_queue.Dequeue());
    }

    /// <summary>Marks the current toast finished and starts the next (next UI tick, to avoid reentrancy).</summary>
    private static void Done()
    {
        _active = false;
        Dispatcher.UIThread.Post(Pump);
    }

    private static void ShowCore(ToastReq req)
    {
        var (title, subtitle, icon, accentKey) = req;
        try
        {
            // Don't pop a visual overlay over an EXCLUSIVE-fullscreen game: a topmost window yanks it
            // out of fullscreen (Windows behaviour — not fixable without an in-game hook, which we avoid
            // for anti-cheat safety). The save SOUND still plays, so there's still confirmation. We key
            // off Windows' OWN "an exclusive-fullscreen D3D app is running" signal, so BORDERLESS /
            // windowed games AND the desktop still get the visual toast (they compose fine under it) —
            // only a true exclusive-fullscreen game falls back to sound-only.
            if (IsExclusiveFullscreen())
            {
                Console.WriteLine("[Toast] exclusive-fullscreen app running — sound-only (skipped visual toast).");
                Done();   // don't stall the queue — move straight to the next
                return;
            }

            var text = new StackPanel { Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
            text.Children.Add(new TextBlock
            {
                Text = title,
                FontSize = 13,
                FontWeight = FontWeight.Medium,
                Foreground = Res("FgBrush", "#E7EAEE"),
            });
            if (!string.IsNullOrWhiteSpace(subtitle))
            {
                text.Children.Add(new TextBlock
                {
                    Text = subtitle,
                    FontSize = 11,
                    Foreground = Res("Fg2Brush", "#9AA3AD"),
                    MaxWidth = 260,
                    TextTrimming = TextTrimming.CharacterEllipsis,
                });
            }

            var row = new StackPanel
            {
                Orientation = Orientation.Horizontal,
                Spacing = 10,
                VerticalAlignment = VerticalAlignment.Center,
            };
            row.Children.Add(new Material.Icons.Avalonia.MaterialIcon
            {
                Kind = icon,
                Width = 20,
                Height = 20,
                Foreground = Res(accentKey, "#22D3EE"),
                VerticalAlignment = VerticalAlignment.Center,
            });
            row.Children.Add(text);

            var card = new Border
            {
                Background = CardBg(),
                BorderBrush = Res("PopupBorderBrush", "#1FFFFFFF"),
                BorderThickness = new Thickness(1),
                CornerRadius = new CornerRadius(12),
                Padding = new Thickness(16, 11),
                Child = row,
            };

            var toast = new Window
            {
                SystemDecorations = SystemDecorations.None,
                TransparencyLevelHint = new[] { WindowTransparencyLevel.Transparent },
                Background = Brushes.Transparent,
                Topmost = true,
                ShowInTaskbar = false,
                ShowActivated = false,   // must not steal focus from the game
                CanResize = false,
                Focusable = false,
                SizeToContent = SizeToContent.WidthAndHeight,
                Content = card,
                Opacity = 0,
                Transitions = new Transitions
                {
                    new DoubleTransition { Property = Visual.OpacityProperty, Duration = TimeSpan.FromMilliseconds(220) },
                },
            };

            toast.Opened += (_, _) =>
            {
                // True non-activating overlay: stamp WS_EX_NOACTIVATE | TOOLWINDOW | TOPMOST on the
                // native window so it can NEVER take foreground — a borderless game keeps running
                // (ShowActivated=false alone still let Windows briefly foreground it, which yanked
                // exclusive/borderless games out and minimized them).
                if (toast.TryGetPlatformHandle()?.Handle is { } hwnd && hwnd != IntPtr.Zero)
                {
                    int ex = GetWindowLong(hwnd, GWL_EXSTYLE);
                    SetWindowLong(hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW | WS_EX_TOPMOST);
                }

                // Pin to the top-right of the primary display's working area.
                var screen = toast.Screens.Primary ?? toast.Screens.ScreenFromWindow(toast);
                if (screen != null)
                {
                    var wa = screen.WorkingArea;
                    int w = (int)Math.Ceiling(toast.Bounds.Width * screen.Scaling);
                    toast.Position = new PixelPoint(wa.X + wa.Width - w - 24, wa.Y + 24);
                }

                toast.Opacity = 1;

                // Visible ~2.4s, then fade out, close, and release the queue for the next toast.
                DispatcherTimer.RunOnce(() =>
                {
                    toast.Opacity = 0;
                    DispatcherTimer.RunOnce(() => { try { toast.Close(); } catch { } Done(); }, TimeSpan.FromMilliseconds(260));
                }, TimeSpan.FromMilliseconds(2400));
            };

            toast.Show();
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Toast] failed: {ex.Message}");
            Done();   // never let a failed toast jam the queue
        }
    }

    /// <summary>Resolves a Palette brush by key for the ACTIVE theme variant (Light/Dark), so the toast
    /// matches the app. Falls back to a hex literal if the resource is missing.</summary>
    private static IBrush Res(string key, string fallbackHex)
    {
        var app = Application.Current;
        if (app != null && app.TryGetResource(key, app.ActualThemeVariant, out var v) && v is IBrush b) return b;
        return new SolidColorBrush(Color.Parse(fallbackHex));
    }

    /// <summary>The card background for the active theme, kept at the toast's slight translucency.</summary>
    private static IBrush CardBg()
    {
        var app = Application.Current;
        Color c = Color.Parse("#181C23");
        if (app != null && app.TryGetResource("CardBrush", app.ActualThemeVariant, out var v) && v is ISolidColorBrush b)
            c = b.Color;
        return new SolidColorBrush(Color.FromArgb(0xF0, c.R, c.G, c.B));
    }

    /// <summary>True when an exclusive-fullscreen Direct3D app is running — Windows' OWN notification
    /// signal (the same one the OS uses to mute its toasts). Borderless / windowed games and the
    /// desktop do NOT trip this (they're DWM-composed), so they still get the visual toast; only a
    /// true exclusive-fullscreen game falls back to sound-only.</summary>
    private static bool IsExclusiveFullscreen()
    {
        try
        {
            return SHQueryUserNotificationState(out int state) == 0 && state == QUNS_RUNNING_D3D_FULL_SCREEN;
        }
        catch { return false; }
    }

    private const int GWL_EXSTYLE = -20;
    private const int WS_EX_TOPMOST = 0x00000008;
    private const int WS_EX_TOOLWINDOW = 0x00000080;
    private const int WS_EX_NOACTIVATE = 0x08000000;
    private const int QUNS_RUNNING_D3D_FULL_SCREEN = 3;   // a full-screen (exclusive) D3D app is running

    [DllImport("user32.dll", SetLastError = true)]
    private static extern int GetWindowLong(IntPtr hWnd, int nIndex);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern int SetWindowLong(IntPtr hWnd, int nIndex, int dwNewLong);

    [DllImport("shell32.dll")]
    private static extern int SHQueryUserNotificationState(out int pquns);
}
