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

public record EditorSpeedOption(string Display, double Value);

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
/// ViewModel for the built-in clip editor (Medal-style): trim on a thumbnail timeline,
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

    // ── Aspect ratio (export crop) ──

    public IReadOnlyList<EditorAspectOption> AspectOptions { get; }

    [ObservableProperty]
    private EditorAspectOption _selectedAspect;

    partial void OnSelectedAspectChanged(EditorAspectOption value) => ApplyPreviewAspect();

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

    partial void OnSelectedFilterChanged(EditorFilterOption value) => ApplyPreviewFilter();

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

    private void ApplyPreviewAspect()
    {
        try
        {
            if (_mediaPlayer != null)
                _mediaPlayer.CropGeometry = SelectedAspect.CropGeometry;
        }
        catch (Exception ex) { Debug.WriteLine($"[Editor] CropGeometry failed: {ex.Message}"); }
    }

    /// <summary>Approximates the chosen filter with libvlc's adjust filter.</summary>
    private void ApplyPreviewFilter()
    {
        if (_mediaPlayer == null) return;
        try
        {
            var f = SelectedFilter;
            if (f.HasAdjust)
            {
                _mediaPlayer.SetAdjustInt(VideoAdjustOption.Enable, 1);
                _mediaPlayer.SetAdjustFloat(VideoAdjustOption.Contrast, f.Contrast);
                _mediaPlayer.SetAdjustFloat(VideoAdjustOption.Brightness, f.Brightness);
                _mediaPlayer.SetAdjustFloat(VideoAdjustOption.Saturation, f.Saturation);
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
        ApplyPreviewAspect();
        ApplyPreviewFilter();
        ApplyPreviewVolume();
    }

    // ───────────── Opening a clip ─────────────

    /// <summary>Loads a clip into the editor: probes streams, builds the timeline, prepares preview.</summary>
    public void OpenClip(ReplayClip clip)
    {
        EnsureVlc();
        CurrentClip = clip;

        // Reset edit state for the new clip.
        TrimStartSec = 0;
        DurationSec = Math.Max(1, clip.Duration.TotalSeconds);
        TrimEndSec = DurationSec;
        SpeedText = "1";
        SelectedAspect = AspectOptions[0];
        SelectedFilter = FilterOptions[0];
        IsVideoSelected = false;
        ExportStatus = string.Empty;
        ExportProgress = 0;

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

            await Dispatcher.UIThread.InvokeAsync(() =>
            {
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

                // libvlc resets crop/adjust/volume per playback — re-assert immediately.
                ApplyPreviewAspect();
                ApplyPreviewFilter();
                ApplyPreviewVolume();
                // Re-assert crop + adjust a beat later too: applied at the instant Playing fires (the
                // vmem vout isn't fully up yet) they often don't "stick", so the aspect crop and the
                // colour filter appeared to do nothing. Rate is also deferred — forcing 2× at t=0
                // makes the decoder drop the first GOP and flash blank ("gray screen on speeded start").
                DispatcherTimer.RunOnce(() =>
                {
                    ApplyPreviewAspect();
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
            // The playhead lives strictly between the trim handles: when playback
            // reaches the end handle, park exactly on it and pause.
            double posSec = position * DurationSec;
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

        try
        {
            string outPath = BuildOutputPath(clip);
            double speed = EffectiveSpeed;
            double start = Math.Clamp(TrimStartSec, 0, DurationSec);
            double duration = Math.Max(0.2, Math.Min(TrimEndSec, DurationSec) - start);
            double effectiveDuration = duration / speed;

            var progress = new Progress<double>(p =>
            {
                ExportProgress = (int)Math.Clamp(p, 0, 100);
                ExportStatus = Localizer.Format("Editor_Exporting", ExportProgress);
            });

            // Hardware encode first (nvenc), transparent fallback to x264 on any failure.
            int exit = await RunExportAsync(clip, outPath, start, duration, speed, useNvenc: true,
                                            effectiveDuration, progress, _exportCts.Token);
            if (exit != 0)
                exit = await RunExportAsync(clip, outPath, start, duration, speed, useNvenc: false,
                                            effectiveDuration, progress, _exportCts.Token);

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

    /// <summary>"name-edit.mp4" next to the original, with a numeric suffix when taken.</summary>
    private static string BuildOutputPath(ReplayClip clip)
    {
        string dir = Path.GetDirectoryName(clip.FilePath) ?? ".";
        string baseName = Path.GetFileNameWithoutExtension(clip.FilePath);

        string candidate = Path.Combine(dir, $"{baseName}-edit.mp4");
        for (int i = 2; File.Exists(candidate); i++)
            candidate = Path.Combine(dir, $"{baseName}-edit{i}.mp4");
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

    private Task<int> RunExportAsync(ReplayClip clip, string outPath, double start, double duration,
                                     double speed, bool useNvenc, double effectiveDuration,
                                     IProgress<double> progress, CancellationToken ct)
    {
        var inv = CultureInfo.InvariantCulture;
        var args = new System.Text.StringBuilder();
        bool speedChanged = Math.Abs(speed - 1.0) > 0.001;

        // -ss/-t as INPUT options: fast keyframe seek + read exactly the trimmed segment.
        // (-t after -i would limit the OUTPUT length instead, which breaks speed ≠ 1×.)
        args.Append($"-y -ss {start.ToString(inv)} -t {duration.ToString(inv)} -i \"{clip.FilePath}\" ");

        var filters = new List<string>();
        var maps = new List<string>();

        // ── Video chain: speed → aspect crop → fun filter ──
        var vparts = new List<string>();
        if (speedChanged)
            vparts.Add($"setpts=PTS/{speed.ToString(inv)}");
        if (SelectedAspect.Ratio is double ratio)
        {
            string r = ratio.ToString("0.#####", inv);
            vparts.Add($"crop='min(iw\\,ih*{r})':'min(ih\\,iw/{r})'");
            vparts.Add("scale='trunc(iw/2)*2':'trunc(ih/2)*2'"); // encoders need even dimensions
        }
        if (SelectedFilter.Graph is { } graph)
            vparts.Add(graph);

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

        return RunFfmpegAsync(args.ToString(), ct, progress, effectiveDuration);
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
