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

    // Smooth millisecond clock: VLC reports time only every ~100ms, so a display-rate timer
    // interpolates between ticks (wall time × playback speed), like the editor's playhead.
    private readonly DispatcherTimer _timeTimer = new() { Interval = TimeSpan.FromMilliseconds(33) };
    private double _lastTimeSec;
    private DateTime _lastTimeAt = DateTime.UtcNow;

    // While the speed flyout is open the control bar must stay put (it would otherwise auto-hide
    // when the cursor moves onto the popup).
    private bool _controlsPinned;

    public PlayerView()
    {
        InitializeComponent();

        AttachedToVisualTree += OnAttached;
        DetachedFromVisualTree += OnDetached;

        _cursorIdleTimer.Tick += OnCursorIdle;
        _timeTimer.Tick += OnTimeTick;

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
            // Windowed: the control bar reveals only while the cursor is over the video (YouTube-style);
            // the replays column stays put. A pinned bar (speed flyout open) stays visible.
            SetControlsVisible(_controlsPinned || IsPointerOverVideo(e));
            SetReplaysVisible(true);
            return;
        }

        // Fullscreen: the bottom strip belongs to the control bar; the right zone stops above it so the
        // two overlays (and their reveal zones) never fight over the bottom-right corner.
        var p = e.GetPosition(PlayerArea);
        bool inBottomZone = p.Y >= PlayerArea.Bounds.Height - RevealZoneHeight;
        bool inRightZone = p.X >= PlayerArea.Bounds.Width - ReplaysZoneWidth && !inBottomZone;
        SetControlsVisible(_controlsPinned || inBottomZone);
        SetReplaysVisible(inRightZone);
    }

    /// <summary>True when the pointer is currently within the video frame (incl. the control bar,
    /// which lives inside it) — used to reveal the windowed control bar.</summary>
    private bool IsPointerOverVideo(PointerEventArgs e)
    {
        var pt = e.GetPosition(VideoFrame);
        return pt.X >= 0 && pt.Y >= 0
            && pt.X <= VideoFrame.Bounds.Width && pt.Y <= VideoFrame.Bounds.Height;
    }

    private void OnAreaPointerExited(object? sender, PointerEventArgs e)
    {
        // Leaving the player area hides the control bar in both modes — unless it's pinned open
        // by the speed flyout. The replays column stays visible while windowed.
        if (!_controlsPinned) SetControlsVisible(false);
        if (_mainVm?.IsFullscreen ?? false) SetReplaysVisible(false);
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

            _lastTimeSec = vm.PositionSeconds;
            _lastTimeAt = DateTime.UtcNow;
            _timeTimer.Start();
        }

        // Keep the control bar pinned while the playback-speed flyout is open.
        if (SpeedButton.Flyout is { } speedFlyout)
        {
            speedFlyout.Opened += OnSpeedFlyoutOpened;
            speedFlyout.Closed += OnSpeedFlyoutClosed;
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
        _timeTimer.Stop();

        if (SpeedButton.Flyout is { } speedFlyout)
        {
            speedFlyout.Opened -= OnSpeedFlyoutOpened;
            speedFlyout.Closed -= OnSpeedFlyoutClosed;
        }

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
        switch (e.PropertyName)
        {
            case nameof(PlayerViewModel.StillImage):
            case nameof(PlayerViewModel.IsViewingImage):
                UpdateVideoSource();
                break;
            // Resync the smooth clock whenever VLC reports a fresh time or play-state flips.
            case nameof(PlayerViewModel.PositionSeconds):
            case nameof(PlayerViewModel.IsPlaying):
                if (_playerVm != null)
                {
                    _lastTimeSec = _playerVm.PositionSeconds;
                    _lastTimeAt = DateTime.UtcNow;
                }
                break;
        }
    }

    /// <summary>Display-rate interpolation of the playback clock → a smooth millisecond readout.</summary>
    private void OnTimeTick(object? sender, EventArgs e)
    {
        if (_playerVm == null) return;
        double total = _playerVm.DurationSeconds;
        double t = _lastTimeSec;
        if (_playerVm.IsPlaying)
            t += (DateTime.UtcNow - _lastTimeAt).TotalSeconds * Math.Max(0.01, _playerVm.PlaybackSpeed);
        if (total > 0) t = Math.Clamp(t, 0, total);
        TimeText.Text = $"{FormatClock(t)} / {FormatClock(total)}";
    }

    private static string FormatClock(double seconds)
    {
        var ts = TimeSpan.FromSeconds(Math.Max(0, seconds));
        return ts.TotalHours >= 1 ? ts.ToString(@"h\:mm\:ss\.fff") : ts.ToString(@"m\:ss\.fff");
    }

    private void OnSpeedFlyoutOpened(object? sender, EventArgs e)
    {
        _controlsPinned = true;
        SetControlsVisible(true);
    }

    private void OnSpeedFlyoutClosed(object? sender, EventArgs e) => _controlsPinned = false;

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

        // The control bar starts hidden in BOTH modes — it reveals on cursor proximity
        // (over the video windowed; near the bottom edge in fullscreen). The replays column
        // is shown windowed and hidden (reveal-on-approach) in fullscreen.
        SetControlsVisible(false);
        SetReplaysVisible(!fs);

        // Cursor: armed to auto-hide in fullscreen, always shown windowed.
        if (fs) WakeCursor();
        else { _cursorIdleTimer.Stop(); PlayerArea.Cursor = Cursor.Default; }

        // Pull keyboard focus back to the view so SPACE keeps meaning play/pause.
        Focus();
    }
}
