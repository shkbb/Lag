using System;
using System.Collections.Generic;
using System.Linq;
using System.Runtime.InteropServices;
using NAudio.CoreAudioApi;
using NAudio.Wave;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Captures the game/system audio AND (optionally) a microphone for the VFR engine via WASAPI, and
/// raises raw PCM buffers as they arrive. Game audio is the default render endpoint in loopback —
/// the simple, robust path (everything you hear); the mic is a separate capture endpoint, mixed
/// into the same track downstream by the encoder.
///
/// Both streams hand their raw interleaved buffers straight to the encoder, which owns PTS and the
/// mix. Capture runs on NAudio's own per-stream WASAPI event threads.
/// </summary>
public sealed class WasapiAudioSource : IDisposable
{
    private WasapiLoopbackCapture? _loopback;
    private WasapiCapture? _mic;
    private bool _disposed;
    private volatile float _micGain = 1f;     // linear mic volume (1.0 = 100%)
    private volatile bool _micMuted;          // push-to-talk: drop mic while muted
    private volatile float _systemGain = 1f;  // linear full-system loopback volume
    private bool _systemEnabled = true;       // false = don't capture full-system audio at all
    private volatile bool _micMono;           // downmix the mic to a single channel
    private string? _systemDeviceId;          // chosen render endpoint to loop back; null/"" = default

    // Per-app capture ("Обрані програми"): one process-loopback per selected app, mixed by a steady
    // timer into a single 48k/2ch/float "system" stream raised via GameDataReady — so the engine and
    // encoder see it exactly like a normal full-system loopback.
    private bool _appsMode;
    private readonly List<AppMix> _appMixes = new();
    private System.Threading.Timer? _mixTimer;
    private readonly WaveFormat _systemFormat = WaveFormat.CreateIeeeFloatWaveFormat(48000, 2);
    private const int MixChannels = 2;
    private const int MixIntervalMs = 20;
    private const int MixChunkFloats = 48000 * MixChannels * MixIntervalMs / 1000; // 20ms of 48k stereo

    /// <summary>One selected app: its process-loopback source, linear gain, and a bounded queue of
    /// gain-applied interleaved float samples awaiting the next mix tick.</summary>
    private sealed class AppMix
    {
        public required ProcessLoopbackCapture Capture;
        public float Gain;
        public readonly Queue<float> Buffer = new();
        public readonly object Lock = new();
    }

    /// <summary>Linear mic gain (1.0 = 100%). Applied to every mic buffer.</summary>
    public void SetMicVolume(float linear) => _micGain = linear < 0 ? 0 : linear;

    /// <summary>Push-to-talk: when true, mic buffers are dropped (silence-padded downstream).</summary>
    public void SetMicMuted(bool muted) => _micMuted = muted;

    /// <summary>(buffer, valid byte count, format) for GAME/system loopback — the master track.</summary>
    public event Action<byte[], int, WaveFormat>? GameDataReady;

    /// <summary>(buffer, valid byte count, format) for the MICROPHONE — mixed in by the encoder.</summary>
    public event Action<byte[], int, WaveFormat>? MicDataReady;

    /// <summary>System-audio capture format, available once <see cref="Start"/> has run. In per-app
    /// mode this is the fixed mixer format (48k/2ch/float); otherwise the loopback endpoint format.</summary>
    public WaveFormat? GameFormat => _appsMode ? _systemFormat : (_loopback?.WaveFormat ?? _systemFormat);

    /// <summary>Begins capture. <paramref name="micDeviceId"/>: null = default comms mic, "" = no
    /// mic, otherwise a specific endpoint id. Throws only if game loopback itself fails (mic errors
    /// are non-fatal — we continue game-audio-only).</summary>
    public void Start(string? micDeviceId, bool appsMode = false, IReadOnlyList<(string Exe, float Volume)>? apps = null,
                      bool systemEnabled = true, float systemVolume = 1f, bool micMono = false,
                      uint gamePid = 0, float gameVolume = 1f, bool gameEnabled = true,
                      string? systemDeviceId = null)
    {
        _systemEnabled = systemEnabled;
        _systemGain = systemVolume < 0 ? 0 : systemVolume;
        _micMono = micMono;
        _systemDeviceId = systemDeviceId;
        // Apps mode runs if the user picked apps OR there's a game to capture for the "Звук гри" row.
        _appsMode = appsMode && ((apps is { Count: > 0 }) || (gameEnabled && gamePid != 0));
        if (_appsMode) StartAppCaptures(apps ?? Array.Empty<(string, float)>(), gamePid, gameVolume, gameEnabled);
        else if (_systemEnabled) StartSystemLoopback();
        else Console.WriteLine("[WasapiAudioSource] system audio disabled (mic-only / silent system).");

        if (micDeviceId == "") { Console.WriteLine("[WasapiAudioSource] mic disabled."); return; }
        try
        {
            using var enumerator = new MMDeviceEnumerator();
            MMDevice micDev = string.IsNullOrEmpty(micDeviceId)
                ? enumerator.GetDefaultAudioEndpoint(DataFlow.Capture, Role.Communications)
                : enumerator.GetDevice(micDeviceId);
            using (micDev)
            {
                _mic = new WasapiCapture(micDev);
                _mic.DataAvailable += OnMicData;
                _mic.StartRecording();
                var f = _mic.WaveFormat;
                Console.WriteLine($"[WasapiAudioSource] Mic '{micDev.FriendlyName}': {f.SampleRate}Hz {f.Channels}ch {f.BitsPerSample}bit {f.Encoding}.");
            }
        }
        catch (Exception ex) { Console.WriteLine($"[WasapiAudioSource] mic unavailable — game audio only: {ex.Message}"); }
    }

    /// <summary>"Весь звук ПК": loop back a render endpoint. Uses the chosen output device when set,
    /// otherwise the default multimedia endpoint; an invalid/removed pick falls back to default.</summary>
    private void StartSystemLoopback()
    {
        using var enumerator = new MMDeviceEnumerator();
        MMDevice render;
        if (string.IsNullOrEmpty(_systemDeviceId))
        {
            render = enumerator.GetDefaultAudioEndpoint(DataFlow.Render, Role.Multimedia);
        }
        else
        {
            try { render = enumerator.GetDevice(_systemDeviceId); }
            catch (Exception ex)
            {
                Console.WriteLine($"[WasapiAudioSource] output device '{_systemDeviceId}' unavailable ({ex.Message}) — using default.");
                render = enumerator.GetDefaultAudioEndpoint(DataFlow.Render, Role.Multimedia);
            }
        }
        using (render)
        {
            _loopback = new WasapiLoopbackCapture(render);
            _loopback.DataAvailable += OnGameData;
            _loopback.StartRecording();
            var f = _loopback.WaveFormat;
            Console.WriteLine($"[WasapiAudioSource] Loopback (full system) on '{render.FriendlyName}': {f.SampleRate}Hz {f.Channels}ch {f.BitsPerSample}bit {f.Encoding}.");
        }
    }

    /// <summary>"Обрані програми": one process-loopback per selected app, summed by <see cref="MixTick"/>
    /// into a single system stream. Falls back to full-system loopback if none of the apps are running.</summary>
    private void StartAppCaptures(IReadOnlyList<(string Exe, float Volume)> apps, uint gamePid, float gameVolume, bool gameEnabled)
    {
        // Each process is captured EXACTLY ONCE (by PID). Two process-loopbacks of the same process
        // conflict and one yields no audio — which is why the game went silent when it appeared both
        // as the "Звук гри" row AND as its own cs2.exe row.
        var capturedPids = new HashSet<uint>();

        // Detected game's own audio ("Звук гри") first, so the game row owns the game's PID.
        if (gameEnabled && gamePid != 0 && TryAddCapture(gamePid, gameVolume, "game audio"))
            capturedPids.Add(gamePid);

        foreach (var (exe, volume) in apps)
        {
            string name = exe.EndsWith(".exe", StringComparison.OrdinalIgnoreCase) ? exe[..^4] : exe;
            var procs = System.Diagnostics.Process.GetProcessesByName(name);
            try
            {
                if (procs.Length == 0) { Console.WriteLine($"[WasapiAudioSource] app '{exe}' not running — skipped."); continue; }
                // Prefer the process owning a main window (the tree root that renders audio); else first.
                var proc = procs.FirstOrDefault(p => p.MainWindowHandle != IntPtr.Zero) ?? procs[0];
                uint pid = (uint)proc.Id;
                if (capturedPids.Contains(pid))
                {
                    Console.WriteLine($"[WasapiAudioSource] app '{exe}' is the captured game (pid={pid}) — skipping duplicate.");
                    continue;
                }
                if (TryAddCapture(pid, volume, $"app {exe}")) capturedPids.Add(pid);
            }
            catch (Exception ex) { Console.WriteLine($"[WasapiAudioSource] app '{exe}' loopback failed: {ex.Message}"); }
            finally { foreach (var p in procs) p.Dispose(); }
        }

        if (_appMixes.Count == 0)
        {
            Console.WriteLine("[WasapiAudioSource] no app captures available — falling back to full-system loopback.");
            _appsMode = false;
            StartSystemLoopback();
            return;
        }
        _mixTimer = new System.Threading.Timer(_ => MixTick(), null, MixIntervalMs, MixIntervalMs);
    }

    /// <summary>Starts one process-loopback source feeding the mixer. Returns false on failure.</summary>
    private bool TryAddCapture(uint pid, float gain, string label)
    {
        try
        {
            var cap = new ProcessLoopbackCapture(pid);
            var mix = new AppMix { Capture = cap, Gain = gain };
            cap.DataAvailable += (buf, n) => OnAppData(mix, buf, n);
            cap.Start();
            _appMixes.Add(mix);
            Console.WriteLine($"[WasapiAudioSource] {label} capture: pid={pid} gain={gain:F2}.");
            return true;
        }
        catch (Exception ex) { Console.WriteLine($"[WasapiAudioSource] {label} loopback failed: {ex.Message}"); return false; }
    }

    private void OnAppData(AppMix mix, byte[] buf, int bytes)
    {
        if (_disposed) return;
        int n = bytes / sizeof(float);
        var src = MemoryMarshal.Cast<byte, float>(buf.AsSpan(0, n * sizeof(float)));
        lock (mix.Lock)
        {
            if (mix.Buffer.Count > 48000 * MixChannels) return;   // bound the backlog (~1s)
            float g = mix.Gain;
            foreach (var s in src) mix.Buffer.Enqueue(s * g);
        }
    }

    /// <summary>Sums one <see cref="MixIntervalMs"/> chunk from every app queue (silence where empty),
    /// clamps, and raises it as the system stream — emitted every tick so the timeline stays steady.</summary>
    private void MixTick()
    {
        if (_disposed || !_appsMode || _appMixes.Count == 0) return;
        try
        {
            var mix = new float[MixChunkFloats];
            foreach (var m in _appMixes)
                lock (m.Lock)
                {
                    int take = Math.Min(MixChunkFloats, m.Buffer.Count);
                    for (int i = 0; i < take; i++) mix[i] += m.Buffer.Dequeue();
                }
            for (int i = 0; i < mix.Length; i++) mix[i] = mix[i] > 1f ? 1f : mix[i] < -1f ? -1f : mix[i];

            var bytes = new byte[mix.Length * sizeof(float)];
            Buffer.BlockCopy(mix, 0, bytes, 0, bytes.Length);
            GameDataReady?.Invoke(bytes, bytes.Length, _systemFormat);
        }
        catch { /* never let the mix timer throw */ }
    }

    private void OnGameData(object? sender, WaveInEventArgs e)
    {
        if (_disposed || e.BytesRecorded <= 0) return;
        var fmt = _loopback!.WaveFormat;
        byte[] buf = e.Buffer;
        if (_systemGain != 1f && fmt.Encoding == WaveFormatEncoding.IeeeFloat)
            buf = ApplyGainFloat(e.Buffer, e.BytesRecorded, _systemGain);
        GameDataReady?.Invoke(buf, e.BytesRecorded, fmt);
    }

    private void OnMicData(object? sender, WaveInEventArgs e)
    {
        if (_disposed || e.BytesRecorded <= 0 || _micMuted) return;   // PTT mute = drop the buffer
        var fmt = _mic!.WaveFormat;
        byte[] buf = e.Buffer;
        // Apply mic volume in place on a copy (float PCM only — the WASAPI capture format).
        if (_micGain != 1f && fmt.Encoding == NAudio.Wave.WaveFormatEncoding.IeeeFloat)
            buf = ApplyGainFloat(e.Buffer, e.BytesRecorded, _micGain);

        // Mono channel: downmix a stereo mic to one channel (a 1-ch device is already mono → no-op).
        if (_micMono && fmt.Channels == 2 && fmt.Encoding == WaveFormatEncoding.IeeeFloat)
        {
            var (mbuf, mbytes, mfmt) = DownmixToMonoFloat(buf, e.BytesRecorded, fmt);
            MicDataReady?.Invoke(mbuf, mbytes, mfmt);
            return;
        }
        MicDataReady?.Invoke(buf, e.BytesRecorded, fmt);
    }

    /// <summary>Averages a stereo float buffer to a single channel; returns a fresh buffer + mono format.</summary>
    private static (byte[] buf, int bytes, WaveFormat fmt) DownmixToMonoFloat(byte[] src, int count, WaveFormat stereo)
    {
        int frames = count / sizeof(float) / 2;
        var dst = new byte[frames * sizeof(float)];
        var s = MemoryMarshal.Cast<byte, float>(src.AsSpan(0, frames * 2 * sizeof(float)));
        var d = MemoryMarshal.Cast<byte, float>(dst.AsSpan());
        for (int i = 0; i < frames; i++) d[i] = (s[i * 2] + s[i * 2 + 1]) * 0.5f;
        return (dst, dst.Length, WaveFormat.CreateIeeeFloatWaveFormat(stereo.SampleRate, 1));
    }

    private static byte[] ApplyGainFloat(byte[] src, int count, float gain)
    {
        var dst = new byte[count];
        Buffer.BlockCopy(src, 0, dst, 0, count);
        int n = count / sizeof(float);
        var span = System.Runtime.InteropServices.MemoryMarshal.Cast<byte, float>(dst.AsSpan(0, n * sizeof(float)));
        for (int i = 0; i < span.Length; i++)
        {
            float v = span[i] * gain;
            span[i] = v > 1f ? 1f : v < -1f ? -1f : v;   // clamp
        }
        return dst;
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        _mixTimer?.Dispose(); _mixTimer = null;
        foreach (var m in _appMixes) { try { m.Capture.Dispose(); } catch { } }
        _appMixes.Clear();
        StopOne(_mic, OnMicData); _mic = null;
        StopOne(_loopback, OnGameData); _loopback = null;
    }

    private static void StopOne(IWaveIn? cap, EventHandler<WaveInEventArgs> handler)
    {
        if (cap == null) return;
        try { cap.DataAvailable -= handler; cap.StopRecording(); cap.Dispose(); } catch { }
    }
}
