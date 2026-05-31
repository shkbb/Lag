using System.Runtime.InteropServices;
using System.Runtime.Versioning;

#if WINDOWS
using SharpDX;
using SharpDX.Direct3D11;
using SharpDX.DXGI;
using Device = SharpDX.Direct3D11.Device;
using MapFlags = SharpDX.Direct3D11.MapFlags;
#endif

namespace Lag.Services;

/// <summary>
/// Windows-specific screen capture using DXGI Desktop Duplication API.
/// Provides high-performance GPU-accelerated desktop frame capture.
/// 
/// Architecture Notes:
///   - Creates a D3D11 device and acquires an OutputDuplication interface.
///   - Frames are copied from GPU to a CPU-readable staging texture.
///   - On DXGI_ERROR_ACCESS_LOST (e.g., resolution change, UAC prompt),
///     the duplication is automatically re-created.
///   - Supports multi-monitor by enumerating DXGI outputs per adapter.
/// </summary>
[SupportedOSPlatform("windows")]
public class WindowsCaptureService : IScreenCaptureService
{
#if WINDOWS
    private Device? _device;
    private OutputDuplication? _duplication;
    private Texture2D? _stagingTexture;
    private Output1? _output;
    private int _width;
    private int _height;
    private bool _initialized;
    private readonly object _lock = new();

    /// <inheritdoc />
    public bool IsInitialized => _initialized;

    /// <inheritdoc />
    public IReadOnlyList<MonitorInfo> GetAvailableMonitors()
    {
        var monitors = new List<MonitorInfo>();

        using var factory = new Factory1();
        for (int adapterIdx = 0; adapterIdx < factory.GetAdapterCount1(); adapterIdx++)
        {
            using var adapter = factory.GetAdapter1(adapterIdx);
            for (int outputIdx = 0; outputIdx < adapter.GetOutputCount(); outputIdx++)
            {
                using var output = adapter.GetOutput(outputIdx);
                var desc = output.Description;
                var bounds = desc.DesktopBounds;
                int width = bounds.Right - bounds.Left;
                int height = bounds.Bottom - bounds.Top;

                monitors.Add(new MonitorInfo(
                    Index: monitors.Count,
                    DeviceName: desc.DeviceName,
                    Width: width,
                    Height: height,
                    IsPrimary: bounds.Left == 0 && bounds.Top == 0
                ));
            }
        }

        return monitors;
    }

    /// <inheritdoc />
    /// <remarks>
    /// Initializes DXGI Desktop Duplication on the specified monitor.
    /// Creates the D3D11 device, gets the output, and starts duplication.
    /// A staging texture is pre-allocated for CPU readback of captured frames.
    /// </remarks>
    public void Initialize(int monitorIndex)
    {
        lock (_lock)
        {
            Shutdown();

            using var factory = new Factory1();
            int currentIndex = 0;

            for (int adapterIdx = 0; adapterIdx < factory.GetAdapterCount1(); adapterIdx++)
            {
                using var adapter = factory.GetAdapter1(adapterIdx);
                for (int outputIdx = 0; outputIdx < adapter.GetOutputCount(); outputIdx++)
                {
                    if (currentIndex == monitorIndex)
                    {
                        _device = new Device(adapter,
                            DeviceCreationFlags.BgraSupport |
                            DeviceCreationFlags.VideoSupport);

                        using var output = adapter.GetOutput(outputIdx);
                        _output = output.QueryInterface<Output1>();
                        var desc = output.Description;
                        _width = desc.DesktopBounds.Right - desc.DesktopBounds.Left;
                        _height = desc.DesktopBounds.Bottom - desc.DesktopBounds.Top;

                        // Pre-allocate a staging texture for CPU readback
                        _stagingTexture = new Texture2D(_device, new Texture2DDescription
                        {
                            Width = _width,
                            Height = _height,
                            MipLevels = 1,
                            ArraySize = 1,
                            Format = Format.B8G8R8A8_UNorm,
                            SampleDescription = new SampleDescription(1, 0),
                            Usage = ResourceUsage.Staging,
                            CpuAccessFlags = CpuAccessFlags.Read,
                            BindFlags = BindFlags.None,
                            OptionFlags = ResourceOptionFlags.None
                        });

                        _duplication = _output.DuplicateOutput(_device);
                        _initialized = true;
                        return;
                    }
                    currentIndex++;
                }
            }

            throw new ArgumentException($"Monitor index {monitorIndex} not found.");
        }
    }

    /// <inheritdoc />
    /// <remarks>
    /// Acquires the next frame from the DXGI duplication interface.
    /// The frame is copied to a staging texture and then read from CPU memory.
    /// Returns raw BGRA pixel data suitable for piping into FFmpeg.
    /// 
    /// If DXGI_ERROR_ACCESS_LOST occurs (screen resolution changed, secure desktop, etc.),
    /// the method returns null and a re-initialization should be triggered by the caller.
    /// </remarks>
    public CapturedFrame? AcquireNextFrame(int timeoutMs = 100)
    {
        lock (_lock)
        {
            if (!_initialized || _duplication == null || _device == null || _stagingTexture == null)
                return null;

            try
            {
                var result = _duplication.TryAcquireNextFrame(timeoutMs,
                    out var frameInfo, out var desktopResource);

                if (result.Failure)
                    return null;

                using (desktopResource)
                using (var texture = desktopResource.QueryInterface<Texture2D>())
                {
                    // Copy GPU texture to CPU-readable staging texture
                    _device.ImmediateContext.CopyResource(texture, _stagingTexture);
                }

                _duplication.ReleaseFrame();

                // Map the staging texture for CPU read
                var dataBox = _device.ImmediateContext.MapSubresource(
                    _stagingTexture, 0, MapMode.Read, MapFlags.None);

                try
                {
                    int rowPitch = dataBox.RowPitch;
                    int dataSize = _width * _height * 4; // BGRA = 4 bytes per pixel
                    var frameData = new byte[dataSize];

                    // Copy row-by-row to handle stride differences
                    for (int y = 0; y < _height; y++)
                    {
                        Marshal.Copy(
                            dataBox.DataPointer + y * rowPitch,
                            frameData,
                            y * _width * 4,
                            _width * 4);
                    }

                    return new CapturedFrame
                    {
                        Data = frameData,
                        Width = _width,
                        Height = _height,
                        Timestamp = DateTimeOffset.UtcNow
                    };
                }
                finally
                {
                    _device.ImmediateContext.UnmapSubresource(_stagingTexture, 0);
                }
            }
            catch (SharpDXException ex) when (ex.ResultCode == SharpDX.DXGI.ResultCode.AccessLost)
            {
                // Desktop mode changed or secure desktop — needs re-init
                _initialized = false;
                return null;
            }
        }
    }

    /// <inheritdoc />
    public void Shutdown()
    {
        lock (_lock)
        {
            _initialized = false;
            _duplication?.Dispose();
            _duplication = null;
            _stagingTexture?.Dispose();
            _stagingTexture = null;
            _output?.Dispose();
            _output = null;
            _device?.Dispose();
            _device = null;
        }
    }

    public void Dispose() => Shutdown();

#else
    // Stub for non-Windows compilation
    public bool IsInitialized => false;
    public IReadOnlyList<MonitorInfo> GetAvailableMonitors() => Array.Empty<MonitorInfo>();
    public void Initialize(int monitorIndex) => throw new PlatformNotSupportedException("DXGI is Windows-only.");
    public CapturedFrame? AcquireNextFrame(int timeoutMs = 100) => null;
    public void Shutdown() { }
    public void Dispose() { }
#endif
}
