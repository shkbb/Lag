using System.Collections.Generic;

namespace Lag.Services.ObsIntegration;

/// <summary>
/// Full configuration snapshot for one recording session. Built by the UI layer from the current
/// settings and consumed by <see cref="ObsRecorderService.Initialize(RecorderOptions)"/> on every
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

    /// <summary>Start with the mic muted (push-to-talk holds it muted until the key is pressed).</summary>
    public bool MicStartMuted { get; init; }

    /// <summary>"all" = whole desktop audio; "apps" = only the selected applications.</summary>
    public string AudioCaptureMode { get; init; } = "all";

    /// <summary>Per-application capture entries (used when <see cref="AudioCaptureMode"/> is "apps").</summary>
    public IReadOnlyList<AppAudioCapture> AudioApps { get; init; } = [];

    /// <summary>Save system audio and microphone as two separate tracks in the output file.</summary>
    public bool SeparateAudioTracks { get; init; }
}

/// <summary>One application whose audio should be captured, with its linear volume (1.0 = 100%).</summary>
public sealed record AppAudioCapture(string ExeName, float Volume);
