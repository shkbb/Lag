using Avalonia;
using System;
using System.Runtime.InteropServices;
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
    [DllImport("winmm.dll")]
    private static extern uint timeBeginPeriod(uint uMilliseconds);

    // ── Native crash forensics (SEH top-level filter) ──
    private delegate int TopLevelFilter(IntPtr exceptionInfo);
    [DllImport("kernel32.dll")] private static extern IntPtr SetUnhandledExceptionFilter(TopLevelFilter filter);
    private static TopLevelFilter? _nativeCrashFilter;   // kept alive so the GC can't collect the delegate

    private static void InstallNativeCrashHandler()
    {
        _nativeCrashFilter = info =>
        {
            try
            {
                IntPtr record = Marshal.ReadIntPtr(info, 0);     // EXCEPTION_POINTERS.ExceptionRecord
                uint code = (uint)Marshal.ReadInt32(record, 0);  // EXCEPTION_RECORD.ExceptionCode
                IntPtr addr = Marshal.ReadIntPtr(record, 16);    // EXCEPTION_RECORD.ExceptionAddress (x64)
                Lag.Services.FileLog.LogNativeCrash(code, addr, ResolveModule(addr));
            }
            catch { /* a crash handler must never throw */ }
            return 0;   // EXCEPTION_CONTINUE_SEARCH — let the OS finish the crash (WER, dump, exit)
        };
        SetUnhandledExceptionFilter(_nativeCrashFilter);
    }

    /// <summary>Which loaded module an address falls in — names the DLL that faulted.</summary>
    private static string ResolveModule(IntPtr addr)
    {
        try
        {
            long a = addr.ToInt64();
            foreach (System.Diagnostics.ProcessModule m in System.Diagnostics.Process.GetCurrentProcess().Modules)
            {
                long b = m.BaseAddress.ToInt64();
                if (a >= b && a < b + m.ModuleMemorySize) return $"{m.ModuleName}+0x{(a - b):X}";
            }
        }
        catch { }
        return $"0x{addr.ToInt64():X16} (unknown module)";
    }

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
        // Native (SEH) crashes — access violations inside FFmpeg / D3D / VLC — never reach the managed
        // handler above; this top-level filter logs the faulting DLL to crash.log just before death.
        InstallNativeCrashHandler();

        // TRANSITION SHIM — the app now updates itself via GitHub Releases + the Inno installer
        // (AppUpdateService), but users still on the legacy Velopack layout receive ONE last
        // Velopack package whose Update.exe invokes these hooks; keep processing them until the
        // Velopack channel is retired, then drop this and the package reference.
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
            .AfterSetup(_ => RaiseRenderTimerToDisplayRate());

    /// <summary>
    /// Composition rate for the render timer: the highest refresh rate among the connected
    /// displays. A fixed rate that mismatches the panel's Hz beats against vsync and shows
    /// as playback judder (e.g. a 120 Hz timer on a 170 Hz panel repeats/skips frames on an
    /// irregular cadence). Clamped to 60–240 so a bogus driver value can't stall or spin us.
    /// </summary>
    private static int TargetRenderHz()
    {
        uint max = 0;
        try
        {
            foreach (var m in new Lag.Services.HardwareDetector().GetAvailableMonitors())
                if (m.RefreshRate > max) max = m.RefreshRate;
        }
        catch { /* fall through to the 60 Hz floor below */ }
        return Math.Clamp((int)max, 60, 240);
    }

    /// <summary>The app's adjustable render timer (null when the rebind failed and the stock
    /// 60 Hz timer is in use). MainWindow retunes it to the monitor it currently sits on.</summary>
    public static Lag.Services.DynamicRenderTimer? RenderTimer { get; private set; }

    /// <summary>
    /// Avalonia's default render timer ticks at 60 Hz, which silently halves 120 fps replay
    /// playback in the built-in player no matter how fast the decoder is. Rebinds the timer
    /// to our <see cref="Lag.Services.DynamicRenderTimer"/>: it starts at the fastest connected
    /// display's rate (<see cref="TargetRenderHz"/>) and is then retuned live by MainWindow to
    /// the refresh rate of the monitor the window is actually on (a background periodic timer,
    /// like DefaultRenderTimer — the UI-thread variant caps at ≈76 Hz). AvaloniaLocator is
    /// public at runtime but excluded from the 11.2 reference assemblies, hence reflection.
    /// Any failure leaves the stock 60 Hz timer in place.
    /// </summary>
    private static void RaiseRenderTimerToDisplayRate()
    {
        int hz = TargetRenderHz();
        try
        {
            var timer = new Lag.Services.DynamicRenderTimer(hz);

            var baseAsm = typeof(Avalonia.Rendering.IRenderTimer).Assembly;
            var locatorType = baseAsm.GetType("Avalonia.AvaloniaLocator")
                ?? throw new MissingMemberException("AvaloniaLocator");
            var locator = locatorType.GetProperty("CurrentMutable")!.GetValue(null)!;
            var helper = locatorType.GetMethod("Bind")!
                .MakeGenericMethod(typeof(Avalonia.Rendering.IRenderTimer))
                .Invoke(locator, null)!;
            var toConstant = helper.GetType().GetMethod("ToConstant")!;
            if (toConstant.IsGenericMethodDefinition)
                toConstant = toConstant.MakeGenericMethod(typeof(Avalonia.Rendering.IRenderTimer));
            toConstant.Invoke(helper, new object[] { timer.Interface });

            RenderTimer = timer;

            // Verification: if the compositor really adopted our timer it will subscribe and
            // ticks arrive at ~Hz/s. Counted over t=8–10s (startup starvation would skew earlier).
            int ticks = 0;
            var sw = System.Diagnostics.Stopwatch.StartNew();
            timer.AddTick(_ =>
            {
                long ms = sw.ElapsedMilliseconds;
                if (ms is >= 8000 and < 10000 && ticks >= 0) ticks++;
                else if (ms >= 10000 && ticks > 0)
                {
                    Console.WriteLine($"[RenderTimer] ~{ticks / 2} ticks/s (current target {timer.Hz}).");
                    ticks = int.MinValue; // log once
                }
            });
            Console.WriteLine($"[RenderTimer] Bound DynamicRenderTimer @ {hz} Hz (fastest display; follows the window's monitor).");
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[RenderTimer] rebind failed, staying at 60 Hz: {ex}");
        }
    }
}
