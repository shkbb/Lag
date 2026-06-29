using System;
using System.Diagnostics;
using System.Runtime.InteropServices;

namespace Lag.Services;

/// <summary>
/// Opts the process out of Windows background throttling so capture keeps running at
/// full speed while a game holds the foreground.
///
/// Without this, Windows 11's EcoQoS ("efficiency mode") and the foreground-boost
/// scheduler starve background apps whenever a fullscreen game is focused: the
/// capture/encode thread drops to a few FPS and even our hotkey message loop can wake
/// up seconds late (replays "not saving until alt-tab").
/// </summary>
public static class PerformanceGuard
{
    private const int ProcessPowerThrottling = 4; // PROCESS_INFORMATION_CLASS

    private const uint PROCESS_POWER_THROTTLING_CURRENT_VERSION = 1;
    private const uint PROCESS_POWER_THROTTLING_EXECUTION_SPEED = 0x1;

    [StructLayout(LayoutKind.Sequential)]
    private struct PROCESS_POWER_THROTTLING_STATE
    {
        public uint Version;
        public uint ControlMask;
        public uint StateMask;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    private static extern bool SetProcessInformation(
        IntPtr hProcess, int processInformationClass,
        ref PROCESS_POWER_THROTTLING_STATE processInformation, int processInformationSize);

    [DllImport("kernel32.dll")]
    private static extern IntPtr GetCurrentProcess();

    // GPU scheduler priority (D3DKMT). This is the documented reason running a capture
    // app as administrator fixes encoding lag when a game pegs the GPU: the recorder's
    // D3D11 work stops being starved by the foreground game.
    // Classes: 0=Idle, 1=BelowNormal, 2=Normal, 3=AboveNormal, 4=High, 5=Realtime(admin).
    [DllImport("gdi32.dll")]
    private static extern int D3DKMTSetProcessSchedulingPriorityClass(IntPtr hProcess, int priorityClass);

    /// <summary>Call once at startup. Safe to call on any Windows version (failures are ignored).</summary>
    public static void Apply()
    {
        // 1) Explicitly opt OUT of power/EcoQoS throttling (ControlMask set, StateMask 0 = "never throttle").
        try
        {
            var state = new PROCESS_POWER_THROTTLING_STATE
            {
                Version = PROCESS_POWER_THROTTLING_CURRENT_VERSION,
                ControlMask = PROCESS_POWER_THROTTLING_EXECUTION_SPEED,
                StateMask = 0,
            };
            SetProcessInformation(GetCurrentProcess(), ProcessPowerThrottling,
                ref state, Marshal.SizeOf<PROCESS_POWER_THROTTLING_STATE>());
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[PerformanceGuard] power throttling opt-out failed: {ex.Message}");
        }

        // 2) Raise the GPU scheduling priority so capture/encode keeps getting GPU time
        //    while a game runs at 100% load. CPU priority intentionally stays Normal —
        //    an elevated CPU class was measurably stealing frames from CPU-bound games.
        try
        {
            foreach (int cls in new[] { 5 /* Realtime, needs admin */, 4 /* High */, 3 /* AboveNormal */ })
            {
                if (D3DKMTSetProcessSchedulingPriorityClass(GetCurrentProcess(), cls) == 0)
                {
                    Console.WriteLine($"[PerformanceGuard] GPU scheduling priority class set to {cls}.");
                    return;
                }
            }
            Console.WriteLine("[PerformanceGuard] GPU scheduling priority unchanged (all classes denied — not elevated?).");
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[PerformanceGuard] GPU priority failed: {ex.Message}");
        }
    }
}
