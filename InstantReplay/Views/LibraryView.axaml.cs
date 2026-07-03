using Avalonia;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Interactivity;
using Avalonia.VisualTree;
using Lag.Models;
using Lag.ViewModels;

namespace Lag.Views;

public partial class LibraryView : UserControl
{
    public LibraryView()
    {
        InitializeComponent();

        // The chip strip only scrolls horizontally, so a plain wheel would do nothing —
        // translate vertical wheel ticks into horizontal offset. Tunnel routing so the
        // ScrollViewer's own (vertical) wheel handling never swallows the event.
        ChipsScroller.AddHandler(PointerWheelChangedEvent, OnChipsWheel, RoutingStrategies.Tunnel);

        // The card grid is by far the heaviest layout in the app — building it DURING the
        // page transition ate every animation frame (the library "just popped in, laggy").
        // Keep the body collapsed until the page slide finishes, then fade+rise it in.
        AttachedToVisualTree += (_, _) =>
        {
            LibraryBody.IsVisible = false;
            Avalonia.Threading.DispatcherTimer.RunOnce(() =>
            {
                LibraryBody.IsVisible = true;
                Lag.Core.FxAnimations.SlideFadeIn(LibraryBody, 12, 220);
            }, System.TimeSpan.FromMilliseconds(240));
        };
    }

    /// <summary>Mouse wheel over the category chips: scroll the strip horizontally.</summary>
    private void OnChipsWheel(object? sender, PointerWheelEventArgs e)
    {
        double delta = e.Delta.X != 0 ? e.Delta.X : e.Delta.Y; // support tilt wheels too
        if (delta == 0 || sender is not ScrollViewer sv) return;

        double max = System.Math.Max(0, sv.Extent.Width - sv.Viewport.Width);
        sv.Offset = sv.Offset.WithX(System.Math.Clamp(sv.Offset.X - delta * 60, 0, max));
        e.Handled = true;
    }

    /// <summary>Resolves the clip a context-menu item / card was invoked on (its inherited DataContext).</summary>
    private static ReplayClip? ClipFrom(object? sender) =>
        (sender as Control)?.DataContext as ReplayClip;

    // ───────────── Explorer-style selection ─────────────

    /// <summary>
    /// A press on a clip card. With nothing selected a plain click just plays it. Once a selection
    /// is in progress (one started via the corner circle, or Ctrl/Shift), a plain click toggles the
    /// clip in/out of the selection — accumulating, Photos-style. Ctrl = toggle, Shift = range.
    /// </summary>
    private void OnClipPressed(object? sender, PointerPressedEventArgs e)
    {
        if (DataContext is not LibraryViewModel vm || ClipFrom(sender) is not { } clip) return;

        var props = e.GetCurrentPoint(sender as Visual).Properties;
        if (props.IsRightButtonPressed) return; // let the context menu open; leave the selection as-is
        if (!props.IsLeftButtonPressed) return;

        if (e.KeyModifiers.HasFlag(KeyModifiers.Shift)) vm.SelectRange(clip);
        else if (e.KeyModifiers.HasFlag(KeyModifiers.Control)) vm.ToggleSelectClip(clip);
        else if (vm.HasSelection) vm.ToggleSelectClip(clip);   // selection in progress → click adds/removes
        else vm.PlayClipCommand.Execute(clip);                 // nothing selected → click plays

        // Focus the card so the view's Ctrl+A / Esc key bindings fire afterwards.
        (sender as Control)?.Focus();
    }

    /// <summary>The corner circle: starts/continues a selection by toggling this clip.</summary>
    private void OnSelectCircleClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is LibraryViewModel vm && ClipFrom(sender) is { } clip)
        {
            vm.ToggleSelectClip(clip);
            (sender as Control)?.Focus();
        }
    }

    /// <summary>A press that lands on empty space (not on a card) clears the selection.</summary>
    private void OnBodyPressed(object? sender, PointerPressedEventArgs e)
    {
        if (DataContext is not LibraryViewModel vm) return;

        for (var v = e.Source as Visual; v != null; v = v.GetVisualParent())
            if (v is Border b && b.Classes.Contains("clipcardr"))
                return; // the press was on a card — its own handler dealt with it

        vm.ClearSelectionCommand.Execute(null);
    }

    // ───────────── Context menu (right-click) ─────────────

    /// <summary>Context menu → "Show in folder". Delegates to the VM command.</summary>
    private void OnShowInFolderClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is LibraryViewModel vm && ClipFrom(sender) is { } clip)
            vm.ShowInFolderCommand.Execute(clip);
    }

    /// <summary>Context menu → "Edit". Raises the VM event handled by MainViewModel.</summary>
    private void OnEditClipClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is LibraryViewModel vm && ClipFrom(sender) is { } clip)
            vm.RequestEdit(clip);
    }

    /// <summary>Context menu → "Delete". Delegates to the VM command.</summary>
    private void OnDeleteClipClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is LibraryViewModel vm && ClipFrom(sender) is { } clip)
            vm.DeleteClipCommand.Execute(clip);
    }
}
