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

    /// <summary>The LibVLCSharp MediaPlayer instance, bound to the VideoView.</summary>
    [ObservableProperty]
    private MediaPlayer? _player;

    /// <summary>The clip currently being played.</summary>
    [ObservableProperty]
    private ReplayClip? _currentClip;

    /// <summary>Whether video is currently playing.</summary>
    [ObservableProperty]
    private bool _isPlaying;

    /// <summary>Current playback position (0.0 to 1.0).</summary>
    [ObservableProperty]
    private double _position;

    /// <summary>When set by the user (via slider), seeks to the new position.</summary>
    partial void OnPositionChanged(double value)
    {
        if (_mediaPlayer != null && !_isUpdatingPosition)
        {
            _mediaPlayer.Position = (float)value;
        }
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
        if (_mediaPlayer != null)
            _mediaPlayer.Volume = value;
    }

    /// <summary>
    /// Global input hook, exposed for the view: clicks on the native VLC video window never
    /// reach Avalonia (HWND-level input routing), so PlayerView detects them via this hook
    /// by hit-testing the global click position against the video's screen bounds.
    /// </summary>
    public Lag.Services.GlobalHotkeyManager HotkeyManager { get; }

    public PlayerViewModel(LibraryViewModel library, Lag.Services.GlobalHotkeyManager hotkeyManager)
    {
        Title = "Player";
        _library = library;
        HotkeyManager = hotkeyManager;

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
            // Added --avcodec-hw=none to prevent hardware acceleration crashes when Avalonia detaches the NativeControlHost
            _libVLC = new LibVLC(new string[] { "--no-video-title-show", "--no-osd", "--vout=direct3d11", "--avcodec-hw=none" });
            _mediaPlayer = new MediaPlayer(_libVLC);
            Player = _mediaPlayer;

            // ── Click-to-pause fix ──
            // The native VLC video window normally grabs mouse/keyboard input, so a transparent
            // Avalonia overlay never receives clicks. Telling libVLC NOT to handle input makes the
            // native surface pass events through to the host → our overlay's PointerPressed fires,
            // and the Space key binding keeps working over the video.
            _mediaPlayer.EnableMouseInput = false;
            _mediaPlayer.EnableKeyInput = false;

            // Position update timer (fires every 250ms to update the timeline)
            _positionTimer = new System.Timers.Timer(250);
            _positionTimer.Elapsed += (_, _) => UpdatePlaybackPosition();

            // LibVLC raises these on its own threads — marshal all bound-property writes to the UI.
            _mediaPlayer.Playing += (_, _) => Dispatcher.UIThread.Post(() => { IsPlaying = true; _positionTimer?.Start(); });
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

        var media = new Media(_libVLC, new Uri(clip.FilePath));
        _mediaPlayer.Media = media;
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

    /// <summary>Skips to and plays the next clip in the library list (wraps to the first).</summary>
    [RelayCommand]
    private void PlayNext()
    {
        if (Clips.Count == 0) return;

        int currentIndex = CurrentClip != null ? Clips.IndexOf(CurrentClip) : -1;
        int nextIndex = (currentIndex + 1) % Clips.Count;
        PlayClip(Clips[nextIndex]);
    }

    /// <summary>
    /// Explicitly starts playback once the view is loaded and attached.
    /// </summary>
    public void StartPlayback()
    {
        _mediaPlayer?.Play();
    }

    /// <summary>Toggles between play and pause states.</summary>
    [RelayCommand]
    private void PlayPause()
    {
        if (_mediaPlayer == null) return;

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

        _positionTimer?.Stop();
        _positionTimer?.Dispose();
        _mediaPlayer?.Stop();
        _mediaPlayer?.Dispose();
        _libVLC?.Dispose();
    }
}
