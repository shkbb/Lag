using System;
using System.Runtime.InteropServices;
using Lag.Services.VfrCapture;
using NAudio.CoreAudioApi;
using NAudio.Wave;
using NAudio.Wave.SampleProviders;

namespace Lag.Services;

/// <summary>
/// A lightweight, Settings-only live audio meter. While the Audio tab is open it opens the SELECTED
/// microphone (WASAPI shared capture) and the SELECTED output device (WASAPI loopback) purely to
/// MEASURE their levels — it records nothing and feeds nothing into clips. Each callback accumulates
/// the loudest sample; <see cref="ReadMicPeak"/> / <see cref="ReadOutputPeak"/> return that peak and
/// reset it, so the UI samples a fresh peak each frame and applies its own attack/release smoothing.
/// </summary>
public sealed class AudioMonitor : IDisposable
{
    private WasapiCapture? _mic;
    private WasapiLoopbackCapture? _output;
    private float _micPeak;
    private float _outPeak;

    // "Hear yourself": plays the captured mic back to a chosen output device. Active only while the
    // toggle is on; null otherwise. The mic callback feeds _monitorBuffer; _monitorOut drains it.
    private WasapiOut? _monitorOut;
    private BufferedWaveProvider? _monitorBuffer;

    // Noise gate driving BOTH the UI preview (marker/colour) and the audible cut in "hear yourself",
    // so what you see on the bar is what you hear — exactly while tuning.
    private readonly NoiseGate _gate = new();
    private float _gateThreshold = 0.08f;
    private bool _gateOpen;
    private float _lastMonGain = 1f;

    // Noise suppression (RNNoise/arnndn) for the "hear yourself" feed, so the preview sounds like the
    // recording will. Applied after the gate; disabled = pass-through. Output size varies (frame buffer).
    private readonly MicDenoiser _denoiser = new();

    /// <summary>The gate threshold currently in effect (0..1 perceptual) — for the UI marker.</summary>
    public float GateThreshold => _gateThreshold;

    /// <summary>Whether the live mic is currently passing the gate — for the bar colour.</summary>
    public bool GateOpen => _gateOpen;

    /// <summary>Configures the gate (auto-track the floor vs a fixed threshold). Applies live to the
    /// "hear yourself" monitor.</summary>
    public void SetInputGate(bool auto, float manualThreshold)
    {
        _gate.Auto = auto;
        _gate.ManualThreshold = manualThreshold;
    }

    /// <summary>Noise suppression (RNNoise): on = run the "hear yourself" feed through arnndn after the
    /// gate, so the live preview matches what the recording will sound like.</summary>
    public void SetNoiseSuppression(bool on) => _denoiser.Enabled = on;

    /// <summary>Loudest mic sample (0..1) since the last call; resets the accumulator.</summary>
    public float ReadMicPeak() { float p = _micPeak; _micPeak = 0; return p; }

    /// <summary>Loudest output sample (0..1) since the last call; resets the accumulator.</summary>
    public float ReadOutputPeak() { float p = _outPeak; _outPeak = 0; return p; }

    /// <summary>(Re)opens the chosen mic + output for metering. Safe to call repeatedly; each call
    /// tears down the previous captures first. Never throws — a missing device just leaves that meter
    /// at zero.</summary>
    public void Start(string? micDeviceId, string? outputDeviceId)
    {
        Stop();
        try { StartMic(micDeviceId); } catch (Exception ex) { Log("mic", ex); }
        try { StartOutput(outputDeviceId); } catch (Exception ex) { Log("output", ex); }
    }

    private void StartMic(string? id)
    {
        using var en = new MMDeviceEnumerator();
        MMDevice? dev = SafeGet(en, id);
        if (dev == null)
        {
            try { dev = en.GetDefaultAudioEndpoint(DataFlow.Capture, Role.Communications); }
            catch { return; }   // no microphone on this machine
        }
        _mic = new WasapiCapture(dev);
        var fmt = _mic.WaveFormat;
        _mic.DataAvailable += (_, e) =>
        {
            float rawPeak = PeakLevel(e.Buffer, e.BytesRecorded, fmt);
            if (rawPeak > _micPeak) _micPeak = rawPeak;   // meter shows the RAW input level

            // Run the gate every buffer so the marker/colour are live AND the monitor is cut audibly.
            double dt = fmt.AverageBytesPerSecond > 0 ? (double)e.BytesRecorded / fmt.AverageBytesPerSecond : 0.01;
            float gain = _gate.Process(Curve(rawPeak), dt);
            _gateThreshold = _gate.Threshold;
            _gateOpen = _gate.IsOpen;

            if (_monitorBuffer != null)   // "hear yourself" on → feed the GATED + denoised mic
            {
                byte[] outBuf = e.Buffer;
                int outBytes = e.BytesRecorded;
                if ((gain < 0.999f || _lastMonGain < 0.999f) && fmt.Encoding == WaveFormatEncoding.IeeeFloat)
                {
                    outBuf = new byte[e.BytesRecorded];
                    Buffer.BlockCopy(e.Buffer, 0, outBuf, 0, e.BytesRecorded);
                    ApplyGateRamp(outBuf, e.BytesRecorded, _lastMonGain, gain);
                }
                // RNNoise after the gate — so you hear the suppression too. Size varies / may be 0 while
                // the first frame fills (drop those — a few ms of start-up latency, no drift).
                if (_denoiser.Enabled && fmt.Encoding == WaveFormatEncoding.IeeeFloat
                    && _denoiser.Process(outBuf, outBytes, fmt.SampleRate, fmt.Channels, out var dbuf, out int dbytes))
                {
                    outBuf = dbuf;
                    outBytes = dbytes;
                }
                if (outBytes > 0) _monitorBuffer.AddSamples(outBuf, 0, outBytes);
            }
            _lastMonGain = gain;
        };
        _mic.StartRecording();
    }

    private void StartOutput(string? id)
    {
        using var en = new MMDeviceEnumerator();
        MMDevice dev = SafeGet(en, id) ?? en.GetDefaultAudioEndpoint(DataFlow.Render, Role.Multimedia);
        _output = new WasapiLoopbackCapture(dev);
        var fmt = _output.WaveFormat;
        _output.DataAvailable += (_, e) =>
        {
            float p = PeakLevel(e.Buffer, e.BytesRecorded, fmt);
            if (p > _outPeak) _outPeak = p;
        };
        _output.StartRecording();
    }

    private static MMDevice? SafeGet(MMDeviceEnumerator en, string? id)
    {
        if (string.IsNullOrEmpty(id)) return null;
        try { return en.GetDevice(id); } catch { return null; }
    }

    /// <summary>Turns "hear yourself" on/off: routes the live mic to <paramref name="outputDeviceId"/>
    /// (resampled to that device's format). Settings-only monitoring — nothing is recorded. Needs the
    /// mic already open (via <see cref="Start"/>). Never throws.</summary>
    public void SetMonitoring(bool on, string? outputDeviceId)
    {
        StopMonitoring();
        if (!on || _mic == null) return;
        try
        {
            using var en = new MMDeviceEnumerator();
            MMDevice outDev = SafeGet(en, outputDeviceId) ?? en.GetDefaultAudioEndpoint(DataFlow.Render, Role.Multimedia);

            // Push model (useEventSync = false): the render thread proactively keeps the device fed,
            // which is far steadier for a live mic loop than tight event-sync pulls that underrun.
            _monitorOut = new WasapiOut(outDev, AudioClientShareMode.Shared, false, 100);
            var outFmt = _monitorOut.OutputWaveFormat;

            // ReadFully → on underrun the buffer returns silence padding instead of a short read that
            // makes WasapiOut stutter; the roomy capacity absorbs capture/render clock jitter.
            _monitorBuffer = new BufferedWaveProvider(_mic.WaveFormat)
            {
                ReadFully = true,
                DiscardOnBufferOverflow = true,
                BufferDuration = TimeSpan.FromMilliseconds(500),
            };

            // Stream the mic to the output format. WDL is a real-time-friendly resampler (the previous
            // MediaFoundationResampler glitched/cut out in this live loop); then match the channel count.
            ISampleProvider sample = _monitorBuffer.ToSampleProvider();
            if (sample.WaveFormat.SampleRate != outFmt.SampleRate)
                sample = new WdlResamplingSampleProvider(sample, outFmt.SampleRate);
            sample = MatchChannels(sample, outFmt.Channels);

            _monitorOut.Init(sample);
            _monitorOut.Play();
        }
        catch (Exception ex) { Log("monitor", ex); StopMonitoring(); }
    }

    /// <summary>Up/down-mixes a sample stream to <paramref name="targetChannels"/> (mono↔stereo).</summary>
    private static ISampleProvider MatchChannels(ISampleProvider src, int targetChannels)
    {
        if (src.WaveFormat.Channels == targetChannels) return src;
        if (src.WaveFormat.Channels == 1 && targetChannels == 2) return new MonoToStereoSampleProvider(src);
        if (src.WaveFormat.Channels == 2 && targetChannels == 1) return new StereoToMonoSampleProvider(src);
        return src;   // uncommon layout — best effort
    }

    private void StopMonitoring()
    {
        try { _monitorOut?.Stop(); } catch { }
        try { _monitorOut?.Dispose(); } catch { }
        _monitorOut = null;
        _monitorBuffer = null;
    }

    public void Stop()
    {
        StopMonitoring();
        StopCapture(_mic); _mic = null;        // stops the mic callback before the denoiser graph dies
        StopCapture(_output); _output = null;
        _denoiser.Dispose();                   // freed here; rebuilt lazily on the next enabled buffer
        _micPeak = 0;
        _outPeak = 0;
    }

    private static void StopCapture(IWaveIn? cap)
    {
        if (cap == null) return;
        try { cap.StopRecording(); } catch { }
        try { cap.Dispose(); } catch { }
    }

    public void Dispose() => Stop();

    /// <summary>Peak absolute sample in the buffer, 0..1. Handles 32-bit float (the usual WASAPI mix
    /// format) and 16-bit PCM.</summary>
    private static float PeakLevel(byte[] buf, int bytes, WaveFormat fmt)
    {
        float peak = 0;
        if (fmt.Encoding == WaveFormatEncoding.IeeeFloat && fmt.BitsPerSample == 32)
        {
            for (int i = 0; i + 4 <= bytes; i += 4)
            {
                float s = Math.Abs(BitConverter.ToSingle(buf, i));
                if (s > peak) peak = s;
            }
        }
        else if (fmt.Encoding == WaveFormatEncoding.Pcm && fmt.BitsPerSample == 16)
        {
            for (int i = 0; i + 2 <= bytes; i += 2)
            {
                float s = Math.Abs(BitConverter.ToInt16(buf, i) / 32768f);
                if (s > peak) peak = s;
            }
        }
        return peak > 1f ? 1f : peak;
    }

    private static float Curve(float level) => (float)Math.Pow(level, 0.6);

    /// <summary>Multiplies the float buffer by a gain that ramps linearly from <paramref name="fromGain"/>
    /// to <paramref name="toGain"/> across it — a click-free gate transition for the monitor feed.</summary>
    private static void ApplyGateRamp(byte[] buf, int count, float fromGain, float toGain)
    {
        int n = count / sizeof(float);
        if (n == 0) return;
        var span = MemoryMarshal.Cast<byte, float>(buf.AsSpan(0, n * sizeof(float)));
        for (int i = 0; i < span.Length; i++)
            span[i] *= fromGain + (toGain - fromGain) * (i / (float)span.Length);
    }

    private static void Log(string what, Exception ex) =>
        Console.WriteLine($"[AudioMonitor] {what} meter unavailable: {ex.Message}");
}
