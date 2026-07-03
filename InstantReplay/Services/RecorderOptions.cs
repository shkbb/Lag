using System.Collections.Generic;

namespace Lag.Services;

/// <summary>
/// Full configuration snapshot for one recording session. Built by the UI layer from the current
/// settings and consumed by <see cref="IReplayRecorder.Initialize(RecorderOptions)"/> on every
/// (cold-restart) Start, so all options reliably apply.
/// </summary>
public sealed class RecorderOptions
{
    // ── Video ──
    public int BufferSeconds { get; init; } = 60;
    public int FrameRate { get; init; } = 30;

    /// <summary>Native capture size (the monitor's resolution; never cropped).</summary>
    public uint Width { get; init; }
    public uint Height { get; init; }

    /// <summary>Encoded output size; 0 = same as capture (no downscale).</summary>
    public uint OutputWidth { get; init; }
    public uint OutputHeight { get; init; }

    /// <summary>Video encoder bitrate in kbps (e.g. 20000 = 20 Mbps).</summary>
    public int VideoBitrateKbps { get; init; } = 20000;

    /// <summary>DXGI adapter index to render/capture on (multi-GPU systems). 0 = primary.</summary>
    public int AdapterIndex { get; init; }

    /// <summary>Preferred encoder id; null/empty = automatic hardware fallback chain.</summary>
    public string? PreferredEncoder { get; init; }

    public string? MonitorId { get; init; }
    public string? LibraryPath { get; init; }

    /// <summary>Output container format: "mp4" (default), "mkv", "mov" or "avi".</summary>
    public string FileFormat { get; init; } = "mp4";

    // ── Audio ──
    public string? MicrophoneId { get; init; }

    /// <summary>Linear mic volume, 1.0 = 100%.</summary>
    public float MicVolume { get; init; } = 1.0f;

    /// <summary>Downmix the microphone to mono.</summary>
    public bool MicForceMono { get; init; }

    /// <summary>Noise gate ("input sensitivity"): auto = track the noise floor; false = use the threshold.</summary>
    public bool InputGateAuto { get; init; } = true;

    /// <summary>Manual gate threshold, 0..1 on the meter's perceptual scale (used when not auto).</summary>
    public float InputGateThreshold { get; init; } = 0.08f;

    /// <summary>RNNoise mic noise suppression (FFmpeg arnndn). Applies to recording AND "hear yourself".</summary>
    public bool NoiseSuppression { get; init; }

    /// <summary>Start with the mic muted (push-to-talk holds it muted until the key is pressed).</summary>
    public bool MicStartMuted { get; init; }

    /// <summary>"all" = whole desktop audio; "apps" = only the selected applications.</summary>
    public string AudioCaptureMode { get; init; } = "all";

    /// <summary>When true, the engine switches the audio mode by what it records: a game → selected
    /// apps, the desktop → whole-PC audio. Overrides <see cref="AudioCaptureMode"/> at record time.</summary>
    public bool AudioModeByTarget { get; init; }

    /// <summary>Render endpoint id to loop back for whole-PC audio; null/empty = default output.</summary>
    public string? SystemAudioDeviceId { get; init; }

    /// <summary>Per-application capture entries (used when <see cref="AudioCaptureMode"/> is "apps").</summary>
    public IReadOnlyList<AppAudioCapture> AudioApps { get; init; } = [];

    /// <summary>Save system audio and microphone as two separate tracks in the output file.</summary>
    public bool SeparateAudioTracks { get; init; }

    /// <summary>Capture the full-system audio (the "Звук системи" row). When false, no system audio.</summary>
    public bool SystemAudioEnabled { get; init; } = true;

    /// <summary>System-audio volume in percent (0–100).</summary>
    public int SystemAudioVolume { get; init; } = 100;

    /// <summary>Capture the detected game's own audio (the "Звук гри" row) in specific-apps mode.</summary>
    public bool GameAudioEnabled { get; init; } = true;

    /// <summary>Game-audio volume in percent (0–100).</summary>
    public int GameAudioVolume { get; init; } = 100;

    // ── Overlay (baked into recorded frames). Positions are fractions of the free space
    // (0 = left/top, 1 = right/bottom); scales are element height / frame height. ──

    public bool WebcamOverlay { get; init; }

    /// <summary>Camera device id (null/empty = system default).</summary>
    public string? WebcamDeviceId { get; init; }

    public double WebcamX { get; init; } = 0.97;
    public double WebcamY { get; init; } = 0.95;
    public double WebcamScale { get; init; } = 0.24;

    public bool KeysOverlay { get; init; }
    public double KeysX { get; init; } = 0.03;
    public double KeysY { get; init; } = 0.95;
    public double KeysScale { get; init; } = 0.20;

    /// <summary>Steam-style resource monitor panel; detail: 0 = FPS, 1 = + CPU/GPU, 2 = + RAM.</summary>
    public bool StatsOverlay { get; init; }
    public double StatsX { get; init; } = 0.5;
    public double StatsY { get; init; } = 0.02;
    public double StatsScale { get; init; } = 0.045;
    public int StatsDetail { get; init; } = 1;
}

/// <summary>One application whose audio should be captured, with its linear volume (1.0 = 100%).</summary>
public sealed record AppAudioCapture(string ExeName, float Volume);
