using System;
using Lag.Services.ObsIntegration;

namespace Lag.Services;

/// <summary>
/// The recorder contract the UI talks to, so the app can swap capture back-ends without the
/// ViewModels caring which one runs. Two implementations:
///   • <see cref="ObsIntegration.ObsRecorderService"/> — the mature OBS replay-buffer path (default).
///   • <see cref="VfrCapture.VfrRecorderAdapter"/> — the native WGC/NVENC VFR engine.
///
/// Lifecycle mirrors the OBS cold-restart model the UI already drives: Initialize() with a fresh
/// options snapshot, StartBuffer() to arm the rolling buffer, SaveReplay() on the hotkey, Teardown()
/// to stop. <see cref="ReplaySaved"/> fires (path) once a clip is written.
/// </summary>
public interface IReplayRecorder : IDisposable
{
    /// <summary>Whether the rolling buffer is currently armed/capturing.</summary>
    bool IsRecording { get; }

    /// <summary>Raised after a replay file is written. Arg is the absolute path.</summary>
    event EventHandler<string>? ReplaySaved;

    /// <summary>Applies a full options snapshot. Called on every (cold-restart) start.</summary>
    void Initialize(RecorderOptions options);

    /// <summary>Arms the rolling replay buffer.</summary>
    void StartBuffer();

    /// <summary>Writes the buffered replay to the library (the hotkey action).</summary>
    void SaveReplay();

    /// <summary>Fully tears down the capture pipeline (cold restart re-reads all settings).</summary>
    void Teardown();

    /// <summary>Mutes/unmutes the microphone (push-to-talk). No-op if there is no mic source.</summary>
    void SetMicMuted(bool muted);

    /// <summary>Whether the rolling buffer feed is currently paused. Default false for back-ends that
    /// don't support pausing.</summary>
    bool IsPaused => false;

    /// <summary>Temporarily suspends feeding the rolling buffer (the native engine freezes the A/V
    /// timeline so the paused span is absent from any saved replay) or resumes it. Default no-op for
    /// back-ends that don't support pausing.</summary>
    void SetPaused(bool paused) { }

    /// <summary>What's being captured right now (game/desktop), or null when idle. Default null so
    /// back-ends that don't report it (OBS) need no changes; the native engine overrides it.</summary>
    CaptureTarget? CurrentTarget => null;

    /// <summary>Raised when <see cref="CurrentTarget"/> changes. Default no-op for back-ends that
    /// don't report a target.</summary>
    event EventHandler? CaptureTargetChanged
    {
        add { }
        remove { }
    }
}

/// <summary>The live capture target shown in the UI indicator. <paramref name="IsGame"/> false = a
/// desktop/monitor capture; <paramref name="Name"/> is the game's friendly name (empty for desktop —
/// the UI localizes that); <paramref name="Exe"/> is the process exe, for icon lookup.</summary>
public readonly record struct CaptureTarget(bool IsGame, string Name, string? Exe);
