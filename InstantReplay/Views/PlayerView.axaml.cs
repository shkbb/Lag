using System;
using System.ComponentModel;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Threading;
using Lag.ViewModels;

namespace Lag.Views;

public partial class PlayerView : UserControl
{
    private MainViewModel? _mainVm;
    private PlayerViewModel? _playerVm;

    // Fullscreen: hide the mouse cursor after a short idle (like a video player), reveal it on move.
    private static readonly Cursor HiddenCursor = new(StandardCursorType.None);
    private readonly DispatcherTimer _cursorIdleTimer = new() { Interval = TimeSpan.FromSeconds(2.5) };

    public PlayerView()
    {
        InitializeComponent();

        AttachedToVisualTree += OnAttached;
        DetachedFromVisualTree += OnDetached;

        _cursorIdleTimer.Tick += OnCursorIdle;

        // Fullscreen: the control bar hides until the cursor approaches the bottom edge,
        // the replays panel — until it approaches the right edge. Listening on the root
        // grid keeps the overlays open while the pointer is over them (events bubble).
        PlayerArea.PointerMoved += OnAreaPointerMoved;
        PlayerArea.PointerExited += OnAreaPointerExited;
    }

    /// <summary>Height of the bottom zone (in DIPs) that reveals the controls in fullscreen.</summary>
    private const double RevealZoneHeight = 140;

    /// <summary>Width of the right-edge zone (in DIPs) that reveals the replays panel in fullscreen.</summary>
    private const double ReplaysZoneWidth = 280;

    private void SetControlsVisible(bool visible)
    {
        ControlsBar.Classes.Set("hidden", !visible);
        ControlsBar.IsHitTestVisible = visible;
    }

    private void SetReplaysVisible(bool visible)
    {
        ReplayPanel.Classes.Set("hiddenR", !visible);
        ReplayPanel.IsHitTestVisible = visible;
    }

    /// <summary>Restores the cursor and (re)arms the idle hide while in fullscreen.</summary>
    private void WakeCursor()
    {
        PlayerArea.Cursor = Cursor.Default;
        _cursorIdleTimer.Stop();
        if (_mainVm?.IsFullscreen ?? false) _cursorIdleTimer.Start();
    }

    private void OnCursorIdle(object? sender, EventArgs e)
    {
        _cursorIdleTimer.Stop();
        if (_mainVm?.IsFullscreen ?? false) PlayerArea.Cursor = HiddenCursor;
    }

    private void OnAreaPointerMoved(object? sender, PointerEventArgs e)
    {
        WakeCursor();

        if (!(_mainVm?.IsFullscreen ?? false))
        {
            SetControlsVisible(true);
            SetReplaysVisible(true);
            return;
        }

        // The bottom strip belongs to the control bar; the right zone stops above it so the
        // two overlays (and their reveal zones) never fight over the bottom-right corner.
        var p = e.GetPosition(PlayerArea);
        bool inBottomZone = p.Y >= PlayerArea.Bounds.Height - RevealZoneHeight;
        bool inRightZone = p.X >= PlayerArea.Bounds.Width - ReplaysZoneWidth && !inBottomZone;
        SetControlsVisible(inBottomZone);
        SetReplaysVisible(inRightZone);
    }

    private void OnAreaPointerExited(object? sender, PointerEventArgs e)
    {
        if (_mainVm?.IsFullscreen ?? false)
        {
            SetControlsVisible(false);
            SetReplaysVisible(false);
        }
    }

    private void OnAttached(object? sender, EventArgs e)
    {
        if (DataContext is PlayerViewModel vm)
        {
            _playerVm = vm;

            // Video frames arrive in an Avalonia bitmap — wire it to the Image and repaint
            // on every rendered frame (the bitmap reference doesn't change per frame).
            vm.VideoRenderer.BitmapChanged += OnVideoBitmapChanged;
            vm.VideoRenderer.FrameRendered += OnVideoFrameRendered;
            vm.PropertyChanged += OnPlayerVmPropertyChanged;
            UpdateVideoSource();

            vm.StartPlayback();
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
        _cursorIdleTimer.Stop();

        if (_playerVm != null)
        {
            _playerVm.VideoRenderer.BitmapChanged -= OnVideoBitmapChanged;
            _playerVm.VideoRenderer.FrameRendered -= OnVideoFrameRendered;
            _playerVm.PropertyChanged -= OnPlayerVmPropertyChanged;
            _playerVm.StopPlayback();
            _playerVm = null;
        }
        VideoImage.Source = null;

        if (_mainVm != null)
        {
            _mainVm.PropertyChanged -= OnMainVmPropertyChanged;
            _mainVm = null;
        }
    }

    private void OnVideoBitmapChanged() => UpdateVideoSource();

    private void OnVideoFrameRendered() => VideoImage.InvalidateVisual();

    private void OnPlayerVmPropertyChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName is nameof(PlayerViewModel.StillImage) or nameof(PlayerViewModel.IsViewingImage))
            UpdateVideoSource();
    }

    /// <summary>The video area shows either the live VLC frame bitmap or a screenshot.</summary>
    private void UpdateVideoSource()
    {
        if (_playerVm == null) return;
        VideoImage.Source = _playerVm.IsViewingImage
            ? _playerVm.StillImage
            : _playerVm.VideoRenderer.Bitmap;
    }

    /// <summary>Click anywhere on the video toggles play/pause (now a plain Avalonia event).</summary>
    private void OnVideoSurfacePressed(object? sender, PointerPressedEventArgs e)
    {
        if (_playerVm == null) return;
        if (!e.GetCurrentPoint(this).Properties.IsLeftButtonPressed) return;

        if (_playerVm.PlayPauseCommand.CanExecute(null))
            _playerVm.PlayPauseCommand.Execute(null);

        Focus(); // keep the Space shortcut working
        e.Handled = true;
    }

    private void OnMainVmPropertyChanged(object? sender, PropertyChangedEventArgs e)
    {
        if (e.PropertyName == nameof(MainViewModel.IsFullscreen))
            UpdateFullscreenLayout();
    }

    /// <summary>In fullscreen the video is full-bleed and square; windowed it matches the design frame.</summary>
    private void UpdateFullscreenLayout()
    {
        bool fs = _mainVm?.IsFullscreen ?? false;

        PlayerArea.Margin = fs ? new Thickness(0) : new Thickness(24, 20, 24, 24);
        VideoFrame.CornerRadius = new CornerRadius(fs ? 0 : 12);

        // The replays panel switches between an inline column (windowed) and an overlay
        // pinned to the right edge of the video (fullscreen).
        if (fs)
        {
            Grid.SetColumn(ReplayPanel, 0);
            ReplayPanel.HorizontalAlignment = Avalonia.Layout.HorizontalAlignment.Right;
            // Large bottom margin keeps the panel clear of the floating control bar
            // (so the fullscreen-exit button is never covered).
            ReplayPanel.Margin = new Thickness(0, 12, 12, 118);
        }
        else
        {
            Grid.SetColumn(ReplayPanel, 1);
            ReplayPanel.HorizontalAlignment = Avalonia.Layout.HorizontalAlignment.Stretch;
            ReplayPanel.Margin = new Thickness(16, 0, 0, 0);
        }

        // Entering fullscreen: overlays start hidden (slide in on cursor approach).
        // Leaving it: everything visible again.
        SetControlsVisible(!fs);
        SetReplaysVisible(!fs);

        // Cursor: armed to auto-hide in fullscreen, always shown windowed.
        if (fs) WakeCursor();
        else { _cursorIdleTimer.Stop(); PlayerArea.Cursor = Cursor.Default; }

        // Pull keyboard focus back to the view so SPACE keeps meaning play/pause.
        Focus();
    }
}
