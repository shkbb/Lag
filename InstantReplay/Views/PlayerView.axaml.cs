using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Threading;
using Lag.ViewModels;

namespace Lag.Views;

public partial class PlayerView : UserControl
{
    // ── Win32 interop: round the native VLC window at the OS level ──
    // Avalonia visuals can't clip a NativeControlHost (it's composited above the Avalonia surface),
    // so the only real way to round the video is to apply a rounded HRGN to the native window.
    [DllImport("gdi32.dll", SetLastError = true)]
    private static extern IntPtr CreateRoundRectRgn(int x1, int y1, int x2, int y2, int w, int h);

    [DllImport("user32.dll", SetLastError = true)]
    private static extern int SetWindowRgn(IntPtr hWnd, IntPtr hRgn, bool bRedraw);

    [DllImport("gdi32.dll")]
    private static extern bool DeleteObject(IntPtr hObject);

    private const double CornerRadiusDip = 14.0;

    private MainViewModel? _mainVm;
    private PlayerViewModel? _playerVm;

    public PlayerView()
    {
        InitializeComponent();

        AttachedToVisualTree += OnAttached;
        DetachedFromVisualTree += OnDetached;

        // Re-round the native window whenever the video surface is resized.
        MainVideoView.SizeChanged += (_, _) => ApplyVideoCornerRegion();
    }

    private void OnAttached(object? sender, EventArgs e)
    {
        if (DataContext is PlayerViewModel vm)
        {
            _playerVm = vm;
            vm.PropertyChanged += OnPlayerVmPropertyChanged;
            vm.StartPlayback();
            MainVideoView.MediaPlayer = vm.Player;
        }

        // Observe global fullscreen (the Window's DataContext is MainViewModel).
        if (TopLevel.GetTopLevel(this) is Window { DataContext: MainViewModel mainVm })
        {
            _mainVm = mainVm;
            mainVm.PropertyChanged += OnMainVmPropertyChanged;
            UpdateFullscreenLayout();
        }

        // Grab keyboard focus so the Space KeyBinding works as soon as the view is shown.
        Focus();
    }

    private void OnDetached(object? sender, EventArgs e)
    {
        MainVideoView.MediaPlayer = null;
        if (_playerVm != null)
        {
            _playerVm.PropertyChanged -= OnPlayerVmPropertyChanged;
            _playerVm.StopPlayback();
            _playerVm = null;
        }
        if (_mainVm != null)
        {
            _mainVm.PropertyChanged -= OnMainVmPropertyChanged;
            _mainVm = null;
        }
    }

    private void OnPlayerVmPropertyChanged(object? sender, PropertyChangedEventArgs e)
    {
        // The native vout is created when playback starts — re-apply the region a tick later.
        if (e.PropertyName == nameof(PlayerViewModel.IsPlaying))
            Dispatcher.UIThread.Post(ApplyVideoCornerRegion, DispatcherPriority.Background);
    }

    private void OnMainVmPropertyChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName == nameof(MainViewModel.IsFullscreen))
            UpdateFullscreenLayout();
    }

    /// <summary>In fullscreen the video is full-bleed and square; windowed it sits in a rounded glass frame.</summary>
    private void UpdateFullscreenLayout()
    {
        bool fs = _mainVm?.IsFullscreen ?? false;

        PlayerArea.Margin = fs ? new Thickness(0) : new Thickness(14);
        VideoFrame.CornerRadius = new CornerRadius(fs ? 0 : 16);
        VideoFrame.BorderThickness = new Thickness(fs ? 0 : 1);

        ApplyVideoCornerRegion();
    }

    /// <summary>
    /// Applies (or clears) a rounded rectangular region on the native VLC window so its corners
    /// match the glass frame. Cleared in fullscreen (square, full-screen video).
    /// </summary>
    private void ApplyVideoCornerRegion()
    {
        try
        {
            IntPtr hwnd = MainVideoView.MediaPlayer?.Hwnd ?? IntPtr.Zero;
            if (hwnd == IntPtr.Zero) return;

            bool fs = _mainVm?.IsFullscreen ?? false;
            double scaling = TopLevel.GetTopLevel(this)?.RenderScaling ?? 1.0;

            int w = (int)Math.Round(MainVideoView.Bounds.Width * scaling);
            int h = (int)Math.Round(MainVideoView.Bounds.Height * scaling);

            if (fs || w <= 0 || h <= 0)
            {
                // No rounding in fullscreen (or before layout) — clear any existing region.
                SetWindowRgn(hwnd, IntPtr.Zero, true);
                return;
            }

            int diameter = (int)Math.Round(CornerRadiusDip * 2 * scaling);
            IntPtr rgn = CreateRoundRectRgn(0, 0, w + 1, h + 1, diameter, diameter);

            // SetWindowRgn takes ownership of the region on success; only free it on failure.
            if (SetWindowRgn(hwnd, rgn, true) == 0)
                DeleteObject(rgn);
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[PlayerView] Failed to round native video: {ex.Message}");
        }
    }

    /// <summary>
    /// Toggles play/pause when the user clicks the video surface. This works because libVLC mouse
    /// input is disabled in PlayerViewModel, letting the native window pass clicks to this overlay.
    /// </summary>
    private void OnVideoSurfacePressed(object? sender, PointerPressedEventArgs e)
    {
        Focus();
        if (DataContext is PlayerViewModel vm && vm.PlayPauseCommand.CanExecute(null))
            vm.PlayPauseCommand.Execute(null);
    }
}
