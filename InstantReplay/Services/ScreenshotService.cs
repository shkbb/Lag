using System;
using System.IO;
using System.Runtime.InteropServices;

namespace Lag.Services;

/// <summary>
/// Captures a PNG screenshot of a display via GDI (BitBlt through CopyFromScreen).
/// The target monitor is matched by its GDI device name ("\\.\DISPLAY1") so the shot
/// follows the same display the recorder is configured to capture.
/// </summary>
public static class ScreenshotService
{
    [StructLayout(LayoutKind.Sequential)]
    private struct RECT { public int Left, Top, Right, Bottom; }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct MONITORINFOEX
    {
        public int cbSize;
        public RECT rcMonitor;
        public RECT rcWork;
        public uint dwFlags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)]
        public string szDevice;
    }

    private delegate bool MonitorEnumProc(IntPtr hMonitor, IntPtr hdc, ref RECT rect, IntPtr data);

    [DllImport("user32.dll")]
    private static extern bool EnumDisplayMonitors(IntPtr hdc, IntPtr clip, MonitorEnumProc proc, IntPtr data);

    [DllImport("user32.dll", CharSet = CharSet.Unicode)]
    private static extern bool GetMonitorInfo(IntPtr hMonitor, ref MONITORINFOEX info);

    /// <summary>
    /// Captures the monitor with the given device name (or the primary one when null/not
    /// found) into "Screenshot_*.png" inside <paramref name="outputDir"/>. Returns the
    /// saved file path, or null on failure.
    /// </summary>
    public static string? Capture(string? monitorDeviceName, string outputDir)
    {
        try
        {
            var bounds = FindMonitorBounds(monitorDeviceName)
                         ?? FindMonitorBounds(null); // primary fallback
            if (bounds is not { } rect) return null;

            int w = rect.Right - rect.Left;
            int h = rect.Bottom - rect.Top;
            if (w <= 0 || h <= 0) return null;

            Directory.CreateDirectory(outputDir);
            string path = Path.Combine(outputDir, $"Screenshot_{DateTime.Now:yyyy-MM-dd_HH-mm-ss}.png");

            using var bmp = new System.Drawing.Bitmap(w, h);
            using (var g = System.Drawing.Graphics.FromImage(bmp))
                g.CopyFromScreen(rect.Left, rect.Top, 0, 0, new System.Drawing.Size(w, h));
            bmp.Save(path, System.Drawing.Imaging.ImageFormat.Png);

            return path;
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"[Screenshot] capture failed: {ex.Message}");
            return null;
        }
    }

    /// <summary>Bounds of the monitor matching the device name; primary when name is null.</summary>
    private static RECT? FindMonitorBounds(string? deviceName)
    {
        RECT? found = null;

        EnumDisplayMonitors(IntPtr.Zero, IntPtr.Zero, (IntPtr hMon, IntPtr hdc, ref RECT rect, IntPtr data) =>
        {
            var info = new MONITORINFOEX { cbSize = Marshal.SizeOf<MONITORINFOEX>() };
            if (GetMonitorInfo(hMon, ref info))
            {
                bool isPrimary = (info.dwFlags & 1) != 0; // MONITORINFOF_PRIMARY
                bool matches = deviceName == null
                    ? isPrimary
                    : string.Equals(info.szDevice, deviceName, StringComparison.OrdinalIgnoreCase);

                if (matches)
                {
                    found = info.rcMonitor;
                    return false; // stop enumeration
                }
            }
            return true;
        }, IntPtr.Zero);

        return found;
    }
}
