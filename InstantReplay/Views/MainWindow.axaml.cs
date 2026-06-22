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

            // Hide to tray. Stop video playback first — the window is only hidden (the view
            // stays attached), so without this VLC keeps playing audibly from the tray. The editor
            // has its OWN MediaPlayer, so stop that too (the bug: editor audio kept playing in tray).
            if (DataContext is MainViewModel mainVm)
            {
                mainVm.Player.StopPlayback();
                mainVm.Editor.StopPreview();
            }

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
                    else if (e.PropertyName == nameof(MainViewModel.IsRecording))
                    {
                        UpdateRecordCta(vm.IsRecording);
                    }
                };
                // Set initial state
                UpdateActiveNav(vm.CurrentView);
                UpdateRecordCta(vm.IsRecording);
                ApplyFullscreen(vm.IsFullscreen);
            }
        };
    }

    /// <summary>
    /// Toggles the "active" class on the icon-rail items based on the current view. Avalonia's
    /// Classes property doesn't support bindings, so this is imperative.
    /// </summary>
    private void UpdateActiveNav(ViewModelBase? currentView)
    {
        RailLibrary.Classes.Set("active", currentView is LibraryViewModel);
        RailPlayer.Classes.Set("active", currentView is PlayerViewModel);
        RailEditor.Classes.Set("active", currentView is EditorViewModel);
        RailSettings.Classes.Set("active", currentView is SettingsViewModel);
    }

    /// <summary>Swaps the CTA between cyan "Start" and red "Stop" recording state.</summary>
    private void UpdateRecordCta(bool isRecording) => RecordCta.Classes.Set("rec", isRecording);

    /// <summary>True fullscreen: window state only — sidebar/titlebar collapse via bindings.</summary>
    private void ApplyFullscreen(bool isFullscreen)
    {
        WindowState = isFullscreen ? WindowState.FullScreen : WindowState.Normal;
    }

    // ── Custom window chrome ──

    private void OnTitleBarPointerPressed(object? sender, PointerPressedEventArgs e)
    {
        if (e.GetCurrentPoint(this).Properties.IsLeftButtonPressed)
            BeginMoveDrag(e);
    }

    private void OnMinimizeClick(object? sender, RoutedEventArgs e) =>
        WindowState = WindowState.Minimized;

    private void OnMaximizeClick(object? sender, RoutedEventArgs e) =>
        WindowState = WindowState == WindowState.Maximized ? WindowState.Normal : WindowState.Maximized;

    private void OnCloseClick(object? sender, RoutedEventArgs e) => Close();

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
