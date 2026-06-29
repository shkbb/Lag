using System;
using SharpDX.Direct3D11;
using D3D = SharpDX.Direct3D;
using DxgiDevice = SharpDX.DXGI.Device;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Owns the single Direct3D11 device the whole VFR pipeline shares: Windows Graphics Capture
/// delivers frames as textures on it, the colour converter runs on it, and FFmpeg's NVENC
/// hwframes wrap textures from it — keeping every frame on ONE GPU end to end (zero copies to
/// system RAM, the thing that wrecked the old capture path under game load).
///
/// The device is created with BGRA support (WGC requires it) and the video device interfaces
/// (for the hardware colour-space conversion). We use the default hardware adapter, which is the
/// discrete GPU on the vast majority of gaming rigs; richer multi-GPU adapter matching can be
/// layered on later without changing callers.
/// </summary>
public sealed class D3D11Context : IDisposable
{
    public Device Device { get; }
    public DeviceContext Context => Device.ImmediateContext;

    /// <summary>The DXGI adapter description, for logging which GPU we bound to.</summary>
    public string AdapterName { get; }

    /// <summary>The adapter's PCI vendor id (0x10DE NVIDIA, 0x1002 AMD, 0x8086 Intel). Used to pick
    /// a hardware encoder of the SAME vendor (a cross-vendor hw encoder can't read our textures).</summary>
    public int VendorId { get; }

    /// <summary><paramref name="adapterIndex"/> selects a specific DXGI adapter (the user's GPU
    /// choice); -1 = default hardware adapter (the discrete GPU on most rigs).</summary>
    public D3D11Context(int adapterIndex = -1)
    {
        // BgraSupport: mandatory for WGC interop. VideoSupport: lets us create the
        // ID3D11VideoDevice used for hardware BGRA→NV12 conversion.
        var flags = DeviceCreationFlags.BgraSupport | DeviceCreationFlags.VideoSupport;
        if (adapterIndex >= 0)
        {
            try
            {
                using var factory = new SharpDX.DXGI.Factory1();
                using var chosen = factory.GetAdapter1(adapterIndex);
                Device = new Device(chosen, flags);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[D3D11Context] adapter {adapterIndex} unavailable ({ex.Message}); using default.");
                Device = new Device(D3D.DriverType.Hardware, flags);
            }
        }
        else
        {
            Device = new Device(D3D.DriverType.Hardware, flags);
        }

        using var dxgiDevice = Device.QueryInterface<DxgiDevice>();
        using var adapter = dxgiDevice.Adapter;
        AdapterName = adapter.Description.Description;
        VendorId = adapter.Description.VendorId;

        // The device is shared across the WGC callback thread, the converter and the encoder
        // submit path. D3D11 immediate contexts are NOT thread-safe, so enable the driver's
        // internal locking; callers still serialise their own work, this is defence in depth.
        using var mt = Device.QueryInterface<Multithread>();
        mt.SetMultithreadProtected(true);

        Console.WriteLine($"[D3D11Context] Device created on adapter: {AdapterName}");
    }

    public void Dispose() => Device?.Dispose();
}
