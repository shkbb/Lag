namespace Lag.Services;

/// <summary>
/// Represents information about a connected display monitor.
/// </summary>
public record MonitorInfo(
    int Index,
    string DeviceName,
    int Width,
    int Height,
    bool IsPrimary
)
{
    public override string ToString() =>
        $"{DeviceName} ({Width}x{Height}){(IsPrimary ? " [Primary]" : "")}";
}

/// <summary>
/// Represents a single captured frame of raw pixel data.
/// </summary>
public class CapturedFrame : IDisposable
{
    /// <summary>Raw BGRA pixel data.</summary>
    public byte[] Data { get; init; } = Array.Empty<byte>();

    /// <summary>Frame width in pixels.</summary>
    public int Width { get; init; }

    /// <summary>Frame height in pixels.</summary>
    public int Height { get; init; }

    /// <summary>Timestamp when the frame was captured.</summary>
    public DateTimeOffset Timestamp { get; init; }

    public void Dispose() { /* Data is managed, no unmanaged cleanup needed */ }
}

/// <summary>
/// Platform-agnostic interface for screen capture.
/// Implementations handle DXGI (Windows) or X11/PipeWire (Linux).
/// 
/// Usage Pattern:
///   1. Call <see cref="GetAvailableMonitors"/> to enumerate displays.
///   2. Call <see cref="Initialize"/> with the desired monitor index.
///   3. Call <see cref="AcquireNextFrame"/> in a loop to capture frames.
///   4. Call <see cref="Shutdown"/> when done.
/// </summary>
public interface IScreenCaptureService : IDisposable
{
    /// <summary>
    /// Enumerates all available monitors on the system.
    /// </summary>
    IReadOnlyList<MonitorInfo> GetAvailableMonitors();

    /// <summary>
    /// Initializes the capture session targeting the specified monitor.
    /// </summary>
    /// <param name="monitorIndex">Zero-based index from <see cref="GetAvailableMonitors"/>.</param>
    void Initialize(int monitorIndex);

    /// <summary>
    /// Acquires the next desktop frame. Returns null if no new frame is available
    /// within the timeout period (e.g., static desktop).
    /// </summary>
    /// <param name="timeoutMs">Maximum time to wait for a new frame.</param>
    /// <returns>The captured frame, or null if timeout elapsed without a new frame.</returns>
    CapturedFrame? AcquireNextFrame(int timeoutMs = 100);

    /// <summary>
    /// Releases capture resources. Safe to call multiple times.
    /// </summary>
    void Shutdown();

    /// <summary>
    /// Whether the capture session is actively initialized and ready.
    /// </summary>
    bool IsInitialized { get; }
}
