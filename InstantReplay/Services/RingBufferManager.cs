namespace Lag.Services;

/// <summary>
/// Manages a ring buffer of video segment files for continuous background recording.
/// Old segments are automatically deleted when the buffer exceeds the configured capacity.
/// 
/// Thread Safety:
///   All public methods are thread-safe. Internal state is guarded by a lock
///   to safely handle concurrent segment additions (recording thread) and
///   segment reads (save replay on hotkey press from UI thread).
/// 
/// Segment Lifecycle:
///   1. Recording thread calls <see cref="AddSegment"/> with the path to a completed segment.
///   2. If the buffer is full, the oldest segment file is deleted from disk.
///   3. On hotkey press, <see cref="GetSegmentsForDuration"/> returns an ordered snapshot
///      of the most recent segment files covering the requested replay duration.
/// </summary>
public sealed class RingBufferManager
{
    private readonly LinkedList<SegmentInfo> _segments = new();
    private readonly object _lock = new();

    /// <summary>
    /// Maximum number of segments to retain in the ring buffer.
    /// Calculated as: (BufferDuration / SegmentDuration).
    /// Example: 5 minute buffer with 5-second segments = 60 segments.
    /// </summary>
    public int MaxSegments { get; set; } = 60;

    /// <summary>
    /// Duration of each individual video segment.
    /// Must match the FFmpeg segment_time parameter.
    /// </summary>
    public TimeSpan SegmentDuration { get; set; } = TimeSpan.FromSeconds(5);

    /// <summary>
    /// Adds a completed video segment to the ring buffer.
    /// If the buffer is full, the oldest segment is removed and its file is deleted from disk.
    /// </summary>
    /// <param name="filePath">Absolute path to the segment .mp4 file.</param>
    /// <param name="duration">Actual duration of this segment (may differ slightly from target).</param>
    public void AddSegment(string filePath, TimeSpan? duration = null)
    {
        var segment = new SegmentInfo
        {
            FilePath = filePath,
            Duration = duration ?? SegmentDuration,
            CreatedAt = DateTimeOffset.UtcNow
        };

        lock (_lock)
        {
            _segments.AddLast(segment);

            // Evict oldest segments when over capacity
            while (_segments.Count > MaxSegments)
            {
                var oldest = _segments.First!.Value;
                _segments.RemoveFirst();

                // Clean up the old segment file from disk
                TryDeleteFile(oldest.FilePath);
            }
        }
    }

    /// <summary>
    /// Returns an ordered list of the most recent segment file paths
    /// that together cover at least the requested duration.
    /// Returns a snapshot — safe to use even while new segments are added.
    /// </summary>
    /// <param name="duration">Desired replay duration (e.g., 2 minutes).</param>
    /// <returns>Ordered list of segment file paths, oldest to newest.</returns>
    public IReadOnlyList<string> GetSegmentsForDuration(TimeSpan duration)
    {
        lock (_lock)
        {
            var result = new List<string>();
            var accumulated = TimeSpan.Zero;

            // Walk backwards from the newest segment
            var node = _segments.Last;
            while (node != null && accumulated < duration)
            {
                result.Add(node.Value.FilePath);
                accumulated += node.Value.Duration;
                node = node.Previous;
            }

            // Reverse to get chronological order (oldest first)
            result.Reverse();
            return result;
        }
    }

    /// <summary>
    /// Returns the total accumulated duration of all segments in the buffer.
    /// </summary>
    public TimeSpan GetBufferedDuration()
    {
        lock (_lock)
        {
            var total = TimeSpan.Zero;
            foreach (var segment in _segments)
                total += segment.Duration;
            return total;
        }
    }

    /// <summary>
    /// Returns the current number of segments in the ring buffer.
    /// </summary>
    public int SegmentCount
    {
        get { lock (_lock) { return _segments.Count; } }
    }

    /// <summary>
    /// Clears all segments from the buffer and deletes their files from disk.
    /// Called during shutdown or when changing buffer settings.
    /// </summary>
    public void Clear()
    {
        lock (_lock)
        {
            foreach (var segment in _segments)
                TryDeleteFile(segment.FilePath);

            _segments.Clear();
        }
    }

    /// <summary>
    /// Safely attempts to delete a segment file from disk.
    /// Logs failures but does not throw — deletion failures are non-critical.
    /// </summary>
    private static void TryDeleteFile(string path)
    {
        try
        {
            if (File.Exists(path))
                File.Delete(path);
        }
        catch (IOException)
        {
            // File may be locked by FFmpeg still closing. Non-fatal.
        }
    }

    /// <summary>
    /// Internal record for tracking segment metadata.
    /// </summary>
    private record SegmentInfo
    {
        public string FilePath { get; init; } = string.Empty;
        public TimeSpan Duration { get; init; }
        public DateTimeOffset CreatedAt { get; init; }
    }
}
