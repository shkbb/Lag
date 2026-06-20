using System.Collections.ObjectModel;
using System.Collections.Specialized;
using System.Linq;
using Avalonia.Threading;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Lag.Core;
using Lag.Models;
using LibVLCSharp.Shared;

namespace Lag.ViewModels;

/// <summary>
/// ViewModel for the integrated video player. Wraps LibVLCSharp's MediaPlayer
/// to provide MVVM-friendly playback controls, position tracking, and volume control.
/// 
/// LibVLCSharp Threading Note:
///   LibVLC fires events on its own internal threads. Property updates from
///   these events may need marshalling to the UI thread depending on the
///   Avalonia binding mode.
/// </summary>
public partial class PlayerViewModel : ViewModelBase, IDisposable
{
    private LibVLC? _libVLC;
    private MediaPlayer? _mediaPlayer;
    private bool _disposed;
    private System.Timers.Timer? _positionTimer;

    /// <summary>
    /// Shared library VM — the single source of truth for the saved replay collection.
    /// The Player sidebar binds directly to its <see cref="LibraryViewModel.Clips"/> collection.
    /// </summary>
    private readonly LibraryViewModel _library;

    /// <summary>Guards against the sidebar selection re-triggering playback when we set it ourselves.</summary>
    private bool _suppressSidebarAutoPlay;

    /// <summary>Real saved replay clips shown in the right sidebar (shared with the Library view).</summary>
    public ObservableCollection<ReplayClip> Clips => _library.Clips;

    /// <summary>Total number of saved clips (sidebar stat).</summary>
    public int TotalClips => Clips.Count;

    /// <summary>Sum of all clip durations, formatted as "X год Y хв" (sidebar stat).</summary>
    public string TotalDurationDisplay => FormatTotalDuration();

    /// <summary>Total disk size of all clips (e.g., "38.4 GB") for the sidebar stats.</summary>
    public string TotalSizeDisplay
    {
        get
        {
            long bytes = Clips.Sum(c => c.FileSize);
            return bytes >= 1_073_741_824
                ? $"{bytes / 1_073_741_824.0:F1} GB"
                : $"{bytes / 1_048_576.0:F0} MB";
        }
    }

    /// <summary>
    /// The clip currently selected in the sidebar list. Selecting one immediately plays it.
    /// </summary>
    [ObservableProperty]
    private ReplayClip? _activeSidebarClip;

    partial void OnActiveSidebarClipChanged(ReplayClip? value)
    {
        if (value == null || _suppressSidebarAutoPlay) return;
        LoadAndPlay(value);
        StartPlayback();
    }

    /// <summary>The LibVLCSharp MediaPlayer instance.</summary>
    [ObservableProperty]
    private MediaPlayer? _player;

    /// <summary>Renders decoded frames into an Avalonia bitmap (no native HWND).</summary>
    public Lag.Services.VlcVideoRenderer VideoRenderer { get; } = new();

    /// <summary>True while a screenshot (image clip) is displayed instead of video.</summary>
    [ObservableProperty]
    private bool _isViewingImage;

    /// <summary>The decoded screenshot shown in the video area when <see cref="IsViewingImage"/>.</summary>
    [ObservableProperty]
    private Avalonia.Media.Imaging.Bitmap? _stillImage;

    /// <summary>The clip currently being played.</summary>
    [ObservableProperty]
    private ReplayClip? _currentClip;

    /// <summary>Whether video is currently playing.</summary>
    [ObservableProperty]
    private bool _isPlaying;

    /// <summary>Current playback position (0.0 to 1.0).</summary>
    [ObservableProperty]
    private double _position;

    /// <summary>When set by the user (via slider), seeks to the new position (throttled).</summary>
    partial void OnPositionChanged(double value)
    {
        if (_isUpdatingPosition) return;
        _pendingSeekPosition = (float)Math.Clamp(value, 0.0, 1.0);
        StartVlcApplyTimer();
    }

    private bool _isUpdatingPosition;

    /// <summary>Human-readable current time display (e.g., "1:23 / 5:00").</summary>
    [ObservableProperty]
    private string _timeDisplay = "0:00 / 0:00";

    /// <summary>Playback volume (0 to 100).</summary>
    [ObservableProperty]
    private int _volume = 80;

    partial void OnVolumeChanged(int value)
    {
        _pendingVolume = Math.Clamp(value, 0, 100);
        StartVlcApplyTimer();
    }

    // ── Playback speed (incl. deep slow-mo: down to 0.05x = 20× slower) ──
    /// <summary>Speed presets shown in the player's speed menu (fast → slow).</summary>
    public IReadOnlyList<double> SpeedOptions { get; } = new[] { 2.0, 1.5, 1.0, 0.5, 0.25, 0.1, 0.05 };

    /// <summary>Current playback rate (1.0 = normal). Applied to libvlc via SetRate.</summary>
    [ObservableProperty]
    private double _playbackSpeed = 1.0;

    /// <summary>Compact label for the speed button ("1x", "0.25x", "0.05x").</summary>
    public string SpeedLabel => PlaybackSpeed == 1.0 ? "1x" : $"{PlaybackSpeed:0.##}x";

    partial void OnPlaybackSpeedChanged(double value)
    {
        OnPropertyChanged(nameof(SpeedLabel));
        try { _mediaPlayer?.SetRate((float)value); }
        catch (Exception ex) { System.Diagnostics.Debug.WriteLine($"[Player] SetRate failed: {ex.Message}"); }
    }

    // ── Throttled writes into libvlc ──
    // Dragging a slider produces dozens of property changes per second; pushing each one
    // straight into libvlc (volume/seek) floods its audio/input threads — the UI starts
    // stuttering and libvlc eventually crashes the process. Instead the latest value is
    // parked here and flushed at most every 80ms (the first change applies immediately).
    private DispatcherTimer? _vlcApplyTimer;
    private int? _pendingVolume;
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
            ApplyPendingVlcWrites(); // leading edge — the first change feels instant
            _vlcApplyTimer.Start();  // trailing ticks pick up whatever arrives while dragging
        }
    }

    private void ApplyPendingVlcWrites()
    {
        if (_mediaPlayer == null || _disposed)
        {
            _pendingVolume = null;
            _pendingSeekPosition = null;
            _vlcApplyTimer?.Stop();
            return;
        }

        bool appliedAnything = false;
        try
        {
            if (_pendingVolume is int vol)
            {
                _pendingVolume = null;
                _mediaPlayer.Volume = vol;
                appliedAnything = true;
            }
            if (_pendingSeekPosition is float pos)
            {
                _pendingSeekPosition = null;
                if (_mediaPlayer.IsSeekable)
                    _mediaPlayer.Position = pos;
                appliedAnything = true;
            }
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"VLC write failed: {ex.Message}");
        }

        if (!appliedAnything)
            _vlcApplyTimer?.Stop();
    }

    public PlayerViewModel(LibraryViewModel library)
    {
        Title = "Player";
        _library = library;

        // Keep the sidebar stats live as the shared clip collection changes (refresh, save, delete).
        _library.Clips.CollectionChanged += OnClipsCollectionChanged;

        InitializeVLC();
    }

    private void OnClipsCollectionChanged(object? sender, NotifyCollectionChangedEventArgs e)
    {
        // CollectionChanged may arrive on a background thread; marshal stat updates to the UI.
        Dispatcher.UIThread.Post(() =>
        {
            OnPropertyChanged(nameof(TotalClips));
            OnPropertyChanged(nameof(TotalDurationDisplay));
            OnPropertyChanged(nameof(TotalSizeDisplay));
        });
    }

    /// <summary>Formats the summed clip durations using localized unit labels (e.g. "2 h 15 min").</summary>
    private string FormatTotalDuration()
    {
        var total = TimeSpan.FromTicks(Clips.Sum(c => c.Duration.Ticks));

        if (total.TotalHours >= 1)
            return Lag.Core.Localizer.Format("Time_HourMinShort", (int)total.TotalHours, total.Minutes);
        if (total.TotalMinutes >= 1)
            return Lag.Core.Localizer.Format("Time_MinShort", (int)total.TotalMinutes);
        return Lag.Core.Localizer.Format("Time_SecShort", total.Seconds);
    }

    /// <summary>
    /// Initializes the LibVLC engine and creates the MediaPlayer instance.
    /// </summary>
    private void InitializeVLC()
    {
        try
        {
            LibVLCSharp.Shared.Core.Initialize();
            // No --avcodec-hw here: VLC ignores it for vmem players (the callbacks attach
            // forces it back to "none"). Hardware decode is enabled per-media in LoadAndPlay.
            _libVLC = new LibVLC(new string[] { "--no-video-title-show", "--no-osd" });
            _mediaPlayer = new MediaPlayer(_libVLC);

            // Frames are rendered into an Avalonia bitmap (vmem callbacks) instead of a native
            // HWND — the video becomes a normal visual: clipping, overlays and clicks just work.
            VideoRenderer.Attach(_mediaPlayer);

            Player = _mediaPlayer;

            // Position update timer (fires every 250ms to update the timeline)
            _positionTimer = new System.Timers.Timer(250);
            _positionTimer.Elapsed += (_, _) => UpdatePlaybackPosition();

            // LibVLC raises these on its own threads — marshal all bound-property writes to the UI.
            _mediaPlayer.Playing += (_, _) => Dispatcher.UIThread.Post(() =>
            {
                IsPlaying = true;
                _positionTimer?.Start();
                // libvlc applies volume per audio output instance — re-assert ours on each start.
                _pendingVolume = Volume;
                StartVlcApplyTimer();
                // Rate resets to 1 on a new media — re-assert the chosen speed so it persists per clip.
                try { _mediaPlayer.SetRate((float)PlaybackSpeed); } catch { }
            });
            _mediaPlayer.Paused += (_, _) => Dispatcher.UIThread.Post(() => { IsPlaying = false; _positionTimer?.Stop(); });
            _mediaPlayer.Stopped += (_, _) => Dispatcher.UIThread.Post(() =>
            {
                IsPlaying = false;
                _positionTimer?.Stop();
                _isUpdatingPosition = true;
                Position = 0;
                _isUpdatingPosition = false;
                TimeDisplay = "0:00 / 0:00";
            });
            // Natural end of the clip: show the play button and park the timeline at 100%
            // (PlayPause restarts from zero in this state).
            _mediaPlayer.EndReached += (_, _) => Dispatcher.UIThread.Post(() =>
            {
                IsPlaying = false;
                _positionTimer?.Stop();
                _isUpdatingPosition = true;
                Position = 1;
                _isUpdatingPosition = false;
            });
        }
        catch (Exception ex)
        {
            System.Diagnostics.Debug.WriteLine($"VLC init failed: {ex.Message}");
        }
    }

    /// <summary>
    /// Loads and starts playing the specified replay clip.
    /// </summary>
    public void LoadAndPlay(ReplayClip clip)
    {
        if (_mediaPlayer == null || _libVLC == null) return;

        CurrentClip = clip;

        // Reflect the selection in the sidebar list WITHOUT re-triggering auto-play.
        _suppressSidebarAutoPlay = true;
        ActiveSidebarClip = clip;
        _suppressSidebarAutoPlay = false;

        // Screenshots: stop any playback and show the image in the video area.
        if (clip.IsImage)
        {
            StopPlayback();
            try
            {
                // Full-resolution decode — screenshots are lossless PNGs and must stay sharp.
                using var fs = File.OpenRead(clip.FilePath);
                var old = StillImage;
                StillImage = new Avalonia.Media.Imaging.Bitmap(fs);
                old?.Dispose();
            }
            catch (Exception ex)
            {
                System.Diagnostics.Debug.WriteLine($"[Player] Failed to load screenshot: {ex.Message}");
            }
            IsViewingImage = true;
            IsPlaying = false;
            TimeDisplay = "";
            return;
        }

        // Back to video mode: drop the still image.
        if (IsViewingImage)
        {
            IsViewingImage = false;
            var oldStill = StillImage;
            StillImage = null;
            oldStill?.Dispose();
        }

        var oldMedia = _mediaPlayer.Media;
        var media = new Media(_libVLC, new Uri(clip.FilePath));
        // SOFTWARE decode for the player (same as the editor). Two reasons:
        //   • Hardware (d3d11va) decoder init stalls audio ~1s at the start of every clip (audio waits
        //     for the HW pipeline to stabilise), and it produced a green screen on a live media swap
        //     unless we fully Stop() first (which itself re-opened the audio output, delaying sound).
        //   • The player is used to REVIEW clips, not while a game is hammering the GPU/CPU — the
        //     "~2 fps software decode" problem only happened under game load. 1080p H.264 (the default)
        //     decodes far above real-time on the CPU when no game is running.
        // Net: no green screen (so no Stop() needed → audio output stays warm), and sound starts
        // immediately. Heavy codecs (AV1/HEVC) on a weak CPU are the only caveat.
        media.AddOption(":avcodec-hw=none");
        _mediaPlayer.Media = media;
        // The player keeps its own native reference — release ours (and the previous one)
        // so switching clips doesn't leak native Media handles.
        media.Dispose();
        oldMedia?.Dispose();
        // DO NOT call Play() here to prevent the VLC popup.
        // The View will call StartPlayback() when the VideoView is loaded.
    }

    /// <summary>Plays a clip chosen from the sidebar list (loads it and starts playback immediately).</summary>
    [RelayCommand]
    private void PlayClip(ReplayClip? clip)
    {
        if (clip == null) return;
        LoadAndPlay(clip);
        StartPlayback();
    }

    /// <summary>Skips to and plays the next VIDEO clip in the library list (screenshots are skipped).</summary>
    [RelayCommand]
    private void PlayNext()
    {
        if (Clips.Count == 0) return;

        int currentIndex = CurrentClip != null ? Clips.IndexOf(CurrentClip) : -1;
        for (int step = 1; step <= Clips.Count; step++)
        {
            var candidate = Clips[(currentIndex + step) % Clips.Count];
            if (!candidate.IsImage)
            {
                PlayClip(candidate);
                return;
            }
        }
    }

    /// <summary>
    /// Explicitly starts playback once the view is loaded and attached.
    /// </summary>
    public void StartPlayback()
    {
        if (IsViewingImage) return; // a screenshot is on display — nothing to play
        _mediaPlayer?.Play();
    }

    /// <summary>Toggles between play and pause states; replays from the start after the end.</summary>
    [RelayCommand]
    private void PlayPause()
    {
        if (_mediaPlayer == null || IsViewingImage) return;

        // After EndReached libvlc sits in the Ended state where Play() is a no-op —
        // a "watch it again" click must restart the clip from the beginning.
        if (_mediaPlayer.State == VLCState.Ended)
        {
            _mediaPlayer.Stop();
            _mediaPlayer.Play();
            return;
        }

        if (_mediaPlayer.IsPlaying)
            _mediaPlayer.Pause();
        else
            _mediaPlayer.Play();
    }

    /// <summary>Stops playback and resets position.</summary>
    [RelayCommand]
    private void Stop()
    {
        StopPlayback();
    }

    /// <summary>
    /// Explicitly stops playback. Used by lifecycle hooks to stop ghost audio.
    /// </summary>
    public void StopPlayback()
    {
        _mediaPlayer?.Stop();
    }

    /// <summary>Seeks forward by 10 seconds.</summary>
    [RelayCommand]
    private void SeekForward()
    {
        if (_mediaPlayer == null) return;
        _mediaPlayer.Time += 10_000; // milliseconds
    }

    /// <summary>Seeks backward by 10 seconds.</summary>
    [RelayCommand]
    private void SeekBackward()
    {
        if (_mediaPlayer == null) return;
        _mediaPlayer.Time = Math.Max(0, _mediaPlayer.Time - 10_000);
    }

    /// <summary>
    /// Updates the position and time display from the media player's current state.
    /// Called periodically by the position timer.
    /// </summary>
    private void UpdatePlaybackPosition()
    {
        if (_mediaPlayer == null) return;

        // Snapshot native values on the timer thread, then push UI updates on the UI thread.
        float position = _mediaPlayer.Position;
        var current = TimeSpan.FromMilliseconds(_mediaPlayer.Time);
        var total = TimeSpan.FromMilliseconds(_mediaPlayer.Length);

        Dispatcher.UIThread.Post(() =>
        {
            _isUpdatingPosition = true;
            Position = position;
            _isUpdatingPosition = false;

            TimeDisplay = $"{FormatTime(current)} / {FormatTime(total)}";
        });
    }

    private static string FormatTime(TimeSpan ts) =>
        ts.TotalHours >= 1 ? ts.ToString(@"h\:mm\:ss") : ts.ToString(@"m\:ss");

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;

        _library.Clips.CollectionChanged -= OnClipsCollectionChanged;

        _vlcApplyTimer?.Stop();
        _positionTimer?.Stop();
        _positionTimer?.Dispose();
        _mediaPlayer?.Stop();
        _mediaPlayer?.Dispose();
        _libVLC?.Dispose();
        VideoRenderer.Dispose();
    }
}
