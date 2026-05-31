using Avalonia;
using System;
using Velopack;

namespace Lag;

/// <summary>
/// Application entry point. Configures and launches the Avalonia application.
/// </summary>
public static class Program
{
    /// <summary>
    /// Main entry point for the application.
    /// </summary>
    [STAThread]
    public static void Main(string[] args)
    {
        // MUST run first: handles Velopack's install/update/uninstall hooks and exits early
        // for those special invocations before any UI is created.
        VelopackApp.Build().Run();

        BuildAvaloniaApp().StartWithClassicDesktopLifetime(args);
    }

    /// <summary>
    /// Builds the Avalonia application configuration with platform detection.
    /// </summary>
    public static AppBuilder BuildAvaloniaApp()
        => AppBuilder.Configure<App>()
            .UsePlatformDetect()
            .WithInterFont()
            .LogToTrace();
}
