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
}
