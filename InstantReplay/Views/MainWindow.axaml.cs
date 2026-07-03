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

        // Keep the render timer matched to the refresh rate of the monitor this window is ON
        // (fullscreen on the 165 Hz panel → 165 Hz composition; drag to a 180 Hz panel → 180).
        // A mismatched fixed rate beats against vsync and shows as playback judder.
        Opened += (_, _) => RetuneRenderTimer();
        PositionChanged += (_, _) => RetuneRenderTimer();
        PropertyChanged += (_, e) =>
        {
            if (e.Property == WindowStateProperty)
                Avalonia.Threading.Dispatcher.UIThread.Post(RetuneRenderTimer);
        };

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

    /// <summary>Points the render timer at the refresh rate of the monitor under this window.
    /// Cheap enough to run on every PositionChanged tick (three user32 calls; the timer ignores
    /// writes that don't change the value).</summary>
    private void RetuneRenderTimer()
    {
        if (Program.RenderTimer is not { } timer) return;
        var handle = TryGetPlatformHandle()?.Handle ?? System.IntPtr.Zero;
        if (handle == System.IntPtr.Zero) return;

        // Hz feeds the clock fallback; the HMONITOR lets the timer tick on the panel's REAL
        // vblank (DXGI WaitForVBlank) — phase-locked composition, like a native video player.
        uint hz = Lag.Services.HardwareDetector.GetWindowRefreshRate(handle);
        if (hz > 0) timer.Hz = (int)hz;
        timer.SetMonitor(Lag.Services.HardwareDetector.GetWindowMonitor(handle));
    }

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

    // ── Capture-target hover flyout (rail icon → animated panel with the game/desktop name) ──

    private void OnCaptureEntered(object? sender, PointerEventArgs e) =>
        CaptureFlyout.Classes.Set("shown", true);

    private void OnCaptureExited(object? sender, PointerEventArgs e) =>
        CaptureFlyout.Classes.Set("shown", false);

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
