using System.Runtime.InteropServices;
using Avalonia;
using Avalonia.Controls;
using Avalonia.Controls.ApplicationLifetimes;
using Avalonia.Markup.Xaml;
using Avalonia.Markup.Xaml.Styling;
using System.IO;
using FFMpegCore;
using Microsoft.Extensions.DependencyInjection;
using Lag.Services;
using Lag.Services.ObsIntegration;
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

    public override void Initialize()
    {
        // Crucial: Set Working Directory explicitly so libobs internal shaders resolve properly
        string obsCoreDir = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "obs-core"));
        Environment.CurrentDirectory = obsCoreDir;
        
        // Crucial: Allow DllImports to find obs.dll inside the obs-core folder
        SetDllDirectory(obsCoreDir);

        // Crucial: Inject obs-core into the process PATH so that deeply nested plugin dependencies
        // (like avcodec-61.dll for obs-ffmpeg or graphics-hook64.dll for win-capture) 
        // can be natively resolved by the Windows module loader (Fixes LoadLibrary Error 126)
        string currentPath = Environment.GetEnvironmentVariable("PATH") ?? string.Empty;
        Environment.SetEnvironmentVariable("PATH", obsCoreDir + ";" + currentPath);

        // Configure FFMpegCore to use ffmpeg.exe/ffprobe.exe from our bundled obs-core directory
        GlobalFFOptions.Configure(new FFOptions { BinaryFolder = obsCoreDir });

        AvaloniaXamlLoader.Load(this);

        // Load the default language dictionary (English) so {DynamicResource} keys resolve
        // before any view is created. SettingsViewModel may switch it to the persisted language.
        SetLanguage("en");
    }

    // ───────────── Localization ─────────────

    private static IResourceProvider? _currentLanguage;

    /// <summary>Language codes with a dictionary in Assets/Langs. "en" is the fallback.</summary>
    private static readonly string[] SupportedLanguages =
        ["en", "uk", "de", "fr", "be", "lt", "et", "lv", "fi", "sv", "no", "da", "nl", "it"];

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
            var mainViewModel = _serviceProvider.GetRequiredService<MainViewModel>();

            desktop.MainWindow = new MainWindow
            {
                DataContext = mainViewModel
            };

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
                    if (mainViewModel.SaveReplayCommand.CanExecute(null))
                    {
                        mainViewModel.SaveReplayCommand.Execute(null);
                    }
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

        // ── OBS Integration services ──
        services.AddSingleton<HardwareDetector>();
        services.AddSingleton<ObsRecorderService>();

        // ── ViewModels ──
        services.AddSingleton<SettingsViewModel>();
        services.AddSingleton<LibraryViewModel>();
        services.AddSingleton<PlayerViewModel>();
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
        if (ApplicationLifetime is IClassicDesktopStyleApplicationLifetime desktop)
        {
            // Set ShutdownMode to OnExplicitShutdown to bypass our Closing hide logic
            desktop.ShutdownMode = Avalonia.Controls.ShutdownMode.OnExplicitShutdown;
            
            // Dispose everything properly through DI container if needed, though ShutdownRequested handles it
            desktop.Shutdown();
        }
    }
}
