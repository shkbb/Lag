using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Threading;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Headless end-to-end smoke test for the native VFR engine, invoked via LAG_VFR_TEST=1 so we can
/// validate capture→convert→encode→buffer→mux on a live game without the whole app/UI. Arms the
/// engine, lets it lock onto whatever game is running, buffers for a bit, saves, prints the path.
/// </summary>
public static class VfrEngineTest
{
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr AddDllDirectory(string dir);
    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetDefaultDllDirectories(uint flags);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern bool SetDllDirectory(string dir);

    public static void Run()
    {
        string ffmpegDir = Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "ffmpeg"));
        SetDefaultDllDirectories(0x00001000 /* LOAD_LIBRARY_SEARCH_DEFAULT_DIRS */);
        AddDllDirectory(ffmpegDir);
        SetDllDirectory(ffmpegDir);
        Environment.CurrentDirectory = ffmpegDir;
        string path = ffmpegDir + ";" + (Environment.GetEnvironmentVariable("PATH") ?? "");
        Environment.SetEnvironmentVariable("PATH", path);

        Console.WriteLine($"[VfrTest] WGC supported: {WgcCaptureSource.IsSupported}");
        Console.WriteLine($"[VfrTest] Engine available: {VfrReplayEngine.IsAvailable(ffmpegDir)}");

        string outDir = Environment.GetEnvironmentVariable("LAG_VFR_OUT") ?? Path.Combine(Path.GetTempPath(), "lag_vfr_test");
        Directory.CreateDirectory(outDir);

        // Optional forced target by PID (lets us validate against a backgrounded game window).
        IntPtr forced = IntPtr.Zero;
        if (int.TryParse(Environment.GetEnvironmentVariable("LAG_VFR_PID"), out var pid))
        {
            try
            {
                var proc = System.Diagnostics.Process.GetProcessById(pid);
                forced = proc.MainWindowHandle;
                Console.WriteLine($"[VfrTest] Forced target pid {pid} '{proc.ProcessName}' hwnd 0x{forced:X}");
            }
            catch (Exception ex) { Console.WriteLine($"[VfrTest] forced pid lookup failed: {ex.Message}"); }
        }

        // Desktop/monitor capture mode (LAG_VFR_DESKTOP=1): record the whole monitor, no game lock.
        // In this mode we ignore any forced PID so the monitor path actually runs.
        bool desktop = Environment.GetEnvironmentVariable("LAG_VFR_DESKTOP") == "1";
        if (desktop) forced = IntPtr.Zero;

        using var engine = new VfrReplayEngine();
        engine.ReplaySaved += (_, p) => Console.WriteLine($"[VfrTest] SAVED: {p}");
        engine.Start(new VfrEngineOptions
        {
            LibraryPath = outDir,
            BufferSeconds = 30,
            PreferredCodec = Environment.GetEnvironmentVariable("LAG_VFR_CODEC"), // null/"" = auto
            BitrateKbps = int.TryParse(Environment.GetEnvironmentVariable("LAG_VFR_BITRATE"), out var br) ? br : 30000,
            FileFormat = Environment.GetEnvironmentVariable("LAG_VFR_FORMAT") ?? "mp4",
            TargetHeight = int.TryParse(Environment.GetEnvironmentVariable("LAG_VFR_HEIGHT"), out var th) ? th : 1080,
            MaxFps = int.TryParse(Environment.GetEnvironmentVariable("LAG_VFR_MAXFPS"), out var mf) ? mf : 120,
            Preset = int.TryParse(Environment.GetEnvironmentVariable("LAG_VFR_PRESET"), out var pr) ? pr : 4,
            CaptureCursor = true,
            ForcedWindow = forced,
            Mode = desktop ? CaptureMode.Display : CaptureMode.AutoGame,
            DuplicationOnly = Environment.GetEnvironmentVariable("LAG_VFR_DUPONLY") == "1",
            SeparateAudioTracks = Environment.GetEnvironmentVariable("LAG_VFR_SEPAUDIO") == "1",
            AudioEnabled = Environment.GetEnvironmentVariable("LAG_VFR_NOAUDIO") != "1",
            // Per-app audio test: LAG_VFR_APPS="firefox,discord" → capture only those apps (gain 1.0).
            AppsAudioMode = !string.IsNullOrEmpty(Environment.GetEnvironmentVariable("LAG_VFR_APPS")),
            AudioApps = (Environment.GetEnvironmentVariable("LAG_VFR_APPS") ?? "")
                .Split(',', StringSplitOptions.RemoveEmptyEntries | StringSplitOptions.TrimEntries)
                .Select(e => (e, 1.0f)).ToList(),
            // Overlay: LAG_VFR_WEBCAM=1 draws the default camera PiP (bottom-right by default);
            // LAG_VFR_KEYS=1 draws the keyboard+mouse block (bottom-left; fed synthetically below).
            WebcamOverlay = Environment.GetEnvironmentVariable("LAG_VFR_WEBCAM") == "1",
            KeysOverlay = Environment.GetEnvironmentVariable("LAG_VFR_KEYS") == "1",
            StatsOverlay = Environment.GetEnvironmentVariable("LAG_VFR_STATS") == "1",
            StatsDetail = 2,   // headless test shows the full panel
        });

        int seconds = int.TryParse(Environment.GetEnvironmentVariable("LAG_VFR_SECONDS"), out var s) ? s : 15;
        bool fakeKeys = Environment.GetEnvironmentVariable("LAG_VFR_KEYS") == "1";
        Console.WriteLine($"[VfrTest] Buffering for {seconds}s (start/keep a game in the foreground)...");
        for (int i = 0; i < seconds; i++)
        {
            // Feed the keystroke tracker directly (in-process state only) so the badges render
            // in a headless run where no global hook is active.
            if (fakeKeys)
            {
                KeystrokeTracker.Instance.OnKey(SharpHook.Native.KeyCode.VcW, down: true);
                KeystrokeTracker.Instance.OnKey(SharpHook.Native.KeyCode.VcLeftShift, down: i % 2 == 0);
                KeystrokeTracker.Instance.OnMouseButton(1, down: i % 3 != 0);
            }
            Thread.Sleep(1000);
            if (i % 5 == 4)
            {
                var (cms, ems) = engine.AvgStageMs;
                Console.WriteLine($"[VfrTest] ...{i + 1}s, captured={engine.FramesCaptured}, encoded={engine.FramesEncoded}, buffered={engine.BufferedSeconds:F1}s, convert={cms:F1}ms encode={ems:F1}ms");
            }
        }

        Console.WriteLine("[VfrTest] Saving replay...");
        string? saved = engine.SaveReplay();
        Console.WriteLine(saved != null ? $"[VfrTest] DONE → {saved}" : "[VfrTest] SaveReplay returned null (no game locked?).");
        engine.Stop();
    }
}
