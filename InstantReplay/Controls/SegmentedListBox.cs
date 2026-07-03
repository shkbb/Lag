using System;
using System.Globalization;
using Avalonia;
using Avalonia.Animation;
using Avalonia.Animation.Easings;
using Avalonia.Controls;
using Avalonia.Controls.Presenters;
using Avalonia.Controls.Templates;
using Avalonia.Layout;
using Avalonia.Media.Transformation;

namespace Lag.Controls;

/// <summary>
/// The app's segmented control: a ListBox whose selection highlight is ONE pill that
/// SLIDES from the previously selected segment to the new one (instead of each item
/// painting its own background). Keeps the ListBox style key, so every existing
/// "ListBox.segctl" style (track, item foregrounds, hovers) applies untouched — the
/// only style difference is that items no longer paint a selected background; the
/// moving pill (SegSelBrush) is the selection visual.
/// </summary>
public class SegmentedListBox : ListBox
{
    protected override Type StyleKeyOverride => typeof(ListBox);

    private readonly Border _pill;
    private bool _pillPlaced;                       // first placement snaps; later ones slide
    private double _lastX = double.NaN, _lastW = double.NaN, _lastH = double.NaN, _lastY = double.NaN;

    public SegmentedListBox()
    {
        _pill = new Border
        {
            HorizontalAlignment = HorizontalAlignment.Left,
            VerticalAlignment = VerticalAlignment.Top,
            CornerRadius = new CornerRadius(6),
            IsVisible = false,
            RenderTransform = TransformOperations.Parse("translateX(0px) translateY(0px)"),
        };
        _pill.Bind(Border.BackgroundProperty, this.GetResourceObservable("SegSelBrush"));

        // Own the template (same approach as the ToggleSwitch knob fix): a pill layer
        // UNDER the items presenter. No ScrollViewer — segments always fit their strip.
        Template = new FuncControlTemplate<SegmentedListBox>((owner, scope) =>
        {
            (owner._pill.Parent as Panel)?.Children.Remove(owner._pill); // re-templating safety

            var presenter = new ItemsPresenter { Name = "PART_ItemsPresenter" };
            presenter.RegisterInNameScope(scope);
            presenter[~ItemsPresenter.ItemsPanelProperty] = owner[~ItemsPanelProperty];

            var root = new Panel { Children = { new Panel { Children = { owner._pill } }, presenter } };

            var track = new Border { Child = root };
            track[~Border.BackgroundProperty] = owner[~BackgroundProperty];
            track[~Border.CornerRadiusProperty] = owner[~CornerRadiusProperty];
            track[~Border.PaddingProperty] = owner[~PaddingProperty];
            return track;
        });

        SelectionChanged += (_, _) => UpdatePill();
    }

    protected override void OnAttachedToVisualTree(VisualTreeAttachmentEventArgs e)
    {
        base.OnAttachedToVisualTree(e);
        LayoutUpdated += OnLayoutUpdated;
    }

    protected override void OnDetachedFromVisualTree(VisualTreeAttachmentEventArgs e)
    {
        base.OnDetachedFromVisualTree(e);
        LayoutUpdated -= OnLayoutUpdated;
        _pillPlaced = false;
    }

    /// <summary>Containers realize/resize after SelectionChanged (startup, language or option
    /// rebuilds, window resize) — keep the pill glued to the selected segment's bounds.</summary>
    private void OnLayoutUpdated(object? sender, EventArgs e) => UpdatePill();

    private void UpdatePill()
    {
        if (SelectedIndex < 0 || ContainerFromIndex(SelectedIndex) is not Control c ||
            !c.IsArrangeValid || c.Bounds.Width <= 0 || _pill.Parent is not Visual host)
        {
            _pill.IsVisible = false;
            _pillPlaced = false;
            ResetLast();
            return;
        }

        if (c.TranslatePoint(default, host) is not { } p) return;

        // LayoutUpdated fires often — only touch the pill when the target actually moved.
        if (Math.Abs(p.X - _lastX) < 0.5 && Math.Abs(p.Y - _lastY) < 0.5 &&
            Math.Abs(c.Bounds.Width - _lastW) < 0.5 && Math.Abs(c.Bounds.Height - _lastH) < 0.5)
            return;
        (_lastX, _lastY, _lastW, _lastH) = (p.X, p.Y, c.Bounds.Width, c.Bounds.Height);

        bool firstShow = !_pillPlaced;
        if (firstShow) _pill.Transitions = null;   // initial placement must snap, not glide in

        _pill.Width = c.Bounds.Width;
        _pill.Height = c.Bounds.Height;
        _pill.RenderTransform = TransformOperations.Parse(
            string.Create(CultureInfo.InvariantCulture, $"translateX({p.X}px) translateY({p.Y}px)"));
        _pill.IsVisible = true;

        if (firstShow)
        {
            // Values are already set — attaching now arms the slide for the NEXT change only.
            _pill.Transitions = BuildPillTransitions();
            _pillPlaced = true;
        }
    }

    private void ResetLast() => (_lastX, _lastY, _lastW, _lastH) = (double.NaN, double.NaN, double.NaN, double.NaN);

    private static Transitions BuildPillTransitions() => new()
    {
        new TransformOperationsTransition
        {
            Property = RenderTransformProperty,
            Duration = TimeSpan.FromMilliseconds(220),
            Easing = new CubicEaseOut(),
        },
        new DoubleTransition
        {
            Property = WidthProperty,
            Duration = TimeSpan.FromMilliseconds(220),
            Easing = new CubicEaseOut(),
        },
        new DoubleTransition
        {
            Property = HeightProperty,
            Duration = TimeSpan.FromMilliseconds(220),
            Easing = new CubicEaseOut(),
        },
    };
}
