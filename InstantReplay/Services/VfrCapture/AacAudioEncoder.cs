using System;
using System.Runtime.InteropServices;
using FFmpeg.AutoGen;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Encodes captured PCM audio to AAC for the VFR mp4. Resamples the WASAPI capture format
/// (typically interleaved 48 kHz float) to the planar float AAC wants, buffers it into the codec's
/// fixed frame size, and emits <see cref="EncodedPacket"/>s on the same microsecond timeline as the
/// video — so the muxer interleaves them and audio stays in sync.
///
/// PTS is sample-accurate: audio is a strict CFR stream (sampleIndex / sampleRate) offset by the
/// engine clock at first sample, regardless of when WASAPI hands us each buffer. That keeps lips in
/// sync with the VFR video, whose PTS comes from the same engine clock.
/// </summary>
public sealed unsafe class AacAudioEncoder : IDisposable
{
    public const int OutSampleRate = 48000;
    public const int OutChannels = 2;

    private AVCodecContext* _ctx;
    private SwrContext* _swr;        // game/loopback → planar float
    private AVAudioFifo* _fifo;      // mixed master audio, drained to AAC
    private AVFrame* _frame;
    private AVPacket* _pkt;
    private long _writtenSamples;   // total samples pushed into the FIFO (incl. silence padding)
    private long _readSamples;      // total samples drained → the next frame's pts (in samples)
    private bool _disposed;
    private readonly object _writeLock = new();   // guards the master FIFO/timeline: loopback thread + pump timer

    // Microphone mix: a second resampler + its own FIFO, filled on the mic's WASAPI thread and
    // drained (mixed into the loopback samples) on the game thread — hence the lock.
    private SwrContext* _swrMic;
    private AVAudioFifo* _micFifo;
    private readonly object _micLock = new();
    private volatile bool _micReady;

    /// <summary>Compressed AAC packets, timestamped in engine-clock microseconds.</summary>
    public event Action<EncodedPacket>? PacketReady;

    public AacAudioEncoder(int inSampleRate, int inChannels, AVSampleFormat inFmt, int bitrateKbps)
    {
        var codec = ffmpeg.avcodec_find_encoder(AVCodecID.AV_CODEC_ID_AAC);
        if (codec == null) throw new InvalidOperationException("AAC encoder not found in avcodec.");

        _ctx = ffmpeg.avcodec_alloc_context3(codec);
        _ctx->sample_rate = OutSampleRate;
        _ctx->bit_rate = Math.Max(64, bitrateKbps) * 1000L;
        _ctx->sample_fmt = AVSampleFormat.AV_SAMPLE_FMT_FLTP;
        ffmpeg.av_channel_layout_default(&_ctx->ch_layout, OutChannels);
        _ctx->time_base = new AVRational { num = 1, den = OutSampleRate };
        // mp4 needs the AudioSpecificConfig in the stsd box → ask for global header extradata.
        _ctx->flags |= ffmpeg.AV_CODEC_FLAG_GLOBAL_HEADER;

        int r = ffmpeg.avcodec_open2(_ctx, codec, null);
        if (r < 0) throw new InvalidOperationException($"open aac: {FfmpegInterop.Err(r)}");

        // Resampler: capture format (interleaved) → planar float at our output rate/layout.
        var inLayout = new AVChannelLayout();
        ffmpeg.av_channel_layout_default(&inLayout, inChannels);
        SwrContext* swr = null;
        r = ffmpeg.swr_alloc_set_opts2(&swr,
            &_ctx->ch_layout, AVSampleFormat.AV_SAMPLE_FMT_FLTP, OutSampleRate,
            &inLayout, inFmt, inSampleRate, 0, null);
        if (r < 0 || swr == null) throw new InvalidOperationException($"swr alloc: {FfmpegInterop.Err(r)}");
        _swr = swr;
        r = ffmpeg.swr_init(_swr);
        if (r < 0) throw new InvalidOperationException($"swr init: {FfmpegInterop.Err(r)}");

        _fifo = ffmpeg.av_audio_fifo_alloc(AVSampleFormat.AV_SAMPLE_FMT_FLTP, OutChannels, 1);
        _micFifo = ffmpeg.av_audio_fifo_alloc(AVSampleFormat.AV_SAMPLE_FMT_FLTP, OutChannels, 1);
        _frame = ffmpeg.av_frame_alloc();
        _pkt = ffmpeg.av_packet_alloc();
        Console.WriteLine($"[AacAudioEncoder] AAC open: in {inSampleRate}Hz {inChannels}ch {inFmt} → 48kHz stereo, {bitrateKbps}kbps.");
    }

    /// <summary>Resamples one interleaved loopback/system PCM buffer at engine-time
    /// <paramref name="chunkEngineUs"/>, buffers it (padding any silence gap first, mic mixed in), and
    /// drains whole AAC frames.</summary>
    public void Encode(byte[] interleaved, int byteCount, int inSampleRate, int inChannels, int inBytesPerSample, long chunkEngineUs)
    {
        if (_disposed || byteCount <= 0) return;
        int inSamples = byteCount / (inBytesPerSample * inChannels);
        if (inSamples <= 0) return;

        fixed (byte* src = interleaved)
        {
            long outMax = ffmpeg.swr_get_out_samples(_swr, inSamples);
            if (outMax <= 0) return;

            byte** outData = stackalloc byte*[OutChannels];
            int planeBytes = (int)outMax * sizeof(float);
            byte* buf0 = (byte*)ffmpeg.av_malloc((ulong)planeBytes);
            byte* buf1 = (byte*)ffmpeg.av_malloc((ulong)planeBytes);
            outData[0] = buf0; outData[1] = buf1;
            try
            {
                byte*[] inData = { src };
                int got;
                fixed (byte** inPtr = inData)
                    got = ffmpeg.swr_convert(_swr, outData, (int)outMax, inPtr, inSamples);
                if (got > 0)
                {
                    lock (_writeLock)
                    {
                        PadTo(chunkEngineUs);          // fill the silence gap (mic mixed) up to this chunk
                        MixMicInto(outData, got);      // blend mic into the real loopback samples
                        ffmpeg.av_audio_fifo_write(_fifo, (void**)outData, got);
                        _writtenSamples += got;
                        DrainFrames();
                    }
                }
            }
            finally { ffmpeg.av_free(buf0); ffmpeg.av_free(buf1); }
        }
    }

    /// <summary>
    /// Advances the audio timeline to <paramref name="engineUs"/> with digital silence, even when NO
    /// loopback buffer is arriving (WASAPI loopback goes completely silent — emits no callbacks — when
    /// nothing is playing, e.g. desktop recording or a paused video). A steady engine-side timer calls
    /// this so audio keeps pace with the VFR video instead of ending at the last system sound; the mic
    /// is mixed into the padded silence so the player's voice is still heard during system quiet.
    /// </summary>
    public void PumpSilenceTo(long engineUs)
    {
        if (_disposed) return;
        lock (_writeLock)
        {
            PadTo(engineUs);
            DrainFrames();
        }
    }

    /// <summary>Pads the FIFO with silence (mic mixed in) up to engine-time <paramref name="targetUs"/>.
    /// Caller must hold <see cref="_writeLock"/>.</summary>
    private void PadTo(long targetUs)
    {
        long expected = targetUs * OutSampleRate / 1_000_000L;
        long gap = expected - _writtenSamples;
        if (gap > 0 && gap <= OutSampleRate * 5L) { WriteSilenceWithMic(gap); _writtenSamples += gap; }
    }

    /// <summary>Writes <paramref name="samples"/> of digital silence into the FIFO, mixing in any
    /// queued mic samples so the microphone stays audible while system audio is quiet. Lock held.</summary>
    private void WriteSilenceWithMic(long samples)
    {
        const int chunk = 4096;
        byte* z0 = (byte*)ffmpeg.av_malloc((ulong)(chunk * sizeof(float)));
        byte* z1 = (byte*)ffmpeg.av_malloc((ulong)(chunk * sizeof(float)));
        byte** planes = stackalloc byte*[OutChannels];
        planes[0] = z0; planes[1] = z1;
        try
        {
            long left = samples;
            while (left > 0)
            {
                int n = (int)Math.Min(chunk, left);
                // Re-zero each chunk (MixMicInto adds into the planes), then blend mic on top.
                new Span<byte>(z0, n * sizeof(float)).Clear();
                new Span<byte>(z1, n * sizeof(float)).Clear();
                MixMicInto(planes, n);
                ffmpeg.av_audio_fifo_write(_fifo, (void**)planes, n);
                left -= n;
            }
        }
        finally { ffmpeg.av_free(z0); ffmpeg.av_free(z1); }
    }

    /// <summary>Resamples one mic PCM buffer to 48 kHz stereo float and queues it for mixing. Runs
    /// on the mic's WASAPI thread; the backlog is capped so the mic can't drift far ahead.</summary>
    public void EncodeMic(byte[] interleaved, int byteCount, int inSampleRate, int inChannels, int inBytesPerSample, AVSampleFormat inFmt)
    {
        if (_disposed || byteCount <= 0) return;
        int inSamples = byteCount / (inBytesPerSample * inChannels);
        if (inSamples <= 0) return;
        if (!_micReady) InitMicSwr(inSampleRate, inChannels, inFmt);
        if (_swrMic == null) return;

        fixed (byte* src = interleaved)
        {
            long outMax = ffmpeg.swr_get_out_samples(_swrMic, inSamples);
            if (outMax <= 0) return;
            byte* b0 = (byte*)ffmpeg.av_malloc((ulong)(outMax * sizeof(float)));
            byte* b1 = (byte*)ffmpeg.av_malloc((ulong)(outMax * sizeof(float)));
            byte** outData = stackalloc byte*[OutChannels];
            outData[0] = b0; outData[1] = b1;
            try
            {
                byte*[] inData = { src };
                int got;
                fixed (byte** inPtr = inData)
                    got = ffmpeg.swr_convert(_swrMic, outData, (int)outMax, inPtr, inSamples);
                if (got > 0)
                    lock (_micLock)
                    {
                        int sz = ffmpeg.av_audio_fifo_size(_micFifo);
                        if (sz + got > OutSampleRate)   // keep <~1s backlog: drop oldest
                            ffmpeg.av_audio_fifo_drain(_micFifo, Math.Min(sz, sz + got - OutSampleRate));
                        ffmpeg.av_audio_fifo_write(_micFifo, (void**)outData, got);
                    }
            }
            finally { ffmpeg.av_free(b0); ffmpeg.av_free(b1); }
        }
    }

    private void InitMicSwr(int inSampleRate, int inChannels, AVSampleFormat inFmt)
    {
        var inLayout = new AVChannelLayout();
        ffmpeg.av_channel_layout_default(&inLayout, inChannels);
        SwrContext* swr = null;
        int r = ffmpeg.swr_alloc_set_opts2(&swr,
            &_ctx->ch_layout, AVSampleFormat.AV_SAMPLE_FMT_FLTP, OutSampleRate,
            &inLayout, inFmt, inSampleRate, 0, null);
        if (r < 0 || swr == null) { Console.WriteLine("[AacAudioEncoder] mic swr alloc failed."); return; }
        if (ffmpeg.swr_init(swr) < 0) { ffmpeg.swr_free(&swr); return; }
        _swrMic = swr; _micReady = true;
        Console.WriteLine($"[AacAudioEncoder] mic mix on: {inSampleRate}Hz {inChannels}ch {inFmt} → 48kHz stereo.");
    }

    /// <summary>Adds up to <paramref name="samples"/> queued mic samples into the loopback planes
    /// (clamped). Called on the game thread under the mic lock.</summary>
    private void MixMicInto(byte** dst, int samples)
    {
        if (!_micReady) return;
        lock (_micLock)
        {
            int avail = ffmpeg.av_audio_fifo_size(_micFifo);
            int mix = Math.Min(samples, avail);
            if (mix <= 0) return;

            byte* m0 = (byte*)ffmpeg.av_malloc((ulong)(mix * sizeof(float)));
            byte* m1 = (byte*)ffmpeg.av_malloc((ulong)(mix * sizeof(float)));
            byte** mp = stackalloc byte*[OutChannels];
            mp[0] = m0; mp[1] = m1;
            try
            {
                if (ffmpeg.av_audio_fifo_read(_micFifo, (void**)mp, mix) < mix) return;
                float* d0 = (float*)dst[0], d1 = (float*)dst[1];
                float* s0 = (float*)m0, s1 = (float*)m1;
                for (int i = 0; i < mix; i++)
                {
                    d0[i] = Clamp(d0[i] + s0[i]);
                    d1[i] = Clamp(d1[i] + s1[i]);
                }
            }
            finally { ffmpeg.av_free(m0); ffmpeg.av_free(m1); }
        }
    }

    private static float Clamp(float v) => v > 1f ? 1f : v < -1f ? -1f : v;

    private void DrainFrames()
    {
        int frameSize = _ctx->frame_size > 0 ? _ctx->frame_size : 1024;
        byte** planes = stackalloc byte*[OutChannels];
        while (ffmpeg.av_audio_fifo_size(_fifo) >= frameSize)
        {
            ffmpeg.av_frame_unref(_frame);
            _frame->nb_samples = frameSize;
            _frame->format = (int)AVSampleFormat.AV_SAMPLE_FMT_FLTP;
            ffmpeg.av_channel_layout_copy(&_frame->ch_layout, &_ctx->ch_layout);
            _frame->sample_rate = OutSampleRate;
            if (ffmpeg.av_frame_get_buffer(_frame, 0) < 0) break;

            // Read the FIFO straight into the freshly-allocated planar frame buffers.
            planes[0] = _frame->data[0];
            planes[1] = _frame->data[1];
            if (ffmpeg.av_audio_fifo_read(_fifo, (void**)planes, frameSize) < frameSize) break;

            // pts in the codec's own time_base (1/sample_rate) = sample position; Submit converts
            // the resulting packet pts to microseconds for the muxer.
            _frame->pts = _readSamples;
            _readSamples += frameSize;
            Submit(_frame);
        }
    }

    private void Submit(AVFrame* frame)
    {
        int sr = ffmpeg.avcodec_send_frame(_ctx, frame);
        if (sr < 0 && sr != ffmpeg.AVERROR(ffmpeg.EAGAIN)) return;
        while (true)
        {
            int rr = ffmpeg.avcodec_receive_packet(_ctx, _pkt);
            if (rr == ffmpeg.AVERROR(ffmpeg.EAGAIN) || rr == ffmpeg.AVERROR_EOF) break;
            if (rr < 0) break;
            var data = new byte[_pkt->size];
            Marshal.Copy((IntPtr)_pkt->data, data, 0, _pkt->size);
            // AAC packet pts is in 1/sample_rate units; convert to microseconds for the muxer.
            long ptsUs = _pkt->pts == ffmpeg.AV_NOPTS_VALUE
                ? 0 : _pkt->pts * 1_000_000L / OutSampleRate;
            PacketReady?.Invoke(new EncodedPacket(data, ptsUs, ptsUs, IsKeyframe: true));
            ffmpeg.av_packet_unref(_pkt);
        }
    }

    /// <summary>Stream parameters (codec id, sample rate, channels, ASC extradata) for the muxer.</summary>
    public StreamSpec GetStreamSpec()
    {
        byte[] extra = Array.Empty<byte>();
        if (_ctx != null && _ctx->extradata != null && _ctx->extradata_size > 0)
        {
            extra = new byte[_ctx->extradata_size];
            Marshal.Copy((IntPtr)_ctx->extradata, extra, 0, _ctx->extradata_size);
        }
        return new StreamSpec(AVCodecID.AV_CODEC_ID_AAC, IsVideo: false, 0, 0, OutSampleRate, OutChannels, extra);
    }

    public void Flush()
    {
        if (_disposed || _ctx == null) return;
        try { Submit(null); } catch { }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        fixed (AVPacket** p = &_pkt) if (_pkt != null) ffmpeg.av_packet_free(p);
        fixed (AVFrame** f = &_frame) if (_frame != null) ffmpeg.av_frame_free(f);
        if (_fifo != null) { ffmpeg.av_audio_fifo_free(_fifo); _fifo = null; }
        if (_micFifo != null) { ffmpeg.av_audio_fifo_free(_micFifo); _micFifo = null; }
        fixed (SwrContext** s = &_swr) if (_swr != null) ffmpeg.swr_free(s);
        fixed (SwrContext** sm = &_swrMic) if (_swrMic != null) ffmpeg.swr_free(sm);
        fixed (AVCodecContext** c = &_ctx) if (_ctx != null) ffmpeg.avcodec_free_context(c);
    }
}
