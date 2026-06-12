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
        // Crash forensics: any unhandled exception lands in %AppData%\Lag\crash.log with a
        // full stack trace, so failures on users' machines can be diagnosed remotely.
        AppDomain.CurrentDomain.UnhandledException += (_, e) =>
            LogCrash("AppDomain", e.ExceptionObject as Exception);
        System.Threading.Tasks.TaskScheduler.UnobservedTaskException += (_, e) =>
            LogCrash("UnobservedTask", e.Exception);

        // MUST run first: handles Velopack's install/update/uninstall hooks and exits early
        // for those special invocations before any UI is created. (Velopack maintenance must not
        // be blocked by the single-instance guard below.)
        VelopackApp.Build().Run();

        // Single-instance guard: if another copy of Lag already holds the mutex, exit immediately.
        // 'using' keeps the mutex alive for the whole app lifetime and releases it on exit.
        using var mutex = new Mutex(true, "LagAppSingleInstanceMutex", out bool createdNew);
        if (!createdNew)
            return;

        try
        {
            BuildAvaloniaApp().StartWithClassicDesktopLifetime(args);
        }
        catch (Exception ex)
        {
            LogCrash("Main", ex);
            throw;
        }
    }

    /// <summary>Appends a crash record to %AppData%\Lag\crash.log (never throws).</summary>
    private static void LogCrash(string source, Exception? ex)
    {
        try
        {
            string dir = System.IO.Path.Combine(
                Environment.GetFolderPath(Environment.SpecialFolder.ApplicationData), "Lag");
            System.IO.Directory.CreateDirectory(dir);
            System.IO.File.AppendAllText(
                System.IO.Path.Combine(dir, "crash.log"),
                $"───── {DateTime.Now:yyyy-MM-dd HH:mm:ss} [{source}] v{typeof(Program).Assembly.GetName().Version} ─────{Environment.NewLine}" +
                $"{ex}{Environment.NewLine}{Environment.NewLine}");
        }
        catch
        {
            // logging must never take the process down with it
        }
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
