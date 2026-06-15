using System;
using System.Runtime.InteropServices;
using FFmpeg.AutoGen;
using SharpDX.Direct3D11;

namespace Lag.Services.VfrCapture;

/// <summary>One encoded, compressed frame ready for the ring buffer / muxer. Data is a private
/// copy so the encoder's internal packet can be reused immediately.</summary>
public sealed record EncodedPacket(byte[] Data, long PtsUs, long DtsUs, bool IsKeyframe);

/// <summary>
/// Variable-frame-rate hardware video encoder. Wraps the app's shared D3D11 device into an FFmpeg
/// hardware context, builds an NV12 D3D11 frame pool, and runs the user's selected encoder
/// (AV1/HEVC/H.264 on NVIDIA/AMD, or CPU x264) against it — every frame stays on the GPU.
///
/// VFR: each frame is submitted with its REAL capture timestamp (microseconds), so we encode only
/// the frames the game actually produced, at their true cadence — no CFR duplicate-frame padding.
///
/// Convert + submit run on the caller's capture thread (single-threaded D3D context use — no
/// cross-thread contention). Packets are drained opportunistically (receive-after-send, not
/// drain-to-empty) so the hardware encoder pipelines rather than being measured at its latency.
/// </summary>
public sealed unsafe class NvencVfrEncoder : IDisposable
{
    private readonly Bgra2Nv12Converter _converter;
    private readonly EncoderChoice _choice;

    private AVBufferRef* _hwDevice;
    private AVBufferRef* _hwFrames;
    private AVCodecContext* _ctx;
    private AVPacket* _pkt;
    private AVFrame* _frame;
    private bool _disposed;

    /// <summary>Raised for every compressed packet the encoder emits (on the capture thread).</summary>
    public event Action<EncodedPacket>? PacketReady;

    public int Width { get; }
    public int Height { get; }
    public EncoderChoice Choice => _choice;

    /// <summary>Diagnostics: cumulative microseconds in colour-convert vs encode submit + frame count.</summary>
    public long ConvertUs, EncodeUs, TimedFrames;

    /// <summary>Microsecond time base — fine enough to carry exact present times for VFR.</summary>
    public const int TimeBaseDen = 1_000_000;

    private readonly int _fpsHint;
    private readonly bool _hwMode;   // true = D3D11 hwframe (vendor-matched GPU); false = system NV12

    public NvencVfrEncoder(D3D11Context d3d, EncoderChoice choice, int width, int height,
                           int bitrateKbps, int preset /*1=fastest..7=slowest*/, int fpsHint = 120)
    {
        _choice = choice;
        Width = width & ~1;
        Height = height & ~1;
        _fpsHint = fpsHint > 0 ? fpsHint : 120;
        // A hardware encoder can only read our D3D11 textures if it's the SAME GPU vendor as the
        // capture device. Otherwise (x264, or AMF/QSV on a different GPU than capture) fall back to
        // the system-memory path: download NV12 to RAM and let FFmpeg feed the encoder. Universal,
        // a bit slower, never produces garbage.
        _hwMode = IsVendorMatched(choice.FfmpegName, d3d.VendorId);
        Console.WriteLine($"[NvencVfrEncoder] {choice.FriendlyName}: {(_hwMode ? "D3D11 hardware" : "system-memory")} path (capture GPU vendor 0x{d3d.VendorId:X4}).");
        _converter = new Bgra2Nv12Converter(d3d, Width, Height);
        try
        {
            if (_hwMode) SetupHardware(d3d);
            OpenEncoder(bitrateKbps, preset);
            _pkt = ffmpeg.av_packet_alloc();
            _frame = ffmpeg.av_frame_alloc();
        }
        catch { Dispose(); throw; }
    }

    private static bool IsVendorMatched(string name, int vendorId) =>
        (name.Contains("nvenc") && vendorId == 0x10DE) ||
        (name.Contains("amf")   && vendorId == 0x1002) ||
        (name.Contains("qsv")   && vendorId == 0x8086);

    private void SetupHardware(D3D11Context d3d)
    {
        _hwDevice = ffmpeg.av_hwdevice_ctx_alloc(AVHWDeviceType.AV_HWDEVICE_TYPE_D3D11VA);
        if (_hwDevice == null) throw new InvalidOperationException("av_hwdevice_ctx_alloc failed.");
        var devCtx = (AVHWDeviceContext*)_hwDevice->data;
        var d3dDevCtx = (AVD3D11VADeviceContext*)devCtx->hwctx;
        Marshal.AddRef(d3d.Device.NativePointer); // ffmpeg releases on teardown
        d3dDevCtx->device = (ID3D11Device*)d3d.Device.NativePointer;
        // Bind our SHARED immediate context too, so any work ffmpeg does on the device runs on the
        // same context as our colour converter — GPU-ordered, no cross-context race. (ffmpeg would
        // otherwise GetImmediateContext() the same object; we make the sharing explicit.) The device
        // is already multithread-protected (D3D11Context), so ffmpeg's default lock is enough.
        Marshal.AddRef(d3d.Context.NativePointer); // ffmpeg releases on teardown
        d3dDevCtx->device_context = (ID3D11DeviceContext*)d3d.Context.NativePointer;
        int r = ffmpeg.av_hwdevice_ctx_init(_hwDevice);
        if (r < 0) throw new InvalidOperationException($"hwdevice init: {FfmpegInterop.Err(r)}");

        _hwFrames = ffmpeg.av_hwframe_ctx_alloc(_hwDevice);
        if (_hwFrames == null) throw new InvalidOperationException("av_hwframe_ctx_alloc failed.");
        var fc = (AVHWFramesContext*)_hwFrames->data;
        fc->format = AVPixelFormat.AV_PIX_FMT_D3D11;
        fc->sw_format = AVPixelFormat.AV_PIX_FMT_NV12;
        fc->width = Width;
        fc->height = Height;
        fc->initial_pool_size = 16;
        var d3dFrames = (AVD3D11VAFramesContext*)fc->hwctx;
        d3dFrames->BindFlags = (uint)(BindFlags.RenderTarget | BindFlags.ShaderResource);
        r = ffmpeg.av_hwframe_ctx_init(_hwFrames);
        if (r < 0) throw new InvalidOperationException($"hwframes init: {FfmpegInterop.Err(r)}");
    }

    private void OpenEncoder(int bitrateKbps, int preset)
    {
        var codec = ffmpeg.avcodec_find_encoder_by_name(_choice.FfmpegName);
        if (codec == null) throw new InvalidOperationException($"encoder {_choice.FfmpegName} vanished.");

        _ctx = ffmpeg.avcodec_alloc_context3(codec);
        _ctx->width = Width;
        _ctx->height = Height;
        _ctx->pix_fmt = _hwMode ? AVPixelFormat.AV_PIX_FMT_D3D11 : AVPixelFormat.AV_PIX_FMT_NV12;
        // Colour metadata so players decode exactly what the converter produced (full-range BT.709).
        // Missing/mismatched tags were why the picture looked darker / colour-shifted.
        _ctx->colorspace = AVColorSpace.AVCOL_SPC_BT709;
        _ctx->color_range = AVColorRange.AVCOL_RANGE_MPEG;          // limited/tv (16-235) — like Medal
        _ctx->color_primaries = AVColorPrimaries.AVCOL_PRI_BT709;
        _ctx->color_trc = AVColorTransferCharacteristic.AVCOL_TRC_BT709;
        _ctx->time_base = new AVRational { num = 1, den = TimeBaseDen };
        // framerate is a HINT for the rate controller only (CBR splits bit_rate across fps); the
        // real timing stays VFR via per-frame microsecond PTS. Leaving it 0 made NVENC's CBR think
        // there were 0 fps and starve the bitrate to a fraction of the target.
        _ctx->framerate = new AVRational { num = _fpsHint, den = 1 };
        long bps = Math.Max(1, bitrateKbps) * 1000L;
        _ctx->bit_rate = bps;
        _ctx->rc_max_rate = bps;            // hard ceiling so the in-RAM buffer stays bounded
        _ctx->rc_buffer_size = (int)(bps * 2);
        _ctx->gop_size = 240;
        _ctx->max_b_frames = 0;
        if (_hwMode) _ctx->hw_frames_ctx = ffmpeg.av_buffer_ref(_hwFrames);

        ApplyCodecOptions(preset);

        int r = ffmpeg.avcodec_open2(_ctx, codec, null);
        if (r < 0) throw new InvalidOperationException($"open {_choice.FfmpegName}: {FfmpegInterop.Err(r)}");
        Console.WriteLine($"[NvencVfrEncoder] {_choice.FriendlyName} open @ {Width}x{Height}, target {bitrateKbps}kbps, ctx.bit_rate={_ctx->bit_rate}, rc_max={_ctx->rc_max_rate}, VFR.");
    }

    /// <summary>Per-vendor knobs, tuned for SPEED under game load (the encoder shares the GPU with
    /// the game, so quality features that cost latency are off — high bitrate carries quality).</summary>
    private void ApplyCodecOptions(int preset)
    {
        string name = _choice.FfmpegName;
        void Opt(string k, string v)
        {
            int r = ffmpeg.av_opt_set(_ctx->priv_data, k, v, 0);
            if (r < 0) Console.WriteLine($"[NvencVfrEncoder] opt '{k}={v}' rejected: {FfmpegInterop.Err(r)}");
        }

        if (name.Contains("nvenc"))
        {
            Opt("preset", $"p{Math.Clamp(preset, 1, 7)}");
            Opt("tune", "hq");          // high-quality tune (recording) — NOT "ll", which starves bits
            // CBR (Medal-style): always FILLS the target bitrate. VBR+cq starved AV1 to ~1.5 Mbps
            // (cq is a quality target and AV1 is so efficient it sat far below the ceiling). CBR puts
            // AV1's efficiency into quality at a stable, predictable size.
            Opt("rc", "cbr");
            Opt("multipass", "qres");   // 2-pass (quarter-res): clearly better at a fixed bitrate, cheap
            Opt("rc-lookahead", "0");
            Opt("spatial-aq", "1");     // adaptive quantisation — sharpens detail and motion
        }
        else if (name.Contains("amf"))
        {
            Opt("quality", "speed");
            Opt("rc", "vbr_peak");
            Opt("preanalysis", "false");
        }
        else if (name.Contains("qsv")) Opt("preset", "veryfast");
        else if (name == "libx264") { Opt("preset", "veryfast"); Opt("tune", "zerolatency"); }
    }

    /// <summary>
    /// Converts and submits one captured BGRA frame at its real present time (microseconds), then
    /// drains whatever packets are ready (without waiting for this frame's own packet — that keeps
    /// the NVENC pipeline full instead of measuring its per-frame latency).
    /// </summary>
    public bool Encode(Texture2D bgraFrame, long ptsUs)
    {
        if (_disposed) return false;
        try
        {
            long t0 = System.Diagnostics.Stopwatch.GetTimestamp();
            if (!(_hwMode ? ConvertHardware(bgraFrame) : ConvertSystem(bgraFrame))) return true;
            long t1 = System.Diagnostics.Stopwatch.GetTimestamp();

            _frame->pts = ptsUs;
            int sr = ffmpeg.avcodec_send_frame(_ctx, _frame);
            ffmpeg.av_frame_unref(_frame);
            if (sr < 0 && sr != ffmpeg.AVERROR(ffmpeg.EAGAIN))
            { Console.WriteLine($"[NvencVfrEncoder] send: {FfmpegInterop.Err(sr)}"); return false; }

            DrainReady();
            long t2 = System.Diagnostics.Stopwatch.GetTimestamp();
            long freq = System.Diagnostics.Stopwatch.Frequency;
            ConvertUs += (t1 - t0) * 1_000_000 / freq;
            EncodeUs += (t2 - t1) * 1_000_000 / freq;
            TimedFrames++;
            return true;
        }
        catch (Exception ex) { Console.WriteLine($"[NvencVfrEncoder] Encode error: {ex.Message}"); return false; }
    }

    /// <summary>Hardware path: pull a pool slice and convert straight into it (zero CPU copy).
    /// Returns false to skip this frame (pool momentarily exhausted).</summary>
    private bool ConvertHardware(Texture2D bgraFrame)
    {
        int gb = ffmpeg.av_hwframe_get_buffer(_hwFrames, _frame, 0);
        if (gb < 0) { Console.WriteLine($"[NvencVfrEncoder] get_buffer: {FfmpegInterop.Err(gb)}"); return false; }

        var arrayTexPtr = (IntPtr)_frame->data[0];
        int slice = (int)_frame->data[1];
        Marshal.AddRef(arrayTexPtr);
        using (var arrayTex = new Texture2D(arrayTexPtr))
            _converter.ConvertInto(bgraFrame, arrayTex, slice);
        return true;
    }

    /// <summary>System-memory path: convert + download NV12 into a RAM frame the encoder can read
    /// (x264, or a hardware encoder on a different GPU than capture).</summary>
    private bool ConvertSystem(Texture2D bgraFrame)
    {
        ffmpeg.av_frame_unref(_frame);
        _frame->format = (int)AVPixelFormat.AV_PIX_FMT_NV12;
        _frame->width = Width;
        _frame->height = Height;
        if (ffmpeg.av_frame_get_buffer(_frame, 32) < 0) return false;

        var box = _converter.MapSystemNv12(bgraFrame);
        try
        {
            byte* ySrc = (byte*)box.DataPointer;
            byte* yDst = _frame->data[0];
            int yPitch = _frame->linesize[0];
            for (int r = 0; r < Height; r++)
                System.Buffer.MemoryCopy(ySrc + (long)r * box.RowPitch, yDst + (long)r * yPitch, Width, Width);

            byte* uvSrc = ySrc + (long)box.RowPitch * Height;   // UV plane follows Y in the NV12 staging
            byte* uvDst = _frame->data[1];
            int uvPitch = _frame->linesize[1];
            for (int r = 0; r < Height / 2; r++)
                System.Buffer.MemoryCopy(uvSrc + (long)r * box.RowPitch, uvDst + (long)r * uvPitch, Width, Width);
        }
        finally { _converter.UnmapSystemNv12(); }
        return true;
    }

    private void DrainReady()
    {
        while (true)
        {
            int rr = ffmpeg.avcodec_receive_packet(_ctx, _pkt);
            if (rr == ffmpeg.AVERROR(ffmpeg.EAGAIN) || rr == ffmpeg.AVERROR_EOF) break;
            if (rr < 0) { Console.WriteLine($"[NvencVfrEncoder] receive: {FfmpegInterop.Err(rr)}"); break; }

            var data = new byte[_pkt->size];
            Marshal.Copy((IntPtr)_pkt->data, data, 0, _pkt->size);
            bool key = (_pkt->flags & ffmpeg.AV_PKT_FLAG_KEY) != 0;
            long dts = _pkt->dts == ffmpeg.AV_NOPTS_VALUE ? _pkt->pts : _pkt->dts;
            PacketReady?.Invoke(new EncodedPacket(data, _pkt->pts, dts, key));
            ffmpeg.av_packet_unref(_pkt);
        }
    }

    /// <summary>Stream parameters (codec id, dimensions, extradata) for the muxer.</summary>
    public StreamSpec GetStreamSpec()
    {
        byte[] extra = Array.Empty<byte>();
        if (_ctx != null && _ctx->extradata != null && _ctx->extradata_size > 0)
        {
            extra = new byte[_ctx->extradata_size];
            Marshal.Copy((IntPtr)_ctx->extradata, extra, 0, _ctx->extradata_size);
        }
        return new StreamSpec(_choice.CodecId, IsVideo: true, Width, Height, 0, 0, extra);
    }

    public void Flush()
    {
        if (_disposed || _ctx == null) return;
        try { ffmpeg.avcodec_send_frame(_ctx, null); DrainReady(); } catch { }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        fixed (AVPacket** p = &_pkt) if (_pkt != null) ffmpeg.av_packet_free(p);
        fixed (AVFrame** f = &_frame) if (_frame != null) ffmpeg.av_frame_free(f);
        fixed (AVCodecContext** c = &_ctx) if (_ctx != null) ffmpeg.avcodec_free_context(c);
        fixed (AVBufferRef** hf = &_hwFrames) if (_hwFrames != null) ffmpeg.av_buffer_unref(hf);
        fixed (AVBufferRef** hd = &_hwDevice) if (_hwDevice != null) ffmpeg.av_buffer_unref(hd);
        _converter.Dispose();
    }
}
