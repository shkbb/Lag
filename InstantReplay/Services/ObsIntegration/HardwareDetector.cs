using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Collections.Generic;
using System.Text.RegularExpressions;
using NAudio.CoreAudioApi;

namespace Lag.Services.ObsIntegration;

public record MicrophoneInfo(string Id, string Name);

/// <summary>
/// Hardware capability service that queries the system (via libobs) to detect the best
/// available video encoder. Enforces a strict hardware-first priority queue to maximize
/// performance on gaming rigs while seamlessly falling back to software encoding on low-end PCs.
/// 
/// Cross-platform: Uses OS-specific encoder priority lists and monitor enumeration strategies.
///   Windows: NVENC → AMF → QuickSync → x264, Win32 EnumDisplayMonitors, WASAPI mics
///   Linux:   NVENC(ffmpeg) → VAAPI → x264, xrandr monitors, pactl mics
/// </summary>
public sealed partial class HardwareDetector
{
    /// <summary>
    /// Windows encoder IDs in order of preference.
    /// Hardware encoders are prioritized to minimize CPU impact during gameplay.
    /// </summary>
    private static readonly string[] WindowsEncoderPriority =
    [
        "jim_nvenc",      // NVIDIA NVENC (Modern OBS 28+)
        "amd_amf_h264",   // AMD Advanced Media Framework
        "obs_qsv11",      // Intel Quick Sync Video
        "obs_x264"        // Software Fallback (x264)
    ];

    public string DetectOptimalEncoder()
    {
        var encoderList = WindowsEncoderPriority;

        foreach (var encoderId in encoderList)
        {
            if (TryCreateEncoder(encoderId))
            {
                Debug.WriteLine($"[HardwareDetector] Optimal encoder selected: {encoderId}");
                return encoderId;
            }
            Debug.WriteLine($"[HardwareDetector] Encoder {encoderId} is not supported on this hardware.");
        }

        throw new NotSupportedException(
            "No supported video encoders found on this system, including the software fallback.");
    }

    /// <summary>
    /// Attempts to silently create the encoder via libobs. If it succeeds, the hardware 
    /// supports it. It is immediately released.
    /// </summary>
    private bool TryCreateEncoder(string encoderId)
    {
        try
        {
            using var settings = ObsInterop.obs_data_create();
            
            if (encoderId == "obs_x264")
            {
                ObsInterop.obs_data_set_string(settings, "preset", "veryfast");
                ObsInterop.obs_data_set_int(settings, "crf", 23);
            }
            else
            {
                ObsInterop.obs_data_set_string(settings, "rate_control", "CQP");
                ObsInterop.obs_data_set_int(settings, "cqp", 20);
            }

            using var encoder = ObsInterop.obs_video_encoder_create(encoderId, "test_encoder", settings, IntPtr.Zero);
            
            if (encoder.IsInvalid)
                return false;
                
            // Dummy encoder detection: a dummy yields null properties and cannot produce frames
            IntPtr props = ObsInterop.obs_encoder_properties(encoder);
            if (props == IntPtr.Zero)
            {
                Debug.WriteLine($"[HardwareDetector] Encoder {encoderId} is a dummy object. Rejecting.");
                return false;
            }
            
            ObsInterop.obs_properties_destroy(props);
            return true;
        }
        catch (DllNotFoundException)
        {
            string libName = "obs.dll";
            throw new InvalidOperationException(
                $"{libName} is missing or not resolvable. Ensure libobs is installed and on the library path.");
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[HardwareDetector] Probe exception for {encoderId}: {ex.Message}");
            return false;
        }
    }

    // ────────────────────── GPU Adapter Detection ────────────────────── //

    /// <summary>A physical GPU adapter as seen by DXGI (Index matches obs_video_info.adapter).</summary>
    public record GpuInfo(int Index, string Name)
    {
        public override string ToString() => Name;
    }

    /// <summary>
    /// Enumerates hardware GPU adapters via DXGI (same ordering libobs uses for the `adapter`
    /// field). Software adapters (Microsoft Basic Render Driver) are skipped.
    /// </summary>
    public IReadOnlyList<GpuInfo> GetGpuAdapters()
    {
        var gpus = new List<GpuInfo>();
        try
        {
            using var factory = new SharpDX.DXGI.Factory1();
            int count = factory.GetAdapterCount1();
            for (int i = 0; i < count; i++)
            {
                using var adapter = factory.GetAdapter1(i);
                var desc = adapter.Description1;

                // 0x1414 = Microsoft (Basic Render Driver) — not a real GPU.
                if (desc.VendorId == 0x1414) continue;

                gpus.Add(new GpuInfo(i, desc.Description.TrimEnd('\0')));
            }
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[HardwareDetector] DXGI adapter enumeration failed: {ex.Message}");
        }

        if (gpus.Count == 0)
            gpus.Add(new GpuInfo(0, "Default GPU"));

        return gpus;
    }

    // ────────────────────── Monitor Detection ────────────────────── //

    /// <summary>
    /// Represents an available display monitor and its unscaled native render bounds.
    /// </summary>
    public record MonitorInfo(int Index, string DeviceName, uint Width, uint Height, bool IsPrimary)
    {
        public override string ToString() => $"{DeviceName} ({Width}x{Height}){(IsPrimary ? " [Primary]" : "")}";
    }

    public IReadOnlyList<MonitorInfo> GetAvailableMonitors()
    {
        var monitors = new List<MonitorInfo>();
        EnumerateWindowsMonitors(monitors);

        // Last-resort fallback: prevents obs_reset_video crash from 0×0 resolution
        if (monitors.Count == 0)
        {
            Debug.WriteLine("[HardwareDetector] No monitors detected via native APIs. Using 1920x1080 default.");
            monitors.Add(new MonitorInfo(0, "Default Display", 1920, 1080, true));
        }

        return monitors;
    }

    // ────────────────────── Windows Monitor Enumeration ────────────────────── //

    private void EnumerateWindowsMonitors(List<MonitorInfo> monitors)
    {
        EnumDisplayMonitors(IntPtr.Zero, IntPtr.Zero,
            (IntPtr hMonitor, IntPtr hdcMonitor, ref RECT lprcMonitor, IntPtr dwData) =>
            {
                var info = new MONITORINFOEX();
                info.cbSize = Marshal.SizeOf(typeof(MONITORINFOEX));
                    
                if (GetMonitorInfo(hMonitor, ref info))
                {
                    uint width = (uint)(info.rcMonitor.Right - info.rcMonitor.Left);
                    uint height = (uint)(info.rcMonitor.Bottom - info.rcMonitor.Top);
                    bool isPrimary = (info.dwFlags & MONITORINFOF_PRIMARY) != 0;
                    string name = new string(info.szDevice).TrimEnd('\0');

                    monitors.Add(new MonitorInfo(monitors.Count, name, width, height, isPrimary));
                }
                return true;
            }, IntPtr.Zero);
    }

    private delegate bool MonitorEnumDelegate(IntPtr hMonitor, IntPtr hdcMonitor, ref RECT lprcMonitor, IntPtr dwData);

    [DllImport("user32.dll")]
    private static extern bool EnumDisplayMonitors(IntPtr hdc, IntPtr lprcClip, MonitorEnumDelegate lpfnEnum, IntPtr dwData);

    [DllImport("user32.dll", CharSet = CharSet.Auto)]
    private static extern bool GetMonitorInfo(IntPtr hMonitor, ref MONITORINFOEX lpmi);

    [StructLayout(LayoutKind.Sequential)]
    private struct RECT
    {
        public int Left;
        public int Top;
        public int Right;
        public int Bottom;
    }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Auto)]
    private struct MONITORINFOEX
    {
        public int cbSize;
        public RECT rcMonitor;
        public RECT rcWork;
        public int dwFlags;
        [MarshalAs(UnmanagedType.ByValArray, SizeConst = 32)]
        public char[] szDevice;
    }
    private const int MONITORINFOF_PRIMARY = 1;

    // ────────────────────── Microphone Detection ────────────────────── //

    public IReadOnlyList<MicrophoneInfo> GetMicrophones()
    {
        var mics = new List<MicrophoneInfo>();
        EnumerateWindowsMicrophones(mics);

        if (mics.Count == 0)
        {
            mics.Add(new MicrophoneInfo("default", "Default Microphone"));
        }

        return mics;
    }

    private void EnumerateWindowsMicrophones(List<MicrophoneInfo> mics)
    {
        try
        {
            var enumerator = new MMDeviceEnumerator();
            var devices = enumerator.EnumerateAudioEndPoints(DataFlow.Capture, DeviceState.Active);
            foreach (var endpoint in devices)
            {
                mics.Add(new MicrophoneInfo(endpoint.ID, endpoint.FriendlyName));
            }
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[HardwareDetector] WASAPI mic enumeration failed: {ex.Message}");
        }
    }
}
