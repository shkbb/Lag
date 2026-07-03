using System;
using System.Collections.Generic;
using SharpDX;
using SharpDX.Direct2D1;
using SharpDX.DXGI;
using SharpDX.Mathematics.Interop;
using D3D11 = SharpDX.Direct3D11;
using DWrite = SharpDX.DirectWrite;

namespace Lag.Services.VfrCapture;

/// <summary>Overlay configuration for one recording session. Positions are FRACTIONS of the
/// free space (0 = flush left/top, 1 = flush right/bottom), scales are the element height as a
/// fraction of the frame height — resolution-independent, set by the interactive preview.</summary>
public sealed record OverlayOptions
{
    public bool WebcamEnabled { get; init; }
    public string? WebcamDeviceId { get; init; }
    public double WebcamX { get; init; } = 0.97;
    public double WebcamY { get; init; } = 0.95;
    public double WebcamScale { get; init; } = 0.24;

    public bool KeysEnabled { get; init; }
    public double KeysX { get; init; } = 0.03;
    public double KeysY { get; init; } = 0.95;
    public double KeysScale { get; init; } = 0.20;

    public bool StatsEnabled { get; init; }
    public double StatsX { get; init; } = 0.5;
    public double StatsY { get; init; } = 0.02;
    public double StatsScale { get; init; } = 0.045;
    /// <summary>0 = FPS, 1 = + CPU/GPU, 2 = + RAM.</summary>
    public int StatsDetail { get; init; } = 1;

    public bool AnyEnabled => WebcamEnabled || KeysEnabled || StatsEnabled;
}

/// <summary>
/// Bakes the webcam PiP and the keyboard+mouse visualization INTO captured frames before they
/// reach the encoder. Runs on the capture thread only (the pipeline is single-threaded by
/// design): the pool-owned WGC texture is copied into an own render-target texture, Direct2D
/// draws the overlay on top, and the composed texture goes to the encoder. Sizes scale with the
/// frame height, so any resolution/monitor works the same.
///
/// The input overlay is a compact left-half QWERTY block plus a mouse: idle keys are dark
/// translucent caps, held keys light up with the brand accent and fade briefly after release.
/// </summary>
public sealed class OverlayCompositor : IDisposable
{
    private readonly D3D11Context _d3d;
    private readonly OverlayOptions _options;
    private readonly SharpDX.Direct2D1.Factory1 _d2dFactory;
    private readonly SharpDX.Direct2D1.Device _d2dDevice;
    private readonly SharpDX.Direct2D1.DeviceContext _dc;
    private readonly DWrite.Factory _dwrite;

    private readonly WebcamCaptureSource? _webcam;
    private byte[]? _camBuffer;
    private long _camSeq;
    private Bitmap1? _camBitmap;         // recreated when the camera frame size changes
    private int _camW, _camH;
    private bool _camHasFrame;

    private D3D11.Texture2D? _rt;        // the composed frame handed to the encoder
    private Bitmap1? _target;
    private int _rtW, _rtH;

    private SolidColorBrush? _keyIdleBrush, _keyActiveBrush, _keyTextBrush, _keyTextActiveBrush, _outlineBrush, _borderBrush;
    private DWrite.TextFormat? _keyFormat;
    private float _keyFontPx;

    private readonly SystemStatsSampler? _stats;   // Steam-style resource monitor (null = off)
    private DWrite.TextLayout? _statsLayout;
    private string _statsText = "";
    private float _statsFontPx;

    private bool _disposed;
    private bool _errorLogged;

    // ── Compact left-half keyboard (the gaming cluster, Medal-style). Id = the
    // KeystrokeTracker label that lights the key; Text = what's printed on the cap. ──
    private static readonly (string Id, string Text, float W)[][] KeyRows =
    {
        new[] { ("Esc", "Esc", 1f), ("1", "1", 1f), ("2", "2", 1f), ("3", "3", 1f), ("4", "4", 1f), ("5", "5", 1f) },
        new[] { ("Tab", "Tab", 1.5f), ("Q", "Q", 1f), ("W", "W", 1f), ("E", "E", 1f), ("R", "R", 1f), ("T", "T", 1f) },
        new[] { ("Caps", "Caps", 1.75f), ("A", "A", 1f), ("S", "S", 1f), ("D", "D", 1f), ("F", "F", 1f), ("G", "G", 1f) },
        new[] { ("Shift", "Shift", 2.25f), ("Z", "Z", 1f), ("X", "X", 1f), ("C", "C", 1f), ("V", "V", 1f), ("B", "B", 1f) },
        new[] { ("Ctrl", "Ctrl", 1.25f), ("Alt", "Alt", 1.25f), ("Space", "", 3.5f) },
    };

    public OverlayCompositor(D3D11Context d3d, OverlayOptions options)
    {
        _d3d = d3d;
        _options = options;

        _d2dFactory = new SharpDX.Direct2D1.Factory1(SharpDX.Direct2D1.FactoryType.SingleThreaded);
        using var dxgiDevice = d3d.Device.QueryInterface<SharpDX.DXGI.Device>();
        _d2dDevice = new SharpDX.Direct2D1.Device(_d2dFactory, dxgiDevice);
        _dc = new SharpDX.Direct2D1.DeviceContext(_d2dDevice, DeviceContextOptions.None);
        _dwrite = new DWrite.Factory(DWrite.FactoryType.Shared);

        if (options.WebcamEnabled)
        {
            try
            {
                _webcam = new WebcamCaptureSource();
                _webcam.Start(options.WebcamDeviceId);
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[Overlay] webcam unavailable ({ex.Message}) — recording without it.");
                _webcam?.Dispose();
                _webcam = null;
            }
        }

        if (options.StatsEnabled) _stats = new SystemStatsSampler();

        KeystrokeTracker.Instance.Active = options.KeysEnabled;
        Console.WriteLine($"[Overlay] compositor ready (webcam={(_webcam != null)} @{options.WebcamX:0.##},{options.WebcamY:0.##}×{options.WebcamScale:0.##}, " +
                          $"keys={options.KeysEnabled} @{options.KeysX:0.##},{options.KeysY:0.##}×{options.KeysScale:0.##}).");
    }

    /// <summary>
    /// Returns the frame to encode: the source itself when there is nothing to draw, else the
    /// composed copy. Never throws — on a compositor error the original frame passes through.
    /// </summary>
    public D3D11.Texture2D Compose(D3D11.Texture2D src)
    {
        if (_disposed) return src;
        try
        {
            PollWebcam();
            _stats?.CountFrame();   // the per-second count IS the recording FPS shown in the panel

            bool drawCam = _camHasFrame && _webcam != null;
            bool drawKeys = _options.KeysEnabled;
            bool drawStats = _stats != null;
            if (!drawCam && !drawKeys && !drawStats) return src;

            var desc = src.Description;
            EnsureTarget(desc.Width, desc.Height);
            _d3d.Context.CopyResource(src, _rt);

            _dc.Target = _target;
            _dc.BeginDraw();
            if (drawCam) DrawWebcam(desc.Width, desc.Height);
            if (drawKeys) DrawInputOverlay(desc.Width, desc.Height);
            if (drawStats) DrawStats(desc.Width, desc.Height);
            _dc.EndDraw();
            _dc.Target = null;

            return _rt!;
        }
        catch (Exception ex)
        {
            if (!_errorLogged)
            {
                _errorLogged = true;   // once — this runs per frame
                Console.WriteLine($"[Overlay] compose failed, passing frames through: {ex.Message}");
            }
            return src;
        }
    }

    private void PollWebcam()
    {
        if (_webcam == null) return;
        if (!_webcam.TryCopyLatest(ref _camBuffer, out int w, out int h, ref _camSeq)) return;

        if (_camBitmap == null || _camW != w || _camH != h)
        {
            _camBitmap?.Dispose();
            _camBitmap = new Bitmap1(_dc, new Size2(w, h),
                new BitmapProperties1(new SharpDX.Direct2D1.PixelFormat(Format.B8G8R8A8_UNorm, SharpDX.Direct2D1.AlphaMode.Premultiplied),
                                      96, 96, BitmapOptions.None));
            _camW = w; _camH = h;
        }
        _camBitmap.CopyFromMemory(_camBuffer, w * 4);
        _camHasFrame = true;
    }

    private void EnsureTarget(int w, int h)
    {
        if (_rt != null && _rtW == w && _rtH == h) return;

        _target?.Dispose();
        _rt?.Dispose();
        _rt = new D3D11.Texture2D(_d3d.Device, new D3D11.Texture2DDescription
        {
            Width = w,
            Height = h,
            MipLevels = 1,
            ArraySize = 1,
            Format = Format.B8G8R8A8_UNorm,
            SampleDescription = new SampleDescription(1, 0),
            Usage = D3D11.ResourceUsage.Default,
            BindFlags = D3D11.BindFlags.RenderTarget | D3D11.BindFlags.ShaderResource,
        });
        using var surface = _rt.QueryInterface<Surface>();
        _target = new Bitmap1(_dc, surface,
            new BitmapProperties1(new SharpDX.Direct2D1.PixelFormat(Format.B8G8R8A8_UNorm, SharpDX.Direct2D1.AlphaMode.Premultiplied),
                                  96, 96, BitmapOptions.Target | BitmapOptions.CannotDraw));
        _rtW = w; _rtH = h;

        _keyIdleBrush ??= new SolidColorBrush(_dc, new RawColor4(0.06f, 0.065f, 0.085f, 0.62f));
        _keyActiveBrush ??= new SolidColorBrush(_dc, new RawColor4(0.133f, 0.827f, 0.933f, 1f));   // brand cyan #22D3EE
        _keyTextBrush ??= new SolidColorBrush(_dc, new RawColor4(1f, 1f, 1f, 0.85f));
        _keyTextActiveBrush ??= new SolidColorBrush(_dc, new RawColor4(0.04f, 0.05f, 0.06f, 1f));
        _outlineBrush ??= new SolidColorBrush(_dc, new RawColor4(1f, 1f, 1f, 0.28f));
        _borderBrush ??= new SolidColorBrush(_dc, new RawColor4(1f, 1f, 1f, 0.35f));
    }

    private void DrawWebcam(int frameW, int frameH)
    {
        float unit = frameH / 1080f;
        float boxH = (float)(_options.WebcamScale * frameH);
        float boxW = boxH * ((float)_camW / Math.Max(1, _camH));   // keep the camera's own aspect
        float radius = 14f * unit;

        float x = (float)(_options.WebcamX * Math.Max(0, frameW - boxW));
        float y = (float)(_options.WebcamY * Math.Max(0, frameH - boxH));
        var dest = new RawRectangleF(x, y, x + boxW, y + boxH);
        var rounded = new RoundedRectangle { Rect = dest, RadiusX = radius, RadiusY = radius };

        using var clip = new RoundedRectangleGeometry(_d2dFactory, rounded);
        _dc.PushLayer(new LayerParameters1
        {
            ContentBounds = dest,
            GeometricMask = clip,
            MaskTransform = new RawMatrix3x2(1, 0, 0, 1, 0, 0),   // identity
            Opacity = 1f,
            LayerOptions = LayerOptions1.None,
        }, null);
        _dc.DrawBitmap(_camBitmap, dest, 1f, SharpDX.Direct2D1.BitmapInterpolationMode.Linear);
        _dc.PopLayer();

        _dc.DrawRoundedRectangle(rounded, _borderBrush, 2f * unit);
    }

    /// <summary>The Medal-style input block: mini keyboard with lit keys + a mouse.</summary>
    private void DrawInputOverlay(int frameW, int frameH)
    {
        // Currently pressed / fading keys as label → opacity.
        var pressed = new Dictionary<string, float>();
        foreach (var (label, opacity) in KeystrokeTracker.Instance.Snapshot())
            pressed[label] = opacity;

        float unit = frameH / 1080f;
        // The user-resizable block height (fraction of the frame) dictates the key size:
        // kbH = 5 keys + 4 gaps (gap = keyU/8.5) ≈ 5.47 · keyU.
        float keyU = (float)(_options.KeysScale * frameH) / 5.47f;
        float gap = keyU / 8.5f;
        float radius = keyU * 0.15f;

        // Key labels scale with the keys — rebuild the text format when the size changes.
        float fontPx = Math.Max(6f, keyU * 0.38f);
        if (_keyFormat == null || Math.Abs(_keyFontPx - fontPx) > 0.5f)
        {
            _keyFormat?.Dispose();
            _keyFormat = new DWrite.TextFormat(_dwrite, "Segoe UI", DWrite.FontWeight.SemiBold,
                                               DWrite.FontStyle.Normal, fontPx)
            {
                TextAlignment = DWrite.TextAlignment.Center,
                ParagraphAlignment = DWrite.ParagraphAlignment.Center,
            };
            _keyFontPx = fontPx;
        }

        // Keyboard block metrics.
        float kbW = 0;
        foreach (var row in KeyRows)
        {
            float w = 0;
            foreach (var k in row) w += k.W * keyU + gap;
            kbW = Math.Max(kbW, w - gap);
        }
        float kbH = KeyRows.Length * (keyU + gap) - gap;

        // Mouse block metrics (to the right of the keyboard).
        float mouseW = 1.82f * keyU, mouseH = 2.82f * keyU, mouseGap = 0.41f * keyU;
        float blockW = kbW + mouseGap + mouseW;
        float blockH = Math.Max(kbH, mouseH);

        float bx = (float)(_options.KeysX * Math.Max(0, frameW - blockW));
        float by = (float)(_options.KeysY * Math.Max(0, frameH - blockH));

        // ── Keyboard ──
        float y = by + (blockH - kbH) / 2f;
        foreach (var row in KeyRows)
        {
            float x = bx;
            foreach (var (id, text, w) in row)
            {
                var rect = new RawRectangleF(x, y, x + w * keyU, y + keyU);
                var rr = new RoundedRectangle { Rect = rect, RadiusX = radius, RadiusY = radius };
                float act = pressed.GetValueOrDefault(id);

                _dc.FillRoundedRectangle(rr, _keyIdleBrush);
                if (act > 0)
                {
                    _keyActiveBrush!.Opacity = act;
                    _dc.FillRoundedRectangle(rr, _keyActiveBrush);
                }
                _dc.DrawRoundedRectangle(rr, _outlineBrush, 1f * unit);
                if (text.Length > 0)
                    _dc.DrawText(text, _keyFormat, rect, act > 0.5f ? _keyTextActiveBrush : _keyTextBrush,
                                 DrawTextOptions.None, MeasuringMode.Natural);

                x += w * keyU + gap;
            }
            y += keyU + gap;
        }

        // ── Mouse (body with LMB / RMB halves + wheel = MMB) ──
        float mx = bx + kbW + mouseGap;
        float my = by + (blockH - mouseH) / 2f;
        var body = new RawRectangleF(mx, my, mx + mouseW, my + mouseH);
        var bodyRR = new RoundedRectangle { Rect = body, RadiusX = mouseW * 0.42f, RadiusY = mouseW * 0.42f };
        float split = my + mouseH * 0.42f;   // buttons end here

        using (var clip = new RoundedRectangleGeometry(_d2dFactory, bodyRR))
        {
            _dc.PushLayer(new LayerParameters1
            {
                ContentBounds = body,
                GeometricMask = clip,
                MaskTransform = new RawMatrix3x2(1, 0, 0, 1, 0, 0),
                Opacity = 1f,
                LayerOptions = LayerOptions1.None,
            }, null);

            _dc.FillRoundedRectangle(bodyRR, _keyIdleBrush);
            float lmb = pressed.GetValueOrDefault("LMB"), rmb = pressed.GetValueOrDefault("RMB");
            if (lmb > 0)
            {
                _keyActiveBrush!.Opacity = lmb;
                _dc.FillRectangle(new RawRectangleF(mx, my, mx + mouseW / 2f, split), _keyActiveBrush);
            }
            if (rmb > 0)
            {
                _keyActiveBrush!.Opacity = rmb;
                _dc.FillRectangle(new RawRectangleF(mx + mouseW / 2f, my, mx + mouseW, split), _keyActiveBrush);
            }
            // Button separators.
            _dc.DrawLine(new RawVector2(mx + mouseW / 2f, my), new RawVector2(mx + mouseW / 2f, split), _outlineBrush, 1f * unit);
            _dc.DrawLine(new RawVector2(mx, split), new RawVector2(mx + mouseW, split), _outlineBrush, 1f * unit);
            _dc.PopLayer();
        }

        // Wheel (MMB) sits over the button split line.
        float wheelW = 9f * unit, wheelH = 24f * unit;
        var wheel = new RawRectangleF(mx + mouseW / 2f - wheelW / 2f, my + 12f * unit,
                                      mx + mouseW / 2f + wheelW / 2f, my + 12f * unit + wheelH);
        var wheelRR = new RoundedRectangle { Rect = wheel, RadiusX = wheelW / 2f, RadiusY = wheelW / 2f };
        float mmb = pressed.GetValueOrDefault("MMB");
        _dc.FillRoundedRectangle(wheelRR, _keyIdleBrush);
        if (mmb > 0)
        {
            _keyActiveBrush!.Opacity = mmb;
            _dc.FillRoundedRectangle(wheelRR, _keyActiveBrush);
        }
        _dc.DrawRoundedRectangle(wheelRR, _outlineBrush, 1f * unit);

        _dc.DrawRoundedRectangle(bodyRR, _outlineBrush, 1.5f * unit);
        _keyActiveBrush!.Opacity = 1f;
    }

    /// <summary>Steam-style one-line resource panel: FPS [+ CPU/GPU [+ RAM]].</summary>
    private void DrawStats(int frameW, int frameH)
    {
        var s = _stats!.Current;
        var inv = System.Globalization.CultureInfo.InvariantCulture;
        string text = _options.StatsDetail switch
        {
            0 => $"{s.Fps} FPS",
            1 => $"{s.Fps} FPS   CPU {s.CpuPercent}%   GPU {s.GpuPercent}%",
            _ => string.Format(inv, "{0} FPS   CPU {1}%   GPU {2}%   RAM {3:0.0}/{4:0.0} GB",
                               s.Fps, s.CpuPercent, s.GpuPercent, s.RamUsedGb, s.RamTotalGb),
        };

        float panelH = (float)(_options.StatsScale * frameH);
        float fontPx = Math.Max(6f, panelH * 0.52f);

        // Re-layout only when the text or the size actually changed (once a second at most).
        if (_statsLayout == null || _statsText != text || Math.Abs(_statsFontPx - fontPx) > 0.5f)
        {
            _statsLayout?.Dispose();
            using var format = new DWrite.TextFormat(_dwrite, "Segoe UI", DWrite.FontWeight.SemiBold,
                                                     DWrite.FontStyle.Normal, fontPx);
            _statsLayout = new DWrite.TextLayout(_dwrite, text, format, frameW, panelH);
            _statsText = text;
            _statsFontPx = fontPx;
        }

        float padX = panelH * 0.45f;
        float panelW = _statsLayout.Metrics.Width + padX * 2;
        float x = (float)(_options.StatsX * Math.Max(0, frameW - panelW));
        float y = (float)(_options.StatsY * Math.Max(0, frameH - panelH));

        var rect = new RawRectangleF(x, y, x + panelW, y + panelH);
        var rr = new RoundedRectangle { Rect = rect, RadiusX = panelH * 0.22f, RadiusY = panelH * 0.22f };
        _dc.FillRoundedRectangle(rr, _keyIdleBrush);
        _dc.DrawRoundedRectangle(rr, _outlineBrush, 1f);
        _dc.DrawTextLayout(new RawVector2(x + padX, y + (panelH - _statsLayout.Metrics.Height) / 2f),
                           _statsLayout, _keyTextBrush);
    }

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;
        KeystrokeTracker.Instance.Active = false;

        _stats?.Dispose();
        _statsLayout?.Dispose();
        _webcam?.Dispose();
        _keyFormat?.Dispose();
        _keyIdleBrush?.Dispose();
        _keyActiveBrush?.Dispose();
        _keyTextBrush?.Dispose();
        _keyTextActiveBrush?.Dispose();
        _outlineBrush?.Dispose();
        _borderBrush?.Dispose();
        _camBitmap?.Dispose();
        _target?.Dispose();
        _rt?.Dispose();
        _dwrite.Dispose();
        _dc.Dispose();
        _d2dDevice.Dispose();
        _d2dFactory.Dispose();
    }
}
