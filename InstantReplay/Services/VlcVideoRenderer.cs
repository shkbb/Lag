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
/// Renders libvlc video into Avalonia <see cref="WriteableBitmap"/>s via the vmem callback API
/// instead of a native HWND — the video is a regular Avalonia visual, so rounded corners,
/// overlays and pointer events all work natively.
///
/// TRIPLE-BUFFERED for thread safety: the libvlc decoder thread only ever writes the WRITE
/// buffer, the compositor only ever reads the DISPLAY buffer, and READY hands frames between
/// them (rotations under one lock). Writing into the same bitmap the render thread is
/// uploading crashed the app natively — hence never a shared buffer, and old bitmaps are
/// disposed DEFERRED (an Image may still reference them for a frame or two).
///
/// libvlc hands the format callback the CODED buffer size (macroblock-aligned: a 2580×1080
/// clip arrives as 2592×1088). Blitting the whole buffer would show the codec's edge padding —
/// wrong aspect ratio and garbage bands — so the bitmaps are the VISIBLE size and only that
/// region is copied out of the (pitch-strided) native buffer.
/// </summary>
public sealed class VlcVideoRenderer : IDisposable
{
    private readonly object _sync = new();

    private MediaPlayer? _player;
    private IntPtr _buffer;
    private uint _width, _height, _pitch;   // the CODED buffer libvlc decodes into
    private uint _visW, _visH;              // the VISIBLE region shown to the user
    private readonly WriteableBitmap?[] _bufs = new WriteableBitmap?[3];
    private int _write, _ready, _display;
    private bool _hasReady;
    private int _invalidatePending;
    private bool _disposed;

    // The delegates must be kept alive for as long as libvlc may call them.
    private MediaPlayer.LibVLCVideoFormatCb? _formatCb;
    private MediaPlayer.LibVLCVideoCleanupCb? _cleanupCb;
    private MediaPlayer.LibVLCVideoLockCb? _lockCb;
    private MediaPlayer.LibVLCVideoDisplayCb? _displayCb;

    /// <summary>The bitmap holding the newest PRESENTED frame. The instance rotates every
    /// frame — views must re-read it on each <see cref="FrameRendered"/>.</summary>
    public WriteableBitmap? Bitmap
    {
        get { lock (_sync) return _bufs[_display]; }
    }

    /// <summary>Raised on the UI thread when the bitmaps were (re)created.</summary>
    public event Action? BitmapChanged;

    /// <summary>Raised on the UI thread (coalesced) after a new frame was presented.</summary>
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

        // Visible size (see the class doc); fall back to the buffer size when unavailable.
        uint visW = 0, visH = 0;
        try { _player?.Size(0, ref visW, ref visH); } catch { visW = visH = 0; }
        if (visW == 0 || visH == 0 || visW > width || visH > height) { visW = width; visH = height; }

        lock (_sync)
        {
            FreeBuffer();
            _width = width;
            _height = height;
            _pitch = pitch;
            _visW = visW;
            _visH = visH;
            _buffer = Marshal.AllocHGlobal((int)(pitch * height));
            // Zero the fresh buffer: at high playback rates libvlc may flash it before the
            // first real frame lands — uninitialized memory showed up as a gray screen.
            ZeroMemory(_buffer, (UIntPtr)(pitch * height));
        }

        pitches = pitch;
        lines = height;

        Console.WriteLine($"[VlcVideoRenderer] format {width}x{height} (visible {visW}x{visH}), pitch {pitch}.");

        uint w = visW, h = visH;
        Dispatcher.UIThread.Post(() =>
        {
            lock (_sync)
            {
                if (_disposed || _visW != w || _visH != h) return;
                for (int i = 0; i < 3; i++)
                {
                    // Deferred: the old display bitmap may still be an Image.Source right now.
                    Lag.Core.DeferredDispose.Later(_bufs[i]);
                    _bufs[i] = new WriteableBitmap(
                        new PixelSize((int)w, (int)h), new Vector(96, 96),
                        PixelFormat.Bgra8888, AlphaFormat.Opaque);
                }
                _write = 0; _ready = 1; _display = 2;
                _hasReady = false;
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
            var target = _bufs[_write];
            if (_disposed || target == null || _buffer == IntPtr.Zero) return;
            if (target.PixelSize.Width != _visW || target.PixelSize.Height != _visH) return;

            // Copy only the VISIBLE region out of the (possibly padded) coded buffer,
            // into the WRITE buffer — never the one the compositor reads.
            using (var fb = target.Lock())
            {
                if (fb.RowBytes == (int)_pitch && _visW == _width)
                {
                    CopyMemory(fb.Address, _buffer, (UIntPtr)(_pitch * _visH));
                }
                else
                {
                    int copyBytes = Math.Min(fb.RowBytes, (int)(_visW * 4));
                    for (uint y = 0; y < _visH; y++)
                    {
                        CopyMemory(fb.Address + (int)(y * (uint)fb.RowBytes),
                                   _buffer + (int)(y * _pitch),
                                   (UIntPtr)copyBytes);
                    }
                }
            }

            // The frame is complete — hand it to the presenter slot.
            (_write, _ready) = (_ready, _write);
            _hasReady = true;
        }

        // Coalesced present: at most one pending rotation regardless of decode rate.
        if (Interlocked.CompareExchange(ref _invalidatePending, 1, 0) == 0)
        {
            Dispatcher.UIThread.Post(() =>
            {
                Interlocked.Exchange(ref _invalidatePending, 0);
                lock (_sync)
                {
                    if (_disposed) return;
                    if (_hasReady)
                    {
                        (_display, _ready) = (_ready, _display);
                        _hasReady = false;
                    }
                }
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
            for (int i = 0; i < 3; i++)
            {
                Lag.Core.DeferredDispose.Later(_bufs[i]);   // an Image may still show one
                _bufs[i] = null;
            }
        }
    }
}
