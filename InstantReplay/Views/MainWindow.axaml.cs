using Avalonia;
using Avalonia.Controls;
using Avalonia.Input;
using Avalonia.Interactivity;
using Lag.Core;
using Lag.ViewModels;

namespace Lag.Views;

public partial class MainWindow : Window
{
    public MainWindow()
    {
        InitializeComponent();

        // Tunnel so the window sees ESC even when focus is inside the native VideoView host.
        AddHandler(KeyDownEvent, OnPreviewKeyDown, RoutingStrategies.Tunnel);

        // Intercept closing to hide the window instead
        Closing += (s, e) =>
        {
            if (Application.Current?.ApplicationLifetime is Avalonia.Controls.ApplicationLifetimes.IClassicDesktopStyleApplicationLifetime desktop &&
                desktop.ShutdownMode == Avalonia.Controls.ShutdownMode.OnExplicitShutdown)
            {
                // This means the app is explicitly shutting down, let it close
                return;
            }

            // Otherwise, hide it to tray
            e.Cancel = true;
            Hide();
        };

        // When the DataContext is set (MainViewModel), wire up active nav highlighting
        DataContextChanged += (_, _) =>
        {
            if (DataContext is MainViewModel vm)
            {
                vm.PropertyChanged += (_, e) =>
                {
                    if (e.PropertyName == nameof(MainViewModel.CurrentView))
                    {
                        UpdateActiveNav(vm.CurrentView);
                    }
                    else if (e.PropertyName == nameof(MainViewModel.IsFullscreen))
                    {
                        ApplyFullscreen(vm.IsFullscreen);
                    }
                };
                // Set initial state
                UpdateActiveNav(vm.CurrentView);
                ApplyFullscreen(vm.IsFullscreen);
            }
        };
    }

    /// <summary>
    /// Toggles the "nav-active" vs "nav" CSS class on sidebar navigation buttons
    /// based on which view is currently selected. Avalonia's Classes property
    /// doesn't support bindings, so we manage this imperatively.
    /// </summary>
    private void UpdateActiveNav(ViewModelBase? currentView)
    {
        SetNavClass(NavSettings, currentView is SettingsViewModel);
        SetNavClass(NavLibrary, currentView is LibraryViewModel);
        SetNavClass(NavPlayer, currentView is PlayerViewModel);
    }

    private static void SetNavClass(Button button, bool isActive)
    {
        button.Classes.Clear();
        button.Classes.Add(isActive ? "nav-active" : "nav");
    }

    /// <summary>
    /// Applies true fullscreen. Besides the window state, we must collapse the reserved title-bar
    /// chrome (ExtendClientAreaTitleBarHeightHint) and drop the system chrome — otherwise Avalonia
    /// keeps a ~44px title-bar strip at the very top even though our custom title bar Border is
    /// hidden. We also zero the content margin so the video truly fills 100% of the screen.
    /// </summary>
    private void ApplyFullscreen(bool isFullscreen)
    {
        if (isFullscreen)
        {
            ExtendClientAreaTitleBarHeightHint = 0;
            ExtendClientAreaChromeHints = Avalonia.Platform.ExtendClientAreaChromeHints.NoChrome;
            WindowState = WindowState.FullScreen;
            if (ContentHost != null) ContentHost.Margin = new Thickness(0);
        }
        else
        {
            WindowState = WindowState.Normal;
            ExtendClientAreaTitleBarHeightHint = 44;
            ExtendClientAreaChromeHints = Avalonia.Platform.ExtendClientAreaChromeHints.PreferSystemChrome;
            if (ContentHost != null) ContentHost.Margin = new Thickness(6, 4, 12, 14);
        }
    }

    /// <summary>ESC exits fullscreen (restores the normal layout).</summary>
    private void OnPreviewKeyDown(object? sender, KeyEventArgs e)
    {
        if (e.Key == Key.Escape && DataContext is MainViewModel { IsFullscreen: true } vm)
        {
            vm.ToggleFullscreenCommand.Execute(null);
            e.Handled = true;
        }
    }
}
