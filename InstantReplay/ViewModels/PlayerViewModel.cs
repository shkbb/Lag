using System.Collections.Concurrent;
using System.Collections.ObjectModel;
using System.Collections.Specialized;
using System.Linq;
using System.Threading;
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

    // EVERY libvlc control call (Play/Stop/Media swap/SetRate/seek) runs on this single
    // background thread, never the UI thread. At deep slow-mo (0.05x) those calls can block
    // for seconds inside libvlc; on the UI thread that froze the whole app (and switching a
    // clip mid-stall deadlocked it). Serialised here → the UI never blocks and ops stay ordered.
    private readonly BlockingCollection<Action> _vlcQueue = new();
    private Thread? _vlcWorker;

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

    /// <summary>Current/total time in seconds — the view interpolates between these (resynced
    /// every position tick) to render a smooth millisecond clock at display framerate.</summary>
    [ObservableProperty]
    private double _positionSeconds;

    [ObservableProperty]
    private double _durationSeconds;

    /// <summary>When set by the user (via slider), seeks to the new position (throttled).</summary>
    partial void OnPositionChanged(double value)
    {
        if (_isUpdatingPosition) return;
        lock (_pendingLock) _pendingSeekPosition = (float)Math.Clamp(value, 0.0, 1.0);
        RequestFlush();
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
        lock (_pendingLock) _pendingVolume = Math.Clamp(value, 0, 100);
        RequestFlush();
    }

    /// <summary>Whether audio output is muted (independent of the volume slider value). Toggled by
    /// clicking the volume icon or pressing M in the player.</summary>
    [ObservableProperty]
    private bool _isMuted;

    partial void OnIsMutedChanged(bool value)
    {
        lock (_pendingLock) _pendingMute = value;
        RequestFlush();
    }

    /// <summary>Mute/unmute toggle (volume icon click / M key).</summary>
    [RelayCommand]
    private void ToggleMute() => IsMuted = !IsMuted;

    // ── Playback speed (incl. deep slow-mo: down to 0.05x = 20× slower) ──
    /// <summary>Speed presets shown in the player's speed menu (fast → slow).</summary>
    public IReadOnlyList<double> SpeedOptions { get; } = new[] { 2.0, 1.5, 1.0, 0.5, 0.25, 0.1 };

    /// <summary>Current playback rate (1.0 = normal). Applied to libvlc via SetRate.</summary>
    [ObservableProperty]
    private double _playbackSpeed = 1.0;

    /// <summary>Compact label for the speed button ("1x", "0.25x", "0.05x").</summary>
    public string SpeedLabel => PlaybackSpeed == 1.0 ? "1x" : $"{PlaybackSpeed:0.##}x";

    partial void OnPlaybackSpeedChanged(double value)
    {
        OnPropertyChanged(nameof(SpeedLabel));
        EnqueueVlc(() => ApplyRate(value));
    }

    // ── Coalesced writes into libvlc (volume / seek) ──
    // A timeline drag fires dozens of changes per second; at deep slow-mo each seek is slow, so
    // queueing them all backs the worker up and the UI lags. Instead we keep only the LATEST value
    // and run ONE flush op that re-reads it until drained — intermediate drag positions are dropped,
    // so at most one seek is ever in flight (no backlog, always the newest target).
    private readonly object _pendingLock = new();
    private int? _pendingVolume;
    private bool? _pendingMute;
    private float? _pendingSeekPosition;
    private bool _flushQueued;

    private void RequestFlush()
    {
        if (_disposed) return;
        lock (_pendingLock) { if (_flushQueued) return; _flushQueued = true; }
        EnqueueVlc(FlushPendingWrites);
    }

    private void FlushPendingWrites()
    {
        while (true)
        {
            int? vol; bool? mute; float? pos;
            lock (_pendingLock)
            {
                vol = _pendingVolume; _pendingVolume = null;
                mute = _pendingMute; _pendingMute = null;
                pos = _pendingSeekPosition; _pendingSeekPosition = null;
                if ((vol == null && mute == null && pos == null) || _mediaPlayer == null) { _flushQueued = false; return; }
            }
            var mp = _mediaPlayer;
            try { if (vol is int v) mp.Volume = v; } catch { }
            try { if (mute is bool m) mp.Mute = m; } catch { }
            // Seeks always run at normal speed (OnPositionChanged snaps slow-mo back to 1.0 first),
            // so Position= returns promptly and never blocks the worker.
            try { if (pos is float p && mp.IsSeekable) mp.Position = p; } catch { }
        }
    }

    // ── Serialised libvlc control worker ──

    private void VlcWorkerLoop()
    {
        try
        {
            foreach (var op in _vlcQueue.GetConsumingEnumerable())
            {
                try { op(); }
                catch (Exception ex) { System.Diagnostics.Debug.WriteLine($"[Player] VLC op failed: {ex.Message}"); }
            }
        }
        catch (Exception) { /* queue completed/disposed during shutdown */ }
    }

    /// <summary>Queues a libvlc control call to run (in order) on the background worker.</summary>
    private void EnqueueVlc(Action op)
    {
        if (_disposed) return;
        try { _vlcQueue.Add(op); } catch (Exception) { /* completed */ }
    }

    /// <summary>Applies the playback rate (worker thread). Kept minimal: changing the audio track at
    /// runtime re-inits libvlc's whole pipeline, which made deep slow-mo stall for 10-20s and turned
    /// the picture into a slideshow — so we ONLY set the rate. SetRate returns fast, so the worker
    /// stays free for the next command (pause / switch).</summary>
    private void ApplyRate(double rate)
    {
        try { _mediaPlayer?.SetRate((float)rate); } catch { }
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

            // Dedicated worker that serialises all libvlc control calls off the UI thread.
            _vlcWorker = new Thread(VlcWorkerLoop) { IsBackground = true, Name = "Lag.VlcControl" };
            _vlcWorker.Start();

            // Position update timer (100ms → the timeline + millisecond time readout stay responsive)
            _positionTimer = new System.Timers.Timer(100);
            _positionTimer.Elapsed += (_, _) => UpdatePlaybackPosition();

            // LibVLC raises these on its own threads — marshal all bound-property writes to the UI.
            _mediaPlayer.Playing += (_, _) => Dispatcher.UIThread.Post(() =>
            {
                IsPlaying = true;
                _positionTimer?.Start();
                // libvlc applies volume per audio output instance — re-assert ours on each start.
                lock (_pendingLock) _pendingVolume = Volume;
                RequestFlush();
                // Rate resets to 1 on a new media — re-assert the chosen speed (off the UI thread,
                // and drop audio if we're in deep slow-mo) so it persists per clip.
                EnqueueVlc(() => ApplyRate(PlaybackSpeed));
            });
            _mediaPlayer.Paused += (_, _) => Dispatcher.UIThread.Post(() => { IsPlaying = false; _positionTimer?.Stop(); });
            _mediaPlayer.Stopped += (_, _) => Dispatcher.UIThread.Post(() =>
            {
                IsPlaying = false;
                _positionTimer?.Stop();
                _isUpdatingPosition = true;
                Position = 0;
                _isUpdatingPosition = false;
                TimeDisplay = "0:00.000 / 0:00.000";
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

        // A new clip always starts at normal speed: carrying deep slow-mo onto the next clip made it
        // re-buffer slowly on start (laggy switch), and resetting is nicer UX anyway.
        PlaybackSpeed = 1.0;

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

        string path = clip.FilePath;
        bool isGif = path.EndsWith(".gif", StringComparison.OrdinalIgnoreCase);

        // Build + swap the media on the worker thread. Assigning Media stops the current input,
        // which can block for SECONDS at deep slow-mo — doing it off the UI thread is what stops the
        // app from freezing when you switch clips mid-stall. Play() is queued separately afterwards
        // (StartPlayback), and the queue preserves order so the swap always lands first.
        EnqueueVlc(() =>
        {
            var mp = _mediaPlayer; var vlc = _libVLC;
            if (mp == null || vlc == null) return;

            // Stopping a 0.05x input is slow — normalise the rate first so the swap is quick.
            // The new clip starts at 1.0 and the Playing handler re-applies the chosen speed.
            try { mp.SetRate(1.0f); } catch { }

            var media = new Media(vlc, new Uri(path));
            if (isGif)
            {
                // libvlc's default image demuxer shows only the FIRST GIF frame. Force ffmpeg's
                // avformat demuxer so every frame plays, and loop it (GIFs are meant to repeat).
                media.AddOption(":demux=avformat");
                media.AddOption(":input-repeat=65535");
            }
            // SOFTWARE decode for the player (same as the editor): hardware d3d11va stalled audio
            // ~1s per clip and green-screened on a live swap unless fully stopped first. On the CPU,
            // 1080p H.264 decodes far above real-time when no game is hammering the machine.
            media.AddOption(":avcodec-hw=none");

            var old = mp.Media;
            mp.Media = media;
            media.Dispose();   // the player keeps its own native ref — release ours + the previous
            old?.Dispose();
        });
        // DO NOT Play() here — the View calls StartPlayback() once the surface is ready.
    }

    /// <summary>Plays a clip chosen from the sidebar list (loads it and starts playback immediately).</summary>
    [RelayCommand]
    private void PlayClip(ReplayClip? clip)
    {
        if (clip == null) return;
        LoadAndPlay(clip);
        StartPlayback();
    }

    // ── Replay right-click menu (sidebar item / the playing video), delegated to the shared Library VM
    //    (same singleton the Library view uses, so Edit/Show/Delete behave identically). ──
    public void RequestEditClip(ReplayClip? clip) { if (clip != null) _library.RequestEdit(clip); }
    public void ShowClipInFolder(ReplayClip? clip) { if (clip != null) _library.ShowInFolderCommand.Execute(clip); }
    public void DeleteReplay(ReplayClip? clip) { if (clip != null) _library.DeleteClipCommand.Execute(clip); }

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
        EnqueueVlc(() => _mediaPlayer?.Play());
    }

    /// <summary>Toggles between play and pause states; replays from the start after the end.</summary>
    [RelayCommand]
    private void PlayPause()
    {
        if (_mediaPlayer == null || IsViewingImage) return;

        EnqueueVlc(() =>
        {
            var mp = _mediaPlayer;
            if (mp == null) return;
            // After EndReached libvlc sits in the Ended state where Play() is a no-op —
            // a "watch it again" click must restart the clip from the beginning.
            if (mp.State == VLCState.Ended) { mp.Stop(); mp.Play(); }
            else if (mp.IsPlaying) mp.Pause();
            else mp.Play();
        });
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
        EnqueueVlc(() => _mediaPlayer?.Stop());
    }

    /// <summary>Seeks forward by 10 seconds.</summary>
    [RelayCommand]
    private void SeekForward()
    {
        EnqueueVlc(() => { var mp = _mediaPlayer; if (mp != null) try { mp.Time += 10_000; } catch { } });
    }

    /// <summary>Seeks backward by 10 seconds.</summary>
    [RelayCommand]
    private void SeekBackward()
    {
        EnqueueVlc(() => { var mp = _mediaPlayer; if (mp != null) try { mp.Time = Math.Max(0, mp.Time - 10_000); } catch { } });
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

            PositionSeconds = current.TotalSeconds;
            DurationSeconds = total.TotalSeconds;
            TimeDisplay = $"{FormatTime(current)} / {FormatTime(total)}";
        });
    }

    private static string FormatTime(TimeSpan ts) =>
        ts.TotalHours >= 1 ? ts.ToString(@"h\:mm\:ss\.fff") : ts.ToString(@"m\:ss\.fff");

    public void Dispose()
    {
        if (_disposed) return;
        _disposed = true;

        _library.Clips.CollectionChanged -= OnClipsCollectionChanged;

        _positionTimer?.Stop();
        _positionTimer?.Dispose();

        // Queue a final stop, then let the worker drain and exit before tearing down libvlc.
        try { _vlcQueue.Add(() => { try { _mediaPlayer?.Stop(); } catch { } }); } catch { }
        _vlcQueue.CompleteAdding();
        _vlcWorker?.Join(2000);
        _vlcQueue.Dispose();

        _mediaPlayer?.Dispose();
        _libVLC?.Dispose();
        VideoRenderer.Dispose();
    }
}
