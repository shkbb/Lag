using System.Runtime.InteropServices;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Markup.Xaml;
using Avalonia.Markup.Xaml.Styling;
using Avalonia.Styling;
using System.IO;
using FFMpegCore;
using Microsoft.Extensions.DependencyInjection;
using Lag.Services;
using Lag.ViewModels;
using Lag.Views;

namespace Lag;

/// <summary>
/// Application entry class. Configures dependency injection, registers
/// platform-specific services, and creates the main window.
/// </summary>
public class App : Application
{
    private ServiceProvider? _serviceProvider;

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    static extern bool SetDllDirectory(string lpPathName);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern IntPtr AddDllDirectory(string newDirectory);

    public override void Initialize()
    {
        // Point the process at our bundled FFmpeg (shared 7.1 build):
        // set it as the working dir + DLL search dir so avcodec-61 and its siblings resolve their
        // inter-dependencies, prepend it to PATH, and tell FFMpegCore (editor export) where
        // ffmpeg.exe / ffprobe.exe live.
        string ffmpegDir = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "ffmpeg"));
        Environment.CurrentDirectory = ffmpegDir;
        SetDllDirectory(ffmpegDir);
        AddDllDirectory(ffmpegDir);
        string currentPath = Environment.GetEnvironmentVariable("PATH") ?? string.Empty;
        Environment.SetEnvironmentVariable("PATH", ffmpegDir + ";" + currentPath);
        GlobalFFOptions.Configure(new FFOptions { BinaryFolder = ffmpegDir });

        // Bind the native FFmpeg libraries now (idempotent). The encoder probe in EncoderSelector
        // only runs once and caches its result, and the Settings codec dropdown triggers it during
        // ViewModel construction — so FFmpeg must be resolvable BEFORE any of that, or the probe
        // finds zero encoders and recording fails with "No usable video encoder".
        Lag.Services.VfrCapture.FfmpegInterop.Initialize(ffmpegDir);

        AvaloniaXamlLoader.Load(this);

        // Load the default language dictionary (English) so {DynamicResource} keys resolve
        // before any view is created. SettingsViewModel may switch it to the persisted language.
        SetLanguage("en");
    }

    // ───────────── Localization ─────────────

    private static IResourceProvider? _currentLanguage;

    /// <summary>Language codes with a dictionary in Assets/Langs. "en" is the fallback.</summary>
    private static readonly string[] SupportedLanguages =
        ["en", "uk", "de", "fr", "be", "lt", "et", "lv", "fi", "sv", "no", "da", "nl", "it", "es", "pt", "ja"];

    /// <summary>
    /// Swaps the active language ResourceDictionary in the application's merged dictionaries.
    /// Unsupported/unknown codes fall back to English. All {DynamicResource} bindings update live.
    /// </summary>
    public static void SetLanguage(string? code)
    {
        if (Current is null) return;

        string lang = SupportedLanguages.Contains(code) ? code! : "en";
        var uri = new Uri($"avares://Lag/Assets/Langs/{lang}.axaml");
        var include = new ResourceInclude(uri) { Source = uri };

        if (_currentLanguage != null)
            Current.Resources.MergedDictionaries.Remove(_currentLanguage);

        Current.Resources.MergedDictionaries.Add(include);
        _currentLanguage = include;
    }

    // ───────────── Theme ─────────────

    /// <summary>The snapshot overlay of a theme cross-fade still in flight (null between fades).</summary>
    private static Avalonia.Controls.Image? _themeFadeOverlay;

    /// <summary>
    /// Switches the active colour theme. "light" / "dark" force the cream Light or the
    /// original Dark palette; anything else ("system") follows the OS theme live via
    /// ThemeVariant.Default. All {DynamicResource} bindings re-resolve from Palette.axaml's
    /// per-variant ThemeDictionaries the instant the variant changes.
    ///
    /// The visible switch is a whole-window CROSS-FADE: the current UI is frozen into a
    /// bitmap overlaid on top, the variant flips instantly underneath, and the frozen frame
    /// fades out — so every surface (including plain backgrounds that have no Transitions)
    /// appears to melt into the new theme together. Skipped at startup (no visible window
    /// yet) and when the flip causes no actual variant change (e.g. system == current).
    /// </summary>
    public static void SetTheme(string? mode)
    {
        if (Current is null) return;

        var target = mode switch
        {
            "light" => ThemeVariant.Light,
            "dark" => ThemeVariant.Dark,
            _ => ThemeVariant.Default,   // "system" → inherit the OS light/dark setting (updates live)
        };

        var win = (Current.ApplicationLifetime as IClassicDesktopStyleApplicationLifetime)?.MainWindow;
        var before = Current.ActualThemeVariant;

        if (win is { IsVisible: true } && TrySnapshotWindow(win, out var overlay))
        {
            Current.RequestedThemeVariant = target;
            if (Current.ActualThemeVariant == before)
            {
                RemoveThemeFadeOverlay(overlay);   // nothing visibly changed — drop the frozen frame
                return;
            }
            _themeFadeOverlay = overlay;
            FadeOutThemeOverlay(overlay);
        }
        else
        {
            Current.RequestedThemeVariant = target;
        }
    }

    /// <summary>Freezes the window's current pixels into an Image parked in its OverlayLayer.</summary>
    private static bool TrySnapshotWindow(Window win, out Avalonia.Controls.Image overlay)
    {
        overlay = null!;
        try
        {
            // A fade may still be running from a rapid previous switch — clear it first so the
            // new snapshot doesn't capture the half-faded old frame on top of everything.
            if (_themeFadeOverlay is { } old) RemoveThemeFadeOverlay(old);

            var layer = Avalonia.Controls.Primitives.OverlayLayer.GetOverlayLayer(win);
            if (layer is null) return false;

            double scale = win.RenderScaling;
            var size = new PixelSize(
                (int)Math.Ceiling(win.ClientSize.Width * scale),
                (int)Math.Ceiling(win.ClientSize.Height * scale));
            if (size.Width <= 0 || size.Height <= 0) return false;

            var bmp = new Avalonia.Media.Imaging.RenderTargetBitmap(size, new Vector(96 * scale, 96 * scale));
            bmp.Render(win);

            overlay = new Avalonia.Controls.Image
            {
                Source = bmp,
                Width = win.ClientSize.Width,
                Height = win.ClientSize.Height,
                Stretch = Avalonia.Media.Stretch.Fill,
                IsHitTestVisible = false,
            };
            layer.Children.Add(overlay);
            return true;
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[App] theme snapshot failed: {ex.Message}");
            return false;   // fall back to the instant switch — never break theming over polish
        }
    }

    private static void FadeOutThemeOverlay(Avalonia.Controls.Image overlay)
    {
        overlay.Transitions = new Avalonia.Animation.Transitions
        {
            new Avalonia.Animation.DoubleTransition
            {
                Property = Visual.OpacityProperty,
                Duration = TimeSpan.FromMilliseconds(280),
                Easing = new Avalonia.Animation.Easings.CubicEaseOut(),
            }
        };
        // Next dispatcher frame: the re-themed UI beneath is already composed, start the fade.
        Avalonia.Threading.Dispatcher.UIThread.Post(() => overlay.Opacity = 0,
            Avalonia.Threading.DispatcherPriority.Background);
        Avalonia.Threading.DispatcherTimer.RunOnce(() => RemoveThemeFadeOverlay(overlay),
            TimeSpan.FromMilliseconds(600));
    }

    private static void RemoveThemeFadeOverlay(Avalonia.Controls.Image overlay)
    {
        if (ReferenceEquals(_themeFadeOverlay, overlay)) _themeFadeOverlay = null;
        var source = overlay.Source as IDisposable;
        (overlay.Parent as Panel)?.Children.Remove(overlay);
        overlay.Source = null;
        source?.Dispose();
    }

    /// <summary>
    /// Called when the framework initialization is complete.
    /// Sets up the DI container with platform-aware service registration.
    /// </summary>
    public override void OnFrameworkInitializationCompleted()
    {
        var services = new ServiceCollection();
        ConfigureServices(services);
        _serviceProvider = services.BuildServiceProvider();

        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            // Opt out of Windows background throttling (EcoQoS) and raise priority a notch:
            // otherwise a focused fullscreen game starves the capture pipeline (2 FPS replays)
            // and delays hotkey delivery until alt-tab.
            PerformanceGuard.Apply();

            var mainViewModel = _serviceProvider.GetRequiredService<MainViewModel>();

            var mainWindow = new MainWindow
            {
                DataContext = mainViewModel
            };

            // "Start minimized": the desktop lifetime always shows MainWindow right after
            // initialization, so suppress the flash by starting minimized with no taskbar
            // button and hiding on the first Opened. The tray "Open" handler restores
            // WindowState to Normal, so we don't touch it here (setting WindowState on a
            // hidden Win32 window can re-show it).
            if (mainViewModel.Settings.StartMinimized)
            {
                mainWindow.WindowState = Avalonia.Controls.WindowState.Minimized;
                mainWindow.ShowInTaskbar = false;
                EventHandler? hideOnce = null;
                hideOnce = (_, _) =>
                {
                    mainWindow.Opened -= hideOnce;
                    mainWindow.Hide();
                    mainWindow.ShowInTaskbar = true;
                };
                mainWindow.Opened += hideOnce;
            }

            desktop.MainWindow = mainWindow;

            // Start the global hotkey listener (SharpHook - legacy, keeping for Settings capture if needed)
            var hotkeyManager = _serviceProvider.GetRequiredService<GlobalHotkeyManager>();
            _ = hotkeyManager.StartAsync();

            // Start the Win32 global hotkey service (Alt+F10 default)
            var globalHotkeyService = _serviceProvider.GetRequiredService<GlobalHotkeyService>();
            // 0x0001 = MOD_ALT, 0x79 = VK_F10
            globalHotkeyService.Start(0x0001, 0x79);
            globalHotkeyService.HotkeyPressed += (_, _) =>
            {
                // Must marshal to UI thread because HotkeyPressed fires from a background thread
                Avalonia.Threading.Dispatcher.UIThread.Post(() =>
                {
                    // Ignore while the user is rebinding hotkeys in Settings (the old
                    // registration is still live and would fire mid-capture).
                    if (mainViewModel.Settings.AreHotkeysSuppressed) return;

                    if (mainViewModel.SaveReplayCommand.CanExecute(null))
                    {
                        mainViewModel.SaveReplayCommand.Execute(null);
                    }
                });
            };

            // Screenshot hotkey (separate combo, configured in Settings → General).
            globalHotkeyService.ScreenshotPressed += (_, _) =>
            {
                Avalonia.Threading.Dispatcher.UIThread.Post(() =>
                {
                    if (mainViewModel.Settings.AreHotkeysSuppressed) return;
                    if (mainViewModel.TakeScreenshotCommand.CanExecute(null))
                        mainViewModel.TakeScreenshotCommand.Execute(null);
                });
            };

            // Pause/resume-recording hotkey (separate combo, configured in Settings → General).
            globalHotkeyService.PausePressed += (_, _) =>
            {
                Avalonia.Threading.Dispatcher.UIThread.Post(() =>
                {
                    if (mainViewModel.Settings.AreHotkeysSuppressed) return;
                    if (mainViewModel.TogglePauseRecordingCommand.CanExecute(null))
                        mainViewModel.TogglePauseRecordingCommand.Execute(null);
                });
            };

            // Start/stop-recording toggle hotkey (separate combo, configured in Settings → General).
            globalHotkeyService.RecordPressed += (_, _) =>
            {
                Avalonia.Threading.Dispatcher.UIThread.Post(() =>
                {
                    if (mainViewModel.Settings.AreHotkeysSuppressed) return;
                    if (mainViewModel.ToggleRecordingCommand.CanExecute(null))
                        mainViewModel.ToggleRecordingCommand.Execute(null);
                });
            };

            // Clean up on shutdown
            desktop.ShutdownRequested += (_, _) =>
            {
                _serviceProvider?.Dispose();
            };

            // Apply the persisted UI language to the tray menu (SettingsViewModel has already
            // loaded it as a side effect of constructing MainViewModel above).
            LocalizeTrayMenu();
        }

        base.OnFrameworkInitializationCompleted();
    }

    /// <summary>
    /// Localizes the tray menu headers via Localizer. Done in code because NativeMenuItem is not
    /// part of the logical tree, so {DynamicResource} cannot reach it from XAML.
    /// </summary>
    private void LocalizeTrayMenu()
    {
        try
        {
            var icons = TrayIcon.GetIcons(this);
            if (icons is not { Count: > 0 } || icons[0].Menu is not { } menu) return;

            string[] keys = ["Tray_SaveReplay", "Tray_OpenLibrary", "", "Tray_Open", "Tray_Exit"];
            for (int i = 0; i < menu.Items.Count && i < keys.Length; i++)
            {
                if (menu.Items[i] is NativeMenuItem item && keys[i].Length > 0)
                    item.Header = Core.Localizer.Get(keys[i]);
            }
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"Tray menu localization failed: {ex.Message}");
        }
    }

    /// <summary>
    /// Registers all services and ViewModels in the DI container.
    /// Platform-specific capture service is selected based on the runtime OS.
    /// </summary>
    private static void ConfigureServices(IServiceCollection services)
    {
        // ── Core services ──
        services.AddSingleton<GlobalHotkeyManager>();
        services.AddSingleton<GlobalHotkeyService>();

        // ── Recording engine ──
        services.AddSingleton<HardwareDetector>();
        services.AddSingleton<Lag.Services.VfrCapture.VfrRecorderAdapter>();
        // The UI talks to IReplayRecorder. The native WGC VFR engine is THE recorder on every
        // supported machine (WGC + FFmpeg + an encoder, i.e. all of Win10 1903+). No Settings
        // dependency here, so SettingsViewModel can itself depend on IReplayRecorder without a DI cycle.
        services.AddSingleton<IReplayRecorder>(sp =>
            sp.GetRequiredService<Lag.Services.VfrCapture.VfrRecorderAdapter>());

        // ── ViewModels ──
        services.AddSingleton<SettingsViewModel>();
        services.AddSingleton<LibraryViewModel>();
        services.AddSingleton<PlayerViewModel>();
        services.AddSingleton<EditorViewModel>();
        services.AddSingleton<MainViewModel>();
    }

    private void TrayIcon_OpenClicked(object? sender, EventArgs e)
    {
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop &&
            desktop.MainWindow != null)
        {
            desktop.MainWindow.Show();
            desktop.MainWindow.WindowState = Avalonia.Controls.WindowState.Normal;
            desktop.MainWindow.Activate();
        }
    }

    /// <summary>Tray → "Save Replay": triggers the same command as the global hotkey / sidebar button.</summary>
    private void TrayIcon_SaveReplayClicked(object? sender, EventArgs e)
    {
        var vm = _serviceProvider?.GetService<MainViewModel>();
        if (vm != null && vm.SaveReplayCommand.CanExecute(null))
            vm.SaveReplayCommand.Execute(null);
    }

    /// <summary>Tray → "Open Library": restores the window and navigates to the Library view.</summary>
    private void TrayIcon_OpenLibraryClicked(object? sender, EventArgs e)
    {
        TrayIcon_OpenClicked(sender, e);

        var vm = _serviceProvider?.GetService<MainViewModel>();
        if (vm != null && vm.NavigateToLibraryCommand.CanExecute(null))
            vm.NavigateToLibraryCommand.Execute(null);
    }

    private void TrayIcon_ExitClicked(object? sender, EventArgs e)
    {
        // The window is already hidden in the tray, so there's nothing to visually close — just tear
        // down and make sure the process actually dies. libVLC (MediaPlayer.Stop / LibVLC.Dispose)
        // and the native encode/capture/audio subsystems can block on Dispose or keep unmanaged
        // threads alive (the 120 fps render-timer thread too); a plain Shutdown() therefore left the
        // process lingering in the background. So a background watchdog force-terminates after a
        // short grace period no matter what, while we run the normal teardown on the UI thread
        // (where the DispatcherTimers expect to be stopped). Whichever finishes first ends the app.
        new Thread(() =>
        {
            Thread.Sleep(2500);
            Environment.Exit(0);
        })
        { IsBackground = true, Name = "ExitWatchdog" }.Start();

        try { _serviceProvider?.Dispose(); } catch { /* never let a Dispose fault block exit */ }
        Environment.Exit(0);
    }
}
