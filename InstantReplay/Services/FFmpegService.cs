using System.Diagnostics;
using System.Runtime.InteropServices;

namespace Lag.Services;

/// <summary>
/// Manages FFmpeg external processes for video encoding, segment concatenation,
/// and thumbnail extraction. All encoding is done via ffmpeg.exe subprocess.
/// 
/// Architecture Decision — Why External Process:
///   Using ffmpeg.exe as an external process (vs. FFmpeg.AutoGen or a .NET wrapper) provides:
///   1. Full access to all encoder flags and features without version-locked bindings.
///   2. Isolation — a crash in FFmpeg won't crash the host application.
///   3. Cross-platform compatibility — same approach works on Windows and Linux.
///   4. Easy updates — just replace the ffmpeg binary.
/// 
/// Encoding Pipeline (Windows):
///   Raw BGRA frames are piped to ffmpeg's stdin as rawvideo.
///   FFmpeg outputs segmented .mp4 files to the temp directory.
///   
/// Encoding Pipeline (Linux):
///   FFmpeg captures directly using x11grab input device.
///   Segments are written to the temp directory.
/// 
/// Thread Safety:
///   Process lifecycle methods are async and use CancellationToken.
///   Multiple concurrent calls are prevented by the _encoderProcess guard.
/// </summary>
public sealed class FFmpegService : IDisposable
{
    private Process? _encoderProcess;
    private readonly object _processLock = new();
    private bool _disposed;

    /// <summary>Path to ffmpeg executable. Defaults to "ffmpeg" (must be on PATH).</summary>
    public string FFmpegPath { get; set; } = "ffmpeg";

    /// <summary>
    /// Available codec identifiers mapped to their FFmpeg encoder names.
    /// The list includes hardware encoders and a software fallback.
    /// </summary>
    public static IReadOnlyDictionary<string, string> AvailableCodecs { get; } =
        new Dictionary<string, string>
        {
            ["NVIDIA NVENC (H.264)"] = "h264_nvenc",
            ["AMD AMF (H.264)"] = "h264_amf",
            ["Intel QSV (H.264)"] = "h264_qsv",
            ["Software x264"] = "libx264"
        };

    /// <summary>
    /// Starts the FFmpeg encoder process for Windows raw-frame piping mode.
    /// Frames from DXGI are piped to stdin as raw BGRA video.
    /// Output is segmented .mp4 files in the specified temp directory.
    /// </summary>
    /// <param name="width">Frame width in pixels.</param>
    /// <param name="height">Frame height in pixels.</param>
    /// <param name="fps">Target frame rate.</param>
    /// <param name="codec">FFmpeg encoder name (e.g., "h264_nvenc").</param>
    /// <param name="segmentDuration">Duration of each segment in seconds.</param>
    /// <param name="tempDir">Directory for segment output files.</param>
    /// <param name="cancellationToken">Cancellation token for graceful shutdown.</param>
    /// <returns>The writable stdin stream to pipe raw frames into.</returns>
    public Stream StartRawFrameEncoder(
        int width, int height, int fps, string codec,
        int segmentDuration, string tempDir,
        CancellationToken cancellationToken = default)
    {
        lock (_processLock)
        {
            if (_encoderProcess != null)
                throw new InvalidOperationException("Encoder is already running.");

            Directory.CreateDirectory(tempDir);

            // Segment output pattern: segment_000.mp4, segment_001.mp4, ...
            string segmentPattern = Path.Combine(tempDir, "segment_%05d.mp4");

            // Build FFmpeg arguments for raw frame input → segmented output
            var args = BuildRawInputArgs(width, height, fps, codec,
                segmentDuration, segmentPattern);

            _encoderProcess = StartFFmpegProcess(args);

            // Register cancellation to gracefully stop the process
            cancellationToken.Register(() => StopEncoder());

            return _encoderProcess.StandardInput.BaseStream;
        }
    }



    /// <summary>
    /// Concatenates multiple video segments into a single .mp4 file
    /// using FFmpeg's concat demuxer. Uses stream copy (no re-encoding)
    /// for near-instant stitching.
    /// </summary>
    /// <param name="segmentPaths">Ordered list of segment file paths.</param>
    /// <param name="outputPath">Path for the concatenated output file.</param>
    /// <param name="cancellationToken">Cancellation token.</param>
    public async Task ConcatenateSegmentsAsync(
        IReadOnlyList<string> segmentPaths, string outputPath,
        CancellationToken cancellationToken = default)
    {
        if (segmentPaths.Count == 0)
            throw new ArgumentException("No segments to concatenate.");

        // Create a temporary concat list file for FFmpeg
        string listFile = Path.Combine(
            Path.GetDirectoryName(segmentPaths[0])!,
            $"concat_{Guid.NewGuid():N}.txt");

        try
        {
            // Write the concat file: each line is "file '/path/to/segment.mp4'"
            var lines = segmentPaths.Select(p => $"file '{p.Replace("\\", "/")}'");
            await File.WriteAllLinesAsync(listFile, lines, cancellationToken);

            string args = $"-f concat -safe 0 -i \"{listFile}\" -c copy -y \"{outputPath}\"";

            using var process = StartFFmpegProcess(args);
            await process.WaitForExitAsync(cancellationToken);

            if (process.ExitCode != 0)
            {
                string error = await process.StandardError.ReadToEndAsync(cancellationToken);
                throw new InvalidOperationException(
                    $"FFmpeg concatenation failed (exit code {process.ExitCode}): {error}");
            }
        }
        finally
        {
            // Clean up the temporary concat list file
            if (File.Exists(listFile))
                File.Delete(listFile);
        }
    }

    /// <summary>
    /// Extracts a thumbnail image from a video file at the 1-second mark.
    /// </summary>
    /// <param name="videoPath">Path to the source video.</param>
    /// <param name="thumbnailPath">Output path for the thumbnail JPEG.</param>
    /// <param name="cancellationToken">Cancellation token.</param>
    public async Task ExtractThumbnailAsync(
        string videoPath, string thumbnailPath,
        CancellationToken cancellationToken = default)
    {
        string args = $"-i \"{videoPath}\" -ss 00:00:01 -vframes 1 " +
                      $"-vf scale=320:-1 -y \"{thumbnailPath}\"";

        using var process = StartFFmpegProcess(args);
        await process.WaitForExitAsync(cancellationToken);
    }

    /// <summary>
    /// Probes a video file and returns its duration.
    /// Uses ffprobe (assumed to be alongside ffmpeg).
    /// </summary>
    public async Task<TimeSpan> GetVideoDurationAsync(
        string videoPath, CancellationToken cancellationToken = default)
    {
        string safePath = FFmpegPath.Trim('"', '\'');
        string ffprobePath = safePath.Replace("ffmpeg", "ffprobe");
        string args = $"-v error -show_entries format=duration " +
                      $"-of default=noprint_wrappers=1:nokey=1 \"{videoPath}\"";

        var psi = new ProcessStartInfo
        {
            FileName = ffprobePath,
            Arguments = args,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true
        };

        using var process = Process.Start(psi)!;
        string output = await process.StandardOutput.ReadToEndAsync(cancellationToken);
        await process.WaitForExitAsync(cancellationToken);

        if (double.TryParse(output.Trim(), System.Globalization.NumberStyles.Float,
            System.Globalization.CultureInfo.InvariantCulture, out double seconds))
        {
            return TimeSpan.FromSeconds(seconds);
        }

        return TimeSpan.Zero;
    }

    /// <summary>
    /// Gracefully stops the running encoder process by sending 'q' to stdin,
    /// which tells FFmpeg to finalize and flush all output.
    /// Falls back to Kill() if the process doesn't exit within 5 seconds.
    /// </summary>
    public void StopEncoder()
    {
        lock (_processLock)
        {
            if (_encoderProcess == null || _encoderProcess.HasExited)
                return;

            try
            {
                // Send 'q' to FFmpeg stdin for graceful shutdown
                _encoderProcess.StandardInput.Write('q');
                _encoderProcess.StandardInput.Flush();

                if (!_encoderProcess.WaitForExit(5000))
                {
                    _encoderProcess.Kill(entireProcessTree: true);
                }
            }
            catch (Exception)
            {
                try { _encoderProcess.Kill(entireProcessTree: true); } catch { }
            }
            finally
            {
                _encoderProcess.Dispose();
                _encoderProcess = null;
            }
        }
    }

    /// <summary>
    /// Builds FFmpeg arguments for raw BGRA frame input from stdin.
    /// </summary>
    private static string BuildRawInputArgs(
        int width, int height, int fps, string codec,
        int segmentDuration, string segmentPattern)
    {
        return $"-f rawvideo -pixel_format bgra -video_size {width}x{height} " +
               $"-framerate {fps} -i pipe:0 " +
               $"-c:v {codec} {GetCodecQualityArgs(codec)} -pix_fmt yuv420p " +
               $"-f segment -segment_time {segmentDuration} " +
               $"-reset_timestamps 1 -y \"{segmentPattern}\"";
    }

    /// <summary>
    /// Returns codec-specific quality/performance arguments.
    /// Hardware encoders use preset flags; software x264 uses ultrafast.
    /// </summary>
    private static string GetCodecQualityArgs(string codec)
    {
        return codec switch
        {
            "h264_nvenc" => "-preset p4 -tune ll -rc vbr -cq 28 -b:v 20M",
            "h264_amf" => "-quality speed -rc vbr_peak -b:v 20M",
            "h264_qsv" => "-preset veryfast -global_quality 28 -b:v 20M",
            "libx264" => "-preset ultrafast -crf 23",
            _ => "-preset ultrafast -crf 23"
        };
    }

    /// <summary>
    /// Creates and starts an FFmpeg process with the given arguments.
    /// Stdin is redirected for piping, stderr for error capture.
    /// </summary>
    private Process StartFFmpegProcess(string arguments)
    {
        string safePath = FFmpegPath.Trim('"', '\'');
        var psi = new ProcessStartInfo
        {
            FileName = safePath,
            Arguments = arguments,
            RedirectStandardInput = true,
            RedirectStandardOutput = false,
            RedirectStandardError = true,
            UseShellExecute = false,
            CreateNoWindow = true
        };

        var process = new Process { StartInfo = psi };

        // Log FFmpeg stderr output for debugging
        process.ErrorDataReceived += (_, e) =>
        {
            if (!string.IsNullOrEmpty(e.Data))
                Debug.WriteLine($"[FFmpeg] {e.Data}");
        };

        process.Start();
        process.BeginErrorReadLine();
        return process;
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        StopEncoder();
    }

    public static string? FindFFmpegExecutable()
    {
        string exeName = "ffmpeg.exe";

        // 1. Check if it's already in PATH (by trying to run it)
        try
        {
            var psi = new ProcessStartInfo
            {
                FileName = exeName,
                Arguments = "-version",
                RedirectStandardOutput = true,
                RedirectStandardError = true,
                UseShellExecute = false,
                CreateNoWindow = true
            };
            using var proc = Process.Start(psi);
            if (proc != null)
            {
                proc.WaitForExit(1000);
                return exeName; // It's in PATH
            }
        }
        catch { /* Not in PATH */ }

        // 2. Search common locations
        var searchPaths = new List<string>
        {
            AppDomain.CurrentDomain.BaseDirectory,
            @"C:\ffmpeg\bin",
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFiles), "ffmpeg", "bin"),
            Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.ProgramFilesX86), "ffmpeg", "bin")
        };
        
        // Search in Downloads folder for common extracted zip names
        string downloadsPath = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.UserProfile), "Downloads");
        if (Directory.Exists(downloadsPath))
        {
            try
            {
                var ffmpegFolders = Directory.GetDirectories(downloadsPath, "ffmpeg*", SearchOption.TopDirectoryOnly);
                foreach (var folder in ffmpegFolders)
                {
                    searchPaths.Add(Path.Combine(folder, "bin"));
                    searchPaths.Add(Path.Combine(folder, "ffmpeg*", "bin"));
                    try 
                    {
                         var subfolders = Directory.GetDirectories(folder, "ffmpeg*", SearchOption.TopDirectoryOnly);
                         foreach(var sub in subfolders) searchPaths.Add(Path.Combine(sub, "bin"));
                    } catch { }
                }
            }
            catch { }
        }

        // Search in WinGet Packages
        string wingetPath = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "Microsoft", "WinGet", "Packages");
        if (Directory.Exists(wingetPath))
        {
            try
            {
                var wingetFolders = Directory.GetDirectories(wingetPath, "*ffmpeg*", SearchOption.TopDirectoryOnly);
                foreach (var folder in wingetFolders)
                {
                    searchPaths.Add(Path.Combine(folder, "bin"));
                    try 
                    {
                         var subfolders = Directory.GetDirectories(folder, "*ffmpeg*", SearchOption.TopDirectoryOnly);
                         foreach(var sub in subfolders) searchPaths.Add(Path.Combine(sub, "bin"));
                    } catch { }
                }
            }
            catch { }
        }

        // Check exact locations first
        foreach (var path in searchPaths)
        {
            string fullPath = Path.Combine(path, exeName);
            if (File.Exists(fullPath))
                return fullPath;
        }

        return null;
    }
}
