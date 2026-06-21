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
    [System.Runtime.InteropServices.DllImport("winmm.dll")]
    private static extern uint timeBeginPeriod(uint uMilliseconds);

    [STAThread]
    public static void Main(string[] args)
    {
        // Capture all console diagnostics to a timestamped rolling .txt (%AppData%\Lag\logs) FIRST,
        // so everything from this point — engine, capture, audio, UI — lands in the log file.
        Lag.Services.FileLog.Initialize();

        // 1 ms system timer granularity: the 120 Hz render timer (and other media timers)
        // are dispatcher-driven and can't tick faster than ~64 Hz at the default 15.6 ms
        // resolution. Held for the process lifetime on purpose — this is a media app.
        timeBeginPeriod(1);

        // Headless end-to-end test of the native VFR engine (no UI). Set LAG_VFR_TEST=1 and run.
        if (Environment.GetEnvironmentVariable("LAG_VFR_TEST") == "1")
        {
            Lag.Services.VfrCapture.VfrEngineTest.Run();
            return;
        }

        // Headless validation of the per-app (process) loopback interop: LAG_PROCTEST=<pid>.
        if (int.TryParse(Environment.GetEnvironmentVariable("LAG_PROCTEST"), out var procTestPid))
        {
            Lag.Services.VfrCapture.ProcessLoopbackCapture.SelfTest(procTestPid);
            return;
        }

        // Crash forensics: any unhandled exception lands in Documents\Lag\Logs\crash.log with a full
        // stack trace AND is echoed into the current session log, so failures on users' machines can
        // be diagnosed remotely.
        AppDomain.CurrentDomain.UnhandledException += (_, e) =>
            Lag.Services.FileLog.LogCrash("AppDomain", e.ExceptionObject as Exception);
        System.Threading.Tasks.TaskScheduler.UnobservedTaskException += (_, e) =>
            Lag.Services.FileLog.LogCrash("UnobservedTask", e.Exception);

        // MUST run first: handles Velopack's install/update/uninstall hooks and exits early
        // for those special invocations before any UI is created. (Velopack maintenance must not
        // be blocked by the single-instance guard below.)
        VelopackApp.Build().Run();

        // Single-instance guard: if another copy of Lag already holds the mutex, exit immediately.
        // 'using' keeps the mutex alive for the whole app lifetime and releases it on exit.
        Mutex mutex;
        try
        {
            mutex = new Mutex(true, "LagAppSingleInstanceMutex", out bool createdNew);
            if (!createdNew)
                return;
        }
        catch (UnauthorizedAccessException)
        {
            // The mutex exists but was created by a process with different elevation
            // (e.g. an elevated instance vs. a normal one) — we can't even open it.
            // It still means another instance is running: two recorders would fight
            // over hotkeys and double the NVENC load, so exit just the same.
            return;
        }

        using var _ = mutex;

        try
        {
            BuildAvaloniaApp().StartWithClassicDesktopLifetime(args);
        }
        catch (Exception ex)
        {
            Lag.Services.FileLog.LogCrash("Main", ex);
            throw;
        }
    }

    /// <summary>
    /// Builds the Avalonia application configuration with platform detection.
    /// </summary>
    public static AppBuilder BuildAvaloniaApp()
        => AppBuilder.Configure<App>()
            .UsePlatformDetect()
            .WithInterFont()
            .LogToTrace()
            .AfterSetup(_ => RaiseRenderTimerTo120Fps());

    /// <summary>
    /// Avalonia's default render timer ticks at 60 Hz, which silently halves 120 fps replay
    /// playback in the built-in player no matter how fast the decoder is. Rebinds the timer
    /// to 120 Hz. AvaloniaLocator/UiThreadRenderTimer are public at runtime but excluded
    /// from the 11.2 reference assemblies, hence reflection. Any failure leaves the stock
    /// 60 Hz timer in place.
    /// </summary>
    private static void RaiseRenderTimerTo120Fps()
    {
        try
        {
            var baseAsm = typeof(Avalonia.Rendering.IRenderTimer).Assembly;
            // DefaultRenderTimer, not UiThreadRenderTimer: the UI-thread variant waits the
            // full interval AFTER each composition pass finishes (work + 8.3 ms ≈ 13 ms ≈
            // 76 Hz ceiling, measured). The background periodic timer keeps a true cadence.
            var timerType = baseAsm.GetType("Avalonia.Rendering.DefaultRenderTimer")
                ?? throw new MissingMemberException("DefaultRenderTimer");
            var timer = (Avalonia.Rendering.IRenderTimer)Activator.CreateInstance(timerType, 120)!;

            var locatorType = baseAsm.GetType("Avalonia.AvaloniaLocator")
                ?? throw new MissingMemberException("AvaloniaLocator");
            var locator = locatorType.GetProperty("CurrentMutable")!.GetValue(null)!;
            var helper = locatorType.GetMethod("Bind")!
                .MakeGenericMethod(typeof(Avalonia.Rendering.IRenderTimer))
                .Invoke(locator, null)!;
            var toConstant = helper.GetType().GetMethod("ToConstant")!;
            if (toConstant.IsGenericMethodDefinition)
                toConstant = toConstant.MakeGenericMethod(typeof(Avalonia.Rendering.IRenderTimer));
            toConstant.Invoke(helper, new object[] { timer });

            // Verification: if the compositor really adopted our timer it will Start() it on
            // the first subscription and ticks arrive at ~120/s. Counted over the first 2s.
            // (IRenderTimer.Tick is also hidden from the reference assemblies.)
            // Window starts at t=8s: during startup the UI thread is busy with OBS init and
            // dispatcher-driven ticks are starved, which would skew the measurement.
            int ticks = 0;
            var sw = System.Diagnostics.Stopwatch.StartNew();
            Action<TimeSpan> onTick = _ =>
            {
                long ms = sw.ElapsedMilliseconds;
                if (ms is >= 8000 and < 10000 && ticks >= 0) ticks++;
                else if (ms >= 10000 && ticks > 0)
                {
                    Console.WriteLine($"[RenderTimer] ~{ticks / 2} ticks/s (target 120).");
                    ticks = int.MinValue; // log once
                }
            };
            timerType.GetEvent("Tick")!.AddEventHandler(timer, onTick);
            Console.WriteLine("[RenderTimer] Rebound to 120 fps DefaultRenderTimer.");
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[RenderTimer] 120 fps rebind failed, staying at 60: {ex}");
        }
    }
}
