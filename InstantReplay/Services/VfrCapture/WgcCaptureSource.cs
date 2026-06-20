using System;
using System.Runtime.InteropServices;
using Windows.Foundation;
using Windows.Foundation.Metadata;
using Windows.Graphics;
using Windows.Graphics.Capture;
using Windows.Graphics.DirectX;
using Windows.Graphics.DirectX.Direct3D11;
using SharpDX.Direct3D11;
using WinRT;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Captures ONE window's swapchain via Windows Graphics Capture and raises a frame event with
/// the underlying D3D11 texture and its real presentation time. This is the Game-Bar
/// style capture: it reads the window's own backbuffer, so it sees a fullscreen game past
/// independent flip (which monitor/DXGI duplication cannot) at the game's true, variable rate —
/// no hooks, no Trusted-Mode conflicts.
///
/// One instance captures one window for its whole lifetime. Switching games means disposing this
/// and creating a new one; we never retarget a live session (retargeting restarts the WGC frame
/// pool, a ~0.5s stall — the exact cause of the freezes in the old OBS overlay approach). The
/// orchestrator decides when a new game warrants a new source.
///
/// <see cref="FrameReady"/> fires on a WGC worker thread (MTA). The supplied texture is owned by
/// the frame pool and is ONLY valid for the duration of the callback — consume it synchronously
/// (convert/encode) and do not retain the reference.
/// </summary>
public sealed class WgcCaptureSource : IDisposable
{
    private readonly D3D11Context _d3d;
    private readonly IntPtr _handle;     // HWND (window capture) or HMONITOR (display capture)
    private readonly bool _isMonitor;

    private GraphicsCaptureItem? _item;
    private Direct3D11CaptureFramePool? _framePool;
    private GraphicsCaptureSession? _session;
    private TypedEventHandler<Direct3D11CaptureFramePool, object>? _frameHandler;
    private SizeInt32 _lastSize;
    private volatile bool _disposed;

    /// <summary>(texture valid only during the call, real present time in 100ns ticks).</summary>
    public event Action<Texture2D, long>? FrameReady;

    /// <summary>Raised when the captured window closes — orchestrator should release the lock.</summary>
    public event Action? Closed;

    /// <summary>Native pixel size of the captured window (updates on resize).</summary>
    public int Width => _lastSize.Width;
    public int Height => _lastSize.Height;

    /// <summary>Capture a single window's backbuffer (game / app window).</summary>
    public WgcCaptureSource(D3D11Context d3d, IntPtr hwnd) : this(d3d, hwnd, isMonitor: false) { }

    /// <summary>Capture a whole monitor (desktop / "record anything" mode). <paramref name="hmonitor"/>
    /// is a Win32 HMONITOR.</summary>
    public static WgcCaptureSource ForMonitor(D3D11Context d3d, IntPtr hmonitor) => new(d3d, hmonitor, isMonitor: true);

    private WgcCaptureSource(D3D11Context d3d, IntPtr handle, bool isMonitor)
    {
        _d3d = d3d;
        _handle = handle;
        _isMonitor = isMonitor;
    }

    /// <summary>Begins capture. Throws on failure so the orchestrator can fall back cleanly.
    /// <paramref name="maxFps"/> (0 = uncapped) sets a generous MinUpdateInterval floor —
    /// setting it explicitly yields the full window rate; without it WGC appears to throttle to
    /// the ~60 vsync rate.</summary>
    public void Start(bool captureCursor, int maxFps = 0)
    {
        // Wrap our shared D3D11 device as the WinRT device WGC renders frames onto.
        using var dxgiDevice = _d3d.Device.QueryInterface<SharpDX.DXGI.Device>();
        int hr = CreateDirect3D11DeviceFromDXGIDevice(dxgiDevice.NativePointer, out IntPtr winrtDevPtr);
        if (hr != 0 || winrtDevPtr == IntPtr.Zero)
            throw new InvalidOperationException($"CreateDirect3D11DeviceFromDXGIDevice failed 0x{hr:X8}");
        var winrtDevice = MarshalInspectable<IDirect3DDevice>.FromAbi(winrtDevPtr);
        Marshal.Release(winrtDevPtr);

        _item = _isMonitor ? CreateItemForMonitor(_handle) : CreateItemForWindow(_handle);
        _lastSize = _item.Size;
        _item.Closed += (_, _) => { if (!_disposed) Closed?.Invoke(); };

        // 3 buffers: a little extra slack so a brief consume hiccup can't make WGC skip delivering
        // the next present (B8G8R8A8 is what windows present in).
        _framePool = Direct3D11CaptureFramePool.CreateFreeThreaded(
            winrtDevice, DirectXPixelFormat.B8G8R8A8UIntNormalized, 3, _lastSize);

        _frameHandler = OnFrameArrived;
        _framePool.FrameArrived += _frameHandler;

        _session = _framePool.CreateCaptureSession(_item);
        TrySet(() => _session.IsCursorCaptureEnabled = captureCursor);

        // Kill the yellow WGC capture border. The earlier "just set IsBorderRequired=false" did NOT
        // remove it on Windows 10 — because the OS keeps drawing the border unless the app first
        // REQUESTS borderless access. So do both, in order:
        //   1) GraphicsCaptureAccess.RequestAccessAsync(Borderless) — get permission to hide the border.
        //   2) IsBorderRequired = false — set unconditionally inside TrySet, so a build without the
        //      property just no-ops instead of being wrongly skipped by a capability check.
        try
        {
            GraphicsCaptureAccess.RequestAccessAsync(GraphicsCaptureAccessKind.Borderless)
                                 .GetAwaiter().GetResult();
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[WgcCaptureSource] borderless access request unavailable: {ex.Message}");
        }

        bool borderOff = false;
        TrySet(() => { _session.IsBorderRequired = false; borderOff = true; });
        Console.WriteLine(borderOff
            ? "[WgcCaptureSource] disabled WinRT capture border."
            : "[WgcCaptureSource] capture-border toggle not available on this OS build.");

        // MinUpdateInterval (Win11 24H2, build 26100). Setting this explicitly appears
        // to switch WGC from vsync-throttled (~60) to present-driven delivery. Use a floor well
        // above the target so it never actually caps us — it's the act of setting it that matters.
        if (maxFps > 0 && OperatingSystem.IsWindowsVersionAtLeast(10, 0, 26100))
        {
            double ms = 1000.0 / (maxFps * 2.0);
            TrySet(() => _session.MinUpdateInterval = TimeSpan.FromMilliseconds(ms));
            Console.WriteLine($"[WgcCaptureSource] MinUpdateInterval set to {ms:F2}ms (floor {maxFps * 2}fps).");
        }

        _session.StartCapture();
        Console.WriteLine($"[WgcCaptureSource] Capturing {(_isMonitor ? "monitor" : "hwnd")} 0x{_handle:X} at {_lastSize.Width}x{_lastSize.Height}.");
    }

    private void OnFrameArrived(Direct3D11CaptureFramePool sender, object args)
    {
        if (_disposed) return;
        Direct3D11CaptureFrame? frame = null;
        try
        {
            frame = sender.TryGetNextFrame();
            if (frame == null) return;

            // The game changed resolution → resize the pool so frames keep coming full-size.
            var size = frame.ContentSize;
            if (size.Width != _lastSize.Width || size.Height != _lastSize.Height)
            {
                _lastSize = size;
                using var dev = GetWinRtDevice();
                sender.Recreate(dev, DirectXPixelFormat.B8G8R8A8UIntNormalized, 2, size);
            }

            using var texture = GetTexture(frame.Surface);
            if (texture != null)
                FrameReady?.Invoke(texture, frame.SystemRelativeTime.Ticks);
        }
        catch (Exception ex)
        {
            // A bad frame must not kill the capture loop.
            Console.WriteLine($"[WgcCaptureSource] frame error: {ex.Message}");
        }
        finally
        {
            frame?.Dispose();
        }
    }

    /// <summary>Extracts the ID3D11Texture2D backing a WGC surface (raw QI+vtable; CsWinRT can't
    /// cast a projected surface to a [ComImport] interop interface directly).</summary>
    private static unsafe Texture2D? GetTexture(IDirect3DSurface surface)
    {
        IntPtr surfPtr = MarshalInspectable<IDirect3DSurface>.FromManaged(surface);
        try
        {
            Guid accessIid = new("A9B3D012-3DF2-4EE3-B8D1-8695F457D3C1"); // IDirect3DDxgiInterfaceAccess
            if (Marshal.QueryInterface(surfPtr, ref accessIid, out IntPtr accessPtr) != 0) return null;
            try
            {
                Guid texIid = new("6f15aaf2-d208-4e89-9ab4-489535d34f9c"); // ID3D11Texture2D
                IntPtr* vtbl = *(IntPtr**)accessPtr;
                var getInterface = (delegate* unmanaged[Stdcall]<IntPtr, Guid*, IntPtr*, int>)vtbl[3];
                IntPtr texPtr;
                int hr = getInterface(accessPtr, &texIid, &texPtr);
                return (hr == 0 && texPtr != IntPtr.Zero) ? new Texture2D(texPtr) : null;
            }
            finally { Marshal.Release(accessPtr); }
        }
        finally { Marshal.Release(surfPtr); }
    }

    private IDirect3DDevice GetWinRtDevice()
    {
        using var dxgiDevice = _d3d.Device.QueryInterface<SharpDX.DXGI.Device>();
        CreateDirect3D11DeviceFromDXGIDevice(dxgiDevice.NativePointer, out IntPtr p);
        var dev = MarshalInspectable<IDirect3DDevice>.FromAbi(p);
        Marshal.Release(p);
        return dev;
    }

    public void Dispose()
    {
        _disposed = true;
        try { if (_frameHandler != null && _framePool != null) _framePool.FrameArrived -= _frameHandler; } catch { }
        TryDispose(_session);
        TryDispose(_framePool);
        _session = null;
        _framePool = null;
        _item = null;
    }

    // ───────────────────────── WinRT / interop plumbing ─────────────────────────

    private static void TrySet(Action a) { try { a(); } catch { } }
    private static void TryDispose(object? o) { try { (o as IDisposable)?.Dispose(); } catch { } }

    [DllImport("d3d11.dll", EntryPoint = "CreateDirect3D11DeviceFromDXGIDevice", SetLastError = true)]
    private static extern int CreateDirect3D11DeviceFromDXGIDevice(IntPtr dxgiDevice, out IntPtr graphicsDevice);

    [DllImport("combase.dll", CharSet = CharSet.Unicode, PreserveSig = false)]
    private static extern void WindowsCreateString([MarshalAs(UnmanagedType.LPWStr)] string src, int len, out IntPtr hstring);

    [DllImport("combase.dll", PreserveSig = false)]
    private static extern void RoGetActivationFactory(IntPtr classId, [In] ref Guid iid, out IntPtr factory);

    private static readonly Guid IID_IGraphicsCaptureItemInterop = new("3628E81B-3CAC-4C60-B7F4-23CE0E0C3356");
    private static readonly Guid IID_IGraphicsCaptureItem = new("79C3F95B-31F7-4EC2-A464-632EF5D30760");

    private static unsafe GraphicsCaptureItem CreateItemForWindow(IntPtr hwnd)
    {
        const string typeName = "Windows.Graphics.Capture.GraphicsCaptureItem";
        WindowsCreateString(typeName, typeName.Length, out IntPtr hstr);
        Guid interopIid = IID_IGraphicsCaptureItemInterop;
        RoGetActivationFactory(hstr, ref interopIid, out IntPtr factoryPtr);
        if (factoryPtr == IntPtr.Zero)
            throw new InvalidOperationException("GraphicsCaptureItem interop factory unavailable (WGC unsupported?).");
        try
        {
            Guid itemIid = IID_IGraphicsCaptureItem;
            IntPtr* vtbl = *(IntPtr**)factoryPtr;
            var createForWindow = (delegate* unmanaged[Stdcall]<IntPtr, IntPtr, Guid*, IntPtr*, int>)vtbl[3];
            IntPtr itemPtr;
            int hr = createForWindow(factoryPtr, hwnd, &itemIid, &itemPtr);
            if (hr != 0 || itemPtr == IntPtr.Zero)
                throw new InvalidOperationException($"CreateForWindow failed 0x{hr:X8}");
            var item = MarshalInspectable<GraphicsCaptureItem>.FromAbi(itemPtr);
            Marshal.Release(itemPtr);
            return item;
        }
        finally { Marshal.Release(factoryPtr); }
    }

    /// <summary>Builds a capture item for a whole monitor (desktop capture). Same interop as the
    /// window path but calls IGraphicsCaptureItemInterop::CreateForMonitor (vtable slot 4) instead of
    /// CreateForWindow (slot 3). HMONITOR in, GraphicsCaptureItem out.</summary>
    private static unsafe GraphicsCaptureItem CreateItemForMonitor(IntPtr hmonitor)
    {
        const string typeName = "Windows.Graphics.Capture.GraphicsCaptureItem";
        WindowsCreateString(typeName, typeName.Length, out IntPtr hstr);
        Guid interopIid = IID_IGraphicsCaptureItemInterop;
        RoGetActivationFactory(hstr, ref interopIid, out IntPtr factoryPtr);
        if (factoryPtr == IntPtr.Zero)
            throw new InvalidOperationException("GraphicsCaptureItem interop factory unavailable (WGC unsupported?).");
        try
        {
            Guid itemIid = IID_IGraphicsCaptureItem;
            IntPtr* vtbl = *(IntPtr**)factoryPtr;
            var createForMonitor = (delegate* unmanaged[Stdcall]<IntPtr, IntPtr, Guid*, IntPtr*, int>)vtbl[4];
            IntPtr itemPtr;
            int hr = createForMonitor(factoryPtr, hmonitor, &itemIid, &itemPtr);
            if (hr != 0 || itemPtr == IntPtr.Zero)
                throw new InvalidOperationException($"CreateForMonitor failed 0x{hr:X8}");
            var item = MarshalInspectable<GraphicsCaptureItem>.FromAbi(itemPtr);
            Marshal.Release(itemPtr);
            return item;
        }
        finally { Marshal.Release(factoryPtr); }
    }

    /// <summary>Whether Windows Graphics Capture is usable on this OS (Win10 1903+).</summary>
    public static bool IsSupported
    {
        get { try { return GraphicsCaptureSession.IsSupported(); } catch { return false; } }
    }
}
