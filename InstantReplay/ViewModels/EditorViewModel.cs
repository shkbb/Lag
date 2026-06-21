using System.Collections.ObjectModel;
using System.Diagnostics;
using System.Globalization;
using Avalonia.Threading;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Lag.Core;
using Lag.Models;
using LibVLCSharp.Shared;

namespace Lag.ViewModels;

/// <summary>
/// One audio track row in the editor. Maps 1:1 to an audio stream in the source file
/// (track 0 = system sound / mixed, track 1 = microphone when "separate tracks" was on).
/// Besides volume/mute it has its own audible range — outside [StartSec, EndSec] the
/// track is silenced on export (video keeps playing).
/// </summary>
public partial class EditorAudioTrack : ObservableObject
{
    /// <summary>Zero-based audio stream index inside the source file (ffmpeg 0:a:N).</summary>
    public int StreamIndex { get; }

    public string Name { get; }

    [ObservableProperty]
    private int _volume = 100;

    [ObservableProperty]
    private bool _isMuted;

    /// <summary>Selected on the timeline (Shift+click) — trim edits follow the video handles.</summary>
    [ObservableProperty]
    private bool _isSelected;

    /// <summary>Track audible range, in seconds of the FULL source clip.</summary>
    [ObservableProperty]
    private double _startSec;

    [ObservableProperty]
    private double _endSec;

    /// <summary>Full source clip duration (range-bar scale).</summary>
    [ObservableProperty]
    private double _clipDurationSec;

    public EditorAudioTrack(int streamIndex, string name, double clipDurationSec)
    {
        StreamIndex = streamIndex;
        Name = name;
        _clipDurationSec = Math.Max(0.1, clipDurationSec);
        _startSec = 0;
        _endSec = _clipDurationSec;
    }
}

/// <summary>One text caption overlaid on the video: content, normalized position in the output
/// frame (0..1), font size as a % of frame height, and a hex colour.</summary>
public partial class EditorTextItem : ObservableObject
{
    [ObservableProperty] private string _text = "Text";
    [ObservableProperty] private double _nx = 0.5;       // 0 = left edge, 1 = right edge (kept in frame)
    [ObservableProperty] private double _ny = 0.12;      // 0 = top, 1 = bottom
    [ObservableProperty] private int _fontPercent = 8;   // font height as % of the frame height
    [ObservableProperty] private string _color = "#FFFFFF";
}

public record EditorSpeedOption(string Display, double Value);

/// <summary>Export resolution preset; Height 0 = keep the source resolution.</summary>
public record EditorExportRes(string Display, int Height);

/// <summary>A caption colour swatch: the hex value (for the command) and a ready brush (for the chip).</summary>
public record EditorTextColor(string Hex, Avalonia.Media.IBrush Brush);

/// <summary>A removed section of the clip (seconds of the full source). On export the kept parts
/// around it are concatenated.</summary>
public record EditorCut(double StartSec, double EndSec);

/// <summary>CropGeometry is libvlc's "W:H" crop string for the live preview.</summary>
public record EditorAspectOption(string Display, double? Ratio, string? CropGeometry);

/// <summary>
/// A fun video filter: ffmpeg graph for export + an approximation for the live preview
/// via libvlc's "adjust" filter (contrast/brightness/saturation/hue).
/// </summary>
public partial class EditorFilterOption : ObservableObject
{
    public string Name { get; }

    /// <summary>ffmpeg video filter graph, or null for "no filter".</summary>
    public string? Graph { get; }

    public float Contrast { get; init; } = 1f;
    public float Brightness { get; init; } = 1f;
    public float Saturation { get; init; } = 1f;
    public float Hue { get; init; }

    /// <summary>True when the live-preview approximation differs from identity.</summary>
    public bool HasAdjust =>
        Math.Abs(Contrast - 1) > 0.001 || Math.Abs(Brightness - 1) > 0.001 ||
        Math.Abs(Saturation - 1) > 0.001 || Math.Abs(Hue) > 0.001;

    [ObservableProperty]
    private Avalonia.Media.Imaging.Bitmap? _preview;

    public EditorFilterOption(string name, string? graph)
    {
        Name = name;
        Graph = graph;
    }
}

/// <summary>
/// ViewModel for the built-in clip editor: trim on a thumbnail timeline,
/// per-track audio volume/mute + audible range, playback speed (presets or manual),
/// aspect-ratio crop, video filters, and re-encode export via the bundled ffmpeg.
/// Preview frames are rendered into an Avalonia bitmap (vmem) — no native HWND.
/// </summary>
public partial class EditorViewModel : ViewModelBase, IDisposable
{
    private readonly LibraryViewModel _library;

    private LibVLC? _libVLC;
    private MediaPlayer? _mediaPlayer;
    private System.Timers.Timer? _positionTimer;
    private bool _disposed;

    /// <summary>Cancels thumbnail/preview generation when another clip is opened.</summary>
    private CancellationTokenSource? _thumbsCts;

    public ObservableCollection<EditorAudioTrack> AudioTracks { get; } = new();
    public ObservableCollection<Avalonia.Media.Imaging.Bitmap> TimelineThumbs { get; } = new();

    /// <summary>Renders decoded frames into an Avalonia bitmap (no native HWND).</summary>
    public Lag.Services.VlcVideoRenderer VideoRenderer { get; } = new();

    // ── Speed (presets + manual field) ──

    public IReadOnlyList<EditorSpeedOption> SpeedOptions { get; } = new[]
    {
        new EditorSpeedOption("0.5×", 0.5),
        new EditorSpeedOption("1×", 1.0),
        new EditorSpeedOption("1.5×", 1.5),
        new EditorSpeedOption("2×", 2.0),
    };

    [ObservableProperty]
    private EditorSpeedOption? _selectedSpeed;

    partial void OnSelectedSpeedChanged(EditorSpeedOption? value)
    {
        if (value != null && Math.Abs(EffectiveSpeed - value.Value) > 0.0001)
            SpeedText = value.Value.ToString("0.##", CultureInfo.InvariantCulture);
    }

    /// <summary>Manual speed input ("0.75", "3"…). Presets just write into it.</summary>
    [ObservableProperty]
    private string _speedText = "1";

    partial void OnSpeedTextChanged(string value)
    {
        double s = EffectiveSpeed;
        SelectedSpeed = SpeedOptions.FirstOrDefault(o => Math.Abs(o.Value - s) < 0.0001);
        ApplyPreviewRate();
    }

    /// <summary>Parsed speed, clamped to a sane ffmpeg-able range.</summary>
    public double EffectiveSpeed
    {
        get
        {
            string raw = (SpeedText ?? "1").Trim().Replace(',', '.');
            return double.TryParse(raw, NumberStyles.Float, CultureInfo.InvariantCulture, out double v)
                ? Math.Clamp(v, 0.1, 10.0)
                : 1.0;
        }
    }

    // ── Aspect (output frame) + interactive reframe (move / zoom / stretch the video in the frame) ──
    //
    // FORMAT = the output frame's aspect (16:9 / 4:3 / … or "Native" = source aspect). It behaves like
    // before: by default the video is scaled to COVER that frame (the old centred-crop look).
    // REFRAME = the video's rectangle WITHIN the output frame; the user can move / zoom / stretch it.
    // (Cropping a sub-region of the source is a separate tool, planned later.)

    public IReadOnlyList<EditorAspectOption> AspectOptions { get; }

    [ObservableProperty]
    private EditorAspectOption _selectedAspect;

    /// <summary>Changing the output aspect re-fits the video to COVER the new frame (old format look);
    /// the user can then move/zoom/stretch it within that frame.</summary>
    partial void OnSelectedAspectChanging(EditorAspectOption value) => PushUndo();
    partial void OnSelectedAspectChanged(EditorAspectOption value)
    {
        OnPropertyChanged(nameof(FrameAspect));
        if (_suppressUndo) return;   // load / undo-restore set this directly
        ResetReframe();
    }

    // The video's rectangle inside the OUTPUT FRAME, normalized to the frame. May extend past the
    // frame (values <0 or >1) when zoomed/panned — the frame clips it on export. Default = COVER.
    [ObservableProperty] private double _vidX;
    [ObservableProperty] private double _vidY;
    [ObservableProperty] private double _vidW = 1;
    [ObservableProperty] private double _vidH = 1;

    /// <summary>Raised when the reframe transform changes so the view redraws the overlay.</summary>
    public event Action? ReframeChanged;

    /// <summary>Source video pixel size (ffprobe) — used for the source aspect and export dims.</summary>
    [ObservableProperty] private int _sourceWidth;
    [ObservableProperty] private int _sourceHeight;

    /// <summary>Raw source pixel aspect (W/H) — the Crop tool edits over the FULL source.</summary>
    public double SourceAspectRaw => SourceWidth > 0 && SourceHeight > 0
        ? (double)SourceWidth / SourceHeight : 16.0 / 9.0;

    /// <summary>Effective source aspect AFTER the crop tool — what the reframe fills the frame with.</summary>
    public double SourceAspect => SourceWidth > 0 && SourceHeight > 0
        ? (double)(SourceWidth * CropW) / (SourceHeight * CropH) : 16.0 / 9.0;

    /// <summary>Output frame aspect — the chosen format, or the (cropped) source aspect for "Native".</summary>
    public double FrameAspect => SelectedAspect?.Ratio is double r && r > 0 ? r : SourceAspect;

    // ── Crop tool (separate "Кадрування" button): keep only a sub-region of the SOURCE. Applied
    //    BEFORE the format/reframe. Edited as its own box on the preview when the tool is active. ──

    /// <summary>When true, the preview shows the full source and the box edits the source crop.</summary>
    [ObservableProperty] private bool _cropToolActive;
    partial void OnCropToolActiveChanged(bool value) => ReframeChanged?.Invoke();

    [ObservableProperty] private double _cropX;
    [ObservableProperty] private double _cropY;
    [ObservableProperty] private double _cropW = 1;
    [ObservableProperty] private double _cropH = 1;

    /// <summary>True when the source crop is not the whole frame (drives export).</summary>
    public bool IsCropped => CropX > 0.002 || CropY > 0.002 || CropW < 0.998 || CropH < 0.998;

    /// <summary>Sets the source-crop rect (clamped to the frame, min 5%) and re-fits the reframe to
    /// the new cropped aspect.</summary>
    public void SetCrop(double x, double y, double w, double h)
    {
        w = Math.Clamp(w, 0.05, 1);
        h = Math.Clamp(h, 0.05, 1);
        x = Math.Clamp(x, 0, 1 - w);
        y = Math.Clamp(y, 0, 1 - h);
        CropX = x; CropY = y; CropW = w; CropH = h;
        OnPropertyChanged(nameof(IsCropped));
        OnPropertyChanged(nameof(SourceAspect));
        OnPropertyChanged(nameof(FrameAspect));
        ResetReframe();   // re-cover with the new cropped aspect (also raises ReframeChanged)
    }

    [RelayCommand]
    private void ToggleCropTool() => CropToolActive = !CropToolActive;

    [RelayCommand]
    private void ResetCrop() => SetCrop(0, 0, 1, 1);

    // ── Undo (Ctrl+Z): a stack of edit snapshots captured just BEFORE each change ──

    private sealed record TextSnap(string Text, double Nx, double Ny, int FontPercent, string Color);

    private sealed record EditSnap(double TrimStart, double TrimEnd, string Speed, int Aspect, int Filter,
        double CropX, double CropY, double CropW, double CropH,
        double VidX, double VidY, double VidW, double VidH,
        int Rotation, bool FlipH, bool FlipV,
        int ColorB, int ColorC, int ColorS, EditorCut[] Cuts, TextSnap[] Texts);

    private readonly Stack<EditSnap> _undo = new();
    private bool _suppressUndo;   // true while loading a clip or restoring an undo snapshot

    private static int IndexOf<T>(IReadOnlyList<T> list, T item)
    {
        for (int i = 0; i < list.Count; i++) if (Equals(list[i], item)) return i;
        return 0;
    }

    /// <summary>Records the current edit state so the NEXT change can be reverted with Ctrl+Z.
    /// Call right BEFORE applying a change (drag start, format/filter switch, reset).</summary>
    public void PushUndo()
    {
        if (_suppressUndo) return;
        _undo.Push(new EditSnap(TrimStartSec, TrimEndSec, SpeedText,
            IndexOf(AspectOptions, SelectedAspect), IndexOf(FilterOptions, SelectedFilter),
            CropX, CropY, CropW, CropH, VidX, VidY, VidW, VidH, Rotation, FlipH, FlipV,
            ColorBrightness, ColorContrast, ColorSaturation, Cuts.ToArray(),
            Texts.Select(t => new TextSnap(t.Text, t.Nx, t.Ny, t.FontPercent, t.Color)).ToArray()));
        UndoCommand.NotifyCanExecuteChanged();
    }

    private bool CanUndo() => _undo.Count > 0;

    [RelayCommand(CanExecute = nameof(CanUndo))]
    private void Undo()
    {
        if (_undo.Count == 0) return;
        var s = _undo.Pop();
        _suppressUndo = true;
        try
        {
            SelectedAspect = AspectOptions[Math.Clamp(s.Aspect, 0, AspectOptions.Count - 1)];
            SelectedFilter = FilterOptions[Math.Clamp(s.Filter, 0, FilterOptions.Count - 1)];
            SpeedText = s.Speed;
            TrimStartSec = s.TrimStart;
            TrimEndSec = s.TrimEnd;
            CropX = s.CropX; CropY = s.CropY; CropW = s.CropW; CropH = s.CropH;
            VidX = s.VidX; VidY = s.VidY; VidW = s.VidW; VidH = s.VidH;
            Rotation = s.Rotation; FlipH = s.FlipH; FlipV = s.FlipV;
            ColorBrightness = s.ColorB; ColorContrast = s.ColorC; ColorSaturation = s.ColorS;
            Cuts.Clear();
            foreach (var c in s.Cuts) Cuts.Add(c);
            SelectedText = null;
            Texts.Clear();
            foreach (var ts in s.Texts)
                Texts.Add(new EditorTextItem { Text = ts.Text, Nx = ts.Nx, Ny = ts.Ny, FontPercent = ts.FontPercent, Color = ts.Color });
        }
        finally { _suppressUndo = false; }

        OnPropertyChanged(nameof(IsCropped));
        OnPropertyChanged(nameof(IsReframed));
        OnPropertyChanged(nameof(SourceAspect));
        OnPropertyChanged(nameof(FrameAspect));
        OnPropertyChanged(nameof(HasCuts));
        OnPropertyChanged(nameof(HasTexts));
        ApplyPreviewFilter();
        ReframeChanged?.Invoke();
        CutsChanged?.Invoke();
        TextsChanged?.Invoke();
        UndoCommand.NotifyCanExecuteChanged();
    }

    /// <summary>Default placement: the video scaled to COVER the output frame, centred (= the old
    /// centred-crop look), as a frame-normalized rect.</summary>
    private (double x, double y, double w, double h) CoverPlacement()
    {
        double sa = SourceAspect, fa = FrameAspect;
        if (sa >= fa) { double w = sa / fa; return ((1 - w) / 2, 0, w, 1); }
        double h = fa / sa; return (0, (1 - h) / 2, 1, h);
    }

    /// <summary>True when the video was moved/zoomed/stretched off its default cover placement.</summary>
    public bool IsReframed
    {
        get
        {
            var (x, y, w, h) = CoverPlacement();
            return Math.Abs(VidX - x) > 0.003 || Math.Abs(VidY - y) > 0.003 ||
                   Math.Abs(VidW - w) > 0.003 || Math.Abs(VidH - h) > 0.003;
        }
    }

    /// <summary>Atomically sets the video rect. This is only a loose backstop — the precise "stay
    /// within the preview field" clamp lives in the view (which knows the stage size).</summary>
    public void SetVideoRect(double x, double y, double w, double h)
    {
        w = Math.Clamp(w, 0.05, 6);
        h = Math.Clamp(h, 0.05, 6);
        x = Math.Clamp(x, -3, 3);
        y = Math.Clamp(y, -3, 3);
        VidX = x; VidY = y; VidW = w; VidH = h;
        OnPropertyChanged(nameof(IsReframed));
        ReframeChanged?.Invoke();
    }

    [RelayCommand]
    private void ResetReframe()
    {
        var (x, y, w, h) = CoverPlacement();
        SetVideoRect(x, y, w, h);
    }

    // ── Rotate / flip the final output ──
    [ObservableProperty] private int _rotation;   // 0 / 90 / 180 / 270 (clockwise)
    [ObservableProperty] private bool _flipH;
    [ObservableProperty] private bool _flipV;
    partial void OnRotationChanged(int value) => ReframeChanged?.Invoke();
    partial void OnFlipHChanged(bool value) => ReframeChanged?.Invoke();
    partial void OnFlipVChanged(bool value) => ReframeChanged?.Invoke();

    [RelayCommand] private void RotateRight() { PushUndo(); Rotation = (Rotation + 90) % 360; }
    [RelayCommand] private void RotateLeft()  { PushUndo(); Rotation = (Rotation + 270) % 360; }
    [RelayCommand] private void ToggleFlipH() { PushUndo(); FlipH = !FlipH; }
    [RelayCommand] private void ToggleFlipV() { PushUndo(); FlipV = !FlipV; }

    // ── Filters ──

    public IReadOnlyList<EditorFilterOption> FilterOptions { get; } = new[]
    {
        new EditorFilterOption("Normal", null),
        new EditorFilterOption("Grayscale", "hue=s=0") { Saturation = 0f },
        new EditorFilterOption("Sepia", "colorchannelmixer=.393:.769:.189:0:.349:.686:.168:0:.272:.534:.131")
            { Saturation = 0.4f, Hue = 30f, Brightness = 1.03f },
        new EditorFilterOption("Warm", "colortemperature=temperature=4500") { Hue = -12f, Saturation = 1.1f },
        new EditorFilterOption("Cold", "colortemperature=temperature=8500") { Hue = 12f, Saturation = 1.05f },
        new EditorFilterOption("Vibrant", "eq=saturation=1.6:contrast=1.05") { Saturation = 1.6f, Contrast = 1.05f },
        new EditorFilterOption("Dramatic", "eq=contrast=1.35:brightness=-0.06:saturation=0.8")
            { Contrast = 1.35f, Brightness = 0.94f, Saturation = 0.8f },
        new EditorFilterOption("Bright", "eq=brightness=0.08:saturation=1.15") { Brightness = 1.08f, Saturation = 1.15f },
    };

    [ObservableProperty]
    private EditorFilterOption _selectedFilter;

    partial void OnSelectedFilterChanging(EditorFilterOption value) => PushUndo();
    partial void OnSelectedFilterChanged(EditorFilterOption value) => ApplyPreviewFilter();

    // ── Manual colour sliders (percent, 100 = neutral) — combine with the selected preset filter ──
    [ObservableProperty] private int _colorBrightness = 100;
    [ObservableProperty] private int _colorContrast = 100;
    [ObservableProperty] private int _colorSaturation = 100;
    partial void OnColorBrightnessChanged(int value) => ApplyPreviewFilter();
    partial void OnColorContrastChanged(int value) => ApplyPreviewFilter();
    partial void OnColorSaturationChanged(int value) => ApplyPreviewFilter();

    /// <summary>True when any manual colour slider is off its neutral 100%.</summary>
    public bool HasColorAdjust => ColorBrightness != 100 || ColorContrast != 100 || ColorSaturation != 100;

    [RelayCommand]
    private void ResetColor()
    {
        PushUndo();
        ColorBrightness = 100; ColorContrast = 100; ColorSaturation = 100;
    }

    // ── Text / captions ──

    public ObservableCollection<EditorTextItem> Texts { get; } = new();

    [ObservableProperty]
    private EditorTextItem? _selectedText;

    public bool HasTexts => Texts.Count > 0;

    /// <summary>Preset colour swatches for captions (Hex for the command, Brush for the button fill).</summary>
    public IReadOnlyList<EditorTextColor> TextColors { get; } = new[]
    {
        "#FFFFFF", "#000000", "#FFD43B", "#FF6B6B", "#4DABF7", "#51CF66"
    }.Select(h => new EditorTextColor(h, Avalonia.Media.Brush.Parse(h))).ToList();

    /// <summary>Raised when the caption list or any caption changes so the view redraws the overlays.</summary>
    public event Action? TextsChanged;

    private void OnTextItemChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e) =>
        TextsChanged?.Invoke();

    [RelayCommand]
    private void AddText()
    {
        PushUndo();
        var t = new EditorTextItem { Text = Localizer.Get("Editor_TextDefault") };
        Texts.Add(t);
        SelectedText = t;
    }

    [RelayCommand]
    private void RemoveSelectedText()
    {
        if (SelectedText is not { } t) return;
        PushUndo();
        SelectedText = null;
        Texts.Remove(t);
    }

    /// <summary>Sets the selected caption's colour (called from the swatch buttons).</summary>
    [RelayCommand]
    private void SetTextColor(string? hex)
    {
        if (SelectedText is { } t && !string.IsNullOrEmpty(hex)) t.Color = hex;
    }

    /// <summary>Selected tools tab in the right panel: 0 = Clip, 1 = Filters (UI-only).</summary>
    [ObservableProperty]
    private int _selectedToolsTab;

    /// <summary>Video strip selected on the timeline (Shift+click).</summary>
    [ObservableProperty]
    private bool _isVideoSelected;

    // ── Clip / playback state ──

    [ObservableProperty]
    private ReplayClip? _currentClip;

    [ObservableProperty]
    private MediaPlayer? _player;

    [ObservableProperty]
    private bool _isPlaying;

    /// <summary>Preview position 0..1 (same semantics as the player view).</summary>
    [ObservableProperty]
    private double _position;

    partial void OnPositionChanged(double value)
    {
        if (_isUpdatingPosition) return;
        _pendingSeekPosition = (float)Math.Clamp(value, 0.0, 1.0);
        StartVlcApplyTimer();
    }

    private bool _isUpdatingPosition;

    [ObservableProperty]
    private string _timeDisplay = "0:00 / 0:00";

    /// <summary>Source clip duration in seconds (from ffprobe; fallback: clip metadata).</summary>
    [ObservableProperty]
    private double _durationSec = 1;

    [ObservableProperty]
    private double _trimStartSec;

    partial void OnTrimStartSecChanged(double value)
    {
        NotifyTrimDisplays();

        // Magnet (start only): the playhead always follows the start handle, so the
        // preview shows exactly the first frame of the future clip.
        if (DurationSec > 0)
        {
            double pos = Math.Clamp(value / DurationSec, 0, 1);
            _pendingSeekPosition = (float)pos;
            StartVlcApplyTimer();

            _isUpdatingPosition = true;
            Position = pos;
            _isUpdatingPosition = false;
        }
    }

    [ObservableProperty]
    private double _trimEndSec = 1;

    partial void OnTrimEndSecChanged(double value) => NotifyTrimDisplays();

    public string TrimStartDisplay => FormatTime(TimeSpan.FromSeconds(TrimStartSec));
    public string TrimEndDisplay => FormatTime(TimeSpan.FromSeconds(TrimEndSec));
    public string TrimDurationDisplay => FormatTime(TimeSpan.FromSeconds(Math.Max(0, TrimEndSec - TrimStartSec)));

    private void NotifyTrimDisplays()
    {
        OnPropertyChanged(nameof(TrimStartDisplay));
        OnPropertyChanged(nameof(TrimEndDisplay));
        OnPropertyChanged(nameof(TrimDurationDisplay));
    }

    // ── Export state ──

    [ObservableProperty]
    [NotifyCanExecuteChangedFor(nameof(ExportCommand))]
    private bool _isExporting;

    [ObservableProperty]
    private int _exportProgress;

    /// <summary>Status line next to the export button ("Exporting… 42%", "Saved: file.mp4").</summary>
    [ObservableProperty]
    private string _exportStatus = string.Empty;

    private CancellationTokenSource? _exportCts;

    // ── Export options (the Export button's flyout) ──
    public IReadOnlyList<EditorExportRes> ExportResolutionOptions { get; }

    [ObservableProperty] private EditorExportRes _selectedExportResolution = null!;

    public IReadOnlyList<string> ExportFormatOptions { get; } = new[] { "mp4", "mkv", "mov" };

    [ObservableProperty] private string _selectedExportFormat = "mp4";

    /// <summary>Export an animated GIF instead of a video (no audio).</summary>
    [ObservableProperty] private bool _exportAsGif;

    public EditorViewModel(LibraryViewModel library)
    {
        Title = "Editor";
        _library = library;
        _selectedSpeed = SpeedOptions[1]; // 1×
        _selectedFilter = FilterOptions[0]; // Normal

        AspectOptions = new[]
        {
            new EditorAspectOption(Localizer.Get("Option_Native"), null, null),
            new EditorAspectOption("16:9", 16.0 / 9.0, "16:9"),
            new EditorAspectOption("9:16", 9.0 / 16.0, "9:16"),
            new EditorAspectOption("16:10", 16.0 / 10.0, "16:10"),
            new EditorAspectOption("21:9", 21.0 / 9.0, "21:9"),
            new EditorAspectOption("4:3", 4.0 / 3.0, "4:3"),
            new EditorAspectOption("1:1", 1.0, "1:1"),
        };
        _selectedAspect = AspectOptions[0];

        ExportResolutionOptions = new[]
        {
            new EditorExportRes(Localizer.Get("Option_Native"), 0),
            new EditorExportRes("1080p", 1080),
            new EditorExportRes("720p", 720),
            new EditorExportRes("480p", 480),
        };
        _selectedExportResolution = ExportResolutionOptions[0];

        // Per-track volume/mute changes feed the live preview.
        AudioTracks.CollectionChanged += (_, e) =>
        {
            if (e.NewItems != null)
                foreach (EditorAudioTrack t in e.NewItems)
                    t.PropertyChanged += OnTrackPropertyChanged;
            if (e.OldItems != null)
                foreach (EditorAudioTrack t in e.OldItems)
                    t.PropertyChanged -= OnTrackPropertyChanged;
            ApplyPreviewVolume();
        };

        // Caption changes (add/remove/edit) redraw the preview overlays.
        Texts.CollectionChanged += (_, e) =>
        {
            if (e.NewItems != null)
                foreach (EditorTextItem t in e.NewItems)
                    t.PropertyChanged += OnTextItemChanged;
            if (e.OldItems != null)
                foreach (EditorTextItem t in e.OldItems)
                    t.PropertyChanged -= OnTextItemChanged;
            OnPropertyChanged(nameof(HasTexts));
            TextsChanged?.Invoke();
        };
    }

    private void OnTrackPropertyChanged(object? sender, System.ComponentModel.PropertyChangedEventArgs e)
    {
        if (e.PropertyName is nameof(EditorAudioTrack.Volume) or nameof(EditorAudioTrack.IsMuted))
            ApplyPreviewVolume();
    }

    // ───────────── Live preview (libvlc-side approximations of the export) ─────────────

    private void ApplyPreviewRate()
    {
        try { _mediaPlayer?.SetRate((float)Math.Clamp(EffectiveSpeed, 0.25, 4.0)); }
        catch (Exception ex) { Debug.WriteLine($"[Editor] SetRate failed: {ex.Message}"); }
    }

    /// <summary>Approximates the chosen filter with libvlc's adjust filter.</summary>
    private void ApplyPreviewFilter()
    {
        if (_mediaPlayer == null) return;
        try
        {
            // Effective adjust = the preset filter combined with the manual sliders (×, neutral=1).
            var f = SelectedFilter;
            if (f.HasAdjust || HasColorAdjust)
            {
                _mediaPlayer.SetAdjustInt(VideoAdjustOption.Enable, 1);
                _mediaPlayer.SetAdjustFloat(VideoAdjustOption.Contrast, f.Contrast * (ColorContrast / 100f));
                _mediaPlayer.SetAdjustFloat(VideoAdjustOption.Brightness, f.Brightness * (ColorBrightness / 100f));
                _mediaPlayer.SetAdjustFloat(VideoAdjustOption.Saturation, f.Saturation * (ColorSaturation / 100f));
                _mediaPlayer.SetAdjustFloat(VideoAdjustOption.Hue, f.Hue);
            }
            else
            {
                _mediaPlayer.SetAdjustInt(VideoAdjustOption.Enable, 0);
            }
        }
        catch (Exception ex) { Debug.WriteLine($"[Editor] adjust failed: {ex.Message}"); }
    }

    private const int NoVlcTrack = int.MinValue;

    /// <summary>
    /// Drives the preview's audible audio. libvlc plays ONE audio track at a time, so muting a row
    /// can't just lower volume — we must SWITCH the active track to match what's enabled:
    ///   • only System enabled → play the system track,
    ///   • only Mic enabled    → play the mic track,
    ///   • both enabled        → play the pre-mixed "All" track (stream 0) when the clip has one
    ///                            (native VFR layout), else the first enabled row (OBS files have no mix).
    /// Volume follows the loudest enabled row. Export still does the true per-track mix + gain; this is
    /// the closest a single-track player can preview. (Previously this only set Volume, so the default
    /// "All" mix kept playing and you heard system+mic no matter which row you toggled.)
    /// </summary>
    private void ApplyPreviewVolume()
    {
        if (_mediaPlayer == null) return;
        try
        {
            var unmuted = AudioTracks.Where(t => !t.IsMuted && t.Volume > 0).ToList();
            if (unmuted.Count == 0) { _mediaPlayer.Mute = true; return; }
            _mediaPlayer.Mute = false;

            int playStream;
            if (unmuted.Count == 1)
            {
                playStream = unmuted[0].StreamIndex;
            }
            else
            {
                // Several rows enabled. The native VFR file carries a pre-mixed "All" track at
                // stream 0 (the editor only exposes streams ≥ 1), so play that for a real blend.
                // OBS/single files have no mix track → fall back to the first enabled row.
                bool hasMixTrack = AudioTracks.All(t => t.StreamIndex >= 1);
                playStream = hasMixTrack ? 0 : unmuted[0].StreamIndex;
            }

            int id = LibVlcAudioId(playStream);
            if (id != NoVlcTrack) _mediaPlayer.SetAudioTrack(id);
            _mediaPlayer.Volume = Math.Clamp(unmuted.Max(t => t.Volume), 0, 100);
        }
        catch (Exception ex) { Debug.WriteLine($"[Editor] preview audio failed: {ex.Message}"); }
    }

    /// <summary>Maps a source audio-stream index to libvlc's audio-track Id. libvlc's track
    /// descriptions (after the leading "Disable" entry, Id = -1) are in source-stream order, so the
    /// Nth real description is stream N. Returns <see cref="NoVlcTrack"/> if not resolvable yet
    /// (descriptions populate once playback starts).</summary>
    private int LibVlcAudioId(int streamIndex)
    {
        var descs = _mediaPlayer?.AudioTrackDescription;
        if (descs == null) return NoVlcTrack;
        int i = 0;
        foreach (var d in descs)
        {
            if (d.Id < 0) continue; // skip the "Disable" pseudo-track
            if (i == streamIndex) return d.Id;
            i++;
        }
        return NoVlcTrack;
    }

    private void ApplyAllPreview()
    {
        ApplyPreviewRate();
        ApplyPreviewFilter();
        ApplyPreviewVolume();
    }

    // ───────────── Opening a clip ─────────────

    /// <summary>Loads a clip into the editor: probes streams, builds the timeline, prepares preview.</summary>
    public void OpenClip(ReplayClip clip)
    {
        EnsureVlc();
        CurrentClip = clip;

        // Reset edit state for the new clip (no undo capture during the reset; fresh history).
        _undo.Clear();
        _suppressUndo = true;
        TrimStartSec = 0;
        DurationSec = Math.Max(1, clip.Duration.TotalSeconds);
        TrimEndSec = DurationSec;
        SpeedText = "1";
        SourceWidth = 0;
        SourceHeight = 0;
        CropToolActive = false;
        SelectedAspect = AspectOptions[0];
        SetCrop(0, 0, 1, 1);   // resets the source crop AND re-fits the reframe to cover
        Rotation = 0; FlipH = false; FlipV = false;
        ColorBrightness = 100; ColorContrast = 100; ColorSaturation = 100;
        ClearCuts();
        SelectedText = null;
        Texts.Clear();
        SelectedFilter = FilterOptions[0];
        IsVideoSelected = false;
        ExportStatus = string.Empty;
        ExportProgress = 0;
        _suppressUndo = false;
        UndoCommand.NotifyCanExecuteChanged();

        // Default to a single mixed track until ffprobe tells us better.
        AudioTracks.Clear();
        AudioTracks.Add(new EditorAudioTrack(0, Localizer.Get("Editor_TrackAudio"), DurationSec));

        if (_mediaPlayer != null && _libVLC != null)
        {
            var oldMedia = _mediaPlayer.Media;
            var media = new Media(_libVLC, new Uri(clip.FilePath));
            // SOFTWARE decode in the EDITOR (unlike the player's d3d11va): libvlc's "adjust" colour
            // filter and crop only apply to software-decoded frames — hardware (d3d11va) frames bypass
            // the filter chain, which is exactly why the filter + aspect-ratio previews stopped working
            // after the player switched to HW decode. Smooth 120fps matters less here than seeing the
            // edit; export still does the real ffmpeg filter/crop.
            media.AddOption(":avcodec-hw=none");
            _mediaPlayer.Media = media;
            media.Dispose();
            oldMedia?.Dispose();
        }

        _ = ProbeAsync(clip);
        _ = GenerateTimelineThumbsAsync(clip);
    }

    /// <summary>ffprobe: exact duration + audio stream count → track rows.</summary>
    private async Task ProbeAsync(ReplayClip clip)
    {
        try
        {
            var info = await FFMpegCore.FFProbe.AnalyseAsync(clip.FilePath);
            if (CurrentClip != clip) return; // user switched clips while probing

            int audioStreams = info.AudioStreams.Count;
            double duration = info.Duration.TotalSeconds > 0 ? info.Duration.TotalSeconds : DurationSec;
            int vw = info.PrimaryVideoStream?.Width ?? 0;
            int vh = info.PrimaryVideoStream?.Height ?? 0;

            await Dispatcher.UIThread.InvokeAsync(() =>
            {
                SourceWidth = vw;
                SourceHeight = vh;
                OnPropertyChanged(nameof(SourceAspectRaw));
                OnPropertyChanged(nameof(SourceAspect));
                OnPropertyChanged(nameof(FrameAspect));
                ResetReframe();   // re-cover with the now-known real source aspect
                bool endWasAtMax = Math.Abs(TrimEndSec - DurationSec) < 0.05;
                DurationSec = Math.Max(1, duration);
                if (endWasAtMax || TrimEndSec > DurationSec) TrimEndSec = DurationSec;
                TrimStartSec = Math.Min(TrimStartSec, TrimEndSec);

                AudioTracks.Clear();
                if (audioStreams >= 3)
                {
                    // Native VFR engine: stream 0 = "All" (mixed, the default playback track),
                    // 1 = system, 2 = mic. Edit the two source tracks; "All" plays automatically.
                    AudioTracks.Add(new EditorAudioTrack(1, Localizer.Get("Editor_TrackSystem"), DurationSec));
                    AudioTracks.Add(new EditorAudioTrack(2, Localizer.Get("Editor_TrackMic"), DurationSec));
                }
                else if (audioStreams == 2)
                {
                    // OBS "separate audio tracks": 0 = system sound, 1 = microphone.
                    AudioTracks.Add(new EditorAudioTrack(0, Localizer.Get("Editor_TrackSystem"), DurationSec));
                    AudioTracks.Add(new EditorAudioTrack(1, Localizer.Get("Editor_TrackMic"), DurationSec));
                }
                else if (audioStreams == 1)
                {
                    AudioTracks.Add(new EditorAudioTrack(0, Localizer.Get("Editor_TrackAudio"), DurationSec));
                }
            });
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[Editor] ffprobe failed: {ex.Message}");
        }
    }

    /// <summary>Extracts evenly spaced timeline frames, then renders the filter previews.</summary>
    private async Task GenerateTimelineThumbsAsync(ReplayClip clip)
    {
        _thumbsCts?.Cancel();
        var cts = new CancellationTokenSource();
        _thumbsCts = cts;

        await Dispatcher.UIThread.InvokeAsync(() =>
        {
            TimelineThumbs.Clear();
            foreach (var f in FilterOptions) f.Preview = null;
        });

        const int count = 10;
        string tempDir = Path.Combine(Path.GetTempPath(), "Lag-editor");
        Directory.CreateDirectory(tempDir);

        for (int i = 0; i < count; i++)
        {
            if (cts.IsCancellationRequested || CurrentClip != clip) return;

            double t = DurationSec * (i + 0.5) / count;
            string outPng = Path.Combine(tempDir, $"thumb-{Guid.NewGuid():N}.png");
            string args = $"-ss {t.ToString(CultureInfo.InvariantCulture)} -i \"{clip.FilePath}\" " +
                          $"-frames:v 1 -vf scale=160:-2 -y \"{outPng}\"";

            try
            {
                int exit = await RunFfmpegAsync(args, cts.Token, null, expectedDurationSec: 0);
                if (exit == 0 && File.Exists(outPng))
                {
                    var bmp = new Avalonia.Media.Imaging.Bitmap(outPng);
                    await Dispatcher.UIThread.InvokeAsync(() =>
                    {
                        if (CurrentClip == clip && !cts.IsCancellationRequested)
                            TimelineThumbs.Add(bmp);
                        else
                            bmp.Dispose();
                    });
                    try { File.Delete(outPng); } catch { /* temp leftovers are harmless */ }
                }
            }
            catch (OperationCanceledException) { return; }
            catch (Exception ex)
            {
                Debug.WriteLine($"[Editor] thumb {i} failed: {ex.Message}");
            }
        }

        await GenerateFilterPreviewsAsync(clip, tempDir, cts.Token);
    }

    /// <summary>One mid-clip frame, run through every filter graph → little preview cards.</summary>
    private async Task GenerateFilterPreviewsAsync(ReplayClip clip, string tempDir, CancellationToken ct)
    {
        string basePng = Path.Combine(tempDir, $"fbase-{Guid.NewGuid():N}.png");
        string baseArgs = $"-ss {(DurationSec / 2).ToString(CultureInfo.InvariantCulture)} -i \"{clip.FilePath}\" " +
                          $"-frames:v 1 -vf scale=128:-2 -y \"{basePng}\"";

        try
        {
            if (await RunFfmpegAsync(baseArgs, ct, null, 0) != 0 || !File.Exists(basePng)) return;

            foreach (var filter in FilterOptions)
            {
                if (ct.IsCancellationRequested || CurrentClip != clip) break;

                string srcPng = basePng;
                string outPng = basePng;
                if (filter.Graph != null)
                {
                    outPng = Path.Combine(tempDir, $"f-{Guid.NewGuid():N}.png");
                    if (await RunFfmpegAsync($"-i \"{basePng}\" -vf \"{filter.Graph}\" -y \"{outPng}\"", ct, null, 0) != 0)
                        continue;
                }

                if (!File.Exists(outPng)) continue;
                var bmp = new Avalonia.Media.Imaging.Bitmap(outPng);
                var target = filter;
                await Dispatcher.UIThread.InvokeAsync(() =>
                {
                    if (CurrentClip == clip && !ct.IsCancellationRequested)
                        target.Preview = bmp;
                    else
                        bmp.Dispose();
                });
                if (outPng != basePng)
                    try { File.Delete(outPng); } catch { }
            }
        }
        catch (OperationCanceledException) { }
        catch (Exception ex)
        {
            Debug.WriteLine($"[Editor] filter previews failed: {ex.Message}");
        }
        finally
        {
            try { File.Delete(basePng); } catch { }
        }
    }

    // ───────────── Preview playback (throttled-VLC pattern) ─────────────

    private void EnsureVlc()
    {
        if (_libVLC != null) return;

        try
        {
            LibVLCSharp.Shared.Core.Initialize();
            // No --avcodec-hw here: VLC ignores it for vmem players (the callbacks attach
            // forces it back to "none"). Hardware decode is enabled per-media on load.
            _libVLC = new LibVLC(new[] { "--no-video-title-show", "--no-osd" });
            _mediaPlayer = new MediaPlayer(_libVLC);
            VideoRenderer.Attach(_mediaPlayer);
            Player = _mediaPlayer;

            _positionTimer = new System.Timers.Timer(250);
            _positionTimer.Elapsed += (_, _) => UpdatePlaybackPosition();

            _mediaPlayer.Playing += (_, _) => Dispatcher.UIThread.Post(() =>
            {
                IsPlaying = true;
                _positionTimer?.Start();

                // libvlc resets adjust/volume per playback — re-assert immediately.
                ApplyPreviewFilter();
                ApplyPreviewVolume();
                // Re-assert adjust a beat later too: applied at the instant Playing fires (the vmem
                // vout isn't fully up yet) it often doesn't "stick", so the colour filter appeared to
                // do nothing. Rate is also deferred — forcing 2× at t=0 makes the decoder drop the
                // first GOP and flash blank ("gray screen on speeded start").
                DispatcherTimer.RunOnce(() =>
                {
                    ApplyPreviewFilter();
                    ApplyPreviewRate();
                }, TimeSpan.FromMilliseconds(300));
            });
            _mediaPlayer.Paused += (_, _) => Dispatcher.UIThread.Post(() => { IsPlaying = false; _positionTimer?.Stop(); });
            _mediaPlayer.Stopped += (_, _) => Dispatcher.UIThread.Post(() =>
            {
                IsPlaying = false;
                _positionTimer?.Stop();
            });
            _mediaPlayer.EndReached += (_, _) => Dispatcher.UIThread.Post(() =>
            {
                IsPlaying = false;
                _positionTimer?.Stop();
            });
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[Editor] VLC init failed: {ex.Message}");
        }
    }

    private DispatcherTimer? _vlcApplyTimer;
    private float? _pendingSeekPosition;

    private void StartVlcApplyTimer()
    {
        if (_vlcApplyTimer == null)
        {
            _vlcApplyTimer = new DispatcherTimer { Interval = TimeSpan.FromMilliseconds(80) };
            _vlcApplyTimer.Tick += (_, _) => ApplyPendingVlcWrites();
        }
        if (!_vlcApplyTimer.IsEnabled)
        {
            ApplyPendingVlcWrites();
            _vlcApplyTimer.Start();
        }
    }

    private void ApplyPendingVlcWrites()
    {
        if (_mediaPlayer == null || _disposed)
        {
            _pendingSeekPosition = null;
            _vlcApplyTimer?.Stop();
            return;
        }

        try
        {
            if (_pendingSeekPosition is float pos)
            {
                _pendingSeekPosition = null;
                if (_mediaPlayer.IsSeekable)
                    _mediaPlayer.Position = pos;
                return;
            }
        }
        catch (Exception ex)
        {
            Debug.WriteLine($"[Editor] VLC write failed: {ex.Message}");
        }

        _vlcApplyTimer?.Stop();
    }

    private void UpdatePlaybackPosition()
    {
        if (_mediaPlayer == null) return;

        float position = _mediaPlayer.Position;
        var current = TimeSpan.FromMilliseconds(Math.Max(0, _mediaPlayer.Time));
        var total = TimeSpan.FromMilliseconds(Math.Max(0, _mediaPlayer.Length));

        Dispatcher.UIThread.Post(() =>
        {
            double posSec = position * DurationSec;

            // Skip over removed (cut) sections during playback so the preview matches the export —
            // when the playhead enters a cut, jump straight to its end.
            if (IsPlaying && DurationSec > 0)
            {
                foreach (var c in Cuts)
                {
                    if (posSec >= c.StartSec - 0.03 && posSec < c.EndSec - 0.03)
                    {
                        double target = Math.Clamp(Math.Min(c.EndSec, TrimEndSec) / DurationSec, 0, 1);
                        try { if (_mediaPlayer is { IsSeekable: true }) _mediaPlayer.Position = (float)target; }
                        catch (Exception ex) { Debug.WriteLine($"[Editor] cut-skip failed: {ex.Message}"); }
                        _isUpdatingPosition = true;
                        Position = target;
                        _isUpdatingPosition = false;
                        TimeDisplay = $"{FormatTime(TimeSpan.FromSeconds(Math.Min(c.EndSec, TrimEndSec)))} / {FormatTime(total)}";
                        return;
                    }
                }
            }

            // The playhead lives strictly between the trim handles: when playback
            // reaches the end handle, park exactly on it and pause.
            if (IsPlaying && DurationSec > 0 && posSec >= TrimEndSec)
            {
                try
                {
                    _mediaPlayer?.SetPause(true);
                    if (_mediaPlayer is { IsSeekable: true })
                        _mediaPlayer.Position = (float)Math.Clamp(TrimEndSec / DurationSec, 0, 1);
                }
                catch (Exception ex) { Debug.WriteLine($"[Editor] end-stop failed: {ex.Message}"); }

                _isUpdatingPosition = true;
                Position = Math.Clamp(TrimEndSec / DurationSec, 0, 1);
                _isUpdatingPosition = false;
                TimeDisplay = $"{FormatTime(TimeSpan.FromSeconds(TrimEndSec))} / {FormatTime(total)}";
                return;
            }

            _isUpdatingPosition = true;
            Position = position;
            _isUpdatingPosition = false;
            TimeDisplay = $"{FormatTime(current)} / {FormatTime(total)}";
        });
    }

    public void StartPreview() => _mediaPlayer?.Play();

    public void StopPreview() => _mediaPlayer?.Stop();

    [RelayCommand]
    private void PlayPause()
    {
        if (_mediaPlayer == null) return;

        if (_mediaPlayer.IsPlaying)
        {
            _mediaPlayer.Pause();
            return;
        }

        // After a natural end libvlc is in the Ended state where Play() is a no-op.
        if (_mediaPlayer.State == VLCState.Ended)
        {
            _mediaPlayer.Stop();
            _pendingSeekPosition = (float)Math.Clamp(TrimStartSec / Math.Max(0.1, DurationSec), 0, 1);
            StartVlcApplyTimer();
            _mediaPlayer.Play();
            return;
        }

        // Resuming from outside the trimmed range restarts at the start handle.
        double posSec = CurrentPreviewSec;
        if (posSec < TrimStartSec - 0.05 || posSec >= TrimEndSec - 0.05)
        {
            _pendingSeekPosition = (float)Math.Clamp(TrimStartSec / Math.Max(0.1, DurationSec), 0, 1);
            StartVlcApplyTimer();
        }

        _mediaPlayer.Play();
    }

    /// <summary>Current preview time in seconds (for "set trim point here" + the magnet).</summary>
    private double CurrentPreviewSec =>
        _mediaPlayer is { } mp && mp.Length > 0 ? mp.Time / 1000.0 : Position * DurationSec;

    [RelayCommand]
    private void SetTrimStartHere()
    {
        double t = Math.Clamp(CurrentPreviewSec, 0, DurationSec);
        TrimStartSec = Math.Min(t, Math.Max(0, TrimEndSec - 0.2));
    }

    [RelayCommand]
    private void SetTrimEndHere()
    {
        double t = Math.Clamp(CurrentPreviewSec, 0, DurationSec);
        TrimEndSec = Math.Max(t, Math.Min(DurationSec, TrimStartSec + 0.2));
    }

    [RelayCommand]
    private void ResetTrim()
    {
        TrimStartSec = 0;
        TrimEndSec = DurationSec;
        ClearCuts();
    }

    // ── Cut out the middle: removed sections inside the trim, concatenated on export ──

    /// <summary>Removed sections (seconds of the full source). Drawn as dark bands on the timeline.</summary>
    public ObservableCollection<EditorCut> Cuts { get; } = new();

    public bool HasCuts => Cuts.Count > 0;

    /// <summary>Raised when the cut list changes so the view redraws the timeline bands.</summary>
    public event Action? CutsChanged;

    private double? _pendingCutStart;

    /// <summary>Highlights the Cut button while the first edge of a new cut is set.</summary>
    [ObservableProperty]
    private bool _isMarkingCut;

    /// <summary>Two-click cut: first press marks the start at the playhead, second marks the end and
    /// adds the cut. (A live preview band is drawn between the start and the playhead.)</summary>
    [RelayCommand]
    private void ToggleCut()
    {
        double t = Math.Clamp(CurrentPreviewSec, TrimStartSec, TrimEndSec);
        if (_pendingCutStart is null)
        {
            _pendingCutStart = t;
            IsMarkingCut = true;
            CutsChanged?.Invoke();
            return;
        }

        double a = Math.Min(_pendingCutStart.Value, t);
        double b = Math.Max(_pendingCutStart.Value, t);
        _pendingCutStart = null;
        IsMarkingCut = false;
        if (b - a < 0.05) { CutsChanged?.Invoke(); return; }   // too small — discard
        PushUndo();
        AddCut(a, b);
    }

    /// <summary>The pending cut's first edge (for the live band), or null when not marking.</summary>
    public double? PendingCutStart => _pendingCutStart;

    private void AddCut(double a, double b)
    {
        var list = Cuts.ToList();
        list.Add(new EditorCut(a, b));
        list.Sort((x, y) => x.StartSec.CompareTo(y.StartSec));

        // Merge overlapping / touching cuts.
        var merged = new List<EditorCut>();
        foreach (var c in list)
        {
            if (merged.Count > 0 && c.StartSec <= merged[^1].EndSec + 0.01)
                merged[^1] = new EditorCut(merged[^1].StartSec, Math.Max(merged[^1].EndSec, c.EndSec));
            else
                merged.Add(c);
        }

        Cuts.Clear();
        foreach (var c in merged) Cuts.Add(c);
        OnPropertyChanged(nameof(HasCuts));
        CutsChanged?.Invoke();
    }

    /// <summary>Removes a single cut (the view calls this when a band is right-clicked).</summary>
    public void RemoveCut(EditorCut cut)
    {
        if (!Cuts.Contains(cut)) return;
        PushUndo();
        Cuts.Remove(cut);
        OnPropertyChanged(nameof(HasCuts));
        CutsChanged?.Invoke();
    }

    [RelayCommand]
    private void ClearCuts()
    {
        _pendingCutStart = null;
        IsMarkingCut = false;
        if (Cuts.Count == 0) { CutsChanged?.Invoke(); return; }
        Cuts.Clear();
        OnPropertyChanged(nameof(HasCuts));
        CutsChanged?.Invoke();
    }

    /// <summary>The kept sub-ranges of [start,end] after the cuts are removed (in source seconds).</summary>
    private List<(double s, double e)> KeptSegments(double start, double end)
    {
        var segs = new List<(double s, double e)>();
        double cur = start;
        foreach (var c in Cuts.OrderBy(c => c.StartSec))
        {
            double cs = Math.Clamp(c.StartSec, start, end);
            double ce = Math.Clamp(c.EndSec, start, end);
            if (ce <= cur) continue;
            if (cs > cur) segs.Add((cur, cs));
            cur = Math.Max(cur, ce);
        }
        if (cur < end - 0.01) segs.Add((cur, end));
        return segs;
    }

    // ───────────── Export ─────────────

    private bool CanExport() => !IsExporting;

    [RelayCommand(CanExecute = nameof(CanExport))]
    private async Task ExportAsync()
    {
        if (CurrentClip is not { } clip) return;

        IsExporting = true;
        ExportProgress = 0;
        ExportStatus = Localizer.Format("Editor_Exporting", 0);
        _exportCts = new CancellationTokenSource();

        string outPath = BuildOutputPath(clip, ExportAsGif ? "gif" : SelectedExportFormat);
        try
        {
            double speed = EffectiveSpeed;
            double start = Math.Clamp(TrimStartSec, 0, DurationSec);
            double end = Math.Min(TrimEndSec, DurationSec);

            // Kept ranges after the middle cuts (GIF ignores cuts — concatenating GIFs isn't supported).
            var segments = (ExportAsGif || Cuts.Count == 0)
                ? new List<(double s, double e)> { (start, Math.Max(start + 0.2, end)) }
                : KeptSegments(start, end);
            if (segments.Count == 0) segments.Add((start, Math.Max(start + 0.2, end)));

            var progress = new Progress<double>(p =>
            {
                ExportProgress = (int)Math.Clamp(p, 0, 100);
                ExportStatus = Localizer.Format("Editor_Exporting", ExportProgress);
            });

            int exit = await ExportSegmentsAsync(clip, outPath, segments, speed, progress, _exportCts.Token);

            if (exit == 0 && File.Exists(outPath))
            {
                ExportStatus = Localizer.Format("Editor_ExportDone", Path.GetFileName(outPath));
                await _library.RefreshCommand.ExecuteAsync(null);
            }
            else
            {
                ExportStatus = Localizer.Format("Editor_ExportError", $"ffmpeg exit {exit}");
                try { if (File.Exists(outPath)) File.Delete(outPath); } catch { }
            }
        }
        catch (OperationCanceledException)
        {
            ExportStatus = string.Empty;
            try { if (File.Exists(outPath)) File.Delete(outPath); } catch { /* partial file cleanup */ }
        }
        catch (Exception ex)
        {
            ExportStatus = Localizer.Format("Editor_ExportError", ex.Message);
        }
        finally
        {
            IsExporting = false;
            ExportProgress = 0;
        }
    }

    /// <summary>Exports the kept segments to <paramref name="outPath"/>. One segment goes straight to
    /// the file; several are each rendered to a temp file (the full editor pipeline) then concatenated
    /// with a stream copy (identical params → no re-encode for the join).</summary>
    private async Task<int> ExportSegmentsAsync(ReplayClip clip, string outPath,
        List<(double s, double e)> segments, double speed, IProgress<double> progress, CancellationToken ct)
    {
        if (segments.Count == 1)
        {
            var (s, e) = segments[0];
            return await RunOneAsync(clip, outPath, s, Math.Max(0.05, e - s), speed, progress, ct);
        }

        string ext = Path.GetExtension(outPath);
        var temps = new List<string>();
        try
        {
            for (int i = 0; i < segments.Count; i++)
            {
                var (s, e) = segments[i];
                string tmp = Path.Combine(Path.GetTempPath(), $"lag-seg-{Guid.NewGuid():N}{ext}");
                temps.Add(tmp);

                int captured = i;
                var segProgress = new Progress<double>(p =>
                    progress.Report((captured * 100.0 + Math.Clamp(p, 0, 100)) / segments.Count));

                int exit = await RunOneAsync(clip, tmp, s, Math.Max(0.05, e - s), speed, segProgress, ct);
                if (exit != 0) return exit;
            }

            // Concatenate the rendered segments (same codec params → stream copy).
            string listFile = Path.Combine(Path.GetTempPath(), $"lag-concat-{Guid.NewGuid():N}.txt");
            var sb = new System.Text.StringBuilder();
            foreach (var t in temps) sb.AppendLine($"file '{t.Replace("'", "'\\''")}'");
            await File.WriteAllTextAsync(listFile, sb.ToString(), ct);

            string fast = ext is ".mp4" or ".mov" ? "-movflags +faststart " : "";
            int cexit = await RunFfmpegAsync(
                $"-y -f concat -safe 0 -i \"{listFile}\" -c copy {fast}\"{outPath}\"", ct, null, 0);
            try { File.Delete(listFile); } catch { }
            return cexit;
        }
        finally
        {
            foreach (var t in temps) try { File.Delete(t); } catch { }
        }
    }

    /// <summary>Renders ONE segment with the correct encoder path (GIF palette, or nvenc → x264).</summary>
    private async Task<int> RunOneAsync(ReplayClip clip, string outPath, double start, double duration,
        double speed, IProgress<double> progress, CancellationToken ct)
    {
        double effectiveDuration = duration / speed;
        if (ExportAsGif)
            return await RunExportAsync(clip, outPath, start, duration, speed, false, effectiveDuration, progress, ct);

        int exit = await RunExportAsync(clip, outPath, start, duration, speed, true, effectiveDuration, progress, ct);
        if (exit != 0)
            exit = await RunExportAsync(clip, outPath, start, duration, speed, false, effectiveDuration, progress, ct);
        return exit;
    }

    /// <summary>"name-edit.mp4" next to the original, with a numeric suffix when taken.</summary>
    private static string BuildOutputPath(ReplayClip clip, string ext)
    {
        string dir = Path.GetDirectoryName(clip.FilePath) ?? ".";
        string baseName = Path.GetFileNameWithoutExtension(clip.FilePath);

        string candidate = Path.Combine(dir, $"{baseName}-edit.{ext}");
        for (int i = 2; File.Exists(candidate); i++)
            candidate = Path.Combine(dir, $"{baseName}-edit{i}.{ext}");
        return candidate;
    }

    /// <summary>atempo only accepts 0.5–2 per instance — chain them for extreme speeds.</summary>
    private static string BuildAtempoChain(double speed)
    {
        var inv = CultureInfo.InvariantCulture;
        var parts = new List<string>();
        double s = speed;
        while (s < 0.5) { parts.Add("atempo=0.5"); s /= 0.5; }
        while (s > 2.0) { parts.Add("atempo=2.0"); s /= 2.0; }
        parts.Add($"atempo={s.ToString("0.####", inv)}");
        return string.Join(',', parts);
    }

    private static int Even(double v)
    {
        int i = (int)Math.Round(v);
        if (i < 2) i = 2;
        return i - (i % 2);
    }

    private static string ColorToFfmpeg(string hex)
    {
        string h = (hex ?? "").TrimStart('#');
        return h.Length is 6 or 8 ? "0x" + h : "white";
    }

    /// <summary>Appends a drawtext filter per caption (drawn on the FINAL framed output). The caption
    /// text goes through a temp textfile to avoid filtergraph escaping; temp files are tracked for
    /// cleanup by the caller.</summary>
    private void AppendText(List<string> vparts, List<string> tempFiles)
    {
        var inv = CultureInfo.InvariantCulture;
        foreach (var t in Texts)
        {
            if (string.IsNullOrWhiteSpace(t.Text)) continue;
            string tf = Path.Combine(Path.GetTempPath(), $"lag-txt-{Guid.NewGuid():N}.txt");
            File.WriteAllText(tf, t.Text);
            tempFiles.Add(tf);

            string path = tf.Replace("\\", "/").Replace(":", "\\:");
            string nx = Math.Clamp(t.Nx, 0, 1).ToString("0.###", inv);
            string ny = Math.Clamp(t.Ny, 0, 1).ToString("0.###", inv);
            string frac = (Math.Clamp(t.FontPercent, 1, 50) / 100.0).ToString("0.####", inv);
            vparts.Add($"drawtext=fontfile='C\\:/Windows/Fonts/arialbd.ttf':textfile='{path}':" +
                       $"x=(w-text_w)*{nx}:y=(h-text_h)*{ny}:fontsize=h*{frac}:" +
                       $"fontcolor={ColorToFfmpeg(t.Color)}:borderw=2:bordercolor=black@0.7");
        }
    }

    /// <summary>Appends flip + 90° rotation to the END of the video chain (the final framed output).
    /// Matches the preview's flip-then-rotate order.</summary>
    private void AppendRotateFlip(List<string> vparts)
    {
        if (FlipH) vparts.Add("hflip");
        if (FlipV) vparts.Add("vflip");
        if (Rotation == 90) vparts.Add("transpose=1");       // 90° clockwise
        else if (Rotation == 180) { vparts.Add("transpose=1"); vparts.Add("transpose=1"); }
        else if (Rotation == 270) vparts.Add("transpose=2"); // 90° counter-clockwise
    }

    /// <summary>Appends the reframe pipeline: scale the video, crop the part that spills past the output
    /// frame, then pad onto a black frame at the chosen position — producing the format-aspect output
    /// with the video moved / zoomed / stretched inside it. No-op for Native + default cover.</summary>
    private void AppendReframe(List<string> vparts)
    {
        bool needsAspect = SelectedAspect?.Ratio is double;
        if (!needsAspect && !IsReframed && !IsCropped) return; // Native, untouched → passthrough
        if (SourceWidth <= 0 || SourceHeight <= 0) return;     // can't compute output pixels yet

        // Base = the (already-cropped) source frame.
        int baseW = Even(SourceWidth * CropW);
        int baseH = Even(SourceHeight * CropH);

        // Output frame size = the chosen aspect at that resolution (matches the old format crop).
        int ow, oh;
        if (SelectedAspect?.Ratio is double r && r > 0)
        {
            ow = Even(Math.Min(baseW, baseH * r));
            oh = Even(Math.Min(baseH, baseW / r));
        }
        else { ow = baseW; oh = baseH; }

        // Video placement in output pixels (may be larger than / offset beyond the frame).
        int vw = Even(VidW * ow);
        int vh = Even(VidH * oh);
        int ox = (int)Math.Round(VidX * ow);
        int oy = (int)Math.Round(VidY * oh);

        // Visible portion of the scaled video, then where it lands on the black canvas.
        int cropX = Math.Max(0, -ox), cropY = Math.Max(0, -oy);
        int px = Math.Max(0, ox), py = Math.Max(0, oy);
        int cw = Even(Math.Min(vw - cropX, ow - px));
        int ch = Even(Math.Min(vh - cropY, oh - py));
        if (cw < 2 || ch < 2) return;                     // video entirely outside the frame — skip
        cropX -= cropX % 2; cropY -= cropY % 2;
        px -= px % 2; py -= py % 2;

        vparts.Add($"scale={vw}:{vh}");
        vparts.Add($"crop={cw}:{ch}:{cropX}:{cropY}");
        vparts.Add($"pad={ow}:{oh}:{px}:{py}:black");
    }

    private async Task<int> RunExportAsync(ReplayClip clip, string outPath, double start, double duration,
                                     double speed, bool useNvenc, double effectiveDuration,
                                     IProgress<double> progress, CancellationToken ct)
    {
        var inv = CultureInfo.InvariantCulture;
        var args = new System.Text.StringBuilder();
        var tempTextFiles = new List<string>();
        bool speedChanged = Math.Abs(speed - 1.0) > 0.001;
        try
        {

        // -ss/-t as INPUT options: fast keyframe seek + read exactly the trimmed segment.
        // (-t after -i would limit the OUTPUT length instead, which breaks speed ≠ 1×.)
        args.Append($"-y -ss {start.ToString(inv)} -t {duration.ToString(inv)} -i \"{clip.FilePath}\" ");

        var filters = new List<string>();
        var maps = new List<string>();

        // ── Video chain: speed → aspect crop → fun filter ──
        var vparts = new List<string>();
        if (speedChanged)
            vparts.Add($"setpts=PTS/{speed.ToString(inv)}");
        if (SelectedFilter.Graph is { } graph)
            vparts.Add(graph);
        if (HasColorAdjust)
        {
            // Manual colour: ffmpeg eq (contrast/saturation are ×, brightness is additive −1..1).
            double c = ColorContrast / 100.0, s = ColorSaturation / 100.0, b = ColorBrightness / 100.0 - 1.0;
            vparts.Add($"eq=contrast={c.ToString("0.###", inv)}:brightness={b.ToString("0.###", inv)}:" +
                       $"saturation={s.ToString("0.###", inv)}");
        }
        if (IsCropped)
        {
            // Crop the source to the selected sub-region first (fractions of the input frame).
            string cx = CropX.ToString("0.#####", inv), cy = CropY.ToString("0.#####", inv);
            string cw = CropW.ToString("0.#####", inv), ch = CropH.ToString("0.#####", inv);
            vparts.Add($"crop=iw*{cw}:ih*{ch}:iw*{cx}:ih*{cy}");
        }
        AppendReframe(vparts);
        AppendRotateFlip(vparts);
        if (SelectedExportResolution.Height > 0)
            vparts.Add($"scale=-2:{SelectedExportResolution.Height}");   // even width, chosen height
        AppendText(vparts, tempTextFiles);   // captions drawn last, on the final framed output

        // GIF: a self-contained palette pipeline (no audio, no encoder choice).
        if (ExportAsGif)
        {
            string vchain = vparts.Count > 0 ? string.Join(",", vparts) + "," : "";
            args.Append($"-filter_complex \"[0:v:0]{vchain}fps=15,split[ga][gb];" +
                        $"[ga]palettegen=max_colors=256[gp];[gb][gp]paletteuse=dither=sierra2_4a[v]\" ");
            args.Append("-map \"[v]\" -an -loop 0 -progress pipe:1 ");
            args.Append($"\"{outPath}\"");
            return await RunFfmpegAsync(args.ToString(), ct, progress, effectiveDuration);
        }

        if (vparts.Count > 0)
        {
            filters.Add($"[0:v:0]{string.Join(',', vparts)}[v]");
            maps.Add("-map \"[v]\"");
        }
        else
        {
            maps.Add("-map 0:v:0");
        }

        // ── Audio: per-track volume + audible range gate + tempo ──
        foreach (var track in AudioTracks.Where(t => !t.IsMuted && t.Volume > 0))
        {
            // Track range relative to the trimmed segment (t starts at 0 after input -ss).
            double a = Math.Max(0, track.StartSec - start);
            double b = Math.Min(duration, track.EndSec - start);
            if (b <= 0.01 || a >= duration - 0.01) continue; // range entirely outside the segment — drop the track

            var aparts = new List<string> { $"volume={(track.Volume / 100.0).ToString("0.###", inv)}" };
            bool gated = a > 0.01 || b < duration - 0.01;
            if (gated)
                aparts.Add($"volume=0:enable='lt(t\\,{a.ToString("0.###", inv)})+gt(t\\,{b.ToString("0.###", inv)})'");
            if (speedChanged)
                aparts.Add(BuildAtempoChain(speed));

            filters.Add($"[0:a:{track.StreamIndex}]{string.Join(',', aparts)}[a{track.StreamIndex}]");
            maps.Add($"-map \"[a{track.StreamIndex}]\"");
        }

        bool hasAudio = maps.Any(m => m.Contains("[a"));

        if (filters.Count > 0)
            args.Append($"-filter_complex \"{string.Join(';', filters)}\" ");
        args.Append(string.Join(' ', maps)).Append(' ');

        // Quality-targeted VBR. For nvenc, -b:v 0 is essential on older ffmpeg builds:
        // without it the default average-bitrate cap silently degrades the export.
        args.Append(useNvenc
            ? "-c:v h264_nvenc -preset p5 -rc vbr -cq 19 -b:v 0 "
            : "-c:v libx264 -preset veryfast -crf 19 ");

        args.Append(hasAudio ? "-c:a aac -b:a 192k " : "-an ");

        args.Append("-movflags +faststart -progress pipe:1 ");
        args.Append($"\"{outPath}\"");

            return await RunFfmpegAsync(args.ToString(), ct, progress, effectiveDuration);
        }
        finally
        {
            foreach (var f in tempTextFiles) try { File.Delete(f); } catch { }
        }
    }

    /// <summary>
    /// Runs the bundled ffmpeg, optionally reporting percentage progress parsed from
    /// "-progress pipe:1" (out_time_ms against the expected output duration).
    /// </summary>
    private static async Task<int> RunFfmpegAsync(string arguments, CancellationToken ct,
                                                  IProgress<double>? progress, double expectedDurationSec)
    {
        string ffmpegPath = Path.Combine(AppContext.BaseDirectory, "obs-core", "ffmpeg.exe");
        if (!File.Exists(ffmpegPath)) ffmpegPath = "ffmpeg"; // dev fallback: PATH

        var psi = new ProcessStartInfo
        {
            FileName = ffmpegPath,
            Arguments = arguments,
            UseShellExecute = false,
            CreateNoWindow = true,
            RedirectStandardOutput = true,
            RedirectStandardError = true,
        };

        using var process = new Process { StartInfo = psi };
        process.Start();

        // stderr must be drained or ffmpeg blocks on a full pipe.
        var drainStdErr = process.StandardError.ReadToEndAsync();

        var readProgress = Task.Run(async () =>
        {
            string? line;
            while ((line = await process.StandardOutput.ReadLineAsync()) != null)
            {
                if (progress == null || expectedDurationSec <= 0) continue;
                if (line.StartsWith("out_time_ms=") &&
                    long.TryParse(line.AsSpan("out_time_ms=".Length), out long us))
                {
                    // Despite the name, out_time_ms is in microseconds.
                    progress.Report(us / 1_000_000.0 / expectedDurationSec * 100.0);
                }
            }
        }, ct);

        try
        {
            await process.WaitForExitAsync(ct);
        }
        catch (OperationCanceledException)
        {
            try { process.Kill(entireProcessTree: true); } catch { }
            throw;
        }

        await Task.WhenAll(readProgress, drainStdErr);
        return process.ExitCode;
    }

    [RelayCommand]
    private void CancelExport() => _exportCts?.Cancel();

    private static string FormatTime(TimeSpan ts) =>
        ts.TotalHours >= 1 ? ts.ToString(@"h\:mm\:ss") : ts.ToString(@"m\:ss");

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;

        _thumbsCts?.Cancel();
        _exportCts?.Cancel();
        _vlcApplyTimer?.Stop();
        _positionTimer?.Stop();
        _positionTimer?.Dispose();
        _mediaPlayer?.Stop();
        _mediaPlayer?.Dispose();
        _libVLC?.Dispose();
        VideoRenderer.Dispose();
    }
}
