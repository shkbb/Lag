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

    public VfrRecorderAdapter()
    {
        _engine.ReplaySaved += (_, path) => ReplaySaved?.Invoke(this, path);
    }

    /// <summary>Whether the native engine can run on this machine (WGC + FFmpeg + an encoder).
    /// The factory falls back to OBS when false.</summary>
    public static bool IsAvailable()
    {
        try { return VfrReplayEngine.IsAvailable(ObsCoreDir()); }
        catch { return false; }
    }

    private static string ObsCoreDir() => Path.GetFullPath(Path.Combine(AppContext.BaseDirectory, "obs-core"));

    public void Initialize(RecorderOptions options) => _opts = Map(options);

    public void StartBuffer()
    {
        if (_opts != null) _engine.Start(_opts);
    }

    public void SaveReplay() => _engine.SaveReplay();
    public void Teardown() => _engine.Stop();
    public void SetMicMuted(bool muted) => _engine.SetMicMuted(muted);
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
        AudioApps = (o.AudioApps ?? new List<AppAudioCapture>())
            .Select(a => (a.ExeName, a.Volume)).ToList(),
        SystemAudioEnabled = o.SystemAudioEnabled,
        SystemVolume = o.SystemAudioVolume / 100f,
        GameAudioEnabled = o.GameAudioEnabled,
        GameVolume = o.GameAudioVolume / 100f,
        MicMono = o.MicForceMono,
        CaptureCursor = true,
    };

    /// <summary>Maps an OBS encoder id (e.g. "obs_nvenc_h264_tex") to the VFR codec family the
    /// selector understands; null = let the engine auto-pick the best available.</summary>
    private static string? MapCodec(string? encoder)
    {
        if (string.IsNullOrEmpty(encoder)) return null;
        string e = encoder.ToLowerInvariant();
        // The UI codec dropdown picks a BACKEND/vendor (ffmpeg_nvenc / ffmpeg_amf / obs_qsv11 /
        // obs_x264), not a codec family — map to the vendor tag EncoderSelector honors. (x264 first:
        // "obs_x264" also contains "264".)
        if (e.Contains("x264") || e.Contains("cpu")) return "x264";
        if (e.Contains("nvenc")) return "nvenc";
        if (e.Contains("amf")) return "amf";
        if (e.Contains("qsv")) return "qsv";
        // Legacy explicit codec families, if ever passed.
        if (e.Contains("av1")) return "av1";
        if (e.Contains("hevc") || e.Contains("h265") || e.Contains("265")) return "hevc";
        if (e.Contains("264")) return "h264";
        return null;
    }
}
