using System;
using System.ComponentModel;
using System.Linq;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Interactivity;
using Avalonia.Layout;
using Avalonia.Media;
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
        // The reframe overlay (output frame + the video's transform rect) is re-laid-out whenever
        // the stage resizes.
        VideoSurface.SizeChanged += (_, _) => UpdateReframeOverlay();
        TimelineOverlay.PointerPressed += OnTimelinePointerPressed;
        TimelineOverlay.PointerMoved += OnTimelinePointerMoved;
        TimelineOverlay.PointerReleased += OnTimelinePointerReleased;

        // Clicking ANYWHERE that isn't the video deselects the reframe box. Tunnel + handledEventsToo
        // so it fires before/regardless of inner controls (tools panel, timeline, buttons…).
        AddHandler(PointerPressedEvent, OnRootPointerPressed, RoutingStrategies.Tunnel, handledEventsToo: true);
    }

    /// <summary>Deselects the reframe box when the press lands off the video (empty space / a control).</summary>
    private void OnRootPointerPressed(object? sender, PointerPressedEventArgs e)
    {
        if (_vm == null || CropMode || !_reframeSelected) return;
        if (HitTest(e.GetPosition(CropOverlay)) == CropDrag.None)
        {
            _reframeSelected = false;
            UpdateReframeOverlay();
        }
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
            vm.ReframeChanged += UpdateReframeOverlay;
            vm.CutsChanged += RebuildCutBands;
            vm.TextsChanged += OnTextsChanged;

            _reframeSelected = false;
            vm.StartPreview();
            UpdateReframeOverlay();
            RebuildCutBands();
            RebuildTextOverlays();

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
            _vm.ReframeChanged -= UpdateReframeOverlay;
            _vm.CutsChanged -= RebuildCutBands;
            _vm.TextsChanged -= OnTextsChanged;
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

    // Triple-buffered renderer: the presented bitmap instance rotates every frame — re-read it.
    private void OnVideoFrameRendered()
    {
        if (_vm != null)
            VideoImage.Source = _vm.VideoRenderer.Bitmap;
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
            case nameof(EditorViewModel.SourceWidth):
            case nameof(EditorViewModel.SourceHeight):
                // The output frame ("Native") and the video rect depend on the source aspect.
                Dispatcher.UIThread.Post(UpdateReframeOverlay, DispatcherPriority.Render);
                break;
            case nameof(EditorViewModel.CurrentClip):
                _reframeSelected = false;   // a fresh clip starts unselected (clean preview)
                Dispatcher.UIThread.Post(UpdateReframeOverlay, DispatcherPriority.Render);
                break;
            case nameof(EditorViewModel.SelectedText):
                LayoutTextOverlays();   // refresh the selection highlight
                break;
        }
    }

    // ───────────── Crop / reframe overlay ─────────────

    /// <summary>Which edge/corner (or the whole box) the active crop drag is moving.</summary>
    private enum CropDrag { None, Move, N, S, E, W, NE, NW, SE, SW }

    private CropDrag _cropDrag = CropDrag.None;
    private Point _cropStart;
    private bool _cropMoved;                  // distinguishes a click (play/pause) from a real drag
    private double _csX, _csY, _csW, _csH;   // active rect (normalized) captured at drag start

    /// <summary>Reframe mode only: the handles/dim appear only once the user clicks the video.</summary>
    private bool _reframeSelected;

    /// <summary>Dark bands drawn over the timeline for removed (cut) sections; the last one with a
    /// null cut is the live "pending" band while a cut is being marked.</summary>
    private readonly List<(Border band, EditorCut? cut)> _cutBands = new();

    /// <summary>Draggable caption overlays over the preview, paired with their VM item.</summary>
    private readonly List<(Border container, EditorTextItem item)> _textBlocks = new();
    private EditorTextItem? _dragText;
    private bool _textMoved;
    private Point _dragTextStart;
    private double _dtNx, _dtNy;

    private const double HandleReach = 11;   // px radius for grabbing an edge/corner

    private static readonly Cursor CurMove  = new(StandardCursorType.SizeAll);
    private static readonly Cursor CurWE    = new(StandardCursorType.SizeWestEast);
    private static readonly Cursor CurNS    = new(StandardCursorType.SizeNorthSouth);
    private static readonly Cursor CurNWSE  = new(StandardCursorType.TopLeftCorner);
    private static readonly Cursor CurNESW  = new(StandardCursorType.TopRightCorner);

    /// <summary>True while the separate Crop tool is active (the box edits the source crop).</summary>
    private bool CropMode => _vm?.CropToolActive == true;

    /// <summary>The OUTPUT FRAME rectangle (chosen aspect) centred in the stage.</summary>
    private Rect FrameRect() => FitRect(_vm?.FrameAspect ?? (16.0 / 9.0));

    /// <summary>The full SOURCE rectangle fit in the stage (the crop tool edits over this).</summary>
    private Rect SourceFitRect() => FitRect(_vm?.SourceAspectRaw ?? (16.0 / 9.0));

    private Rect FitRect(double aspect)
    {
        double sw = VideoSurface.Bounds.Width, sh = VideoSurface.Bounds.Height;
        if (sw <= 0 || sh <= 0 || aspect <= 0) return new Rect(0, 0, Math.Max(0, sw), Math.Max(0, sh));
        double w = sw, h = sw / aspect;
        if (h > sh) { h = sh; w = sh * aspect; }
        return new Rect((sw - w) / 2, (sh - h) / 2, w, h);
    }

    /// <summary>The reference rect the current tool edits within: the source (crop) or the frame (reframe).</summary>
    private Rect RefRect() => CropMode ? SourceFitRect() : FrameRect();

    /// <summary>The active normalized rect (crop box or video transform), per the active tool.</summary>
    private (double x, double y, double w, double h) ActiveNorm() =>
        _vm == null ? (0, 0, 1, 1)
        : CropMode ? (_vm.CropX, _vm.CropY, _vm.CropW, _vm.CropH)
                   : (_vm.VidX, _vm.VidY, _vm.VidW, _vm.VidH);

    private void SetActiveNorm(double x, double y, double w, double h)
    {
        if (_vm == null) return;
        if (CropMode) _vm.SetCrop(x, y, w, h);
        else _vm.SetVideoRect(x, y, w, h);
    }

    /// <summary>The active editable rect on screen (crop box / video rect) from the VM transform.</summary>
    private Rect EditRect(Rect refr)
    {
        var (nx, ny, nw, nh) = ActiveNorm();
        return new Rect(refr.X + nx * refr.Width, refr.Y + ny * refr.Height,
                        nw * refr.Width, nh * refr.Height);
    }

    /// <summary>Positions the video, the dim shades outside the output frame, the frame outline, the
    /// video-rect outline and the 8 handles.</summary>
    private void UpdateReframeOverlay()
    {
        if (_vm == null) return;
        var refr = RefRect();
        if (refr.Width <= 0 || refr.Height <= 0) return;
        var edit = EditRect(refr);

        // The video: crop mode shows the FULL source (refr); reframe shows the transformed video (edit).
        var vid = CropMode ? refr : edit;
        Canvas.SetLeft(VideoImage, vid.X);
        Canvas.SetTop(VideoImage, vid.Y);
        VideoImage.Width = Math.Max(1, vid.Width);
        VideoImage.Height = Math.Max(1, vid.Height);

        // Dim what gets cut: crop → outside the crop box; reframe → outside the output frame.
        var dim = CropMode ? edit : refr;
        double sw = VideoSurface.Bounds.Width, sh = VideoSurface.Bounds.Height;
        Place(CropShadeTop,    0, 0, sw, dim.Y);
        Place(CropShadeBottom, 0, dim.Bottom, sw, sh - dim.Bottom);
        Place(CropShadeLeft,   0, dim.Y, dim.X, dim.Height);
        Place(CropShadeRight,  dim.Right, dim.Y, sw - dim.Right, dim.Height);

        // Outline: the output frame (reframe) or the source bounds (crop).
        Place(FrameOutline, refr.X, refr.Y, refr.Width, refr.Height);

        // Edit-rect outline + handles.
        Canvas.SetLeft(CropRect, edit.X);
        Canvas.SetTop(CropRect, edit.Y);
        CropRect.Width = Math.Max(0, edit.Width);
        CropRect.Height = Math.Max(0, edit.Height);

        double bx = edit.X, by = edit.Y, bw = edit.Width, bh = edit.Height;
        PlaceHandle(HTL, bx, by);            PlaceHandle(HTR, bx + bw, by);
        PlaceHandle(HBL, bx, by + bh);       PlaceHandle(HBR, bx + bw, by + bh);
        PlaceHandle(HT, bx + bw / 2, by);    PlaceHandle(HB, bx + bw / 2, by + bh);
        PlaceHandle(HL, bx, by + bh / 2);    PlaceHandle(HR, bx + bw, by + bh / 2);

        // Editing chrome (dim + box outline + handles): always in crop mode, but in reframe mode
        // only after the user clicks the video (selection). The reference outline is always shown.
        bool chrome = CropMode || _reframeSelected;
        CropRect.IsVisible = chrome;
        CropShadeTop.IsVisible = CropShadeBottom.IsVisible =
            CropShadeLeft.IsVisible = CropShadeRight.IsVisible = chrome;
        HTL.IsVisible = HTR.IsVisible = HBL.IsVisible = HBR.IsVisible =
            HT.IsVisible = HB.IsVisible = HL.IsVisible = HR.IsVisible = chrome;
        FrameOutline.IsVisible = true;
        DeselectButton.IsVisible = !CropMode && _reframeSelected;

        UpdatePreviewTransform();
        LayoutTextOverlays();   // captions sit on the (un-rotated) output frame
    }

    // ───────────── Caption (text) overlays ─────────────

    private void OnTextsChanged()
    {
        if (_vm != null && _textBlocks.Count != _vm.Texts.Count) RebuildTextOverlays();
        else LayoutTextOverlays();
    }

    private void RebuildTextOverlays()
    {
        foreach (var (c, _) in _textBlocks) TextOverlay.Children.Remove(c);
        _textBlocks.Clear();
        if (_vm == null) return;

        foreach (var item in _vm.Texts)
        {
            var tb = new TextBlock { TextWrapping = TextWrapping.NoWrap, FontWeight = FontWeight.Bold };
            var border = new Border
            {
                Child = tb,
                Background = Avalonia.Media.Brushes.Transparent,
                BorderThickness = new Thickness(1.5),
                BorderBrush = Avalonia.Media.Brushes.Transparent,
                Padding = new Thickness(4, 2),
                CornerRadius = new CornerRadius(4),
                Cursor = new Cursor(StandardCursorType.SizeAll),
                Tag = item,
            };
            border.PointerPressed += OnTextPressed;
            border.PointerMoved += OnTextMoved;
            border.PointerReleased += OnTextReleased;
            TextOverlay.Children.Add(border);
            _textBlocks.Add((border, item));
        }
        LayoutTextOverlays();
    }

    private void LayoutTextOverlays()
    {
        if (_vm == null) return;
        var fr = FrameRect();
        if (fr.Width <= 0 || fr.Height <= 0) return;

        foreach (var (container, item) in _textBlocks)
        {
            if (container.Child is TextBlock tb)
            {
                tb.Text = item.Text;
                tb.FontSize = Math.Max(6, fr.Height * Math.Clamp(item.FontPercent, 1, 50) / 100.0);
                try { tb.Foreground = Avalonia.Media.Brush.Parse(item.Color); }
                catch { tb.Foreground = Avalonia.Media.Brushes.White; }
            }
            container.BorderBrush = ReferenceEquals(item, _vm.SelectedText)
                ? Avalonia.Media.Brush.Parse("#22D3EE") : Avalonia.Media.Brushes.Transparent;

            container.Measure(Size.Infinity);
            double cw = container.DesiredSize.Width, ch = container.DesiredSize.Height;
            double x = fr.X + (fr.Width - cw) * Math.Clamp(item.Nx, 0, 1);
            double y = fr.Y + (fr.Height - ch) * Math.Clamp(item.Ny, 0, 1);
            Canvas.SetLeft(container, x);
            Canvas.SetTop(container, y);
        }
    }

    private void OnTextPressed(object? sender, PointerPressedEventArgs e)
    {
        if (_vm == null || sender is not Border b || b.Tag is not EditorTextItem item) return;
        if (!e.GetCurrentPoint(this).Properties.IsLeftButtonPressed) return;

        _vm.SelectedText = item;
        _vm.SelectedToolsTab = 2;   // reveal the caption editor
        _dragText = item;
        _textMoved = false;
        _dragTextStart = e.GetPosition(TextOverlay);
        _dtNx = item.Nx; _dtNy = item.Ny;
        e.Pointer.Capture(b);
        e.Handled = true;
    }

    private void OnTextMoved(object? sender, PointerEventArgs e)
    {
        if (_vm == null || _dragText is null || sender is not Border b) return;
        var fr = FrameRect();
        if (fr.Width <= 0) return;

        var p = e.GetPosition(TextOverlay);
        if (!_textMoved && (Math.Abs(p.X - _dragTextStart.X) > 3 || Math.Abs(p.Y - _dragTextStart.Y) > 3))
        {
            _textMoved = true;
            _vm.PushUndo();
        }
        double cw = b.Bounds.Width, ch = b.Bounds.Height;
        double dx = p.X - _dragTextStart.X, dy = p.Y - _dragTextStart.Y;
        _dragText.Nx = Math.Clamp(_dtNx + dx / Math.Max(1, fr.Width - cw), 0, 1);
        _dragText.Ny = Math.Clamp(_dtNy + dy / Math.Max(1, fr.Height - ch), 0, 1);
        e.Handled = true;
    }

    private void OnTextReleased(object? sender, PointerReleasedEventArgs e)
    {
        _dragText = null;
        e.Pointer.Capture(null);
    }

    /// <summary>Applies rotate + flip to the whole preview (video + overlay together), scaled to fit
    /// the stage so a 90°/270° rotation doesn't spill out. Pointer math stays in untransformed local
    /// coords, so dragging keeps working.</summary>
    private void UpdatePreviewTransform()
    {
        if (_vm == null) return;
        double sw = VideoSurface.Bounds.Width, sh = VideoSurface.Bounds.Height;
        double fit = 1;
        if ((_vm.Rotation == 90 || _vm.Rotation == 270) && sw > 0 && sh > 0)
            fit = Math.Min(sw / sh, sh / sw);   // rotated content must fit the stage
        var g = new TransformGroup();
        g.Children.Add(new ScaleTransform((_vm.FlipH ? -1 : 1) * fit, (_vm.FlipV ? -1 : 1) * fit));
        g.Children.Add(new RotateTransform(_vm.Rotation));
        PreviewContent.RenderTransformOrigin = RelativePoint.Center;   // rotate/flip about the centre
        PreviewContent.RenderTransform = g;
    }

    private void OnDeselectClick(object? sender, RoutedEventArgs e)
    {
        _reframeSelected = false;
        UpdateReframeOverlay();
    }

    /// <summary>Snapshots the state before a colour-slider drag so Ctrl+Z reverts it.</summary>
    private void OnColorSliderPressed(object? sender, PointerPressedEventArgs e) => _vm?.PushUndo();

    private static void Place(Border b, double x, double y, double w, double h)
    {
        Canvas.SetLeft(b, x);
        Canvas.SetTop(b, y);
        b.Width = Math.Max(0, w);
        b.Height = Math.Max(0, h);
    }

    private static void PlaceHandle(Border b, double cx, double cy)
    {
        Canvas.SetLeft(b, cx - 6);
        Canvas.SetTop(b, cy - 6);
    }

    /// <summary>Classifies a pointer position against the video rect (corner/edge/inside/outside).</summary>
    private CropDrag HitTest(Point p)
    {
        if (_vm == null) return CropDrag.None;
        var refr = RefRect();
        if (refr.Width <= 0) return CropDrag.None;
        var v = EditRect(refr);

        double bx = v.X, by = v.Y, bw = v.Width, bh = v.Height;
        double r = HandleReach;

        bool nearL = Math.Abs(p.X - bx) <= r, nearR = Math.Abs(p.X - (bx + bw)) <= r;
        bool nearT = Math.Abs(p.Y - by) <= r, nearB = Math.Abs(p.Y - (by + bh)) <= r;
        bool inX = p.X >= bx - r && p.X <= bx + bw + r;
        bool inY = p.Y >= by - r && p.Y <= by + bh + r;

        if (nearL && nearT) return CropDrag.NW;
        if (nearR && nearT) return CropDrag.NE;
        if (nearL && nearB) return CropDrag.SW;
        if (nearR && nearB) return CropDrag.SE;
        if (nearL && inY) return CropDrag.W;
        if (nearR && inY) return CropDrag.E;
        if (nearT && inX) return CropDrag.N;
        if (nearB && inX) return CropDrag.S;
        if (p.X > bx && p.X < bx + bw && p.Y > by && p.Y < by + bh) return CropDrag.Move;
        return CropDrag.None;
    }

    private static Cursor CursorFor(CropDrag m) => m switch
    {
        CropDrag.Move => CurMove,
        CropDrag.W or CropDrag.E => CurWE,
        CropDrag.N or CropDrag.S => CurNS,
        CropDrag.NW or CropDrag.SE => CurNWSE,
        CropDrag.NE or CropDrag.SW => CurNESW,
        _ => Cursor.Default,
    };

    private void OnCropPointerPressed(object? sender, PointerPressedEventArgs e)
    {
        if (_vm == null) return;
        var pt = e.GetCurrentPoint(CropOverlay);

        // Right-click resets the active tool (crop → full source; reframe → default cover).
        if (pt.Properties.IsRightButtonPressed)
        {
            _vm.PushUndo();
            var reset = CropMode ? _vm.ResetCropCommand : _vm.ResetReframeCommand;
            if (reset.CanExecute(null)) reset.Execute(null);
            e.Handled = true;
            return;
        }
        if (!pt.Properties.IsLeftButtonPressed) return;

        var mode = HitTest(pt.Position);

        // Reframe selection: until the video is selected, only an interior click counts (it selects
        // and starts a move); the resize handles activate once selected. Crop mode is always editable.
        if (!CropMode && !_reframeSelected && mode != CropDrag.None)
            mode = CropDrag.Move;

        if (mode == CropDrag.None)
        {
            // Outside the box → deselect (reframe) and toggle play/pause.
            if (!CropMode && _reframeSelected) { _reframeSelected = false; UpdateReframeOverlay(); }
            if (_vm.PlayPauseCommand.CanExecute(null)) _vm.PlayPauseCommand.Execute(null);
            Focus();
            e.Handled = true;
            return;
        }

        if (!CropMode && !_reframeSelected) { _reframeSelected = true; UpdateReframeOverlay(); }

        _cropDrag = mode;
        _cropMoved = false;
        _cropStart = pt.Position;
        (_csX, _csY, _csW, _csH) = ActiveNorm();
        e.Pointer.Capture(CropOverlay);
        e.Handled = true;
    }

    private void OnCropPointerMoved(object? sender, PointerEventArgs e)
    {
        if (_vm == null) return;
        var p = e.GetPosition(CropOverlay);

        if (_cropDrag == CropDrag.None)
        {
            CropOverlay.Cursor = CursorFor(HitTest(p));
            return;
        }

        var refr = RefRect();
        if (refr.Width <= 0 || refr.Height <= 0) return;
        if (!_cropMoved && (Math.Abs(p.X - _cropStart.X) > 3 || Math.Abs(p.Y - _cropStart.Y) > 3))
        {
            _cropMoved = true;
            _vm.PushUndo();   // snapshot the pre-drag state once, for Ctrl+Z
        }

        // Deltas in units of the reference rect (frame for reframe, source for crop).
        double ndx = (p.X - _cropStart.X) / refr.Width;
        double ndy = (p.Y - _cropStart.Y) / refr.Height;

        double l = _csX, t = _csY, rr = _csX + _csW, b = _csY + _csH;
        if (_cropDrag == CropDrag.Move) { l += ndx; t += ndy; rr += ndx; b += ndy; }
        else
        {
            if (_cropDrag is CropDrag.W or CropDrag.NW or CropDrag.SW) l += ndx;
            if (_cropDrag is CropDrag.E or CropDrag.NE or CropDrag.SE) rr += ndx;
            if (_cropDrag is CropDrag.N or CropDrag.NW or CropDrag.NE) t += ndy;
            if (_cropDrag is CropDrag.S or CropDrag.SW or CropDrag.SE) b += ndy;
        }

        if (CropMode)
        {
            // Source crop: SetCrop clamps to [0,1]; just keep the 5% minimum.
            EnforceMin(ref l, ref t, ref rr, ref b);
            SetActiveNorm(l, t, rr - l, b - t);
            return;
        }

        // ── Reframe: magnetise the video edges to the format frame (0 / 1) and keep it in the field. ──
        double sw = VideoSurface.Bounds.Width, sh = VideoSurface.Bounds.Height;
        double snapX = 9 / refr.Width, snapY = 9 / refr.Height;
        double minX = -refr.X / refr.Width, maxX = (sw - refr.X) / refr.Width;
        double minY = -refr.Y / refr.Height, maxY = (sh - refr.Y) / refr.Height;

        if (_cropDrag == CropDrag.Move)
        {
            // Snap the nearest edge to the frame (preserving size), then keep within the field.
            if (Math.Abs(l) < snapX) { double o = -l; l += o; rr += o; }
            else if (Math.Abs(rr - 1) < snapX) { double o = 1 - rr; l += o; rr += o; }
            if (Math.Abs(t) < snapY) { double o = -t; t += o; b += o; }
            else if (Math.Abs(b - 1) < snapY) { double o = 1 - b; t += o; b += o; }
            if (l < minX) { rr += minX - l; l = minX; }
            if (rr > maxX) { l -= rr - maxX; rr = maxX; }
            if (t < minY) { b += minY - t; t = minY; }
            if (b > maxY) { t -= b - maxY; b = maxY; }
        }
        else
        {
            // Snap & clamp only the dragged edges.
            if (_cropDrag is CropDrag.W or CropDrag.NW or CropDrag.SW) { if (Math.Abs(l) < snapX) l = 0; l = Math.Max(l, minX); }
            if (_cropDrag is CropDrag.E or CropDrag.NE or CropDrag.SE) { if (Math.Abs(rr - 1) < snapX) rr = 1; rr = Math.Min(rr, maxX); }
            if (_cropDrag is CropDrag.N or CropDrag.NW or CropDrag.NE) { if (Math.Abs(t) < snapY) t = 0; t = Math.Max(t, minY); }
            if (_cropDrag is CropDrag.S or CropDrag.SW or CropDrag.SE) { if (Math.Abs(b - 1) < snapY) b = 1; b = Math.Min(b, maxY); }
            EnforceMin(ref l, ref t, ref rr, ref b);
        }

        SetActiveNorm(l, t, rr - l, b - t);
    }

    /// <summary>Keeps the dragged rect at least 5% of the reference in each axis.</summary>
    private void EnforceMin(ref double l, ref double t, ref double rr, ref double b)
    {
        const double min = 0.05;
        if (rr - l < min) { if (_cropDrag is CropDrag.W or CropDrag.NW or CropDrag.SW) l = rr - min; else rr = l + min; }
        if (b - t < min) { if (_cropDrag is CropDrag.N or CropDrag.NW or CropDrag.NE) t = b - min; else b = t + min; }
    }

    private void OnCropPointerReleased(object? sender, PointerReleasedEventArgs e)
    {
        // A press-release on the video without dragging = a click → toggle play/pause.
        bool wasClick = _cropDrag == CropDrag.Move && !_cropMoved;
        _cropDrag = CropDrag.None;
        e.Pointer.Capture(null);

        if (wasClick && _vm != null && _vm.PlayPauseCommand.CanExecute(null))
        {
            _vm.PlayPauseCommand.Execute(null);
            Focus();
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

        // Glide the playhead OVER removed sections so it visibly skips the red cut bands.
        double predSec = predicted * _vm.DurationSec;
        foreach (var c in _vm.Cuts)
            if (predSec >= c.StartSec && predSec < c.EndSec) { predicted = c.EndSec / _vm.DurationSec; break; }

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

        // Cut bands (removed sections) + the live pending band while marking.
        foreach (var (band, cut) in _cutBands)
        {
            double cs, ce;
            if (cut is { } c) { cs = c.StartSec; ce = c.EndSec; }
            else
            {
                double p = _vm.PendingCutStart ?? 0;
                double playSec = Math.Clamp(_vm.Position, 0, 1) * _vm.DurationSec;
                cs = Math.Min(p, playSec); ce = Math.Max(p, playSec);
            }
            double bx = Math.Clamp(cs / _vm.DurationSec, 0, 1) * w;
            double be = Math.Clamp(ce / _vm.DurationSec, 0, 1) * w;
            Canvas.SetLeft(band, bx);
            band.Width = Math.Max(0, be - bx);
            band.Height = h;
        }
    }

    /// <summary>Rebuilds the timeline cut bands from the VM's cut list (+ a pending band).</summary>
    private void RebuildCutBands()
    {
        foreach (var (band, _) in _cutBands) TimelineOverlay.Children.Remove(band);
        _cutBands.Clear();
        if (_vm == null) return;

        foreach (var cut in _vm.Cuts) AddCutBand(cut);
        if (_vm.PendingCutStart is not null) AddCutBand(null);   // live preview band

        UpdateOverlay();
    }

    private void AddCutBand(EditorCut? cut)
    {
        bool pending = cut is null;
        var b = new Border
        {
            Background = Avalonia.Media.Brush.Parse(pending ? "#66F04848" : "#CC2A0E12"),
            BorderBrush = Avalonia.Media.Brush.Parse("#F04848"),
            BorderThickness = new Thickness(1, 0, 1, 0),
            IsHitTestVisible = false,
        };
        Canvas.SetTop(b, 0);
        TimelineOverlay.Children.Insert(0, b);   // below the trim handles / playhead
        _cutBands.Add((b, cut));
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

        if (_drag is DragMode.TrimStart or DragMode.TrimEnd) _vm.PushUndo();   // undoable trim

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
