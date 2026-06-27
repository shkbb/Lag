using System;
using System.Collections.Generic;
using System.IO;
using System.Linq;
using Lag.Services.ObsIntegration;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Adapts the native <see cref="VfrReplayEngine"/> to the app's <see cref="IReplayRecorder"/>
/// contract, so the UI can drive it exactly like the OBS recorder. Translates the UI's
/// <see cref="RecorderOptions"/> snapshot into <see cref="VfrEngineOptions"/> on each start.
/// </summary>
public sealed class VfrRecorderAdapter : IReplayRecorder
{
    private readonly VfrReplayEngine _engine = new();
    private VfrEngineOptions? _opts;

    public event EventHandler<string>? ReplaySaved;
    public bool IsRecording => _engine.IsRunning;

    public event EventHandler? CaptureTargetChanged;
    public CaptureTarget? CurrentTarget =>
        _engine.TargetActive ? new CaptureTarget(_engine.TargetIsGame, _engine.TargetName, _engine.TargetExe) : null;

    public VfrRecorderAdapter()
    {
        _engine.ReplaySaved += (_, path) => ReplaySaved?.Invoke(this, path);
        _engine.TargetChanged += () => CaptureTargetChanged?.Invoke(this, EventArgs.Empty);
    }

    /// <summary>Whether the native engine can run on this machine (WGC + FFmpeg + an encoder).
    /// The factory falls back to OBS when false.</summary>
    public static bool IsAvailable()
    {
        try { return VfrReplayEngine.IsAvailable(FfmpegDir()); }
        catch { return false; }
    }

    private static string FfmpegDir() => Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "ffmpeg"));

    public void Initialize(RecorderOptions options) => _opts = Map(options);

    public void StartBuffer()
    {
        if (_opts != null) _engine.Start(_opts);
    }

    public void SaveReplay() => _engine.SaveReplay();
    public void Teardown() => _engine.Stop();
    public void SetMicMuted(bool muted) => _engine.SetMicMuted(muted);
    public bool IsPaused => _engine.IsPaused;
    public void SetPaused(bool paused) => _engine.SetPaused(paused);
    public void Dispose() => _engine.Dispose();

    private static VfrEngineOptions Map(RecorderOptions o) => new()
    {
        LibraryPath = o.LibraryPath ?? "",
        BufferSeconds = o.BufferSeconds,
        PreferredCodec = MapCodec(o.PreferredEncoder),
        BitrateKbps = o.VideoBitrateKbps,
        FileFormat = string.IsNullOrWhiteSpace(o.FileFormat) ? "mp4" : o.FileFormat,
        TargetHeight = (int)o.OutputHeight,          // 0 = native (no downscale)
        MaxFps = o.FrameRate > 0 ? o.FrameRate : 120,
        MonitorDeviceId = o.MonitorId,
        AdapterIndex = o.AdapterIndex,
        MicrophoneId = o.MicrophoneId,
        MicVolume = o.MicVolume,
        MicStartMuted = o.MicStartMuted,
        SeparateAudioTracks = o.SeparateAudioTracks,
        // "Обрані програми": capture only the selected apps' audio (process loopback) instead of the
        // whole system. o.AudioApps is already filtered to the enabled entries by the UI layer.
        AppsAudioMode = string.Equals(o.AudioCaptureMode, "apps", StringComparison.OrdinalIgnoreCase),
        AudioModeByTarget = o.AudioModeByTarget,
        AudioApps = (o.AudioApps ?? new List<AppAudioCapture>())
            .Select(a => (a.ExeName, a.Volume)).ToList(),
        SystemAudioEnabled = o.SystemAudioEnabled,
        SystemAudioDeviceId = o.SystemAudioDeviceId,
        SystemVolume = o.SystemAudioVolume / 100f,
        GameAudioEnabled = o.GameAudioEnabled,
        GameVolume = o.GameAudioVolume / 100f,
        MicMono = o.MicForceMono,
        CaptureCursor = true,
    };

    /// <summary>Maps the UI's codec FORMAT choice ("h264" / "hevc" / "av1"; "" = Auto) to the codec
    /// family the <see cref="EncoderSelector"/> understands. Vendor selection is automatic by design:
    /// the engine picks the best available encoder (NVENC / AMF / QSV, x264 CPU fallback) for the
    /// chosen family — the dropdown deliberately offers formats, not specific backends.</summary>
    private static string? MapCodec(string? encoder)
    {
        if (string.IsNullOrEmpty(encoder)) return null;   // Auto → engine picks family AND vendor
        string e = encoder.ToLowerInvariant();
        if (e.Contains("av1")) return "av1";
        if (e.Contains("hevc") || e.Contains("h265") || e.Contains("265")) return "hevc";
        if (e.Contains("264")) return "h264";
        if (e.Contains("x264") || e.Contains("cpu")) return "x264";   // defensive: explicit force-CPU id, if ever exposed
        return null;
    }
}
