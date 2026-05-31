using Avalonia;
using System;
using System.Threading;
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
        // for those special invocations before any UI is created. (Velopack maintenance must not
        // be blocked by the single-instance guard below.)
        VelopackApp.Build().Run();

        // Single-instance guard: if another copy of Lag already holds the mutex, exit immediately.
        // 'using' keeps the mutex alive for the whole app lifetime and releases it on exit.
        using var mutex = new Mutex(true, "LagAppSingleInstanceMutex", out bool createdNew);
        if (!createdNew)
            return;

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
