using System;
using System.Runtime.InteropServices;
using System.Threading;
using Avalonia;
using Avalonia.Media.Imaging;
using Avalonia.Platform;
using Avalonia.Threading;
using LibVLCSharp.Shared;

namespace Lag.Services;

/// <summary>
/// Renders libvlc video into an Avalonia <see cref="WriteableBitmap"/> via the vmem
/// callback API instead of a native HWND. This removes every airspace limitation of
/// NativeControlHost: the video is a regular Avalonia visual, so rounded corners,
/// overlays and pointer events all work natively.
///
/// Threading model: libvlc invokes the format/lock/display callbacks on its decoder
/// threads. The pixel copy happens there (WriteableBitmap memory is thread-safe to
/// write); only the cheap "repaint" notification is marshalled to the UI thread,
/// coalesced so a fast decoder can't flood the dispatcher.
/// </summary>
public sealed class VlcVideoRenderer : IDisposable
{
    private readonly object _sync = new();

    private MediaPlayer? _player;
    private IntPtr _buffer;
    private uint _width, _height, _pitch;
    private WriteableBitmap? _bitmap;
    private int _invalidatePending;
    private bool _disposed;

    // The delegates must be kept alive for as long as libvlc may call them.
    private MediaPlayer.LibVLCVideoFormatCb? _formatCb;
    private MediaPlayer.LibVLCVideoCleanupCb? _cleanupCb;
    private MediaPlayer.LibVLCVideoLockCb? _lockCb;
    private MediaPlayer.LibVLCVideoDisplayCb? _displayCb;

    /// <summary>The bitmap receiving decoded frames. Recreated when the video size changes.</summary>
    public WriteableBitmap? Bitmap
    {
        get { lock (_sync) return _bitmap; }
    }

    /// <summary>Raised on the UI thread when <see cref="Bitmap"/> was (re)created.</summary>
    public event Action? BitmapChanged;

    /// <summary>Raised on the UI thread (coalesced) after a new frame was written.</summary>
    public event Action? FrameRendered;

    /// <summary>Hooks the vmem callbacks. Must be called before any playback starts.</summary>
    public void Attach(MediaPlayer player)
    {
        _player = player;

        _formatCb = OnVideoFormat;
        _cleanupCb = OnVideoCleanup;
        _lockCb = OnVideoLock;
        _displayCb = OnVideoDisplay;

        player.SetVideoFormatCallbacks(_formatCb, _cleanupCb);
        player.SetVideoCallbacks(_lockCb, null, _displayCb);
    }

    private uint OnVideoFormat(ref IntPtr opaque, IntPtr chroma, ref uint width, ref uint height,
                               ref uint pitches, ref uint lines)
    {
        // Ask libvlc for 32-bit BGRA ("RV32"), keeping the source resolution.
        Marshal.Copy(new[] { (byte)'R', (byte)'V', (byte)'3', (byte)'2' }, 0, chroma, 4);

        uint pitch = (width * 4 + 31) & ~31u; // libvlc requires 32-byte aligned rows

        lock (_sync)
        {
            FreeBuffer();
            _width = width;
            _height = height;
            _pitch = pitch;
            _buffer = Marshal.AllocHGlobal((int)(pitch * height));
            // Zero the fresh buffer: at high playback rates libvlc may flash it before the
            // first real frame lands — uninitialized memory showed up as a gray screen.
            ZeroMemory(_buffer, (UIntPtr)(pitch * height));
        }

        pitches = pitch;
        lines = height;

        uint w = width, h = height;
        Dispatcher.UIThread.Post(() =>
        {
            lock (_sync)
            {
                if (_disposed || _width != w || _height != h) return;
                if (_bitmap == null || _bitmap.PixelSize.Width != w || _bitmap.PixelSize.Height != h)
                {
                    _bitmap?.Dispose();
                    _bitmap = new WriteableBitmap(
                        new PixelSize((int)w, (int)h), new Vector(96, 96),
                        PixelFormat.Bgra8888, AlphaFormat.Opaque);
                }
            }
            BitmapChanged?.Invoke();
        });

        return 1; // number of buffers
    }

    private void OnVideoCleanup(ref IntPtr opaque)
    {
        lock (_sync) FreeBuffer();
    }

    private IntPtr OnVideoLock(IntPtr opaque, IntPtr planes)
    {
        lock (_sync)
            Marshal.WriteIntPtr(planes, _buffer);
        return IntPtr.Zero;
    }

    [DllImport("kernel32.dll", EntryPoint = "RtlMoveMemory")]
    private static extern void CopyMemory(IntPtr dest, IntPtr src, UIntPtr count);

    [DllImport("kernel32.dll", EntryPoint = "RtlZeroMemory")]
    private static extern void ZeroMemory(IntPtr dest, UIntPtr count);

    private void OnVideoDisplay(IntPtr opaque, IntPtr picture)
    {
        lock (_sync)
        {
            if (_disposed || _bitmap == null || _buffer == IntPtr.Zero) return;
            if (_bitmap.PixelSize.Width != _width || _bitmap.PixelSize.Height != _height) return;

            using var fb = _bitmap.Lock();
            int copyBytes = Math.Min(fb.RowBytes, (int)_pitch);
            for (uint y = 0; y < _height; y++)
            {
                CopyMemory(fb.Address + (int)(y * (uint)fb.RowBytes),
                           _buffer + (int)(y * _pitch),
                           (UIntPtr)copyBytes);
            }
        }

        // Coalesced repaint: at most one pending invalidation regardless of decode rate.
        if (Interlocked.CompareExchange(ref _invalidatePending, 1, 0) == 0)
        {
            Dispatcher.UIThread.Post(() =>
            {
                Interlocked.Exchange(ref _invalidatePending, 0);
                FrameRendered?.Invoke();
            }, DispatcherPriority.Render);
        }
    }

    private void FreeBuffer()
    {
        if (_buffer != IntPtr.Zero)
        {
            Marshal.FreeHGlobal(_buffer);
            _buffer = IntPtr.Zero;
        }
    }

    public void Dispose()
    {
        lock (_sync)
        {
            if (_disposed) return;
            _disposed = true;
            FreeBuffer();
            _bitmap?.Dispose();
            _bitmap = null;
        }
    }
}
