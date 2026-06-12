using System;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Media;

namespace Lag.Controls;

/// <summary>
/// A full-width audio track lane for the editor timeline, visually matching the video
/// strip above it: a slate "audio" body with a faux waveform, dark shading outside the
/// audible [RangeStart, RangeEnd] range, and the same cyan trim handles as the video row.
/// Shares the x-scale with the video strip, so timings line up 1:1.
/// All values are in seconds against <see cref="Duration"/>.
/// Rendered directly (no template) — it's a single-purpose widget.
/// </summary>
public class TrimRangeBar : Control
{
    public static readonly StyledProperty<double> DurationProperty =
        AvaloniaProperty.Register<TrimRangeBar, double>(nameof(Duration), 1.0);

    public static readonly StyledProperty<double> RangeStartProperty =
        AvaloniaProperty.Register<TrimRangeBar, double>(nameof(RangeStart), 0.0,
            defaultBindingMode: Avalonia.Data.BindingMode.TwoWay);

    public static readonly StyledProperty<double> RangeEndProperty =
        AvaloniaProperty.Register<TrimRangeBar, double>(nameof(RangeEnd), 1.0,
            defaultBindingMode: Avalonia.Data.BindingMode.TwoWay);

    public double Duration
    {
        get => GetValue(DurationProperty);
        set => SetValue(DurationProperty, value);
    }

    public double RangeStart
    {
        get => GetValue(RangeStartProperty);
        set => SetValue(RangeStartProperty, value);
    }

    public double RangeEnd
    {
        get => GetValue(RangeEndProperty);
        set => SetValue(RangeEndProperty, value);
    }

    /// <summary>Magnetic snap targets (the video trim start/end). NaN = no snapping.</summary>
    public static readonly StyledProperty<double> SnapAProperty =
        AvaloniaProperty.Register<TrimRangeBar, double>(nameof(SnapA), double.NaN);

    public static readonly StyledProperty<double> SnapBProperty =
        AvaloniaProperty.Register<TrimRangeBar, double>(nameof(SnapB), double.NaN);

    public double SnapA
    {
        get => GetValue(SnapAProperty);
        set => SetValue(SnapAProperty, value);
    }

    public double SnapB
    {
        get => GetValue(SnapBProperty);
        set => SetValue(SnapBProperty, value);
    }

    private const double HandleWidth = 10;
    private const double MinGapSec = 0.1;

    private enum DragTarget { None, Start, End }
    private DragTarget _drag;

    static TrimRangeBar()
    {
        AffectsRender<TrimRangeBar>(DurationProperty, RangeStartProperty, RangeEndProperty);
    }

    public TrimRangeBar()
    {
        Cursor = new Cursor(StandardCursorType.Hand);
        MinHeight = 24;
        ClipToBounds = true;
    }

    // Visual palette (mirrors the video strip row)
    private static readonly SolidColorBrush BodyBrush = new(Color.FromRgb(0x23, 0x2C, 0x36));
    private static readonly SolidColorBrush BodyMutedBrush = new(Color.FromRgb(0x1A, 0x1F, 0x27));
    private static readonly SolidColorBrush WaveBrush = new(Color.FromRgb(0x41, 0x50, 0x61));
    private static readonly SolidColorBrush WaveMutedBrush = new(Color.FromRgb(0x2A, 0x31, 0x3B));
    private static readonly SolidColorBrush ShadeBrush = new(Color.FromArgb(0xB3, 0x0A, 0x0C, 0x0F));
    private static readonly SolidColorBrush HandleBrush = new(Color.FromRgb(0x22, 0xD3, 0xEE));
    private static readonly SolidColorBrush HandleMutedBrush = new(Color.FromArgb(0x55, 0xFF, 0xFF, 0xFF));
    private static readonly SolidColorBrush NotchBrush = new(Color.FromArgb(0xA6, 0x0A, 0x0C, 0x0F));

    public override void Render(DrawingContext context)
    {
        double w = Bounds.Width, h = Bounds.Height;
        if (w <= 0 || Duration <= 0) return;

        double sx = Math.Clamp(RangeStart / Duration, 0, 1) * w;
        double ex = Math.Clamp(RangeEnd / Duration, 0, 1) * w;

        // Track body (full width, like the film strip above)
        context.DrawRectangle(IsEnabled ? BodyBrush : BodyMutedBrush, null,
            new RoundedRect(new Rect(0, 0, w, h), 4));

        // Faux waveform: deterministic pseudo-random bars around the centerline
        var wave = IsEnabled ? WaveBrush : WaveMutedBrush;
        double mid = h / 2;
        for (double x = 3; x < w - 3; x += 4)
        {
            double t = x * 0.55;
            double amp = (Math.Sin(t) * 0.5 + Math.Sin(t * 0.31 + 1.7) * 0.32 + Math.Sin(t * 0.071 + 0.4) * 0.18)
                         * 0.5 + 0.5; // 0..1
            double barH = Math.Max(2, amp * (h - 10));
            context.DrawRectangle(wave, null, new Rect(x, mid - barH / 2, 2, barH));
        }

        // Shade everything outside the audible range (same overlay as the video row)
        if (sx > 0)
            context.DrawRectangle(ShadeBrush, null, new Rect(0, 0, sx, h));
        if (ex < w)
            context.DrawRectangle(ShadeBrush, null, new Rect(ex, 0, w - ex, h));

        // Trim handles — full height, same shape as the video handles
        var handle = IsEnabled ? HandleBrush : HandleMutedBrush;
        double hsx = Math.Clamp(sx, 0, w) - (sx <= 5 ? 0 : 5);
        double hex = Math.Clamp(ex, 0, w) - (ex >= w - 5 ? HandleWidth : 5);

        context.DrawRectangle(handle, null,
            new RoundedRect(new Rect(hsx, 0, HandleWidth, h),
                new Vector(3, 3), default, default, new Vector(3, 3)));
        context.DrawRectangle(handle, null,
            new RoundedRect(new Rect(hex, 0, HandleWidth, h),
                default, new Vector(3, 3), new Vector(3, 3), default));

        // Inner notches on the handles
        double notchH = Math.Max(8, h * 0.45);
        context.DrawRectangle(NotchBrush, null,
            new RoundedRect(new Rect(hsx + HandleWidth / 2 - 1, mid - notchH / 2, 2, notchH), 1));
        context.DrawRectangle(NotchBrush, null,
            new RoundedRect(new Rect(hex + HandleWidth / 2 - 1, mid - notchH / 2, 2, notchH), 1));
    }

    protected override void OnPointerPressed(PointerPressedEventArgs e)
    {
        base.OnPointerPressed(e);
        if (!IsEnabled || Bounds.Width <= 0 || Duration <= 0) return;

        // Shift+click is row selection — let it bubble to the timeline row.
        if (e.KeyModifiers.HasFlag(KeyModifiers.Shift)) return;

        double x = e.GetPosition(this).X;
        double sx = RangeStart / Duration * Bounds.Width;
        double ex = RangeEnd / Duration * Bounds.Width;

        // Pick the nearer handle; clicks far from both move the nearer one too.
        _drag = Math.Abs(x - sx) <= Math.Abs(x - ex) ? DragTarget.Start : DragTarget.End;

        e.Pointer.Capture(this);
        ApplyDrag(x);
        e.Handled = true;
    }

    protected override void OnPointerMoved(PointerEventArgs e)
    {
        base.OnPointerMoved(e);
        if (_drag == DragTarget.None) return;
        ApplyDrag(e.GetPosition(this).X);
        e.Handled = true;
    }

    protected override void OnPointerReleased(PointerReleasedEventArgs e)
    {
        base.OnPointerReleased(e);
        _drag = DragTarget.None;
        e.Pointer.Capture(null);
    }

    private void ApplyDrag(double x)
    {
        double sec = Math.Clamp(x / Bounds.Width, 0, 1) * Duration;

        // Magnet to the video trim edges (frame-exact alignment, pro-editor style).
        double threshold = 10.0 / Bounds.Width * Duration; // ~10px in seconds
        foreach (double snap in new[] { SnapA, SnapB })
        {
            if (!double.IsNaN(snap) && Math.Abs(sec - snap) <= threshold)
            {
                sec = snap;
                break;
            }
        }

        if (_drag == DragTarget.Start)
            SetCurrentValue(RangeStartProperty, Math.Clamp(sec, 0, Math.Max(0, RangeEnd - MinGapSec)));
        else if (_drag == DragTarget.End)
            SetCurrentValue(RangeEndProperty, Math.Clamp(sec, Math.Min(Duration, RangeStart + MinGapSec), Duration));
    }
}
