using System;

namespace Lag.Services.VfrCapture;

/// <summary>
/// A noise gate ("input sensitivity"): when the mic level is below a threshold the signal is muted,
/// so background hiss / typing / quiet room tone don't end up in the clip; above it, the mic passes.
/// A hold + attack/release envelope keeps it from chopping word tails or stuttering on brief dips.
///
/// In <see cref="Auto"/> mode the threshold tracks the room's noise floor and sits a little above it
/// (like Discord's automatic sensitivity); otherwise a fixed <see cref="ManualThreshold"/> is used.
/// Levels are 0..1 on the SAME perceptual scale the meter shows, so the UI marker lines up with where
/// your voice visually crosses the threshold. Single-threaded by design (one gate per audio thread).
/// </summary>
public sealed class NoiseGate
{
    /// <summary>Automatic threshold (tracks the noise floor) vs a fixed <see cref="ManualThreshold"/>.</summary>
    public bool Auto { get; set; } = true;

    /// <summary>Manual threshold, 0..1 on the meter's perceptual scale (used when <see cref="Auto"/> is off).</summary>
    public float ManualThreshold { get; set; } = 0.08f;

    /// <summary>The threshold actually in effect right now (for the UI marker).</summary>
    public float Threshold { get; private set; }

    /// <summary>Whether the gate is currently passing audio (for the UI "transmitting" colour).</summary>
    public bool IsOpen { get; private set; }

    private float _noiseFloor = 0.02f;   // auto: slow estimate of the quiet baseline
    private float _gain;                 // current smoothed gate gain, 0..1
    private float _holdRemaining;        // seconds the gate stays open after dropping below threshold

    private const float HoldSeconds = 0.28f;    // keep open briefly after you stop — don't clip word tails
    private const float AttackPerSec = 70f;     // gain rise speed (fast, so onsets aren't lost)
    private const float ReleasePerSec = 7f;     // gain fall speed (slow/smooth, no clicks)
    private const float AutoMargin = 2.3f;      // auto threshold = noiseFloor × margin (≈ +7 dB)
    private const float AutoFloorFall = 2.5f;   // noise floor follows DOWN quickly when the room gets quieter
    private const float AutoFloorRise = 0.03f;  // …and UP very slowly, so speech doesn't drag it up

    /// <summary>Feeds one level reading (0..1, perceptual) measured over <paramref name="dt"/> seconds and
    /// returns the gate gain to apply, 0..1 (smoothed — safe to ramp samples toward it).</summary>
    public float Process(float level, double dt)
    {
        float t = (float)Math.Clamp(dt, 0.0001, 0.5);

        if (Auto)
        {
            // Noise floor ≈ the recent quiet baseline: drop toward the level fast, rise toward it slowly,
            // so persistent room noise raises it but transient speech doesn't.
            float rate = level < _noiseFloor ? AutoFloorFall : AutoFloorRise;
            _noiseFloor += (level - _noiseFloor) * Math.Min(1f, rate * t);
            _noiseFloor = Math.Clamp(_noiseFloor, 0.004f, 0.5f);
            Threshold = Math.Clamp(_noiseFloor * AutoMargin, 0.012f, 0.9f);
        }
        else
        {
            Threshold = Math.Clamp(ManualThreshold, 0f, 1f);
        }

        bool above = level >= Threshold;
        _holdRemaining = above ? HoldSeconds : Math.Max(0f, _holdRemaining - t);
        IsOpen = above || _holdRemaining > 0f;

        float target = IsOpen ? 1f : 0f;
        _gain += (target - _gain) * Math.Min(1f, (target > _gain ? AttackPerSec : ReleasePerSec) * t);
        if (_gain < 0.0005f) _gain = 0f;
        return _gain;
    }

    /// <summary>Clears the envelope/floor state (e.g. on a new capture session).</summary>
    public void Reset()
    {
        _gain = 0f;
        _holdRemaining = 0f;
        _noiseFloor = 0.02f;
    }
}
