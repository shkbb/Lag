using System;
using System.Diagnostics;
using System.Runtime.InteropServices;
using System.Collections.Generic;
using NAudio.CoreAudioApi;

namespace Lag.Services;

public record MicrophoneInfo(string Id, string Name);

/// <summary>A playback (render) endpoint that whole-PC audio can be looped back from.</summary>
public record OutputDeviceInfo(string Id, string Name);

/// <summary>
/// Hardware enumeration service: lists the GPU adapters, displays, microphones and playback
/// endpoints the capture/encode pipeline can target. Video encoder probing lives in the native
/// engine's EncoderSelector (it actually opens each encoder); this type only enumerates devices.
/// </summary>
public sealed partial class HardwareDetector
{
    // ────────────────────── GPU Adapter Detection ────────────────────── //

    /// <summary>A physical GPU adapter as seen by DXGI (Index matches the engine's adapter index).</summary>
    public record GpuInfo(int Index, string Name)
    {
        public override string ToString() => Name;
    }

    /// <summary>
    /// Enumerates hardware GPU adapters via DXGI (the same ordering the engine uses for the
    /// adapter index). Software adapters (Microsoft Basic Render Driver) are skipped.
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
    public record MonitorInfo(int Index, string DeviceName, uint Width, uint Height, bool IsPrimary, uint RefreshRate = 0)
    {
        public override string ToString() =>
            $"{DeviceName} ({Width}x{Height}{(RefreshRate > 0 ? $" @{RefreshRate}Hz" : "")}){(IsPrimary ? " [Primary]" : "")}";
    }

    public IReadOnlyList<MonitorInfo> GetAvailableMonitors()
    {
        var monitors = new List<MonitorInfo>();
        EnumerateWindowsMonitors(monitors);

        // Last-resort fallback: prevents a 0×0 resolution from reaching the capture pipeline.
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
                    uint refresh = QueryRefreshRate(name);

                    monitors.Add(new MonitorInfo(monitors.Count, name, width, height, isPrimary, refresh));
                }
                return true;
            }, IntPtr.Zero);
    }

    /// <summary>Current refresh rate (Hz) of a display by its adapter device name (\\.\DISPLAYn).
    /// 0 when it can't be read.</summary>
    private static uint QueryRefreshRate(string deviceName)
    {
        var dm = new DEVMODE();
        dm.dmSize = (short)Marshal.SizeOf(typeof(DEVMODE));
        if (EnumDisplaySettings(deviceName, ENUM_CURRENT_SETTINGS, ref dm) && dm.dmDisplayFrequency > 1)
            return (uint)dm.dmDisplayFrequency;
        return 0;
    }

    private delegate bool MonitorEnumDelegate(IntPtr hMonitor, IntPtr hdcMonitor, ref RECT lprcMonitor, IntPtr dwData);

    [DllImport("user32.dll")]
    private static extern bool EnumDisplayMonitors(IntPtr hdc, IntPtr lprcClip, MonitorEnumDelegate lpfnEnum, IntPtr dwData);

    private const int ENUM_CURRENT_SETTINGS = -1;

    [DllImport("user32.dll", CharSet = CharSet.Auto)]
    private static extern bool EnumDisplaySettings(string lpszDeviceName, int iModeNum, ref DEVMODE lpDevMode);

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Auto)]
    private struct DEVMODE
    {
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)]
        public string dmDeviceName;
        public short dmSpecVersion;
        public short dmDriverVersion;
        public short dmSize;
        public short dmDriverExtra;
        public int dmFields;
        public int dmPositionX;
        public int dmPositionY;
        public int dmDisplayOrientation;
        public int dmDisplayFixedOutput;
        public short dmColor;
        public short dmDuplex;
        public short dmYResolution;
        public short dmTTOption;
        public short dmCollate;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)]
        public string dmFormName;
        public short dmLogPixels;
        public int dmBitsPerPel;
        public int dmPelsWidth;
        public int dmPelsHeight;
        public int dmDisplayFlags;
        public int dmDisplayFrequency;
        public int dmICMMethod;
        public int dmICMIntent;
        public int dmMediaType;
        public int dmDitherType;
        public int dmReserved1;
        public int dmReserved2;
        public int dmPanningWidth;
        public int dmPanningHeight;
    }

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

    // ────────────────────── Output (render) device detection ────────────────────── //

    /// <summary>Enumerates the active playback endpoints whole-PC audio can be looped back from.</summary>
    public IReadOnlyList<OutputDeviceInfo> GetOutputDevices()
    {
        var outputs = new List<OutputDeviceInfo>();
        try
        {
            var enumerator = new MMDeviceEnumerator();
            var devices = enumerator.EnumerateAudioEndPoints(DataFlow.Render, DeviceState.Active);
            foreach (var endpoint in devices)
                outputs.Add(new OutputDeviceInfo(endpoint.ID, endpoint.FriendlyName));
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[HardwareDetector] WASAPI output enumeration failed: {ex.Message}");
        }
        return outputs;
    }
}
