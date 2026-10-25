using System;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Regularises frame timestamps onto a fixed output cadence — the "VFR stabiliser" step.
///
/// Raw frame times are uneven: a game's frame intervals wobble by a millisecond or two (more once
/// delivery jitter is added), and a player showing those raw values against its own vsync holds one
/// frame for two refreshes and the next for one. That reads as micro-stutter even though nothing was
/// dropped.
///
/// Measured against the reference recorder (a Medal CS2 clip, 2026-09-11): its frame intervals take
/// only THREE distinct values — one grid step, two steps where a frame was missed, and the seam —
/// with 92% of frames within 10% of the median. The same footage captured by us, written with raw
/// times, had 846 distinct interval lengths and only 14% near the median. Snapping is what closes
/// that gap.
///
/// Each frame is placed in the NEAREST slot, so it never moves more than half a step from when it
/// really happened — the cadence is regularised, not invented. A frame landing in a slot that is
/// already taken is dropped (this is also how the frame-rate cap is enforced), and a slot nothing
/// lands in simply leaves a double-length interval, exactly as a genuinely missed frame should.
///
/// Driven from the single capture thread; only <see cref="TakeCollisions"/> may be called from
/// another thread (the stats logger), so the collision counter is updated atomically.
/// </summary>
internal sealed class CadenceGrid
{
    private readonly long _stepUs;    // nominal slot spacing, for reporting
    private readonly long _fps;       // slots per second — slot positions are derived from this
    private long _originUs;
    private bool _pinned;
    private long _lastSlot = long.MinValue;

    /// <summary>Sentinel returned by <see cref="Snap"/> for a frame whose slot is already filled.</summary>
    public const long Drop = -1;

    /// <summary>Frames dropped because their slot was taken, since the last <see cref="TakeCollisions"/>.</summary>
    private long _collisions;

    /// <param name="fps">Slots per second (the frame-rate cap). 0 or less disables snapping.</param>
    public CadenceGrid(long fps)
    {
        _fps = fps;
        _stepUs = fps > 0 ? 1_000_000L / fps : 0;
    }

    /// <summary>True when this grid actually snaps (i.e. a frame-rate cap is configured).</summary>
    public bool Enabled => _fps > 0;

    /// <summary>Slots per second — the cadence being produced.</summary>
    public double Fps => _fps;

    /// <summary>Re-anchors the grid on the next frame. Used whenever the timeline moves under us —
    /// a new capture session, or a resume after a pause.</summary>
    public void Repin()
    {
        _pinned = false;
        _lastSlot = long.MinValue;
    }

    /// <summary>Reads and clears the collision counter (for the periodic capture log).</summary>
    public long TakeCollisions()
    {
        return System.Threading.Interlocked.Exchange(ref _collisions, 0);
    }

    /// <summary>
    /// Returns <paramref name="ptsUs"/> snapped to the grid, or <see cref="Drop"/> when this
    /// frame belongs to a slot that already has one.
    /// </summary>
    public long Snap(long ptsUs)
    {
        if (_fps <= 0) return ptsUs;

        if (!_pinned)
        {
            _originUs = ptsUs;
            _pinned = true;
            _lastSlot = long.MinValue;
        }

        // Slot index and position are both derived from fps rather than from a rounded step, so a
        // rate whose period is not a whole number of microseconds (165fps = 6060.6µs) cannot
        // accumulate drift against the audio clock over a long buffer.
        long delta = ptsUs - _originUs;
        long slot = delta >= 0 ? (delta * _fps + 500_000L) / 1_000_000L
                               : (delta * _fps - 500_000L) / 1_000_000L;

        if (_lastSlot != long.MinValue && slot <= _lastSlot)
        {
            System.Threading.Interlocked.Increment(ref _collisions);
            return Drop;
        }

        _lastSlot = slot;
        return _originUs + (slot * 1_000_000L) / _fps;
    }
}
