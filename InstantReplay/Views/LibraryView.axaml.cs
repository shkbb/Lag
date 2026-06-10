using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Interactivity;
using Lag.Models;
using Lag.ViewModels;

namespace Lag.Views;

public partial class LibraryView : UserControl
{
    public LibraryView()
    {
        InitializeComponent();
    }

    /// <summary>Resolves the clip a context-menu item was invoked on (its inherited DataContext).</summary>
    private static ReplayClip? ClipFrom(object? sender) =>
        (sender as Control)?.DataContext as ReplayClip;

    /// <summary>Context menu → "Show in folder". Delegates to the VM command.</summary>
    private void OnShowInFolderClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is LibraryViewModel vm && ClipFrom(sender) is { } clip)
            vm.ShowInFolderCommand.Execute(clip);
    }

    /// <summary>Context menu → "Delete". Delegates to the VM command.</summary>
    private void OnDeleteClipClick(object? sender, RoutedEventArgs e)
    {
        if (DataContext is LibraryViewModel vm && ClipFrom(sender) is { } clip)
            vm.DeleteClipCommand.Execute(clip);
    }
}
