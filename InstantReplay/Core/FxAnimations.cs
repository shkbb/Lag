using System;
using Avalonia;
using Avalonia.Animation;
using Avalonia.Animation.Easings;
using Avalonia.Controls;
using Avalonia.Media.Transformation;
using Avalonia.Threading;

namespace Lag.Core;

/// <summary>
/// Tiny imperative animation helpers for spots styles can't reach (e.g. re-animating a panel
/// that only toggles IsVisible). Transition-based (the engine-proven kind — see the
/// avalonia-ui-gotchas notes), never keyframes on properties that already carry transitions.
/// </summary>
public static class FxAnimations
{
    /// <summary>Fade + rise-in for a control that is already (or about to become) visible.
    /// Safe to call repeatedly: values snap first with transitions detached, then animate.</summary>
    public static void SlideFadeIn(Control c, double fromY = 16, int ms = 240)
    {
        c.Transitions = null;
        c.Opacity = 0;
        c.RenderTransform = TransformOperations.Parse($"translateY({fromY.ToString(System.Globalization.CultureInfo.InvariantCulture)}px)");

        Dispatcher.UIThread.Post(() =>
        {
            c.Transitions = new Transitions
            {
                new DoubleTransition
                {
                    Property = Visual.OpacityProperty,
                    Duration = TimeSpan.FromMilliseconds(ms),
                    Easing = new CubicEaseOut(),
                },
                new TransformOperationsTransition
                {
                    Property = Visual.RenderTransformProperty,
                    Duration = TimeSpan.FromMilliseconds(ms),
                    Easing = new CubicEaseOut(),
                },
            };
            c.Opacity = 1;
            c.RenderTransform = TransformOperations.Parse("translateY(0px)");
        }, DispatcherPriority.Background);
    }
}
