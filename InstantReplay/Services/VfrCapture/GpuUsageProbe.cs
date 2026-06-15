using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Reads per-process GPU <b>3D-engine</b> utilization — the strongest hardware signal for "is this a
/// game". A real game pegs the 3D engine (measured CS2 ≈ 64%), while an idle/background app (even a
/// Chromium/Electron UI) sits low (≈ 2–16%) and a video player uses the <i>VideoDecode</i> engine,
/// not 3D. We don't need a curated game database to tell them apart — the GPU does.
///
/// Backed by the same Windows performance counters Task Manager uses (PDH wildcard
/// <c>\GPU Engine(*engtype_3D)\Utilization Percentage</c>). One persistent query is sampled once per
/// detection tick; the gap between ticks IS the sampling interval, so the first sample after start
/// returns nothing (a rate needs two collects) and every tick after is live. Never throws — if PDH
/// is unavailable the map is simply empty and the detector falls back to its other signals.
/// </summary>
internal sealed class GpuUsageProbe : IDisposable
{
    private IntPtr _query;
    private IntPtr _counter;
    private bool _opened;
    private Dictionary<uint, double> _last = new();

    private const uint PDH_FMT_DOUBLE = 0x00000200;
    private const uint PDH_MORE_DATA = 0x800007D2;
    private const uint PDH_CSTATUS_VALID_DATA = 0x00000000;

    [StructLayout(LayoutKind.Sequential)]
    private struct PdhFmtCounterValueItemDouble
    {
        public IntPtr szName;     // wide string: the instance name, e.g. "pid_1234_luid_..._engtype_3D"
        public uint CStatus;
        public double Value;      // 8-aligned after CStatus (4 bytes padding on x64)
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

    private void EnsureOpen()
    {
        if (_opened) return;
        _opened = true; // even on failure, don't retry every tick
        try
        {
            if (PdhOpenQueryW(null, IntPtr.Zero, out _query) != 0) { _query = IntPtr.Zero; return; }
            if (PdhAddEnglishCounterW(_query, @"\GPU Engine(*engtype_3D)\Utilization Percentage", IntPtr.Zero, out _counter) != 0)
            { _counter = IntPtr.Zero; return; }
            PdhCollectQueryData(_query); // prime: a rate counter needs a baseline first
        }
        catch { _query = IntPtr.Zero; _counter = IntPtr.Zero; }
    }

    /// <summary>Collects a fresh sample and refreshes the per-PID 3D-utilization map. Call once per
    /// detection tick. Cheap (one collect + one array read). Never throws.</summary>
    public void Sample()
    {
        EnsureOpen();
        if (_counter == IntPtr.Zero) return;
        try
        {
            if (PdhCollectQueryData(_query) != 0) return;

            uint size = 0, count = 0;
            uint r = PdhGetFormattedCounterArrayW(_counter, PDH_FMT_DOUBLE, ref size, out count, IntPtr.Zero);
            if (r != PDH_MORE_DATA || size == 0) return;

            IntPtr buf = Marshal.AllocHGlobal((int)size);
            try
            {
                r = PdhGetFormattedCounterArrayW(_counter, PDH_FMT_DOUBLE, ref size, out count, buf);
                if (r != 0) return;

                var map = new Dictionary<uint, double>((int)count);
                int stride = Marshal.SizeOf<PdhFmtCounterValueItemDouble>();
                for (int i = 0; i < count; i++)
                {
                    var item = Marshal.PtrToStructure<PdhFmtCounterValueItemDouble>(buf + i * stride);
                    if (item.CStatus != PDH_CSTATUS_VALID_DATA || item.szName == IntPtr.Zero) continue;
                    string name = Marshal.PtrToStringUni(item.szName) ?? "";
                    uint pid = ParsePid(name);
                    if (pid == 0) continue;
                    map.TryGetValue(pid, out double cur);
                    map[pid] = cur + item.Value;   // sum all 3D-engine instances for the process
                }
                _last = map;
            }
            finally { Marshal.FreeHGlobal(buf); }
        }
        catch { /* leave _last as-is */ }
    }

    /// <summary>Summed 3D-engine utilization (%) for a process at the last <see cref="Sample"/>; 0 if unknown.</summary>
    public double Get3D(uint pid) => _last.TryGetValue(pid, out double v) ? v : 0.0;

    /// <summary>Parses the PID out of a GPU-engine instance name like "pid_1234_luid_0x..._engtype_3D".</summary>
    private static uint ParsePid(string instance)
    {
        const string tag = "pid_";
        int i = instance.IndexOf(tag, StringComparison.Ordinal);
        if (i < 0) return 0;
        i += tag.Length;
        uint val = 0; bool any = false;
        while (i < instance.Length && char.IsDigit(instance[i]))
        {
            val = val * 10 + (uint)(instance[i] - '0'); any = true; i++;
        }
        return any ? val : 0;
    }

    public void Dispose()
    {
        try { if (_query != IntPtr.Zero) PdhCloseQuery(_query); } catch { }
        _query = IntPtr.Zero; _counter = IntPtr.Zero;
    }
}
