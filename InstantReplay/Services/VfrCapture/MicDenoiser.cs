using System;
using System.IO;
using System.Runtime.InteropServices;
using FFmpeg.AutoGen;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Real-time microphone noise suppression via FFmpeg's <c>arnndn</c> (RNNoise) filter, run through a
/// tiny libavfilter graph: <c>abuffer → arnndn → aformat → abuffersink</c>. Reuses our bundled FFmpeg
/// (avfilter-10) and the bundled BSD-licensed RNNoise model (<c>rnnoise/std.rnnn</c>) — no extra native
/// dependency. Feeds interleaved 32-bit-float mic buffers in and returns the denoised buffer out (same
/// format; the count may differ slightly because RNNoise buffers a 10 ms frame, so callers must handle a
/// variable/zero output size).
///
/// Single-threaded by design — one instance per mic capture thread (the recorder and the monitor each
/// own one). Never throws: a setup failure (or a missing model) leaves it disabled (pass-through).
/// </summary>
public sealed unsafe class MicDenoiser : IDisposable
{
    private AVFilterGraph* _graph;
    private AVFilterContext* _src;
    private AVFilterContext* _sink;
    private AVFrame* _inFrame;
    private AVFrame* _outFrame;

    private int _rate;
    private int _channels;
    private bool _ready;
    private bool _failed;
    private byte[] _out = Array.Empty<byte>();   // reused, growable output accumulator

    /// <summary>Absolute path to the bundled RNNoise model, copied next to the exe by the csproj
    /// (<c>rnnoise/std.rnnn</c>). Resolved once; missing file just disables the denoiser.</summary>
    public static string DefaultModelPath { get; } =
        Path.Combine(AppContext.BaseDirectory, "rnnoise", "std.rnnn");

    /// <summary>Turns suppression on/off. Off = pure pass-through (no graph work).</summary>
    public bool Enabled { get; set; }

    /// <summary>Denoises one interleaved float buffer. Returns true and sets <paramref name="outBuf"/>/
    /// <paramref name="outBytes"/> to the processed audio (may be 0 bytes while the FFT fills); returns
    /// false to mean "use the input unchanged" (disabled, wrong format, or a setup failure).</summary>
    public bool Process(byte[] interleaved, int byteCount, int sampleRate, int channels, out byte[] outBuf, out int outBytes)
    {
        outBuf = interleaved;
        outBytes = byteCount;
        if (!Enabled || _failed || byteCount <= 0) return false;

        if (!_ready || _rate != sampleRate || _channels != channels)
        {
            if (!Setup(sampleRate, channels)) { _failed = true; return false; }
        }

        int inSamples = byteCount / (channels * sizeof(float));
        if (inSamples <= 0) return false;

        try
        {
            ffmpeg.av_frame_unref(_inFrame);
            _inFrame->format = (int)AVSampleFormat.AV_SAMPLE_FMT_FLT;
            _inFrame->sample_rate = sampleRate;
            _inFrame->nb_samples = inSamples;
            ffmpeg.av_channel_layout_default(&_inFrame->ch_layout, channels);
            if (ffmpeg.av_frame_get_buffer(_inFrame, 0) < 0) return false;

            int bytes = inSamples * channels * sizeof(float);
            Marshal.Copy(interleaved, 0, (IntPtr)_inFrame->data[0], bytes);

            if (ffmpeg.av_buffersrc_add_frame(_src, _inFrame) < 0) return false;

            int total = 0;
            while (true)
            {
                int r = ffmpeg.av_buffersink_get_frame(_sink, _outFrame);
                if (r == ffmpeg.AVERROR(ffmpeg.EAGAIN) || r == ffmpeg.AVERROR_EOF) break;
                if (r < 0) break;

                int n = _outFrame->nb_samples * channels * sizeof(float);
                EnsureCapacity(total + n);
                Marshal.Copy((IntPtr)_outFrame->data[0], _out, total, n);
                total += n;
                ffmpeg.av_frame_unref(_outFrame);
            }

            outBuf = _out;
            outBytes = total;
            return true;
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[MicDenoiser] process error: {ex.Message}");
            _failed = true;
            return false;
        }
    }

    private bool Setup(int rate, int channels)
    {
        Teardown();
        try
        {
            if (!File.Exists(DefaultModelPath))
            {
                Console.WriteLine($"[MicDenoiser] RNNoise model not found — suppression disabled: {DefaultModelPath}");
                return false;
            }

            _graph = ffmpeg.avfilter_graph_alloc();
            if (_graph == null) return false;

            string layout = LayoutString(channels);
            string srcArgs = $"time_base=1/{rate}:sample_rate={rate}:sample_fmt=flt:channel_layout={layout}";

            AVFilterContext* denoise, fmt;
            if (CreateFilter(out _src, "abuffer", "in", srcArgs) < 0) return false;
            if (CreateArnndn(out denoise, DefaultModelPath) < 0) return false;
            if (CreateFilter(out fmt, "aformat", "fmt", $"sample_fmts=flt:sample_rates={rate}:channel_layouts={layout}") < 0) return false;
            if (CreateFilter(out _sink, "abuffersink", "out", null) < 0) return false;

            if (ffmpeg.avfilter_link(_src, 0, denoise, 0) < 0) return false;
            if (ffmpeg.avfilter_link(denoise, 0, fmt, 0) < 0) return false;
            if (ffmpeg.avfilter_link(fmt, 0, _sink, 0) < 0) return false;
            if (ffmpeg.avfilter_graph_config(_graph, null) < 0) return false;

            _inFrame = ffmpeg.av_frame_alloc();
            _outFrame = ffmpeg.av_frame_alloc();
            _rate = rate;
            _channels = channels;
            _ready = true;
            Console.WriteLine($"[MicDenoiser] arnndn (RNNoise) graph ready ({rate}Hz {channels}ch).");
            return true;
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[MicDenoiser] setup failed: {ex.Message}");
            return false;
        }
    }

    /// <summary>Builds the <c>arnndn</c> filter in two steps (alloc → set option → init) so the model
    /// path is set via <c>av_opt_set</c> rather than the colon-delimited filter-args string — a Windows
    /// path like <c>C:\…\std.rnnn</c> would otherwise be split on its drive-letter colon.</summary>
    private int CreateArnndn(out AVFilterContext* ctx, string modelPath)
    {
        ctx = null;
        var filter = ffmpeg.avfilter_get_by_name("arnndn");
        if (filter == null) { Console.WriteLine("[MicDenoiser] filter 'arnndn' missing."); return -1; }

        AVFilterContext* c = ffmpeg.avfilter_graph_alloc_filter(_graph, filter, "denoise");
        if (c == null) { Console.WriteLine("[MicDenoiser] alloc 'arnndn' failed."); return -1; }

        int r = ffmpeg.av_opt_set(c, "model", modelPath, ffmpeg.AV_OPT_SEARCH_CHILDREN);
        if (r < 0) { Console.WriteLine($"[MicDenoiser] set arnndn model: {FfmpegInterop.Err(r)}"); return r; }

        r = ffmpeg.avfilter_init_str(c, null);
        if (r < 0) { Console.WriteLine($"[MicDenoiser] init arnndn: {FfmpegInterop.Err(r)}"); return r; }

        ctx = c;
        return 0;
    }

    private int CreateFilter(out AVFilterContext* ctx, string name, string instance, string? args)
    {
        AVFilterContext* c = null;
        var filter = ffmpeg.avfilter_get_by_name(name);
        if (filter == null) { ctx = null; Console.WriteLine($"[MicDenoiser] filter '{name}' missing."); return -1; }
        int r = ffmpeg.avfilter_graph_create_filter(&c, filter, instance, args, null, _graph);
        ctx = c;
        if (r < 0) Console.WriteLine($"[MicDenoiser] create '{name}': {FfmpegInterop.Err(r)}");
        return r;
    }

    private static string LayoutString(int channels)
    {
        try
        {
            AVChannelLayout layout = default;
            ffmpeg.av_channel_layout_default(&layout, channels);
            byte* buf = stackalloc byte[128];
            ffmpeg.av_channel_layout_describe(&layout, buf, 128);
            ffmpeg.av_channel_layout_uninit(&layout);
            string? s = Marshal.PtrToStringAnsi((IntPtr)buf);
            if (!string.IsNullOrEmpty(s)) return s;
        }
        catch { /* fall through to the simple map */ }
        return channels == 1 ? "mono" : "stereo";
    }

    private void EnsureCapacity(int needed)
    {
        if (_out.Length >= needed) return;
        int size = _out.Length == 0 ? 8192 : _out.Length;
        while (size < needed) size *= 2;
        var bigger = new byte[size];
        Buffer.BlockCopy(_out, 0, bigger, 0, _out.Length);
        _out = bigger;
    }

    private void Teardown()
    {
        _ready = false;
        fixed (AVFrame** f = &_inFrame) if (_inFrame != null) ffmpeg.av_frame_free(f);
        fixed (AVFrame** f = &_outFrame) if (_outFrame != null) ffmpeg.av_frame_free(f);
        fixed (AVFilterGraph** g = &_graph) if (_graph != null) ffmpeg.avfilter_graph_free(g);
        _src = null;
        _sink = null;
    }

    public void Dispose() => Teardown();
}
