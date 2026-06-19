using System;
using System.Runtime.InteropServices;
using Avalonia;
using Avalonia.Animation;
using Avalonia.Controls;
using Avalonia.Layout;
using Avalonia.Media;
using Avalonia.Threading;

namespace Lag.Services;

/// <summary>
/// ShadowPlay-style on-screen toast: a small topmost card in the top-right corner of the
/// primary display ("Replay saved", "Screenshot saved"). Shown even while the main window
/// is hidden in the tray; never steals focus from the foreground game.
/// </summary>
public static class ToastService
{
    public static void Show(string title, string? subtitle = null)
    {
        Dispatcher.UIThread.Post(() => ShowCore(title, subtitle));
    }

    private static void ShowCore(string title, string? subtitle)
    {
        try
        {
            var text = new StackPanel { Spacing = 2, VerticalAlignment = VerticalAlignment.Center };
            text.Children.Add(new TextBlock
            {
                Text = title,
                FontSize = 13,
                FontWeight = FontWeight.Medium,
                Foreground = new SolidColorBrush(Color.Parse("#E7EAEE")),
            });
            if (!string.IsNullOrWhiteSpace(subtitle))
            {
                text.Children.Add(new TextBlock
                {
                    Text = subtitle,
                    FontSize = 11,
                    Foreground = new SolidColorBrush(Color.Parse("#9AA3AD")),
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
                Kind = Material.Icons.MaterialIconKind.CheckCircle,
                Width = 20,
                Height = 20,
                Foreground = new SolidColorBrush(Color.Parse("#22D3EE")),
                VerticalAlignment = VerticalAlignment.Center,
            });
            row.Children.Add(text);

            var card = new Border
            {
                Background = new SolidColorBrush(Color.Parse("#F0181C23")),
                BorderBrush = new SolidColorBrush(Color.Parse("#1FFFFFFF")),
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

                // Visible ~2.4s, then fade out and close.
                DispatcherTimer.RunOnce(() =>
                {
                    toast.Opacity = 0;
                    DispatcherTimer.RunOnce(toast.Close, TimeSpan.FromMilliseconds(260));
                }, TimeSpan.FromMilliseconds(2400));
            };

            toast.Show();
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Toast] failed: {ex.Message}");
        }
    }

    private const int GWL_EXSTYLE = -20;
    private const int WS_EX_TOPMOST = 0x00000008;
    private const int WS_EX_TOOLWINDOW = 0x00000080;
    private const int WS_EX_NOACTIVATE = 0x08000000;

    [DllImport("user32.dll", SetLastError = true)]
    private static extern int GetWindowLong(IntPtr hWnd, int nIndex);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern int SetWindowLong(IntPtr hWnd, int nIndex, int dwNewLong);
}
