using System.Diagnostics;
using System.Runtime.InteropServices;

namespace Lag.Services;

/// <summary>
/// Orchestrates the complete recording pipeline: screen capture → FFmpeg encoding → ring buffer.
/// Provides async start/stop and instant replay save triggered by the global hotkey.
/// 
/// Pipeline Architecture:
///   Windows: ScreenCaptureService (DXGI) → raw frames piped to FFmpeg stdin → segmented .mp4 files → RingBuffer
///   Linux:   FFmpeg x11grab captures AND encodes directly → segmented .mp4 files → RingBuffer
/// 
/// Thread Model:
///   - The capture loop runs on a dedicated background thread to avoid blocking the UI.
///   - A FileSystemWatcher monitors the temp directory for new segments to add to the ring buffer.
///   - SaveReplayAsync() snapshots the ring buffer and stitches on a ThreadPool thread.
/// </summary>
public sealed class RecordingEngine : IDisposable
{
    private readonly IScreenCaptureService _captureService;
    private readonly FFmpegService _ffmpegService;
    private readonly RingBufferManager _ringBuffer;

    private CancellationTokenSource? _cts;
    private Task? _captureTask;
    private FileSystemWatcher? _segmentWatcher;
    private bool _disposed;

    /// <summary>Selected monitor index for capture.</summary>
    public int MonitorIndex { get; set; }

    /// <summary>Target frame rate for recording.</summary>
    public int FrameRate { get; set; } = 30;

    /// <summary>FFmpeg encoder name (e.g., "h264_nvenc").</summary>
    public string Codec { get; set; } = "h264_nvenc";

    /// <summary>Duration of each segment in seconds.</summary>
    public int SegmentDurationSeconds { get; set; } = 5;

    /// <summary>Replay buffer duration.</summary>
    public TimeSpan BufferDuration { get; set; } = TimeSpan.FromMinutes(5);

    /// <summary>Temporary directory for segment files.</summary>
    public string TempDirectory { get; set; } = Path.Combine(Path.GetTempPath(), "Lag_Segments");

    /// <summary>Permanent directory for saved replay clips.</summary>
    public string LibraryDirectory { get; set; } = Path.Combine(
        Environment.GetFolderPath(Environment.SpecialFolder.MyVideos), "Lag");

    /// <summary>Whether the recording engine is currently active.</summary>
    public bool IsRecording { get; private set; }

    /// <summary>Fired when recording state changes.</summary>
    public event EventHandler<bool>? RecordingStateChanged;

    /// <summary>Fired when a replay is successfully saved. Provides the output file path.</summary>
    public event EventHandler<string>? ReplaySaved;

    /// <summary>Fired when an error occurs during recording or saving.</summary>
    public event EventHandler<string>? ErrorOccurred;

    public RecordingEngine(
        IScreenCaptureService captureService,
        FFmpegService ffmpegService,
        RingBufferManager ringBuffer)
    {
        _captureService = captureService;
        _ffmpegService = ffmpegService;
        _ringBuffer = ringBuffer;
    }

    /// <summary>
    /// Starts background recording. Initializes the capture service, starts the FFmpeg
    /// encoder, and begins the segment monitoring loop.
    /// </summary>
    public async Task StartAsync()
    {
        if (IsRecording) return;

        try
        {
            await Task.Run(() =>
            {
                // Configure ring buffer capacity based on buffer duration and segment length
                _ringBuffer.MaxSegments = (int)(BufferDuration.TotalSeconds / SegmentDurationSeconds) + 1;
                _ringBuffer.SegmentDuration = TimeSpan.FromSeconds(SegmentDurationSeconds);

                // Initialize capture (can take seconds on Optimus laptops)
                _captureService.Initialize(MonitorIndex);

                // Clean up any leftover temp files
                if (Directory.Exists(TempDirectory))
                    Directory.Delete(TempDirectory, true);
                Directory.CreateDirectory(TempDirectory);

                _cts = new CancellationTokenSource();

                // Start the segment file watcher BEFORE starting the encoder
                StartSegmentWatcher();

                StartWindowsPipeline();
            });

            IsRecording = true;
            RecordingStateChanged?.Invoke(this, true);
        }
        catch (System.ComponentModel.Win32Exception)
        {
            ErrorOccurred?.Invoke(this, "FFmpeg не знайдено! Перевірте шлях у 'Налаштуваннях' або додайте до PATH.");
            await FallbackStopAsync();
        }
        catch (Exception ex)
        {
            ErrorOccurred?.Invoke(this, $"Failed to start recording: {ex.Message}");
            await FallbackStopAsync();
        }
    }

    /// <summary>
    /// Starts the Windows capture pipeline: DXGI frames piped to FFmpeg stdin.
    /// Runs the capture loop on a dedicated background thread.
    /// </summary>
    private void StartWindowsPipeline()
    {
        var monitors = _captureService.GetAvailableMonitors();
        var monitor = monitors[MonitorIndex];

        var stdin = _ffmpegService.StartRawFrameEncoder(
            monitor.Width, monitor.Height, FrameRate, Codec,
            SegmentDurationSeconds, TempDirectory, _cts!.Token);

        // Start the frame capture loop on a dedicated thread
        _captureTask = Task.Factory.StartNew(
            () => CaptureLoop(stdin, _cts!.Token),
            _cts!.Token,
            TaskCreationOptions.LongRunning,
            TaskScheduler.Default);
    }



    /// <summary>
    /// Frame capture loop for Windows. Acquires frames from DXGI and pipes them to FFmpeg.
    /// Runs on a dedicated background thread for consistent frame timing.
    /// 
    /// Performance Note:
    ///   The loop uses a simple frame interval sleep. For production use,
    ///   consider a high-resolution timer or vsync-aligned capture.
    /// </summary>
    private void CaptureLoop(Stream ffmpegStdin, CancellationToken ct)
    {
        int frameIntervalMs = 1000 / FrameRate;
        var sw = Stopwatch.StartNew();

        try
        {
            while (!ct.IsCancellationRequested)
            {
                sw.Restart();

                using var frame = _captureService.AcquireNextFrame(frameIntervalMs);
                if (frame != null)
                {
                    // Pipe raw BGRA frame data to FFmpeg stdin
                    ffmpegStdin.Write(frame.Data, 0, frame.Data.Length);
                    ffmpegStdin.Flush();
                }

                // Maintain frame timing
                int elapsed = (int)sw.ElapsedMilliseconds;
                if (elapsed < frameIntervalMs)
                {
                    Thread.Sleep(frameIntervalMs - elapsed);
                }
            }
        }
        catch (OperationCanceledException) { /* Expected on shutdown */ }
        catch (IOException) { /* FFmpeg process closed stdin */ }
        catch (Exception ex)
        {
            ErrorOccurred?.Invoke(this, $"Capture loop error: {ex.Message}");
        }
    }

    /// <summary>
    /// Monitors the temp directory for new segment files created by FFmpeg.
    /// When a new .mp4 segment appears, it's added to the ring buffer.
    /// </summary>
    private void StartSegmentWatcher()
    {
        _segmentWatcher = new FileSystemWatcher(TempDirectory, "segment_*.mp4")
        {
            NotifyFilter = NotifyFilters.FileName | NotifyFilters.CreationTime,
            EnableRaisingEvents = true
        };

        _segmentWatcher.Created += (_, e) =>
        {
            // Small delay to ensure FFmpeg has finished writing the file
            Task.Delay(500).ContinueWith(_ =>
            {
                if (File.Exists(e.FullPath))
                {
                    _ringBuffer.AddSegment(e.FullPath);
                }
            });
        };
    }

    /// <summary>
    /// Saves an instant replay by stitching the most recent segments from the ring buffer.
    /// Called when the user presses the hotkey.
    /// 
    /// Process:
    ///   1. Snapshot the ring buffer segments covering the replay duration.
    ///   2. Use FFmpeg concat demuxer for zero-reencode stitching.
    ///   3. Extract a thumbnail for the library view.
    ///   4. Fire the ReplaySaved event with the output path.
    /// </summary>
    public async Task SaveReplayAsync()
    {
        try
        {
            var segments = _ringBuffer.GetSegmentsForDuration(BufferDuration);
            if (segments.Count == 0)
            {
                ErrorOccurred?.Invoke(this, "No segments available for replay.");
                return;
            }

            // Generate output filename with timestamp
            Directory.CreateDirectory(LibraryDirectory);
            string timestamp = DateTime.Now.ToString("yyyy-MM-dd_HH-mm-ss");
            string outputPath = Path.Combine(LibraryDirectory, $"Replay_{timestamp}.mp4");

            // Stitch segments (zero-reencode, fast)
            await _ffmpegService.ConcatenateSegmentsAsync(segments, outputPath);

            // Extract thumbnail for library view
            string thumbPath = Path.ChangeExtension(outputPath, ".jpg");
            await _ffmpegService.ExtractThumbnailAsync(outputPath, thumbPath);

            ReplaySaved?.Invoke(this, outputPath);
        }
        catch (System.ComponentModel.Win32Exception)
        {
            ErrorOccurred?.Invoke(this, "FFmpeg не знайдено! Перевірте шлях у 'Налаштуваннях'.");
        }
        catch (Exception ex)
        {
            ErrorOccurred?.Invoke(this, $"Failed to save replay: {ex.Message}");
        }
    }

    /// <summary>
    /// Stops the recording engine gracefully.
    /// Cancels the capture loop, stops FFmpeg, and cleans up resources.
    /// </summary>
    public async Task StopAsync()
    {
        if (!IsRecording && _cts == null) return;

        IsRecording = false;
        RecordingStateChanged?.Invoke(this, false);
        _cts?.Cancel();

        await Task.Run(async () =>
        {
            // Wait for the capture loop to finish
            if (_captureTask != null)
            {
                try { await _captureTask; } catch (OperationCanceledException) { }
            }

            _ffmpegService.StopEncoder(); // Synchronous block (up to 5 seconds) offloaded to ThreadPool
            _captureService.Shutdown();

            _segmentWatcher?.Dispose();
            _segmentWatcher = null;
        });

        _cts?.Dispose();
        _cts = null;
        _captureTask = null;
    }

    private async Task FallbackStopAsync()
    {
        _cts?.Cancel();
        _ffmpegService.StopEncoder();
        _captureService.Shutdown();
        IsRecording = false;
        RecordingStateChanged?.Invoke(this, false);
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;

        StopAsync().GetAwaiter().GetResult();
        _ringBuffer.Clear();
    }
}
