using System;
using System.ComponentModel;
using System.Linq;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Interactivity;
using Avalonia.Threading;
using Lag.ViewModels;

namespace Lag.Views;

public partial class EditorView : UserControl
{
    private EditorViewModel? _vm;

    /// <summary>What the active pointer drag is editing on the timeline.</summary>
    private enum DragMode { None, Scrub, TrimStart, TrimEnd }

    private DragMode _drag = DragMode.None;

    // ── Smooth playhead: the VLC position arrives every 250ms; a 30fps UI timer
    //    interpolates between ticks using wall time × playback rate. ──
    private DispatcherTimer? _playheadTimer;
    private double _lastVmPosition;
    private DateTime _lastVmPositionAt = DateTime.UtcNow;

    public EditorView()
    {
        InitializeComponent();

        AttachedToVisualTree += OnAttached;
        DetachedFromVisualTree += OnDetached;

        TimelineOverlay.SizeChanged += (_, _) => UpdateOverlay();
        TimelineOverlay.PointerPressed += OnTimelinePointerPressed;
        TimelineOverlay.PointerMoved += OnTimelinePointerMoved;
        TimelineOverlay.PointerReleased += OnTimelinePointerReleased;
    }

    private void OnAttached(object? sender, EventArgs e)
    {
        if (DataContext is EditorViewModel vm)
        {
            _vm = vm;
            vm.PropertyChanged += OnVmPropertyChanged;

            VideoImage.Source = vm.VideoRenderer.Bitmap;
            vm.VideoRenderer.BitmapChanged += OnVideoBitmapChanged;
            vm.VideoRenderer.FrameRendered += OnVideoFrameRendered;

            vm.StartPreview();

            _playheadTimer ??= new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(33) };
            _playheadTimer.Tick -= OnPlayheadTick;
            _playheadTimer.Tick += OnPlayheadTick;
            _playheadTimer.Start();

            Dispatcher.UIThread.Post(UpdateOverlay, DispatcherPriority.Background);
        }

        Focus();
    }

    private void OnDetached(object? sender, EventArgs e)
    {
        _playheadTimer?.Stop();

        if (_vm != null)
        {
            _vm.VideoRenderer.BitmapChanged -= OnVideoBitmapChanged;
            _vm.VideoRenderer.FrameRendered -= OnVideoFrameRendered;
            _vm.PropertyChanged -= OnVmPropertyChanged;
            _vm.StopPreview();
            _vm = null;
        }
        VideoImage.Source = null;
    }

    private void OnVideoBitmapChanged()
    {
        if (_vm != null)
            VideoImage.Source = _vm.VideoRenderer.Bitmap;
    }

    private void OnVideoFrameRendered() => VideoImage.InvalidateVisual();

    /// <summary>Click on the preview toggles play/pause.</summary>
    private void OnVideoSurfacePressed(object? sender, PointerPressedEventArgs e)
    {
        if (_vm == null) return;
        if (!e.GetCurrentPoint(this).Properties.IsLeftButtonPressed) return;

        if (_vm.PlayPauseCommand.CanExecute(null))
            _vm.PlayPauseCommand.Execute(null);

        Focus();
        e.Handled = true;
    }

    /// <summary>Right tools panel tabs (Clip / Filters).</summary>
    private void OnToolsTabClick(object? sender, RoutedEventArgs e)
    {
        if (_vm != null && sender is Button { Tag: string tag } && int.TryParse(tag, out int idx))
            _vm.SelectedToolsTab = idx;
    }

    /// <summary>Shift+click on an audio row toggles its selection (for linked trimming).</summary>
    private void OnTrackRowPressed(object? sender, PointerPressedEventArgs e)
    {
        if (!e.KeyModifiers.HasFlag(KeyModifiers.Shift)) return;
        if ((sender as Control)?.DataContext is EditorAudioTrack track)
        {
            track.IsSelected = !track.IsSelected;
            e.Handled = true;
        }
    }

    private void OnVmPropertyChanged(object? sender, PropertyChangedEventArgs e)
    {
        switch (e.PropertyName)
        {
            case nameof(EditorViewModel.Position):
                if (_vm != null)
                {
                    _lastVmPosition = _vm.Position;
                    _lastVmPositionAt = DateTime.UtcNow;
                }
                Dispatcher.UIThread.Post(UpdateOverlay, DispatcherPriority.Render);
                break;
            case nameof(EditorViewModel.TrimStartSec):
            case nameof(EditorViewModel.TrimEndSec):
            case nameof(EditorViewModel.DurationSec):
                Dispatcher.UIThread.Post(UpdateOverlay, DispatcherPriority.Render);
                break;
        }
    }

    /// <summary>30fps interpolation between coarse VLC position updates → buttery playhead.</summary>
    private void OnPlayheadTick(object? sender, EventArgs e)
    {
        if (_vm == null || !_vm.IsPlaying || _drag == DragMode.Scrub) return;

        double w = TimelineOverlay.Bounds.Width;
        if (w <= 0 || _vm.DurationSec <= 0) return;

        double elapsed = (DateTime.UtcNow - _lastVmPositionAt).TotalSeconds;
        double predicted = _lastVmPosition + elapsed * _vm.EffectiveSpeed / _vm.DurationSec;

        // The playhead never leaves the trimmed range.
        double lo = _vm.TrimStartSec / _vm.DurationSec;
        double hi = _vm.TrimEndSec / _vm.DurationSec;
        double px = Math.Clamp(predicted, lo, hi) * w;

        Canvas.SetLeft(Playhead, Math.Clamp(px - 1, 0, w - 2));
    }

    // ───────────── Timeline overlay ─────────────

    /// <summary>Lays out the shades, trim handles and playhead from the VM state.</summary>
    private void UpdateOverlay()
    {
        if (_vm == null) return;

        double w = TimelineOverlay.Bounds.Width;
        double h = TimelineOverlay.Bounds.Height;
        if (w <= 0 || _vm.DurationSec <= 0) return;

        double sx = _vm.TrimStartSec / _vm.DurationSec * w;
        double ex = _vm.TrimEndSec / _vm.DurationSec * w;
        // The playhead is confined to the trimmed range.
        double px = Math.Clamp(Math.Clamp(_vm.Position, 0, 1) * w, sx, ex);

        LeftShade.Width = Math.Max(0, sx);
        Canvas.SetLeft(RightShade, ex);
        RightShade.Width = Math.Max(0, w - ex);

        Canvas.SetLeft(StartHandle, Math.Clamp(sx, 0, w) - (sx <= 5 ? 0 : 5));
        Canvas.SetLeft(EndHandle, Math.Clamp(ex, 0, w) - (ex >= w - 5 ? 10 : 5));

        // When playing, the 30fps interpolator owns the playhead — don't fight it.
        if (!_vm.IsPlaying || _drag == DragMode.Scrub)
            Canvas.SetLeft(Playhead, Math.Clamp(px - 1, 0, w - 2));

        LeftShade.Height = RightShade.Height = StartHandle.Height = EndHandle.Height = Playhead.Height = h;
    }

    private void OnTimelinePointerPressed(object? sender, PointerPressedEventArgs e)
    {
        if (_vm == null) return;

        // Shift+click selects the video row instead of editing.
        if (e.KeyModifiers.HasFlag(KeyModifiers.Shift))
        {
            _vm.IsVideoSelected = !_vm.IsVideoSelected;
            e.Handled = true;
            return;
        }

        double w = TimelineOverlay.Bounds.Width;
        if (w <= 0) return;

        double x = e.GetPosition(TimelineOverlay).X;
        double sx = _vm.TrimStartSec / _vm.DurationSec * w;
        double ex = _vm.TrimEndSec / _vm.DurationSec * w;

        // Grab the nearer handle when within reach; otherwise scrub the preview.
        _drag = Math.Abs(x - sx) <= 12 && Math.Abs(x - sx) <= Math.Abs(x - ex) ? DragMode.TrimStart
              : Math.Abs(x - ex) <= 12 ? DragMode.TrimEnd
              : DragMode.Scrub;

        e.Pointer.Capture(TimelineOverlay);
        ApplyDrag(x);
        e.Handled = true;
    }

    private void OnTimelinePointerMoved(object? sender, PointerEventArgs e)
    {
        if (_drag == DragMode.None) return;
        ApplyDrag(e.GetPosition(TimelineOverlay).X);
        e.Handled = true;
    }

    private void OnTimelinePointerReleased(object? sender, PointerReleasedEventArgs e)
    {
        _drag = DragMode.None;
        e.Pointer.Capture(null);
    }

    /// <summary>Snaps a candidate second value to nearby audio-track edges (~10px reach).</summary>
    private double SnapToTrackEdges(double sec, double w)
    {
        if (_vm == null || w <= 0) return sec;

        double threshold = 10.0 / w * _vm.DurationSec;
        foreach (var track in _vm.AudioTracks)
        {
            if (Math.Abs(sec - track.StartSec) <= threshold) return track.StartSec;
            if (Math.Abs(sec - track.EndSec) <= threshold) return track.EndSec;
        }
        return sec;
    }

    private void ApplyDrag(double x)
    {
        if (_vm == null) return;

        double w = TimelineOverlay.Bounds.Width;
        if (w <= 0) return;

        double sec = Math.Clamp(x / w, 0, 1) * _vm.DurationSec;

        switch (_drag)
        {
            case DragMode.TrimStart:
            {
                sec = SnapToTrackEdges(sec, w);
                double v = Math.Clamp(sec, 0, Math.Max(0, _vm.TrimEndSec - 0.2));
                _vm.TrimStartSec = v;
                // Linked edit: selected audio rows follow the video edge.
                if (_vm.IsVideoSelected)
                    foreach (var t in _vm.AudioTracks.Where(t => t.IsSelected))
                        t.StartSec = Math.Clamp(v, 0, Math.Max(0, t.EndSec - 0.1));
                break;
            }
            case DragMode.TrimEnd:
            {
                sec = SnapToTrackEdges(sec, w);
                double v = Math.Clamp(sec, Math.Min(_vm.DurationSec, _vm.TrimStartSec + 0.2), _vm.DurationSec);
                _vm.TrimEndSec = v;
                if (_vm.IsVideoSelected)
                    foreach (var t in _vm.AudioTracks.Where(t => t.IsSelected))
                        t.EndSec = Math.Clamp(v, Math.Min(t.ClipDurationSec, t.StartSec + 0.1), t.ClipDurationSec);
                break;
            }
            case DragMode.Scrub:
            {
                // Scrubbing is confined to the trimmed range too.
                double lo = _vm.TrimStartSec / _vm.DurationSec;
                double hi = _vm.TrimEndSec / _vm.DurationSec;
                _vm.Position = Math.Clamp(x / w, lo, hi);
                break;
            }
        }
    }
}
