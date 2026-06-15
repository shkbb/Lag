using System;
using System.Runtime.InteropServices;
using System.Threading;
using NAudio.Wave;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Captures the audio rendered by ONE process (and its child process tree) via WASAPI
/// <b>process loopback</b> — the API Medal uses to record per-application sound ("Обрані програми").
/// Plain <c>WasapiLoopbackCapture</c> can only grab the whole system endpoint; isolating a single
/// app needs <c>ActivateAudioInterfaceAsync</c> with <c>AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK</c>
/// (Windows 10 build 19041+). NAudio has no binding for it, so this is hand-rolled COM interop.
///
/// Output is fixed 48 kHz / 2-ch / 32-bit float (we ask the client for exactly that — process
/// loopback has no mix format to query), delivered via <see cref="DataAvailable"/> on a capture
/// thread, matching the shape <see cref="WasapiAudioSource"/> already feeds the encoder.
/// </summary>
public sealed class ProcessLoopbackCapture : IDisposable
{
    public WaveFormat WaveFormat { get; } = WaveFormat.CreateIeeeFloatWaveFormat(48000, 2);

    /// <summary>(buffer, valid byte count) of 48k/2ch/float PCM rendered by the target process.</summary>
    public event Action<byte[], int>? DataAvailable;

    private readonly uint _pid;
    private IAudioClient? _client;
    private IAudioCaptureClient? _capture;
    private IntPtr _event;
    private Thread? _thread;
    private volatile bool _running;
    private bool _disposed;

    public ProcessLoopbackCapture(uint processId) => _pid = processId;

    /// <summary>Headless validation (LAG_PROCTEST=&lt;pid&gt;): captures the process for 5 s and reports
    /// callbacks / bytes / peak amplitude — proves the interop actually delivers that app's audio.</summary>
    public static void SelfTest(int pid)
    {
        Console.WriteLine($"[ProcLoopbackTest] capturing pid {pid} for 5s (play sound in that app)...");
        long totalBytes = 0; float peak = 0; int callbacks = 0;
        using var cap = new ProcessLoopbackCapture((uint)pid);
        cap.DataAvailable += (buf, n) =>
        {
            callbacks++; totalBytes += n;
            var span = MemoryMarshal.Cast<byte, float>(buf.AsSpan(0, n));
            foreach (var s in span) { float a = Math.Abs(s); if (a > peak) peak = a; }
        };
        try { cap.Start(); }
        catch (Exception ex) { Console.WriteLine($"[ProcLoopbackTest] Start FAILED: {ex.Message}"); return; }
        Thread.Sleep(5000);
        Console.WriteLine($"[ProcLoopbackTest] done: {callbacks} callbacks, {totalBytes} bytes " +
                          $"(~{totalBytes / 8.0 / 48000:F2}s), peak amplitude {peak:F4} " +
                          $"({(peak > 0.0001f ? "REAL AUDIO ✓" : "silent/none")}).");
    }

    private readonly ManualResetEvent _ready = new(false);
    private Exception? _startError;

    /// <summary>Launches the capture on a dedicated MTA thread (ActivateAudioInterfaceAsync requires
    /// an MTA caller, and every COM object must be created AND used on that same apartment — otherwise
    /// the cross-thread QueryInterface fails with E_NOINTERFACE). Blocks until setup succeeds or throws.</summary>
    public void Start()
    {
        _thread = new Thread(Run) { IsBackground = true, Name = $"ProcLoopback{_pid}" };
        _thread.SetApartmentState(ApartmentState.MTA);
        _thread.Start();
        _ready.WaitOne(6000);
        if (_startError != null) throw _startError;
    }

    private void Run()
    {
        try { Setup(); _running = true; }
        catch (Exception ex) { _startError = ex; _ready.Set(); return; }
        _ready.Set();
        CaptureLoop();
    }

    private void Setup()
    {
        _client = ActivateProcessLoopbackClient(_pid);

        // Format we force the client to deliver (process loopback has no GetMixFormat).
        var wf = new WAVEFORMATEX
        {
            wFormatTag = 3, // WAVE_FORMAT_IEEE_FLOAT
            nChannels = 2,
            nSamplesPerSec = 48000,
            wBitsPerSample = 32,
            nBlockAlign = 8,            // channels * bytes/sample
            nAvgBytesPerSec = 48000 * 8,
            cbSize = 0,
        };
        IntPtr pFmt = Marshal.AllocHGlobal(Marshal.SizeOf<WAVEFORMATEX>());
        try
        {
            Marshal.StructureToPtr(wf, pFmt, false);
            const uint flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK;
            // 200 ms shared buffer; periodicity 0 = default.
            int hr = _client!.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 2_000_000, 0, pFmt, IntPtr.Zero);
            if (hr != 0) throw new COMException("IAudioClient.Initialize (process loopback) failed", hr);
        }
        finally { Marshal.FreeHGlobal(pFmt); }

        _event = CreateEventW(IntPtr.Zero, false, false, null);
        if (_event == IntPtr.Zero) throw new InvalidOperationException("CreateEvent failed for process loopback.");
        Check(_client.SetEventHandle(_event), "SetEventHandle");

        var iid = IID_IAudioCaptureClient;
        Check(_client.GetService(ref iid, out object svc), "GetService(IAudioCaptureClient)");
        _capture = (IAudioCaptureClient)svc;

        Check(_client.Start(), "IAudioClient.Start");
    }

    private void CaptureLoop()
    {
        const int frameBytes = 8; // 2ch * 4 bytes float
        while (_running)
        {
            // Wake on the client's event (or time out so teardown is responsive).
            WaitForSingleObject(_event, 100);
            if (!_running || _capture == null) break;

            while (_capture.GetNextPacketSize(out uint packetFrames) == 0 && packetFrames > 0)
            {
                int hr = _capture.GetBuffer(out IntPtr data, out uint frames, out uint bufFlags, out _, out _);
                if (hr != 0 || frames == 0) { if (hr == 0) _capture.ReleaseBuffer(frames); break; }

                int bytes = (int)frames * frameBytes;
                var managed = new byte[bytes];
                if ((bufFlags & AUDCLNT_BUFFERFLAGS_SILENT) != 0)
                    Array.Clear(managed, 0, bytes);     // silent packet → deliver zeros (keeps timing)
                else
                    Marshal.Copy(data, managed, 0, bytes);

                _capture.ReleaseBuffer(frames);
                try { DataAvailable?.Invoke(managed, bytes); } catch { /* never kill the capture loop */ }
            }
        }
    }

    /// <summary>Runs ActivateAudioInterfaceAsync for the process-loopback virtual device and blocks
    /// until the IAudioClient is ready.</summary>
    private static IAudioClient ActivateProcessLoopbackClient(uint pid)
    {
        var activationParams = new AUDIOCLIENT_ACTIVATION_PARAMS
        {
            ActivationType = 1, // PROCESS_LOOPBACK
            TargetProcessId = pid,
            ProcessLoopbackMode = 0, // INCLUDE_TARGET_PROCESS_TREE
        };
        IntPtr pParams = Marshal.AllocHGlobal(Marshal.SizeOf<AUDIOCLIENT_ACTIVATION_PARAMS>());
        IntPtr pProp = Marshal.AllocHGlobal(Marshal.SizeOf<PROPVARIANT>());
        try
        {
            Marshal.StructureToPtr(activationParams, pParams, false);
            var prop = new PROPVARIANT { vt = VT_BLOB, blobSize = (uint)Marshal.SizeOf<AUDIOCLIENT_ACTIVATION_PARAMS>(), blobData = pParams };
            Marshal.StructureToPtr(prop, pProp, false);

            var handler = new ActivationHandler();
            var iid = IID_IAudioClient;
            ActivateAudioInterfaceAsync(VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK, ref iid, pProp, handler, out IActivateAudioInterfaceAsyncOperation op);

            if (!handler.Completed.WaitOne(5000))
                throw new TimeoutException("ActivateAudioInterfaceAsync did not complete (process loopback).");

            op.GetActivateResult(out int activateHr, out object iface);
            if (activateHr != 0) throw new COMException("Process-loopback activation failed", activateHr);
            return (IAudioClient)iface;
        }
        finally { Marshal.FreeHGlobal(pParams); Marshal.FreeHGlobal(pProp); }
    }

    private static void Check(int hr, string what)
    {
        if (hr != 0) throw new COMException($"process loopback: {what} failed", hr);
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        _running = false;
        try { if (_event != IntPtr.Zero) SetEvent(_event); } catch { }
        try { _thread?.Join(500); } catch { }
        try { _client?.Stop(); } catch { }
        if (_capture != null) { Marshal.ReleaseComObject(_capture); _capture = null; }
        if (_client != null) { Marshal.ReleaseComObject(_client); _client = null; }
        if (_event != IntPtr.Zero) { CloseHandle(_event); _event = IntPtr.Zero; }
    }

    // ───────────────────────── COM completion handler (managed → native CCW) ─────────────────────────

    [ComVisible(true)]
    [Guid("41D949AB-9862-444A-80F6-C261334DA5EB")]
    [InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IActivateAudioInterfaceCompletionHandler
    {
        [PreserveSig] int OnActivateCompleted(IntPtr activateOperation);
    }

    [ComVisible(true)]
    private sealed class ActivationHandler : IActivateAudioInterfaceCompletionHandler
    {
        public readonly ManualResetEvent Completed = new(false);
        public int OnActivateCompleted(IntPtr activateOperation) { Completed.Set(); return 0; }
    }

    // ───────────────────────── native interop ─────────────────────────

    private const string VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK = "VAD\\Process_Loopback";
    private const uint AUDCLNT_SHAREMODE_SHARED = 0;
    private const uint AUDCLNT_STREAMFLAGS_LOOPBACK = 0x00020000;
    private const uint AUDCLNT_STREAMFLAGS_EVENTCALLBACK = 0x00040000;
    private const uint AUDCLNT_BUFFERFLAGS_SILENT = 0x2;
    private const ushort VT_BLOB = 0x0041;

    private static Guid IID_IAudioClient = new("1CB9AD4C-DBFA-4c32-B178-C2F568A703B2");
    private static Guid IID_IAudioCaptureClient = new("C8ADBD64-E71E-48a0-A4DE-185C395CD317");

    [DllImport("Mmdevapi.dll", ExactSpelling = true, PreserveSig = false)]
    private static extern void ActivateAudioInterfaceAsync(
        [MarshalAs(UnmanagedType.LPWStr)] string deviceInterfacePath,
        ref Guid riid, IntPtr activationParams,
        IActivateAudioInterfaceCompletionHandler completionHandler,
        out IActivateAudioInterfaceAsyncOperation asyncOp);

    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    private static extern IntPtr CreateEventW(IntPtr attrs, bool manualReset, bool initialState, string? name);
    [DllImport("kernel32.dll")] private static extern uint WaitForSingleObject(IntPtr handle, uint ms);
    [DllImport("kernel32.dll")] private static extern bool SetEvent(IntPtr handle);
    [DllImport("kernel32.dll")] private static extern bool CloseHandle(IntPtr handle);

    [StructLayout(LayoutKind.Sequential)]
    private struct AUDIOCLIENT_ACTIVATION_PARAMS
    {
        public int ActivationType;
        public uint TargetProcessId;
        public int ProcessLoopbackMode;
    }

    [StructLayout(LayoutKind.Sequential, Pack = 1)]
    private struct WAVEFORMATEX
    {
        public ushort wFormatTag, nChannels;
        public uint nSamplesPerSec, nAvgBytesPerSec;
        public ushort nBlockAlign, wBitsPerSample, cbSize;
    }

    // Minimal PROPVARIANT carrying a VT_BLOB (the 8-byte union after vt+padding holds cbSize+ptr on x64).
    [StructLayout(LayoutKind.Sequential)]
    private struct PROPVARIANT
    {
        public ushort vt;
        public ushort r1, r2, r3;
        public uint blobSize;
        public IntPtr blobData;
    }

    [ComImport, Guid("72A22D78-CDE4-431D-B8CC-843A71199B6D"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IActivateAudioInterfaceAsyncOperation
    {
        [PreserveSig] int GetActivateResult(out int activateResult, [MarshalAs(UnmanagedType.IUnknown)] out object activatedInterface);
    }

    [ComImport, Guid("1CB9AD4C-DBFA-4c32-B178-C2F568A703B2"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IAudioClient
    {
        [PreserveSig] int Initialize(uint shareMode, uint streamFlags, long hnsBufferDuration, long hnsPeriodicity, IntPtr format, IntPtr audioSessionGuid);
        [PreserveSig] int GetBufferSize(out uint numBufferFrames);
        [PreserveSig] int GetStreamLatency(out long latency);
        [PreserveSig] int GetCurrentPadding(out uint numPaddingFrames);
        [PreserveSig] int IsFormatSupported(uint shareMode, IntPtr format, out IntPtr closestMatch);
        [PreserveSig] int GetMixFormat(out IntPtr deviceFormat);
        [PreserveSig] int GetDevicePeriod(out long defaultPeriod, out long minimumPeriod);
        [PreserveSig] int Start();
        [PreserveSig] int Stop();
        [PreserveSig] int Reset();
        [PreserveSig] int SetEventHandle(IntPtr eventHandle);
        [PreserveSig] int GetService(ref Guid interfaceId, [MarshalAs(UnmanagedType.IUnknown)] out object service);
    }

    [ComImport, Guid("C8ADBD64-E71E-48a0-A4DE-185C395CD317"), InterfaceType(ComInterfaceType.InterfaceIsIUnknown)]
    private interface IAudioCaptureClient
    {
        [PreserveSig] int GetBuffer(out IntPtr data, out uint numFramesToRead, out uint bufferFlags, out long devicePosition, out long qpcPosition);
        [PreserveSig] int ReleaseBuffer(uint numFramesRead);
        [PreserveSig] int GetNextPacketSize(out uint numFramesInNextPacket);
    }
}
