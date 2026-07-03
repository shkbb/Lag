using System;
using System.Runtime.InteropServices;
using System.Threading;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Once-a-second system metrics for the recording stats overlay (Steam-style): recording FPS
/// (composed frames — for a locked game that IS the game's present rate), total CPU load
/// (GetSystemTimes deltas), total GPU 3D-engine load (the same PDH counter family Task Manager
/// uses, summed over all processes) and RAM usage (GlobalMemoryStatusEx). Sampling runs on a
/// timer thread; the capture thread reads the latest immutable snapshot lock-free.
/// </summary>
public sealed class SystemStatsSampler : IDisposable
{
    public sealed record Snapshot(int Fps, int CpuPercent, int GpuPercent, double RamUsedGb, double RamTotalGb);

    private volatile Snapshot _current = new(0, 0, 0, 0, 0);
    public Snapshot Current => _current;

    private readonly Timer _timer;
    private long _frames;
    private long _prevIdle, _prevKernel, _prevUser;
    private IntPtr _query, _counter;

    public SystemStatsSampler()
    {
        try
        {
            if (PdhOpenQueryW(null, IntPtr.Zero, out _query) == 0)
            {
                if (PdhAddEnglishCounterW(_query, @"\GPU Engine(*engtype_3D)\Utilization Percentage",
                                          IntPtr.Zero, out _counter) != 0)
                    _counter = IntPtr.Zero;
                else
                    PdhCollectQueryData(_query);   // prime — rate counters need a baseline
            }
        }
        catch { _query = IntPtr.Zero; _counter = IntPtr.Zero; }

        GetSystemTimes(out _prevIdle, out _prevKernel, out _prevUser);
        _timer = new Timer(_ => Sample(), null, 1000, 1000);
    }

    /// <summary>The compositor calls this once per composed frame — the per-second count is the FPS.</summary>
    public void CountFrame() => Interlocked.Increment(ref _frames);

    private void Sample()
    {
        try
        {
            int fps = (int)Interlocked.Exchange(ref _frames, 0);

            // CPU: 1 − idle share of the elapsed kernel+user time.
            int cpu = 0;
            if (GetSystemTimes(out long idle, out long kernel, out long user))
            {
                long dIdle = idle - _prevIdle, dKernel = kernel - _prevKernel, dUser = user - _prevUser;
                _prevIdle = idle; _prevKernel = kernel; _prevUser = user;
                long total = dKernel + dUser;   // kernel time INCLUDES idle
                if (total > 0) cpu = (int)Math.Clamp(100.0 * (total - dIdle) / total, 0, 100);
            }

            int gpu = SampleGpu();

            double usedGb = 0, totalGb = 0;
            var mem = new MEMORYSTATUSEX { dwLength = (uint)Marshal.SizeOf<MEMORYSTATUSEX>() };
            if (GlobalMemoryStatusEx(ref mem))
            {
                totalGb = mem.ullTotalPhys / 1073741824.0;
                usedGb = (mem.ullTotalPhys - mem.ullAvailPhys) / 1073741824.0;
            }

            _current = new Snapshot(fps, cpu, gpu, usedGb, totalGb);
        }
        catch { /* sampling must never take the app down */ }
    }

    /// <summary>Sum of every process's 3D-engine utilization (Task Manager's "GPU" column).</summary>
    private int SampleGpu()
    {
        if (_counter == IntPtr.Zero) return 0;
        try
        {
            if (PdhCollectQueryData(_query) != 0) return 0;
            uint size = 0;
            uint status = PdhGetFormattedCounterArrayW(_counter, PDH_FMT_DOUBLE, ref size, out _, IntPtr.Zero);
            if (status != PDH_MORE_DATA || size == 0) return 0;

            IntPtr buf = Marshal.AllocHGlobal((int)size);
            try
            {
                if (PdhGetFormattedCounterArrayW(_counter, PDH_FMT_DOUBLE, ref size, out uint count, buf) != 0)
                    return 0;
                double sum = 0;
                int stride = Marshal.SizeOf<PdhFmtCounterValueItemDouble>();
                for (uint i = 0; i < count; i++)
                {
                    var item = Marshal.PtrToStructure<PdhFmtCounterValueItemDouble>(buf + (int)(i * stride));
                    if (item.CStatus == 0) sum += item.Value;
                }
                return (int)Math.Clamp(sum, 0, 100);
            }
            finally { Marshal.FreeHGlobal(buf); }
        }
        catch { return 0; }
    }

    public void Dispose()
    {
        _timer.Dispose();
        if (_query != IntPtr.Zero) { try { PdhCloseQuery(_query); } catch { } _query = IntPtr.Zero; }
    }

    // ── native ──

    private const uint PDH_FMT_DOUBLE = 0x00000200;
    private const uint PDH_MORE_DATA = 0x800007D2;

    [StructLayout(LayoutKind.Sequential)]
    private struct PdhFmtCounterValueItemDouble
    {
        public IntPtr szName;
        public uint CStatus;
        public double Value;
    }

    [DllImport("pdh.dll", CharSet = CharSet.Unicode)]
    private static extern uint PdhOpenQueryW(string? dataSource, IntPtr userData, out IntPtr query);
    [DllImport("pdh.dll", CharSet = CharSet.Unicode)]
    private static extern uint PdhAddEnglishCounterW(IntPtr query, string fullPath, IntPtr userData, out IntPtr counter);
    [DllImport("pdh.dll")]
    private static extern uint PdhCollectQueryData(IntPtr query);
    [DllImport("pdh.dll", CharSet = CharSet.Unicode)]
    private static extern uint PdhGetFormattedCounterArrayW(IntPtr counter, uint format, ref uint bufferSize, out uint itemCount, IntPtr itemBuffer);
    [DllImport("pdh.dll")]
    private static extern uint PdhCloseQuery(IntPtr query);

    [DllImport("kernel32.dll")]
    private static extern bool GetSystemTimes(out long idleTime, out long kernelTime, out long userTime);

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

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode)]
    private static extern bool GlobalMemoryStatusEx(ref MEMORYSTATUSEX lpBuffer);
}
