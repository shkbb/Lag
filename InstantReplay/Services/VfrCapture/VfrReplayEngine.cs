using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using FFmpeg.AutoGen;
using SharpDX.Direct3D11;

namespace Lag.Services.VfrCapture;

/// <summary>What the engine records. <see cref="AutoGame"/> = capture a detected game window, and
/// fall back to the monitor when no game is running ("record anything"). <see cref="Display"/>
/// = always capture the chosen monitor, no game detection (explicit desktop recording).</summary>
public enum CaptureMode { AutoGame, Display }

/// <summary>Options for a recording session, mirroring the user's settings.</summary>
public sealed record VfrEngineOptions
{
    public string LibraryPath { get; init; } = "";
    public double BufferSeconds { get; init; } = 120;
    public string? PreferredCodec { get; init; }          // "", av1, hevc, h264
    public int BitrateKbps { get; init; } = 30000;
    public string FileFormat { get; init; } = "mp4";     // output container: mp4 / mkv / mov
    public int TargetHeight { get; init; }                // 0 = native window height
    public int MaxFps { get; init; } = 120;               // cap to bound load on very fast games
    public int Preset { get; init; } = 7;                 // 1 fastest … 7 best; p7 holds fps here (encode ~0.5ms)
    public bool CaptureCursor { get; init; } = true;
    public string? MonitorDeviceId { get; init; }         // recorded monitor (null = primary)
    public CaptureMode Mode { get; init; } = CaptureMode.AutoGame;   // game-auto (fallback desktop) vs always-display
    public int AdapterIndex { get; init; } = -1;          // GPU/DXGI adapter (-1 = default)
    public string? MicrophoneId { get; init; }            // null = default mic, "" = no mic, else endpoint id
    public float MicVolume { get; init; } = 1.0f;         // linear mic gain (1.0 = 100%)
    public bool MicStartMuted { get; init; }              // start with mic muted (push-to-talk)
    public bool SeparateAudioTracks { get; init; }        // true = system + mic as separate tracks (editor); false = one mixed track
    public bool AppsAudioMode { get; init; }              // true = capture only selected apps (process loopback) vs full system
    public bool AudioModeByTarget { get; init; }          // true = pick mode by capture target: game→apps, desktop→full system
    public IReadOnlyList<(string Exe, float Volume)> AudioApps { get; init; } = Array.Empty<(string, float)>();
    public bool SystemAudioEnabled { get; init; } = true; // false = don't capture full-system audio (mic-only / apps-only)
    public string? SystemAudioDeviceId { get; init; }     // chosen render endpoint to loop back (null/"" = default)
    public float SystemVolume { get; init; } = 1.0f;      // linear gain for the full-system loopback (1.0 = 100%)
    public bool MicMono { get; init; }                    // downmix the microphone to a single channel
    public bool GameAudioEnabled { get; init; } = true;   // apps mode: also capture the locked game's own audio
    public float GameVolume { get; init; } = 1.0f;        // linear gain for the game-audio source
    public bool AudioEnabled { get; init; } = true;       // false = video-only (diagnostics / A-B)
    public IntPtr ForcedWindow { get; init; }             // capture this exact window, bypass detection (testing/manual)
    public bool DuplicationOnly { get; init; }            // testing: skip foreground gating + WGC (measure duplication path)
}

/// <summary>
/// The native VFR replay engine — our capture core. It locks onto a running game window,
/// captures it via WGC at the game's real frame cadence, encodes each frame on the GPU with its
/// true timestamp into a rolling buffer, and writes a variable-frame-rate MP4 on demand. No CFR
/// padding, no duplicate frames, no system-RAM copies.
///
/// Game lock (the fix for the alt-tab freezes): once a real game is detected fullscreen on the
/// recorded monitor we hold the capture on it through alt-tabs, releasing only when the game
/// exits — we never retarget a live WGC session.
///
/// Video-only for now; audio is layered on next. All capture/encode runs on the WGC callback
/// thread; Save runs on the caller's thread and only touches the thread-safe ring buffer.
/// </summary>
public sealed class VfrReplayEngine : IDisposable
{
    private readonly object _gate = new();
    private readonly GameDetector _detector = new();
    private VfrEngineOptions _options = new();
    private D3D11Context? _d3d;
    private EncoderChoice? _encoderChoice;

    private Timer? _lockTimer;
    private Timer? _audioPumpTimer;   // steady pulse: advances audio timeline through system-silence gaps
    private WgcCaptureSource? _capture;
    private NvencVfrEncoder? _encoder;
    private PacketRingBuffer? _ring;
    private bool _fpsAdaptDone;   // fixed-bitrate fps-hint retune: done once per capture session
    private WasapiAudioSource? _audio;
    private readonly List<AudioTrackState> _audioTracks = new();   // guarded by _gate

    /// <summary>One muxed audio track being recorded. Encoder/Ring may be filled lazily (the mic
    /// track is created on its first buffer, when the device format is known).</summary>
    private sealed class AudioTrackState
    {
        public string Title;
        public AacAudioEncoder? Encoder;
        public PacketRingBuffer? Ring;
        public AudioTrackState(string title, AacAudioEncoder? enc, PacketRingBuffer? ring)
        { Title = title; Encoder = enc; Ring = ring; }
    }

    private enum LockKind { None, Game, Desktop }
    private LockKind _lockKind = LockKind.None;
    private IntPtr _lockedHwnd;
    private uint _lockedPid;
    private DateTime _lockCooldownUntil = DateTime.MinValue;   // back-off after a failed lock (no retry-thrash)
    private long _lastEncodedUs = long.MinValue;
    private long _minIntervalUs;
    private long _startQpc;   // engine clock origin for this capture session

    /// <summary>Immutable snapshot of the pause state, swapped atomically via the volatile
    /// <see cref="_pause"/> field. Keeping the accumulator, the pause-start timestamp and the flag
    /// in ONE reference lets <see cref="EngineNowUs"/> read a consistent view lock-free from the hot
    /// capture/audio threads (separate volatile fields could be read mid-update and glitch the clock).</summary>
    private sealed class PauseState
    {
        public readonly long AccumUs;        // total paused microseconds folded into the clock so far
        public readonly long PauseStartQpc;  // QPC ticks when the current pause began (0 when not paused)
        public readonly bool Paused;
        public PauseState(long accumUs, long pauseStartQpc, bool paused)
        { AccumUs = accumUs; PauseStartQpc = pauseStartQpc; Paused = paused; }
    }

    /// <summary>Pause freezes the engine clock so a paused span never reaches the buffer — a saved
    /// replay stitches the moments before and after the pause together with no held frame.</summary>
    private volatile PauseState _pause = new(0, 0, false);

    private volatile bool _running;
    private volatile bool _disposed;

    /// <summary>Raised after a replay file is written. Arg is the absolute path.</summary>
    public event EventHandler<string>? ReplaySaved;

    public bool IsRunning => _running;
    public bool IsPaused => _pause.Paused;
    public string FriendlyEncoder => _encoderChoice?.FriendlyName ?? "—";

    // What's currently being captured — surfaced to the UI's "now recording X" indicator.
    public bool TargetActive { get; private set; }
    public bool TargetIsGame { get; private set; }
    public string TargetName { get; private set; } = "";
    public string? TargetExe { get; private set; }
    /// <summary>Raised (on the lock-timer thread) whenever the capture target changes.</summary>
    public event Action? TargetChanged;

    /// <summary>Diagnostics: frames delivered by WGC and frames actually submitted to the encoder.</summary>
    public long FramesCaptured;
    public long FramesEncoded;
    public double BufferedSeconds => _ring?.BufferedSeconds ?? 0;

    /// <summary>Diagnostics: average per-frame milliseconds in colour-convert and encode-submit.</summary>
    public (double ConvertMs, double EncodeMs) AvgStageMs
    {
        get
        {
            var e = _encoder;
            if (e == null || e.TimedFrames == 0) return (0, 0);
            return (e.ConvertUs / 1000.0 / e.TimedFrames, e.EncodeUs / 1000.0 / e.TimedFrames);
        }
    }

    /// <summary>
    /// Whether this machine can run the native engine: WGC supported (Win10 1903+), FFmpeg bound,
    /// and at least one usable encoder. Callers fall back to the OBS recorder when false.
    /// </summary>
    public static bool IsAvailable(string obsCoreDir)
        => WgcCaptureSource.IsSupported
           && FfmpegInterop.Initialize(obsCoreDir)
           && EncoderSelector.Available.Count > 0;

    /// <summary>Arms the engine. It begins capturing automatically when a game is detected, and
    /// keeps a rolling buffer thereafter.</summary>
    public void Start(VfrEngineOptions options)
    {
        lock (_gate)
        {
            if (_disposed) return;
            _options = options;
            // Only drop frames spaced WAY tighter than the target (a 2× burst) — a hard 1/MaxFps
            // gate threw away ~half the VFR bursts (encoded 68 of 120 captured) and, since encoded
            // fps fell below the CBR fps-hint, also starved the bitrate. A 2× floor keeps the stream
            // near the captured rate so fps AND bitrate land where intended.
            _minIntervalUs = options.MaxFps > 0 ? 1_000_000L / (options.MaxFps * 2L) : 0;

            _d3d ??= new D3D11Context(options.AdapterIndex);
            _encoderChoice = EncoderSelector.Select(options.PreferredCodec)
                ?? throw new InvalidOperationException("No usable video encoder.");
            Console.WriteLine($"[VfrReplayEngine] Armed. Encoder: {_encoderChoice.FriendlyName}, buffer {options.BufferSeconds}s.");

            _running = true;
            _lockTimer ??= new Timer(_ => SafeEvaluateLock(), null, 500, 2000);
        }
    }

    private void SafeEvaluateLock()
    {
        if (_disposed || !_running) return;
        try { EvaluateLock(); }
        catch (Exception ex) { Console.WriteLine($"[VfrReplayEngine] lock eval error: {ex.Message}"); }
    }

    private void EvaluateLock()
    {
        // Hold a live WINDOW lock (detected game OR forced window) while its process lives —
        // survives alt-tabs; we never retarget a live WGC session.
        if (_lockKind == LockKind.Game)
        {
            if (IsWindow(_lockedHwnd) && IsProcessAlive(_lockedPid)) return;
            Console.WriteLine("[VfrReplayEngine] Locked window ended — releasing capture.");
            TeardownCapture();
        }

        // After a failed lock (e.g. NVENC open returns -2 because sessions weren't released yet),
        // back off instead of retrying every tick — a tight retry loop re-allocates the big D3D11
        // hwframe pool + re-attempts NVENC each time, which can thrash GPU resources hard enough to
        // hang the device (a hard reboot with no bugcheck). Give the driver time to recover.
        if (DateTime.UtcNow < _lockCooldownUntil) return;

        // Forced target (manual "capture this window" / tests): lock it directly, bypassing
        // detection and capture-mode. Held until its process exits.
        if (_options.ForcedWindow != IntPtr.Zero)
        {
            if (_lockKind != LockKind.None) return;
            if (!IsWindow(_options.ForcedWindow)) return;
            GetWindowThreadProcessId(_options.ForcedWindow, out uint fpid);
            if (fpid == 0) return;
            string fexe;
            try { fexe = System.Diagnostics.Process.GetProcessById((int)fpid).ProcessName + ".exe"; }
            catch { return; }
            LockOnto(_options.ForcedWindow, fpid, fexe);
            return;
        }

        // Display mode: always capture the chosen monitor — no game detection.
        if (_options.Mode == CaptureMode.Display)
        {
            if (_lockKind != LockKind.Desktop) { TeardownCapture(); LockOntoMonitor(); }
            return;
        }

        // Auto mode: record a game if one is present, otherwise the desktop ("record anything").
        var cand = _detector.FindBestGame(_options.MonitorDeviceId);
        if (cand != null)
        {
            // A game appeared while we were capturing the desktop — switch to it.
            if (_lockKind == LockKind.Desktop)
            {
                Console.WriteLine($"[VfrReplayEngine] Game appeared ({cand.Exe}) — switching from desktop to game.");
                TeardownCapture();
            }
            if (_lockKind == LockKind.None)
            {
                Console.WriteLine($"[VfrReplayEngine] Game detected: {cand.GameName} ({cand.Exe}) score={cand.Score} fullscreen={cand.Fullscreen}");
                LockOnto(cand.Hwnd, cand.Pid, cand.Exe, cand.GameName);
            }
            return;
        }

        // No game running → fall back to capturing the monitor (desktop / YouTube / anything).
        if (_lockKind == LockKind.None) LockOntoMonitor();
    }

    /// <summary>Lock onto a game/app WINDOW — WGC reads the window's own backbuffer, so it follows
    /// the game through alt-tabs (no source switch) and sees a fullscreen game past independent flip.</summary>
    private void LockOnto(IntPtr hwnd, uint pid, string exe, string? gameName = null)
    {
        if (!GetWindowRect(hwnd, out var rect)) return;
        int winW = rect.Right - rect.Left, winH = rect.Bottom - rect.Top;
        if (winW < 2 || winH < 2) return;
        var (outW, outH) = OutputDims(winW, winH);
        StartSession(new WgcCaptureSource(_d3d!, hwnd), outW, outH, LockKind.Game, hwnd, pid, $"game {exe}",
                     string.IsNullOrWhiteSpace(gameName) ? exe : gameName!, exe);
    }

    /// <summary>Lock onto a MONITOR — desktop / "record anything". Captures the whole selected
    /// display (primary when no MonitorDeviceId set).</summary>
    private void LockOntoMonitor()
    {
        IntPtr hmon = ResolveMonitor(_options.MonitorDeviceId, out int monW, out int monH, out string monName);
        if (hmon == IntPtr.Zero || monW < 2 || monH < 2)
        {
            Console.WriteLine($"[VfrReplayEngine] no monitor to capture (id='{_options.MonitorDeviceId}').");
            return;
        }
        var (outW, outH) = OutputDims(monW, monH);
        StartSession(WgcCaptureSource.ForMonitor(_d3d!, hmon), outW, outH, LockKind.Desktop, IntPtr.Zero, 0,
                     $"desktop {monName} ({monW}x{monH})", "", null);
    }

    /// <summary>Output resolution: downscale to the target height preserving aspect; even dims for NV12.</summary>
    private (int W, int H) OutputDims(int srcW, int srcH)
    {
        int outH = (_options.TargetHeight > 0 && _options.TargetHeight < srcH) ? _options.TargetHeight : srcH;
        int outW = (int)Math.Round((double)srcW * outH / srcH);
        return (outW & ~1, outH & ~1);
    }

    /// <summary>Shared start path for a window or monitor capture: build encoder + ring + audio,
    /// wire the frame callback, lift priority, and begin. <paramref name="hwnd"/>/<paramref name="pid"/>
    /// are 0 for a desktop (monitor) lock.</summary>
    private void StartSession(WgcCaptureSource capture, int outW, int outH, LockKind kind,
                              IntPtr hwnd, uint pid, string label, string targetName, string? targetExe)
    {
        try
        {
            var encoder = new NvencVfrEncoder(_d3d!, _encoderChoice!, outW, outH, _options.BitrateKbps, _options.Preset,
                                              fpsHint: _options.MaxFps > 0 ? _options.MaxFps : 120);
            var ring = new PacketRingBuffer(_options.BufferSeconds);
            encoder.PacketReady += p => ring.Add(p);
            capture.FrameReady += OnFrame;

            lock (_gate)
            {
                _encoder = encoder;
                _ring = ring;
                _capture = capture;
                _lockKind = kind;
                _lockedHwnd = hwnd;
                _lockedPid = pid;
                _lastEncodedUs = long.MinValue;
                _fpsAdaptDone = false;                  // re-measure the fps-hint for this new session
                _startQpc = System.Diagnostics.Stopwatch.GetTimestamp();
                _pause = new PauseState(0, 0, false);   // a fresh session/clock is never paused
                TargetActive = true;
                TargetIsGame = kind == LockKind.Game;
                TargetName = targetName;
                TargetExe = targetExe;
            }

            capture.Start(_options.CaptureCursor, _options.MaxFps);
            // Lift the process to High while actively capturing so the converter/encoder don't get
            // starved by the game under GPU+CPU load. Restored on teardown.
            SetProcessPriority(System.Diagnostics.ProcessPriorityClass.High);
            // The rolling buffer holds hundreds of MB of packet byte[]; default GC does periodic
            // gen2/LOH collections that freeze the process ~50ms and make WGC skip presents. Defer
            // those while capturing so the frame stream stays smooth.
            try { System.Runtime.GCSettings.LatencyMode = System.Runtime.GCLatencyMode.SustainedLowLatency; } catch { }

            StartAudio();
            Console.WriteLine($"[VfrReplayEngine] Locked onto {label} @ {outW}x{outH} (WGC {(kind == LockKind.Desktop ? "monitor" : "window")} capture).");
            TargetChanged?.Invoke();
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[VfrReplayEngine] Failed to lock onto {label}: {ex.Message} — backing off 15s (no retry-thrash).");
            _lockCooldownUntil = DateTime.UtcNow.AddSeconds(15);
            TeardownCapture();
        }
    }

    /// <summary>Resolves a monitor device id ("\\.\DISPLAY2") to its HMONITOR + physical size; null/empty
    /// picks the primary monitor. Physical pixels (the process is per-monitor DPI aware).</summary>
    private static IntPtr ResolveMonitor(string? deviceId, out int width, out int height, out string name)
    {
        IntPtr found = IntPtr.Zero; int w = 0, h = 0; string nm = "";
        MonitorEnumProc cb = (IntPtr hMon, IntPtr hdc, ref Win32Rect r, IntPtr d) =>
        {
            if (found != IntPtr.Zero) return true;
            var info = new MonitorInfoEx { Size = (uint)Marshal.SizeOf<MonitorInfoEx>() };
            if (GetMonitorInfoW(hMon, ref info))
            {
                bool isPrimary = (info.Flags & 1) != 0; // MONITORINFOF_PRIMARY
                bool match = string.IsNullOrEmpty(deviceId)
                    ? isPrimary
                    : string.Equals(info.Device, deviceId, StringComparison.OrdinalIgnoreCase);
                if (match)
                {
                    found = hMon;
                    w = info.Monitor.Right - info.Monitor.Left;
                    h = info.Monitor.Bottom - info.Monitor.Top;
                    nm = info.Device;
                }
            }
            return true;
        };
        EnumDisplayMonitors(IntPtr.Zero, IntPtr.Zero, cb, IntPtr.Zero);
        GC.KeepAlive(cb);
        width = w; height = h; name = nm;
        return found;
    }

    private void OnFrame(Texture2D bgra, long sourceTicks)
    {
        var encoder = _encoder;
        if (encoder == null) return;
        // Paused: drop the frame. The engine clock is frozen too, so nothing reaches the buffer and
        // the paused span is simply absent from the timeline.
        if (_pause.Paused) return;
        System.Threading.Interlocked.Increment(ref FramesCaptured);

        // PTS from the ENGINE's own clock at receive time, not the source timestamp: the engine clock
        // is one monotonic timeline (paused time subtracted) shared by video and audio, so they stay
        // in sync and a source switch / pause never breaks the VFR cadence.
        long ptsUs = EngineNowUs();

        // Frame-rate cap: skip frames closer than the min interval to bound encoder load on very
        // high-fps games (the file stays VFR, just capped at MaxFps).
        if (_minIntervalUs > 0 && _lastEncodedUs != long.MinValue && ptsUs - _lastEncodedUs < _minIntervalUs) return;
        _lastEncodedUs = ptsUs;

        encoder.Encode(bgra, ptsUs);
        System.Threading.Interlocked.Increment(ref FramesEncoded);

        // Fixed-bitrate mode only: once, after a short warm-up, retune the encoder's fps-hint to the
        // MEASURED capture rate so a CBR target actually holds. A hint above the real fps underfills
        // (bits/frame = bit_rate/hint); a hint at-or-below it is capped by rc_max, so it holds.
        if (_options.BitrateKbps > 0 && !_fpsAdaptDone)
        {
            try { MaybeAdaptFpsHint(); }
            catch (Exception ex) { _fpsAdaptDone = true; Console.WriteLine($"[fps-adapt] error, keeping encoder: {ex.Message}"); }
        }
    }

    /// <summary>One-time (per session) fps-hint retune for the fixed-bitrate path. Runs on the
    /// capture thread right after Encode, so disposing/recreating the encoder is D3D-context-safe.</summary>
    private void MaybeAdaptFpsHint()
    {
        long elapsedUs = EngineNowUs();
        if (elapsedUs < 3_000_000) return;                 // need ~3s of frames to measure
        _fpsAdaptDone = true;                               // one-shot, whatever the outcome

        var enc = _encoder;
        if (enc == null) return;
        // Floor-rounded measured fps (the startup ramp biases it slightly LOW — the safe side, since
        // hint ≤ real fps holds via rc_max). Never raise the hint above the MaxFps cap it opened with.
        int measured = (int)(FramesEncoded * 1_000_000L / Math.Max(1, elapsedUs));
        measured = Math.Clamp(measured, 5, enc.FpsHint);
        // Only worth a reopen if the current hint is meaningfully too high (the underfill case).
        if (enc.FpsHint - measured < enc.FpsHint * 0.15) return;
        SwapEncoderFpsHint(measured);
    }

    /// <summary>Reopens the video encoder with a new fps-hint (fixed-bitrate adaptation). The packet
    /// ring is reset because the new encoder starts a fresh stream — the same reset a game↔desktop
    /// switch already does, and it only happens a few seconds into a session so no useful buffer is
    /// lost. Holds <see cref="_gate"/> so it can't race a save / lock-switch.</summary>
    private void SwapEncoderFpsHint(int hint)
    {
        lock (_gate)
        {
            var old = _encoder;
            if (old == null) return;
            int oldHint = old.FpsHint;
            Console.WriteLine($"[fps-adapt] reopening encoder {oldHint}→{hint} ...");
            NvencVfrEncoder fresh;
            try
            {
                fresh = new NvencVfrEncoder(_d3d!, _encoderChoice!, old.Width, old.Height,
                                            _options.BitrateKbps, _options.Preset, fpsHint: hint);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[fps-adapt] reopen failed, keeping current encoder: {ex.Message}");
                return;
            }
            var freshRing = new PacketRingBuffer(_options.BufferSeconds);
            fresh.PacketReady += p => freshRing.Add(p);
            _encoder = fresh;
            _ring = freshRing;
            _lastEncodedUs = long.MinValue;
            Console.WriteLine("[fps-adapt] new encoder live, disposing old ...");
            old.Dispose();
            Console.WriteLine($"[fps-adapt] done — fixed-bitrate fps-hint {oldHint}→{hint}, buffer reset.");
        }
    }

    /// <summary>Microseconds since this capture session's clock origin — the shared A/V timeline —
    /// with paused time subtracted, so the clock holds still while paused. Reads the pause snapshot
    /// once so the view is consistent even when pause/resume races this hot-path call.</summary>
    private long EngineNowUs()
    {
        long ts = System.Diagnostics.Stopwatch.GetTimestamp();
        long freq = System.Diagnostics.Stopwatch.Frequency;
        var p = _pause;
        long pausedUs = p.AccumUs + (p.Paused ? (ts - p.PauseStartQpc) * 1_000_000L / freq : 0);
        return (ts - _startQpc) * 1_000_000L / freq - pausedUs;
    }

    /// <summary>Starts loopback game/system audio capture → AAC into its own rolling buffer, on the
    /// SAME engine clock as the video so the two stay in sync on save. Best-effort: any failure
    /// leaves the session video-only.</summary>
    private void StartAudio()
    {
        if (!_options.AudioEnabled) { Console.WriteLine("[VfrReplayEngine] audio disabled (video-only)."); return; }
        try
        {
            var audio = new WasapiAudioSource();
            // In apps mode, also feed the locked game's own audio into the "Звук гри" row source.
            uint gamePid = _lockKind == LockKind.Game ? _lockedPid : 0;
            // "Switch audio by capture target": when on, the engine picks the mode by what it's
            // recording — a game → selected-apps audio, the desktop → whole-PC audio — overriding the
            // manual choice. Off → use the manual AppsAudioMode the user set.
            bool appsMode = _options.AudioModeByTarget ? (_lockKind == LockKind.Game) : _options.AppsAudioMode;
            audio.Start(_options.MicrophoneId, appsMode, _options.AudioApps,
                        _options.SystemAudioEnabled, _options.SystemVolume, _options.MicMono,
                        gamePid, _options.GameVolume, _options.GameAudioEnabled,
                        _options.SystemAudioDeviceId);
            audio.SetMicVolume(_options.MicVolume);
            audio.SetMicMuted(_options.MicStartMuted);
            var gf = audio.GameFormat!;
            var tracks = new List<AudioTrackState>();

            if (_options.SeparateAudioTracks)
            {
                // Track 0: "All" = game + mic mixed. DEFAULT track, so EVERY player plays it and you
                // hear everything; tracks 1-2 stay separate for the editor. This is the layout
                // we use: an "All Audio" default + per-source tracks.
                var allEnc = new AacAudioEncoder(gf.SampleRate, gf.Channels, MapSampleFormat(gf), 160);
                var allRing = new PacketRingBuffer(_options.BufferSeconds);
                allEnc.PacketReady += p => allRing.Add(p);
                tracks.Add(new AudioTrackState("All", allEnc, allRing));

                // Track 1: System (game only).
                var gameEnc = new AacAudioEncoder(gf.SampleRate, gf.Channels, MapSampleFormat(gf), 160);
                var gameRing = new PacketRingBuffer(_options.BufferSeconds);
                gameEnc.PacketReady += p => gameRing.Add(p);
                tracks.Add(new AudioTrackState("System", gameEnc, gameRing));

                audio.GameDataReady += (buf, n, f) =>
                {
                    if (_pause.Paused) return;   // paused: drop audio (clock frozen) so it stays in sync
                    long t = EngineNowUs();
                    allEnc.Encode(buf, n, f.SampleRate, f.Channels, f.BitsPerSample / 8, t);   // into the mix
                    gameEnc.Encode(buf, n, f.SampleRate, f.Channels, f.BitsPerSample / 8, t);  // system-only track
                };

                // Track 2: Microphone (mic only), created on its first buffer. Also fed into the mix.
                var micTrack = new AudioTrackState("Microphone", null, null);
                AacAudioEncoder? micEnc = null;
                audio.MicDataReady += (buf, n, f) =>
                {
                    if (_pause.Paused) return;   // paused: drop mic too
                    long t = EngineNowUs();
                    allEnc.EncodeMic(buf, n, f.SampleRate, f.Channels, f.BitsPerSample / 8, MapSampleFormat(f)); // into the mix
                    if (micEnc == null)
                    {
                        micEnc = new AacAudioEncoder(f.SampleRate, f.Channels, MapSampleFormat(f), 160);
                        var micRing = new PacketRingBuffer(_options.BufferSeconds);
                        micEnc.PacketReady += p => micRing.Add(p);
                        micTrack.Encoder = micEnc; micTrack.Ring = micRing;
                    }
                    micEnc.Encode(buf, n, f.SampleRate, f.Channels, f.BitsPerSample / 8, t);    // mic-only track
                };
                tracks.Add(micTrack);
            }
            else
            {
                // Single mixed track: game loopback + mic blended in by the encoder.
                var allEnc = new AacAudioEncoder(gf.SampleRate, gf.Channels, MapSampleFormat(gf), 160);
                var allRing = new PacketRingBuffer(_options.BufferSeconds);
                allEnc.PacketReady += p => allRing.Add(p);
                audio.GameDataReady += (buf, n, f) =>
                {
                    if (_pause.Paused) return;   // paused: drop audio (clock frozen) so it stays in sync
                    allEnc.Encode(buf, n, f.SampleRate, f.Channels, f.BitsPerSample / 8, EngineNowUs());
                };
                audio.MicDataReady += (buf, n, f) =>
                {
                    if (_pause.Paused) return;
                    allEnc.EncodeMic(buf, n, f.SampleRate, f.Channels, f.BitsPerSample / 8, MapSampleFormat(f));
                };
                tracks.Add(new AudioTrackState("Audio", allEnc, allRing));
            }

            lock (_gate) { _audio = audio; _audioTracks.Clear(); _audioTracks.AddRange(tracks); }

            // Steady 100 ms pulse: advance every audio track to the engine clock even when WASAPI
            // loopback emits no callbacks (system silent / paused video / desktop) — otherwise the
            // audio stream ends at the last system sound while the video keeps going (the "audio cut
            // out halfway" bug). The pump pads silence and mixes the mic so the voice stays audible.
            _audioPumpTimer = new Timer(_ => PumpAudio(), null, 100, 100);
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[VfrReplayEngine] audio capture unavailable — video-only: {ex.Message}");
        }
    }

    /// <summary>Pulses every audio encoder up to the current engine time (called by the pump timer
    /// and once more right before save, so the tail is filled to the exact clip length).</summary>
    private void PumpAudio()
    {
        if (_disposed || !_running) return;
        long now = EngineNowUs();
        List<AudioTrackState> tracks;
        lock (_gate) { tracks = new List<AudioTrackState>(_audioTracks); }
        foreach (var t in tracks)
        {
            try { t.Encoder?.PumpSilenceTo(now); }
            catch (Exception ex) { Console.WriteLine($"[VfrReplayEngine] audio pump error: {ex.Message}"); }
        }
    }

    /// <summary>Maps the user's file-format choice to a container extension our codecs mux cleanly
    /// into. AVI can't carry HEVC/AV1/AAC well, so anything unknown falls back to mp4.</summary>
    private static string NormalizeContainer(string? fmt) => (fmt ?? "").Trim().ToLowerInvariant() switch
    {
        "mkv" => "mkv",
        "mov" => "mov",
        _ => "mp4",
    };

    private static AVSampleFormat MapSampleFormat(NAudio.Wave.WaveFormat wf) =>
        wf.Encoding == NAudio.Wave.WaveFormatEncoding.IeeeFloat ? AVSampleFormat.AV_SAMPLE_FMT_FLT
        : wf.BitsPerSample == 16 ? AVSampleFormat.AV_SAMPLE_FMT_S16
        : wf.BitsPerSample == 32 ? AVSampleFormat.AV_SAMPLE_FMT_S32
        : AVSampleFormat.AV_SAMPLE_FMT_FLT;

    /// <summary>
    /// Writes the buffered replay to the library folder. Thread-safe relative to capture; only
    /// reads the ring's thread-safe snapshot. Returns the path, or null if nothing to save.
    /// </summary>
    public string? SaveReplay()
    {
        PacketRingBuffer? ring;
        NvencVfrEncoder? encoder;
        List<AudioTrackState> tracks;
        lock (_gate) { ring = _ring; encoder = _encoder; tracks = new List<AudioTrackState>(_audioTracks); }
        if (ring == null || encoder == null)
        {
            Console.WriteLine("[VfrReplayEngine] SaveReplay: no active capture (no game?).");
            return null;
        }

        // Flush the audio timeline right up to NOW so the saved tail matches the video length
        // (otherwise the clip can end on the last system sound, short of the video).
        PumpAudio();

        var packets = ring.Snapshot(out long baseUs);
        if (packets.Count == 0) { Console.WriteLine("[VfrReplayEngine] SaveReplay: buffer empty."); return null; }

        // Each audio track, re-based to the SAME origin as the video clip so they stay in sync.
        var audioTracks = new List<Mp4Muxer.AudioTrack>();
        foreach (var t in tracks)
        {
            if (t.Encoder == null || t.Ring == null) continue;
            var ap = t.Ring.SnapshotRebasedFrom(baseUs);
            if (ap.Count > 0) audioTracks.Add(new Mp4Muxer.AudioTrack(t.Encoder.GetStreamSpec(), ap, t.Title));
        }

        string dir = _options.LibraryPath;
        System.IO.Directory.CreateDirectory(dir);
        string ext = NormalizeContainer(_options.FileFormat);
        string path = System.IO.Path.Combine(dir, $"{DateTime.Now:yyyy-MM-dd_HH-mm-ss}-Replay.{ext}");

        bool ok = Mp4Muxer.Write(path, encoder.GetStreamSpec(), packets, audioTracks);
        if (!ok) return null;

        ReplaySaved?.Invoke(this, path);
        return path;
    }

    /// <summary>Push-to-talk: mute/unmute the microphone mid-capture. No-op when video-only.</summary>
    public void SetMicMuted(bool muted)
    {
        WasapiAudioSource? a;
        lock (_gate) { a = _audio; }
        a?.SetMicMuted(muted);
    }

    /// <summary>Pauses or resumes feeding the rolling buffer. While paused, video frames and audio
    /// are dropped and the engine clock is frozen, so the paused span is absent from the timeline — a
    /// saved replay stitches the moments before and after the pause together with no held frame.
    /// No-op when not running or already in the requested state. WGC and the encoder keep running, so
    /// resume is instant (no NVENC session churn).</summary>
    public void SetPaused(bool paused)
    {
        lock (_gate)
        {
            if (_disposed || !_running) return;
            var cur = _pause;
            if (paused == cur.Paused) return;
            if (paused)
            {
                _pause = new PauseState(cur.AccumUs, System.Diagnostics.Stopwatch.GetTimestamp(), true);
            }
            else
            {
                long span = (System.Diagnostics.Stopwatch.GetTimestamp() - cur.PauseStartQpc)
                            * 1_000_000L / System.Diagnostics.Stopwatch.Frequency;
                _pause = new PauseState(cur.AccumUs + span, 0, false);
            }
            Console.WriteLine($"[VfrReplayEngine] {(paused ? "paused" : "resumed")} (total paused {_pause.AccumUs / 1000}ms).");
        }
    }

    private void TeardownCapture()
    {
        // Stop the audio pulse first so it can't touch an encoder we're about to dispose.
        _audioPumpTimer?.Dispose();
        _audioPumpTimer = null;

        WgcCaptureSource? cap; NvencVfrEncoder? enc;
        WasapiAudioSource? aud; List<AudioTrackState> tracks;
        bool wasActive;
        lock (_gate)
        {
            cap = _capture; enc = _encoder; aud = _audio;
            tracks = new List<AudioTrackState>(_audioTracks);
            _capture = null; _encoder = null; _ring = null;
            _audio = null; _audioTracks.Clear();
            _lockedHwnd = IntPtr.Zero; _lockedPid = 0; _lockKind = LockKind.None;
            _pause = new PauseState(0, 0, false);   // capture gone — clear any pause state
            wasActive = TargetActive;
            TargetActive = false; TargetIsGame = false; TargetName = ""; TargetExe = null;
        }
        if (wasActive) TargetChanged?.Invoke();
        // Step-by-step timing: a crash/hang on stop pins the blame on the exact native Dispose that
        // didn't return (capture / encoder / audio). Pairs with the native crash filter in Program.cs.
        var swTd = System.Diagnostics.Stopwatch.StartNew();
        Console.WriteLine("[Teardown] begin");
        try { if (cap != null) cap.FrameReady -= OnFrame; } catch { }
        cap?.Dispose();   Console.WriteLine($"[Teardown] capture disposed @ {swTd.ElapsedMilliseconds}ms");
        enc?.Flush();     Console.WriteLine($"[Teardown] encoder flushed @ {swTd.ElapsedMilliseconds}ms");
        enc?.Dispose();   Console.WriteLine($"[Teardown] encoder disposed @ {swTd.ElapsedMilliseconds}ms");
        aud?.Dispose();   Console.WriteLine($"[Teardown] audio disposed @ {swTd.ElapsedMilliseconds}ms");   // stop WASAPI first so no more PCM arrives
        foreach (var t in tracks) { t.Encoder?.Flush(); t.Encoder?.Dispose(); }
        Console.WriteLine($"[Teardown] complete @ {swTd.ElapsedMilliseconds}ms");
        SetProcessPriority(System.Diagnostics.ProcessPriorityClass.Normal);
        try { System.Runtime.GCSettings.LatencyMode = System.Runtime.GCLatencyMode.Interactive; } catch { }
    }

    private static void SetProcessPriority(System.Diagnostics.ProcessPriorityClass cls)
    {
        try { System.Diagnostics.Process.GetCurrentProcess().PriorityClass = cls; }
        catch (Exception ex) { Console.WriteLine($"[VfrReplayEngine] priority set failed: {ex.Message}"); }
    }

    public void Stop()
    {
        _running = false;
        _lockTimer?.Dispose();
        _lockTimer = null;
        TeardownCapture();
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        Stop();
        _d3d?.Dispose();
        _d3d = null;
    }

    private static bool IsProcessAlive(uint pid)
    {
        if (pid == 0) return false;
        try { using var p = System.Diagnostics.Process.GetProcessById((int)pid); return !p.HasExited; }
        catch { return false; }
    }

    // ─── window plumbing (game DETECTION + scoring now lives in GameDetector) ───

    [DllImport("user32.dll")] private static extern bool IsWindow(IntPtr hWnd);
    [DllImport("user32.dll")] private static extern bool GetWindowRect(IntPtr hWnd, out Win32Rect rect);
    [DllImport("user32.dll")] private static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint processId);

    private delegate bool MonitorEnumProc(IntPtr hMonitor, IntPtr hdc, ref Win32Rect rect, IntPtr data);
    [DllImport("user32.dll")] private static extern bool EnumDisplayMonitors(IntPtr hdc, IntPtr clip, MonitorEnumProc cb, IntPtr data);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] private static extern bool GetMonitorInfoW(IntPtr hMonitor, ref MonitorInfoEx info);

    [StructLayout(LayoutKind.Sequential)]
    private struct Win32Rect { public int Left, Top, Right, Bottom; }

    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    private struct MonitorInfoEx
    {
        public uint Size;
        public Win32Rect Monitor;
        public Win32Rect Work;
        public uint Flags;
        [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string Device;
    }
}
