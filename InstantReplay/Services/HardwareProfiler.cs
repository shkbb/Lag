using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using System.Runtime.InteropServices;
using System.Text;
using Lag.Services.VfrCapture;
using Microsoft.Win32;

namespace Lag.Services;

/// <summary>One GPU adapter as seen by DXGI, with the numbers that decide a recording tier.</summary>
public sealed record GpuProfile(int Index, string Name, int VendorId, long DedicatedVramBytes, long SharedMemoryBytes)
{
    public string Vendor => VendorId switch
    {
        0x10DE => "NVIDIA",
        0x1002 => "AMD",
        0x8086 => "Intel",
        0x1414 => "Microsoft (software)",
        _ => $"0x{VendorId:X4}",
    };

    public double VramGiB => DedicatedVramBytes / 1024.0 / 1024.0 / 1024.0;

    /// <summary>Integrated GPU heuristic: an iGPU has little/no dedicated VRAM and leans on shared
    /// system memory. Arc and other discrete cards report real dedicated VRAM, so they read as false.</summary>
    public bool IsIntegrated => DedicatedVramBytes < 1L * 1024 * 1024 * 1024;
}

/// <summary>
/// A full static snapshot of the machine's recording-relevant hardware: the GPU(s) and their VRAM,
/// the video encoders that actually opened on this box, the CPU, total RAM, free disk on the library
/// drive, and the primary monitor. Built WITHOUT a benchmark — DXGI for the GPU/VRAM, the engine's
/// own encoder probe for real codec support, WMI-free Win32/registry for CPU+RAM, DriveInfo for disk.
///
/// This is the input the hardware-tailored preset resolver maps an intent (Performance / Balanced /
/// Quality) onto. It only READS the system; it changes nothing.
/// </summary>
public sealed record MachineProfile(
    IReadOnlyList<GpuProfile> Gpus,
    GpuProfile? PrimaryGpu,
    IReadOnlyList<string> HardwareEncoders,
    bool HasAv1, bool HasHevc, bool HasH264,
    string CpuName, int CpuLogicalCores,
    double RamGiB,
    string LibraryDrive, double DiskFreeGiB, double DiskTotalGiB,
    int MonitorWidth, int MonitorHeight, int MonitorHz)
{
    /// <summary>Human-readable multi-line report for the session log (one place to eyeball detection).</summary>
    public string Describe()
    {
        var sb = new StringBuilder();
        sb.AppendLine("===== Hardware profile =====");
        foreach (var g in Gpus)
            sb.AppendLine($"  GPU{g.Index}: {g.Name} [{g.Vendor}] VRAM {g.VramGiB:0.0} GiB{(g.IsIntegrated ? " (integrated)" : "")}"
                          + (PrimaryGpu == g ? "  <- primary" : ""));
        sb.AppendLine($"  HW encoders: {(HardwareEncoders.Count > 0 ? string.Join(", ", HardwareEncoders) : "none (CPU x264 only)")}");
        sb.AppendLine($"  Codecs: AV1={(HasAv1 ? "yes" : "no")} HEVC={(HasHevc ? "yes" : "no")} H264={(HasH264 ? "yes" : "no")}");
        sb.AppendLine($"  CPU: {CpuName} ({CpuLogicalCores} logical)");
        sb.AppendLine($"  RAM: {RamGiB:0.0} GiB");
        sb.AppendLine($"  Disk ({LibraryDrive}): {DiskFreeGiB:0.0} GiB free / {DiskTotalGiB:0.0} GiB");
        sb.AppendLine($"  Monitor: {MonitorWidth}x{MonitorHeight} @ {MonitorHz}Hz");
        sb.Append("============================");
        return sb.ToString();
    }
}

/// <summary>Builds a <see cref="MachineProfile"/> from the live system. Read-only; never throws
/// (a failed probe just leaves that field at a safe default).</summary>
public static class HardwareProfiler
{
    public static MachineProfile Capture(string? libraryPath)
    {
        var gpus = ProbeGpus();
        // Primary = the adapter with the most dedicated VRAM (the dGPU on a hybrid laptop; the only
        // one elsewhere). That's the GPU the engine's capture/encode will realistically lean on.
        var primary = gpus.OrderByDescending(g => g.DedicatedVramBytes).FirstOrDefault();

        var (hwEncoders, hasAv1, hasHevc, hasH264) = ProbeEncoders();
        var (cpuName, cpuCores) = ProbeCpu();
        double ramGiB = ProbeRamGiB();
        var (drive, freeGiB, totalGiB) = ProbeDisk(libraryPath);
        var (mw, mh, mhz) = ProbeMonitor();

        return new MachineProfile(gpus, primary, hwEncoders, hasAv1, hasHevc, hasH264,
                                  cpuName, cpuCores, ramGiB, drive, freeGiB, totalGiB, mw, mh, mhz);
    }

    /// <summary>Captures the profile and writes the report to the session log.</summary>
    public static void LogReport(string? libraryPath)
    {
        try { Console.WriteLine(Capture(libraryPath).Describe()); }
        catch (Exception ex) { Console.WriteLine($"[HardwareProfiler] report failed: {ex.Message}"); }
    }

    private static IReadOnlyList<GpuProfile> ProbeGpus()
    {
        var list = new List<GpuProfile>();
        try
        {
            using var factory = new SharpDX.DXGI.Factory1();
            int count = factory.GetAdapterCount1();
            for (int i = 0; i < count; i++)
            {
                using var adapter = factory.GetAdapter1(i);
                var d = adapter.Description1;
                if (d.VendorId == 0x1414) continue; // Microsoft Basic Render Driver — not a real GPU
                list.Add(new GpuProfile(i, d.Description.TrimEnd('\0'), d.VendorId,
                                        (long)d.DedicatedVideoMemory, (long)d.SharedSystemMemory));
            }
        }
        catch (Exception ex) { Console.WriteLine($"[HardwareProfiler] GPU probe failed: {ex.Message}"); }
        return list;
    }

    private static (IReadOnlyList<string> hw, bool av1, bool hevc, bool h264) ProbeEncoders()
    {
        try
        {
            var avail = EncoderSelector.Available;   // cached after the first probe; FFmpeg is bound at startup
            var hw = avail.Where(e => e.IsHardware).Select(e => e.FriendlyName).ToList();
            bool av1 = avail.Any(e => e.Tier == CodecTier.Av1);
            bool hevc = avail.Any(e => e.Tier == CodecTier.Hevc);
            bool h264 = avail.Any(e => e.Tier == CodecTier.H264);
            return (hw, av1, hevc, h264);
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[HardwareProfiler] encoder probe failed: {ex.Message}");
            return (Array.Empty<string>(), false, false, false);
        }
    }

    private static (string name, int cores) ProbeCpu()
    {
        string name = "Unknown CPU";
        try
        {
            using var key = Registry.LocalMachine.OpenSubKey(@"HARDWARE\DESCRIPTION\System\CentralProcessor\0");
            if (key?.GetValue("ProcessorNameString") is string s && s.Length > 0) name = s.Trim();
        }
        catch (Exception ex) { Console.WriteLine($"[HardwareProfiler] CPU probe failed: {ex.Message}"); }
        return (name, Environment.ProcessorCount);
    }

    private static double ProbeRamGiB()
    {
        try
        {
            var m = new MEMORYSTATUSEX { dwLength = (uint)Marshal.SizeOf<MEMORYSTATUSEX>() };
            if (GlobalMemoryStatusEx(ref m)) return m.ullTotalPhys / 1024.0 / 1024.0 / 1024.0;
        }
        catch (Exception ex) { Console.WriteLine($"[HardwareProfiler] RAM probe failed: {ex.Message}"); }
        return 0;
    }

    private static (string drive, double freeGiB, double totalGiB) ProbeDisk(string? libraryPath)
    {
        try
        {
            string path = string.IsNullOrWhiteSpace(libraryPath)
                ? Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.MyVideos), "Lag")
                : libraryPath!;
            string root = Path.GetPathRoot(Path.GetFullPath(path)) ?? "C:\\";
            var di = new DriveInfo(root);
            return (di.Name, di.AvailableFreeSpace / 1024.0 / 1024.0 / 1024.0,
                             di.TotalSize / 1024.0 / 1024.0 / 1024.0);
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[HardwareProfiler] disk probe failed: {ex.Message}");
            return ("?", 0, 0);
        }
    }

    private static (int w, int h, int hz) ProbeMonitor()
    {
        try
        {
            var monitors = new HardwareDetector().GetAvailableMonitors();
            var m = monitors.FirstOrDefault(x => x.IsPrimary) ?? monitors.FirstOrDefault();
            if (m != null) return ((int)m.Width, (int)m.Height, (int)m.RefreshRate);
        }
        catch (Exception ex) { Console.WriteLine($"[HardwareProfiler] monitor probe failed: {ex.Message}"); }
        return (0, 0, 0);
    }

    [StructLayout(LayoutKind.Sequential)]
    private struct MEMORYSTATUSEX
    {
        public uint dwLength;
        public uint dwMemoryLoad;
        public ulong ullTotalPhys;
        public ulong ullAvailPhys;
        public ulong ullTotalPageFile;
        public ulong ullAvailPageFile;
        public ulong ullTotalVirtual;
        public ulong ullAvailVirtual;
        public ulong ullAvailExtendedVirtual;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    [return: MarshalAs(UnmanagedType.Bool)]
    private static extern bool GlobalMemoryStatusEx(ref MEMORYSTATUSEX lpBuffer);
}
