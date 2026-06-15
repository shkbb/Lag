using System;
using System.Runtime.InteropServices;
using FFmpeg.AutoGen;

namespace Lag.Services.VfrCapture;

/// <summary>
/// One-time process setup for the bundled FFmpeg 7.0 libraries (avcodec-61, avformat-61,
/// avutil-59, swresample-5) that ship inside <c>obs-core/</c>. FFmpeg.AutoGen resolves the
/// native DLLs from <see cref="ffmpeg.RootPath"/>, so we point it at obs-core and never rely on
/// a system-wide FFmpeg.
///
/// The VFR capture engine uses these libs for two things the OBS canvas can't do:
///   • <c>av1_nvenc</c> encoding fed one frame at a time with its REAL presentation timestamp
///     (variable frame rate — no CFR duplicate-frame padding, the root cause of the stutter),
///   • a libavformat MP4 muxer that writes those variable timestamps out faithfully.
/// </summary>
internal static class FfmpegInterop
{
    private static bool _initialized;
    private static readonly object Gate = new();

    /// <summary>True once <see cref="Initialize"/> has successfully bound the native libs.</summary>
    public static bool Available { get; private set; }

    /// <summary>Human-readable reason initialization failed (null when OK). Surfaced in logs.</summary>
    public static string? InitError { get; private set; }

    /// <summary>
    /// Resolves and binds the obs-core FFmpeg DLLs and verifies the encoders we depend on exist.
    /// Idempotent and thread-safe; safe to call from app start. Never throws — failure leaves
    /// <see cref="Available"/> false and the caller falls back to the legacy OBS recorder.
    /// </summary>
    public static bool Initialize(string obsCoreDir)
    {
        lock (Gate)
        {
            if (_initialized) return Available;
            _initialized = true;

            try
            {
                // RootPath tells the loader where the avcodec-61/avutil-59/... DLLs live; their
                // inter-dependencies are resolved by the OS loader, which already searches
                // obs-core because App.axaml.cs called AddDllDirectory/SetDllDirectory on it.
                ffmpeg.RootPath = obsCoreDir;
                // 7.x uses lazily-bound function pointers — this wires them up. Without it every
                // ffmpeg.* call throws NotSupportedException.
                DynamicallyLoadedBindings.Initialize();

                // Touch a trivial symbol to force the native bind to happen HERE (inside the
                // try) rather than at some later call site where the failure is opaque.
                uint version = ffmpeg.avcodec_version();
                int major = (int)(version >> 16);
                Console.WriteLine($"[Ffmpeg] Bound libavcodec {major}.{(version >> 8) & 0xFF}.{version & 0xFF} from {obsCoreDir}");

                if (major != 61)
                {
                    // We compiled against the 7.0 ABI (avcodec 61). A different major would be a
                    // silent ABI mismatch → memory corruption. Refuse rather than risk it.
                    InitError = $"libavcodec major {major} != expected 61 (FFmpeg 7.0 ABI).";
                    Console.WriteLine($"[Ffmpeg] {InitError}");
                    return false;
                }

                bool hasAv1Nvenc;
                unsafe { hasAv1Nvenc = ffmpeg.avcodec_find_encoder_by_name("av1_nvenc") != null; }
                if (!hasAv1Nvenc)
                {
                    InitError = "av1_nvenc encoder not present in the bundled avcodec.";
                    Console.WriteLine($"[Ffmpeg] {InitError}");
                    return false;
                }

                Available = true;
                Console.WriteLine("[Ffmpeg] av1_nvenc available — native VFR engine can start.");
                return true;
            }
            catch (Exception ex)
            {
                InitError = ex.Message;
                Console.WriteLine($"[Ffmpeg] Native bind failed: {ex.Message}");
                return false;
            }
        }
    }

    /// <summary>
    /// Formats an FFmpeg error code into a readable string. Many libav* calls return a negative
    /// AVERROR; this turns it into the textual reason for logs.
    /// </summary>
    public static unsafe string Err(int averror)
    {
        const int bufSize = 1024;
        byte* buffer = stackalloc byte[bufSize];
        ffmpeg.av_strerror(averror, buffer, bufSize);
        return $"{Marshal.PtrToStringAnsi((IntPtr)buffer)} ({averror})";
    }
}
