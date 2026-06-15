using System;
using System.Runtime.InteropServices;
using SharpDX.Direct3D11;
using SharpDX.DXGI;
using SharpDX.Mathematics.Interop;
using D3D11Device = SharpDX.Direct3D11.Device;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Hardware BGRA→NV12 colour conversion and scaling via the D3D11 video processor — the same
/// fixed-function block GPUs use for video playback, so it is effectively free and keeps the
/// frame on the GPU. It writes straight into the encoder's own hwframe texture (an NV12 texture
/// ARRAY slice), so there is not a single copy to system RAM anywhere in capture→convert→encode.
///
/// Input is the captured window texture (any size, BGRA); output is one slice of the encoder's
/// NV12 frame pool at the recording resolution. WGC pool textures don't carry video-processor
/// input bind flags, so we first CopyResource them into our own input texture (a cheap GPU→GPU
/// copy) and convert from that.
/// </summary>
public sealed class Bgra2Nv12Converter : IDisposable
{
    private readonly D3D11Device _device;
    private readonly VideoDevice _videoDevice;
    private readonly VideoContext _videoContext;
    private readonly int _outW, _outH;

    private VideoProcessor? _processor;
    private VideoProcessorEnumerator? _enumerator;
    private Texture2D? _inputCopy;
    private int _inW, _inH;

    // We DON'T blit straight into the encoder hwframe's array slice: D3D11 VideoProcessor output
    // to a Texture2D *array slice* is unreliable on NVIDIA — it silently skipped ~half the frames,
    // leaving them raw-zero (= solid green). Instead we blit into our own single (non-array) NV12
    // scratch texture, then a plain engine CopySubresourceRegion moves it into the pool slice.
    private Texture2D? _scratchNv12;
    private VideoProcessorOutputView? _scratchView;
    private Texture2D? _stagingNv12;   // CPU-readable NV12 for the system-memory (x264) download path

    // ── Green-flash diagnostics (LAG_VFR_DIAG=1) ──────────────────────────────────────────────
    // Distinguishes WHERE the solid-green frames come from: a black SOURCE frame converts to Y≈16
    // (black, NOT green); only a NV12 slice the video processor NEVER WROTE stays Y=0 (= green).
    // So we sample, post-fence, both the source patch and the just-written output slice. One run
    // tells us: nv12Green>0 with srcBlack≈0 ⇒ the GPU blit/slice path; srcBlack tracks dark scenes.
    private static readonly bool Diag = Environment.GetEnvironmentVariable("LAG_VFR_DIAG") == "1";
    private Texture2D? _srcProbe, _nv12Probe;
    private Format _srcProbeFmt;
    private long _diagFrames, _srcBlack, _nv12Green;

    public Bgra2Nv12Converter(D3D11Context ctx, int outputWidth, int outputHeight)
    {
        _device = ctx.Device;
        _outW = outputWidth & ~1;   // NV12 needs even dimensions
        _outH = outputHeight & ~1;
        _videoDevice = _device.QueryInterface<VideoDevice>();
        _videoContext = _device.ImmediateContext.QueryInterface<VideoContext>();
    }

    /// <summary>Scales + colour-converts the BGRA frame into the single NV12 scratch texture.
    /// Shared by both the hardware (copy-to-pool-slice) and system-memory (download) paths.</summary>
    private void BlitToScratch(Texture2D srcBgra)
    {
        var d = srcBgra.Description;
        EnsurePipeline(d.Width, d.Height);

        _device.ImmediateContext.CopyResource(srcBgra, _inputCopy);

        var ivd = new VideoProcessorInputViewDescription { FourCC = 0, Dimension = VpivDimension.Texture2D };
        ivd.Texture2D.MipSlice = 0;
        ivd.Texture2D.ArraySlice = 0;
        _videoDevice.CreateVideoProcessorInputView(_inputCopy, _enumerator, ivd, out var inputView);
        try
        {
            var stream = new VideoProcessorStream
            {
                Enable = new RawBool(true),
                OutputIndex = 0,
                InputFrameOrField = 0,
                PastFrames = 0,
                FutureFrames = 0,
                PInputSurface = inputView,
            };
            // Reliable into a single (non-array) NV12 scratch; an array-slice blit silently skipped
            // ~half the frames (green) on NVIDIA.
            _videoContext.VideoProcessorBlt(_processor, _scratchView, 0, 1, new[] { stream });
        }
        finally { inputView.Dispose(); }
    }

    /// <summary>
    /// HARDWARE path: convert and GPU-copy the NV12 into the encoder's pool slice (zero CPU copy).
    /// Must run on the device-context thread (the WGC frame callback).
    /// </summary>
    public void ConvertInto(Texture2D srcBgra, Texture2D dstNv12Array, int slice)
    {
        BlitToScratch(srcBgra);
        // Plain GPU copy of the finished NV12 into the pool slice; encoder reads it afterwards on the
        // same immediate context, so no per-frame CPU fence is needed.
        _device.ImmediateContext.CopySubresourceRegion(_scratchNv12, 0, null, dstNv12Array, slice, 0, 0, 0);
        if (Diag) ProbeForGreen(srcBgra, dstNv12Array, slice);
    }

    /// <summary>
    /// SYSTEM-MEMORY path (x264 / cross-vendor encoders): convert, then download the NV12 into a
    /// CPU-readable staging texture and map it. Returns the mapped box — Y plane at DataPointer,
    /// UV plane at DataPointer + RowPitch*Height. Caller copies into its frame then calls
    /// <see cref="UnmapSystemNv12"/>. <see cref="OutWidth"/>/<see cref="OutHeight"/> give the size.
    /// </summary>
    public SharpDX.DataBox MapSystemNv12(Texture2D srcBgra)
    {
        BlitToScratch(srcBgra);
        var ic = _device.ImmediateContext;
        ic.CopyResource(_scratchNv12, _stagingNv12);   // GPU NV12 → CPU-readable staging
        return ic.MapSubresource(_stagingNv12, 0, SharpDX.Direct3D11.MapMode.Read, SharpDX.Direct3D11.MapFlags.None);
    }

    public void UnmapSystemNv12() => _device.ImmediateContext.UnmapSubresource(_stagingNv12, 0);

    public int OutWidth => _outW;
    public int OutHeight => _outH;

    /// <summary>Diagnostic: after the convert has fully completed on the GPU, read back a small
    /// patch of the source and of the just-written NV12 slice to classify solid-green frames.
    /// Green = NV12 luma still 0 (slice never written); a genuinely black source reads Y≈16.</summary>
    private void ProbeForGreen(Texture2D srcBgra, Texture2D dstNv12Array, int slice)
    {
        const int P = 16; // even patch, top-left corner — whole-frame green shows here
        var ic = _device.ImmediateContext;
        try
        {
            // Source patch (any 32-bit desktop format): black if all bytes ~0.
            var sd = srcBgra.Description;
            if (_srcProbe == null || _srcProbeFmt != sd.Format)
            {
                _srcProbe?.Dispose();
                _srcProbeFmt = sd.Format;
                _srcProbe = NewStaging(P, P, sd.Format);
            }
            ic.CopySubresourceRegion(srcBgra, 0, new ResourceRegion(0, 0, 0, P, P, 1), _srcProbe, 0);
            if (MaxByte(_srcProbe, P, P * 4) < 4) _srcBlack++;

            // Output NV12 slice luma: 0 ⇒ unwritten ⇒ renders solid green.
            _nv12Probe ??= NewStaging(P, P, Format.NV12);
            ic.CopySubresourceRegion(dstNv12Array, slice, new ResourceRegion(0, 0, 0, P, P, 1), _nv12Probe, 0);
            if (MaxByte(_nv12Probe, P, P) < 8) _nv12Green++;

            if (++_diagFrames % 120 == 0)
                Console.WriteLine($"[DIAG] frames={_diagFrames} srcBlack={_srcBlack} nv12Green={_nv12Green}");
        }
        catch (Exception ex) { Console.WriteLine($"[DIAG] probe error: {ex.Message}"); }
    }

    private Texture2D NewStaging(int w, int h, Format fmt) => new(_device, new Texture2DDescription
    {
        Width = w, Height = h, MipLevels = 1, ArraySize = 1, Format = fmt,
        SampleDescription = new SampleDescription(1, 0), Usage = ResourceUsage.Staging,
        BindFlags = BindFlags.None, CpuAccessFlags = CpuAccessFlags.Read, OptionFlags = ResourceOptionFlags.None,
    });

    /// <summary>Max byte value over <paramref name="rows"/> rows × <paramref name="rowBytes"/> bytes
    /// of a mapped staging texture's first plane (Y for NV12).</summary>
    private byte MaxByte(Texture2D staging, int rows, int rowBytes)
    {
        var box = _device.ImmediateContext.MapSubresource(staging, 0, MapMode.Read, SharpDX.Direct3D11.MapFlags.None);
        try
        {
            byte max = 0;
            for (int y = 0; y < rows; y++)
            {
                IntPtr row = box.DataPointer + y * box.RowPitch;
                for (int x = 0; x < rowBytes; x++)
                {
                    byte v = Marshal.ReadByte(row, x);
                    if (v > max) max = v;
                }
            }
            return max;
        }
        finally { _device.ImmediateContext.UnmapSubresource(staging, 0); }
    }

    private void EnsurePipeline(int inW, int inH)
    {
        if (_processor != null && inW == _inW && inH == _inH) return;

        _inW = inW; _inH = inH;
        _scratchView?.Dispose();
        _scratchNv12?.Dispose();
        _stagingNv12?.Dispose();
        _processor?.Dispose();
        _enumerator?.Dispose();
        _inputCopy?.Dispose();

        var content = new VideoProcessorContentDescription
        {
            InputFrameFormat = VideoFrameFormat.Progressive,
            InputWidth = inW,
            InputHeight = inH,
            OutputWidth = _outW,
            OutputHeight = _outH,
            InputFrameRate = new Rational(120, 1),
            OutputFrameRate = new Rational(120, 1),
            Usage = VideoUsage.PlaybackNormal,
        };
        _videoDevice.CreateVideoProcessorEnumerator(ref content, out _enumerator);
        _videoDevice.CreateVideoProcessor(_enumerator, 0, out _processor);

        _videoContext.VideoProcessorSetStreamSourceRect(_processor, 0, true, new RawRectangle(0, 0, inW, inH));
        _videoContext.VideoProcessorSetStreamDestRect(_processor, 0, true, new RawRectangle(0, 0, _outW, _outH));
        _videoContext.VideoProcessorSetOutputTargetRect(_processor, true, new RawRectangle(0, 0, _outW, _outH));

        // Colour space — universal for any SDR game, not CS2-specific: WGC captures the window's
        // DWM-composited surface, which is ALWAYS full-range sRGB RGB (0-255, B8G8R8A8) regardless
        // of the game. BT.709 here is just the matrix WE encode with, and the encoder is tagged to
        // match (NvencVfrEncoder), so the RGB→NV12→RGB round-trip is exact for every title. Without
        // this the driver defaults to BT.601 + limited range → darker, colour-shifted picture.
        // NOTE: HDR games (10-bit BT.2020/PQ → R10G10B10A2/FP16 from WGC) are NOT handled by this
        // 8-bit path yet — they'd need tone-mapping or a P010 pipeline (see _inputCopy format).
        var inCs = new VideoProcessorColorSpace { RgbRange = false };           // false = full-range RGB in
        _videoContext.VideoProcessorSetStreamColorSpace(_processor, 0, inCs);
        // Output LIMITED-range (16-235) BT.709 — the standard video range every recorder (incl.
        // Medal: color_range=tv) uses and players handle unambiguously. Full-range out looked
        // over-contrasted/dark because the round-trip got double-expanded. Encoder tags MPEG/tv.
        var outCs = new VideoProcessorColorSpace { YCbCrMatrix = true, NominalRange = 1 }; // BT.709, 16-235
        _videoContext.VideoProcessorSetOutputColorSpace(_processor, outCs);

        _inputCopy = new Texture2D(_device, new Texture2DDescription
        {
            Width = inW,
            Height = inH,
            MipLevels = 1,
            ArraySize = 1,
            Format = Format.B8G8R8A8_UNorm,
            SampleDescription = new SampleDescription(1, 0),
            Usage = ResourceUsage.Default,
            BindFlags = BindFlags.RenderTarget | BindFlags.ShaderResource,
            CpuAccessFlags = CpuAccessFlags.None,
            OptionFlags = ResourceOptionFlags.None,
        });

        // Single (non-array) NV12 scratch the VideoProcessor writes into reliably; copied per frame
        // into the encoder's pool slice. RenderTarget bind is what VideoProcessorBlt output needs.
        _scratchNv12 = new Texture2D(_device, new Texture2DDescription
        {
            Width = _outW,
            Height = _outH,
            MipLevels = 1,
            ArraySize = 1,
            Format = Format.NV12,
            SampleDescription = new SampleDescription(1, 0),
            Usage = ResourceUsage.Default,
            BindFlags = BindFlags.RenderTarget,
            CpuAccessFlags = CpuAccessFlags.None,
            OptionFlags = ResourceOptionFlags.None,
        });
        var ovd = new VideoProcessorOutputViewDescription { Dimension = VpovDimension.Texture2D };
        ovd.Texture2D.MipSlice = 0;
        _videoDevice.CreateVideoProcessorOutputView(_scratchNv12, _enumerator, ovd, out _scratchView);

        _stagingNv12 = new Texture2D(_device, new Texture2DDescription
        {
            Width = _outW,
            Height = _outH,
            MipLevels = 1,
            ArraySize = 1,
            Format = Format.NV12,
            SampleDescription = new SampleDescription(1, 0),
            Usage = ResourceUsage.Staging,
            BindFlags = BindFlags.None,
            CpuAccessFlags = CpuAccessFlags.Read,
            OptionFlags = ResourceOptionFlags.None,
        });

        Console.WriteLine($"[Bgra2Nv12] Pipeline {inW}x{inH} → {_outW}x{_outH} NV12.");
    }

    public void Dispose()
    {
        _scratchView?.Dispose();
        _scratchNv12?.Dispose();
        _stagingNv12?.Dispose();
        _processor?.Dispose();
        _enumerator?.Dispose();
        _inputCopy?.Dispose();
        _srcProbe?.Dispose();
        _nv12Probe?.Dispose();
        _videoContext?.Dispose();
        _videoDevice?.Dispose();
    }
}
