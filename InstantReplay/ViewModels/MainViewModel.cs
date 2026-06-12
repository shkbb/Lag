using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using Lag.Core;
using Lag.Services;
using Lag.Services.ObsIntegration;

namespace Lag.ViewModels;

/// <summary>
/// Root ViewModel for the application shell. Manages navigation between views,
/// recording state, and coordinates the hotkey → save replay flow.
/// 
/// Navigation:
///   Uses a <see cref="CurrentView"/> property bound to a ContentControl in MainWindow.
///   Switching views updates this property, triggering a DataTemplate match in the UI.
/// </summary>
public partial class MainViewModel : ViewModelBase
{
    private readonly ObsRecorderService _engine;
    private readonly GlobalHotkeyManager _hotkeyManager;

    /// <summary>Child ViewModels for each view.</summary>
    public SettingsViewModel Settings { get; }
    public LibraryViewModel Library { get; }
    public PlayerViewModel Player { get; }
    public EditorViewModel Editor { get; }

    /// <summary>The currently displayed view (bound to ContentControl).</summary>
    [ObservableProperty]
    private ViewModelBase _currentView;

    /// <summary>Whether the recording engine is active.</summary>
    [ObservableProperty]
    private bool _isRecording;

    /// <summary>Status text shown in the status bar.</summary>
    [ObservableProperty]
    private string _statusText = Lag.Core.Localizer.Get("Status_Ready");

    /// <summary>Buffer fill indicator text (e.g., "2:35 / 5:00").</summary>
    [ObservableProperty]
    private string _bufferStatus = "0:00";

    /// <summary>Sidebar buffer-bar fill, 0–100. Animates up once while the ring buffer fills.</summary>
    [ObservableProperty]
    private double _bufferFillPercent;

    private Avalonia.Threading.DispatcherTimer? _bufferTimer;
    private int _bufferElapsedSec;
    private int _bufferTotalSec;

    private static string FmtSec(int s) => $"{s / 60}:{s % 60:00}";

    /// <summary>Starts the once-per-second buffer fill ticker (design: "3:12 / 5:00" + bar).</summary>
    private void StartBufferTicker(int totalSeconds)
    {
        _bufferTotalSec = Math.Max(1, totalSeconds);
        _bufferElapsedSec = 0;
        BufferFillPercent = 0;
        BufferStatus = $"{FmtSec(0)} / {FmtSec(_bufferTotalSec)}";

        _bufferTimer ??= new Avalonia.Threading.DispatcherTimer { Interval = TimeSpan.FromSeconds(1) };
        _bufferTimer.Tick -= OnBufferTick;
        _bufferTimer.Tick += OnBufferTick;
        _bufferTimer.Start();
    }

    private void OnBufferTick(object? sender, EventArgs e)
    {
        if (_bufferElapsedSec < _bufferTotalSec) _bufferElapsedSec++;
        BufferFillPercent = 100.0 * _bufferElapsedSec / _bufferTotalSec;
        BufferStatus = $"{FmtSec(_bufferElapsedSec)} / {FmtSec(_bufferTotalSec)}";
        if (_bufferElapsedSec >= _bufferTotalSec) _bufferTimer?.Stop();
    }

    private void StopBufferTicker()
    {
        _bufferTimer?.Stop();
        BufferFillPercent = 0;
        BufferStatus = "0:00";
    }

    /// <summary>Whether a replay save is currently in progress.</summary>
    [ObservableProperty]
    private bool _isSaving;

    /// <summary>
    /// Whether the player is in fullscreen mode. When true, MainWindow collapses the left
    /// navigation sidebar and the player collapses its right "Replays" sidebar, giving the
    /// VideoView the entire window. Toggled from the Player and restored with ESC.
    /// </summary>
    [ObservableProperty]
    private bool _isFullscreen;

    public MainViewModel(
        ObsRecorderService engine,
        GlobalHotkeyManager hotkeyManager,
        SettingsViewModel settings,
        LibraryViewModel library,
        PlayerViewModel player,
        EditorViewModel editor)
    {
        Title = "Lag";
        _engine = engine;
        _hotkeyManager = hotkeyManager;

        Settings = settings;
        Library = library;
        Player = player;
        Editor = editor;
        _currentView = settings; // Default to Settings view

        // Wire up events
        _engine.ReplaySaved += (_, outputVideoPath) =>
        {
            IsSaving = false;
            StatusText = Localizer.Format("Status_ReplaySaved", Path.GetFileName(outputVideoPath));

            // Auto-refresh library
            _ = Library.RefreshCommand.ExecuteAsync(null);

            // On-screen toast (topmost, visible over games) + the save sound.
            Lag.Services.ToastService.Show(
                Localizer.Get("Toast_ReplaySaved"), Path.GetFileName(outputVideoPath));

            // Play the custom "replay saved" sound (replaces the old system beep).
            PlaySaveSound();
        };

        // Global hotkey → save replay. Ignored while the user is REBINDING keys in
        // Settings (and briefly after) — otherwise the combo being typed fires instantly.
        _hotkeyManager.HotkeyPressed += (_, _) =>
        {
            if (Settings.AreHotkeysSuppressed) return;

            if (IsRecording && !IsSaving)
            {
                IsSaving = true;
                Avalonia.Threading.Dispatcher.UIThread.Post(() => StatusText = Localizer.Get("Status_Saving"));
                _ = Task.Run(() => _engine.SaveReplay());
            }
        };

        // Push-to-talk: mic is live only while the PTT key is held (no-ops when not recording
        // or when PTT is disabled; engine ignores the call if the mic source doesn't exist).
        _hotkeyManager.PttPressed += (_, _) =>
        {
            if (Settings.AreHotkeysSuppressed) return;
            if (IsRecording && Settings.PushToTalkEnabled)
                _engine.SetMicMuted(false);
        };
        _hotkeyManager.PttReleased += (_, _) =>
        {
            if (Settings.AreHotkeysSuppressed) return;
            if (IsRecording && Settings.PushToTalkEnabled)
                _engine.SetMicMuted(true);
        };

        // Library clip play navigation
        Library.PlayClipRequested += (_, clip) =>
        {
            Player.LoadAndPlay(clip);
            CurrentView = Player;
        };

        // Library clip edit navigation (context menu → Edit)
        Library.EditClipRequested += (_, clip) => OpenEditor(clip);

        // Auto-start recording on launch if the user enabled it. Deferred to the UI loop (Background
        // priority) so it runs after the window and framework finish initializing, not mid-construction.
        if (Settings.AutoStartRecording)
        {
            Avalonia.Threading.Dispatcher.UIThread.Post(
                () => { _ = StartRecordingCommand.ExecuteAsync(null); },
                Avalonia.Threading.DispatcherPriority.Background);
        }
    }

    // ───────────── Navigation Commands ─────────────

    [RelayCommand]
    private void NavigateToSettings()
    {
        if (CurrentView == Player) Player.StopPlayback();
        CurrentView = Settings;
    }

    [RelayCommand]
    private async Task NavigateToLibrary()
    {
        if (CurrentView == Player) Player.StopPlayback();
        CurrentView = Library;
        await Library.RefreshCommand.ExecuteAsync(null);
    }

    [RelayCommand]
    private async Task NavigateToPlayer()
    {
        CurrentView = Player;
        // Ensure the Player sidebar shows the latest clips even on direct navigation.
        await Library.RefreshCommand.ExecuteAsync(null);
    }

    /// <summary>
    /// Toggles fullscreen playback. Bound from the Player's fullscreen button and the ESC key.
    /// The actual WindowState change is applied in MainWindow code-behind, which observes this flag.
    /// </summary>
    [RelayCommand]
    private void ToggleFullscreen() => IsFullscreen = !IsFullscreen;

    /// <summary>Opens a clip in the built-in editor (view teardown stops any active playback).</summary>
    private void OpenEditor(Lag.Models.ReplayClip clip)
    {
        if (clip.IsImage) return; // screenshots have nothing to trim
        Editor.OpenClip(clip);
        CurrentView = Editor;
    }

    /// <summary>Player → "Edit clip": sends the currently playing clip to the editor.</summary>
    [RelayCommand]
    private void OpenEditorFromPlayer()
    {
        if (Player.CurrentClip is { } clip)
            OpenEditor(clip);
    }

    /// <summary>
    /// Global screenshot hotkey: grabs the configured display into the library folder,
    /// then shows the toast and plays the save sound. Capture runs off the UI thread
    /// (a 1440p GDI blit takes tens of milliseconds).
    /// </summary>
    [RelayCommand]
    private void TakeScreenshot()
    {
        string? deviceName = Settings.SelectedMonitor?.DeviceName;
        string outputDir = Settings.LibraryPath;
        string toastTitle = Localizer.Get("Toast_ScreenshotSaved");

        _ = Task.Run(() =>
        {
            string? path = Lag.Services.ScreenshotService.Capture(deviceName, outputDir);
            if (path != null)
            {
                Lag.Services.ToastService.Show(toastTitle, Path.GetFileName(path));
                PlayScreenshotSound();
            }
        });
    }

    // ───────────── Recording Commands ─────────────

    /// <summary>Starts background recording with current settings.</summary>
    [RelayCommand]
    private async Task StartRecordingAsync()
    {
        if (IsRecording) return;
        
        try
        {
            StatusText = Localizer.Get("Status_Starting");
            int bufferSeconds = Settings.BufferSeconds;
            int fps = Settings.EffectiveFps;

            if (Settings.SelectedMonitor == null)
                throw new InvalidOperationException(Localizer.Get("Status_NoMonitor"));

            uint width = Settings.SelectedMonitor.Width;
            uint height = Settings.SelectedMonitor.Height;
            string? micId = Settings.SelectedMicrophone?.Id;
            string? monitorId = Settings.SelectedMonitor.DeviceName;

            // Output (render/encode) resolution: capture stays native; encode can be downscaled.
            // Aspect ratio is preserved from the native monitor; encoders need even dimensions.
            uint outputWidth = 0, outputHeight = 0;
            int targetHeight = Settings.SelectedResolution.TargetHeight;
            if (targetHeight > 0 && targetHeight < height)
            {
                outputHeight = (uint)targetHeight & ~1u;
                outputWidth = (uint)Math.Round((double)width * outputHeight / height) & ~1u;
            }

            // Full options snapshot for this session (cold-restart re-reads everything).
            var options = new RecorderOptions
            {
                BufferSeconds = bufferSeconds,
                FrameRate = fps,
                Width = width,
                Height = height,
                OutputWidth = outputWidth,
                OutputHeight = outputHeight,
                MicrophoneId = micId,
                MonitorId = monitorId,
                LibraryPath = Settings.LibraryPath,
                FileFormat = Settings.SelectedFormat,
                MicVolume = Settings.MicVolume / 100f,
                // "Auto" has an empty encoder id → null lets the engine run its automatic chain.
                PreferredEncoder = string.IsNullOrEmpty(Settings.SelectedCodec.EncoderId)
                    ? null : Settings.SelectedCodec.EncoderId,
                VideoBitrateKbps = Settings.EffectiveBitrateKbps,
                AdapterIndex = Settings.SelectedGpu.Index < 0 ? 0 : Settings.SelectedGpu.Index,
                SeparateAudioTracks = Settings.SeparateAudioTracks,
                MicForceMono = Settings.MicMono,
                MicStartMuted = Settings.PushToTalkEnabled,
                AudioCaptureMode = Settings.IsAppsMode ? "apps" : "all",
                AudioApps = Settings.AudioApps
                    .Where(a => a.IsEnabled)
                    .Select(a => new AppAudioCapture(a.Exe, a.Volume / 100f))
                    .ToList()
            };

            // Shift heavy native initialization to a background thread to prevent UI freezing/crashing
            await Task.Run(() =>
            {
                _engine.Initialize(options);
                _engine.StartBuffer();
            });
            
            IsRecording = true;
            Console.WriteLine($"[DEBUG] Recording started! IsRecording={IsRecording}");
            StatusText = Localizer.Get("Status_Recording");
            StartBufferTicker(bufferSeconds);
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[DEBUG] StartRecording FAILED: {ex.Message}");
            StatusText = Localizer.Format("Status_ObsError", ex.Message);
        }
    }

    /// <summary>Stops background recording.</summary>
    [RelayCommand]
    private void StopRecording()
    {
        if (!IsRecording) return;
        try
        {
            StatusText = Localizer.Get("Status_Stopping");
            // COLD RESTART: fully tear down the capture pipeline (output → encoders → sources → scene),
            // not just the buffer. The libobs core stays resident, but the next StartRecordingAsync()
            // runs a clean Initialize() that re-reads ALL current settings (monitor, mic, FPS,
            // codec, buffer length). The core is only shut down on app exit (DI → Dispose()).
            _engine.Teardown();
            IsRecording = false;
            StatusText = Localizer.Get("Status_Stopped");
            StopBufferTicker();
            Settings.HasPendingChanges = false;
        }
        catch (Exception ex)
        {
            Console.WriteLine($"[DEBUG] StopRecording FAILED: {ex.Message}");
            // Force UI state to stopped regardless of native error —
            // leaving IsRecording=true after a failed stop would lock out the user.
            IsRecording = false;
            StatusText = Localizer.Format("Status_StopError", ex.Message);
            StopBufferTicker();
            Settings.HasPendingChanges = false;
        }
    }

    /// <summary>Toggles recording state.</summary>
    [RelayCommand]
    private async Task ToggleRecordingAsync()
    {
        if (IsRecording)
            StopRecording();
        else
            await StartRecordingAsync();
    }

    /// <summary>Manually triggers a replay save (same as hotkey).</summary>
    [RelayCommand]
    private void SaveReplay()
    {
        Console.WriteLine($"[DEBUG] SaveReplay() EXECUTING! IsRecording={IsRecording}, IsSaving={IsSaving}");
        
        if (!IsRecording)
        {
            Console.WriteLine("[DEBUG] SaveReplay() BLOCKED: Not recording.");
            StatusText = Localizer.Get("Status_NotRecording");
            return;
        }
        
        if (IsSaving)
        {
            Console.WriteLine("[DEBUG] SaveReplay() BLOCKED: Already saving.");
            return;
        }
        
        IsSaving = true;
        StatusText = Localizer.Get("Status_Saving");
        _ = Task.Run(() =>
        {
            try
            {
                _engine.SaveReplay();
                Console.WriteLine("[DEBUG] Save command sent to OBS. Wait 3-5 seconds for muxing to complete...");
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[DEBUG] SaveReplay() EXCEPTION: {ex.Message}");
                Avalonia.Threading.Dispatcher.UIThread.Post(() =>
                {
                    IsSaving = false;
                    StatusText = Localizer.Format("Status_SaveError", ex.Message);
                });
            }
        });
    }

    /// <summary>Plays the "replay saved" chime (Assets/save.wav).</summary>
    private static void PlaySaveSound() => PlaySound("save.wav");

    /// <summary>Plays the camera-shutter sound for screenshots (Assets/screenshot.wav).</summary>
    private static void PlayScreenshotSound() => PlaySound("screenshot.wav");

    /// <summary>
    /// Plays a feedback sound from the Assets folder next to the executable.
    /// Runs on a background thread with PlaySync so the clip plays in full without blocking the UI,
    /// and the SoundPlayer is disposed only after playback completes (Play() + immediate dispose
    /// would truncate the audio).
    /// </summary>
    private static void PlaySound(string fileName)
    {
        string soundPath = Path.Combine(AppDomain.CurrentDomain.BaseDirectory, "Assets", fileName);
        if (!File.Exists(soundPath)) return;

        _ = Task.Run(() =>
        {
            try
            {
                using var player = new System.Media.SoundPlayer(soundPath);
                player.PlaySync();
            }
            catch (Exception ex)
            {
                Console.WriteLine($"[MainViewModel] Failed to play sound '{fileName}': {ex.Message}");
            }
        });
    }

}
