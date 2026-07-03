using System;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Interactivity;
using Lag.ViewModels;

namespace Lag.Views;

public partial class SettingsView : UserControl
{
    public SettingsView()
    {
        InitializeComponent();

        // A control in a freshly materialized tab can request bring-into-view AFTER our
        // scroll reset (first show of the Audio tab did) — swallow those during a switch.
        AddHandler(RequestBringIntoViewEvent, (_, e) => { if (_suppressBringIntoView) e.Handled = true; });
    }

    private bool _suppressBringIntoView;

    private SettingsViewModel? Vm => DataContext as SettingsViewModel;
    private Window? _hostWindow;

    // The live audio meter (and "hear yourself") runs only while this view is actually on screen AND
    // the window is focused — so navigating away or minimising to the tray stops it, no surprise
    // background mic-to-speakers.
    protected override void OnAttachedToVisualTree(VisualTreeAttachmentEventArgs e)
    {
        base.OnAttachedToVisualTree(e);
        LayoutOverlayBoxes();
        Vm?.SetAudioViewAttached(true);

        _hostWindow = TopLevel.GetTopLevel(this) as Window;
        if (_hostWindow != null)
        {
            _hostWindow.Activated += OnWindowActivated;
            _hostWindow.Deactivated += OnWindowDeactivated;
            Vm?.SetWindowActive(_hostWindow.IsActive);
        }
    }

    protected override void OnDetachedFromVisualTree(VisualTreeAttachmentEventArgs e)
    {
        base.OnDetachedFromVisualTree(e);
        Vm?.SetAudioViewAttached(false);

        if (_hostWindow != null)
        {
            _hostWindow.Activated -= OnWindowActivated;
            _hostWindow.Deactivated -= OnWindowDeactivated;
            _hostWindow = null;
        }
    }

    private void OnWindowActivated(object? sender, EventArgs e) => Vm?.SetWindowActive(true);
    private void OnWindowDeactivated(object? sender, EventArgs e) => Vm?.SetWindowActive(false);

    private static int TagToInt(object? sender) =>
        sender is Control { Tag: { } tag } && int.TryParse(tag.ToString(), out int v) ? v : 0;

    /// <summary>Tab strip (Video / Audio / General) — drives the Carousel + underline animation.</summary>
    private void OnTabClick(object? sender, RoutedEventArgs e)
    {
        if (Vm != null) Vm.SelectedSettingsTab = TagToInt(sender);
    }

    /// <summary>Every settings tab opens at the TOP — reusing the previous tab's scroll offset
    /// dropped the user somewhere mid-list.</summary>
    private void OnTabsChanged(object? sender, SelectionChangedEventArgs e)
    {
        if (SettingsScroll == null) return;
        _suppressBringIntoView = true;
        SettingsScroll.Offset = default;

        // Fade+rise the tab's content in (snap happens now, the animation next layout pass).
        var panel = (Vm?.SelectedSettingsTab ?? 0) switch
        { 0 => TabPanel0, 1 => TabPanel1, 2 => TabPanel2, _ => TabPanel3 };
        Lag.Core.FxAnimations.SlideFadeIn(panel);
        // Re-assert AFTER the new tab has laid out (its first materialization can scroll),
        // then allow bring-into-view again.
        Avalonia.Threading.Dispatcher.UIThread.Post(() =>
        {
            SettingsScroll.Offset = default;
            _suppressBringIntoView = false;
        }, Avalonia.Threading.DispatcherPriority.Background);
    }

    // ───────────── Interactive overlay preview (drag to move, corner grip to resize) ─────────────

    private const double StageW = 736, StageH = 414;
    private const double WebcamAspect = 4.0 / 3.0;   // preview approximation; recordings use the real one
    private const double KeysAspect = 1.84;          // keyboard block + mouse (see OverlayCompositor)
    private const double StatsAspect = 7.5;          // the one-line resource panel is wide and thin

    private Border? _ovTarget;         // the box being dragged/resized (null = no gesture)
    private bool _ovResizing;
    private Point _ovStart;            // pointer position at gesture start (stage coordinates)
    private double _ovStartLeft, _ovStartTop, _ovStartH;

    /// <summary>Positions/sizes the preview boxes from the persisted fractions.</summary>
    private void LayoutOverlayBoxes()
    {
        if (Vm == null) return;
        PlaceBox(OvWebcamBox, Vm.WebcamX, Vm.WebcamY, Vm.WebcamScale, WebcamAspect);
        PlaceBox(OvKeysBox, Vm.KeysX, Vm.KeysY, Vm.KeysScale, KeysAspect);
        PlaceBox(OvStatsBox, Vm.StatsX, Vm.StatsY, Vm.StatsScale, StatsAspect);
    }

    private double AspectOf(Border box) =>
        ReferenceEquals(box, OvWebcamBox) ? WebcamAspect :
        ReferenceEquals(box, OvKeysBox) ? KeysAspect : StatsAspect;

    private static void PlaceBox(Border box, double xf, double yf, double scale, double aspect)
    {
        double h = scale * StageH, w = h * aspect;
        box.Width = w;
        box.Height = h;
        Canvas.SetLeft(box, xf * Math.Max(0, StageW - w));
        Canvas.SetTop(box, yf * Math.Max(0, StageH - h));
    }

    // Edge-zone resize (window-style): grab any edge/corner to resize, the middle to move.
    private const double EdgeZone = 10;
    private bool _ovL, _ovR, _ovT, _ovB;   // which edges the gesture grabbed
    private double _ovStartW;

    private static (bool L, bool R, bool T, bool B) HitZone(Border box, Point p)
    {
        double w = box.Bounds.Width, h = box.Bounds.Height;
        // Small elements: edge zones would swallow the whole box and make MOVING impossible —
        // shrink them (never more than 1/5 of the box) and drop them entirely below ~45px;
        // tiny elements move by drag and resize by the mouse wheel.
        if (Math.Min(w, h) < 45) return (false, false, false, false);
        double z = Math.Min(EdgeZone, Math.Min(w, h) / 5.0);
        return (p.X <= z, p.X >= w - z, p.Y <= z, p.Y >= h - z);
    }

    private static Cursor CursorFor(bool l, bool r, bool t, bool b) =>
        (l && t) || (r && b) ? new Cursor(StandardCursorType.TopLeftCorner)
        : (r && t) || (l && b) ? new Cursor(StandardCursorType.TopRightCorner)
        : l || r ? new Cursor(StandardCursorType.SizeWestEast)
        : t || b ? new Cursor(StandardCursorType.SizeNorthSouth)
        : new Cursor(StandardCursorType.SizeAll);

    private void OnOvBoxPressed(object? sender, PointerPressedEventArgs e)
    {
        if (sender is not Border box) return;
        var (l, r, t, b) = HitZone(box, e.GetPosition(box));
        _ovTarget = box;
        _ovResizing = l || r || t || b;
        (_ovL, _ovR, _ovT, _ovB) = (l, r, t, b);
        _ovStart = e.GetPosition(OverlayStage);
        _ovStartLeft = Canvas.GetLeft(box);
        _ovStartTop = Canvas.GetTop(box);
        _ovStartW = box.Bounds.Width;
        _ovStartH = box.Bounds.Height;
        e.Pointer.Capture(box);
        e.Handled = true;
    }

    private void OnOvBoxMoved(object? sender, PointerEventArgs e)
    {
        // Hovering (no gesture): just show the right cursor for the zone under the pointer.
        if (_ovTarget is not { } box)
        {
            if (sender is Border hov)
            {
                var (hl, hr, ht, hb) = HitZone(hov, e.GetPosition(hov));
                hov.Cursor = CursorFor(hl, hr, ht, hb);
            }
            return;
        }

        var p = e.GetPosition(OverlayStage);
        double aspect = AspectOf(box);
        double dx = p.X - _ovStart.X, dy = p.Y - _ovStart.Y;

        if (_ovResizing)
        {
            bool isStats = ReferenceEquals(box, OvStatsBox);
            double minH = (isStats ? 0.02 : 0.06) * StageH;
            double maxH = (ReferenceEquals(box, OvWebcamBox) ? 0.45 : isStats ? 0.10 : 0.42) * StageH;

            // New size from whichever edges are held; the aspect ratio stays locked.
            double w = _ovStartW + (_ovR ? dx : _ovL ? -dx : 0);
            double h = _ovStartH + (_ovB ? dy : _ovT ? -dy : 0);
            h = (_ovL || _ovR) && !(_ovT || _ovB) ? w / aspect
              : (_ovT || _ovB) && !(_ovL || _ovR) ? h
              : Math.Max(w / aspect, h);
            h = Math.Clamp(h, minH, maxH);

            // Anchor the opposite edge; keep everything inside the stage.
            double right = _ovStartLeft + _ovStartW, bottom = _ovStartTop + _ovStartH;
            h = Math.Min(h, _ovL ? right / aspect : (StageW - _ovStartLeft) / aspect);
            h = Math.Min(h, _ovT ? bottom : StageH - _ovStartTop);
            w = h * aspect;

            box.Width = w;
            box.Height = h;
            Canvas.SetLeft(box, _ovL ? right - w : _ovStartLeft);
            Canvas.SetTop(box, _ovT ? bottom - h : _ovStartTop);
        }
        else
        {
            double w = box.Bounds.Width, h = box.Bounds.Height;
            Canvas.SetLeft(box, Math.Clamp(_ovStartLeft + dx, 0, Math.Max(0, StageW - w)));
            Canvas.SetTop(box, Math.Clamp(_ovStartTop + dy, 0, Math.Max(0, StageH - h)));
        }
        e.Handled = true;
    }

    private void OnOvBoxReleased(object? sender, PointerReleasedEventArgs e)
    {
        if (_ovTarget is not { } box) return;
        _ovTarget = null;
        e.Pointer.Capture(null);
        CommitBox(box);
    }

    /// <summary>Persists a box's current place/size as resolution-independent fractions.</summary>
    private void CommitBox(Border box)
    {
        if (Vm == null) return;
        double w = box.Bounds.Width, h = box.Bounds.Height;
        double xf = StageW - w > 1 ? Math.Clamp(Canvas.GetLeft(box) / (StageW - w), 0, 1) : 0;
        double yf = StageH - h > 1 ? Math.Clamp(Canvas.GetTop(box) / (StageH - h), 0, 1) : 0;
        double scale = h / StageH;

        if (ReferenceEquals(box, OvWebcamBox)) { Vm.WebcamX = xf; Vm.WebcamY = yf; Vm.WebcamScale = scale; }
        else if (ReferenceEquals(box, OvKeysBox)) { Vm.KeysX = xf; Vm.KeysY = yf; Vm.KeysScale = scale; }
        else { Vm.StatsX = xf; Vm.StatsY = yf; Vm.StatsScale = scale; }
        Vm.CommitOverlayLayout();
    }

    /// <summary>Mouse wheel over an element = smooth grow/shrink (like zooming in the editor).</summary>
    private void OnOvBoxWheel(object? sender, PointerWheelEventArgs e)
    {
        if (sender is not Border box || Vm == null) return;
        bool isStats = ReferenceEquals(box, OvStatsBox);
        double minScale = isStats ? 0.02 : 0.06;
        double maxScale = ReferenceEquals(box, OvWebcamBox) ? 0.45 : isStats ? 0.10 : 0.42;
        double aspect = AspectOf(box);

        double h = box.Bounds.Height * (e.Delta.Y > 0 ? 1.08 : 0.92);
        h = Math.Clamp(h, minScale * StageH, maxScale * StageH);
        h = Math.Min(h, (StageW - Canvas.GetLeft(box)) / aspect);   // stay inside the stage
        h = Math.Min(h, StageH - Canvas.GetTop(box));
        box.Width = h * aspect;
        box.Height = h;

        CommitBox(box);
        e.Handled = true;
    }

    /// <summary>"I understand" on the intensive-quality disclaimer — dismiss for the session.</summary>
    private void OnIntensiveUnderstood(object? sender, RoutedEventArgs e)
    {
        Vm?.AcknowledgeIntensive();
    }

    /// <summary>A hardware preset card was clicked — apply its resolved settings.</summary>
    private void OnPresetClick(object? sender, TappedEventArgs e)
    {
        if (Vm != null && sender is Control { DataContext: PresetCard card })
            Vm.ApplyPresetCommand.Execute(card);
    }

    // Press/drag anywhere on the input-sensitivity bar to set the threshold (0–100% across the bar).
    // PointerPressed and PointerMoved take different event-arg types, so two thin handlers share one body.
    private void OnGatePressed(object? sender, PointerPressedEventArgs e) => SetGateFromPointer(sender, e);
    private void OnGateMoved(object? sender, PointerEventArgs e) => SetGateFromPointer(sender, e);

    private void SetGateFromPointer(object? sender, PointerEventArgs e)
    {
        if (Vm == null || sender is not Control bar) return;
        var p = e.GetCurrentPoint(bar);
        if (!p.Properties.IsLeftButtonPressed) return;
        double w = bar.Bounds.Width;
        if (w <= 0) return;
        Vm.InputSensitivityThreshold = (int)Math.Round(Math.Clamp(p.Position.X / w * 100.0, 0, 100));
        e.Pointer.Capture(bar);
    }
}
