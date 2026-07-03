using System;
using System.Collections.Generic;
using System.Linq;
using System.Runtime.InteropServices.WindowsRuntime;
using System.Threading.Tasks;
using Windows.Devices.Enumeration;
using Windows.Graphics.Imaging;
using Windows.Media.Capture;
using Windows.Media.Capture.Frames;

namespace Lag.Services.VfrCapture;

/// <summary>
/// Webcam frames for the recording overlay, via WinRT MediaCapture + MediaFrameReader.
/// Frames land as BGRA8 bytes in a double-buffered latest-frame slot; the capture thread
/// polls <see cref="TryCopyLatest"/> per composed frame (webcam ~30 fps ≪ capture fps, so
/// most polls are free). CPU frames on purpose: MediaCapture decodes on its own D3D device,
/// and crossing devices with shared handles is far more fragile than one small memcpy —
/// the overlay picture is only a corner PiP (~360p), ~10 MB/s at 30 fps.
/// </summary>
public sealed class WebcamCaptureSource : IDisposable
{
    private MediaCapture? _capture;
    private MediaFrameReader? _reader;
    private readonly object _gate = new();
    private byte[]? _frame;          // BGRA, tightly packed (_width * 4 per row)
    private int _width, _height;
    private long _seq;               // bumps on every stored frame
    private volatile bool _disposed;

    public sealed record DeviceInfo(string Id, string Name)
    {
        public override string ToString() => Name;   // ComboBox display
    }

    /// <summary>Attached cameras for the settings dropdown (empty when none / query fails).</summary>
    public static async Task<List<DeviceInfo>> ListDevicesAsync()
    {
        try
        {
            var all = await DeviceInformation.FindAllAsync(DeviceClass.VideoCapture);
            return all.Select(d => new DeviceInfo(d.Id, d.Name)).ToList();
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[Webcam] enumeration failed: {ex.Message}");
            return new List<DeviceInfo>();
        }
    }

    /// <summary>Opens the camera and starts streaming. Throws on failure (engine logs + records
    /// without the webcam). <paramref name="deviceId"/> empty → the system default camera.</summary>
    public void Start(string? deviceId)
    {
        var settings = new MediaCaptureInitializationSettings
        {
            StreamingCaptureMode = StreamingCaptureMode.Video,
            MemoryPreference = MediaCaptureMemoryPreference.Cpu,   // SoftwareBitmap frames
        };
        if (!string.IsNullOrEmpty(deviceId)) settings.VideoDeviceId = deviceId;

        _capture = new MediaCapture();
        _capture.InitializeAsync(settings).AsTask().GetAwaiter().GetResult();

        var source = _capture.FrameSources.Values
                         .FirstOrDefault(s => s.Info.SourceKind == MediaFrameSourceKind.Color)
                     ?? throw new InvalidOperationException("camera has no color stream");

        // The overlay is a small corner PiP — prefer a modest mode (closest to 640 wide, ≥15 fps)
        // over the camera's default, so conversion cost stays negligible.
        var best = source.SupportedFormats
            .Where(f => f.VideoFormat != null && Fps(f) >= 15)
            .OrderBy(f => Math.Abs((int)f.VideoFormat.Width - 640))
            .ThenBy(f => Fps(f))
            .FirstOrDefault();
        if (best != null)
        {
            try { source.SetFormatAsync(best).AsTask().GetAwaiter().GetResult(); }
            catch { /* stay on the default format */ }
        }

        _reader = _capture.CreateFrameReaderAsync(source).AsTask().GetAwaiter().GetResult();
        _reader.AcquisitionMode = MediaFrameReaderAcquisitionMode.Realtime;   // always the newest frame
        _reader.FrameArrived += OnFrameArrived;

        var status = _reader.StartAsync().AsTask().GetAwaiter().GetResult();
        if (status != MediaFrameReaderStartStatus.Success)
            throw new InvalidOperationException($"frame reader start: {status}");

        Console.WriteLine($"[Webcam] streaming '{source.Info.DeviceInformation?.Name}' " +
                          $"{best?.VideoFormat.Width}x{best?.VideoFormat.Height}@{(best != null ? Fps(best) : 0):F0}.");
    }

    private static double Fps(MediaFrameFormat f) =>
        f.FrameRate.Denominator > 0 ? (double)f.FrameRate.Numerator / f.FrameRate.Denominator : 0;

    private void OnFrameArrived(MediaFrameReader sender, MediaFrameArrivedEventArgs args)
    {
        if (_disposed) return;
        try
        {
            using var frame = sender.TryAcquireLatestFrame();
            var bmp = frame?.VideoMediaFrame?.SoftwareBitmap;
            if (bmp == null) return;

            // Normalize to premultiplied BGRA8 (what Direct2D wants for CopyFromMemory).
            SoftwareBitmap bgra = bmp.BitmapPixelFormat == BitmapPixelFormat.Bgra8
                                  && bmp.BitmapAlphaMode == BitmapAlphaMode.Premultiplied
                ? bmp
                : SoftwareBitmap.Convert(bmp, BitmapPixelFormat.Bgra8, BitmapAlphaMode.Premultiplied);
            try
            {
                int w = bgra.PixelWidth, h = bgra.PixelHeight;
                int bytes = w * h * 4;
                lock (_gate)
                {
                    if (_frame == null || _frame.Length != bytes) _frame = new byte[bytes];
                    bgra.CopyToBuffer(_frame.AsBuffer());
                    _width = w; _height = h;
                    _seq++;
                }
            }
            finally
            {
                if (!ReferenceEquals(bgra, bmp)) bgra.Dispose();
            }
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[Webcam] frame error: {ex.Message}");
        }
    }

    /// <summary>Copies the newest frame out when it changed since <paramref name="lastSeq"/>.
    /// Returns false (and leaves outputs untouched) when there's nothing new.</summary>
    public bool TryCopyLatest(ref byte[]? buffer, out int width, out int height, ref long lastSeq)
    {
        width = 0; height = 0;
        lock (_gate)
        {
            if (_frame == null || _seq == lastSeq) return false;
            if (buffer == null || buffer.Length != _frame.Length) buffer = new byte[_frame.Length];
            Array.Copy(_frame, buffer, _frame.Length);
            width = _width; height = _height;
            lastSeq = _seq;
            return true;
        }
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        try
        {
            if (_reader != null)
            {
                _reader.FrameArrived -= OnFrameArrived;
                _reader.StopAsync().AsTask().GetAwaiter().GetResult();
                _reader.Dispose();
            }
        }
        catch { /* teardown must not throw */ }
        try { _capture?.Dispose(); } catch { }
        _reader = null;
        _capture = null;
    }
}
